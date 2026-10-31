# Commands, operations and recovery

Status: design, nothing built. It implements REQUIREMENTS.md, "Embark is
foundational" (commands describe their arguments, effects and
documentation; prompts are argument acquisition), "History that makes
experimentation safe" and "Agents are native participants", and the
language requirement "Conditions and restarts are the recovery
protocol"; PLAN.md, "Commands and targets precede applications" and
"History, recovery and attention share operation identity", Stage 1
language step 10 (recovery contracts) and Stage 2's operation identity,
pending decisions and MCP adapter. Discussed with Muse Wonder and
GPT-6.1 Sol, and reviewed by GPT-6.1 Sol.

A command today is a procedure of a session and a count
(`lisp/editor/dispatch.scm`). What else it needs it reads from the
session (point, the region, the prefix) or asks for in the minibuffer,
whose callbacks run after the command has returned: `lsp-rename` has
returned before the name is typed, and its edits are made later still,
in a task, when the server answers. Nothing records that it ran, with
what, or what it changed; an error it raises becomes a line in
*Messages*, with nothing to act on. So an agent cannot call a command
with arguments, nothing can run one again with different ones, a
failure offers no way on, and a change in a document cannot say what
made it.

This document makes every run an **operation**, a record of what ran,
for whom, with what, and what came of it, which is a target like any
other; and gives commands a **contract**, the arguments they take as
data and the effects they may have.

## Four identities, kept apart

