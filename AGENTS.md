# Working in this repository

## Dogfooding

We want to dogfood our techne lisp.
Find or build a techne binary and use it with the `techne tool` command.
If you notice any issue or can suggest any improvements to these tools notify the user.
This tool can use scheme in a stateful way. This is available for you so you don't need to repeat yourself.
Unless there is a very good reason not to, try to make new tools in techne lisp if you think they are reusable
and useful. Don't hillclimb this however.

## Scheme first; Rust only when Scheme is not feasible

As an example a terminal emulator:
Write new functionality in Techne's Scheme (`lisp/`), the way
`../el-be-back` is a whole terminal emulator in Elisp: parsers,
protocols, terminal emulation, document formats and their rendering into
presentations, applications. Do not wrap a Rust (or C) library for a
subsystem because one exists.

Use Rust only where Scheme cannot do the job:

- an OS, hardware or GPU boundary (syscalls, PTYs, sockets, drawing);
- a safety invariant the runtime must hold whatever Lisp does
  (capabilities, the journal, revision checks);
- a measured performance need the VM cannot meet, after trying to meet
  it in Scheme.

Keep that Rust a small mechanism with its policy exposed to Lisp, and say
in the commit why it could not be Scheme.

## Look at what you changed, with screenshots

Before handing off a change that affects what is drawn — a frontend,
rendering, layout, faces, a theme, syntax highlighting, anything whose
result is pixels — take screenshots of it and **look at them yourself**.
Tests passing is not a substitute: they do not see colours swapped,
text clipped, underlines under the wrong thing, or a face silently
drawn plain.

The screenshots are for your own verification, not a deliverable. Do
not hand them to the user as the evidence; say what you looked at and
what you saw (and fix what looks wrong before handing off).

How, without touching the desktop:

    nix develop .#window --command \
      target/debug/techne run crates/techne-window/headless.scm COMMAND...

Inside, `grim FILE.png` takes the screenshot and `wtype` types (start
each `wtype` with a key and a pause, `wtype -k Shift_L -s 300 ...`:
keys a new `wtype` sends before the window has its keymap are lost). Run the debug binary
(`target/debug/techne window FILE`) in the background, give it a few
seconds to draw, then `grim`. Read the PNG back and check it against
what the change was supposed to do — including the corners: mode line,
gutter, minibuffer, popups, unfocused panes. For a change both
frontends draw, check the terminal frontend too (its tests' grids, or
a capture of its output), not only the window.
