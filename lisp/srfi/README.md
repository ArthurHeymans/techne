# Shipped Scheme libraries

These libraries load on import, without Nix or upstream checkouts at
runtime. They do not add bindings to programs that do not import them.

| Import | Provides |
|---|---|
| `(srfi 1)` | List constructors, folds, searches, partitions and list sets |
| `(srfi 27)` | Random sources |
| `(srfi 69)` | Hash tables (built into the VM) |
| `(srfi 113)` | Sets and bags, using SRFI 128 comparators |
| `(srfi 115)` | Regular expressions |
| `(srfi 117)` | List queues |
| `(srfi 128)` | Comparators and compatible hash functions |
| `(srfi 130)` | String cursors and string operations |
| `(srfi 132)` | Sorting, merging, selection and duplicate removal |
| `(srfi 133)` | Vector folds, searches, partitioning and mutation |
| `(srfi 151)` | Bitwise operations on exact integers |

For example:

```scheme
(import (scheme base) (scheme write) (srfi 1) (srfi 132))
(write (list-sort < (filter odd? (iota 10))))
(newline)
;; (1 3 5 7 9)
```

The libraries live beside this file. The VM searches here after the
importing file's directory and `TECHNE_LIBRARY_PATH`; see
[the library rules](../../runtime/R7RS.md). The current installation still
needs this source-tree library directory; bundling the libraries does
not make the binary relocatable.

## Upstream sources

The following sources are vendored at the revisions pinned in
`flake.lock`. Their Scheme implementation files are unchanged; library
declarations adapt names and relative include paths. Updating a source
must retain its notices and rerun its upstream tests against the shipped
copy.

| Files | Source | Revision |
|---|---|---|
| `1.sld`, `1/`, `117.sld`, `117/` | [chibi-scheme](https://github.com/ashinn/chibi-scheme) | `c4e7367e867428889d8fe898a0b39f42e418b3f1` |
| `113.sld`, `113/` | [SRFI 113](https://github.com/scheme-requests-for-implementation/srfi-113), `sets/` | `b15bfd15d0ea365cfca972691484168ac386fb8e` |
| `128.sld`, `128/` | [SRFI 128](https://github.com/scheme-requests-for-implementation/srfi-128), `comparators/` | `3ec333638e787d75a16de83fcf9645c998e4d976` |
| `132.sld`, `132/` | [SRFI 132](https://github.com/scheme-requests-for-implementation/srfi-132), `sorting/` | `3e94af40ac389bb9dfd020e3f47728a470d61ec3` |
| `133.sld`, `133/` | [SRFI 133](https://github.com/scheme-requests-for-implementation/srfi-133), `vectors/` | `12923135eeebedcff84972a4b5140b0f97306160` |
| `151.sld`, `151/` | [SRFI 151](https://github.com/scheme-requests-for-implementation/srfi-151), `srfi-151/` | `6eb63d8d9da64c5bade24eaa2f91a2c0da2f5dc6` |

SRFI 133 also re-exports the vector procedures supplied by `(scheme base)`.
SRFI 128 uses the upstream `comparators/` implementation, whose default
hash traverses pairs and vectors rather than relying on an R6RS hash
primitive. SRFI 151 uses its portable arithmetic implementation.

The chibi sources carry their BSD license in `1/LICENSE` and
`117/LICENSE`. SRFI 113 and 128 carry MIT licenses in their `LICENSE`
files; SRFI 133 is public-domain code with the MIT fallback in
`133/LICENSE`. SRFI 132's MIT license is in `132/LICENSE`, with additional
permission notices retained in its source files.
SRFI 151's MIT, SLIB and public-domain notices are retained in `151/`.
These notices apply to the imported code, rather than Techne's own
MIT-or-Apache-2.0 dual license.

The conformance harness uses pinned upstream *tests*, not upstream
library search paths, for these shipped libraries. SRFI 158 remains
external test coverage only: its coroutine generator requires re-entered
continuations, which Techne deliberately does not support.
