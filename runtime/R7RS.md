# techne-vm and R7RS

techne-vm implements R7RS-small with the deviations below, each decided
once. Conformance is measured by chibi-scheme's R7RS suite and the
r7rs-benchmarks programs, run in every execution mode by
`crates/techne-vm/tests/suites.rs`. Every failure they still show is listed
in `tests/suites/expected-failures.txt` under the tag of its deviation, and
a test checks that each tag there is documented here.

Libraries (5.6) are supported: `define-library` with `export` (including
`rename`), `import`, `begin`, `include`, `include-ci`,
`include-library-declarations` and `cond-expand`, and import sets with
`only`, `except`, `prefix` and `rename`. A library is a module named by its
written name, such as `(srfi 1)`. Libraries not yet defined are loaded from
`a/b.sld` (for `(a b)`) in the importing file's directory or on
`TECHNE_LIBRARY_PATH`. The standard libraries `(scheme ...)` are the root
module, which every module sees, so importing one adds nothing. For the
same reason `environment`, `scheme-report-environment` and
`null-environment` all give one shared module that sees the root.

## Deviations

### `rationals`: no exact non-integer numbers

Exact numbers are integers (fixnums and bignums), inexact numbers are
floats. `/` on exact integers gives the exact quotient when the divisor
divides evenly and the nearest float otherwise, so `(/ 6 3)` is `2` and
`(/ 1 2)` is `0.5`. A literal `n/d` reads the same way. `exact` of a float
with a fraction, and `#e` before one, are errors. `numerator` and
`denominator` of a float give those of its exact binary fraction, as
floats.

We keep `/` exact when it can be, rather than always inexact: portable
code often divides evenly and expects an exact integer (an index, a
count). Real rationals can come later behind the same `/` if a workload
needs them.

### `complex`: no complex numbers

`real?` and `complex?` are `number?`. Complex number syntax (`1+2i`,
`+i`, `1@2`) is a read error rather than an identifier. `sqrt`, `log`,
`asin` and `acos` outside their real domain give NaN. `make-rectangular`,
`make-polar`, `real-part`, `imag-part`, `magnitude` and `angle` are absent.

### `immutable-strings`: strings cannot be changed in place

Strings are immutable UTF-8. `string-set!`, `string-fill!` and
`string-copy!` are absent; build a new string instead (`string-append`,
`string-map`, a string port). `string-ref` and `string-length` are
constant time on ASCII strings and linear otherwise; string cursors (Stage
1 step 12) are the way to walk text.

### `escape-continuations`: continuations only escape

`call/cc` captures an escape-only continuation: calling it inside the
extent of its `call/cc` unwinds to it, through `dynamic-wind` exits.
Calling it after that extent has ended is an error the program can catch.
Generators and coroutines use tasks and channels instead.

### `bytevectors`: bytevectors and binary ports come with Stage 1 step 11

Not yet a deviation by decision: bytevectors, `#u8(...)` syntax, binary
ports and `utf8->string`/`string->utf8` arrive with step 11. Until then
`#u8(` is a read error, `binary-port?` is always false and
`textual-port?` always true.

## Smaller choices

- `#!fold-case` and `#!no-fold-case` hold until the end of the datum
  `read` returns, not for the rest of the port.
- The exponent markers `s`, `f`, `d` and `l` of R5RS read as `e`.
- `write` uses datum labels only for cycles; `write-shared` labels all
  sharing and `write-simple` none. Identifiers that could be taken for
  numbers by other readers (`|1+|`, `|+inf.0x|`) are written between bars.
- Floats print with the shortest digits that read back exactly, with an
  exponent outside 1e-6 to 1e21 (`1.0e+21`, `5.0e-324`).
- `char-foldcase` and `string-foldcase` use Unicode simple case folding
  (plus `ß` to `ss`); `digit-value` knows every Unicode decimal digit.
- `features` is `r7rs exact-closed ratios-as-floats full-unicode`, the
  operating system and architecture, and `techne`.
- `exit` and `emergency-exit` need the host-control capability, file
  procedures the files capability, and `get-environment-variable(s)` the
  environment capability; a world without them sees them unbound.
