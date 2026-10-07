# Techne: the editor

Status: design for the editor probe (PLAN.md Stage 1, workstream B), agreed with
Arthur after two independent reviews (GPT-6 Astra) and a study of `../neomacs`
and `../doomconfig`. Section 13 lists what is deliberately left open; the first
slice tests the rest. Where a promise here exceeds what is specified, the
specification wins.

The editor comes before the desktop. It runs hosted in an ordinary window and in
a terminal; later it runs remotely in a browser, and the same window model
becomes the desktop (section 9). Nothing here depends on Wayland.

## 1. What a buffer is

"Buffer" stays the user-facing name for anything you can switch to: a file, an
agenda, a terminal, a conversation. Underneath it is not always mutable text.
Emacs got the uniform interaction right (everything moves, searches, selects and
copies the same way) and the universal mutable-text storage wrong (Magit status
is not text to edit with `inhibit-read-only`).

| Layer | What it is | Examples |
|---|---|---|
| Resource | Durable identity and execution context; no authority | a file, mailbox, repository, conversation |
| Document or model | The authoritative state | a text document (rope); a git model, query result, mail index, terminal grid, conversation log |
| View | One interaction with that state | selections, scroll anchor, folds, input profile; two views of one document differ |
| Presentation | An immutable snapshot of logical rows (section 2) | derived; maps back to targets and source ranges |

Commands act on typed targets. Staging a hunk is an action on a hunk target, and
the status view is presented again from the git model; deleting display text
never stages anything. Closing the last view of a buffer never silently drops a
dirty document or a persistent task.

How applications map:

- **Source file:** text document; edits are revision-checked transactions.
- **VCS status, agenda, mail list:** a model plus a row provider; actions on
  targets (hunk, heading, message). Agenda actions edit the underlying Org text.
- **Terminal application:** the terminal engine's cell grid and bounded
  scrollback. Its rows are fixed-width cell rows with continuation cells for
  wide characters and soft-wrap marks for copying; they are never reflowed.
  Input goes to the PTY; a copy mode selects. (This is the terminal *buffer*;
  the terminal *frontend* is section 6.)
- **REPL, agent conversation:** a log of entries (inputs, results, conditions,
  messages, tool calls) as rows, plus an editable input document.
- **Lens:** rows whose text maps to `{document, revision, anchored range}`
  segments with non-editable separators; editing goes through to those
  documents. Edits across segment boundaries are refused.

## 2. One presentation primitive: logical rows

A presentation is an ordered stream of **logical rows**. Providers produce
logical content; **frontends produce visual lines**: wrapping, shaping and
measuring happen only in the frontend, and wrapping never changes a row's
identity. A logical row has:

- a stable key (scroll anchors, folds and row selections survive updates; a
  key, once deleted, is never reused within the presentation);
- text runs: text, a style role, an optional target and an optional source map
  (the source range each run's text came from, or none for generated text);
- section metadata (id, depth) for folding and navigation, not a widget tree;
- or, instead of text, one embedded block of a negotiated type (image, rule,
  progress bar) with intrinsic size constraints; the frontend decides its
  realized size.

This is an experiment with named limits, revisited after the first slice:

- **Columns:** a presentation may declare column stops with declared or bounded
  widths, so lists and logs align with proportional fonts. Widths are never
  found by scanning every row. Tables in the first slice have no spanning or
  wrapped cells.
- **Inline images** inside running text are promoted to their own block rows at
  first; a richer inline layout is not assumed.
- **Two-dimensional content** (a PDF page, a graph) is a block that draws and
  hit-tests itself in the frontend; it exposes targets but gives up the generic
  text features.
- **Side-by-side diffs** are two views plus alignment groups (which rows
  correspond) and coordinated scrolling; padding fills wrapping differences.
- **Bidirectional text** keeps logical order in the source; logical and visual
  cursor motion are separate commands, and a visual selection may paint as
  several rectangles.

Shared semantic operations (motion, search, selection, copy, the target at
point, what an agent is shown) are written once over logical rows; each frontend
supplies a layout adapter for the geometric parts (section 6).

## 3. Layers instead of text properties and overlays

Meaning attached to ranges without changing source text, split by purpose:

1. **Annotations (meaning):** syntax classes, diagnostics, links, targets,
   source maps.
2. **Decorations (looks):** style roles, underlines, gutters, selection and
   search highlights, inline hints.
3. **Projections (layout):** folds, replacement displays, virtual lines,
   embedded blocks.

Each layer has an owner scope and generation (PLAN.md, language step 5), so
unloading a package removes its layers. Ranges are anchors with explicit
insertion affinity and deletion behavior, and carry the revision they were
computed for; late results for an old revision or generation are dropped or
revalidated. No layer runs code during layout. Faces become named style roles
resolved by the theme; composition follows layer classes, not priority-number
races. The inspector can answer "why is this underlined" and "who hid these
lines". Read-only is an editing capability, not a property. Generated text,
hidden text and lens separators are copyable but not editable.

## 4. Positions and the editing algebra

Coordinate spaces stay separate:

- **source positions:** byte offsets at a document revision;
- **editing units:** graphemes, words, syntax objects;
- **presentation positions:** row key plus an offset inside the row's text,
  plus an affinity (which side of a wrap or a run boundary the caret is on);
- **screen positions:** frontend geometry, never stored.

LSP conversions happen in the LSP adapter. There is no universal "column".

The algebra:

- **Ranges** are half-open source ranges `[start, end)`. Vim's inclusive
  selections are an input-profile view of them; linewise ranges own their
  final newline, and at the end of a file without one, deleting the last line
  removes the preceding newline instead.
- **Selections** are an ordered, normalized set (overlapping ones merge) with a
  primary selection that keeps its identity across merges. A selection is a
  directed anchor/head range, a caret when empty.
- **Motions** return a destination; **operator extents** are computed from a
  motion by the operator's rules (`w` moves to the next word start, `dw`
  deletes to it, `cw` changes only to the end of the word). Text objects
  return extents directly.
