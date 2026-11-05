# Self-recursive numeric fast-call experiment

The reference is the bounded JIT in `INVESTIGATION.md`. Its six-argument C
entry point, VM register traffic and per-call frame setup dominate fib/tak;
merely retaining more SSA values increased spills. This experiment keeps
that implementation as the fallback, not as an ABI silently reinterpreted.

## Admission and ABI

Only small (at most 32 bytecodes / 64 registers), fixed-arity (1–3 arguments),
self-recursive integer functions qualify. The accepted instructions are
integer constants, moves, arithmetic, fused comparisons, branches and calls
to guarded globals holding this same code. No heap operations, handlers,
unknown calls, native calls or effects. Every live register must be proven
a fixnum under a checked fixnum-argument hypothesis. Self-call results can
be assumed fixnums only because every normal kernel return is proven one;
overflow or a changed procedure leaves the kernel, not a fabricated return.

A companion native function takes `(ctx, bp, numeric arguments..., remaining)`
and returns `(shifted fixnum result, status)`. There are no more than six
arguments, even for tak. Only the ordinary pc-0 loader can enter it, after
checking all arguments. Normal kernel status is zero; a high-bit marker
keeps every cold status distinct, including EXIT at pc 0. The entry strips
it before returning the original dispatch status. OSR, non-numeric inputs
and unsupported functions retain the original JIT and interpreter. The kernel uses one numeric SSA
form and does not carry both tags and unboxed values across recursive calls.

## Rooting and exact exits

The kernel cannot collect, grow registers or invoke arbitrary code. Calls
check the expected closure, available virtual-register capacity, native depth
and the same preemption fuel as the original JIT. Tail self-calls are jumps.
The outer entry also requires its actual closure and every expected self
callee to be the same object. This preserves the closure slot across tail
jumps, including weak-key observations of otherwise unused environments;
code identity alone is insufficient here. Distinct instances use the old path.
Capacity is bounded using the largest call-frame advance; it is not reserved
for an unconditional thousand frames in small worlds. A kernel chain has at
most 128 native activations (numeric live ranges can enlarge its frames),
and consumes the original native-depth budget including its initial call.

The outer entry retains normal frame initialization/publication. It records
its frame base in the native-run context. On an innermost exceptional exit:

1. Initialize the active kernel register span to safe zero values, retaining
   the original outer closure below that span. No collection occurs here.
2. Store the innermost live numeric registers in their tagged representation,
   publish the root window, and record the exact original pc/code/frame.
3. As native callers unwind, store only caller registers below the child's
   callee slot; never overwrite the materialized child's registers. Store
   the guarded closure in that slot and push the ordinary suspended frame.
4. Resume the existing interpreter dispatcher. It executes a slow arithmetic
   instruction once, or makes the deferred call/polls preemption as usual.

Each admitted function has the same frame size at every recursive level,
so the innermost materialized extent covers the ancestors. Liveness admission
also requires suspended caller values to lie below the call base, excluding
its not-yet-produced result. There is no replay of completed subtrees, no GC
with uninitialized frames, and no collecting "fallback helper" inside the
kernel. The ctx and register buffer stay valid throughout its native run.

## Compilation and lifetime budget

This is a bounded companion, not a general second optimizing tier. Admit
both functions' working/code memory, including separately rounded mappings.
Recheck the optional working set against current owner accounting; on
admission refusal try the ordinary compilation without the kernel.
Keep the pair in one isolated arena so
partial compilation failure can discard it whole. Charge both code mappings,
retain the same code dependencies, and reclaim both together. Reuse the same
Cranelift working contexts sequentially; do not create a new backend per call.

`tests/fast_calls.rs` compares values, errors and source frames against the
interpreter, including deep recursion, preemption, overflow in arguments and
results, non-fixnum arguments, redefined globals, and heap roots in an ordinary
caller during a collecting slow exit. Benchmark only with counters disabled.
The experiment is opt-in with `TECHNE_JIT_FAST=1` until measured and validated.

Rust is necessary for this mechanism: it changes the native call convention
and must preserve root publication, code lifetime and exact deoptimization
whatever Scheme does. Workloads and measurement policy remain Scheme.
