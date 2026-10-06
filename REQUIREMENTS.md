# Techne: requirements

Status: requirements baseline, including the approved interaction ideas, revised
after building the runtime (language requirements, worlds, a Techne-owned
compositor). Product commitments come from the discussion with Arthur;
engineering defaults below are recommendations, not validated implementation
claims. See [PLAN.md](PLAN.md) for the proposed approach.

## Purpose

Replace Emacs + EWM with a coherent, live-programmable personal computing
environment on Linux. Preserve the all-in-one experience while removing the
single-interactive-thread bottleneck and making agents and remote execution
native participants.

This is not a new kernel, distribution, browser engine, or Emacs emulator.

## Core ideas

### One environment, not just one screen

Editing, Org, mail, version control, terminals, agents, and desktop applications
share navigation, commands, selection, search, and project context. Native
applications can compose their objects and operations, not merely their windows.
Existing graphical applications remain usable even without semantic integration.

### Embark is foundational

Selection and search return typed, actionable targets rather than display
strings. Applicable actions are discoverable on individual targets and sets of
targets. Results remain actionable: a search result can become an input to a
subsequent operation.

Commands describe their arguments, availability, effects, and documentation.
Humans, live programs, and authorized agents use the same underlying operations.
Interactive prompts are argument acquisition, not hidden requirements of the
operation itself. A graphical widget is not the only way to invoke behavior.

This does not mean exposing every internal function as an agent tool. Visibility
and authority are separate from a command's existence.

Selections support a common flow: find targets, refine the set, inspect, choose
an action, configure arguments, preview where supported, and execute. Argument
controls should be discoverable without removing efficient keyboard paths.

Sequences of supported commands can become inspectable, editable **recipes**,
with explicit parameters and results rather than recorded screen coordinates or
keystrokes. Recipes use the same command/task machinery as direct interaction,
not a separate workflow engine. Replay rechecks target validity and permissions;
it does not blindly repeat irreversible effects or ambiguous partial operations.

### Editable lenses

A lens presents selected parts of underlying resources without copying them into
an unrelated document. Examples include code search excerpts, a change's affected
functions, or an agenda's matching headings. Source identity and actionable
navigation survive presentation changes.

Text excerpts may be edited through to their versioned source documents. Other
lenses expose explicit operations or remain read-only when reverse mapping is
ambiguous. Stale or overlapping source ranges require reconciliation, not guessed
writes. A lens can provide shared human/agent context but grants no authority and
must distinguish a current view from the snapshot actually sent to a model.

### Live programmability

Users can inspect running objects, find the code responsible for behavior, and
redefine application behavior without restarting the environment. Lisp is the
application language, not just a configuration syntax around a fixed Rust UI.

Package registrations have ownership and cleanup: commands, keybindings,
subscriptions, tasks, and views must not accumulate accidentally after reloads.
User documents belong to the environment, not to the package displaying them.

Arbitrary state migration and atomic upgrades of every package are not initial
promises. Unsupported reloads must fail explicitly and preserve user work.

A universal **object microscope** makes this practical. Native UI elements,
commands, targets, tasks, and packages expose identity, ownership, source
locations where available, and domain-specific views and actions. Inspection
should explain recorded decisions such as keymap resolution or query membership,
not invent explanations for unrecorded events. Users can navigate from behavior
to its live definition and change it.

Inspectors are extensible through the same owned view/action registrations as
other applications. External applications expose only what their protocols allow;
Techne cannot introspect Firefox's internals merely by hosting its window.

### Responsive, concurrent, recoverable

Remote requests, agent activity, indexing, subprocess output, and computation
must not monopolize interaction. A stuck Lisp evaluation must be interruptible;
a stalled or failed application runtime must not take down the compositor.

Behavior is replaceable; user state is not disposable. Unsaved content,
persistent tasks and external application windows survive termination or
restart of the application runtime, so restarting it is an ordinary development
operation rather than a loss. Hot reload is a convenience, not the only defense
against losing work.

Cancellation, resource lifetime, output limits, and failure are part of APIs.
Concurrency does not permit silent races between human and agent edits.

Failures are structured conditions with relevant operation context and explicitly
supported recovery choices. A human or authorized agent may resolve a conflict,
reauthenticate, or resume a task where its contract permits. This does not require
resumable arbitrary Lisp stack frames or make unsafe retries safe.

### History that makes experimentation safe

