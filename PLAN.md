# Techne: engineering direction and staged plan

This plan implements [REQUIREMENTS.md](REQUIREMENTS.md). It chooses a direction
without treating untested properties as established facts. There are no
delivery estimates yet: the probes should establish feasibility and expose the
expensive parts before estimating the full application scope.

The governing principle: make behavior replaceable without making user state
disposable.

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
 Techne compositor   Local services   Remote node services
```

This is a conceptual separation, not a prescription for one process per box.

- **Compositor process (Techne-owned, Stage 4):** owns Wayland clients, DRM
  and input, rendering, the last applied layout, compiled global keymaps, and
  capture enforcement. Retains a minimal recovery/control path independently of
  application Lisp. Layout policy arrives as transactions.
- **Frontends:** draw presentation snapshots and send input back (EDITOR.md,
  section 6): GPU in a window or as the desktop, terminal, and a remote browser
  frontend. The runtime does not depend on which one is attached.
- **Application runtime:** a restartable process hosting the user's trusted
  world, hosted by Rust. Uses asynchronous services and background workers. No
  blocking remote calls in redisplay, completion, or input handling. An edit
  journal written beside it makes unsaved content survive its termination.
- **Sandboxed worlds:** separate processes for untrusted code such as
  agent-written programs, reached through the node protocol.
- **Node service:** manages resources and processes locally or remotely through
  the same application-facing contracts. A local fast path need not serialize
  every call. Only remote deployments require the transport boundary.
- **Optional node-side Lisp host:** supports inspectable remote services and
  explicit computation near data. Its lifetime can outlive a frontend connection.
- **External tools/agents:** keep their own processes, with adapters mapping
  available capabilities into Techne objects.

Initially target Linux/NixOS for the desktop. Broader remote-platform support is
an extension of the node service, not a reason to delay a useful Linux path.

### The compositor (Stage 4)

A new internal model rather than EWM's, whose core is shaped around Emacs
frames; EWM pieces are reused where they prove sound. Mechanism in Rust, policy
in Lisp:

- **Mechanisms only.** Surfaces, rectangles, focus, input routing, atomic
  layout transactions, animations, capture enforcement and a minimal emergency
  control path independent of Lisp. The compositor keeps the last applied
  layout, so other applications' windows outlive the application runtime; the
  runtime's own views are client surfaces that die with it, so the compositor
  shows placeholders until the restarted runtime recreates them.
- **Invariants stay in the compositor.** Locking, focus authorization, capture
  enforcement and emergency input are enforced there whatever Lisp policy
  requests; the mechanism/policy split is not a security boundary.
- **Window management is a Lisp package** (EDITOR.md, section 8): a window tree
  and placement rules compute rectangles. The default gives the Emacs/EWM feel;
  anyone can write another against the same primitives.
- **The policy channel is a Wayland protocol extension** on the runtime's own
  connection, which it needs anyway to draw native views. One connection does
  not make a layout change atomic by itself: a layout transaction names the
  surfaces taking part, waits for their commits, and applies on all of them in
  one frame, with a timeout; it never waits indefinitely on an arbitrary
  client. (river has moved its window manager into a separate client through a
  protocol; here the compositor also keeps the layout.)
- **Two layers of keymaps.** Global keymaps are compiled in the compositor into
  a prefix state machine: bound keys become commands for the runtime, unbound
  ones go to the focused surface, and a small built-in set works while the
  runtime is down. Editing keymaps stay in the runtime.
- **Inspectable decisions.** Windows, outputs, views and capture sessions are
  typed targets; routing and layout decisions record the keymap and layout
  generation that produced them.
- **Agents under capability.** Metadata, pixel capture, input injection and
  disclosure to a model are separate, target-scoped, revocable grants checked
  at execution. Injected input is bound to its target, never to whatever has
  focus. Lock surfaces and recovery controls are outside agent authority.
- **Lens outputs.** Screen sharing can export an offscreen output containing
  only an authorized lens, not a crop of the desktop.
- **Replayable tests.** Input decisions, transactions and acknowledgements are
  recorded for headless replay.

`crates/techne-compositor` holds an attributed import of EWM's compositor with
its Emacs boundary removed and a nested backend added; it is parked until
Stage 4. Its core (state, the Emacs-frame layout, focus handoffs, keyboard
capture) will be replaced; its periphery (DRM and output handling,
screencasting and portals, protocol implementations, input device
configuration, the input-method relay, render helpers, the headless test
fixture) moves into the new core with its tests. Techne, like EWM, is
GPL-3.0-or-later; imported files keep their copyright notices.

### Worlds, packages and the language

A world is one techne-vm instance: its own heap, module graph and granted
capabilities. Natives such as file, environment and process access are granted
per world; `exit` asks the host instead of ending the process. Worlds exchange
data notation and remote handles exactly as nodes do, so an in-process world is
a node without a transport. Hostile code gets a world in a separate process.

A package is a set of modules with an owning scope (after Racket's custodians)
and a generation. Loading stages the new generation with its registrations
unpublished, then publishes atomically; failure leaves the previous generation
in place. Success shuts the previous generation's scope: its tasks are
cancelled and its processes killed. Work that must outlive a reload moves to a
longer-lived scope first, and keeps its generation, as existing closures do;
upgrade points are explicit indirections (registries). Grace periods for a
retiring generation's tasks can come later, if a workload needs them.
Unloading shuts a generation now; its code is reclaimed when unreachable.

Unsaved content survives the runtime through an append-only edit journal kept
by the Rust document primitives, not through a separate document process.
Introduce a separate document authority only if the journal proves insufficient.

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
a spawned shell can escape a host-API allowlist. Worlds hold only granted
capabilities, but claim isolation only where runtime or OS boundaries actually
enforce it: against hostile code that means a separate process.

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
| Live application language | techne-vm, Techne's own runtime ([runtime/TECHNE-VM.md](runtime/TECHNE-VM.md), [runtime/R7RS.md](runtime/R7RS.md)) |
| Compositor | Techne's own on Smithay: a new core reusing EWM's backends and protocols; niri as engineering reference |
| Rendering/text | `wgpu`, `cosmic-text` and `swash`, after neomacs (EDITOR.md, section 7); a terminal frontend |
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

### Stage 0 — Runtime and process contracts (done)

techne-vm replaced everything below Steel's parser; design, results and the
gate status are in [runtime/TECHNE-VM.md](runtime/TECHNE-VM.md). The Steel
modernization that preceded it is in `runtime/history/`. Two of the four
platform probes are done: the native process contract (`crates/techne-process`)
and persistence through node sessions (`crates/techne-node`). The other two
move on: the compositor boundary to Stage 4, the interaction contracts to
Stage 2.

No further JIT speed work until a Techne workload measures a need. The known
gaps to Chez (bintrees 1.7×, hof 1.3×) do not block anything.

### Stage 1 — Language foundations and the editor probe

Two workstreams run side by side, so the language is shaped by a real consumer
rather than by Common Lisp completeness. A language step is done only when its
acceptance test passes. Steps not needed by the probe or by Stage 2 wait.
Workstream A owns `crates/techne-vm` (with `techne-node` for A1); workstream B
owns new crates.

**Workstream A — language.** Each step is one change. Every step adds its
tests to the Scheme suites (`crates/techne-vm/tests/suites`), which run in all
execution modes in CI.

0. **Conformance baseline** (done: [runtime/R7RS.md](runtime/R7RS.md)).
   Chibi-scheme's R7RS suite and the r7rs-benchmarks programs run in CI;
   `expected-failures.txt` records what fails. Fix the bugs they found
   (symbol and string printing, `(_ . args)` patterns, `(... ...)` escapes,
   `list?` and `equal?` on circular or shared structure, continuations as
   procedures, I/O errors reported as end of file) and decide each deviation
   from R7RS once, in writing: `/` on integers (decided: exact, with
   ratios), strings (decided: UTF-8, changed in place at any size through
   an indirection once they change size; `string-ref` stays, with
   documented cost), complex numbers (decided: built in, off the fast
   paths), escape-only continuations. Support
   R7RS `define-library` and `import` over the module system, so portable
   libraries (SRFI reference implementations) load unchanged instead of
   being rewritten.
   *Acceptance:* every entry left in `expected-failures.txt` is a documented
   deviation; `read` gives back every datum `write` prints.
1. **Evaluate in a chosen module** (done). REPL, nREPL, `node-eval` and Lisp
   `eval` take a module; completion, `help` and definition lookup follow it.
   *Acceptance:* two modules define the same name; two sessions inspect and
   redefine their own binding without touching the other.
2. **Worlds with granted capabilities** (done). A VM is built from a pure core
   plus granted native sets (files, environment, processes, network, evaluation
   and loading, host control). `exit` requests termination from the host; module
   loading goes through a granted loader.
   *Acceptance:* a restricted world cannot reach files, environment, processes
   or host termination through direct calls, imports or values handed to it.
3. **Bounded channels and select** (done). Capacity in messages and bytes; close,
   cancellation and rendezvous semantics; `select` commits exactly one winner
   and deregisters the losers. A few scheduler classes with fairness, not
   arbitrary priorities.
   *Acceptance:* a flooded channel with stalled consumers stays bounded;
   cancelling removes waiters; select between data and timeout never loses or
   duplicates a delivery.
4. **Identity and weak tables** (done). Identity hashes stable across
   nursery moves, `eq`/`eqv`/`equal` hash tables with any key, ephemeron
   weak-key tables.
   *Acceptance:* lookups survive minor and full collections; a weak table
   whose value refers to its key does not keep an unreachable cycle alive.
5. **Owned scopes** (done; no finalizer fallback yet). Custodian-like
   scopes own commands, keymaps, hooks, subscriptions, tasks, processes,
   timers and channels; shutting a scope removes them. Documents and
   persistent tasks can move to a longer-lived owner. Finalizers are a leak
   fallback, not the cleanup protocol.
   *Acceptance:* loading and unloading a sample mode a hundred times leaves no
   registrations, tasks or processes behind; late callbacks from an unloaded
   mode cannot affect its replacement.
6. **Packages and generations** (done). Staged load, atomic publish,
   previous generation kept on failure and shut after success. Documented
   redefinition of records (new type identity unless migrated), macros
   (dependents re-expanded) and primitives (sealed; shadowing instead of
   redefining what is inlined). JIT code is tagged with its generation.
   *Acceptance:* a failing reload changes nothing visible; a successful one
   switches commands, while a task moved to a longer-lived scope finishes on
   its own generation and the others are cancelled.
7. **Reclaim code.** Bytecode, constants, globals, JIT code and debug metadata
   of retired generations are freed when unreachable; delayed JIT results for
   retired code are discarded. A "why is this retained" query exists.
   *Acceptance:* a thousand load/use/unload cycles plateau in memory, and
   retained closures stay safe.
8. **Execution and memory limits.** Per-world heap limits and per-task CPU
   budgets, also covering expansion and compilation; termination by the host
   that code cannot catch, after a bounded cleanup. OS limits back this up
   for sandboxed processes.
   *Acceptance:* an infinite loop, an allocation flood and code that catches
   interrupts each cannot stall the editor; stopping their world leaves other
   worlds, documents and the compositor working.
9. **Data notation and persistent collections.** Persistent maps, sets and
   vectors with literals; a versioned, non-evaluating notation with bounded
   size and depth and allowlisted tags. Node and compositor messages move to it.
   References are data, never authority.
   *Acceptance:* shared command and layout fixtures round-trip byte for byte;
   executable or oversized payloads are rejected.
10. **Recovery contracts.** Typed condition hierarchy; restarts with argument
    schemas, applicability, expiry and required authority. Deferred recovery is
    operation state, not a captured continuation. Built with the first
    documents in Stage 2.
    *Acceptance:* a stale document edit offers recovery; after another edit or
    a revoked capability, the old choice is revalidated and refused or redone.

General-purpose language work, each step scheduled when a slice or package
needs it, not before:

11. **Bytevectors and binary ports** (done: `crates/techne-vm/src/bytes.rs`,
    `ports.rs`, `crates/techne-process`). Byte I/O for processes, files and
    protocols; UTF-8 decoding across buffer boundaries with an explicit policy
    for invalid input; partial reads; I/O errors distinct from end of file.
    Done ahead of the slices that need it, so that the data notation (step 9)
    has bytes from its first version.
    *Acceptance:* arbitrary bytes round-trip exactly through files and
    processes; a UTF-8 sequence split across reads decodes once.
    *Tested* in the R7RS suites and `crates/techne-process/tests/probe.rs`.
    `utf8->string` refuses invalid UTF-8; `process-read` decodes text with
    invalid bytes as U+FFFD, and `process-read-bytes` gives the bytes.
12. **Text: cursors and regular expressions** (done: SRFI 130 over native
    cursors; SRFI 115 over `regex-cursor`, searching strings and documents'
    ropes, `search-text-regexp`). String cursors (SRFI 130 style)
    for linear traversal and slicing; compiled regular expressions with
    captures and replacement over strings and over ropes without flattening
    them (Rust's `regex-cursor`, as Helix does); an `rx`-like s-expression
    syntax as a plain library. Searches are interruptible.
    *Acceptance:* traversing a non-ASCII string is linear; a match spanning
    rope chunks is found; a search over a large rope stops on interrupt.
13. **Sequences and iteration.** A small sequence protocol (next element,
    end, early exit with cleanup) and `for`, `for/list`, `for/fold` over
    lists, vectors, strings, hash tables, rope lines and matches, without
    building intermediate lists. No general lazy-stream framework.
    *Acceptance:* iterating a rope's lines uses bounded memory and can be
    suspended in a task and exited early with its cleanup run.
14. **Owned advice.** Around, before and after advice on named functions and
    commands, owned by the package that adds it (step 5), ordered, listed by
    the inspector and removed on unload; method combination on generics only
    when a consumer needs it. Advice cannot reach sealed primitives (step 6).
    *Acceptance:* unloading a package removes exactly its advice; the
    inspector shows who changed a function.
15. **Procedural macros.** Explicit-renaming macros on the existing
    Clinger-Rees renaming, when an authoring form (`define-command`,
    `define-mode`) cannot be written with `syntax-rules`. Expansion is
    bounded, reports macro origins in errors, and the language server learns
    binding forms from expansion.
    *Acceptance:* an authoring macro stays hygienic across renamed imports,
    and its errors point at the user's source.

The schemas of step 10 are a small shared vocabulary for values (types,
ranges, choices, records), used by command arguments, options and restart
arguments alike, so that a human and an agent invoking a command get the same
validation; not one semantic system for options, recovery and serialization.

**Workstream B — editor probe** (EDITOR.md). Narrow vertical slices: every
slice ends with something visible or usable, and general machinery (keyed
deltas, layers, projections) is added only when a slice needs it.

1. **Text core and command semantics** (done: `crates/techne-text`,
   `crates/techne-editor`, `lisp/editor`). Rust, exposed to Lisp: file round trip,
   rope, revisions, anchors with insertion affinity, transactions with actor,
   undo that refuses on conflict, the edit journal. A few editing commands run
   through both key profiles in a headless harness.
   *Acceptance:* differential fuzzing against a plain string model, including
   anchor positions; a process killed mid-edit recovers every acknowledged
   transaction and discards a torn last record; the same scripted scenario in
   both profiles gives the same document, selections and undo grouping, and the
   same result when cancelled midway.
2. **Minimal GPU editor** (done: `crates/techne-window`). One view, full
   snapshots, insertion and deletion, shaping with proportional fonts,
   wrapping, selection, scrolling by anchor.
   *Acceptance:* on the daily hardware, a 100k-line file, a file with one 1 MB
   line and a file of mixed-width Unicode all scroll and edit within the
   budgets (p99 keystroke to frame, REQUIREMENTS.md); resizing keeps the scroll
   anchor; a click made against a stale snapshot is re-resolved or rejected,
   never applied to the wrong text.
   *Measured* with `crates/techne-window/bench.sh` (Ryzen 9 8945HS, Radeon
   780M, headless sway), p99 key to frame: 1.6 ms on 100k lines, 3.5 ms on
   the 1 MB line, 1.6 ms on mixed Unicode, 4.4 ms on 100k lines with a busy
   Lisp task (key to snapshot 3.1 ms there, 0.2 ms otherwise).
3. **Small terminal frontend** (done: `crates/techne-term`). The same view in
   cells: grapheme widths, wide characters, column stops, key limits.
   *Acceptance:* the headless terminal tests show the same semantic state as the
   GPU frontend for a scripted session; unsendable chords are reported, not
   silently lost.
4. **Two views and the live loop** (done: panes in the presentation
   protocol, `lisp/editor/api.scm` as the library `(techne editor)`,
   `lisp/editor/examples`, `techne_editor::host`). Two views of one document;
   evaluate in the file's module, invoke, inspect the result, redefine, jump
   to definitions.
   *Acceptance:* redefining a command changes the next invocation without a
   restart; edits in one view appear in the other with each view's selections
   and scroll anchor intact; a runtime crash recreates the window with unsaved
   text restored. The first two canonical extension examples (EDITOR.md,
   section 11: a command on the region, a minor mode with a keymap and a
   highlighting layer) are written with the authoring layer, each about as
   short as its Emacs Lisp equivalent, and reload and unload cleanly.
   *Tested* in `crates/techne-editor/tests/live.rs`, the terminal tests and
   the window/terminal parity test (split, focus, click in the other pane).
   A restarted runtime also brings back the session: the files it had
   open, with their unsaved edits, its panes, carets, scroll anchors and
   focus (`techne_editor::host`; lenses and views are not kept).
   The inspector (`lisp/editor/inspect.scm`; `C-c c k` or Geiser's `C-c
   C-d C-d` on a name, `C-c M-i` on the last result) is a structured view:
   type, documentation, definition (a location), the command's keys, and
   the value's parts, each inspected in turn with RET, `l` going back.
   *Left open:* the inspector does not yet show owners (package, scope)
   or what a binding shadows.
5. **Minibuffer and one lens** (done: `lisp/editor/minibuffer.scm`,
   `targets.scm`, `buffers.scm`, `lens.scm`, `techne_editor::lens`).
   Completion with candidate targets and actions; one editable search lens;
   keyed deltas and layers as these need them.
   *Acceptance:* open files, switch buffers, split, act on a candidate; an edit
   through the lens lands in its source documents; an edit whose source
   changed underneath is refused with an explanation. The other two canonical
   examples (a structured view with targets and actions, a completion source
   with preview) are as short as their Emacs Lisp equivalents.
   *Tested* in `crates/techne-editor/tests/minibuffer.rs` and `lens.rs`, the
   lens's unit tests and the terminal tests. The minibuffer is Vertico-like
   with Orderless matching, Marginalia-like annotations and Consult-like
   preview; C-; acts on a candidate's target (Embark), and C-c C-e exports
   location candidates as a lens. The search lens (M-s o) checks each
   excerpt against its source's history: edits of generated text, across
   excerpts, or of an excerpt whose source changed are refused. A
   structured view is a presentation of generated rows only, so read-only.
   Since the architecture review, rows are keyed (EDITOR.md, section 2, "As
   built"): a view made again keeps carets on their rows, and lenses keep no
   history of their own.
   *Left open:* snapshots are whole (no deltas yet; nothing needed them); a
   structured view aligns its columns with spaces, in the text, rather than
   with column stops; no minibuffer history;
   typing more narrows the last matches (about 1 ms once a few hundred are
   left), but the first key and deleting scan every candidate again (on the
   lines of a 100k-line file, release build: 80 ms for the first key, 61 ms
   for DEL, 28 to 42 ms per key while many still match; far over the
   keystroke budget, slice 7); the search lens covers open buffers, not a
   project's files.
6. **Buffers, modes and options** (done: `lisp/editor/modes.scm`,
   `options.scm`; EDITOR.md, section 1, "Buffers, modes and options").
   What differs between buffers was properties hung on
   documents, keymaps looked up in five places with five orders, and minor
   modes on for the whole session; this replaces them with buffer records,
   major modes with one parent, declared options resolved by cell, and one
   key resolver. Then the first options shown by frontends: line numbers
   and the `~` past the end of the text, as Arthur's Doom has them
   (absolute numbers in programming, text and configuration buffers; `~`
   in programming and text buffers). In order, one change each:
   1. buffer records and major modes, keymaps per input state and one
      resolver; the editor's own buffers (*Messages*, lenses, views, the
      inspector, itl) become modes, and the Scheme keys move to
      `scheme-mode`;
   2. options: `define-option`, `set-option!`, `option`,
      `explain-option`, `describe-option`; minor modes on per buffer as
      boolean options; read-only as one;
   3. display settings in the presentation protocol, drawn in a gutter by
      both frontends;
   4. hooks as owned named events, the first after each key
      (which-key's).
   *Acceptance:* `C-M-x` and `C-c C-k` are undefined in a text buffer, as
   in Arthur's Emacs (where `C-x C-e` stays global); RET evaluates in
   itl in both profiles; the inspector has the keys of every structured
   view; `todo-mode` on for `prog-mode` highlights in a Scheme buffer and
   not in *Messages*; line numbers and `~` appear where Arthur's Doom shows
   them, in both frontends; redefining `pane-status` while running changes
   the next mode line.
   *Tested* in `crates/techne-editor/tests/modes.rs`, the terminal's
   gutter test and the window/terminal parity test, which now runs with a
   gutter.
   *Deferred:* saving options changed while running, a Customize-like
   interface, settings per view, choosing a mode from a file's first line,
   and a mode's setup and teardown (until a language server needs them).
7. **Keystroke budgets in CI.** The runtime's share of a keystroke (input
   to the snapshot showing it) is held to the 4 ms budget of
   REQUIREMENTS.md by CI, not only by benchmarks run by hand on the daily
   hardware (slice 2). Wall time on a shared runner is noise, so CI judges
   instructions, which are deterministic; about 28 million make 4 ms on the
   daily hardware (the search's first key: 567 M instructions, 80 ms). In
   order:
   1. named workloads (done: `crates/techne-editor/examples/keys.rs`):
      typing and moving in a 100k-line file and a 1 MB line, both
      profiles, the minibuffer's first key, narrowing and deleting over
      100k lines, `M-x`. CI counts instructions per key for each
      (`runtime/bench/icount.sh`), flags a 5% regression as for the VM,
      and shows the wall times;
   2. a hard budget: a workload over 28 M instructions per key fails CI,
      unless it is listed as a known violation with the slice that will
      fix it. The minibuffer's are listed until filtering is incremental
      (slice 5, left open);
   3. fuzzing: seeded random key sequences drawn from the keys bound in
      each profile and mode, over a corpus (large files, long lines,
      mixed Unicode, Scheme with deep nesting, many buffers, an open
      minibuffer and lens), run in CI with fixed seeds and longer with a
      new seed nightly. Keys slower than ten times the budget in wall time
      are candidates; each is replayed under instruction counting (its
      prefix, then with the key), the prefix shrunk while it stays over
      budget, and what remains is reported as a workload to add. A Lisp
      error escaping a command, or a panic, fails it too.
   *Acceptance:* a change that makes a key in the corpus exceed the budget
   fails CI with a short key sequence reproducing it; the known violations
   are listed in one place, each with its plan.

Language steps the slices need: none for slices 1 to 3 beyond what exists;
for slice 4, identity and weak tables (4) for the inspector, and owned scopes
(5) with enough of packages (6) that its minor mode reloads and unloads
cleanly; regular expressions (12) and iteration (13) as the canonical
examples need them to be as short as their Emacs Lisp equivalents.

**Exit:** develop Techne's Lisp in Techne for a working session (EDITOR.md,
section 12), in both key profiles and both frontends; crash the runtime and
recover unsaved text. Keystroke, GC and restart budgets are measured and
recorded.

### Stage 2 — A daily-use hosted slice

Build the shared primitives in vertical slices, running hosted in a window,
and in a terminal once that frontend exists.

- Document editing, save/reload/conflict handling, undo, multiple views and
  bounded local history, with sensitive-resource exclusions applied before
  capture.
- Typed selection and completion, contextual actions, argument controls,
  optional previews and actionable results. One lens: editable search excerpts.
- Object microscope, ownership/source lookup, REPL and evaluation in modules.
- Local/remote project context, resource browsing, search and terminals.
- Named commands with argument/result schemas, also exposed to agents through
  an MCP adapter: inspection, versioned reads and staged edits first, never
  unrestricted evaluation. MCP is an adapter, not the internal command model.
- Operation identity, structured conditions with recovery choices, and a
  discoverable list of pending decisions.
- Packages with owned registrations; declarative configuration overrides that
  can be compared with the baseline and saved.
- Package distribution: packages from source pinned in a lockfile, declared
  dependencies and capabilities, interfaces marked stable or experimental
  (EDITOR.md, section 11).

**Exit:** open a remote project, refine search results into a lens, edit through
it, run a build and act on its output. Inspect and redefine the responsible
command. An agent applies a staged edit over MCP while a human edits the same
document, without silent loss. Recover an uncommitted edit from history and
restart the runtime without losing unsaved work. Nothing blocks the UI.

### Stage 3 — Agent and version-control development loop

- Integrate one existing agent with native conversation/task views and explicit
  mapping of its actual capabilities; avoid maintaining several shallow adapters.
- Provide revision-aware editor operations, reviewable changes and activity logs
  linked to document history. Supply lens snapshots as agent context when supported.
- Build actionable Git/Jujutsu status, diffs, history and common operations.
- Add language-server diagnostics/navigation and project-scoped execution.
- Run remote tools and agents in the project's remote environment; run
  agent-written programs in sandboxed worlds.
- Use source-backed lenses for diff/search/diagnostic views where meaningful;
  historical content remains read-only unless an explicit apply action exists.
- Route agent approvals and failures into the pending-decisions and recovery UI.

**Exit:** carry out a real local and remote code-change workflow with an agent,
review the result and use version control, including a concurrent human edit and
a disconnect/reconnect case without silent data loss. Inspect the origin of a
tracked change, restore it safely, and resolve an approval without focus theft.

### Stage 4 — Become the desktop

This stage builds the compositor (section 1, "The compositor") and makes it
the daily session.

- Host native views and ordinary Wayland applications in one layout/navigation
  system; expose application windows as actionable targets.
- Global command routing and focus through compiled keymaps; standard
  clipboard with text formats. Present pending decisions as notifications rather
  than introducing a second task/approval system.
- Extend the microscope to native UI ownership, keymap decisions, layout rules
  and external-window metadata; do not claim inspection of opaque app internals.
- Validate multi-monitor hotplug/scaling, input configuration, locking, suspend/
  resume, fullscreen, and screen sharing on the actual target hardware.
- Capability-scoped capture and input for agents; lens outputs for sharing.
- Add shell/editor handoff and browser integration incrementally.

**Exit:** safely use Techne as the daily compositor, including recovery after a
failed live-code experiment and a runtime restart that keeps every window.
Inspect why a native view is positioned as it is, and handle background
decisions without unsolicited focus changes. Keep the old desktop available as
a fallback. Ordinary Emacs can remain a guest application until native Org/mail
meet the replacement requirements; this does not require an Elisp compatibility
layer.

### Stage 5 — Native Org and personal information workflows

Org is a substantial workstream, not a small parser feature. Preserve source text
as authoritative; syntax trees and indexes are derived and rebuildable.
`lisp/org` is an early start on dates, parsing and agenda queries.

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
and Org through shared references and actions; add patch workflows where
needed. Owning the mail storage/sync engine can wait. Agenda and linked-note
results become lenses using explicit heading operations; inspectors can explain
query membership.

**Exit:** complete the established Org and email workflows on representative data,
with supported behavior checked against references and unsupported syntax kept
intact. Follow an email reference into a note, inspect an agenda match and act
on it at the source; unavailable links must be explained.

### Stage 6 — Recipes, contexts, semantic transfer and deeper automation

These build on the primitives above once real workflows ask for them:

- Semantic recipes: sequential, parameterized compositions of named commands,
  validated on replay as described in section 1.
- Resumable work contexts that restore references, navigation, desktop layout
  and task reattachment without replaying side effects.
- Semantic clipboard and durable links through the shared reference resolver.
- Programmable attention routing over the pending-decisions model.
- Native agents composed from the same task, command, context and permission
  APIs; remote live Lisp services with explicit ownership and generations.
- Supported state migrations across package generations.

**Exit:** a user or authorized agent records and safely replays a recipe,
resumes an interrupted context after a restart, and develops and inspects a
remote service live, without a separate automation framework or an implicit
distributed heap.

## 4. Coherence and dependency map

Applications consume shared primitives instead of implementing parallel systems.
Core means the contract is foundational, not that its complete UI ships at once.

| Capability | Shared foundation | First usable slice | Later extension |
| --- | --- | --- | --- |
| Live replacement | Worlds, packages, owned scopes, edit journal | Stage 1 probe | Stage 6 state migration |
| Object microscope | Identity, ownership, registrations, source metadata | Stage 2 commands/resources | Stage 4 UI/layout; Stage 5 queries; Stage 6 remote services |
| Editable lenses | Versioned documents, source anchors, typed targets | Stage 2 search excerpts | Stage 3 diagnostics/diffs; Stage 5 Org views |
| Safe history | Document revisions, operation identity, retention policy | Stage 2 document/config recovery | Stage 3 agent-change attribution |
| Interactive recovery | Conditions and restarts, task state, authorization | Stage 2 recovery choices, pending decisions | Stage 3 agent approvals; Stage 6 attention routing |
| Agent access | Command schemas, capabilities, sandboxed worlds | Stage 2 MCP adapter | Stage 3 agent loop; Stage 4 capture/input |
| Semantic recipes | Named commands, arguments/results, task executor | Stage 6 | — |
| Semantic clipboard/links | Reference resolver, typed targets, disclosure policy | Stage 4 plain clipboard | Stage 6 semantic transfer |
| Work contexts | Saved references, task supervisor, navigation | Stage 6 | — |

Persist documents and declared records, not a universal object heap. Compose
commands, not simulated UI interaction. Reattach tasks, do not replay their side
effects. Resolve references, do not turn them into bearer credentials. These rules
keep remote operation, live programming and the new interaction ideas compatible.

## 5. Validation and scope discipline

- Use end-to-end workflow acceptance cases plus focused contract tests, rather
  than matching package counts or generating a test for every removed feature.
- Measure the responsiveness budgets in REQUIREMENTS.md from the Stage 1 probe
  onwards, then under output floods, slow networks and agent workloads. CI
  holds the keystroke budget in instructions, with fuzzed key sequences
  (Stage 1, slice 7).
- Kill and restart the application runtime as a routine test, not a disaster
  drill: windows, unsaved content and persistent tasks must survive it.
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
