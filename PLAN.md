# Techne: engineering direction and staged plan

This plan implements [REQUIREMENTS.md](REQUIREMENTS.md). It chooses a direction
without treating untested runtime properties as established facts. There are
no delivery estimates yet: the first experiments should establish feasibility
and expose the expensive parts before estimating the full application scope.

The interaction goal is: anything native that can be seen should be inspectable;
anything actionable should be composable; automation should remain understandable.
The object microscope, editable lenses and semantic recipes are the principal
additions. History, recovery and attention make them safe to use daily.

## 1. Architectural direction

### A small substrate and a substantial live application world

```text
Human interaction         Existing agents / native agents
        \                         /
      Targets · Selection · Commands · Recipes · Lenses
                         |
               Live application world
       Editor · Org · VCS · Mail · Desktop policy
                         |
       Resources · Documents · Tasks · Work contexts
                         |
                  Rust mechanisms
          /              |                \
 Compositor engine   Local services   Remote node services
```

This is a conceptual separation, not a prescription for one process per box.

- **Compositor process:** owns Wayland clients, input delivery, rendering and a
  last-known-valid layout. Retains a minimal recovery/control path independently
  of application Lisp. Layout policy arrives as validated changes.
- **Interactive application runtime:** initially one trusted live Lisp world,
  hosted by Rust. Uses asynchronous services and background workers. No blocking
  remote calls in redisplay, completion, or input handling.
- **Node service:** manages resources and processes locally or remotely through
  the same application-facing contracts. A local fast path need not serialize
  every call. Only remote deployments require the transport boundary.
- **Optional node-side Lisp host:** supports inspectable remote services and
  explicit computation near data. Its lifetime can outlive a frontend connection.
- **External tools/agents:** keep their own processes, with adapters mapping
  available capabilities into Techne objects.

Initially target Linux/NixOS for the desktop. Broader remote-platform support is
an extension of the node service, not a reason to delay a useful Linux path.

### Separate resource, document, and view

A resource identifies something being worked with. A document owns editable
content and versions. A view presents content or an external application.
Do not force a Wayland surface into a mutable-text abstraction.

A project associates resources, views, processes, agents, and execution context.
Multiple views can share a document; switching presentation must not duplicate
ownership of user data. Keep the initial object kinds concrete rather than
building a universal graph database before useful applications exist.

A work context is a saved collection of references, navigation and attention
preferences, possibly spanning projects. It does not own copies of their resources
or replace execution context. Task lifetime is explicit: closing a context cannot
implicitly kill a persistent task or grant it new authority.

Distinguish persistent references (resource/entity identity and best-effort source
anchors) from live handles (owner and generation). A restored reference is resolved
and reauthorized, not treated as a still-valid pointer. Resolution reports missing,
inaccessible, stale or ambiguous targets rather than silently picking another one.

### Lenses are views, not alternative stores

Implement the first lens as source-backed text excerpts over versioned documents.
Each segment maps to a document and tracked range. Edits resolve through document
operations and their conflict checks; duplicate or overlapping excerpts must not
apply the same edit twice. Multi-document edits have an explicit changeset/result,
without claiming atomic remote filesystem writes.

Queries may update asynchronously. Freeze and validate the intended target set
when applying an operation so a refreshing view cannot silently change its scope.
Structured lenses such as agendas provide typed actions rather than arbitrary
writeback through every calculated column. Indexes remain derived and rebuildable.

An agent receives a versioned snapshot or explicitly supported live resource
access, never the assumption that its context automatically updates when a lens
does. A lens is a context-selection tool, not a security boundary.

### Commands and targets precede applications

Build contextual action discovery, argument acquisition, selection, search,
preview, and action results once. Native applications supply target types and
operations. Availability can depend on capabilities and current state.

Use the same command implementation for interactive and programmatic use, with
separate admission/permission checks for external callers. Some operations need
confirmation or cannot sensibly apply to multiple targets; metadata must say so.

Give commands stable names, argument/result schemas, source/package ownership,
and declared effects. Build transient-like argument controls from these contracts,
with custom controls where useful. Preview is optional and explicit; a preview
must not accidentally perform the irreversible operation it claims to describe.

