# techne-vm: new runtime core (proposal A prototype)

Location: `crates/techne-vm` in this repository (developed first as
`experiments/techne-vm` in the Steel fork at `../techne-steel/readiness-integrated`).
Steel's parser is vendored in `crates/steel-parser` and used without lowering;
everything below it is new.
Steel's macro expander was not reused: it is entangled with steel-core (~4,700
lines across its AST visitors, module system and an engine-based kernel) and not
fully hygienic.

## Design

- **Values** (`value.rs`): one `u64`, NaN-boxed. Floats unboxed; 48-bit fixnums,
  heap pointers, chars, symbols, constants and native procedures in the negative
  quiet-NaN space. No destructor. All-zero bits are the float `0.0`, so a
  zero-filled register is a safe non-pointer for the GC.
- **Heap** (`heap.rs`): generational copying GC. 8 MiB bump nursery, en-masse
  promotion into a chunked old space (Cheney), full copying collection when the old
  space doubles. Remembered-set write barrier. Objects are a header word plus
  fields; strings/bigints are unscanned. Large objects go straight to the old space.
  `TECHNE_GC_STRESS=1` collects on every allocation, `=full` makes them full GCs.
- **Compiler** (`compiler.rs`): desugaring and scope resolution, then register
  allocation in one pass. Only variables that are both captured and assigned are
  boxed. Named `let` with tail-only self calls becomes a loop. Builtins that are
  never redefined compile inline (`+ - * < = car cdr cons vector-ref ...`).
  Comparisons fuse with branches (including compare-with-immediate), and global
  calls fuse into `CallG`/`TailCallG`.
- **VM** (`vm.rs`): register machine with arguments passed in place (no copying
  on call), proper tail calls, fixnum and float fast paths inline, closures holding
  their code pointer. Natives get a window of the rooted register stack.
- **JIT** (`jit.rs`, Cranelift): mixed mode. A function is compiled whole
  once it has been called or has looped 1,000 times. Its entry points (pc 0,
  loop heads, the instruction after each call) become `EnterJit`
  instructions, where the interpreter enters native code.
  - Native code runs straight-line instructions, branches, loops, closure
    creation, calls to Rust natives and calls to other compiled functions.
    Calls go on the native stack without interpreter frames. Tail calls go
    through a trampoline, and a tail call to the same code is a jump.
  - Anything exceptional hands over to the interpreter by unwinding the
    native stack: errors, task preemption, async waits, a reallocated
    register stack, more than 1,000 nested native calls, a callee that is not
    compiled or takes a rest argument, and handler installation. Each native
    frame records its interpreter frame on the way out, and the interpreter
    continues from the innermost one. So unwinding, `guard`, tasks and error
    traces have one implementation.
  - Scheme registers are kept in machine registers. They are written back
    before calls and slow paths, which may run the moving GC, and re-read
    afterwards.
  - Inline: fixnum/float arithmetic, comparisons, pair/vector access,
    globals, boxes, and nursery allocation for `cons` and boxes. Other cases
    run one instruction through `Vm::jit_slow_op`, so semantics cannot drift
    from the interpreter.
  - `TECHNE_JIT=0` disables it, `TECHNE_JIT=n` sets the threshold (`1` for
    testing), `TECHNE_JIT_LOG=1` reports compiled functions; `vm.set_jit` from
    Rust.
- **Macros** (`expand.rs`): `syntax-rules` with nested/middle ellipses, literals,
  `_`, vector and improper patterns, custom ellipsis, `(... ...)`. Hygiene by
  Clinger–Rees renaming: template identifiers become aliases that are fresh when
  bound by the expansion and otherwise resolve in the macro's definition
  environment (lexical depth + module). `define-syntax`, `let-syntax`,
  `letrec-syntax`, internal `define-syntax`; macros expanding to definitions work in
  bodies and at top level. The prelude defines `do`, `case-lambda`, `let-values`,
  `define-values`, `receive`, `parameterize`, `delay`/`force`, `assert` this way.