Undo, local document history, version control, and activity history are distinct
but connected. Recover earlier content independently of commits; inspect which
command or actor caused a tracked change; restore against current revisions
without overwriting subsequent work silently.

Supported live-configuration overrides can be compared with a reproducible
baseline and deliberately saved. Arbitrary heap mutations are not automatically
serializable configuration, and external side effects are not globally undoable.

History is bounded, local-first, and subject to exclusion, redaction, and deletion
policies. Sensitive content must be excludable before capture. Activity records
are explanations of tracked operations, not a promise of perfect system auditing.

### Semantic clipboard and durable links

Native targets support portable references with useful plain-text fallbacks.
Internal copying may preserve type, source identity and an explicit snapshot;
pasting chooses an appropriate representation for the destination. External
applications still receive standard clipboard formats.

Persistent links identify resources or entities independently of a particular
view or live process handle. Missing, moved, ambiguous, and inaccessible targets
are explicit states. Link resolution and snapshot disclosure respect authority;
a copied reference never transfers credentials or access rights implicitly.

### Attention and resumable work contexts

Tasks expose progress and attention needs independently: informational updates,
decisions needed, and explicitly permitted urgent interruptions. Deterministic,
programmable policies route these without requiring an LLM. Background activity
must not steal focus by default, and pending decisions remain discoverable.

A work context groups references to projects, views, tasks, agents, notes and
other relevant resources, plus navigation and attention preferences. Contexts
may overlap; they do not replace projects or duplicate ownership of resources.
Putting work down and resuming it restores references and reattaches to supported
tasks, while explaining what could not be recovered. It does not serialize
arbitrary browser, OS process, or Lisp heap state.

### Agents are native participants

Support existing coding agents and eventually native programmable agents.
Conversations, tasks, tool activity, approvals, context, and cancellation are
inspectable environment objects associated with projects and work contexts.
Agents use the same lenses, commands, recovery choices and attention model as
humans, within the capabilities actually exposed by their adapters.

Agents can work with structured resources and unsaved editor state where their
integration supports it. Edits must detect stale versions rather than overwrite
newer human changes. External agents that only support filesystem access must
have that limitation surfaced, not disguised as full buffer integration.

Reading data, sending it to a model, modifying it, and performing external
side effects require distinct authority. Core workflows remain usable without
an available model service.

### Remote operation is part of the runtime

A project has an execution context: node, directory, environment, and authority.
Files, processes, terminals, tools, version control, and agents honor it.
Applications should not each invent remote handling.

Local and remote resources share application-facing protocols, with explicit
capabilities and asynchronous operations. Uniform interaction does not hide
network latency, disconnection, unsupported operations, or uncertain outcomes.

Processes are genuine runtime objects, not local relay subprocesses pretending
to be remote children. Their contracts cover byte streams, separate stderr,
EOF, signals, PTYs, terminal resizing, output ordering, and lifecycle events.
A remote PID is backend metadata, not a local process identity.

### Data and workflow compatibility over Elisp compatibility

No Elisp compatibility layer on the critical path. Preserve existing files and
important semantics instead. In particular, unsupported Org syntax must survive
editing without being silently discarded or reformatted destructively.

## Language requirements

Techne's Lisp is a general-purpose language, usable outside Techne and embeddable
from Rust. Techne sets its priorities; Common Lisp, Clojure or R7RS completeness
does not. These are promises, not mechanisms; [PLAN.md](PLAN.md) chooses those.

- **Interactive development in any module.** The REPL, the editor, nREPL clients
  and remote evaluation choose the module they evaluate in. Inspection,
  completion and redefinition apply to that module.
- **Packages with lifetimes.** A package is the unit of loading, reloading and
  unloading. Everything it registers (commands, keymaps, hooks, subscriptions,
  tasks, processes, timers, views) has an owner and is removed with it. A
  failed load changes nothing visible; a successful reload switches atomically.
  Running work keeps the generation it started with. Unloaded code and data are
  reclaimed once unreachable. Redefining records, macros and inlined primitives
  has specified behavior.
- **Worlds.** A world is a domain of authority and resources. Code holds only
  the authority it is given: files, environment, processes, network, evaluation
  and host control are capabilities, and nothing can terminate the host. A world
  can have memory and CPU limits and can be stopped without harming others.
  Values cross between worlds as data or explicit capabilities, never as shared
  mutable objects. Worlds in one process contain mistakes; hostile code, such as
  agent-written programs, runs in a world in a separate OS process.