Recipes compose these named commands with explicit data flow and parameters.
Start with sequential composition, not a general workflow language. Record only
supported operations and redacted arguments; resolve dynamic targets deliberately.
On replay, validate command compatibility, resolve targets, check revisions and
reauthorize effects. Changed commands or uncertain partial completion require an
explicit decision. Recipes invoke the existing task executor and return the same
results/conditions as manual commands.

### The object microscope is the common inspection UI

Register inspector views and actions through the normal package-owned registry.
Expose identity, owner/generation, capabilities, source definitions and associated
tasks. Retain bounded decision provenance at important dispatch points: keymap
resolution, action availability, layout rules and query matches. Prefer current
explanations or explicit recorded facts to speculative reconstruction.

Source navigation and live redefinition reuse the editor/REPL. Cheap inspection
uses metadata; remote or expensive views load asynchronously and are cancellable.
Inspection does not implicitly grant access to sensitive content or turn a
read-only inspection into arbitrary evaluation. External targets show only the
metadata and actions their adapters support.

### Async services with explicit authority

Application APIs return results, tasks, or streams without concealing blocking
network access in ordinary object field reads. Streams have bounded queues and
backpressure. Cancellation distinguishes abandoning a wait from stopping a task.

All document writes carry a base revision or an equivalent conflict check.
Agent changes can be staged and reviewed, but shell commands and external effects
are not falsely presented as reversible transactions.

A permissions dialog is not a sandbox. A package with unrestricted native FFI or
a spawned shell can escape a host-API allowlist. Initially distinguish trusted
live application code from restricted external actors; claim isolation only
where runtime or OS boundaries actually enforce it.

### History, recovery and attention share operation identity

Use a small operation record linking command, actor, target revisions, task, work
context and outcome. Correlation IDs connect these records to document history
and bounded diagnostic traces. This is not a global event-sourced system; document
storage and task ownership remain authoritative.

Keep undo separate from retained document revisions and version control. Snapshot
policy applies before capture, including to recipes, agent context and logs.
Provide retention limits, sensitive-resource exclusions, deletion and redaction.
History for remote documents must respect whether local content retention is
allowed; remote authority is not permission to cache everything locally.

Restoration is a new revision-aware operation, with conflicts surfaced. Track
supported declarative configuration overrides against a pinned baseline and allow
saving them; do not promise to serialize arbitrary live mutations or credentials.

Conditions identify the failing operation and offer explicitly implemented
recovery actions. Pending recoveries are resumable task state, not necessarily
captured Lisp continuations. Revalidate authority and target state on recovery;
an uncertain external effect cannot be replayed simply because the UI says retry.

Progress and attention are orthogonal task properties. Route informational updates,
requests for decisions and allowed urgent interruptions through context-aware,
programmable policy. Start with a task/decision list in the hosted workbench;
desktop notifications are a later presentation of that same model. Coalesce noisy
updates without losing pending decisions. The recovery/control path for a stalled
runtime remains available outside ordinary Lisp attention routing.

### Semantic transfer and resumable contexts

Use the same reference resolver for links, clipboard targets, recipe arguments
and saved work contexts. Transfer plain text plus typed representations where
supported. Optional snapshots are explicit and subject to disclosure policy.
Treat incoming clipboard payloads as untrusted data: resolving a reference neither
executes embedded code nor grants its sender's authority.

Save reference sets, navigation and presentation preferences rather than heaps or
process handles. Resuming a context resolves resources, reports missing ones and
reattaches to persistent tasks through their supervisor. It must not rerun recipes,
relaunch side-effecting commands or resend messages merely to reconstruct a view.

### Remote execution without distributed-memory semantics

Start with authenticated SSH transport and a versioned, negotiated node protocol.
Reuse ideas and potentially independent components from TRAMP-RPC after reviewing
coupling and licenses; do not drag Emacs compatibility behavior into the new API.

A process has stable logical identity, node/backend metadata, streams, lifecycle,
and supported control operations. Transport parsing queues events; it does not
run arbitrary application callbacks inline. Output, EOF, and exit ordering are
specified and tested independently of UI code.

Persistent tasks belong to a supervisor, not a transport. Reconnection explicitly
reattaches to known task identities. Keep connection generations distinct so stale
replies cannot mutate replacement objects. Use sequenced output with bounded
replay and explicit gaps. A lost reply may leave an operation's outcome unknown;
there is no blanket exactly-once guarantee.

