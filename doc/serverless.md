# A serverless runtime for techne

Status: design, not started. Where it relies on something that exists, it
says where; everything else is new work, and the milestones say which
parts are probes before they are commitments.

## What it is

Cheap serverless compute for Lisp, in the manner of Cloudflare Workers: a
developer deploys a package, it answers HTTP requests in worlds started from
a snapshot, and they pay for the CPU it uses. Three things set it apart:

- **A REPL into what runs, production included.** From Emacs (or Techne, or
  an agent), attach to a production instance, route real requests to it,
  trace and redefine, and when a request fails, get its condition with the
  restarts still live: fix the function, pick `retry`, and the request
  succeeds. As Common Lisp systems have long been debugged, on a serverless
  platform.
- **Hot reload of code and libraries.** A deploy switches new requests to a
  new snapshot while requests in flight finish on the old one; nothing
  restarts. A redeploy takes about a second from the editor.
- **Made for agents as much as for people.** Every operation Emacs uses is
  also a JSON call over HTTPS, errors are data, and running a piece of
  code once in a fresh, limited world is one call. Many users will be
  programs no human watches ("Agents").

techne-vm has much of what this needs: worlds with granted capabilities,
copied from a per-thread template (about 0.2 ms and 0.4 MiB for a world
with the prelude) or from a snapshot of a world with code loaded
(`Vm::snapshot`: 0.43 ms and 430 KiB with the 581-line Org library,
which takes 3.7 ms to load); per-execution break and kill and per-world memory
limits; owned scopes; packages with staged, atomic generations; a REPL,
nREPL server and inspector ([runtime/TECHNE-VM.md](../runtime/TECHNE-VM.md)).
What it lacks is listed under "Runtime work".

The runtime is open source, as the rest of techne, and self-hostable; the
service is running it well. The same handler contract runs in the Techne
editor ("Open source and the editor").

The first deployment is an open-signup alpha on one cheap VPS. Nothing
below builds for several hosts, but nothing should prevent it.

## Non-goals for the alpha

- Running anywhere but Linux on x86-64.
- WebAssembly, other languages, or Workers API compatibility.
- Edge placement, replication, multi-region state.
- Heap persistence: state survives in storage, never in heaps.
- Changing production other than through a deploy, beyond the instance
  a debugging session holds (see "Debugging in production").
- Paid plans: the alpha has hard quotas and measures what a tenant costs.

## Concepts

