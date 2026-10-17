# Techne

A live, programmable environment in the spirit of Emacs: an editor first,
later the desktop, extended and inspected in its own Lisp while it runs.
[REQUIREMENTS.md](REQUIREMENTS.md) says what it must be,
[PLAN.md](PLAN.md) how to get there, [EDITOR.md](EDITOR.md) designs the
editor. [doc/manual.org](doc/manual.org) says how to use it, and the
editor itself, under C-h, what each command, procedure and option does.

The Lisp is meant to be a general-purpose language in its own right,
embeddable from Rust. Its runtime is described in
[runtime/TECHNE-VM.md](runtime/TECHNE-VM.md). Its
[shipped Scheme libraries](lisp/srfi/README.md) include lists, sets and
bags, comparators, sorting, vectors, bitwise operations, random sources,
string cursors and regular expressions; [lisp/techne](lisp/techne) adds
base64, hex, SHA-2 digests, HMACs and random bytes (`(techne crypto)`),
UUIDs (`(techne uuid)`), and time zones and RFC 3339 (`(techne time)`).

## Crates

| crate | what |
|---|---|
| `techne-cli` | the `techne` binary: the editor and the other crates' commands as subcommands (`techne help`) |
| `techne-vm` | the Lisp: reader, hygienic macros, compiler, interpreter, Cranelift JIT, generational GC, tasks, worlds; `techne run` and `techne repl` |
| `techne-text` | text documents: changes, revisions, anchors, undo, the edit journal |
| `techne-editor` | documents and views for Lisp, and the runtime frontends attach to; the commands and key profiles are in `lisp/editor` |
| `techne-window` | `techne window`: the editor in a GPU window (winit, wgpu, glyphon) |
| `techne-term` | `techne term`: the editor in a terminal, in cells, with the kitty keyboard protocol where there is one |
| `techne-lsp` | `techne lsp`: language server for other editors (Techne's own asks the running VM) |
| `techne-process` | child processes with pipes or a pty |
| `techne-net` | TCP listeners and connections for the Lisp |
| `techne-node` | `techne node`: remote evaluation and processes, sessions, nREPL server |
| `techne-host` | serving deployments from worlds: instances started from snapshots, each request under a lifetime (doc/serverless.md) |
| `techne-compositor` | Wayland compositor, imported from EWM (parked until Stage 4; a workspace of its own) |

`editors/emacs` has an Emacs client for the nREPL server.

## Building and testing

Everything runs in the flake's dev shell. Nix and rustup use the nightly
pinned in `rust-toolchain.toml` for allocator API work; Miri and the coverage
LLVM tools come from the same toolchain.

```sh
nix develop                     # Rust, valgrind, chez and the pinned test suites
cargo test --release            # all crates but the compositor
nix develop .#window            # plus what the editor window needs at run time
nix develop .#compositor        # plus the compositor's system libraries,
                                # to build it in crates/techne-compositor
cargo build --release -p techne-cli                        # target/release/techne
cargo build --release -p techne-cli --no-default-features  # without the editor's frontends, for servers
nix build                       # the package: result/bin/techne, its Lisp in result/share/techne
```

`techne-vm`, `techne-node` and `techne-lsp` also build binaries of their own,
which their tests run. The counting allocator, which charges Rust
allocations to the world making them (`counting-allocator` in
`crates/techne-vm/Cargo.toml`), is the application's choice: `techne`
installs it, library crates do not, and their tests turn it on. Those
three binaries need it too: `cargo test -p techne-node` alone skips the
tests running them, `--features counting-allocator` builds them.

`crates/techne-vm/tests/suites.rs` runs the Scheme suites: our own
(`tests/suites/lang`), chibi-scheme's R7RS conformance tests and the
r7rs-benchmarks programs, each in four modes (default, interpreter only, JIT
compiling everything, a GC on every allocation) that must agree.
`tests/suites/expected-failures.txt` lists what fails today; any change to
that set fails the run. Useful variables:

- `TECHNE_TEST_MODES=default,jit` selects modes;
- `TECHNE_BLESS=1` rewrites the expected failures from the run;
- `TECHNE_FUZZ_SEEDS`, `TECHNE_FUZZ_START` size the JIT fuzzer (`tests/fuzz.rs`).

Everything the editor loads is documented: every name a module provides,
every built-in, command, mode, option and hook has a docstring following
the convention in [runtime/TECHNE-VM.md](runtime/TECHNE-VM.md)
("Docstrings"). `crates/techne-editor/tests/documentation.rs` fails on
anything missing or wrong, as M-x checkdoc shows it and the language
server warns while a file is edited; `tests/help.rs` checks that every
key and name the manual refers to exists. CI also builds the Rust
documentation with warnings denied and checks spelling with `typos`.

## Performance

Wall time on a shared machine or CI runner depends on the load, so CI compares
instruction counts instead: `runtime/bench/icount.sh` runs each benchmark
under cachegrind with the JIT and with the interpreter and reports the work of
the longest GC pause; it also counts the instructions per request of the HTTP
benchmark's server (`runtime/bench/http`) on in-memory connections, and per
key of the editor's keystrokes. Repeat runs agree to about 0.01%; CI keeps the
history on the `gh-pages` branch and fails a pull request that regresses a
number by more than 5%. For wall-clock comparisons with other implementations
on a quiet machine, see `runtime/bench/run.sh`.

## License

Techne-authored code and documentation are dual-licensed under
[MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option
(`MIT OR Apache-2.0`).

Imported code retains its original licenses and notices. In particular,
the [EWM-derived compositor](crates/techne-compositor/README.md) remains
GPL-3.0-or-later, and imported Scheme libraries retain their
[upstream licenses](lisp/srfi/README.md#upstream-sources).
See [LICENSE](LICENSE) for the scope of the dual license.

Unless explicitly stated otherwise, contributions intentionally submitted
for inclusion in Techne-authored code are offered under both licenses.