- **Modules**: one per file. `(require "path.scm")` loads once and imports its
  `provide`d names (all definitions without `provide`), variables and macros alike.
  Lookup order: own definitions, imports, root (builtins + prelude). Top-level
  definitions are predeclared so they shadow imports for the whole file.
- **Conditions**: `raise`, `raise-continuable`, `error` (error objects with message
  and irritants), `guard`, `with-exception-handler`, `dynamic-wind`, escape-only
  `call/cc`. Every runtime error (type, arity, unbound, Rust-native errors) is a
  catchable error object. Uncaught errors print `file:line:col` per frame.
- **Arguments**: optional (`[x default]`) and keyword (`#:k x`, `#:k [x default]`)
  parameters, defaults may use earlier parameters; unknown/missing keywords are
  errors naming the function. Keywords are their own immediate type.
- **`match`**: literals, quote, `_`, variables, list/dotted/vector patterns,
  trailing `...`, `?`, `and`, `or`, `not`, `cons`, `list`, `vector`, record
  patterns by type name (`(point x y)`), `#:when` guards; compiles to inline tests
  (no closures). `match-lambda`, `match-let`.
- **Generic functions**: `define-generic` / `define-method` dispatch on the first
  argument's type: record types → `record` → `t`, `integer`/`float` → `number`,
  `pair`/`null` → `list`, named foreign Rust types → `foreign`. Generics are
  applicable records (any record type can name a procedure field and be called).
  `applicable?`, `find-method`, `type-of`. Intended as the base for Embark-style
  actions (`examples/actions.scm`).
- **Restarts**: `restart-case`, `invoke-restart`, `compute-restarts`,
  `find-restart`, `handler-bind`. Handler procedures run at the raise point, at
  any native nesting depth, so restarts are still available. The REPL offers the
  active restarts on an uncaught error (`1 42` picks restart 1 with argument 42).
- **Tasks**: `spawn`, `task-join`, `yield`, `sleep`, channels (`make-channel`,
  `channel-send`, `channel-recv`), `run-tasks`. Each task has its own register,
  frame and handler stacks (switching is a swap); preemption after 10,000 calls or
  loop back-edges, compiled only into the task instantiation of the dispatch loop
  (non-task code pays nothing; a tight float loop runs ~25% slower inside a task).
  Code outside tasks waits by running the scheduler. Deadlocks are reported.
  `dynamic-wind`, escapes, handlers and `call-with-values` are VM operations, so
  tasks can suspend inside them; parameters (including `current-output-port`
  and the restart list) are task-local and inherited by spawned tasks. Only a
  Rust native calling back into Scheme with `vm.call` cannot be suspended
  across; that is reported as an error.
- **Also**: `define-record-type`, quasiquote, multiple values, `apply` (in the VM
  call path, so tail calls stay proper), `eval`, string/file ports and
  `with-output-to-string`, hash tables with deletion, merge `sort`, SRFI-1-style list
  library.
- **Tooling**:
  - REPL (`techne-vm` without arguments): on a terminal, line editing,
    history in `~/.techne_history`, completion of global names and multi-line
    input. Piped input works line by line.
  - Docstrings: a string before the body of a `define`/`lambda`.
    `(help name)` prints the signature, source location and docstring, or
    describes a macro, special form or built-in. `(documentation f)` returns
    the docstring.
  - Reader errors report `file:line:col`.
  - `techne-lsp` (crate `crates/techne-lsp`, stdio). It provides diagnostics
    (reader errors, unbound identifiers, missing `require`d files),
    go-to-definition across `require`, hover (signature and docstring, or the
    built-in description), completion and document symbols. Analysis is
    syntactic; it knows the core binding forms and prelude binding macros and
    never runs user code. Built-in names come from a VM instance.
