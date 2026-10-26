# Remote access

Status: the first slice is built, the rest is design. Built: locations
in TRAMP's syntax, methods (ssh, sudo, doas, podman, docker) defined in
Lisp, endpoints shared by route and connected again once lost, and hops
started by the endpoint before them (`lisp/editor/remote.scm`); nodes
reporting their facts, and their file operations (`crates/techne-node`,
[node-protocol.md](node-protocol.md)); documents of remote files,
opened, saved against their revision and journaled here
(`techne-text`'s `Place::Remote`); `find-file`, the shell commands,
`project-root` and `vc-status` on remote locations. Tested over a pipe
in CI (`crates/techne-cli/tests/remote.rs`), and over ssh to a real
machine when `TECHNE_REMOTE_TEST` names one. Not yet: the protocol's
second version as specified below (its envelope, features, pushed
streams, structured errors and cancellation: version 2 so far adds the
facts and file operations to the first), the shared engine with
tramp-rpc, askpass (ssh and sudo must not ask), deploying the node,
services on endpoints, remote search and dired, documents recognised
across routes, and narrowed endpoint capabilities. It implements REQUIREMENTS.md,
"Remote operation is part of the runtime" and "Remote state: ownership,
not a shared heap", and PLAN.md, "Remote execution without
distributed-memory semantics". It draws on
[emacs-tramp-rpc](https://github.com/ArthurHeymans/emacs-tramp-rpc)
(`../tramp-rpc`), whose server becomes part of the shared engine
described here.

Remote access is a core component: files, processes, terminals, search,
version control, language servers and agents reach other machines through
one mechanism, and no application invents its own. The editor's code must
not assume a local machine; a test mode keeps it honest ("Order of work").

## Two ways to reach a machine

- **Primitive operations.** Local Lisp reads, writes, lists and watches
  files and runs processes on another machine, as TRAMP does. Every
  endpoint offers these.
- **Lisp beside the data.** A service runs in a world on the other
  machine, and local code calls it: a search of a project, a version
  control refresh, an index. One call replaces the dozens of round trips
  the same work costs in primitives. Endpoints running the full Techne
  runtime offer this.

Both use the same locations, routes and protocol. A package wanting the
second checks the endpoint's features and, without them, does the same
work in primitives or raises an explicit "unsupported" condition; it
never falls back silently.

What TRAMP and tramp-rpc teach:

- **Keep:** one integration point, so that every feature becomes remote
  at once; the file name syntax; methods and hops; reusing OpenSSH's
  configuration and authentication; sharing a connection between all
  uses of a route; tramp-rpc's one round trip per operation, batches,
  pushed process output, file system watches that invalidate caches,
  connection generations, and C-g abandoning a wait without killing the
  connection.
- **Leave:** file names as strings dispatched on every call, where a
  predicate can authenticate, deploy and wait on the network; an ambient
  `default-directory` that runs VC locally for a remote file, or finds a
  local executable and runs it remotely; local relay processes pretending
  to be remote children; matching password prompts in a stream that also
  carries data; proxies hidden in a table rather than in the name; caches
  guessed by time to live. And server methods added one editor feature at
  a time (`highlevel.*`, `ancestors.scan`, Magit's prefetch, which
  rebuilds the commands Magit will run): that is Lisp beside the data,
  reinvented per application.

## Locations

A location is a value: a route and a path on the endpoint the route
reaches. It is read and printed in TRAMP's syntax, which is its only
string form:

```
/home/arthur/src/techne/README.md                 ; local: the empty route
/ssh:build:/src/coreboot/Makefile
/ssh:arthur@build#2222|sudo:root@build:/etc/nixos/configuration.nix
/ssh:build|podman:dev:/work/src/main.rs
```

- Paths are bytes on the wire, so any Unix file name survives there; a
  location is displayed with replacement characters where its path is
  not UTF-8. (Lisp still carries locations as strings, so such a path
  cannot be opened yet.)
- `~` and relative paths are resolved against the endpoint (its home, a
  directory there), never against the local machine.
- Path operations (parent, name, joining, the project root) are
  operations on locations; code never takes a location apart as text.
- A location is an address, not authority ("Authority").

## Routes and methods

A route is a sequence of hops; each hop names a method and its
parameters (user, host, port). A route is canonical: defaults (ssh
configuration aliases, a default user) are resolved when it is parsed
and shown with it, not kept in a separate table. The route is the key
under which connections are shared: every location on
`/ssh:build:` uses one connection.

A method knows two things: how to parse its hop, and how to open a byte
stream to the next endpoint from the one before it. Optionally, how to
deploy the node binary there and how to authenticate. Methods are
defined in Lisp, as `tramp-methods` entries are; the mechanism under
them (starting processes, pipes, the protocol) is Rust.

```scheme
(define-method 'sudo
  "Become another user, root by default, on the same machine."
  #:command (lambda (hop jumps program)
              (append (list "sudo" "-n" "-u" (or (hop-user hop) "root") "--") program)))

(define-method 'podman
  "Enter the running container HOST with podman exec."
  #:command (lambda (hop jumps program)
              (append (list "podman" "exec" "-i" (hop-host hop)) program)))
```

A method's command runs PROGRAM, the node, one hop further: here for
the first hop, on the endpoint before it for a later one.

- **Leading ssh hops become one ssh command** with `ProxyJump`: a bastion
  in the middle runs nothing of ours, and OpenSSH does what it does best.
- **Later hops are opened by the endpoint before them**, as a process
  there whose standard input and output carry the next endpoint's
  protocol. `/ssh:build|podman:dev:` is one ssh connection to `build`,
  and a node in the container started by the node on `build`. Unlike
  tramp-rpc, a container at the end of a route is as fast as a host.
- What passes through an intermediate endpoint is a byte stream to it,
  not frames it decodes: no relaying protocol, but the streams must be
  bounded and must not delay control messages ("The protocol").

## Endpoints report what they are

Methods do not say what a hop changes (machine, file system, user,
process space). Classifying methods is wrong at the edges: `ssh
localhost` keeps the file system, `distrobox` changes namespaces but
keeps the home directory, `nsenter` depends on its flags. Instead each
endpoint reports facts about itself when it connects:

- machine id and boot id;
- user id, user name, home directory, login shell;
- mount and process namespace ids;
- architecture, operating system, host name.

Everything else is derived from them:

| question | answer |
|---|---|
| Do two endpoints see the same files? | Same machine and same mount namespace. |
| Is this the same file? | Same file system, device and inode: `/ssh:build:/etc/x` and `/ssh:build\|sudo:root@build:/etc/x` are one document, not two edited in parallel. |
| Do two routes reach one endpoint? | Same facts: `/ssh:build:` and `/ssh:build.lan:` keep their own connections, but caches and documents meet. |
| What is `~`? | The endpoint's home. |
| Which process is this? | Its logical id, qualified by endpoint and boot id. |

A fact a platform cannot report (namespaces on macOS) is unknown, and
unknown counts as different. Facts are only known once connected;
deploying the binary needs an answer before that, which a method's
`#:deploy` gives. By default it tries the path the previous endpoint
installed, then sends the binary through the method's own stream (which
needs a `sh` there).

## Endpoints, connections and sessions

- An **endpoint** is what a route reaches, identified by its facts.
- A **connection** is one generation of a stream to it. A reconnection is
  a new generation; handles of the old one raise `node-lost`, never
  silently name objects of the new one, and a late reply of the old one
  is discarded.
- A **session** (`techne-node --session`) is the supervisor on the
  endpoint that owns persistent processes and services across
  connections. Network loss, the session ending and the machine
  rebooting are three different events, reported as such.

## Worlds and services

An endpoint hosts worlds. Code calls a service, a world running a module,
not "the node": the call has the shape of the editor's `service-call`
(`crates/techne-editor/src/service.rs`), with its bounded queue,
deadline, memory limit, cancellation and restart, whether the world runs
on a thread of the editor's process or on another machine. The failures
differ and are not hidden: a remote call can also end in `node-lost`,
and a call whose reply was lost has an unknown outcome. A non-idempotent
call is never retried by itself.

- A service's code is a module's source, sent when the service starts;
  its hash identifies it (and its generation), never permits running it.
- Closures are never shipped. Remote values stay as they are, for
  inspecting and applying interactively (node-protocol.md).
- `node-eval`, evaluating source text, is the interactive tool and is
  gated as evaluation; services are the substrate packages build on.

## Execution context

A project has an execution context: an endpoint, a root location, a
working directory, an environment and the authority granted to it. It is
an inspectable target, and buffers take theirs from their project. A
buffer's default directory is derived from it, never the other way
round.

- Processes, terminals, version control, language servers, builds and
  agents run in the context: on its endpoint, in its directory, with its
  environment.
- Executables are found on the endpoint, in its `PATH`.
- The environment is the endpoint's, reported at connection (the login
  shell's `PATH`) and refined there (direnv), not reconstructed locally.

## Documents on other machines

The editor's runtime stays the one authority for a document's text,
wherever its file is. Typing is local.

- Opening reads the file and its revision (the hash of its bytes, as
  `Document::save` uses for local files today).
- Unsaved edits are journalled locally, as for local files.
- Saving sends the text and the revision it was based on; the endpoint
  writes atomically (a temporary file and a rename) only if the file
  still has that revision. Otherwise the save fails with a conflict, and
  the choices of a conflict. A save whose reply was lost has an unknown
  outcome: the document stays modified until it is resolved, by reading
  the file's revision again.
- A search of a remote project reads the files there and overlays the
  local unsaved text of open documents. Opening a remote project does
  not send open documents to it; a service sees a document's text only
  when the document is granted to it (PLAN.md, Stage 1, step 8.6).

## Authority

A route is an address. Writing `/ssh:build|sudo:root@build:` in a link or
a file grants nothing; resolving it checks authority as any other
reference does.

- **A connected endpoint is a capability** a world holds, as a value,
  not a grant bit. It can be narrowed before it is handed on: read only,
  one subtree, no processes, no evaluation, no further connections.
  Services and agents get narrowed ones.
- **Connections are opened by the runtime**, not by the world asking:
  today `node-connect` runs any local command for a world with the
  network capability (`crates/techne-node/src/lib.rs`, `install`). A
  world asks for a route and gets an endpoint, or a condition.
- **Features are availability, not permission.** An endpoint offering
  evaluation does not let every package evaluate there.
- **Elevation is new authority.** A sudo hop asks for it each time it
  connects, through the pending decisions, unless the user stored a
  policy for that route.
- **Secrets never share a stream with data.** ssh authenticates through
  OpenSSH, with an askpass program bridged to the runtime's pending
  decisions. sudo runs with `-A` and an askpass program the endpoint
  provides, which asks over the protocol: the password never precedes
  protocol frames on the same input, as it does in tramp-rpc.

## The protocol

The node protocol's framing stays (a big-endian `u32` length, then
MessagePack, at most 16 MiB); the rest changes, in a second version
shared with tramp-rpc's server.

