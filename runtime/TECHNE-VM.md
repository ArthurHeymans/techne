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
  - Nursery: 256 KiB until the world's first collection, then 8 MiB
    (`TECHNE_NURSERY_KB`) if its memory limit has room; bump-allocated
    (also inline in JIT code), copied out en masse by each minor
    collection. A world that never collects holds little.
  - Old generation: non-moving. Small objects live in blocks of one size
    class each (32 KiB for a class's first, doubling up to 256 KiB; exact
    up to 16 words, then about 12.5% apart), allocated
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
  - No compaction: free memory is reused by size class; blocks a sweep
    finds empty, and large objects, are returned to the OS.
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
  - Code is collected too. Each `Code` (bytecode, constants, spans) has a
    handle on the heap holding its constants and the handles of the codes
    it makes closures of; the closures its machine code expects to call
    live while it does. A closure keeps its code's handle, and so do
    running frames, suspended tasks and compilations in progress. A code whose handle dies is freed with its
    pending JIT job (cancelled; a late result is dropped) and its function
    in the JIT's arenas, Cranelift modules of up to 64 functions of one
    package generation, freed when all their functions are. Source text
    goes with the last code compiled from it. Globals are roots except
    those of retired package generations (`Vm::retire_generation`): a
    retired module lives while live code uses one of its globals, a live
    module imports one, or an evaluation runs in it, which the remark
    settles like ephemerons; then its globals and slot are reused.
    `why-retained` shows the chain of references that keeps an object.
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
  Allocating operations publish the live frame, finish computing the result,
  then rederive register access before reading moved operands or storing it.
  GC root visitation can invalidate a cached Rust pointer even without moving
  the register buffer. Rust JIT slow paths therefore take frame indices, not
  register pointers; native calls and stack growth likewise end cached access.
  Compiled-call helpers carry frame indices across rest-list construction and
  tail-call transitions, deriving a fresh register pointer for each entry.
  `Vm::with_heap` supplies a scoped `HeapAccess` without a collecting or
  reentrant VM interface. Its invariant lifetime brands `HeapValue`s: they
  cannot escape the callback or mix with another access scope. Nursery-only
  `try_cons` and `try_string` return failure rather than collecting. UTF-8
  references additionally borrow the context, excluding mutable access until
  their last use; owned copies can outlive it. JIT slow pair/box initialization
  reloads operands and roots its result within a fresh scope after allocation.
  This is the first structural boundary: the interpreter and other legacy
  raw-value paths still require their explicit refresh discipline. Persistent
  host handles and complete embedding safety are separate work.
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
  - Call sites through a global are specialised for the procedure the
    global held when the function was queued. For a closure, one comparison
    of its code pointer replaces the arity and frame checks, and recursion
    is a direct call. For a Rust native, one comparison of the global's
    value; the arity is checked when compiling, and native code calls the
    native's function itself: `natives!` defines each with the C calling
    convention, returning its value and its error, if any, in two machine
    registers, and typed closures holding nothing (`procedures!`,
    `register_fn`) get such a function made for their type. One test of the error and of a counter of the moves of the
    register stack's and the globals' buffers (`Slots`, the only way they
    grow) tells the common case. Known fixnums are stored to memory only
    if native code is left after the call. A redefinition only makes the
    guard fail.
  - Small functions run inline at call sites specialised for their
    closure: at most 20 instructions, no loops, handlers, closures made or
    calls but of Rust natives (at most one without an inline case) and of
    small functions that run inline in turn (nested, up to three calls
    deep and 40 instructions a call in all, each guarded as the first),
    not the caller itself. The callee's instructions are compiled after the
    caller's, its registers those above the call's base register, where
    a call would put its frame; a return is a move to the base register.
    The guard precedes them, and the call still counts for preemption;
    if the global holds another procedure, the call is made (by a helper,
    `jit_call_slow`, which costs little to compile). That the frame fits
    the register stack is checked once, at the caller's entry points, for
    all of its calls inlined (a helper grows the stack, or if it cannot,
    the interpreter runs the caller from then on). Leaving native code
    inside the callee (an error, a rare case) before it changed anything
    outside its registers or entered a nested call lets the interpreter
    make the call instead, as if it had not been inlined: all such exits
    share one block, and no frame is made. Otherwise (after a store, any
    call, a write to an argument) the callee's frame is made, as before a native it
    calls: its live registers are stored (the others cleared), its
    closure put below them and the GC's root window raised over it, as a
    call's prologue would; leaving, the caller's frame is recorded below
    it, in one block per call inlined. The interpreter then continues in
    the callee from its original instructions, so errors, traces and
    waits are those of a call. A call returns a known fixnum if each of
    the callee's returns does; when the call is made instead, native code
    checks it. At a tail call (in a function without handlers) the callee
    takes over the caller's frame, as the call would: its registers are
    the caller's own, moves put the arguments in place, its returns are
    the caller's, and its frame, when made, replaces the caller's, which
    is not recorded. Leaving native code in a function nested in others
    makes the frames of each (but those a tail call replaced). Once a
    nested call entered, the outermost call cannot be replayed: entering
    counts fuel and may clear registers holding its original arguments.
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
  - Registers known to hold fixnums (constants, results of fixnum
    arithmetic on known fixnums, which steps out on overflow, and values
    of natives that return only fixnums, such as `string-length`, at call
    sites specialised for them) need no tag
    checks or float paths, and are kept shifted as arithmetic wants them
    (re-read and shifted anew after calls, rather than kept in both forms
    across them). Native code's own paths prove them; an entry point
    checks them, and if the interpreter left something else there (a
    bignum after an overflow) leaves that frame to the interpreter, which
    runs its original instructions until it calls or loops on. A loop
    counting fixnums runs about 40
    instructions an iteration instead of 80.
  - Registers known to hold floats likewise (`flonums`: float constants,
    and results of arithmetic with an operand known a float, which goes on
    only when the other operand is a float or a fixnum and steps out
    otherwise) are kept as `f64`s: their arithmetic and comparisons have no
    tag checks, and the tagged, NaN-canonical form is made only where it is
    used (an exit, a meeting of paths). mandel's inner loop went from about 245
    instructions an iteration to about 75.
  - At the loop head where the interpreter asks for the code, or at the
    start of the call that does, it notes which registers (arguments)
    hold a fixnum and which a float (`Job::observed`), and native
    code assumes those whose kind it uses (arithmetic, comparisons, vector
    indices: `kind_used`). Edges into the head that do not know them check
    them and leave for the interpreter if they do not hold; the head's
    loader checks them too, and leaves the frame to the interpreter if
    they do not, as do self tail calls. So loop counters that start from
    the function's parameters are known fixnums: qsort takes 22% fewer
    instructions, mandel 28%; with arguments assumed, nqueens 8% fewer.
  - A function that loops counts the calls and back-edges for preemption
    and stops in a machine register, writing it through to memory (which
    callees and the interpreter count in): the count does not wait for
    memory.
  - Inline: fixnum and float arithmetic (including mixed), comparisons,
    `quotient`/`remainder`/`modulo`, pair and vector access, globals,
    boxes, and nursery allocation for `cons` and boxes. At call sites
    specialised for them, the common case of a few natives:
    `string-length` and `string-ref` of ASCII strings, `vector-length`,
    `bytevector-u8-ref`, `char->integer`, `char=?` of two characters,
    `%record-ref` (record accessors) (anything else calls the native).
  - Environment: `TECHNE_JIT=0` disables it, `TECHNE_JIT=n` sets the
    threshold (`1` compiles everything synchronously, for testing),
    `TECHNE_JIT_SYNC=1` waits for each compilation, `TECHNE_JIT_INLINE=0`
    runs no function inline, `TECHNE_JIT_LOG=1`
    reports compiled functions and compile times (and entry points handed
    back to the interpreter), `TECHNE_JIT_IR=1` reports
    IR sizes (`=name` prints that function's IR), `TECHNE_JIT_ASM=name`
    prints that function's machine code. From Rust: `vm.set_jit`.
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
  and lookup follow. The current module is the task's own: spawned tasks
  inherit it, and `in-module` in one does not change another's. `eval` and
  `eval-source` compile a form at a time (`%compile-expression`,
  `%read-forms` and `%next-form`) and call it from the prelude, with the
  module evaluated in current meanwhile: what they evaluate can wait in a
  task, as code a native calls back cannot. The source text and its
  module stay while its forms are evaluated (`Forms`, which holds them),
  and are let go of once they are done, or, for an evaluation that
  failed or was killed, once the forms are collected. A module a
  top-level `require` of those forms needs, and a package's files
  (`load-package`), load the same way: before the form is compiled, the
  module is made and marked loading, and its forms are evaluated from the
  prelude (`%eval-forms`), so what they run can wait too; the form then
  finds it loaded. A load whose task was killed is over, its module as
  far as it got; another task's `require` meanwhile is told the module is
  being loaded, and finds published modules, not those of a package being
  staged. A `require` inside another form, `import` of R7RS
  libraries and `include` still load as they are compiled, from Rust.
  Libraries' Lisp code (process, node, nREPL) lives in root.
- **Worlds**: a VM is a world, granted capabilities: files, environment,
  processes, network, loading modules from files, host control (`exit`).
  `Vm::with_grants` builds a restricted one; natives registered inside
  `vm.requiring(capability, ..)` (built-ins, `techne-process`, `techne-node`)
  only raise "NAME: not granted in this world" there, so no name, import,
  `eval` or procedure value reaches them. VMs share no values, so a world's
  grants are all it has. `Vm::new` grants everything; the nREPL server and
  nodes grant all but host control, so `exit` cannot end them. Printing still
  goes to the host's output port unless the host gives the world its own.
- **Templates** (`src/vm/template.rs`): `Vm::new` and `Vm::with_grants`
  start a world by copying a template, booted once per thread and grants
  when a second world is asked for, not by installing the builtins and
  compiling the prelude again (about 0.2 ms and 0.4 MiB per world,
  booting 1.5 ms and 0.65 MiB). The copy shares only immutable parts
  (instructions, names, source text, macros, native implementations);
  heap objects are copied with their identity hashes, the standard ports
  made afresh, runtime state started empty. `TECHNE_TEMPLATE=0` boots
  every world, `TECHNE_TEMPLATE=1` copies even the first (for testing).
  A host makes a template of a world it loaded code into with
  `Vm::snapshot`, and starts worlds from it with `Snapshot::world`, as a
  serverless host starts each instance of a deployment
  ([doc/serverless.md](../doc/serverless.md)). The world must be at rest:
  no unfinished tasks, timers, channels, foreign objects but the standard
  ports, or values rooted from Rust; otherwise the snapshot is refused
  with an error saying what is left. Top-level parameter values are
  carried over. Natives are shared, not copied: one registered with a
  closure holding state is registered on each world instead. With the Org library loaded (581 lines,
  `examples/snapshot.rs`), a world starts from its snapshot in 0.43 ms
  and 430 KiB, where loading the library into a world takes 3.7 ms.
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
  any native nesting depth, so restarts are still available: the raising
  frame calls them (through the prelude's `%handle-raise`), with the handler
  entries from theirs up hidden (`Handler::Mask`), so they are frames of the
  raising task like any other, and in a task one can wait (for a debugger's
  choice, say) and then invoke a restart. Without the prelude, and for
  `Vm::raise_continuable` from Rust, they are called from Rust and cannot
  wait. A handler raising the condition it was called for again keeps the
  trace of the first raise. The REPL offers the
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
  The scheduler runs only the tasks ready, in the order they became ready:
  futures' wakers (from any thread), channel operations, a joined task
  finishing and expiring timers (a heap by deadline) put them in its run
  queue, so idle tasks cost nothing per round.
  `dynamic-wind`, escapes, `guard`, exception handlers and
  `call-with-values` are VM operations, so tasks can suspend inside them;
  parameters (including `current-output-port` and the restart list) are
  task-local and inherited by spawned tasks. A Rust native calling back
  into Scheme with `vm.call` (`dynamic-wind`'s after thunks when unwinding,
  `member` with an equivalence of its own) cannot be suspended across;
  that is reported as an error.
  - `task-cancel` / `vm.cancel_task` raise "task cancelled" where the task is
    suspended and drop what it waits on (including a Rust future), so its
    `dynamic-wind` and `guard` cleanup runs; a task may catch it. A task that
    dies of any error runs its `dynamic-wind` cleanups.
  - Tasks and channels live in slots of the VM, which their Lisp objects
    name with the slot's generation. A finished task's result goes into
    its object (rooted by a Rust `TaskId`), and its slot to the next task
    spawned; a channel closed and drained frees its slot, and its object
    then reads as closed and empty. So finished tasks and closed channels
    hold nothing but their objects, which the GC reclaims.
  - A host with its own event loop calls `vm.run_tasks_for(budget)`, which
    never blocks: it returns `Finished`, `Blocked` or `OutOfTime`.
    `vm.next_timer()` and `vm.set_wake_notifier(f)` (called from any thread
    when a future is ready) say when to call it again.
- **Stops** (`src/stop.rs`): every top-level evaluation and every task is an
  execution, with an id never reused (`Vm::new_execution`,
  `Vm::task_execution`). `vm.interrupt_handle().stop(id, Stop::Break)`, from
  any thread, raises the catchable condition "interrupted" in that execution
  and no other; `Stop::Kill` ends it past every handler, runs none of its
  `dynamic-wind` after thunks, restores the dynamic state of a top-level one,
  and gives a task joining a killed task the catchable "task killed"
  (`task-kill` from Lisp). A task spawned with `#:child` is a child of
  the execution spawning it, killed when that ends killed; other tasks go
  on. `interrupt()` breaks the outermost top-level
  evaluation running, as Ctrl-C at the REPL. A stop waits until its
  execution runs; one flag is set while the running execution has one. The
  interpreter checks every 256 calls or loop iterations of a function, piggy-
  backing on the JIT's counters. Native code outside tasks returns every
  65,536 calls or back-edges to check. A waiting VM is woken. Measured latency
  is 0.2-0.5 ms. Not interruptible: a long-running Rust native. The
  `techne-vm` binary maps Ctrl-C to an interrupt; a second Ctrl-C before
  delivery exits.
- **Stack overflow**: recursion past 16M registers (128 MiB) raises a
  catchable "stack overflow" error.
- **Memory limit** (`src/alloc.rs`, `src/vm/held.rs`): what a world holds
  (`Vm::held`) is kept under `Vm::memory_limit` (4 GiB,
  `TECHNE_MEMORY_MB`). GC backing buffers (nursery, old blocks and large
  objects), register/task stacks and the globals' value buffer use `Budget`,
  a standard-library allocator backed by `System`:
  their allocator carries ownership, without a trailer or thread-local
  allocation attribution. Its reservations charge the world's account
  once, and release it when freed. `Budget::new(limit)` also supplies
  independent, fallible reservations and allocation; collection itself
  still uses the VM's pressure policy, not a quota refusal halfway through
  moving objects. Register growth reserves fallibly after noncollecting
  VM admission; a failed reservation leaves the stack and JIT movement
  count unchanged and returns a catchable error. Task creation allocates
  its registers before publishing a task or consuming a free slot; Rust
  hosts can use `try_spawn` (`spawn` retains caller-managed admission).
  Task switches retain allocator ownership without constructing budgets,
  and completion releases the buffer. Initial VM construction and globals
  growth remain infallible; frame/handler stacks and other metadata are
  not yet migrated. Heap storage ownership is fixed at construction. The
  removal of public `Heap::charge_to` is an intentional host API break:
  standalone `Heap::new()` uses an independent budget, not a world account;
  embeddings needing world accounting should use `Vm`, which constructs its
  heap with the world's owner from the outset. There is no public API for
  assigning or transferring a heap's owner after allocation.
  Allocations not yet migrated use the global allocator:
  each has an 8-byte trailer naming its account, credited when freed on any
  thread. A thread counts those allocations in a local delta, added to the
  account when it changes accounts. The VM enters its account while it runs,
  collects or allocates in the old generation; the JIT thread while it compiles.
  Machine code is mapped apart and counted by the JIT. The cost is 1-3% of
  the instructions of a keystroke, 3.4% of the editor's startup before the
  explicit GC allocator migration. Growth of a
  known size is admitted first (`Vm::admit`): if it does not fit even
  after a full collection, it is a catchable "out of memory". Admitted:
  what natives can make far more of than their arguments hold (sized
  constructors, conversions such as a string's characters as a list,
  replacements, bignum arithmetic and powers, files, read a line at a
  time within the room left), `spawn`, `make-channel`, new symbols, and a
  register stack growing (without the collection, which cannot run in
  the middle of a call); requests under 64 KB pass unchecked. Printing
  stops at the room left; an error message shows the values it is about
  cut after 4 KB each, 16 KB in all. Reading, macro expansion (matching
  and templates), compilation, constants and the conversion of data for
  `eval` and `environment` are metered (`src/vm/meter.rs`): every 1024
  parts made, and after a long copy, a stop pending for the execution is
  delivered (so an expansion that goes on for ever is stopped), and what
  they made past the room left when they began is refused. Stack
  segments grown for deep recursion are counted for the world. A
  function is not compiled while there is no room for its machine code.
  The rest is the pressure check's:
  collections and large objects look at what the world holds, and past
  the limit the allocator raises a flag the VM looks at in its checks,
  when a task switches away and when an execution ends (without
  collecting there). Found over its limit, the world collects fully; each
  step of growth past that (a 64th of the limit, at least 8 MB) refuses the running execution with
  the catchable stop `Stop::OutOfMemory`, the third refusal, or one past
  twice the limit, a kill. Growth is refused, not whoever runs when the
  world is found over, and with no execution running nothing is
  (`Vm::pressure` reports it; the editor's echo area says so). Past three
  times the limit, which only a native allocating on reaches, the
  allocator ends the program.
- **Heap cage** (`src/alloc/cage.rs`): a thread's worlds share one cage of
  4 GiB (`TECHNE_CAGE_MB` lowers what heaps may use of it, to 64 MiB at least). A world's room
  is also the cage's free room, less a reserve kept for collections (a
  sixteenth, at least 64 MiB): a world growing into it is refused as one
  past its memory limit is, and one still growing once half of it is gone
  is killed, so the other worlds of the thread go on. A stop the VM makes
  for what runs ends the slice of compiled code running it.
- **Also**: `define-record-type`, quasiquote, multiple values, `apply` (in the VM
  call path, so tail calls stay proper), `eval`, string/file ports and
  `with-output-to-string`, packages (below), owned scopes (`make-scope`, `with-scope`,
  `scope-own!`, `scope-shutdown!`, `scope-transfer!`, `scope-procedure`:
  tasks, channels, processes, sockets and registry entries belong to the current
  scope and go when it is shut; the scopes are a table in Rust
  (`src/vm/scopes.rs`), which releases what has a native cleanup without
  running Lisp, the Lisp cleanups left running in a task of their own,
  stopped past `scope-cleanup-budget`; a `scope-procedure` runs in the scope it was
  made in, so work it starts belongs there whoever calls it;
  `make-registry` and `registry-add!` for owned named entries such as
  commands: entries of one key added by different scopes stack, the newest
  in effect, and shutting a scope uncovers what its entries shadowed), hash tables (SRFI 69: `eq?`, `eqv?` or `equal?`
  natively with any key, or any equivalence with its hash procedure, called
  as tables probe; `make-weak-hash-table` with ephemeron entries),
  persistent dicts and sets (`{key value ...}`, `#{obj ...}`: literals that
  read and write as such and take quasiquote; `dict-set`, `dict-delete`,
  `set-add`... return new ones sharing most of the old; keys compare with
  `equal?`, equal dicts are `equal?` and `hash` the same; CHAMP tries in
  the heap, `crates/techne-vm/src/dict.rs`), JSON (`string->json`,
  `json->string`: objects as dicts with string keys, arrays as vectors,
  `null` as the symbol; written with members in key order; reading
  metered and nesting bounded as `read`'s), merge `sort`, SRFI-1-style
  list library; SRFI 27 random numbers as the library `(srfi 27)` (`lisp/srfi/27.sld`);
  string cursors (SRFI 130); regular expressions (SRFI 115's SREs over
  compiled `regexp`s, which search strings and, in the editor, ropes in
  place, stopping at the next chunk on an interrupt).
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
  - Docstrings: a string before the body of a `define`/`lambda`, or after
    the literals of a `syntax-rules`, as in Guile. Records document their
    procedures as what they do; special forms have theirs in the compiler
    (`SPECIAL_FORM_DOCS`); natives in their Rust definitions.
    `(help name)` prints the signature, source location and docstring of
    any of these. `(documentation f)` returns a procedure's docstring,
    `(binding-description 'name [module])` all of it as an alist.
    The convention is Guile's, which Emacs Lisp shares (`doc.rs`): the
    first line a complete sentence of at most 72 characters; procedures in
    the imperative ("Return", not "Returns"), naming every parameter in
    upper case; lines of at most 80 characters, continuation lines in the
    first column; keys as `\\[command]`, which help shows as the user's
    key; other names and code in backquotes. `(docstring-problems doc
    subject params)` says what a docstring breaks of it.
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
    pauses where an error was raised, with its restarts and a `retry`
    of its own; the client picks a restart with arguments or aborts, and
    can evaluate and inspect meanwhile) and an inspector (labelled parts
    of lists, vectors, records, hash tables, closures with their captured
    values). All sessions share one VM; each evaluation runs in a task of
    its own, so one waiting (in the debugger, say) holds up no other.
    Tests: `crates/techne-node/tests/nrepl.rs`.
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
    (`#f`), `Result<T, E: Display>` (raises), `Value`, `Foreign<T>`;
    `register_fn_vm` for closures that need the VM. Arguments convert from
    an `Arg`, a view that nothing collects or changes while it is looked
    at (borrowed `&str` and bytes included); `vm.inspect(&v, |arg| ...)`
    gives the same view of a held value.
  - `api::Value`: what the host holds. An immediate is held as it is (a
    symbol by its name); a heap object or a native through a root of its
    world, kept alive and updated across moving collections while the
    `Value` lives. It is `Clone`, not `Copy`, and not `Send`. Every method a value goes into checks its world: a
    value of another world, or of a dropped one, is an error (and a native
    returning one fails), never a look into another heap. An error a native
    hands on from another world keeps its message, not its objects. The
    VM's own raw value type is private.
  - What the safe API cannot do: reach the heap's memory or its
    collector (`Vm::heap` gives statistics only), make a value from an
    address, or replace a world's heap, registers, modules or tables. The
    VM's internals (values, compiler, JIT, codes, ports, numbers, printing)
    are private; `Arg::kind`, `Arg::fields` and `Arg::dict_entries`
    (through `vm.inspect`) let a host, an inspector say, look into objects
    and hold only the parts it keeps.
  - `vm.make_string` accepts UTF-8 `&str`. Raw pointer/bit constructors and
    unchecked record allocation are private to the VM implementation.
  - `Foreign<T>`: a Rust value owned by a Scheme object, dropped when the GC frees
    it (`Foreign<RefCell<T>>` for mutable state).
  - `vm.call(&f, args)` / `call_global` re-enter the VM (also from inside natives),
    Scheme errors come back as `Err` with the raised object (`Error::payload`).
  - `vm.register_async(name, arity, |vm, args| future)`: Rust futures (woken from
    any thread) suspend only the calling task; `vm.spawn`, `vm.run_tasks`,
    `vm.task_result` drive tasks from Rust (a `TaskId` is checked as a value is).
  - `vm.name_foreign_type::<T>("buffer")` makes Rust types dispatchable.
  - The VM's own natives are `fn(&mut Vm, args, n) -> Result<Value>`
    (`natives!`); those that allocate several objects reserve one block
    (`Bulk`) so only one GC can happen. Hosts define typed procedures
    (`procedures!`, `register_fn`).

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

`tests/host_api.rs` uses the host API at random across three worlds of a
thread, under GC stress for some: values made, held across collections,
handed to their own and other worlds (as arguments, globals, natives'
results), their worlds dropped and others made in their place. A value of
a live world means what it meant; one of another or a dropped world is
refused. It runs 100 seeds; nightly CI runs 2,000 more
(`TECHNE_HOST_API_START`, `TECHNE_HOST_API_SEEDS`).

`tests/miri.rs` runs small programs under Miri, which checks the unsafe
code they reach for undefined behaviour: a VM without the prelude (which
would take hours), the interpreter only (Miri cannot run machine code),
collections at every allocation. Run it in the pinned nightly dev shell
with `cargo miri test -p techne-vm --test miri` and
`MIRIFLAGS="-Zmiri-permissive-provenance
-Zmiri-tree-borrows"`, as configured in nightly CI. Values, forwarding
headers and free lists hold addresses as integers, which become pointers
again through the thread's cage, with its provenance. It takes half an hour; CI runs it
every night. `cargo miri test -p techne-vm --test counting` separately
checks fallback allocator metadata under default Stacked Borrows, without
relaxation flags. The allocator exposes both its full System allocation
and the caller's owning pointer before recovering access to its private
trailer; it no longer dereferences a payload-only tag past its extent. The library tests `vm::tests` exercise the JIT's Rust slow paths
without running machine code, and `api::tests` exercise typed native entries.
Run each with `cargo miri test -p techne-vm --lib <filter>` and the
same flags; nightly CI runs both as well.

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
longest GC pause, and per request the instructions of the HTTP benchmark's
server (`runtime/bench/http`) on in-memory connections
(`crates/techne-vm/examples/serve.rs`). Repeat runs agree to about 0.01%, so
CI compares them between commits regardless of machine load.

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
| Networking | Done for TCP: `crates/techne-net` gives Lisp listeners (`tcp-listen`, with `#:reuse-port`) and connections (`tcp-accept`, `tcp-connect`, `tcp-read` as text with UTF-8 joined across reads, `tcp-read-bytes`, `tcp-write`, `tcp-close-output`, `tcp-close`, `call-with-socket`). Every wait suspends only its task and is dropped when the task is cancelled or killed; nothing is buffered: a write sends its string or bytevector from the heap, without a copy, and waits until the kernel has taken it all, so writers wait for slow readers, and a read admits what it may allocate first; text reads never hold back a byte that starts no character; end of stream is the eof object, a reset or refused connection an error. The current scope owns each socket, `tcp-close` wakes tasks waiting on it, and the procedures need the network capability. Sockets live on a shared I/O thread by default, or on a runtime the host drives, one per VM thread, as the HTTP benchmark's server does (`runtime/bench/http`). No TLS, UDP or HTTP library yet. |
| Thread ownership | One VM per thread; values do not cross threads (`Vm` is not `Send`) |

## Not done yet

The language foundations (limits, data notation; modules in tools, worlds,
bounded channels, identity tables, owned scopes, packages and code
reclamation are done) are planned step by step in [PLAN.md](../PLAN.md)
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
