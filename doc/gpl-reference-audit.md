# GPL reference and provenance inventory

**Audit snapshot: 2026-11-14.** Findings, line numbers and statements about
what is present or planned describe the audited base below, not current
`main`. Check current history and plans before acting on a follow-up.

Audited base: `285d96fc` (`vkusmywn`, completion/separator), in the separate
jj workspace `gpl-reference-audit`. This is a list of review points, not a
finding of infringement or a legal opinion. No implementation was changed.

## Summary

- The seven completion/discovery references below appear on **122 distinct
  lines across 39 files**. This includes documentation, tests, generated
  documentation, identifiers and face names, not just implementation comments.
- Corfu alone: **17 lines / 5 files**. Orderless: **9 lines / 5 files**.
- The clearest actual code import is **EWM's GPL compositor in history**.
  The clearest planned extraction is **TRAMP-RPC's GPL server handlers**.
  Neither should be confused with merely naming an interaction precedent.
- The inspected completion files describe behavioral references; that does
  **not establish whether their implementation was independently authored**.
  No upstream implementation-to-implementation similarity audit was done.
- Scheme is not a licensing escape hatch: translating protected code can
  remain a derivative work. Independently implementing an idea or observed
  behavior is a different question. Removing names/comments cannot resolve
  copied-code provenance; retain attribution and investigate the origin.

Techne's declared license is `MIT OR Apache-2.0` (`LICENSE:1-15`,
`Cargo.toml:5-6`). `deny.toml:18-40` allows permissive dependency licenses,
but a Cargo dependency check cannot establish the provenance of handwritten
Scheme/Rust, copied examples or historical imports.

## 1. Completion and discovery: implemented references

All seven named upstream packages have GPL-3.0-or-later source headers.
Counts are matching **lines / files**, case-insensitive; rows overlap.

| Reference | Lines / files | What Techne explicitly follows; primary evidence |
| --- | --- | --- |
| **Corfu** | **17 / 5** | Automatic popup after two characters and a pause; candidate preview in the buffer; prompt preselection; separator insertion; boundary behavior; popup dimensions. `lisp/editor/completion.scm:1-33,89-94,170-184`; `crates/techne-editor/tests/completion.rs:1,35,239`; `crates/techne-term/src/lib.rs:52`; `crates/techne-editor/src/present.rs:367`; `EDITOR.md:574-586`. |
| **Orderless** | **9 / 5** | Space-separated components matching in any order, shared by minibuffer and completion popup. `lisp/editor/minibuffer.scm:1-5`; `lisp/editor/completion.scm:9,17,170-173`; `crates/techne-editor/tests/completion.rs:82`; `EDITOR.md:564-566,584-585`; `PLAN.md:753`. |
| **Vertico**, including vertico-directory | **25 / 9** | Vertical candidate layout, candidate count, selectable prompt/input, cycling, history ordering and directory deletion/tidying. `lisp/editor/minibuffer.scm:3,28,180,225,376,454-458`; `lisp/editor/buffers.scm:180,248,263,299`; `crates/techne-editor/tests/minibuffer.rs:68,113,151,471,536`; `EDITOR.md:581`. Also tests in `dired.rs`, `inspect.rs`, presentation comments and generated `lisp/runtime-catalog.json`. |
| **Consult** | **14 / 9** | Preview while selecting, asynchronous project search, xref, kill-ring selection and diagnostics. `lisp/editor/projects.scm:1-2`; `xref.scm:11`; `main.scm:150`; `lsp.scm:1070`; `EDITOR.md:562-569`; `PLAN.md:753`; tests in `minibuffer.rs` and `killring.rs`. **A commented Emacs Lisp example uses Consult's private APIs** in `lisp/editor/examples/goto-definition.scm:2-14`; establish its provenance rather than assuming it was copied upstream. |
| **Embark** | **12 / 10** | Typed targets and context actions, at point and on candidates; export into editable lenses. `lisp/editor/targets.scm:1-5`; `minibuffer.scm:20-22`; `lens.scm:9`; `EDITOR.md:560,571`; `REQUIREMENTS.md:27`; `doc/operations.md:8`; `runtime/TECHNE-VM.md:458`; `crates/techne-vm/examples/actions.scm:1`; tests and `PLAN.md:754`. |
| **Marginalia** | **6 / 5** | Annotation columns and command-key display preferences. `lisp/editor/minibuffer.scm:46-48`; `help.scm:50`; `main.scm:100`; `EDITOR.md:555-561`; `PLAN.md:753`. |
| **which-key** | **49 / 21** | Delayed prefix-key hints, immediate updates once open, sorted keys, named groups, description truncation and popup layout. `lisp/editor/which-key.scm:1-5,18-20,30-80`; `keymaps.scm:58,80`; `faces.scm:382-384`; `main.scm:271`. The name is also part of Scheme state/options, generated docs, Rust presentation/runtime/face code, both frontends and tests. This is more than a removable header comment. |

