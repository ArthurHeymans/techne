# Fixing the optimizer's GC-work regression

Follow-up to the [cached-world measurements](CACHED.md). The source optimizer
still defaults off; this fixes the JIT root-retention bug rather than disabling
`bindings` or `dead`, changing collection thresholds, or moving collections.

## Cause

The callback workload's interpreter copied at most 327,538 words with either
configuration. Only JIT execution regressed to 371,278 with `bindings,dead`.
The bytecode difference removed moves and reduced `round`'s frame from 17
registers to 16; it did not introduce extra list allocations.

Register-root tracing found a dead reversed filter accumulator still in a
native frame's memory. Its 14,580 pairs account for **43,740 words**, exactly
the observed increase. The useful forward list shared its elements, but not
that obsolete spine.

The JIT keeps values in SSA and spills dirty **live** registers before code
that can collect. A dead SSA write need not be emitted, but memory may still
hold the register's *previous* heap pointer. GC scans the register window,
not SSA liveness, so that obsolete object remains rooted. Register layout
changes from the source optimizer exposed this pre-existing JIT problem.

## Fix

- Before a handoff that exposes registers to GC or the interpreter, zero
  **dirty, dead** slots. Clean slots still agree with bytecode execution;
  there is no need to clear every unused register repeatedly.
- Apply this to ordinary spills, guard exits, native calls and inlined-frame
  materialization. Clearing only an inlined callee misses stale caller slots.
- Known fixnums deferred in SSA across a native call also get non-pointer
  memory placeholders. Otherwise their old heap pointers can remain rooted
  despite the current value being an integer. Unusual exits still store the
  actual tagged fixnums, as before.
- Keep live values and exception/escape landing registers in the existing
  liveness sets. No bytecode, scheduling checkpoint or allocation is removed.

This uses the existing liveness/dirtiness analysis; it adds no runtime shadow
stack, cache format, new opcode or optimizer-specific escape hatch.

## Tests and measurements

A focused JIT test passes a weak key as a procedure argument, overwrites it,
and collects. The obsolete pointer must disappear both when its replacement
is dead and when a live fixnum replaces it. A separate case keeps the actual
pair live across collection and checks its relocated contents. The test runs
with interpreter/first-call JIT and direct/inlined Scheme collector calls.

The callback campaign runs off, `bindings,dead`, the selected four passes and
all passes in interpreter and synchronous ordinary-threshold JIT modes,
three times each, before and after the fix. An independent Python calculation
checks the result, **5,362,247,396**, on every run. Raw counters, binary/source
hashes and the instruction campaign are in
[`results/gc-retention.json`](results/gc-retention.json).

| JIT callback GC work | Before | Fixed |
|---|---:|---:|
| maximum copied words, passes off | 327,538 | 327,535 |
| maximum copied words, `bindings,dead` / selected / all | 371,278 | 327,535 |
| total promoted words, passes off | 921,007 | 862,852 |
| total promoted words, `bindings,dead` / selected / all | 958,147 | 862,852 |

All 48 runs return the independent oracle. There are still seven minor
collections and no full collections; interpreter counters are unchanged.
The optimizer no longer increases this pause-work bound at all. Compared
with the affected pre-fix JIT run, maximum copying falls **11.78%** and total
promotion falls **9.95%**.

Whole-process JIT instruction counts, measured with the existing Cachegrind
protocol (including compilation, not just warmed execution):

| Workload | Fix cost with passes off | Fix cost with all passes |
|---|---:|---:|
| callbacks | +0.69% | −0.79% |
| number/string conversion | +2.67% | +2.67% |
| higher-order functions | +1.03% | +1.03% |
| tasks | +2.67% | +2.50% |

These costs are recorded rather than described as a free optimization.
An initial clear-every-dead-slot prototype cost over 5% on tasks; restricting
clearing to dirty slots avoids redundant stores. Interpreter counts differ
by at most 0.03% in this campaign.

The final fix passes full VM/editor release library/integration tests with
all IR passes enabled, 100-seed full-diagnostic optimizer differential fuzzing,
the kernel oracles and snapshot-backed SRFI workload with first-call JIT and
GC stress/verification, and the new weak-key test under full-GC stress.
Release Clippy (VM library/examples/tests, warnings denied) and formatting pass.

The original [cached-world artifact](results/cached.json) remains unchanged;
it records the regression before this fix. GC work is an upper bound in
words, not a wall-time guarantee. The collections' own wall times are recorded
but are not used as a stable performance oracle.

## Reproduce

```sh
cargo build --release -p techne-vm --features counting-allocator \
  --bin techne-vm --example ir --example ir_cache
TECHNE_IR=all cargo test --release -p techne-vm \
  --features counting-allocator --lib dead_register_slots_do_not_keep_weak_keys_alive
TECHNE_IR=all TECHNE_GC_STRESS=full TECHNE_GC_VERIFY=1 \
  cargo test --release -p techne-vm --features counting-allocator \
  --lib dead_register_slots_do_not_keep_weak_keys_alive

# Existing benchmark protocol; repeat with TECHNE_IR=0 and a saved pre-fix VM.
PROGS='callbacks numstr hof tasks' TECHNE_IR=all \
  nix develop --command bash runtime/bench/icount.sh .local/gc-fixed.json
```
