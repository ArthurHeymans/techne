# Fast startup at scale: lazy packages and a compiled-code cache

Status: design, not started. Written to keep the package and module design
compatible with it; build it when the editor's Lisp grows enough to need it.

## The problem

Every start reads and compiles all the Lisp it loads, from source. Measured in
instructions (`runtime/bench/icount.sh`, October 2026):

| What starts                          | Lines loaded | Instructions |
|--------------------------------------|-------------:|-------------:|
| The VM (prelude)                     |        1,000 |        14 M |
| The editor on an empty document      |        6,400 |       118 M |

Running code is under 1% of that: it is reading and compiling. At about 18 K
instructions per line, a configuration the size of a well-grown Emacs (700 K
lines) would cost some 13 billion instructions, several seconds, at every
start. REQUIREMENTS.md asks for a runtime start under 100 ms warm.

Inlining makes compiling dearer: calls passing lambdas to `map` and the like
expand at compile time (`Compiler::inline_call`), about 15% of the editor's
start. Other implementations make that affordable by not compiling at every
start, which is the second half of this design.

## What others do

- **Emacs** loads little of its Lisp at startup: *autoloads* define stubs for
  a package's entry points (commands, modes), and the first call loads it;
  `with-eval-after-load` defers configuration until then. What it does load is
  compiled once, to `.elc` bytecode or `.eln` native code, keyed by the
  source's hash.
- **Chez Scheme, Guile and Racket** compile modules ahead of time (`.so`,
  `.go`, `compiled/*.zo`) and inline at that time, with budgets (Chez's `cp0`,
  after Waddell and Dybvig's *Fast and Effective Procedure Inlining*).
  Loading compiled code is a small fraction of compiling it.
- **LuaJIT and tiered JITs** compile nothing eagerly: they profile in the
  interpreter and optimize, inlining included, only what runs hot.

Lazy loading and a compiled-code cache are complementary: the first means most
code is never loaded at startup, the second makes what is loaded cheap.

## Part 1: lazy packages (autoloads)

Most of a large configuration is features used now and then. A package should
be able to say what it offers without loading:

- **Its entry points**: commands, procedures, modes, the files or buffers that
  turn its modes on, its keys. Each is a stub, registered when the package is
  *declared*; calling one (or the editor dispatching to it) loads the package,
  publishes its generation, and repeats the call against the real definition.
- **Their documentation**, so help, completion and which-key show a declared
  package's commands as if it were loaded.
- **Configuration to run after it loads** (`with-eval-after-load`): a hook on
  the package's first publish, rerun on each new generation.

How a package declares this is the main open choice:

1. *Markers in the source* (Emacs's `;;;###autoload`), collected by a scan
   into a declaration file. Familiar, keeps the declaration next to the code;
   needs a scan step, and its output can go stale.
2. *A manifest* the package ships (`package.scm`: name, entry points, after-load
   hooks). Explicit and cheap to read; duplicates names that the code defines.
3. *Derived from the compiled-code cache* (part 2): the cache knows each
   module's definitions, docstrings and commands, so declaring a package is
   reading its cached interface. No duplication, but a package needs one load
   (or a compile) before it can be declared lazily.

Option 3 with option 2 as an override fits techne best: the common case needs
nothing from package authors.

It builds on the existing package machinery (`Vm::stage_package`,
`publish_staged`, generations): an autoload stub belongs to the package's
registry, and publishing a generation replaces the stubs. A load that fails
leaves the stub, which reports the error, and the previous generation if any.

Things to settle:

- What the stub of a non-command procedure does when called inside a task or
  while another package loads (`check_loading` refuses nested loads today).
- Macros cannot be stubbed: code using a package's macros loads it when it is
  compiled, or the macros are part of the declared interface.
- Keys and modes are registrations; their stubs must be indistinguishable from
  the real registrations to whatever inspects them (the microscope, help).

## Part 2: a compiled-code cache

Cache, per source file, what compiling it produced, and load that instead of
reading and compiling the file again.

### What is cached

A file's module compiles to a sequence of top-level forms, each a `Code`
(`Compiler::compile_toplevel`) that is then run. The cache holds:

- The `Code` objects: instructions, constants, captures, spans, names,
  parameter names, docstrings and definition positions (`Code::definition`).
- The module's macros (`syntax-rules`: patterns and templates, as data) and
  its exports, imports and requirements.
- The source's hash, for `find-definition` and error locations to stay
  right without rereading unless shown.

Loading still *runs* the top-level code; it skips reading and compiling. JIT
state is not cached: it is per process and recompiled when hot.

### Relocation

Compiled code refers to the running VM:

| Reference                   | Stored as                         | On load                          |
|-----------------------------|-----------------------------------|----------------------------------|
| Symbols, keywords           | names                             | interned                         |
| Global slots                | (module, name)                    | looked up or created             |
| Code indices (`Closure`)    | index within the file             | offset by where they are added   |
| Constants                   | data notation                     | rebuilt (immutable, old space)   |
| Native procedures           | name                              | looked up; absent fails the load |
| Inline guards' closures     | (module, name, definition hash)   | see below                        |

An inlined call's guard compares a global with the closure it held when
compiling (`Expr::Object`). The cache records which definition that was, by
the hash of its source form. On load, if the global's current closure comes
from that same definition the guard gets it; otherwise the guard gets a value
nothing equals, and the call is made as written: correct, only slower, until
the file is recompiled.

### Validity

A cache entry is valid for the same:

- source bytes (hash);
- compiler and VM version (a constant bumped with the bytecode format);
- interfaces of the modules it requires, their exports and macros (hash),
  since macros and inlining read them at compile time;
- prelude (hash), for the same reason.

Anything else invalidates it, and the file compiles from source as now, and
writes a new entry. Entries live under `$XDG_CACHE_HOME/techne/`, named by the
source path's hash, written atomically. The prelude's can be produced at build
time.

### Testing

- The language suites and fuzzers run twice, cold and warm, with identical
  results.
- A deliberately stale entry (changed dependency interface, changed VM
  version) is never used.
- `editor startup` in the benchmarks, warm and cold.

## Part 3: inlining, afterwards

With compiled code cached, eager inlining costs nothing at startup, and its
budget can grow: more procedures (`sort`, `vector-map`), deeper nesting, the
prelude's own calls (not inlined today, because the prelude is compiled at
every start). Tiered inlining, recompiling a function with inlining when the
JIT finds it hot, is then only worth it if profiles show hot code that eager
inlining missed.

## Order

1. Lazy packages: an API decision more than compiler work, and the larger
   saving; packages are easier to write for it from the start than to convert.
2. The compiled-code cache.
3. Inlining budgets and the prelude's own calls.

Measure each against `editor startup` and against a synthetic large
configuration (generated packages totalling hundreds of thousands of lines).