- **Bounded execution.** Interruption is a recovery tool that code may handle;
  termination by the host cannot be refused. Macro expansion and compilation
  are interruptible and bounded too.
- **A canonical data notation.** Versioned, deterministic and never evaluated
  when read, with bounded size. It carries messages between worlds, nodes, the
  compositor and agents, and serves for saved records and clipboard payloads.
  Persistent maps, sets and vectors have literal syntax in it.
- **Identity.** Stable identity hashing, identity-keyed tables and weak tables
  (ephemerons), kept distinct from persistent resource identifiers.
- **Bounded communication.** Channels have capacity limits and composable
  selection; overload behavior is explicit, never unbounded buffering.
- **Conditions and restarts are the recovery protocol.** Failures are typed
  conditions; recovery choices carry argument schemas, applicability and
  required authority, so a human or an agent can choose them.

## Required application scope

These are eventual replacement requirements, not one initial release.

| Area | Required scope |
| --- | --- |
| Editor | Strong text editing, familiar keyboard interaction, completion, search, undo, structured navigation, language tooling, diagnostics, builds |
| Org | Editing and folding, TODOs, agenda, habits, capture, linked notes/backlinks, Babel, tables, export, calendar synchronization |
| Version control | Native Git and Jujutsu workflows, actionable changes and history, local and remote repositories |
| Agents | Existing-agent integration, project context, task supervision; native programmable agents later |
| Email | Native reading, search, composition, attachments, and contextual actions; reuse existing backends initially |
| Shell and processes | Interactive terminals, ordinary commands, execution environments, persistent sessions, shell/editor handoff |
| Desktop | Techne-owned compositor: unified native views and application windows, global commands, focus/layout, multi-monitor operation, clipboard, notifications, locking, screen sharing |
| Browser | Existing browser as an application; semantic integration where browser APIs permit it |

Doom configuration is evidence of workflows, not a requirement to clone every
installed package. The precise Org/export/calendar and Git/Jujutsu feature
matrices must be derived during implementation from representative use cases.
Existing Emacs bindings are a starting point; exact keybinding compatibility
has not been established as a requirement.

## Selected engineering defaults

### Remote state: ownership, not a shared heap

- Each stateful object has one authoritative owner.
- Values can be transferred; remote state is accessed through explicit handles,
  commands, snapshots, and event subscriptions.
- Remote nodes can host live Lisp services. Inspecting, evaluating, and redefining
  code there is a first-class workflow.
- Computation can run beside the data, through explicit entry points and
  serializable arguments.
- No automatic transfer of arbitrary closures, stack frames, OS handles, or
  shared mutable object graphs.
- Stateful operations expose failures and version conflicts. Non-idempotent
  operations are not blindly retried after a connection loss.

The intended experience is one inspectable environment spanning machines, not
the fiction that a network is shared memory.

### Persistence is explicit

Tasks declare whether they are session-bound or persistent. Builds, terminals,
and agents may opt into persistence and survive a frontend disconnect/restart
while their owning node remains alive. Applications choose sensible defaults.

A persistent node-side supervisor owns such tasks independently of an SSH
connection. Reattachment uses stable task identity and explicit replay/snapshot
semantics. Retained output is bounded, with any gaps reported.

Network interruption is distinct from task termination. Node reboot is also a
different event: preserve task records and declared restart policies, but do
not promise to checkpoint and resurrect arbitrary running processes.

Concurrent multi-computer editing and migration of live computations are not
initial requirements. Reattachment is not the same as collaborative editing.

### Rust mechanisms, live policy

Lisp is the default for application behavior. Rust is used where it enforces
correctness or is measured to be necessary: OS and Wayland protocol boundaries,
rendering machinery, low-level input, document primitives and persistence
integrity, scheduling, transport, isolation and enforced limits, and measured
hot paths. Live code owns commands, modes, views, layout policy, keymaps,
workflows, and agent composition. Rust-owned rendering still exposes
programmable presentation.

Trusted packages share a world for fluid development. Slow work moves off its
interactive executor. Separate processes protect the compositor and contain
externally hosted or untrusted execution; not every package needs its own
service.

### Responsiveness budgets

Targets, measured on the daily hardware and reported as p99 under named
workloads, with overload behavior stated rather than hidden:

