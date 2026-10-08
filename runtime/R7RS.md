# techne-vm and R7RS

techne-vm implements R7RS-small, with the one deviation below.
Conformance is measured by chibi-scheme's R7RS suite, the portable SRFI
libraries of chibi's tree loaded unchanged with their tests (SRFI 1, 117,
133 and 158), chibi's tests of the SRFIs Techne provides itself (SRFI 69),
the reference implementations of SRFI 113 (sets and bags), 128
(comparators), 132 (sorting), 133 (vectors) and 151 (bitwise operations)
likewise, and the r7rs-benchmarks programs, run in every execution mode by
`crates/techne-vm/tests/suites.rs`. Every failure they still show
is listed in `tests/suites/expected-failures.txt` under the tag of its
deviation, and a test checks that each tag there is documented here.

Libraries (5.6) are supported: `define-library` with `export` (including
`rename` and re-exports of imports), `import`, `begin`, `include`,
`include-ci`, `include-library-declarations` and `cond-expand`, and import
sets with `only`, `except`, `prefix` and `rename`. A library is a module
named by its written name, such as `(srfi 1)`, that sees only what it
imports. Libraries not yet defined are loaded from `a/b.sld` (for `(a b)`)
in the importing file's directory or on `TECHNE_LIBRARY_PATH`. The R7RS
libraries are views of the root module: `(scheme base)`, `(scheme char)`,
`(scheme complex)`, `(scheme cxr)`, `(scheme case-lambda)`,
`(scheme eval)`, `(scheme file)`, `(scheme inexact)`, `(scheme lazy)`,
`(scheme process-context)`, `(scheme read)`, `(scheme repl)`,
`(scheme time)`, `(scheme write)` and `(scheme r5rs)`, and so is
`(srfi 69)` (hash tables); `(techne)` is the whole root module.
`(srfi 27)` (random numbers) is Techne's own as well, a library in Scheme
compiled when a program first imports it. Importing another
`(scheme ...)` library is an error, and `cond-expand` knows it is
missing. Syntax (`define`, `lambda`, `if` and the other special forms) is
visible everywhere. `environment` gives a fresh module seeing only
its imports; `scheme-report-environment` imports `(scheme r5rs)` and
`null-environment` nothing. Ordinary modules (files, the REPL's) see the
whole root module as well as their imports.

## Numbers

Exact numbers are integers (fixnums and bignums) and ratios, inexact
numbers are floats. `/` of exact numbers is exact: `(/ 6 3)` is `2` and
`(/ 1 2)` is `1/2`, and code that wants a float says so with a float
operand or `inexact`. A ratio's arithmetic is exact and allocates, and
its terms can grow; it never becomes a float on its own. Conversions to
floats round to the nearest, and `exact` of a float gives its exact
binary value (`(exact 0.1)` is `3602879701896397/36028797018963968`).
Comparisons between exact and inexact numbers compare exact values.

Non-real numbers are complex, with any two real numbers as parts, so
`1/2+3i` is exact; an exact zero imaginary part makes a number real
(`3+0i` is `3`) and an inexact one does not (`-2.5+0.0i` is not `real?`).
`sqrt`, `log`, `expt`, `asin` and `acos` give complex results outside the
real domain (`(sqrt -4)` is `+2i`), and real ones inside it, as before.
`<` and the other orderings, rounding and integer division take real
numbers only. Ratios and complex numbers live off the fixnum and float
fast paths, as bignums do, so they cost nothing to code that does not use
them.

## Deviation

This is a choice for the language, not work left.

### `escape-continuations`: continuations only escape

`call/cc` (also spelled `call/ec`, which says what it is) captures an
escape-only continuation: calling it inside the extent of its `call/cc`
unwinds to it, through `dynamic-wind` exits. Calling it after that extent
has ended is an error the program can catch. Generators and coroutines use
tasks and channels in Techne code; portable libraries built on re-entered
continuations (SRFI 158's reference generators, for one) do not run
unchanged.

Re-entering a continuation would mean resuming across Rust frames of
natives and async tasks, keeping whole stacks (and with them retired
package generations) alive, and running again code that already committed
a channel `select` or released a scope's resources.


## Smaller choices

- Literals in code (quoted lists, vectors, strings and bytevectors, and
  their self-evaluating forms) cannot be changed: `set-car!`,
  `vector-set!` and the others raise an error. Data from `read` and from
  constructors can. The check is the kind test these operations already
  make, with the literal flag in its mask.
- Strings are UTF-8 in one block. `string-set!`, `string-fill!` and
  `string-copy!` change the bytes where they are when the new characters
  take as many bytes as the old (always so for ASCII); otherwise the string
  gets new bytes and points to them, keeping its identity, so only a
  string that has changed size pays for an indirection. `string-ref` and
  `string-length` are constant time on ASCII strings and linear otherwise;
  string cursors (Stage 1 step 12) are the way to walk text. String
  literals cannot be changed, and changing a string that is a key of an
  `equal?` hash table loses its entry.
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
- `features` is `r7rs exact-closed exact-complex ratios complex
  full-unicode`, the operating system and architecture, `techne`,
  `srfi-27` and `srfi-69`.
- `exit` and `emergency-exit` ask the host to end the program: the request
  unwinds past every handler (running `dynamic-wind` exits) to the host,
  which decides what ending means; `techne-vm` exits with the status. They
  need the host-control capability, file procedures and `include` the
  files and loading capabilities, and `get-environment-variable(s)` the
  environment capability; a world without them sees them unbound or
  refused.