- **Operators** consume extents and produce one transaction.
- **Rectangles** are resolved by the frontend from screen geometry into a set
  of source ranges; they are not a permanent range kind. (Deferred: not in the
  first slice.)

Two input profiles share the commands and algebra:

- **Emacs chords** (the default): point and mark are the primary selection and
  its inactive saved anchor.
- **Modal** (Vim-like): normal, insert, visual and operator-pending states.

"Equal" means equal standing in the architecture and an agreed minimal set in
the first slice, not parity with Emacs or evil: insert and delete, motions by
character, word, line and buffer, kill/yank (copy/paste) and registers, undo,
search, and for the modal profile counts, `d`/`c`/`y` with motions and a few
text objects, and `.` for simple changes.

Keymaps resolve through declared scopes (transient controls, input state, view
mode, enabled packages, global) with a specified precedence; the inspector shows
the winning binding and what it shadows. Prefix keys and operator-pending share
the state machine, but counts, cancellation, failed motions and repeat are
defined per profile. Repeat records intent ("change the next word to this
text"), not raw keys; full intent repeat is deferred. Keyboard macros exist but
are not the basis of repeat or recipes.

## 5. History

Undo belongs to the text document and records transactions with their actor and
operation. Undo reverses your own last transaction if later edits by others do
not conflict with it, and refuses with an explanation if they do; selective
undo around other actors' edits is not promised. Domain actions (staging,
sending mail, running a command) have their own recovery or none; text undo
never pretends to reverse them. Selection and scroll restoration is separate
navigation history.

The edit journal records each transaction before it is acknowledged. After a
crash of the process, everything acknowledged is recovered and a torn last
record is discarded. Surviving power loss needs an fsync policy, chosen
separately (it costs latency).

## 6. Frontends and the presentation protocol

The runtime holds documents, views and presentations; a frontend draws
presentations and sends input back over one protocol (as Neovim's UI protocol
lets many GUIs share one editor). Every frontend attachment gets its own views,
so a GPU window and a terminal never fight over one scroll position.

**The protocol.** Everything in it is serializable data: no native pointers or
callbacks, even when frontend and runtime share a process.

- **Snapshots and deltas:** each presentation version has an id and names its
  base. A delta applies atomically to its base; a frontend missing the base asks
  for a full snapshot. A snapshot is bound to the model revision it was made
  from, including rows materialized lazily later.
- **Lazy rows:** the frontend requests rows around an anchor, lays them out,
  and requests more until the viewport is full. Providers support seeking,
  ranges and cancellation, and never run Lisp on the frame path.
- **Geometry is versioned, not queried.** The frontend reports a layout identity
  (snapshot id, viewport, scale and font configuration, layout epoch). Input
  events carry the identity they were made against. Visual operations (vertical
  motion, a click, page down, "what is visible") are resolved by the frontend
  into semantic positions; the runtime applies them, re-resolves them, or
  rejects them if their snapshot is stale. Vertical motion keeps a
  frontend-specific preferred horizontal position.
- **Scroll position** is an anchor (row key plus offset inside the row), never
  a pixel position.
- **Lifetime and flow:** the frontend acknowledges which snapshot is active;
  the runtime keeps referenced snapshots for a bounded time. Queues are bounded;
  pending deltas coalesce without losing what later deltas depend on. After a
  reconnect, the frontend starts from a full snapshot.
- **Input:** keys, committed text, IME composition and bracketed paste are
  distinct events.
