# techne-vm and R7RS

techne-vm implements R7RS-small with the deviations below, each decided
once. Conformance is measured by chibi-scheme's R7RS suite and the
r7rs-benchmarks programs, run in every execution mode by
`crates/techne-vm/tests/suites.rs`. Every failure they still show is listed
in `tests/suites/expected-failures.txt` under the tag of its deviation, and
a test checks that each tag there is documented here.

Libraries (5.6) are supported: `define-library` with `export` (including
`rename` and re-exports of imports), `import`, `begin`, `include`,
`include-ci`, `include-library-declarations` and `cond-expand`, and import
sets with `only`, `except`, `prefix` and `rename`. A library is a module
named by its written name, such as `(srfi 1)`, that sees only what it
imports. Libraries not yet defined are loaded from `a/b.sld` (for `(a b)`)
in the importing file's directory or on `TECHNE_LIBRARY_PATH`. The R7RS
libraries are views of the root module: `(scheme base)`, `(scheme char)`,
`(scheme cxr)`, `(scheme case-lambda)`, `(scheme eval)`, `(scheme file)`,
`(scheme inexact)`, `(scheme lazy)`, `(scheme process-context)`,
`(scheme read)`, `(scheme repl)`, `(scheme time)`, `(scheme write)` and
`(scheme r5rs)`, less the identifiers the deviations below leave out;
`(techne)` is the whole root module. Importing another `(scheme ...)`
library, such as `(scheme complex)`, is an error, and `cond-expand` knows
it is missing. Syntax (`define`, `lambda`, `if` and the other special
forms) is visible everywhere. `environment` gives a fresh module seeing only
its imports; `scheme-report-environment` imports `(scheme r5rs)` and
`null-environment` nothing. Ordinary modules (files, the REPL's) see the
whole root module as well as their imports.

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

An editor and desktop runtime has no use for them, and they would cost
every arithmetic path. `real?` and `complex?` are `number?`. Complex number syntax (`1+2i`,
`+i`, `1@2`) is a read error rather than an identifier. `sqrt`, `log`,
`asin` and `acos` outside their real domain give NaN. `make-rectangular`,
`make-polar`, `real-part`, `imag-part`, `magnitude` and `angle` are absent.

### `string-size`: strings change in place only at the same UTF-8 size

Strings are UTF-8 in one block. `string-set!`, `string-fill!` and
`string-copy!` change a string in place when the new characters take as
many bytes as those they replace, which is always so for ASCII; otherwise
they raise an error, and a new string has to be built (`string-append`,
`string-map`, a string port). String literals cannot be changed. Changing a
string that is a key of an `equal?` hash table loses its entry.
`string-ref` and `string-length` are constant time on ASCII strings and
linear otherwise; string cursors (Stage 1 step 12) are the way to walk text.

Strings that grow or shrink in place would need a string object pointing
to its bytes, an indirection on every string operation; no workload has
asked for it.

### `escape-continuations`: continuations only escape

`call/cc` (also spelled `call/ec`, which says what it is) captures an
escape-only continuation: calling it inside the extent of its `call/cc`
unwinds to it, through `dynamic-wind` exits. Calling it after that extent
has ended is an error the program can catch. Generators and coroutines use
tasks and channels in Techne code; portable libraries built on re-entered
continuations do not run.

## Smaller choices

- `utf8->string` refuses invalid UTF-8 (an error naming the byte where
  it starts) rather than replacing it. Bytevector literals, like string
  literals, cannot be changed.
- The standard input and output ports are textual; binary I/O goes
  through bytevector and file ports.
- `#!fold-case` and `#!no-fold-case` hold until the end of the datum
  `read` returns, not for the rest of the port.
- The exponent markers `s`, `f`, `d` and `l` of R5RS read as `e`.
- `write` writes data (numbers, strings, characters, symbols, booleans,
  lists, vectors, bytevectors) as text `read` gives back; other objects (procedures,
  ports, records, hash tables) print as `#<...>`, which does not read. It
  uses datum labels only for cycles; `write-shared` labels all sharing and
  `write-simple` none. Identifiers that could be taken for
  numbers by other readers (`|1+|`, `|+inf.0x|`) are written between bars.
- Floats print with the shortest digits that read back exactly, with an
  exponent outside 1e-6 to 1e21 (`1.0e+21`, `5.0e-324`).
- `char-foldcase` and `string-foldcase` use Unicode simple case folding
  (plus `ß` to `ss`); `digit-value` knows every Unicode decimal digit.
- `features` is `r7rs exact-closed ratios-as-floats full-unicode`, the
  operating system and architecture, and `techne`.
- `exit` and `emergency-exit` ask the host to end the program: the request
  unwinds past every handler (running `dynamic-wind` exits) to the host,
  which decides what ending means; `techne-vm` exits with the status. They
  need the host-control capability, file procedures and `include` the
  files and loading capabilities, and `get-environment-variable(s)` the
  environment capability; a world without them sees them unbound or
  refused.