- **Rust embedding** (`api.rs`):
  - `vm.register_fn("name", |a: i64, s: String| -> R)` with `FromValue`/`IntoValue`
    for numbers, strings, chars, bools, `Vec<T>` (lists or vectors), `Option<T>`
    (`#f`), `Result<T, E: Display>` (raises), `Root`, `Foreign<T>`;
    `register_fn_vm` for closures that need the VM.
  - `Root`: a handle that keeps a value alive and updated across moving GCs.
  - `Foreign<T>`: a Rust value owned by a Scheme object, dropped when the GC frees
    it (`Foreign<RefCell<T>>` for mutable state).
  - `vm.call(f, args)` / `call_global` re-enter the VM (also from inside natives),
    Scheme errors come back as `Err` with the raised object as a `Root`.
  - `vm.register_async(name, arity, |vm, args| future)`: Rust futures (woken from
    any thread) suspend only the calling task; `vm.spawn`, `vm.run_tasks`,
    `vm.task_result` drive tasks from Rust.
  - `vm.name_foreign_type::<T>("buffer")` makes Rust types dispatchable.
  - Low-level natives remain `fn(&mut Vm, args, n) -> Result<Value>`; those that
    allocate several objects reserve one block (`Bulk`) so only one GC can happen.

## Results (ms, wall time incl. startup; host load ~60, CPU 4 pinned)

Medians from one session (`IMPLS='^(chez|guile|lua5.4|techne|techne-interp)$'
./run.sh`), with techne-vm's interpreter (`TECHNE_JIT=0`) and its default
mode (JIT):

| program | Chez | Guile | Lua 5.4 | techne interp | **techne JIT** |
|---|---|---|---|---|---|
| startup | 47 | 10 | 1 | 5 | **3** |
| fib | 57 | 40 | 87 | 96 | **53** |
| tak | 59 | 49 | 102 | 149 | **81** |
| nqueens | 71 | 103 | 247 | 183 | **81** |
| bintrees | 110 | 303 | 1795 | 525 | **264** |
| hof | 89 | 122 | 116 | 191 | **125**¹ |
| qsort | 85 | 83 | 121 | 218 | **93** |
| mandel | 144 | 767 | 104 | 138 | **54** |
| hash | 380 | 235 | 106 | 93 | **93** |
| orgparse | 77 | 95 | 71 | 87 | **78** |

¹ After inlining fixnum `quotient`/`remainder`/`modulo` (142 before); the
other rows did not change measurably.

An earlier run of the interpreter alone, against Emacs native-comp (speed 3)
and stock Steel:

| program | Emacs native 3 | techne interp | Steel stock |
|---|---|---|---|
| startup | 56 | 2 | 213 |
| fib | 120 | 101 | 406 |
| tak | 53 | 152 | 471 |
| nqueens | 198 | 192 | 916 |
| bintrees | 370 | 562 | 1894 |
| hof | 197 | 193 | 1161 |
| qsort | 151 | 216 | 1139 |
| mandel | 537 | 145 | 971 |
| hash | 164 | 111 | >300 s |
| orgparse | 102 | 85 | 493 |

GC on bintrees: 42 minor collections, 28 ms total, 4.4 ms max pause.

## Verification

`cargo test --release`: about 70 end-to-end programs (core, macros and hygiene,
records, values, conditions, library, modules, error locations) and Rust API
tests (typed functions, roots across GCs, foreign finalisation, calls in both
directions). Programs run normally, with a minor GC on every allocation, and
with a full GC on every allocation. All benchmark outputs match Chez, and the
timings above are unchanged after these additions (startup 2.5 ms).
`examples/org-summary.scm` is a small realistic program (records, macros, hash
tables, string library, sort, guard): 46 ms on the 48k-line Org sample.

## Not done yet

- Language: full re-entrant continuations (only escapes now), bignums beyond i64,
  rationals, string interpolation, procedural macros (`syntax-case`), module
  renaming (`prefix-in`/`only-in`), multiple dispatch, method inline caches
  for generic dispatch.
- Tooling: formatter; the language server does not expand user macros, so
  identifiers bound by user-defined binding macros show as unbound.
- Runtime: a startup image once the prelude grows (startup is 4 ms now).
- Speed: remaining gaps to Chez are bintrees (2.4×, allocation and GC), tak
  (1.4×, call overhead) and hof (1.4×, GC and calls through closures). Next:
  cheaper call sequences (only spill registers that changed), native entry
  for rest-argument functions (`map`, `for-each`), inline caches and type
  feedback.
