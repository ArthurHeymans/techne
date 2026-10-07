# Techne

A live, programmable environment in the spirit of Emacs: an editor first,
later the desktop, extended and inspected in its own Lisp while it runs.
[REQUIREMENTS.md](REQUIREMENTS.md) says what it must be,
[PLAN.md](PLAN.md) how to get there, [EDITOR.md](EDITOR.md) designs the
editor.

The Lisp is meant to be a general-purpose language in its own right,
embeddable from Rust. Its runtime is described in
[runtime/TECHNE-VM.md](runtime/TECHNE-VM.md).

## Crates

| crate | what |
|---|---|
| `techne-vm` | the Lisp: reader, hygienic macros, compiler, interpreter, Cranelift JIT, generational GC, tasks, worlds; the `techne-vm` REPL |
| `techne-text` | text documents: changes, revisions, anchors, undo, the edit journal |
| `techne-editor` | documents and views for Lisp, and the runtime a frontend talks to; the commands and key profiles are in `lisp/editor` |
| `techne-window` | the `techne` binary: the editor in a GPU window (winit, wgpu, glyphon) |
| `techne-term` | the `techne-term` binary: the editor in a terminal, in cells, with the kitty keyboard protocol where there is one |
| `techne-lsp` | language server for other editors (Techne's own asks the running VM) |
| `techne-process` | child processes with pipes or a pty |
| `techne-node` | remote evaluation and processes, sessions, nREPL server |
| `techne-compositor` | Wayland compositor, imported from EWM (parked until Stage 4) |

`editors/emacs` has an Emacs client for the nREPL server.

## Building and testing

Everything runs in the flake's dev shell:

```sh
nix develop                     # Rust, valgrind, chez and the pinned test suites
cargo test --release            # all default crates
nix develop .#window            # plus what the editor window needs at run time
nix develop .#compositor        # plus the compositor's system libraries
```

`crates/techne-vm/tests/suites.rs` runs the Scheme suites: our own
(`tests/suites/lang`), chibi-scheme's R7RS conformance tests and the
r7rs-benchmarks programs, each in four modes (default, interpreter only, JIT
compiling everything, a GC on every allocation) that must agree.
`tests/suites/expected-failures.txt` lists what fails today; any change to
that set fails the run. Useful variables:

- `TECHNE_TEST_MODES=default,jit` selects modes;
- `TECHNE_BLESS=1` rewrites the expected failures from the run;
- `TECHNE_FUZZ_SEEDS`, `TECHNE_FUZZ_START` size the JIT fuzzer (`tests/fuzz.rs`).

## Performance

Wall time on a shared machine or CI runner depends on the load, so CI
compares instruction counts instead: `runtime/bench/icount.sh` runs each
benchmark under cachegrind with the JIT and with the interpreter and reports
the work of the longest GC pause. Repeat runs agree to about 0.001%; CI keeps
the history on the `gh-pages` branch and fails a pull request that regresses
a number by more than 5%. For wall-clock comparisons with other
implementations on a quiet machine, see `runtime/bench/run.sh`.

## License

GPL-3.0-or-later.