- **Capabilities:** each frontend declares what it has (proportional fonts,
  images, true color, key protocol, block types). Presentations degrade instead
  of the design shrinking: blocks fall back to text, column stops become cell
  columns, images use the kitty or sixel protocols where a terminal has them.
  Keys a terminal cannot send (some chords; the kitty keyboard protocol helps)
  are handled by the key normalizer, not the keymaps.

What an agent is shown of "the visible region" is a versioned observation (the
source and target ranges visible, and how much of each), not a live property or
a grant of access. Logical excerpts are preferred unless visibility matters.

**Frontends and their order:**

1. **GPU** (section 7), first; in the runtime's process for the hosted editor.
2. **Terminal**, small and early in the probe: it tests cell geometry, key
   limits and the protocol before the protocol freezes. Its tests run headless.
3. **Browser**, later: a remote frontend to a native runtime (WebGPU, which
   `wgpu` targets, or a canvas). Before it ships: authentication, who may attach
   to a session, and rules for clipboard and disclosure. Local latency budgets
   do not extend to it. Compiling all of Techne to WebAssembly is not a goal.

**What survives what.** A frontend in the runtime's process dies with it: after
a runtime crash the hosted window is recreated with views, scroll anchors and
unsaved text recovered from the journal. A frontend in its own process (a
terminal frontend, a browser, possibly later the GPU one) can keep its window
and show a disconnected state until the runtime returns.

## 7. The GPU frontend

Following neomacs, which proves the stack (and is GPL-3.0, so code can be
borrowed with attribution):

- `wgpu`; text shaped with `cosmic-text`, rasterized with `swash` into a glyph
  atlas, drawn as instanced quads; on a render thread separate from Lisp.
- Frames are scheduled by damage and animation, not a busy loop. No Lisp, I/O
  or whole-document walk on the frame path.
- Proportional fonts from the start; monospace is the default style, not an
  assumption. Vertical motion and hit-testing use shaped geometry.
- Variable row heights keep a height index and scroll anchors, so an image
  loading above the viewport does not move the reader.
- Huge files: layout only around the viewport. Long lines: horizontally bounded
  work and an explicit policy for pathological lines.

neomacs' layout engine is ~220k lines because it reproduces Emacs display
properties and overlays exactly; avoiding those semantics (section 3) is what
keeps ours small.

## 8. The terminal frontend

Cells instead of glyph geometry: grapheme widths from Unicode tables, wide
characters taking two cells, column stops rounded to cells. Embedded blocks show
their text fallback unless the terminal supports kitty or sixel images. It reads
keys through the kitty keyboard protocol when available and otherwise accepts
that some chords do not exist (the key normalizer reports them unbindable).

## 9. Windows, and later the desktop

The window tree is Lisp, like Emacs' `window.el`: splits, "where does this
buffer go", the frame and strip arrangement. Lisp computes rectangles in
abstract units; the frontend realizes them:

- **hosted, terminal or browser:** one window, screen or page; the frontend
  draws all the editor's windows inside it;
- **desktop (later, GPU):** each window is a surface the compositor places, and
  application buffers are real Wayland windows (PLAN.md, "The compositor").

The default window manager is a Lisp package with the Emacs/EWM feel; others
can write their own against the same primitives.

## 10. Minibuffer and completion

The reference look is Arthur's current Doom setup (Vertico, Orderless, Consult,
Embark, Marginalia, nerd-icons, Corfu, Transient, which-key, popup rules). It
is a design reference, not a compatibility commitment. The pieces map as:

- **Minibuffer:** a view with an input document and a candidate row list.
- **Candidates** carry targets, so Embark-style actions apply to them directly.
- **Annotations (Marginalia):** columns derived from the candidate's type.
- **Preview (Consult):** showing the target in the focused pane while moving;
  cancelling puts the pane back.
- **Matching (Orderless):** a style written in Lisp.
- **Export (Embark):** candidates that are locations become an editable lens.
- **Argument controls (Transient):** generated from command schemas.
- **In-buffer completion (Corfu):** a popup over the same candidate protocol.

The first slice has the minibuffer, candidates with targets and actions,
annotations, preview, matching and export (`lisp/editor/minibuffer.scm`,
`targets.scm`, `lens.scm`); its layout follows Vertico's. Screenshots of the
Doom setup, taken in an off-screen session, remain the reference for the
rest.

## 11. Extensions and user control

The goal is a solid base and an ecosystem that flourishes as Emacs' did: not by
copying Emacs' design, but by giving users as much control as Emacs does.

**Nothing privileged.** Everything written in Lisp can be read, redefined and
replaced while running: commands, keymaps, views, themes, the minibuffer, the
modal profile, the window manager. The built-in features are packages written
against the same public interfaces extensions use; if a built-in needs an
interface, so may anyone. Rust mechanisms expose their policy points to Lisp.
Only safety invariants are closed: capability checks, the journal, revision
checks on writes, and the compositor's locking, focus and capture rules.