- **Envelope.** Requests are maps of id, method and parameters; replies
  of id and a result or an error; notifications have no id. Method names
  are namespaced: `fs.*`, `process.*`, `watch.*`, `system.*` everywhere;
  `techne.*` (evaluation, services, remote values) only on full nodes;
  Emacs' `highlevel.*` in tramp-rpc's own namespace.
- **Hello.** Each side states the protocol versions it speaks, its
  features with their versions, its limits, and (the endpoint) its
  facts. They agree on a version or close with an error naming both;
  there is no exact-match rule. Features are what is available, nothing
  more.
- **Bytes are bytes.** Paths, contents and process data are MessagePack
  binaries, never strings.
- **Errors are structured**: a kind (not found, permission, exists,
  conflict, unsupported, limit, lost, ...), the operation and a message,
  so a condition can be raised with the right type and restarts.
- **Cancellation** is a notification naming a request; the endpoint
  acknowledges it. It stops waiting and asks the operation to stop; it
  undoes nothing.
- **Streams.** Process output is pushed, not read by requests: per
  stream, sequenced chunks, credit-based flow control, an explicit end of
  file, and the exit after the last output. A persistent process keeps a
  bounded backlog; reattaching replays it from a sequence number and
  reports a gap. No order between standard output and error is promised.
  Bulk data is chunked, and control messages (cancellation, a terminal's
  input, C-g) have capacity reserved ahead of it.