Remote Lisp starts with explicit service/module entry points, serializable
values, remote handles, inspection, and evaluation on a chosen node. Record the
code generation used by long-running tasks. Redefinition affects subsequent
calls under documented semantics; it does not silently rewrite active stacks.
Arbitrary heap persistence and automatic state migration are deferred.

## 2. Implementation candidates, not product requirements

| Need | Initial candidate / reuse strategy |
| --- | --- |
| Systems substrate | Rust |
| Live application language | techne-vm, a new runtime below Steel's parser ([runtime/TECHNE-VM.md](runtime/TECHNE-VM.md)) |
| Wayland implementation | Smithay; use EWM and niri as engineering references |
| Rendering/text | Evaluate existing GPU, font shaping, and text-layout libraries together |
| Text storage/parsing | Existing rope/incremental parsing libraries where suitable; preserve source text |
| Language intelligence | LSP plus structural parsing |
| Remote connection | SSH bootstrap; structured binary protocol selected after requirements review |
| Terminal | Reuse a maintained terminal engine; own PTY/process integration |
| Git/Jujutsu | Their supported commands/interfaces first; no new VCS engine |
| Email | Existing indexing, synchronization, and sending backend(s) |
| Agents | One existing-agent adapter first; native orchestration later |
| Browser | Existing browser; extension/native-messaging bridge where justified |
| Reproducibility | Nix development environment and pinned dependencies |

Do focused library research before implementation. Do not select rendering,
terminal, Lisp, or Org libraries solely because they are written in Rust.

## 3. Staged delivery

### Stage 0 — Runtime readiness, then platform contracts

#### 0A — Qualify the runtime

This is a dedicated engineering phase, not a brief embedding smoke test.

**Current direction: techne-vm.** Steel was modernized first (below). Its value
representation, collector and JIT capped performance at 6–17× slower than Chez,
so a new runtime core, techne-vm, replaced everything below Steel's parser:
NaN-boxed values, a generational copying GC, a register VM with tasks, and a
Cranelift JIT with the interpreter as semantic reference. It is near Chez on the
benchmark suite and is differentially fuzzed against its interpreter; see
[runtime/TECHNE-VM.md](runtime/TECHNE-VM.md) for design, results and the gate
status. The contracts and the runtime gate below apply to it unchanged; the
Steel documents that follow are history.

The Steel phase began with four isolated implementation experiments; see
[runtime/EXPERIMENTS.md](runtime/EXPERIMENTS.md) for baseline, ownership and gates.
Those experiments led to a reviewed, tested, bounded integrated milestone; see
[runtime/MODERN-RUNTIME.md](runtime/MODERN-RUNTIME.md): resumable owner-thread VM
slices, a gc-arena incremental managed-heap subset, and an executable verified
semantic IR/optimization path now work together in an isolated Steel checkout.
This does not replace the runtime gate below. The follow-on assembly now includes
wake-driven budgeted host waiting/cancellation, persistent task roots, actual
built-in atomic marking with incremental sweep, scalar-native safepoints, broader
IR admission, and owned descriptor/graph-transfer corrections. Independent reviews
closed the identified lifetime/transfer blockers; the final combined rerun and
controlled performance qualification remain outstanding. Incremental marking,
heap-capable native execution and a compact-value ABI are not delivered by this
assembly. Evidence, supported semantics and remaining gates are tracked in
[runtime/READINESS.md](runtime/READINESS.md).

- **Async and safe concurrency:** host-driven suspension/resumption, wake-driven
  I/O, cancellation, bounded CPU execution and explicit thread/heap ownership.
  Interrupting or aborting a computation is not the same as resumably yielding it.
- **Efficient values:** measure current representation and compare compact tagging
  or handles with safe Rust host boundaries. Test numeric semantics, heap identity,
  root lifetimes and portability; do not assume an eight-byte value always wins.
- **Modern GC:** choose and exercise a collector/rooting strategy against pause,
  allocation and lifetime requirements. Account for cycles, suspended tasks,
  Rust-held roots and deterministic cleanup of external resources. Evaluate
  established collector libraries before designing a replacement from scratch.
- **Compiler IR and modern JIT (one lane):** map the existing AST analysis/passes,
  bytecode lowering and Cranelift path before changing them. Invest in a minimal,
  verified language-level IR with explicit control flow, values, effects, source
  locations and runtime boundaries. Build on Cranelift for machine-code generation,
  not a new backend. Measure compilation latency and warm throughput, retaining
  semantic guards/fallback, interruption/GC safepoints and code-generation identity.