| Name | What |
|---|---|
| Tenant | An account: users, tokens, quotas. The compromise boundary: a tenant's projects share its worker processes, so code of one project that escapes the VM reaches the tenant's others. Tokens still separate projects and production authority. |
| Project | A tenant's application: routes, bindings, deployments. |
| Deployment | An immutable, content-addressed bundle (the project's sources, pinned libraries, a manifest) and the bindings it was published with. Routes point at one. |
| Snapshot | A world after loading a deployment, from which instances are copied. In memory, per worker thread. |
| Instance | A stateless world serving a deployment's requests. Disposable. |
| Durable world | A named, long-lived world of a project with storage of its own; one per name. After the alpha's first milestones. |
| Dev world | A world for a developer's REPL session: a deployment's code and chosen bindings, serving only its own preview URL. |
| Debug instance | A production instance held by a REPL session: serves the requests routed to it, can be changed, and is retired when the session ends. |
| Run | One evaluation of code in a fresh world, for an agent or a test: code and input in, result or condition out ("Runs"). |
| Binding | A capability handed to a deployment by name: a KV namespace, a secret, allowed fetch destinations, later a durable world class. A project's bindings are *preview* or *production*; a world holds production bindings only with production authority ("Authority"). |

## Programming model

A deployment's manifest names an entry module (a file); the module
provides its handler:

```scheme
(provide fetch)

(define (fetch request env ctx)
  "Answer REQUEST, using the bindings in ENV."
  (match (request-path request)
    ("/" (response "Hello, World!"))
    ("/weather"
     ;; A cache: a lost update costs one more fetch, nothing else.
     (let ([cached (kv-get (env 'cache) "weather")])
       (or (and cached (json-response cached))
           (let ([w (fetch-json (env 'weather-api) "/today")])
             (kv-put! (env 'cache) "weather" w #:ttl 600)
             (json-response w)))))
    (_ (response "not found" #:status 404))))
```

- `request`: a record (method, URL, headers, body, client metadata).
  `response`: status, headers, body. Bodies are strings or bytevectors at
  first, bounded in size; streaming comes after async body I/O exists
  (runtime work 4).
- `env`: the deployment's bindings and nothing else. An instance's world
  has no files, environment, processes, host control or sockets
  (`Vm::with_grants`; capabilities in `crates/techne-vm/src/vm.rs`); its
  only way out is the supervisor, through its bindings.
- `ctx`: the request's id, its deadline, and `wait-until` (below).
- Concurrency inside an instance is techne's tasks. A request runs as a
  task; waits on fetch, KV and timers suspend only it. Every task a request
  spawns, directly or not, belongs to the request: it counts against its
  budgets and is killed when the request ends (runtime work 2). A request
  ends when its response is sent and its `wait-until` work is done, or at
  its deadline.
- **Initialization has no external effects.** The entry module and what it
  requires are read by the host from the bundle and run once, to make the
  snapshot, with no bindings and no clocks or randomness beyond a fixed
  seed. It may define, compile and compute constants. If it leaves
  anything a snapshot cannot hold (a task, a channel, a foreign object) the
  deployment is rejected with an ordinary error (runtime work 1).
- Globals an instance sets survive to later requests on that instance and
  no further. Which requests share an instance is not promised (below).
- Scheduled handlers (cron) and queues come later, with their delivery and
  retry semantics.

## Worlds

### Instances: one API, a policy underneath

The API does not say whether requests share a world; the host decides per
project and may change it without breaking a correct program:

- **reuse** (the alpha's): an instance serves up to *k* requests at once
  and is recycled after *n* requests, *t* seconds idle, or a deploy. Caches
  warm, globals persist for a while: Workers' model.
- **fresh** (later, when someone needs it): each request gets a world
  copied from the snapshot and dropped after. Strict isolation between
  requests.

### Failures

Other requests on an instance share its heap and storage, so "unaffected"
below means their code keeps running, not that what a failed request
changed is undone.

| What happens | The request | Its instance | Other requests on it |
|---|---|---|---|
| Uncaught condition | 500, error record | kept | unaffected |
| CPU or wall budget past, break caught or not | break, then kill: 500, error record | retired (a kill runs no cleanup, so shared state may be half-changed) | finish, then the instance goes |
| World over its memory limit (a world limit, shared by its requests) | refused, then killed | retired | finish if they can, then it goes |
| Native panic (`Vm::poisoned`) | 500 | dropped at once | 503 |
| Kill does not take effect (a long Rust native) | supervisor kills the worker process | gone | 503; the worker restarts |
| Deploy | finishes on its snapshot | drained, dropped | finish |
| Waiting in a debugger, past its deadline, or detached | fails (504) | the debug instance is retired | finish |

### Durable worlds (after open signup)

A durable world is addressed by a class (a binding) and a name; one world
runs per name, its heap a cache, its state in an SQLite database of its
own. For the alpha the contract is the simplest that is safe: **calls run
one at a time, each to completion** (tasks inside a call may interleave
with each other, not with another call); a call to the same world from
within a call fails rather than deadlocks; a deploy drains the world and
starts it again from the new snapshot. Interleaving calls at I/O waits,
Cloudflare-style input and output gates, and in-place reload are later
choices, made when a workload needs them.

### Dev worlds

A dev world is made for a REPL session from a deployment's snapshot. Its
bindings are chosen when it is made: preview bindings by default, or the
production ones, read-only or read-write, with production authority. It
answers
requests only on its own preview URL, lives while its session does (up to
a limit), and is metered as the tenant's.

Inside a dev world, hot reload is techne's own: `load-package` stages and
publishes a package's next generation in place, keeping the heap.
(`load-package` shuts the previous generation's scope at once,
`crates/techne-vm/src/prelude.scm`, cancelling its tasks; that is fine in a
REPL, not for production, which switches snapshots instead.)

## Deploying

A deployment goes through these states, each recorded:

1. **Uploaded**: the CLI or Emacs sends changed sources; the control plane
   makes the content-addressed bundle (sources, manifest, libraries pinned
   by hash).
2. **Built**: a sandboxed worker initializes it with bounded CPU, memory
   and size. A reader, macro, compile or initialization error rejects it
   with its condition, and nothing else changes.
3. **Ready**: each worker that serves the project has made its snapshot.
4. **Published**: the route points at it, atomically, if it still points
   at the deployment the publisher saw (compare and publish: a retried or
   late publish cannot undo a newer one). Each request is pinned at
   admission to one deployment and its bindings.
5. **Draining**, for the previous one: its instances finish their requests
   (up to the wall limit) and are dropped.

Rollback publishes an earlier deployment that is still Ready or can be
again. **It rolls back code only**: storage, schema changes and external
effects stay. Storage changes must stay compatible with the previous
deployment, or be undone by the developer.

Libraries are packages pinned by hash; updating one is a new deployment of
each project that uses it. The aim is a second from `C-c C-k` to serving:
compiling a thousand lines takes a few milliseconds (about 18 K
instructions a line today); a snapshot of the loaded deployment takes
half a millisecond for the Org library, more for larger projects
(runtime work 1).

## The REPL

### What it reaches

- **Dev worlds**: everything: evaluate, load, redefine, the debugger with
  live restarts (an uncaught condition in a preview request can break into
  the session for up to a minute), the inspector, completion, xref.
- **Production**, through a debug instance (next section), with
  production authority.
- **Durable worlds**, once they exist: in place (there is only one of
  each), with production authority, audited, and started again from
  their deployment when the session ends. Their storage keeps what the session wrote.
- Without evaluating, with `inspect`: error records, logs, metrics,
  instances and the requests they are running.

### Debugging in production

A session attaches to a deployment in production and gets a **debug
instance**: a world made from the same snapshot, with the production
bindings, in the same worker as the serving instances. Then:

- **Routing**: the requests sent to it are those carrying a routing
  credential the session issues (a short-lived, signed value in a header
  or cookie, for the developer reproducing a problem), and, if the
  session asks and the decision below allows it, a bounded share of real
  traffic or the requests matching a path or header filter. Nothing is
  routed to it by default. Changing routes needs production authority;
  the credential is not the session's id, which responses show.
  Outstanding requests on a debug instance are bounded.
- **What one can do**: everything one can in a dev world: evaluate,
  redefine, `trace` a procedure (its calls and results go to the session),
  inspect the heap, set *break on error*.
- **Break on error**: a request routed to the debug instance that raises
  an uncaught condition waits in its handler, and the session gets the
  debugger with the live restarts: the handler's own and those the host
  establishes around every request (`restart-case`, runtime/TECHNE-VM.md,
  "Restarts"). Several requests may wait at once, each with its own
  debugger state; the others on the instance go on. The developer
  inspects, redefines, and invokes a restart:
  - `retry` calls the deployment's entry point again, looked up afresh so
    a redefinition takes effect, as the same request: same id, same
    input (bodies are kept while a request may be retried), its CPU and
    fetch budgets cumulative across attempts. What the failed attempt
    wrote or sent is not undone, and the retry may do it again; tasks it
    spawned are killed first.
  - `use-value` answers with a value, which must be a response;
  - `abort` fails the request.
- **Waiting is bounded**: a request is *running*, *paused* (in the
  debugger) or *ended*. While paused it uses no CPU, and its other tasks
  go on within its budgets. The supervisor gives it an absolute deadline
  when it pauses (its wall limit plus a minute, at most); at that
  deadline, or when the session detaches, its token expires, it
  disconnects, or a deploy drains the instance, the request fails with
  504. The caller may give up before a fixed response exists. Hard limits
  (memory, the watchdog) apply while paused as at any time.
  Evaluations in the session are metered to the session, not the request.
- **Marked**: responses from a debug instance carry
  `techne-debug: <correlation id>` and `Cache-Control: private, no-store`,
  so no shared cache keeps them. Attaching, routing changes, restart
  choices and every evaluation are in the audit log with user, time and
  instance (the forms themselves as "Audit" says).
- **Retired**: when the session ends, is idle too long, or a deploy comes,
  the debug instance stops taking requests, finishes those it has, and is
  dropped. It never returns to the pool.

So production *code* differs from its deployment only in the instance a
session holds, while it holds it, and only for the requests routed there.
Its *effects* are real: what the session or a routed request wrote to
storage or sent elsewhere stays done, and other requests see it, since a
debug instance has the production bindings. Routing a share of real
traffic to it serves real users from changed code; that is the
developer's choice to make, and the default is not to. A fix found there
reaches everyone the usual way: save the buffer, deploy.

Not all of this runs today: an exception handler can wait in its task,
with the live restarts (runtime/TECHNE-VM.md, "Restarts"), and the
nREPL server's evaluations do, each in a task of its own, several
waiting at once while others run; debug instances, routing and `trace`
are still to do (runtime work 7).

Dev worlds remain the place for experiments with preview storage; a
debug instance is for the problem that only shows in production.

### Operations, in two encodings

Everything a client does, a person in Emacs or an agent, is an
**operation**: a message with an `op` and its fields, answered by one or
more replies, the last with the status `done`. This is nREPL's model, and
the nREPL server (`crates/techne-node/src/nrepl.rs`) already works this
way: its messages are values of one small type (integers, byte strings,
lists, dictionaries: `crates/techne-node/src/bencode.rs`), encoded only
when they reach the socket.

- **The operations**: today's (`clone`, `close`, `eval`, `load-file`,
  `interrupt`, `completions`, `info`, `eldoc`, `describe`, the inspector's
  and the debugger's) and the service's (`deploy`, `deployments`,
  `rollback`, `logs`, `errors`, `attach`, `route`, `trace`, `run`). Each is
  specified once: fields, replies, the token scope it needs.
- **Two encodings of the same messages**:
  - **bencode over a TLS stream**, for interactive sessions: replies
    arrive as they happen (output while an evaluation runs, a trace, a
    debugger prompt), an interrupt can be sent while one waits. This is
    what `techne.el` speaks today, and what other nREPL clients (CIDER,
    Conjure, Calva) speak. `techne.el` connects with
    `open-network-stream :type 'tls` instead of `make-network-process`,
    and verifies the certificate.
  - **JSON over HTTPS**, for calls: `POST /v1/ops` with the message as a
    JSON object. Operations that end on their own (`eval`, `run`,
    `deploy`, `completions`) answer with the JSON array of their replies,
    or, with `Accept: application/x-ndjson`, the replies one per line as
    they come; operations that do not end (`events`, `logs` followed)
    must be asked for as lines, and are refused otherwise before they
    start. This is what agents, the CLI and the MCP server use; `curl` is
    enough.
- **The messages** have a schema per operation, the same for both
  encodings: each field's type (string, integer within ±2⁵³, boolean,
  list, dictionary, bytes), checked before an operation is dispatched;
  duplicate and unknown fields refused; ids are strings. Bencode has no
  booleans or null: booleans are the integers 0 and 1 there, absent fields
  stand for null. Byte fields (a bundle's file) are named as such and
  carried as base64 in JSON. Values a client may make the server allocate
  from (a line or column, a count) are bounded too.
- **Sessions** outlive connections over HTTPS: a session is a lease,
  renewed by use and ended by `close` or idleness, not by a connection
  closing (over the stream, closing it closes its sessions, as today).
  Events a session did not ask for in a reply (a debugger break of a
  routed request, a trace, its output past an evaluation) go to its
  `events` stream, kept for a while and bounded; a client reconnecting
  resumes it from the last event it saw. An interrupt or a restart choice
  is another call naming the session; a debugger break is an event saying
  which restarts wait. Operations of one session run in the order
  received; calls racing on one session from two connections have no
  order but that.
- **Retries**: an operation may carry an idempotency key; the server keeps
  each key's outcome for a day per principal, refuses its reuse with
  another message, and answers a repeat with the kept outcome instead of
  running it again. Without a key, a retried `eval` or `run` runs again,
  with its effects. A restart choice is consumed once.
- **Replies say what happened**: an operation accepted is not an
  operation done, so replies distinguish *accepted*, *done* and *failed*,
  and failures are typed (refused by authority or admission, interrupted,
  past a deadline, killed, a condition in the code: "Errors are data" in
  "Agents").
- **Operations never depend on their transport.** Authentication, scopes,
  limits and audit are checked where an operation is handled, not in an
  encoding; the operations' tests run over both encodings, and so do
  tests of what differs (streaming, leases, reconnecting, refusals before
  dispatch). Adding an operation adds it to both.
- **Authentication**: over HTTPS, `Authorization: Bearer <token>`; over
  the stream, an `authenticate` operation before any other. A session is
  made (`clone`) for a target: a dev world, a debug instance, a durable
  world.
- If nobody but `techne.el` ends up using bencode, it can be dropped and
  `techne.el` moved to JSON over a WebSocket; until then it costs a parser
  and keeps the nREPL clients that exist.

The nREPL server is for a trusted local user today; before it faces the
internet:

- **Tokens**: see "Authority" below.
- **Sessions, debugger states and inspector handles belong to their
  principal and target**; today sessions are global and found by id
  (`nrepl.rs`, `handle`), which must change.
- **Inspecting does not evaluate.** Today's `techne-inspect` evaluates its
  argument (`eval_in`) and renders values by calling Scheme; under the
  `inspect` scope only data the host captured (error records) is shown,
  rendered by Rust. The evaluating inspector needs `eval`.
- **Bounded**, in both encodings: message size, parser depth, sessions
  per user, output queued per session, unauthenticated connections.
- **Audit**: every operation that changes or evaluates (sessions,
  evaluations, routes, restart choices, deploys, token changes) is logged
  with principal, target, deployment, request and time, and its outcome.
  The forms evaluated and the values traced are logged only if the project
  says so (off by default: they may hold secrets), and are then kept,
  shown and deleted under the rules of error records.

### Authority

A token belongs to a user, or to an agent a user made it for. It names:

- **projects** it may act on, never another tenant's;
- **scopes**: `inspect` (logs, error records, metrics, instances; never
  evaluates), `eval` (dev worlds and runs with preview bindings), `prod`
  (anything holding production bindings: dev worlds bound to them, debug
  instances, durable worlds, runs given them), `deploy` (deploy, publish,
  roll back);
- an **expiry** and **quotas** (CPU, requests, runs, sessions).

What a world may hold decides the scope it needs, not what kind of world
it is: an operation asking for production bindings, in any world, needs
`prod`. Bindings are named from the project's own; a request cannot
describe a binding of its own making. A token made from another (for an
agent) has at most its maker's projects, scopes and expiry and a share of
its remaining quota, never more; revoking a token revokes those made from
it.

### Error records

A request ending in an uncaught condition produces an error record: the
condition's message and irritants, the backtrace (procedure and source
location per frame, as `raise_backtrace` gives today), the deployment, the
request's method and path. Captured by Rust, bounded, without calling
Scheme. Not captured by default: query strings, headers, bodies, local
values. Records are kept per project for a few days within a quota.

Frame locals in records are the obvious next step, and runtime work of its
own (runtime work 5); a project turns them on knowing they may hold
secrets.

## Isolation and trust

Open signup means hostile code from day one. In-process worlds isolate
faults and resources between code that trusts each other
([PLAN.md](../PLAN.md), "Worlds, packages and the language"); they are no
boundary against an attacker who finds a bug in the reader, the JIT or a
native's `unsafe`. So the design assumes a tenant's worker process is
compromised, and keeps everyone else safe anyway.

### Workers

- **A worker process per tenant and runtime version**, never shared with
  another tenant. A worker runs threads, each with its own VM worlds,
  cage and snapshots, for any of the tenant's projects.
- **Started by exec** from the supervisor, sandboxed before it reads any
  tenant code: user, network, mount and PID namespaces; `no_new_privs`, no
  capabilities, inherited descriptors closed but one Unix socket to the
  supervisor; landlock with nothing writable and only the binary's files
  readable; a seccomp filter allowing what the VM needs (memory, futexes,
  `mmap` and `mprotect` for the JIT's code, the socket) and nothing else;
  in a cgroup bounding memory, CPU and PIDs, applied before it starts.
- A zygote that forks pre-booted workers would cut cold starts further,
  but VM boot starts the JIT's thread and the allocator and cage hold
  process-wide state; it waits for a probe showing fork is safe and a
  measurement showing exec is too slow.
- A tenant without traffic has no worker. Idle workers stop after some
  minutes; the number of live workers is bounded, the least recently used
  stopped first.
- **Runtime versions**: a deployment records the runtime it was built
  with; workers run that binary, so a tenant whose projects are on two
  versions has two workers. An upgrade rolls out per project; a
  security fix is forced on all. Running a tenant interpreter-only, if a
  JIT bug is found, is a worker restart with `TECHNE_JIT=0`.

### The supervisor as broker

Everything a worker does outside itself is a request to the supervisor,
which trusts nothing the worker says:

- Authority comes from which socket a request arrives on, which says the
  tenant, and from handles the supervisor issued: when it admits a
  request, run or session it gives the worker a handle naming its
  deployment, bindings and budgets; the worker's requests to the broker
  name that handle and a binding by the deployment's name for it, never a
  project, path or database. A handle the worker was not given, or one
  that ended, is refused. So a compromised worker reaches what its
  tenant's live handles reach, and no more.
- Requests and replies have a fixed schema, bounded size and depth, and
  bounded numbers in flight per worker; past those the worker is killed.
- KV and storage are served by the supervisor, confined to the tenant's
  namespaces; a worker never names a file or writes SQL against a shared
  database.
- Meters (CPU, requests, egress) are the supervisor's and the cgroup's,
  not reports from the worker. The cgroup enforces the tenant's CPU and
  memory as a whole; which of its requests used the CPU is the worker's
  account, cooperative and not independently checked.
- Tests include a hostile worker: a client that sends malformed, oversized
  and mismatched requests over the socket, not only Lisp code testing its
  bindings.

### Fetch

`fetch-url` goes through the supervisor, which:

- allows HTTP and HTTPS on ports 80 and 443 (and those a binding names);
- resolves the name itself, checks every address, and connects to the
  address it checked (no second lookup, so no DNS rebinding);
- refuses loopback, private, link-local, multicast, unspecified, IPv4-mapped
  and other special-use addresses (IPv4 and IPv6), the host's own
  addresses, and the VPS provider's metadata service;
- verifies TLS certificates for the host name asked for;
- follows redirects itself, at most a few, checking each target as a new
  request and dropping credentials when the origin changes;
- bounds request and response sizes, time and the number per request, and
  counts egress per tenant.

Each rule has an adversarial test.

### Side channels and residual risk

A process per tenant keeps one tenant from reading another's memory
through a VM bug. It does not remove shared-kernel or shared-CPU risks on a
VPS: speculative execution across processes, cache timing, a kernel bug
reachable through the allowed system calls. The alpha accepts these, says
so to users, and keeps the kernel patched. Clocks visible to tenant code
(`current-jiffy`, microseconds today; `current-second`; anything the
supervisor returns) are coarsened to a millisecond in workers.

### Abuse and capacity

Open signup on one small host needs limits on the whole as well as per
tenant:

- verified email, signup and deploy rate limits, a kill switch per tenant;
- no outbound SMTP or raw sockets; egress capped per tenant;
- global admission: a bound on live workers, queued requests, concurrent
  requests per tenant, builds at once, open REPL sessions; past it work is
  refused with 503, not queued without bound;
- quotas on bundles, logs, error records and storage, enforced on write;
- the supervisor and control plane keep a reserve of memory and CPU
  outside the tenants' cgroups.

## Limits

| Limit | Alpha default | Enforced by |
|---|---|---|
| CPU per request, its tasks included | 50 ms | VM break then kill; supervisor kills the worker if that fails |
| Wall time per request, `wait-until` included | 30 s | supervisor deadline |
| Tasks per request | 1,000 | VM |
| Memory per world (shared by its requests) | 128 MiB | world memory limit (`Vm::memory_limit`) |
| Memory per worker | 512 MiB | cgroup |
| Fetches per request | 50 | supervisor |
| Request and response body | 10 MiB, not streamed | supervisor |
| Requests per day per tenant | 100 K | front |
| Runs: CPU, wall time, output, result | 50 ms, 10 s, 1 MiB, 1 MiB | as requests |
| Runs and sessions at once per token | 10 runs, 4 sessions | supervisor |

The VM's limits are cooperative: interpreted and compiled code check
often, but a long Rust native does not (runtime/TECHNE-VM.md, "Stops"), and
the scheduler checks its budget between tasks. Natives reachable by tenant
code must bound their work (most are admitted against the memory limit
already); what escapes is caught by the supervisor's watchdog, which kills
the worker. CPU per request is the thread's CPU time over the slices its
tasks run; compiling on the JIT's thread is charged to the tenant, not to
the request, and bounded by the tenant's cgroup. Runs, evaluations in
sessions and debug instances count against the same tenant and global
admission as requests.

## Architecture on one VPS

```text
          internet
             |
   supervisor: front (TLS, routes, admission), operations endpoint,
               broker (fetch, KV, storage), control plane
             |  one Unix socket per worker
   worker   worker   ...   (sandboxed, a tenant each; worlds on threads)

   disk: control plane SQLite, bundles by hash, KV and storage SQLite
```

- **Front**: rustls and hyper. Maps a host name (`project.tenant.example`;
  custom domains later) to a project, and admits or refuses.
- **Control plane**: accounts, tokens, projects, deployments, routes, in
  one SQLite database; bundles as files named by hash, read by the
  supervisor and passed to workers over their socket.
- **Storage**: a KV namespace is a table in an SQLite file per tenant.
  Backups use SQLite's online backup API, and restoring one is tested.

Several hosts later means a router in front, tenants placed on hosts, a
directory placing durable worlds, replicated KV. What assumes one host here
is the storage layout and that the supervisor can start any worker locally.

## Developer tooling

- **CLI** (`techne cloud`): `login`, `init`, `dev`, `deploy`, `rollback`,
  `tail` (logs and error records), `repl`.
- **`techne cloud dev`** runs the same supervisor and worker locally,
  without the sandbox, so the REPL, error records and limits are developed
  and tested without the service.
- **Emacs** (`editors/emacs/techne.el`): connect with a token over TLS;
  choose a dev world or attach a debug instance, shown in the mode line;
  route one's own requests to it, set break on error, trace; deploy the
  saved buffers; tail logs; list error records and open one in a read-only
  inspector. Its evaluation, completion, eldoc, xref, debugger and
  inspector work as they do today.
- **Techne** gets the same through its own client when it has one.
- All of these are clients of the same operations; nothing they do is
  closed to an agent.

## Agents

Many users will be agents: programs that write code, deploy it, call it
and read the results, with no human looking at any of it. They need what
people need, made explicit:

- **The operations, in JSON over HTTPS** ("Operations, in two
  encodings"): the same ones Emacs uses, so an agent can do anything a
  person can. An MCP server offers each operation as a tool; it is a
  third client, not a second model.
- **Runs**: code execution for agents (what sandbox services sell with
  containers that start in a hundred milliseconds or more), and how an
  agent tries an expression before deploying it. Below.
- **Sessions**: a dev world kept between calls (`clone`, then `eval` in
  it), for an agent working the REPL way: evaluate, look, redefine.
- **Errors are data**: a failure comes back typed (refused, interrupted,
  past a deadline, killed, a condition), and a condition as its kind,
  message, irritants (written briefly, by Rust, never by calling the
  program's printers), source location (file, line, column), backtrace,
  and, at best effort, a hint (an unbound name close to a bound one, a
  wrong number of arguments with the procedure's signature). Today nREPL
  sends rendered text (runtime work 8). The docstrings
  checked by `crates/techne-editor/tests/documentation.rs` are served by
  `info`, so an agent can look up what it is calling.
- **Idempotent and checkable**: deploying the same bundle twice is one
  deployment (bundles are content-addressed); `deploy` with `dry-run`
  builds and reports without publishing.
- **Delegated tokens** ("Authority"): a person's account mints tokens for
  its agents, each with at most its maker's authority and a share of its
  quota, so an agent's mistakes cost what its owner allowed. Agents do
  not sign up by themselves.
- **Documentation an agent can read in one go**: a language and API
  reference sized for a model's context (`llms.txt` and one Markdown
  file), generated from the docstrings, with worked examples.

Models write JavaScript and Python more fluently than a Scheme they have
not seen. The language stays close to R7RS and to the forms Clojure and
Racket made familiar, and error messages say how to fix what they report;
how agents fare is measured (a benchmark of tasks agents carry out
against the API) rather than assumed.

### Runs

`run` takes:

- `code`: source text, evaluated in a fresh world as one module;
- `input` (optional): JSON, bound to `input` as the value `string->json`
  makes of it (objects as dicts with string keys, arrays as vectors);
- `libraries` (optional): libraries of the project to load first, from
  a deployment;
- `bindings` (optional, none by default): the project's, by name, as in
  "Authority";
- `limits` (optional): lower than the token's.

The world is started from the runtime's template, granted nothing, or
from a deployment's snapshot when libraries are asked for. The reply is
one of:

- `value`: the last form's value as JSON, if it is representable (what
  `json->string` writes); otherwise its written form as a string, marked
  so;
- a typed failure, as above.

with the output the code printed (bounded; cut short and marked past the
limit), and the CPU, wall time and fetches it used. When the run ends,
every task it spawned is killed and its world dropped. A run is never
retried by the server; with an idempotency key, a repeat gets the first
outcome. Runs count against the token's quota and the tenant's and global
admission, each counted at least at a fixed minimum, since starting and
dropping a world is not free; the overhead is measured in milestone 2, not
promised here.

## Open source and the editor

The runtime is open source, dual-licensed as the rest of techne, in three
parts:

- **The handler host** (a crate): the handler contract, snapshots,
  instances and their policy, request lifetimes and limits, debug
  instances. No sandbox, no network of its own: it is given requests and a
  broker. The worker embeds it; so does the Techne editor, which can serve
  a project from a world of its own runtime, with the editor's REPL,
  inspector and debugger on it, as `techne cloud dev` does. That world is
  apart from the editor's own (`Vm::new`, which holds files, processes
  and the editor): a world granted only what the project's bindings give,
  as on the service.
- **The supervisor**: front, broker, sandbox, control plane. Open source
  too, so anyone can host their own on a VPS.
- **Clients**: the CLI, `techne.el`, the MCP server.

The service is running these well: operations, capacity, abuse handling,
backups, upgrades. Keeping the supervisor open means its security is
reviewed in the open as well.

## Libraries

Rust-backed:

- request and response records, headers, URL parsing and encoding;
- `fetch-url` and `fetch-json` (bindings, through the supervisor);
- base64, hex, digests (SHA-2), HMAC, random bytes, UUIDs;
- time: instants, durations, RFC 3339, at millisecond resolution;
- JSON (exists: `string->json`, `json->string`);
- KV with TTLs (bindings);
- later: streamed bodies, durable storage, WebSockets on durable worlds,
  scheduled handlers, queues.

Language work the example projects are expected to want, done when they
do: iteration (`for`, PLAN.md step 13), string interpolation, destructuring
dicts in `match` and `let`.

## Runtime work

What techne-vm and techne-node need that they do not have:

1. **Deployment snapshots** (started). `Vm::snapshot` makes a world at
   rest a template that `Snapshot::world` starts worlds from, refusing
   with an error what it cannot carry (unfinished tasks, timers,
   channels, foreign objects, values rooted from Rust); snapshots are
   bound to their thread, as worlds are. With the Org library loaded a
   world starts in 0.43 ms (`examples/snapshot.rs`). Still needed: a
   loader confined to a bundle's contents, given by the host; an
   initialization phase without clocks or randomness; measurements of a
   larger project and of the cost of the first requests before the JIT
   has compiled anything.
2. **Request lifetimes.** An accounting identity the host gives a request's
   execution and that every task it spawns inherits, whatever scope they
   are in: CPU summed across them, a task count, and all killed when the
   request ends. Spawned tasks today get executions of their own, linked to
   the parent only with `#:child` and only for kills
   (`crates/techne-vm/src/tasks.rs`).
3. **Hard stops.** A watchdog in the worker that tells the supervisor when
   a stop is not delivered in time; an audit of natives tenant code can
   reach, bounding their work.
4. **Async body I/O.** Ports are synchronous today (`ports.rs`), and a
   native calling back into Scheme cannot suspend (PLAN.md step 8.5).
   Streamed bodies need reads and writes that suspend their task, or a
   bounded chunk API, with backpressure, cancellation on disconnect and
   errors after headers are sent defined.
5. **Structured backtraces.** `raise_backtrace` gives strings; frame locals
   need capture before unwinding, values the JIT keeps only in registers,
   bounded copying of cyclic values, all without calling Scheme.
6. **Operations for the internet**: the JSON encoding and its HTTP
   endpoint, authentication, sessions owned by their principal, a
   non-evaluating inspector, bounds; as listed under "Operations, in two
   encodings".
7. **Debugging requests** (started). Exception handlers are frames of
   the raising task, so a request's task can wait in its handler while
   the worker serves others, then invoke a live restart
   (`tests/suites/lang/handlers.scm`); `eval` and `eval-source` compile
   a form at a time and call it from Scheme, so what they evaluate can
   wait too. The nREPL server runs each evaluation in a task of its own,
   whose debugger waits on a channel for the client's choice, several at
   once (`nrepl.rs`). Needed: `trace`; a routing hook in the host.
8. **Conditions as data.** A bounded export of a condition by Rust: its
   kind, message, irritants written briefly, source sites and frames, and
   a stable code for each kind of failure, without calling Scheme (nREPL
   reports conditions by calling `condition/report-string` today).

## Milestones

Each ends with something usable and an acceptance test.

1. **Probes.** Deployment snapshots (runtime work 1) of a realistic
   project, measured; handlers that can wait (7), since production
   debugging depends on them; request lifetimes (2) with kill escalation;
   a sandboxed worker started by exec, measured.
   *Acceptance:* a snapshot refuses a world that leaves a task behind;
   two tasks wait in their exception handlers while a third finishes,
   and one, after a redefinition, invokes its restart and succeeds; a
   request spawning tasks that loop is stopped at its CPU budget with all
   its tasks, and a stuck native at the watchdog; cold start and snapshot
   costs are written down.
2. **Local vertical slice.** `techne cloud dev`: supervisor and one worker,
   the handler contract with whole bodies, reuse policy, fetch through the
   broker, KV, the operations in both encodings with runs and sessions, a
   dev world on the REPL from Emacs over TLS with a token; `deploy`
   publishes at once, without the states of milestone 4.
   *Acceptance:* the example project runs; redefining its handler from
   Emacs changes the next preview response; an agent given only the
   operations over HTTPS and `llms.txt` writes, tries and deploys a small
   service; a request looping for ever is stopped and the next one is
   served; requests per second and the overhead of a run measured.
3. **Hosted isolation.** The sandbox, broker contract, fetch policy,
   accounts and tokens, admission and quotas, the TLS front, on the VPS.
   *Acceptance:* the hostile-worker and fetch tests pass; a tenant cannot
   reach files, the network but through its bindings, the metadata service
   or another tenant; a review of the sandbox and the REPL's
   authentication before signup opens.
4. **Deploy loop.** Deployment states, publish, drain, rollback, log tail,
   error records in Emacs.
   *Acceptance:* a redeploy under load drops no request and serves no
   request from a mix of deployments; a failing build changes nothing; an
   error in a handler opens in Emacs with its backtrace.
5. **Debugging in production**: debug instances, routing by header and
   share, break on error with live restarts, trace, the audit log; in Emacs
   and in the API. It is the alpha's headline: **open signup here.**
   *Acceptance:* a handler failing only on production data breaks into
   Emacs, is fixed by redefinition, and the waiting request succeeds on
   `retry`; detaching retires the instance; requests not routed to it run
   the deployment's code; a routing credential from another session or
   past its expiry routes nothing.
6. **Durable worlds**, with the alpha contract above.
   *Acceptance:* a durable counter stays exact under concurrent calls,
   across eviction and across a deploy.
7. Then, as alpha users ask: streaming, frame locals in error records,
   scheduled handlers, the fresh policy, a zygote, custom domains, paid
   plans, several hosts.

## Questions

- Should a debug instance be able to take a share of real traffic, or
  only requests the developer marks? A share finds problems one cannot
  reproduce; it also serves users from changed code. Milestone 5 can ship
  with marked requests only and add the share later.
- Which applications should the alpha be good for? Workers' common ones
  (JSON APIs, webhooks, bots, small sites) and agents' code execution are
  the assumption; they decide egress rules, body sizes, and whether
  streaming, cron or durable worlds come before open signup.
- Are runs (code execution for agents) a product of their own, priced and
  limited apart from deployed handlers?
- What does one tenant cost (CPU, memory, disk, egress) on the chosen VPS?
  Milestone 2 measures it; it decides the free limits and later prices.
- Who reviews the sandbox and REPL authentication before signup opens?
  The proposal: hostile tests in CI, an adversarial review by several
  models against the code, the supervisor public for anyone to read; an
  outside audit before paid plans.
- A name for the service.
- Should the REPL protocol stay close enough to standard nREPL for CIDER,
  Conjure or Calva to evaluate in a dev world?