The central design specification is `EDITOR.md:553-587`: it explicitly calls
Arthur's Doom setup a design reference, not a compatibility commitment, and
maps each package to Techne features. `PLAN.md:752-755` repeats that mapping.

Not everything is Scheme: the shared incremental matcher is Rust
(`crates/techne-editor/src/matcher.rs:1-11`), invoked by the Scheme completion
policy. Any provenance review must cover that implementation too.

## 2. Other GPL references and reuse plans

| Reference | Evidence and classification |
| --- | --- |
| **EWM compositor — confirmed historical import** | `PLAN.md:102-111` explicitly records an attributed GPL-3.0-or-later import under `crates/techne-compositor`, removed until Stage 4, and proposes reusing its periphery. History confirms import `rwoqvmpx` / `287ca007` ("Import EWM's compositor as crates/techne-compositor") and removal `rvnrvtwr` / `85fecdc3` ("Drop the parked compositor until Stage 4"). **Not present in the audited current tree**, but retained in history. `REQUIREMENTS.md:344,384-386` and `PLAN.md:63-64,312` still propose reuse. EWM name: 9 lines / 3 current files. Review license compatibility before resurrection; the top-level permissive grant does not relicense it. |
| **TRAMP / emacs-tramp-rpc — implemented behavior plus planned code extraction** | TRAMP filename syntax and routes are implemented in `lisp/editor/remote.scm:1-4,82-85`; framing is attributed in `crates/techne-node/src/protocol.rs:1-3`. `doc/remote.md:3-25` explicitly says the shared engine is **not yet built**. Nevertheless `doc/remote.md:300-331` plans to start the engine with extracted tramp-rpc handlers and their tests. Upstream emacs-tramp-rpc declares GPL-3.0-or-later in its Cargo workspace. Treat this planned extraction as a license decision, not just inspiration. Combined TRAMP/tramp-rpc name matches: 33 lines / 9 files. |
| **neomacs — rendering design reference; borrowing explicitly contemplated** | `EDITOR.md:4,504-527` records a study of neomacs, labels it GPL-3.0, and says any borrowed code must retain its upstream license. `PLAN.md:313` follows its rendering stack. 4 lines / 2 files. This is evidence of a reference and a conditional borrowing policy, **not proof of a code import**; inspect frontend history if stronger assurance is needed. |
| **Magit — implemented status-view analogy and remote planning** | GPL-3.0-or-later. `lisp/editor/packages/vc.scm:1-4` compares its status view to Magit/vc-dir. `EDITOR.md:18` uses Magit status as a document-model example. `doc/remote.md:64-65,354` discusses Magit prefetch. 5 lines / 3 files; no copied Magit implementation established here. |
| **Evil — modal behavior/key references** | GPL-3.0-or-later. `lisp/editor/modal.scm:12`; `main.scm:219,241`; `EDITOR.md:315`; `crates/techne-editor/tests/minibuffer.rs:379`. 5 relevant lines / 4 files, excluding an unrelated `evil.test` hostname. Review as behavior/keymap references, not a proven Evil port. |
| **Transient — argument-control design reference, partly deferred** | GPL-3.0-or-later. `EDITOR.md:556,572,728`; `PLAN.md:199`; `REQUIREMENTS.md:399`; `doc/operations.md:397`. 6 package-reference lines / 4 files. Generic `transient` variables, transient mark mode and allocation comments are not evidence of using this package. |
| **el-be-back — Scheme-first terminal architecture precedent** | Upstream repository has a GPLv3 LICENSE. `AGENTS.md:21` and `PLAN.md:318` use it as the example for implementing terminal emulation in Lisp rather than wrapping a native subsystem. 2 lines / 2 files; no emulator import established. |
| **niri — future compositor engineering reference** | GPL-3.0-or-later in upstream Cargo metadata. `PLAN.md:312`, 1 line / 1 file. Design-only reference in the inspected tree. |
| **Elpaca — package-management analogy** | GPL-3.0-or-later. `EDITOR.md:707` refers to git packages pinned in lockfiles, "as Doom and elpaca pin today". 1 line / 1 file; not an established implementation port. |
| **nerd-icons.el — reference setup only** | GPL-3.0-or-later in its source header. Named in `EDITOR.md:556`, 1 line / 1 file. Do not confuse the Emacs package's license with the separate licenses of fonts/icon assets. No imported icon implementation or asset established here. |