**Explicit, owned extension points:**

- commands and target actions (a new target type gets completion, actions at
  point and agent context at once);
- keymaps in declared scopes, and whole input profiles;
- row providers (views), layers, completion sources;
- hooks: named events with documented arguments;
- advice: around, before and after commands and functions, owned by the
  package that added it, removed with it, and listed by the inspector ("who
  changed this function");
- frontend block types, in Rust, for content rows cannot express.

Every registration belongs to its package's scope and generation (PLAN.md,
language steps 5 and 6), so reloading replaces it and unloading removes it.

**Simple things stay simple.** A thin authoring layer hides the parts a small
extension does not care about: `define-command`, `define-mode`,
`define-target`, `define-view`, `define-completion-source`, `define-layer`. Four
canonical examples are acceptance tests (PLAN.md, Stage 1 slices 4 and 5), each
about as short as its Emacs Lisp equivalent:

1. a command acting on the region;
2. a minor mode with a keymap and a highlighting layer;
3. a structured view ("TODOs in this project") with targets and actions;
4. a minibuffer completion source with preview.

All four exist, in `lisp/editor/examples`, written against the library
`(techne editor)` and loaded as packages (`load-package`), so reloading
replaces them and unloading removes their commands, key bindings, modes,
layers and actions. The second:

```scheme
(import (techne editor))

(define-command (next-todo s n)
  "Move to the next TODO."
  (goto-next! s "TODO"))

(define (todos doc from to)
  (map (lambda (m) (list (car m) (cadr m) 'warning)) (search-all doc "TODO" from to)))

(define-mode todo-mode
  "Highlight TODOs; C-c t moves to the next one."
  #:keys '(("C-c t" next-todo))
  #:layer todos)
```

The third, a structured view: its rows are generated text, read-only, each
with a target.

```scheme
(define (todos path)
  (let loop ((lines (guard (e (#t '())) (file->lines path))) (n 1) (acc '()))
    (cond ((null? lines) (reverse acc))
          ((string-contains (car lines) "TODO")
           (loop (cdr lines) (+ n 1)
                 (cons (row (string-append (file-name path) ":" (number->string n)) (string-trim (car lines))
                            #:target (target 'location (file-location path n)))
                       acc)))
          (else (loop (cdr lines) (+ n 1) acc)))))

(define-view (directory-todos s)
  "TODO comments in the files of this buffer's directory."
  (let ((dir (default-directory s)))
    (append-map (lambda (name) (if (string-suffix? "/" name) '() (todos (string-append dir name))))
                (directory-list dir))))
```

Jumping to a TODO, searching, copying and acting on it from the minibuffer come
from the location target, with no further code.

**Discoverable.** Docstrings and `help` for everything; the inspector shows
where a thing is defined, which package owns it, what it shadows and why a key
is bound; source navigation and evaluation in the module work on built-ins as
on one's own code.

**Packages.** A package is a set of modules with an owner scope, a generation,
declared dependencies and declared capabilities. Packages come from source
(git), pinned in a lockfile, as Doom and elpaca pin today; reproducible builds
through Nix are possible, not required. Interfaces are marked stable or
experimental so extension authors know what they may rely on. Untrusted packages
run in a restricted world with only the capabilities they declare and the user
grants. Rust extensions are crates linked into the runtime for now; loading them
dynamically comes later, under the same capabilities.

**Testable.** The headless frontend and test helpers let a package test its
commands and views without a window.

## 12. The first slice

Techne's own Lisp, on a general text editor: open and save files, recover unsaved
edits after a crash, undo, two views of one document, incremental search,
completion in the minibuffer, evaluation in the file's module, inspecting
results, jumping to definitions, and one editable search lens. The GPU frontend
and a small terminal frontend; both key profiles at the minimal set of section
4. The loop: edit a command, evaluate it, invoke it, inspect it, revise it,
without a restart. Org files must open and survive edits byte for byte; agenda
and rich Org come later.

Deferred past the first slice: rectangles, full intent repeat, Corfu and
Transient equivalents, general embedded blocks, selective undo around other
actors' edits, the browser frontend.

## 13. Hardest to change later, and still open

Settled in the first slice, because they are hardest to change later:

1. Authority and identity: documents own content, views do not; resource
   references are not live handles; every write has a conflict contract.
2. Positions and selections: source ranges, anchors, presentation positions
   with affinity, selection sets; geometry owned by frontends and versioned.
3. Command and history semantics: commands act on targets; text undo, domain
   recovery and irreversible effects are different contracts.

Open, to be decided by the first slice: whether logical rows suffice (or a
second primitive is needed); the fsync policy of the journal; whether the GPU
frontend moves to its own process.