The compiler experiment should lower a supported subset of expanded/resolved Scheme
into an inspectable CFG/SSA-style representation, with verification before and
after each pass. Explicitly distinguish proven types from speculative guards and
represent allocation, mutation, suspension, exceptions, unknown calls and tail
calls so transformations cannot erase observable behavior. Preserve the contracts
needed for continuations, Rust roots and live redefinition; mark unsupported forms
for existing-path fallback rather than approximating them.

Start with effect-aware constant propagation, branch simplification or dead-code
elimination over that subset, using differential tests against the existing path.
Inlining and unboxing follow only with correct guards, invalidation and recovery
metadata. Arbitrary Rust callbacks are effectful unless an explicit sound contract
says otherwise. Stage a future common lowering to interpreter bytecode and native
code rather than maintaining two independent language semantics; do not require
rewriting both backends in the first experiment. Track per-phase compile cost and
source mapping quality as well as execution speed. Avoid speculative stacks of IRs
or adopting a new compiler framework without a demonstrated need.

The experiments inform one combined contract for semantic IR, values, host handles,
roots, safepoints, task ownership and generated code. Review before integrating; four
individually promising prototypes are not a usable runtime. Integrate accepted
changes serially, retaining the interpreter as the semantic reference and fallback.

The integrated Rust embedding testbed must exercise live redefinition, delayed
futures, cancellation/late wakeups, CPU-bound tasks, allocation-heavy work, cyclic
data, Rust objects and compiled loops together. Measure tail latency, pauses,
throughput, memory and host-call costs; compare cold/warm modes and rerun accepted
performance measurements serially rather than while all lanes compile/benchmark.
Include a two-process explicit Lisp invocation/inspection probe. Investigate
compilation and macro expansion as sources of interactive pauses too.

**Runtime gate:** an evidence-backed, safety-reviewed combined implementation with
reproducible tests and documented supported semantics, gaps and measured budgets.
Unsupported JIT forms fall back correctly. Blocking native calls and unbounded
pauses have explicit boundaries; benchmarks cannot conceal those limitations.
If the runtime cannot meet the essential contracts cleanly, reconsider the
implementation strategy rather than declaring it ready. Do not begin substantial native
application development before this gate; tiny consumers exist to test it.

#### 0B — Prove platform contracts on the qualified runtime

Build small executable probes, not a framework with empty application APIs.

1. **Native process contract:** run local and remote pipe/PTY children through one
   API. Exercise concurrent output, separate stderr, signals, EOF, slow readers,
   transport loss, and independent task cleanup.
2. **Persistence:** disconnect and reconnect to a running terminal/build owned by
   a node supervisor, with bounded output replay and explicit state reconciliation.
3. **Rendering/compositor boundary:** measure normal text/input latency and prove
   an external Wayland client plus the emergency control path remain usable while
   application Lisp is stalled or restarted. Lisp-owned application behavior may
   pause until interrupted; do not claim otherwise. Test a nested environment
   before relying on it as the desktop.
4. **Interaction contracts:** use one small text-excerpt lens and named command to
   exercise source mapping, stale revisions, overlapping selections, inspection
   provenance and recipe replay validation. This probes the abstractions before
   every application depends on them, not a complete interaction framework.

**Exit:** the runtime gate plus tested platform failure boundaries, sufficient to
build the workbench without reinventing async, process or ownership behavior in
each application.

### Stage 1 — A usable hosted workbench

Develop inside an ordinary desktop window before replacing the current session.

- Document editing, save/reload/conflict handling, undo, multiple views and bounded
  local history, with sensitive-resource exclusions available before capture.
- Typed selection/refinement, completion, contextual actions, argument controls,
  optional previews and actionable results. A minimal editable search-excerpt lens.
- Object microscope, ownership/source lookup, REPL and definition evaluation.
- Local/remote project context, resource browsing, search and terminals.
- Named command contracts and simple parameterized recipes with validation.
- Shared reference resolution and internal typed copy/paste with text fallback.
- Structured conditions and explicit recovery choices; task/decision list with
  basic focus-preserving attention policy.
- Minimal save/resume of work-context references and navigation; report missing
  resources and reattach supported tasks without replaying their creation.