## 3. GNU Emacs itself, examples, and non-GPL false positives

An `emacs` name search finds **553 lines / 120 files**. Much of that is the
`emacs` key profile and tests, not borrowed implementation. Review these more
specific provenance surfaces instead of treating every match as equivalent:

- **Indentation:** `crates/techne-editor/src/lisp_indent.rs:1-13,30-56`
  explicitly follows `calculate-lisp-indent` / `scheme-indent-function`,
  including an indentation-spec table. Scheme integration is in
  `lisp/editor/indent.scm:1-7`. This merits an origin review, including Rust,
  not a conclusion that the algorithm or table was copied.
- **Eldoc and ordinary editor features:** `lisp/editor/eldoc.scm:1-11`
  explicitly follows Emacs; `emacs.scm`, `commands.scm`,
  `packages/dired.scm`, `listings.scm`, `windows.scm` and keymap/mode code
  implement familiar Emacs interactions. Names and familiar behavior alone
  do not establish source translation.
- **Side-by-side Emacs Lisp/Scheme examples:**
  `lisp/editor/examples/upcase-region.scm:1-8`,
  `todo-mode.scm:1-13`, `directory-todos.scm:1-22` and
  `goto-definition.scm:1-14` contain actual commented Elisp snippets, not
  only package names. Their source/author is not established by this audit.
  The Consult example is the most directly relevant to this request.
- **External Emacs experiment:** `REQUIREMENTS.md:388-389,412-414` names an
  Emacs revision and an experiment document in that reference repository.
  It explicitly calls that repository read-only reference material.

Not every Emacs-related project is GPL:

- **Doom Emacs itself is MIT**; its packages have separate licenses.
  The Doom-setup references must not all be classified as GPL code.
- **Geiser is BSD-3-Clause** in the inspected source header.
  Its 7 matching lines / 4 files concern live Scheme interactions and keys
  (`lisp/editor/live.scm:3,194,206`, tests and `PLAN.md:736`).
- **SLIME's README says public domain unless a file states otherwise**.
  `REQUIREMENTS.md:403` is an inspection/recovery design reference; do not
  label all of SLIME GPL without examining the particular source file.
- **Catppuccin's Emacs theme is MIT**; the palette is explicitly named in
  `lisp/editor/faces.scm:341`. Any actual reuse still needs its applicable
  notices, but this is not a GPL example.
- Imported SRFI libraries retain separate notices (`LICENSE:9-11`);
  these should not be swept into a GPL cleanup based on the word "Lisp".

## 4. Suggested order of follow-up

1. Keep the historical EWM import and planned TRAMP-RPC extraction behind
   an explicit licensing/provenance decision. A shared author name alone
   does not prove ownership of every contribution or the right to relicense it.