- **Bounds.** Pending requests, held remote values and buffered output
  are bounded per connection, and the limits are in the hello.
- **Data in `techne.*` calls** stays the written form node-protocol.md
  describes, until the data notation (PLAN.md, Stage 1, step 9) replaces
  it.

## The shared engine

One Rust implementation of file and process operations serves Techne
and tramp-rpc, in one Cargo workspace for now:

- **an engine crate**: typed file system operations (with tramp-rpc's
  careful copy, rename and access semantics), process supervision with
  pipes and ptys, and watches. It knows neither Lisp nor MessagePack;
- **a protocol crate**: framing, the envelope, the hello, streams,
  cancellation, bounds and errors, with schemas and conformance tests
  both clients run;
- **two servers**: `techne-node`, the full runtime, and a small
  primitives-only binary, static, for any architecture tramp-rpc builds
  for, which needs no VM. tramp-rpc's server is the latter plus its
  `highlevel.*` adapter.

Techne's local file and process natives call the same engine. The Lisp
file API dispatches on a location's endpoint: local calls go straight to
the engine, remote ones through the protocol. The engine starts as
tramp-rpc's handlers, extracted with their tests passing, then given
typed interfaces in place of their MessagePack values and global output
writer.

## Order of work

1. **Locations and execution contexts, local only.** The location type,
   its TRAMP syntax, the local endpoint; `lisp/editor/files.scm`,
   `projects.scm`, search, the shell and running tools on them. A test
   mode runs the editor's tests with the project behind a node over a
   pipe, so a local assumption fails CI.
