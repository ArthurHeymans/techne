# techne-vm: Techne's runtime

Location: `crates/techne-vm` in this repository (developed first as
`experiments/techne-vm` in the Steel fork at `../techne-steel/readiness-integrated`).
It began below Steel's parser; its reader (`reader.rs`, R7RS syntax) is now
its own too, and Steel's parser is gone. How it stands to R7RS is in
[R7RS.md](R7RS.md).
Steel's macro expander was not reused: it is entangled with steel-core (~4,700
lines across its AST visitors, module system and an engine-based kernel) and not
fully hygienic.

## Design

- **Values** (`value.rs`): one `u64`, NaN-boxed. Floats unboxed; 48-bit fixnums,
  heap pointers, chars, symbols, constants and native procedures in the negative
  quiet-NaN space. No destructor. All-zero bits are the float `0.0`, so a
  zero-filled register is a safe non-pointer for the GC.
- **Heap** (`heap.rs`): generational, with the structure of OCaml's heap.
  - Nursery: 8 MiB, bump-allocated (also inline in JIT code), copied out
    en masse by each minor collection.
  - Old generation: non-moving. Small objects live in 256 KiB blocks of one
    size class each (exact up to 16 words, then about 12.5% apart), allocated
    from a per-class free list or bump-allocated in a fresh block; large
    objects are allocated individually.
  - Collection cycles start when the old generation doubles (at least 64
    MiB). Marking is incremental: a slice after each minor collection, of
    128K words plus one word per word promoted, and one remark at the end
    that rescans the roots. Sweeping is lazy (a size class sweeps a block or
    two when it needs room) and runs in slices of 64 blocks. Objects promoted
    or allocated during marking are marked.
  - One write barrier for both jobs: storing a nursery pointer into an old
    object remembers that object; while marking, storing an old pointer
    shades it (incremental update, Dijkstra-style). The JIT calls it for
    pointer stores.
  - No compaction: free memory is reused by size class but not returned to
    the OS (large objects are).
  - Objects are a header word plus fields; strings/bigints are unscanned.
  - Identity hashes are addresses. Old objects never move; a nursery object
    whose hash was taken is flagged, and its promotion copies the old
    address into a word after its fields, so the hash survives the move.
  - Ephemerons: weak hash tables keep their key/value slots in an
    `Ephemerons` object, whose value is traced only once its key is
    reachable some other way. A minor collection repeats evacuating values
    of surviving keys until nothing more moves; the remark of a cycle
    repeats marking them until nothing more is marked. Then entries with a
    dead key are cleared, so a value referring to its own key does not keep
    it. That remark is not bounded: each round scans every weak table
    marked in the cycle, and a chain of entries whose values reach the next
    key needs a round per link, so heavy use of long weak chains lengthens
    the pause.
  - `TECHNE_GC_STRESS=1` collects on every allocation with a cycle always in
    progress and 64-word slices; `=full` completes a whole cycle on every
    allocation. `TECHNE_GC_STATS=1` prints counts, times, the longest pause,
    minor collection and slice, and the most words marked in one pause.
  - Libraries considered: MMTk (the Rust GC toolkit) has one low-pause plan,
    ConcurrentImmix, without a young generation, and expects a process-wide
    heap whose mutator threads all stop together; gc-arena is a safe-Rust,
    non-moving collector that cannot host this NaN-boxed raw heap.
- **Compiler** (`compiler.rs`): desugaring and scope resolution, then register
  allocation in one pass. Only variables that are both captured and assigned are
  boxed. Named `let` with tail-only self calls becomes a loop. Builtins that are
  never redefined compile inline (`+ - * < = car cdr cons vector-ref ...`).
  Comparisons fuse with branches (including compare-with-immediate), and global
  calls fuse into `CallG`/`TailCallG`.
- **VM** (`vm.rs`): register machine with arguments passed in place (no copying
  on call), proper tail calls, fixnum and float fast paths inline, closures holding
  their code pointer. Natives get a window of the rooted register stack.