- Owned registrations and cleanup; explicit declarative configuration overrides
  that can be inspected against the baseline and saved.

**Exit:** open a remote project, refine search results into a lens, edit through it,
run a build and act on its output. Inspect and redefine the responsible command,
record a safe parameterized recipe and replay it with validation. Recover an
uncommitted edit from history and resume the work context after a frontend restart.
All of this must work without silent stale writes or blocking the UI.

Build this in three usable cuts: (1) document/command identity, basic editing,
inspection and history; (2) local/remote tasks, lenses and recovery; (3) recipes,
typed transfer and context restoration. Use basic presentations and a small
command set: Stage 1 proves vertical slices of the shared primitives, not polished
implementations of every future feature.

### Stage 2 — Agent and version-control development loop

- Integrate one existing agent with native conversation/task views and explicit
  mapping of its actual capabilities; avoid maintaining several shallow adapters.
- Provide revision-aware editor operations, reviewable changes and activity logs
  linked to document history. Supply lens snapshots as agent context when supported.
- Build actionable Git/Jujutsu status, diffs, history and common operations.
- Add language-server diagnostics/navigation and project-scoped execution.
- Run remote tools and agents in the project's remote environment.
- Use source-backed lenses for diff/search/diagnostic views where meaningful;
  historical content remains read-only unless an explicit apply action exists.
- Route agent approvals and failures into the existing decision/recovery UI.
  Associate conversations, changes and tasks with resumable work contexts.

**Exit:** carry out a real local and remote code-change workflow with an agent,
review the result and use version control, including a concurrent human edit and
a disconnect/reconnect case without silent data loss. Inspect the origin of a
tracked change, restore it safely, and resolve an approval without focus theft.
Replaying a recipe with stale targets or changed authority must fail safely.

### Stage 3 — Become the desktop

- Host native views and ordinary Wayland applications in one layout/navigation
  system; expose application windows as actionable targets.
- Implement global command routing and focus; carry semantic clipboard formats
  across the desktop with safe text fallbacks. Present the existing attention model
  through notifications rather than introducing a second task/approval system.
- Extend the microscope to native UI ownership, keymap decisions, layout rules
  and external-window metadata; do not claim inspection of opaque app internals.
- Validate multi-monitor hotplug/scaling, input configuration, locking, suspend/
  resume, fullscreen, and screen sharing on the actual target hardware.
- Add shell/editor handoff and browser integration incrementally.
- Extend work-context restoration to desktop layout/application references and
  attention policies; do not promise serialization of third-party application state.

**Exit:** safely use Techne as the daily compositor, including recovery after a
failed live-code experiment. Resume an interrupted context, inspect why a native
view is positioned as it is, and handle background decisions without unsolicited
focus changes. Clipboard links must not grant authority or trigger execution.
Keep the old desktop available as a fallback. Ordinary Emacs can remain a guest
application until native Org/mail meet the replacement requirements; this does
not require an Elisp compatibility layer. The Stage 0 compositor experiment
reduces risk here; it is not itself a desktop.

### Stage 4 — Native Org and personal information workflows

Org is a substantial workstream, not a small parser feature. Preserve source text
as authoritative; syntax trees and indexes are derived and rebuildable.

Deliver in useful vertical slices:

1. Lossless editing/folding, headings, properties, timestamps, links and TODOs.
2. Capture, stable node IDs, linked notes, backlinks and external-change handling.
3. Agenda, scheduling, recurrence, habits and queries with correct date semantics.
4. Tables, Babel execution/results/tangling and the required export formats.
5. Calendar synchronization with explicit recurrence, timezone, conflict and
   deletion handling; avoid treating synchronization as simple file copying.

Babel uses the native execution model, including remote contexts and execution
approval. Opening a document must not implicitly grant its code execution rights.

Introduce native email views/actions using an existing backend. Connect email
and Org through the shared reference and action protocols; add patch workflows
where needed. Owning the mail storage/sync engine can wait.

Agenda and linked-note results become lenses using explicit heading operations.
Inspectors can explain query membership; captures and email references use the
semantic clipboard. Recipes can compose mail-to-task or patch-to-build workflows,
with sending mail and other external effects explicitly authorized. Attention
policies and work contexts are reused, not rebuilt inside the mail/Org apps.

