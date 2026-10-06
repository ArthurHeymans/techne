# Techne: the editor

Status: design decisions for the editor probe (PLAN.md Stage 1, workstream B),
agreed with Arthur after comparing an independent review (GPT-6 Astra) and
studying `../neomacs` and `../doomconfig`. They implement REQUIREMENTS.md; the
first slice will test them.

The editor comes before the desktop. It runs hosted in an ordinary window; the
same model later becomes the desktop (section 8), so nothing here depends on
Wayland. It also runs in a terminal and, remotely, in a browser (section 6).

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
| View | One interaction with that state | selections, scroll, folds, input profile; two views of one document differ |
| Presentation | An immutable snapshot of rows (section 2) | derived; maps back to targets and source ranges |

Commands act on typed targets. Staging a hunk is an action on a hunk target and
the status view is presented again from the git model; deleting display text
never stages anything. Closing the last view of a buffer never silently drops a
dirty document or a persistent task.

How applications map:

- **Source file:** text document; edits are revision-checked transactions.
- **VCS status, agenda, mail list:** a model plus a row provider; actions on
  targets (hunk, heading, message). Agenda actions edit the underlying Org text.
- **Terminal:** the terminal engine's grid plus bounded scrollback, as rows;
  input goes to the PTY, a copy mode selects.
- **REPL, agent conversation:** a log of entries (inputs, results, conditions,
  messages, tool calls) as rows, plus an editable input document.
- **Lens:** rows whose text maps to `{document, revision, anchored range}`
  segments; editing goes through to those documents (REQUIREMENTS, lenses).

## 2. One presentation primitive: rows

A presentation is an ordered stream of rows produced lazily by a row provider
for the visible range plus a margin. A row has:

- a stable key (scroll anchoring, folds and row selections survive updates);
- text runs: text, a style role, an optional target and an optional source map;
- section metadata (id, depth) for folding and navigation, not a widget tree;
- at most one embedded block (image, rule, progress bar) with a measured size.

A presentation may declare column stops, so mail lists, logs and tables align
with proportional fonts. Content that is truly two-dimensional (a PDF page, a
graph) is a block that draws and hit-tests itself: it exposes targets but gives
up the generic text features, deliberately second class. Dashboards and
side-by-side diffs are layouts of several views, not one presentation.

Every generic feature is written once against rows: motion, search, selection,
copy, the target at point (Embark), hit-testing, laziness for huge lists, and
what an agent is shown. A second primitive can be added later if one is ever
needed; removing one is not possible, so we start with one.

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
races. The microscope can answer "why is this underlined" and "who hid these
lines". Read-only is an editing capability, not a property.

## 4. Positions, selections and the two key profiles

Coordinate spaces stay separate: source positions (byte offsets at a revision),
editing units (graphemes, words, syntax objects), presentation positions (row,
run) and screen positions (shaped geometry). LSP conversions happen in the LSP
adapter. There is no universal "column".

Editing is one algebra:

- an ordered set of selections with a primary one; a selection is a directed
  anchor/head range, a caret when empty; characterwise, linewise or rectangular;
- motions and text objects return ranges;
- operators consume ranges and produce transactions.

Two input profiles over the same commands, equal from the first slice:

- **Emacs chords** (the default): point and mark are the primary selection and
  its inactive saved anchor.
- **Modal** (Vim/evil-like): normal, insert, visual and operator-pending states.
  Modal is a translation of neither; it is a test of the algebra's generality.

Keymaps resolve through declared scopes (transient controls, input state, view
mode, enabled packages, global) with a specified precedence; the microscope
shows the winning binding and what it shadows. Operator-pending is the same
cancellable state machine as an Emacs prefix key. Repeat (`.`) records intent
("change the next inner string to this text"), not raw keys; keyboard macros
exist but are not the basis of repeat or recipes.

## 5. History

Undo belongs to the text document and records transactions with their actor and
operation, so your undo does not silently consume an agent's edit; reversal
checks current state. Domain actions (staging, sending mail, running a command)
have their own recovery or none; text undo never pretends to reverse them.
Selection and scroll restoration is separate navigation history. The edit
journal (PLAN.md section 1) makes unsaved text survive a runtime crash.

