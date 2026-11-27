# Repository-tool regression suites

Run all repository-tool suites with `techne test`:

```
target/debug/techne test lisp/tests/tools
```

Each file declares its world via `(test-file ...)` — `#:environment 'host`
for node/CLI suites, and `#:environment 'host #:without '(files processes)`
for `help.scm`. `support.scm` (marked `#:ignore #t`) supplies shared fixture
helpers on top of `(techne test)`.

CI runs these same Scheme files directly through `techne test --all`.

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