**Exit:** complete the established Org and email workflows on representative data,
with supported behavior checked against references and unsupported syntax kept
intact. Also follow an email reference into a note, inspect an agenda match and
act on it at the source; unavailable links must be explained. Unsupported cases
are documented, not silently approximated.

### Stage 5 — Deeper programmable automation

- Native agents composed from the same task, command, context and permission APIs.
- Remote live Lisp services, inspection and code updates with explicit ownership
  and generation semantics, built on the node execution foundations.
- Better live package replacement: validation, supported state migrations, and
  rollback of registrations where possible.
- Broader cross-application actions, domain-specific inspector views and browser
  semantic integration through the existing command/reference/lens protocols.
- Extend recipes only where real workflows demand more than sequential commands;
  avoid inventing a separate agent workflow language or durable workflow engine.

**Exit:** a user or authorized agent can develop and inspect a remote service,
change its behavior live, and compose it with native applications without
introducing a separate automation framework or an implicit distributed heap.

## 4. Coherence and dependency map

Applications consume shared primitives instead of implementing parallel systems.
Core means the contract is foundational, not that its complete UI ships at once.

| Capability | Shared foundation | First usable slice | Later extension |
| --- | --- | --- | --- |
| Object microscope | Identity, ownership, registrations, source metadata | Stage 1 commands/resources | Stage 3 UI/layout; Stage 4 queries; Stage 5 remote services |
| Editable lenses | Versioned documents, source anchors, typed targets | Stage 1 search excerpts | Stage 2 diagnostics/diffs; Stage 4 Org views |
| Semantic recipes | Named commands, arguments/results, task executor | Stage 1 sequential parameterized recipes | Stage 2 agents/VCS; Stage 4 mail/Org |
| Safe history | Document revisions, operation identity, retention policy | Stage 1 document/config recovery | Stage 2 agent-change attribution |
| Interactive recovery | Structured conditions, task state, authorization | Stage 1 explicit recovery actions | Stage 2 agent approvals; Stage 5 remote services |
| Semantic clipboard/links | Reference resolver, typed targets, disclosure policy | Stage 1 internal transfer and text fallback | Stage 3 desktop; Stage 4 email/Org |
| Attention | Task/decision state, deterministic routing | Stage 1 task/decision list | Stage 2 agents; Stage 3 desktop notifications |
| Work contexts | Saved references, task supervisor, navigation | Stage 1 minimal save/resume | Stage 2 conversations; Stage 3 layouts; Stage 4 personal information |

Persist documents and declared records, not a universal object heap. Compose
commands, not simulated UI interaction. Reattach tasks, do not replay their side
effects. Resolve references, do not turn them into bearer credentials. These rules
keep remote operation, live programming and the new interaction ideas compatible.

## 5. Validation and scope discipline

- Use end-to-end workflow acceptance cases plus focused contract tests, rather
  than matching package counts or generating a test for every removed feature.
- Establish measured input/render latency and resource budgets during the first
  experiments; then test under output floods, slow networks and agent workloads.
- Exercise dropped connections, stale replies, disk errors, stalled tasks, and
  crashes before relying on persistence or claims of safe recovery.
- Audit authentication, authorization and data disclosure before exposing remote
  evaluation or broad agent actions. SSH access alone is not an agent policy.
- Test selected Org semantics and round trips against fixtures/reference behavior;
  do not use an entire personal mail/notes archive as an unreviewed test corpus.
- Test lens edits against stale/overlapping ranges and asynchronously changing
  queries. Verify read-only projections cannot accidentally mutate sources.
- Exercise recipe replay after command changes, permission revocation and uncertain
  completion; prevent automatic replay of potentially completed effects and report
  partial outcomes explicitly.
- Verify history exclusions apply before recording, restoration checks revisions,
  and saved contexts/clipboard payloads do not smuggle credentials or execution.
- Check inspection, notifications and query updates remain bounded under load;
  pending decisions must remain discoverable even when updates are coalesced.
- Check licenses before incorporating reference implementations.
- Defer collaborative editing, universal object graphs, transparent task migration,
  automatic closure shipping, and universal transactional undo until concrete
  workflows demonstrate a need.

The first useful milestone is not “all of Doom rewritten.” It is a small live
workbench whose commands, objects and process model already behave coherently
across humans, agents, local execution and remote execution.