2. **The engine and protocol version 2**, alongside: extract the engine,
   keep tramp-rpc's tests passing on it; spawning with a directory and
   environment, stat, read, list, the conditional write, pushed streams,
   the hello with facts.
3. **One remote project over ssh**: open, edit, save, search, build.
   Tested for latency, cancellation, a file changed on the other machine,
   a connection lost during a save, standard error and end-of-file
   order, and a runtime restart.
4. **sudo and containers**: hops opened by the endpoint before them,
   authentication through askpass, documents recognised across routes.
   Tests chain nodes over pipes with a trivial method; a container where
   CI has one.
5. **Services on endpoints**: `make-service` on an endpoint; search and
   version control status beside the data. Then watches invalidating
   caches, and reattaching to persistent processes and services after a
   reconnection.

## Not yet

- Recursive relaying through several full nodes beyond what hops need.
- Distributing modules by content hash, and deploying packages.
- Deploying the full runtime automatically: it is installed; only the
  primitives-only binary is deployed.
- Monitors and links in the manner of Erlang.
- Speculative caches in the manner of tramp-rpc's Magit prefetch.
- Exactly-once delivery, transparent retries, distributed transactions,
  moving running computations.

## Questions

- Whether remote documents' text may be journalled locally on every
  route, or a route can forbid it (REQUIREMENTS.md: remote authority is
  not permission to cache everything locally). Journalled by default
  until a route needs otherwise.
- Whether packages use Lisp beside the data wherever an endpoint offers
  it, or only where the user allows it per route.
- Endpoints that are not Linux: which facts macOS can report.