- **JIT** (`jit.rs`, Cranelift): mixed mode. A function is queued for
  compilation once it has been called or has looped 1,000 times. A
  background thread compiles it from a self-contained job, and the VM
  installs the result when it next counts calls. Its entry points (pc 0,
  loop heads, the instruction after each call) become `EnterJit`
  instructions, where the interpreter enters native code.
  - Native code runs straight-line instructions, branches, loops, closure
    creation, calls to Rust natives, and calls to other compiled functions,
    including rest-argument ones. Calls go on the native stack without
    interpreter frames.
  - Tail calls go through a trampoline. A tail call to the same code is a
    jump, and a tail call to a Rust native returns its value directly.
  - Call sites through a global are specialised for the closure the global
    held when the function was queued. One comparison of its code pointer
    replaces the arity and frame checks, and recursion is a direct call.
    Calls through globals holding natives go straight to a Rust helper. A
    redefinition only makes the guard fail.
  - Anything exceptional hands over to the interpreter by unwinding the
    native stack: errors, task preemption, async waits, a reallocated
    register stack, more than 1,000 nested native calls, a callee that is not
    compiled, handler installation, and rare cases (overflow, bignums, a full
    nursery), which leave with `STEP` and run that one instruction in Rust.
    Each native frame records its interpreter frame on the way out, and the
    interpreter continues from the innermost one. So unwinding, `guard`,
    tasks and error traces have one implementation, and `Vm::jit_slow_op`
    is the only second implementation of instruction semantics.
  - Scheme registers live in machine registers. A liveness and dirtiness
    analysis decides which ones are written back before calls and exits and
    re-read afterwards. Registers a `guard` landing reads count as live
    everywhere.
  - Inline: fixnum and float arithmetic (including mixed), comparisons,
    `quotient`/`remainder`/`modulo`, pair and vector access, globals,
    boxes, and nursery allocation for `cons` and boxes.
  - Environment: `TECHNE_JIT=0` disables it, `TECHNE_JIT=n` sets the
    threshold (`1` compiles everything synchronously, for testing),
    `TECHNE_JIT_SYNC=1` waits for each compilation, `TECHNE_JIT_LOG=1`
    reports compiled functions and compile times, `TECHNE_JIT_IR=1` reports
    IR sizes (`=name` prints that function's IR). From Rust: `vm.set_jit`.
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
  Modules are named `user`, `root`, or by their file's path. The REPL, nREPL
  (`ns`), `node-eval` (`#:module`) and `(eval datum module)` evaluate in a
  chosen module, loading a file's module on first use; `(in-module name)`
  switches a REPL or session from its next evaluation, and completion, `help`
  and lookup follow. Libraries' Lisp code (process, node, nREPL) lives in root.
- **Worlds**: a VM is a world, granted capabilities: files, environment,
  processes, network, loading modules from files, host control (`exit`).
  `Vm::with_grants` builds a restricted one; natives registered inside
  `vm.requiring(capability, ..)` (built-ins, `techne-process`, `techne-node`)
  only raise "NAME: not granted in this world" there, so no name, import,
  `eval` or procedure value reaches them. VMs share no values, so a world's
  grants are all it has. `Vm::new` grants everything; the nREPL server and
  nodes grant all but host control, so `exit` cannot end them. Printing still
  goes to the host's output port unless the host gives the world its own.
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
- **Tasks**: `spawn`, `task-join`, `yield`, `sleep`, `run-tasks` and bounded
  channels: `(make-channel)` is a rendezvous, `(make-channel n #:bytes b)`
  buffers up to `n` messages and `b` bytes of strings; senders wait when it is
  full. `channel-close` fails later and waiting sends, receivers then get eof.
  `(select (recv ch (v) ...) (send ch x ...) (timeout ms ...))` does exactly
  one ready operation; offers of a waiting select are withdrawn when it wins
  elsewhere, is cancelled or interrupted. Each task has its own register,
  frame and handler stacks (switching is a swap); preemption after 10,000 calls or
  loop back-edges, compiled only into the task instantiation of the dispatch loop
  (non-task code pays nothing; a tight float loop runs ~25% slower inside a task).
  Code outside tasks waits by running the scheduler. Deadlocks are reported.
  `dynamic-wind`, escapes, handlers and `call-with-values` are VM operations, so
  tasks can suspend inside them; parameters (including `current-output-port`
  and the restart list) are task-local and inherited by spawned tasks. Only a
  Rust native calling back into Scheme with `vm.call` cannot be suspended
  across; that is reported as an error.
  - `task-cancel` / `vm.cancel_task` raise "task cancelled" where the task is
    suspended and drop what it waits on (including a Rust future), so its
    `dynamic-wind` and `guard` cleanup runs; a task may catch it. A task that
    dies of any error runs its `dynamic-wind` cleanups.
  - A host with its own event loop calls `vm.run_tasks_for(budget)`, which
    never blocks: it returns `Finished`, `Blocked` or `OutOfTime`.
    `vm.next_timer()` and `vm.set_wake_notifier(f)` (called from any thread
    when a future is ready) say when to call it again.
- **Interrupts**: `vm.interrupt_handle().interrupt()`, from any thread, raises
  the catchable condition "interrupted" in the running evaluation. The
  interpreter checks every 256 calls or loop iterations of a function, piggy-
  backing on the JIT's counters. Native code outside tasks returns every
  65,536 calls or back-edges to check. A waiting VM is woken. Measured latency
  is 0.2-0.5 ms. Not interruptible: a long-running Rust native. The
  `techne-vm` binary maps Ctrl-C to an interrupt; a second Ctrl-C before
  delivery exits.
- **Stack overflow**: recursion past 16M registers (128 MiB) raises a
  catchable "stack overflow" error.
- **Also**: `define-record-type`, quasiquote, multiple values, `apply` (in the VM
  call path, so tail calls stay proper), `eval`, string/file ports and
  `with-output-to-string`, packages (below), owned scopes (`make-scope`, `with-scope`,
  `scope-own!`, `scope-shutdown!`, `scope-transfer!`, `scope-procedure`:
  tasks, channels, processes and registry entries belong to the current
  scope and go when it is shut; a `scope-procedure` runs in the scope it was
  made in, so work it starts belongs there whoever calls it;
  `make-registry` and `registry-add!` for owned named entries such as
  commands: entries of one key added by different scopes stack, the newest
  in effect, and shutting a scope uncovers what its entries shadowed), hash tables (`eq?`, `eqv?` or `equal?`, any key;
  `make-weak-hash-table` with ephemeron entries), merge `sort`, SRFI-1-style
  list library.
- **Packages** (`load-package`, `unload-package`, `find-package`): a file,
  and the files it requires from its directory, loaded as a generation into
  fresh modules and a scope of its own. Loading again stages the next
  generation, holding back its registry entries; a failure shuts it and
  leaves the current one as it was, a success publishes its modules and
  entries at once, each in its predecessor's place among stacked entries,
  and shuts the previous generation's scope. Closures keep
  the generation they were made in, so records of a new generation are new
  types and macros are re-expanded by recompiling the package; other
  packages see new definitions through registries. Code records its
  generation. Primitives are sealed: `set!` of one from a module is an
  error, and a module shadows one by defining it.
- **Tooling**:
  - REPL (`techne-vm` without arguments): on a terminal, line editing,
    history in `~/.techne_history`, completion of global names and multi-line
    input. Piped input works line by line.
  - Docstrings: a string before the body of a `define`/`lambda`.
    `(help name)` prints the signature, source location and docstring, or
    describes a macro, special form or built-in. `(documentation f)` returns
    the docstring.
  - Reader errors report `file:line:col`.
  - Language server (`techne-lsp`), for editors other than Techne's own,
    which asks the running VM as an nREPL client does: it reads files with
    the VM's reader (`read_syntax`, spans on every datum) and analyses them
    without running anything; what the runtime defines comes from a VM
    with the runtime's libraries (processes, nodes, the editor) installed.
    It follows `require`, `import` (with import sets) and `include`.
  - nREPL: `techne-node --nrepl PORT` (localhost; writes `.nrepl-port`).
    Standard operations work with generic clients (checked with Rail):
    sessions, `eval` with output streamed while it runs, `load-file`,
    `interrupt`, `completions`, `lookup`/`info`/`eldoc` from docstrings,
    parameter lists and definition sites (the client's file and line are
    kept). Extensions: a restarts debugger (an `eval` with `techne-debug`
    pauses where an error with restarts was raised; the client picks a
    restart with arguments or aborts, and can evaluate and inspect
    meanwhile) and an inspector (labelled parts of lists, vectors,
    records, hash tables, closures with their captured values). All
    sessions share one VM. Tests: `crates/techne-node/tests/nrepl.rs`.
  - Emacs: `editors/emacs/techne.el`, no dependencies. `techne-jack-in`,
    `techne-mode` (evaluate, load, interrupt, inspect, describe; completion,
    eldoc, xref), a REPL buffer, the debugger and inspector buffers. ERT
    tests in `techne-test.el` run against a real server.
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
./run.sh`, results/20261005-2022; the hash row was re-run as -2025 after a
disturbance hit Lua and techne in that block). techne-vm's interpreter
(`TECHNE_JIT=0`) and its default mode (JIT). The JIT's compiler thread
shares the single pinned CPU, so compile time is included.

| program | Chez | Guile | Lua 5.4 | techne interp | **techne JIT** |
|---|---|---|---|---|---|
| startup | 49 | 10 | 2 | 4 | **3** |
| fib | 57 | 42 | 88 | 101 | **40** |
| tak | 60 | 50 | 103 | 143 | **50** |
| nqueens | 72 | 103 | 247 | 192 | **70** |
| bintrees | 112 | 304 | 1789 | 569 | **186** |
| hof | 88 | 122 | 117 | 229 | **117** |
| qsort | 88 | 82 | 122 | 239 | **85** |
| mandel | 144 | 763 | 104 | 156 | **55** |
| hash | 377 | 232 | 106 | 95 | **91** |
| orgparse | 78 | 94 | 71 | 109 | **69** |

A larger program: the Org library (`lisp/org`) parsing, writing and
querying a 12k-heading file in one run (`runtime/bench/org-lib.scm`, medians of
9): interpreter 88 ms; JIT 108 ms on one CPU (compilation competes with the
program), 81 ms when the compiler thread has its own CPU. In a second round
in the same process (everything hot compiled), parsing takes 38 ms against
the interpreter's 44-49.

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

GC on bintrees: 42 minor collections, about 14 ms of 190 ms, 2.8 ms max pause.

Pauses with a large old generation (`TECHNE_GC_STATS`, a list of 2-word
vectors kept live while 30M short-lived pairs are allocated):

| live | before (copying old space) | incremental: longest pause | of which old-generation slice | words marked in one pause |
|---|---|---|---|---|
| ~100 MB | 48 ms | 10-13 ms | 3-4 ms | 1.18M |
| ~400 MB | 181 ms | 10-13 ms | 3-4 ms | 1.18M |

The longest pause is now a minor collection in which the whole nursery
survives plus its slice; neither depends on the heap size. Peak memory for
384 MB live: 435 MB.

The nursery window bounds the minor collection: `vm.set_nursery_window(bytes)`
(capacity: `TECHNE_NURSERY_KB`). The trade-off, same session:

| window | worst pause (all survives) | bintrees | hof | Org library |
|---|---|---|---|---|
| 8 MiB (default) | 9-14 ms | 204 ms | 131 ms | 119 ms |
| 2 MiB | 2.5-4.6 ms | 227 ms | 153 ms | 118 ms |

Batch use keeps the default; an interactive host with frame deadlines sets
2 MiB. Adapting the window to the survival rate was tried and rejected: it
cannot bound the first collection after a phase change, and it cost hof 15%.
The nursery size (`TECHNE_NURSERY_KB`, default 8 MiB) barely matters: 4-16
MiB are within noise, 1 MiB is 30% slower.

## Verification

`cargo test --release`: about 70 end-to-end programs (core, macros and hygiene,
records, values, conditions, library, modules, error locations) and Rust API
tests (typed functions, roots across GCs, foreign finalisation, calls in both
directions). Programs run normally, with a minor GC on every allocation, and
with a full GC on every allocation. All benchmark outputs match Chez, and the
timings above are unchanged after these additions (startup 2.5 ms).
`examples/org-summary.scm` is a small realistic program (records, macros, hash
tables, string library, sort, guard): 46 ms on the 48k-line Org sample.

`tests/fuzz.rs` checks the JIT against the interpreter on random programs:
arithmetic at the fixnum boundary, lists, vectors, strings, loops, closures
with `set!`, `guard`/`raise`, bounded recursion, redefined globals and tasks.
Each runs under the JIT compiling synchronously, in the background, and under
GC stress, and must print exactly what the interpreter prints. 40 seeds run with
`cargo test`; a 20,000-program run (`TECHNE_FUZZ_START`, `TECHNE_FUZZ_SEEDS`)
found no difference. The fuzzer found seven of seven deliberately introduced JIT
bugs, mostly within ten programs. CI runs 5,000 more seeds every night.

`tests/suites.rs` runs Scheme suites in four modes that must agree (default,
interpreter, JIT compiling everything, GC on every allocation): our own
(`tests/suites/lang`), chibi-scheme's R7RS suite by section, and the 58
r7rs-benchmarks programs once each, checked by their own result predicates.
`tests/suites/expected-failures.txt` is the conformance record, each failure
under the deviation that explains it ([R7RS.md](R7RS.md)). First results were
447 failing R7RS tests, three sections that did not finish and 20 of 58
benchmarks failing; after the bugs they found were fixed (symbol and string
printing, `(_ . args)` patterns, `(... ...)` escapes, circular `list?` and
`equal?`, continuations as procedures, exactness of mixed comparisons) and the
missing procedures and libraries added, 129 tests remain: complex numbers,
rationals, string changes of UTF-8 size, re-entered continuations and
bytevectors. Two benchmarks failed on complex numbers and one on
bytevectors. Bytevectors (step 11), rationals, strings that change
size and complex numbers have since arrived; re-entered continuations
remain, by choice.

`runtime/bench/icount.sh` counts the instructions each benchmark executes
(cachegrind; JIT compiling synchronously, and interpreter) and the work of the
longest GC pause. Repeat runs agree to about 0.001%, so CI compares them
between commits regardless of machine load.

## Runtime gate status

Against the runtime contracts of [PLAN.md](../PLAN.md) Stage 0 and
[REQUIREMENTS.md](../REQUIREMENTS.md):

| Contract | Status |
|---|---|
| Async embedding: Rust futures suspend only their task; host-driven scheduling with time budgets, timers and wake notification | Done, tested |
| Interrupting a stuck evaluation | Done (0.2-0.5 ms); not inside long-running Rust natives |
| Cancellation with cleanup | Done; cooperative (a task may catch it) |
| Efficient values | NaN boxing, 48-bit fixnums, heap bignums beyond (num-bigint for arithmetic past `i64`), heap ratios and complex numbers (off the fast paths) |
| JIT with correct interpreter fallback | Done; differentially fuzzed |
| Low-pause GC | Done: incremental mark-sweep old generation; worst-case pause independent of heap size (was 181 ms at 400 MB) and set by the nursery window: 9-14 ms at the default 8 MiB, 2.5-4.6 ms at 2 MiB. |
| Rust interop, live inspection and redefinition | Done for the language (`help`, redefinition, typed Rust functions, roots, foreign values); application-level registration ownership is Stage 1 work |
| Two-process Lisp invocation/inspection probe | Done: `crates/techne-node`. `techne-node` serves a framed MessagePack protocol (length-prefixed, as emacs-tramp-rpc) on stdio, locally or as `ssh host techne-node`. `node-eval` evaluates on the node: data values cross in written form, other values become remote values (handles the node keeps until the client's object is collected, or its connection ends), printed output is relayed, errors arrive as conditions, `node-interrupt` stops it. `node-apply` calls a remote value with data or remote values; `node-describe` inspects a definition or a remote value. |
| Process contract and persistence | Done for local and remote children through one API: `crates/techne-process` runs children with pipes or a pty (resizable), with separate stderr, EOF, process-group signals, bounded buffering against slow readers (the child blocks), output as bytes (`process-read-bytes`) or as text with UTF-8 joined across reads, read errors distinct from end of file, and cleanup when a task is cancelled or a body fails (`call-with-process`). `process-spawn ... #:node n` runs the child on a node. Transport loss fails pending and later operations with "node connection lost". A plain node kills its processes when its client goes. A session (`techne-node --session NAME`, a per-user daemon on a Unix socket) keeps its Lisp state and `#:persist` processes across connections; their unread output is kept up to 1 MiB per stream (older output dropped and counted, the child never blocks). After reconnecting, `node-processes` lists what runs (status, dropped bytes) and `node-process` reattaches. Checked by tests and once over real ssh, killing the ssh client mid-session. |
| Thread ownership | One VM per thread; values do not cross threads (`Vm` is not `Send`) |

## Not done yet

The language foundations (code reclamation, limits, data notation; modules
in tools, worlds, bounded channels, identity tables, owned scopes and
packages are done) are planned step by step in [PLAN.md](../PLAN.md)
Stage 1, workstream A.

- Language: string interpolation, procedural macros (explicit renaming,
  PLAN.md step 15), multiple dispatch, method inline caches for generic
  dispatch. Re-entrant continuations are deliberately absent
  ([R7RS.md](R7RS.md)).
- Tooling: formatter. The language server does not expand macros it does
  not know: inside their uses it resolves identifiers but does not report
  unbound ones, since the macro may bind them.
- Runtime: a startup image once the prelude grows (startup is 4 ms now).
- Nodes: remote values are opaque handles (no remote objects inside data);
  a session daemon keeps output of persistent processes but not a terminal
  screen (a client re-renders from the replayed bytes); session state lives
  in the daemon's memory only.
- Speed: remaining gaps to Chez are bintrees (1.7×: calls and allocation;
  GC is only 7%) and hof (1.3×: calls through closures, which are not
  specialised). Compilation costs about 0.1-0.2 ms per bytecode
  instruction, so short programs on a single CPU lose some of the gain.
  Candidates: cheaper code for unspecialised calls, a lighter first tier,
  survivor aging in the nursery, inline caches and type feedback. Paused until
  a Techne workload measures a need.