## 6. Frontends

The runtime holds documents, views and presentations; a frontend draws
presentation snapshots and sends input back, over one presentation protocol
(as Neovim's UI protocol lets many GUIs share one editor). This is also what
lets the runtime restart while a window stays, and what makes remote use work.

- **Snapshots:** immutable, with ids: prepared, active, retired. A frontend
  draws and hit-tests the active one, so a click never lands in stale geometry.
  Updates are deltas of rows, keyed (section 2).
- **Layout belongs to the frontend.** The GPU frontend measures shaped glyphs,
  the terminal counts cells. Vertical motion, hit-testing and "what is visible"
  ask the active frontend's layout; there is no global geometry.
- **Capabilities:** each frontend declares what it has (proportional fonts,
  images, true color, key protocol). Presentations degrade instead of the
  design shrinking: embedded blocks fall back to text, column stops become cell
  columns, images use the kitty or sixel protocols where a terminal has them.
  Keys a terminal cannot send (some chords; the kitty keyboard protocol helps)
  are a property of the key normalizer, not of the keymaps.
- **In process or remote:** the hosted GPU frontend runs in the runtime's
  process without serializing; the terminal frontend can too; a browser
  frontend talks to a native runtime (local, or on a node) over the network.

Order: the GPU frontend first; the terminal second (it validates the protocol
early and its tests run headless); the browser (WebGPU, which `wgpu` targets, or
a canvas) when there is a reason. Compiling all of Techne to WebAssembly is not
a goal: the JIT, processes, files and nodes do not exist there. The desktop is
the GPU frontend only.

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

## 8. Windows, and later the desktop

The window tree is Lisp, like Emacs' `window.el`: splits, "where does this
buffer go", the frame/strip arrangement. Lisp computes rectangles; Rust applies
them atomically. The frontend realizes them:

- **hosted, terminal or browser:** one window, screen or page; the frontend
  draws all the editor's windows inside it;
- **desktop (later, GPU):** each window is a surface the compositor places, and
  application buffers are real Wayland windows.

The default window manager is a Lisp package with the Emacs/EWM feel; others
can write their own against the same primitives (surfaces, rectangles, focus,
input routing, atomic layout transactions). See PLAN.md, "The compositor".

## 9. Minibuffer and completion

The target is Arthur's current Doom setup: Vertico, Orderless, Consult, Embark,
Marginalia and nerd-icons, Corfu, Transient, which-key, popup rules.

- **Minibuffer:** a view with an input document and a candidate row list.
- **Candidates** carry targets, so Embark-style actions apply to them directly.
- **Marginalia:** annotation columns derived from the candidate's type.
- **Consult preview:** showing the target in another view while moving.
- **Orderless:** a matching style written in Lisp.
- **Transient:** argument controls generated from command schemas.
- **Corfu:** an in-buffer candidate popup over the same candidate protocol.

Screenshots of the Doom setup, taken in an off-screen session, will be the
reference when the minibuffer is built.

## 10. The first slice

Techne's own Lisp, on a general text editor: open and save files, recover unsaved
edits after a crash, undo, two views of one document, incremental search, the
minibuffer with completion, evaluation in the file's module, inspecting results,
jumping to definitions, and one editable search lens. Both key profiles, in the
GPU frontend; the presentation protocol is real from the start, so the terminal
frontend can follow without redesign. The loop: edit a command, evaluate it,
invoke it, inspect it, revise it, without a restart. Org files must open and
survive edits byte for byte; agenda and rich Org come later.

## 11. Hardest to change later

1. Authority and identity: documents own content, views do not; resource
   references are not live handles; every write has a conflict contract.
2. Positions and selections: anchors, display mappings, selection sets; layout
   owned by the frontend.
3. Command and history semantics: commands act on targets; text undo, domain
   recovery and irreversible effects are different contracts.

These get settled in the first slice; everything else stays replaceable.
