# Repository-tool regression suites

Run the normal suite directly in Scheme:

```
target/debug/techne run lisp/tests/tools/run.scm
```

Run the CLI suite with the real binary as its argument (use its absolute path):

```
target/debug/techne run lisp/tests/tools/cli.scm /absolute/path/to/techne
```

These suites live in their own directory because the VM-only harness discovers
all top-level `lisp/tests/*.scm`; tool suites need node bindings or the real CLI.

The suites use `lisp/test.scm`. `support.scm` supplies named cases,
short-lived temporary directories and text/binary fixtures. Case exceptions count
as failures, directories are cleaned on either path, and `test-summary` exits
nonzero if any check fails. The CLI suite creates isolated node directories and
explicitly shuts its nodes down; it tests the actual CLI, not a replacement
renderer.

Cargo runs these same Scheme files:

```
cargo test -p techne-node --test tools_scheme
cargo test -p techne-cli --test tools
```

The node bridge only installs the VM's native mechanisms and chooses grants.
`help.scm` must run in its host-created world without file/process
grants, so use Cargo for that case. Scheme cannot grant itself those privileges.
The CLI bridge only supplies the binary path and checks the suite's exit status.
There are no tool assertions or fixture programs embedded in either bridge.

## Migration coverage

This is the test-only follow-up to the source-tool dogfooding changes. It moves
all 17 cases in the five Rust files below, plus the two new CLI-rendering cases:

| Former Rust suite | Scheme cases |
|---|---|
| `citations.rs` | `citations.scm` (3) |
| `compact.rs` | `compact.scm` (2) |
| `enclosing.rs` | `enclosing.scm` (3) |
| `search.rs` | `search.scm` (5) |
| `tools_shared.rs` | `shared.scm` (3), restricted `help.scm` (1) |
| CLI source rendering | `cli.scm` (2) |

Fixtures still exercise external disk edits, invalid UTF-8, oversized files,
cache eviction pressure, mutable returned strings, Unicode coordinates, partial
syntax, literal/grouped/context search, budget continuation, raw/structured CLI
output and process-label/source-file distinction. The older Rust integration
suites are intentionally unchanged. Cargo's wrapper count is smaller; the cases
and runtime checks now live in Scheme instead.