2. Review introduction history for completion, minibuffer, matching, hints
   and indentation. Useful starting changes: `okmtnszy` / `13f3b04f`
   (Corfu-like completion), `wpxktmwv` / `e5e045c3` (minibuffer), and
   `vkusmywn` / `285d96fc` (completion separator). Establish what inputs
   were used: observed behavior, original specifications, or upstream source.
3. Establish the provenance of the four commented Elisp examples.
4. If the policy is to avoid source-derived GPL implementations, write
   original behavior requirements and preserve evidence of independent
   authorship. Get legal review for uncertain existing adaptations.
5. Only then consider neutral feature names/documentation. Do not erase
   notices or treat removing "Corfu" / "Orderless" as remediation.

## Method and limits

Counts use `rg -c -i --stats --glob '!Cargo.lock' PATTERN .` through
`techne tool` in the audit workspace, before adding this report. Patterns
are word-bounded names (hyphens count as boundaries), so `corfu-auto`,
`consult-ripgrep` and `vertico-directory` are included. The scan searched
677 files under rg's normal non-hidden/non-ignored traversal. Generated
`lisp/runtime-catalog.json` is included; comments and tests count too.

The combined seven-name search returned 123 lines / 40 files before
excluding the ordinary English "Consult the parent" in
`runtime/history/READINESS.md:502`; the Consult row likewise excludes it.
Evil and Transient counts above explicitly exclude unrelated word matches.
Counts are not amounts of GPL code and must not be added across rows to
obtain unique totals. This report itself would add many matches on a rerun.

Inspected current source/comments, relevant design documents, selected
introduction/removal history and upstream license declarations. This is
not a full historical scan, dependency-license audit, asset audit,
line-by-line comparison to upstream, or a certification of clean provenance.
Upstream links below are moving branches, checked for license declarations,
not pinned versions proven to be inputs to Techne's implementation.

### Upstream license evidence

- GPL-3.0-or-later headers: [Corfu](https://github.com/minad/corfu/blob/main/corfu.el), [Orderless](https://github.com/oantolin/orderless/blob/master/orderless.el), [Vertico](https://github.com/minad/vertico/blob/main/vertico.el), [Consult](https://github.com/minad/consult/blob/main/consult.el), [Embark](https://github.com/oantolin/embark/blob/master/embark.el), [Marginalia](https://github.com/minad/marginalia/blob/main/marginalia.el), [which-key](https://github.com/justbur/emacs-which-key/blob/master/which-key.el).
- Other GPL declarations: [Magit](https://github.com/magit/magit/blob/main/lisp/magit.el), [Transient](https://github.com/magit/transient/blob/main/lisp/transient.el), [Evil](https://github.com/emacs-evil/evil/blob/master/evil.el), [Elpaca](https://github.com/progfolio/elpaca/blob/master/elpaca.el), [nerd-icons.el](https://github.com/rainstormstudio/nerd-icons.el/blob/main/nerd-icons.el), [TRAMP-RPC Cargo metadata](https://github.com/ArthurHeymans/emacs-tramp-rpc/blob/master/Cargo.toml), [niri Cargo metadata](https://github.com/niri-wm/niri/blob/main/Cargo.toml), [el-be-back LICENSE](https://github.com/ArthurHeymans/el-be-back/blob/master/LICENSE).
- Non-GPL distinctions: [Doom MIT](https://github.com/doomemacs/doomemacs/blob/master/LICENSE), [Geiser BSD-3-Clause header](https://github.com/emacsmirror/geiser/blob/master/elisp/geiser.el), [SLIME README](https://github.com/slime/slime/blob/master/README.md#license), [Catppuccin MIT](https://github.com/catppuccin/emacs/blob/main/LICENSE).
- [GNU GPL FAQ: translating code to another programming language](https://www.gnu.org/licenses/gpl-faq.html#TranslateCode) explains why changing language does not by itself remove the original license obligations. This is license guidance, not a legal determination about Techne.