- **Principal**: who acts. The user (a session's actor, `"user"`), an
  agent, a service through its grant. Undo and authority are a
  principal's: this is the `actor` documents record now.
- **Interaction**: acquiring intent: keys, M-x, the minibuffer's
  prompts, a form, modal operator-pending. Aborting one leaves nothing
  but its trace (below).
- **Operation**: one run of one command, by a principal, with the work
  it holds (below) until that settles.
- **Transaction**: a committed change of a document. It names its
  principal, as now, and its operation.

An operation is never a principal: undo stacks are kept per actor, and
typing would come apart into one stack per key. A staged proposal
(`crates/techne-editor/src/access.rs`) links two operations: the one
that proposed it, and the one that accepted it, perhaps much later,
whose transaction it becomes; its principal stays the grant's.

## Operations

**Every command run is an operation.** `run-command` takes the next id,
never reused (the runtime's incarnation and a counter), and makes it
the **current operation**. Commands run inside an interaction that only
reads (the minibuffer's own keys) and hooks get none. M-x is an
interaction: the command it runs is an operation whose parent is M-x's
own, so the trace says how it was reached.

**Joining.** A command may declare that its runs join (`#:join`:
self-insertion, motions). A run from a key of such a command continues
the last operation instead of starting one, counting its runs, when
that operation is of the same command and code, by the same principal,
and the run only moves views or extends its undo unit (typing,
`Group::Extend`). So a typed word is one operation, whose transactions
all name it, and typing's undo unit holds that operation's changes. Runs
invoked with explicit arguments never join.

**The current operation is one task-local value**, a parameter of the
VM: tasks inherit it when spawned and it switches with them
(`crates/techne-vm/src/tasks.rs`). Callbacks a command leaves (a
minibuffer's accept and preview, a request's delivery) capture it and
restore it when they run, as `scope-procedure` restores the scope a
command runs in. Natives that commit (`View::edit`, accepting a
proposal, saving, starting a process) read it from the running task,
so no call site passes it, and nothing keeps a second copy that could
disagree. Writers of the editor's own text (*Messages*, the
minibuffer's input, a process's output buffer) commit under no
operation, or every operation would edit *Messages*; no operation
means no principal's authority beyond the editor's own.

**Holding.** An operation is pending while its procedure runs and while
work it holds is unfinished. It holds a task spawned as its child or a
request made in it, a minibuffer it opened (a legacy command's prompt),
and a process it waits for. Work started for longer than the operation,
in another scope (a language server's session, its document-following
tasks, a service), does not hold it: it records the operation that
started it, and goes on. So `lsp-format-buffer`'s edits, made when the
server answers three keys later, are its edits, and it ends when they
are made; `lsp`, which starts a server, ends when the server is asked
for, not when it exits.

**Ending.** The first failure (a condition escaping its procedure or
work it holds, not one a handler deals with, as a live decision does;
or code that catches a failure saying so with `operation-failed!`:
`lsp.scm`'s `asking` turns a server's error into a message, and must),
or a cancellation, makes the operation *ending*: the work it still
holds is cancelled, and it ends when that work has. A native that
commits under an ending or ended operation refuses, so nothing it
started lands after it failed. The outcome is decided then, from what
committed. An operation ends:

- **done**, with its result;
- **failed**, with its condition and the recovery choices it offers,
  nothing committed;
- **cancelled**: C-g, an aborted prompt, its scope shut, its request
  superseded before it committed anything;
- **partial**: some effects committed, then it failed or was stopped (a
  rename that changed two files of three), with what was done. A
  request superseded in its slot (`requests.scm`) is not undone, so it
  is partial if its producer committed anything;
- **unknown**: an external effect whose outcome was not learnt (a lost
  reply). It never offers to try again, only to inspect or adopt.

**Records.** An operation's state lives with it, held by the work that
holds it; logs only list operations. Two logs, in memory, each bounded
in entries and bytes, the oldest entry evicted first, pending or not:

- the **trace**: every operation, newest kept: id, parent, command and
  its code identity, principal, prefix, buffer and revision at the
  start, its runs, outcome, and which binding won the key that ran it
  (a command's name cannot say why a key ran it once the key is bound
  to something else). The arguments of a command with a contract are
  in it from the start: the dict `invoke` validated.
- the **kept operations**: a trace entry is copied here at its first
  effect beyond moving views (a transaction, a write, a process, a
  proposal), at a failure or violation, or when it was invoked with
  explicit arguments (a form, an agent, a recipe, a recovery), with its
  arguments and effects as data, sensitive values redacted (below), and
  the condition rendered with its typed fields, never the live
  condition, a closure or the session. One record is bounded too, to
  16 KiB: past a hundred transactions it keeps their count per
  document, and a field over its share keeps only its size. The
  documents and files it names are kept whole up to a bound; past it,
  the record says it names more, and counts as naming one an agent may
  not read.

Evicting a pending operation's entry does not lose the operation: it
still settles and stamps its transactions, and `M-x operations` lists
the pending ones from the work that holds them. Asked for an evicted
one that has ended, a log says it is forgotten, with its id.

**Transactions name operations.** `Transaction` and the history's
entries get an operation id beside the actor, journaled with them (the
journal's next version; earlier records read as none). After a restart a
change still names its operation, whose details are forgotten. An edit
through a lens keeps its operation across all its sources; an undo is
an operation of its own.

So a change can be blamed: from a transaction to its operation, its
command, arguments and principal, and the code that ran, which
redefining the command since does not erase.

## Operations as targets

`M-x operations` lists the kept operations, newest first, pending and
failed ones marked; an operation is a target wherever it appears (the
list, the inspector, a transaction's "made by"). Its actions:

- **inspect**: command, principal, arguments, outcome and condition,
  parent and children, the changes it made as locations;
- **changes**: a lens of the text it changed, editable through;
- **again**: run its command with its arguments, rehearsed first
  (below);
- **undo**: reverse its changes, as an operation of its own;
- **choose**: one of its recovery choices, when it failed;
- **forget**: delete its record.

**Undo of an operation.** In each document, its transactions must be
exactly its principal's latest undo units, one or several, holding no
other operation's transactions. Otherwise it is refused, naming the
document and who changed it since, or that the change is joined with
another operation's (undo it there). Every document is checked before
any is changed. A journal failure halfway leaves some undone, and the
undo says which: it is not atomic across documents, and EDITOR.md
section 5's refusal of selective undo stands.

Reading operations discloses what they hold (paths, arguments, a
region's text). An agent reads its own operations, and another's only
when it may read every document and file that operation names; of the
rest it sees the id, the command and the outcome.

## Contracts

```scheme
(define-command (lsp-rename s at new-name)
  "Rename the symbol at AT to NEW-NAME, in every file of the project."
  #:arguments '((at location #:doc "Where the symbol is.")
                (new-name (string #:min 1) #:doc "Its new name."))
  #:read `((at ,point-location)
           (new-name ,(prompt-string "Rename to: " #:initial symbol-at-point)))
  #:effects '(read query edit)
  #:expose '(agents)
  (...))
```

A contract describes the command's boundary, for whoever invokes it:
agents, forms, recipes, recovery. The body stays ordinary code, and
never prompts.

- **Arguments** are a `dict` schema (`lisp/schema.scm`): names, value
  schemas, defaults, documentation. Validation is `explain`, JSON for
  agents `schema->json-schema`, and data from an agent or a saved form
  becomes values by `conform`. Everything the body uses that a caller
  could mean differently is an argument, including what Emacs reads from
  point and the region: an agent renames at a location, not at "point".
- **References are data.** Schemas gain editor kinds: `location`,
  `range`, `buffer`, `file`, `(target kind)`. Each kind registers a
  written form (a location: a file or document, a position, the
  revision it is of) and a resolver that answers missing, stale or
  ambiguous rather than guessing. Arguments hold these, never live
  objects, so an operation can be inspected and run again after the
  code that ran it changed, and a reference handed to an agent carries
  no authority.
- **Acquisition** sits beside the schema: a schema says what a value may
  be, not how to ask for it (a file name, a search and a replacement are
  all strings). `#:read` gives a parameter a reader, a target kind to
  complete from, or a default from context; one not given gets a reader
  by its schema (choices complete, booleans ask y or n). A parameter
  whose schema depends on earlier arguments (`set-option`'s value, of
  the option's type) gives a pure procedure of them, called again when
  an earlier argument changes. A command whose prompts no table
  expresses gives an acquirer procedure of the session and the
  arguments so far, written with today's `completing-read`. Context
  defaults are values when read: a location from point names that
  document and revision, so a prompt that ends after the focus moved
  does not act on the buffer focused then.
- **Sensitive** parameters (`#:sensitive`), and text taken from context
  that its document's history excludes, are redacted before anything is
  recorded or formatted: in the records; in the schema's diagnostics,
  which `lisp/schema.scm` writes into each problem's message and into
  `conform`'s error text, so it learns which paths are sensitive and
  names them instead of writing their values; and in the irritants of
  errors the operation raises, where a sensitive value appears as
  itself. A body that copies one into its own message or a document is
  not caught: redaction covers what the envelope records and formats.
  A sensitive argument is not kept, so running the operation again
  asks for it.
- **Effects** are what admission, confirmation and replay decide on:
  `read` (content goes to the invoker, an agent or a model), `query`
  (asks a server or process something, changing nothing there),
  `edit` (documents, revision-checked, undoable), and `external`
  (anything else that changes the world: writing files, running
  processes, sending), with properties: needs confirming, safe to
  repeat, irreversible. Moving views is the absence of effects. Effects
  may be a procedure of the arguments (`shell-command-on-region` edits
  only when it replaces), bounded by the declaration. They describe;
  confining is the runtime's (below). Effects observed but not declared
  are a violation (`violation?`) kept on the operation, so tests and
  keyfuzz find them.
- **Availability**: an optional cheap procedure of the session, giving
  #t or why not (no region, no language server); `invoke` checks it,
  and M-x shows unavailable commands as such.
- **Exposure**: `#:expose '(agents)` opts a command in. The registry is
  never exported whole, and an exposed contract may hold only values
  data can carry: no procedures; characters and bytevectors by their
  written forms.
- **Result**: a schema, required only of exposed commands.
- **Identity**: an operation records its command's contract hash and
  its code's identity (the definition's generation), separately: a
  changed contract makes old arguments suspect, a changed body only
  worth saying.

A command defined as now, `(name s n)`, keeps working and is
**opaque**: its arguments unknown (the count is not enough: `yank`
reads the raw prefix and the kill ring, modal actions run a closure),
its effects assumed all, never exposed, not run again from its record.
Its operations are still traced, stamped and kept: blame works for every
command from the start, contracts for those converted.

Commands that make their own views must make them for the operation's
principal. `lsp.scm`'s `apply-text-edits!` uses the buffer's view or a
new one as `"user"`: an agent's rename would be the user's edits. It
changes with `lsp-rename`'s contract.

**Actions** on targets (`lisp/editor/targets.scm`) register contracts
too, their first parameter the target, and run through the same
invocation; they stay a namespace of their own, as their order per kind
(the first is the default) means nothing to M-x.

**Modal operator-pending** is an interaction: operator, count and motion
acquire an extent, and the operation starts when the operator runs.
`.` repeating the last change stays as it is until operations can do
it.

## Invocation

One entry point:

    (invoke s 'lsp-rename {at: loc new-name: "frobnicate"}
            #:principal agent #:mode 'apply)

It checks that the command is there, available, exposed to the
principal and admitted (the principal's grants against its effects for
these arguments, and against the documents its references name);
conforms and validates the arguments, refusing with `explain`'s
problems; opens the operation; and runs the body in the command's
scope. Keys and M-x run the acquisition first, which ends in `invoke`,
or in nothing when aborted. A form, an action, a recovery choice, a
recipe and an agent's adapter call `invoke` directly.

**Modes.** `apply` commits. `stage` plans: `read` and `query` effects
run (a rename asks its server for the edits), edits become proposals
(`access.rs`, generalized from services to operations), and `external`
effects are refused, which ends the operation failed, or partial when
something had committed, with a choice to apply. `check` validates and admits only. Staging serves an agent's
staged edits, a preview of what a rename will change, and a recipe's
dry run. Bodies written to stage must not assume their edits landed: a
code action that applies its edits and then asks its server to run a
command is staged as the edits and a refused command.

**An agent's session.** A body takes a session for views and messages.
An agent gets a headless session with views of its own over the shared
documents (each attachment has its own views already, EDITOR.md section
6), so its commands do not move the user's point. A command whose
effect is the layout itself is not exposed.

## Authority

Effects describe; they do not confine. An exposed command's body is
trusted code in the editor's world, which holds every capability the
editor has. Agents never run Lisp in the editor's world: an agent's own
programs run in sandboxed worlds (PLAN.md, Stage 3). What the runtime
checks, for every operation whose principal is an agent, in Rust:

- **admission**: the command is exposed to agents, and the principal's
  grants cover its effects for these arguments;
- **references**: each document or file an argument names resolves only
  within the principal's grants;
- **reads**: the natives that read a document's text or a file's check
  the current operation's principal against its grants to read, as
  `access.rs` gives a service only snapshots of documents granted to
  it. A body cannot read through them text the principal may not;
- **commits**: the natives that commit check the principal against its
  grants to change: an edit without a grant to edit is staged or
  refused;
- **results**: the adapter refuses a result whose references name
  documents or files outside the grants to read.

What no native guards is text a body has without reading it then: the
editor's other state (the buffer list, options, the kill ring), text cached in Lisp, and a server's replies,
which may quote files not granted (a shared language server knows the
whole project). Nor what it sends under `query`. For those, exposing a
command is an audit that it gives the principal, and sends on its
behalf, only what the principal's grants allow: filtering a server's
reply to granted files, sending only granted text. Exposure is the
user's or a package's decision, reviewed as code is, not a property
the runtime can check.

The principal is the runtime's: the adapter's entry sets it on the
operation it opens, and the operation keeps it; Lisp reads it, and only
`invoke` in the editor's trusted code chooses it. Code that catches
failures, runs under no operation, or starts work for longer than the
operation does not shed or gain a principal by doing so: work started
from an agent's operation keeps that principal.

## Rehearsing

"Again" fills the command's arguments with the operation's, then checks
them against the state now before anything runs: locations mapped
through the edits since or marked stale, defaults that would now
differ, a changed contract or body, grants the principal no longer has,
the effects for these arguments. What changed is shown; RET runs. A
record is evidence of what happened, not permission, and not a promise
that the same arguments mean the same thing now. Recipes (PLAN.md,
Stage 6) are sequences of operations checked the same way.

The argument **form** is the Transient of EDITOR.md, section 10: a row
per argument, a key to change each, RET to invoke. The same form, empty
or with context defaults, runs a command with arguments from M-x. Until
it exists, "again" asks through the command's prompts, filled with the
old arguments.

## Recovery

By whether the stack that raised is still there.

**Live.** While it is, CL's restarts as the prelude has them
(`restart-case`, `invoke-restart`), chosen by a handler in code. A
restart gains documentation and an argument schema. A failure in a
task an operation holds (a request, anything after a wait) can wait for
a person: the handler makes a **decision**, an id never reused, with
the restarts as choices, and waits on it. Choosing sends data, the
choice and its arguments, to the waiting task, which checks them and its
own applicability again and invokes its restart itself; no closure
leaves the task. The first valid choice settles the decision; later
ones are refused. When the task is cancelled, its scope shut, the
decision expires, or the principal's grant is revoked, the task is
woken with that, its operation ends, and what choices remain are
deferred ones.

**Deferred.** A command in the editor's world cannot wait for the user
without holding up every frontend (commands run one at a time), and a
waiting task can be stopped. Then the operation ends, and its choices
remain as data. A **recovery choice** carries:

- a label and documentation;
- what it invokes: a command and arguments under its contract, some
  still to acquire. Not necessarily the failed command again: a code
  action that applied its edits and then failed on its server command
  offers running the rest, not all of it;
- what is known done: the effects so far (two files of three);
- what it expects: targets and revisions, the failed operation;
- applicability, checked when shown and when chosen, and expiry;
- the authority choosing it needs, checked against the chooser then:
  neither the failed operation nor its record grants any;
- whether it can run once only.

A condition carries its choices (`raise` with `#:choices`, or a
condition type that makes them from its fields: a save refused because
the file changed offers to overwrite, to reload, to compare). Choosing
one is an operation of its own, a child of the failed one, validated as
any invocation is: after another edit, a choice expecting the older
revision is refused, or acquires its arguments again.

**Who chooses.** The user, for any operation. An agent, among the
choices of its own operations, within its grants. Choosing for another
principal's operation, or a choice with irreversible effects, becomes
a decision for the user.

**Pending decisions** are live decisions, operations failed with
applicable choices, and decisions asked for on purpose (an agent
asking approval). `M-x decisions` lists them, and the echo area offers
the latest failure's choices on a key. One leaves the list when chosen,
dismissed or expired, when its scope is shut, or when no choice applies
any more. They are the model REQUIREMENTS.md's attention routing
presents, and the desktop's notifications later.

## Agents

An agent is a principal. Its adapter (MCP first, PLAN.md Stage 2) lists
the commands exposed to agents, with their documentation and their
arguments' JSON Schema, and invokes them with arguments as data,
through `invoke`, in its own session: the same validation, operations,
recovery and decisions as the user's, under the checks of "Authority".
Its edits go where its grants say: into the document, or staged as
proposals the user accepts.

## Order of work

Each step one change, measured before the next.

1. **Operation identity.** The current operation through `run-command`,
   task inheritance and minibuffer and request callbacks; transactions
   stamped and journaled; holding and outcomes; `operation-failed!`.
   No logs or interface yet beyond a transaction's operation id.
   *Acceptance:* `lsp-format-buffer` from two editor sessions (two
   attachments), each in a buffer of its own, both servers answering
   after keys typed in each: each stays pending until its edits are
   made, and its transactions, read back from the journal after a
   restart, name it, not the keys. Twice from one editor session, whose
   request slot (`asking`'s) supersedes the first even for buffers of
   two servers: the first ends cancelled, or partial when it had
   committed. A swallowed server
   error ends it failed; shutting its scope ends it cancelled, and a
   commit after that is refused; a nested `run-command` is a child.
   *Messages* changes under no operation. The
   key workloads (`crates/techne-editor/examples/keys.rs`, both
   profiles) stay within 5% of their instructions and allocations, no
   key over 28M.
2. **Records and blame.** Joining, the trace and the kept operations
   with their bounds, `M-x operations`, inspect and changes; a
   transaction's operation in the inspector.
   *Acceptance:* a typed word is one operation, and two when the
   command is redefined in the middle; logs full of pending operations
   evict them and still list them as pending, which still settle and
   stamp; one with a thousand transactions keeps their counts; an
   evicted ended one is "forgotten"; the key workloads as in step 1.
3. **Contracts and `invoke`**: arguments, acquisition, sensitivity,
   effects, availability, exposure, identity; reference kinds for
   locations and files. Converted: `set-option` (dependent arguments),
   `lsp-rename` (asynchronous, several documents, a default from point;
   its views the principal's), `find-file` (a reader by target kind),
   `shell-command-on-region` (effects by argument). Exposed contracts'
   JSON Schemas tested as fixtures.
   *Acceptance:* each, run from keys and invoked with arguments as data,
   shares validation and body; an invalid value is refused with the
   schema's explanation and changes nothing; a sensitive argument's
   value appears in no record, schema diagnostic or error the envelope
   formats, valid or not.
4. **Again, undo and forget.**
   *Acceptance:* a rename invoked as data by a test principal and one by
   keys each leave one operation, both files' transactions naming it
   with the right actor; undoing it reverses both as one child
   operation; after a conflicting edit in one file the undo refuses and
   changes neither; a journal failure injected after the first file
   leaves the undo partial, saying which; running it again after the
   body was redefined says so, after the contract changed asks again.
5. **Recovery and pending decisions**, beginning with saving a changed
   file and accepting proposals; then live decisions in tasks.
   *Acceptance:* PLAN.md's for step 10: a refused save offers choices;
   after another edit or a revoked grant a choice is revalidated, and
   refused or redone. Two choices of one live decision raced: one runs,
   the other is refused; an expired or revoked live decision ends its
   operation and leaves its deferred choices.
6. **The form**, then **stage mode**.
   *Acceptance:* a form changes an argument and checks the later,
   dependent ones; a staged rename changes no document, leaves one
   proposal per file and runs no external effect; accepting the
   proposals is one operation linked to it.
7. **Agent principals and authority**: the checks of "Authority", in
   Rust, before anything outside the editor can invoke.
   *Acceptance:* a test principal is refused an unexposed command, a
   reference outside its grants, a read by an exposed body of a
   document not granted to it, an edit without a grant (staged
   instead), and a result naming a document it may not read; it sees
   of the user's operations only the id, command and outcome.
8. **The MCP adapter** over exposed commands: Stage 2's exit.
   *Acceptance:* PLAN.md's: an agent's staged edit applied while the
   user edits the same document, without silent loss; a stale proposal
   refused naming who changed the text; a denied read refused.
9. **Recipes** (Stage 6).

## Hardest to change later

1. What one operation is across prompts, callbacks, tasks, joining and
   nesting, and the operation id in `Transaction`, the history and the
   journal's format.
2. Arguments as data: by name, with references that resolve or say why
   not. It is the agents' protocol and the recipes' format.
3. Where authority is checked: by the runtime, at admission, references,
   reads, commits and results, for every operation an agent invokes.

## Questions

- Whether the trace records the winning binding of every key, or only
  while asked (Emacs's `view-lossage`), given the key budget.
- How long kept operations live, and whether operations waiting on a
  choice persist across restarts (their choices are data already).
- Whether an agent's edits default to proposals or to edits under a
  grant, per document.