- Input event to command dispatch in the application runtime: under 4 ms.
  Dispatch to a ready frame: under 8 ms. Missed frames are counted.
- GC pause in an interactive world: under 4 ms.
- Restarting the application runtime to a usable session with unsaved content
  restored: target set by the first probe, then tracked.
- Runtime startup: under 100 ms warm. Cold full-session startup is measured
  separately; fonts, GPU setup and restored projects do not block input.

### Reuse engines; own the interaction

Reuse mature compositor libraries, language servers, terminal engines, Git/JJ
commands, mail backends, browser engines, and agent runtimes where appropriate.
Their user-facing integration is native to Techne.

The compositor is Techne's own. It starts from an attributed fork of EWM's
Smithay compositor, taking its backends, protocols, input and screencasting
where they fit and replacing everything shaped around Emacs. Owning it lets
Techne change the compositor's contract freely: keymaps resolved without a
round trip to Lisp, layout as validated transactions, windows as typed and
inspectable targets, capability-scoped capture and input for agents, and
windows that survive an application runtime restart. It is a separate process
from the application runtime.

The runtime is techne-vm, Techne's own language core below Steel's parser
([runtime/TECHNE-VM.md](runtime/TECHNE-VM.md); the Steel modernization that
preceded it is in `runtime/history/`). It provides asynchronous Rust embedding,
NaN-boxed values, a low-pause generational collector and a JIT whose interpreter
remains the semantic reference. Runtime changes keep that reference, are
differentially tested and measured, and must respect live redefinition,
generations and Rust host-call contracts. Compile latency and source information
matter alongside execution speed. Rust is the systems language.

## Deliberate non-goals for the initial system

- Executing existing Emacs Lisp packages.
- Writing a browser engine, mail server, kernel, or distribution.
- A transparently distributed Lisp heap or automatic closure migration.
- Collaborative multi-writer sessions or live task migration between machines.
- Universal rollback of external effects such as email sending or remote commands.
- Perfect hot upgrades of arbitrary stateful packages.
- A separate VM or service for every small feature.
- Treating in-process worlds as a security boundary against hostile code.
- Common Lisp, Clojure or full R7RS compatibility.
- A universal graph database, global event-sourced architecture, or separate
  workflow engine solely to support links, history, or recipes.
- Automatic snapshots of secrets, arbitrary application state, or the entire heap.

## Evidence and references

- Shared discussion: https://chatgpt.com/share/6ac271a9-7f74-83ed-98a9-4a28c41fcaf8
- `../doomconfig/init.el` and `../doomconfig/config.org`: workflow evidence.
- `../ewm/README.md` and `../ewm/compositor`: origin of the compositor fork
  (GPL-3.0-or-later, as is Techne).
- `../tramp-rpc/README.org`: remote operations and transport lifecycle reference.
- `../emacs` commit `1f80e44c73d`, documented in
  `etc/MANAGED-PROCESS-EXPERIMENT.md`: backend-managed process semantics.

Interaction precedents (design references, not dependencies or performance claims):

- [Glamorous Toolkit inspectors](https://book.gtoolkit.com/inspector-6k9vwubemen05fcxg4kv6wi6b):
  contextual object views and actions.
- [Zed multibuffers](https://zed.dev/docs/multibuffers) and
  [Emacs indirect buffers](https://www.gnu.org/s/emacs/manual/html_node/emacs/Indirect-Buffers.html):
  editable source-backed views.
- [Kakoune selections](https://kakoune.org/why-kakoune/why-kakoune.html) and
  [Transient](https://www.gnu.org/software/emacs/manual/html_mono/transient.html):
  composable selection/action and discoverable argument construction.
- [JetBrains Local History](https://www.jetbrains.com/help/idea/local-history.html):
  recovery independent of version-control commits.
- [SLIME](https://common-lisp.net/project/slime/doc/slime.pdf):
  interactive inspection and explicit recovery choices.
- [Plan 9 plumber](https://9fans.github.io/plan9port/man/man4/plumber.html):
  routing meaningful content to appropriate tools.

Semantic recipes, shared agent lenses, attention routing and resumable work
contexts extend these precedents to the whole environment; they are proposals
for Techne, not claims that the referenced tools implement them all.

The Emacs experiment is a pipe-only prototype, not evidence of production PTY,
reconnection, or scheduling support. Its repository is read-only reference
material for this planning work; no Emacs contribution is proposed here.
