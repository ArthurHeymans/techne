# Follow-up: calls, slow exits and retained memory

Investigation after [HILLCLIMB.md](HILLCLIMB.md), against its final binary
(`3674d820…`). The earlier runtime improvements are the baseline here.

## Findings

### 1. Fib/tak are not repeatedly deoptimizing

Added opt-in, bounded `TECHNE_JIT_STATS=1` diagnostics at the interpreter/JIT
boundary. No counters are emitted into generated code. A fixed 64-site table
records names/locations, statuses and STEP operand classes; it keeps neither
guest values nor code alive. Overflow of the detail table is reported, while
total status counters continue. Storage is charged to the world. Snapshots start
with fresh diagnostics, not their template's counts.

Three calls per program, synchronous compilation at threshold 1000:

| Program | Preemption exits (`TICK`) | Slow-op exits (`STEP`) | Generic call helper |
|---|---:|---:|---:|
| fib | 2,114 | 0 | 0 |
| tak | 2,717 | 0 | 0 |
| hof | 2,600 | 34 | 0 |
| qsort | 3,282 | 0 | 2 |

HOF's 34 slow instructions were nursery allocation of `cons` (27 in `my-map`,
7 in `my-filter`), not arithmetic. Qsort's two generic calls occurred during
warm-up. These totals include warm-up; they are not hot-only counts.

The `RETURNED` counter counts a native run returning to the interpreter, **not**
every recursive native function call. `TAILCALL` is consumed by the trampoline.
For non-STEP exits the reported pc is the continuation/resume location, not a
claim that its displayed instruction failed. Inlined exits are attributed to
the original callee location after native frames have been reconstructed.

This confirms a normal compiled-call target for fib/tak, rather than a need to
patch frequent arithmetic overflow or guard misses.

### 2. Keeping more numeric SSA values across calls backfires

A temporary prototype preserved known numeric caller locals across ordinary
compiled calls, instead of reloading them and recreating their shifted form.
It continued refreshing pointers and excluded the result register. The same
binary was compared with that change enabled/disabled, three processes each,
eight calls, medians of the last four:

| Program | Reload as before, ms | Preserve numbers, ms | Change |
|---|---:|---:|---:|
| fib | 155.87 | 157.97 | +1.3% |
| tak | 229.85 | 258.14 | **+12.3%** |
| qsort | 179.19 | 192.96 | **+7.7%** |
| hof | 333.03 | 331.97 | -0.3% |

Tak's generated native stack frame grew from `0x90` to `0xa0` bytes. Static
`%rsp` references across its whole assembly grew from 300 to 348 (these are
not dynamically weighted spill counts). Longer live ranges increased pressure
under the current six-argument C ABI instead of saving work.

**Rejected and removed.** The old reload strategy remains. Its existing
comment about reloading being cheaper than carrying both forms was justified.
This is evidence against a naive register-retention change, not proof that
an unboxed internal call ABI would fail.

The next call experiment should reduce setup/context pressure or provide a
narrow fast-entry/return contract, rather than merely extending live ranges.
Cold exits must still reconstruct initialized, rooted frames before GC; a
fast-path no-GC claim cannot silently include a fallback native that collects.
No such new ABI or root-publication policy was implemented in that
investigation. The subsequent [numeric kernel](FAST-CALLS.md) implements the
narrow contract separately, retaining the original ordinary call strategy.

### 3. HOF is now predominantly an allocation/list problem

Callgrind on two calls with synchronous compilation recorded 2.262 billion
whole-process instructions. Inclusive function costs:

- minor GC: **37.2%**;
- `reverse`: **20.7%**;
- `num::add`: **9.9%**;
- `num::wide`: **6.4%**.

These percentages **overlap**: reverse and arithmetic can collect, and wide
arithmetic is inside add. They must not be summed as disjoint costs. The
collector alone accounted for 16.9% exclusively at its main `minor` symbol,
with more work attributed to its inlined routines and callees.

The accumulator does leave the 48-bit fixnum range. It does so inside the
direct Rust `+` native, so zero arithmetic STEP exits does **not** mean zero
boxing. Native internals need a profile in addition to boundary counters.
Changing the fold's initial accumulator to `0.0` as a diagnostic ablation did
not improve this workload, and changes exactness anyway; it is not retained.

Allocation reduction or a measured survivor policy is a more promising HOF
investigation than concentrating only on boxed addition. Opt-in Scheme
iterator/transducer pipelines can avoid intermediates; transparently fusing
arbitrary callbacks requires an effects proof and is not a free optimization.

### 4. The retained-memory test failure was cached compiler workspace

The late compilation of `load-package` expands 73 bytecodes into 3,467 IR
instructions (44 bytecodes inlined at seven calls). Cranelift's reusable
`Context` and `FunctionBuilderContext` kept its high-water working buffers.
They are not executable code, Scheme objects or leaked package generations.

Evidence:

1. Keeping all buffers reproduces the original test's late `other` plateau
   near 3.92 MB versus roughly 2 MB in early samples.
2. Resetting both contexts after every job makes the unchanged test pass.
3. With the mutator stopped for synchronous compilation, resetting after the
   loader alone released **3,092,452 bytes** of owner-accounted memory in one
   measurement, while executable arenas remained installed.
4. The retained default resets after jobs exceeding **2,048 pre-optimization
   IR instructions or 256 registers**. Ordinary small-function buffers remain
   reusable. Both reclamation tests, including the previously failing ordinary
   asynchronous one, now pass without skipping or loosening any assertion.
5. `TECHNE_JIT_CACHE=1` forces the old keep-all behavior and reproduces the
   failure; `=0` forces discard-all for comparison.

`TECHNE_JIT_MEMORY=1` reports owner bytes before compilation, after compilation,
after optional buffer reset, and total machine-code bytes. The owner total
includes intercepted allocations, not just explicit `Budget` reservations.
Use synchronous compilation for attribution: asynchronous deltas also include
mutator activity and other threads' pending accounting deltas. Cache policy
uses IR/register sizes, not those racy diagnostic deltas.

This stays in Rust because the workspace belongs to the native compiler thread;
Scheme cannot release Cranelift's internal allocations. The diagnostics likewise
observe native boundary/root state unavailable to guest code. Workload policy
and the reusable comparison runner remain Scheme.

## Retained change: runtime and compilation cost

The final version keeps diagnostics (off by default) and selective workspace
trimming, **not** the numeric-retention experiment. Three alternating process
pairs, eight calls per process:

| Program | Before, ms | Final, ms |
|---|---:|---:|
| fib | 156.13 | 156.00 |
| tak | 230.26 | 230.93 |
| qsort | 178.61 | 178.45 |
| hof | 332.39 | 333.82 |
| orgparse | 75.83 | 75.87 |

These are essentially flat: this investigation fixes retained memory, not the
remaining V8 execution gap. Timing comparisons do not enable diagnostic counters.
The summed native-job compile-time medians were 5.03 -> 5.13 ms for fib,
8.50 -> 8.69 for tak, 36.48 -> 37.01 for qsort, 19.05 -> 19.52 for HOF and
56.23 -> 57.47 for orgparse. These small 1-3% differences are timing samples,
not a claim of zero compilation cost or a compiler-speed improvement.

## Vector loops: a smaller next candidate

Qsort's warmed run also shows no slow-op exits. The generated inline `swap!`
paths still check both read indices and both write indices. A local proof could
reuse successful accesses before attempting a general induction/range solver.
But read and write bounds use different lengths: the write length is zero for
immutable vectors. Read success alone does **not** authorize a write. Any such
optimization must preserve vector identity, index definitions, mutability and
fallback/error ordering. This remains a candidate, not a measured gain.

## Reproduce

```sh
nix develop --command cargo build --release -p techne-vm \
  --features counting-allocator --bin techne-vm

# Existing compare.scm assembles steady workloads in runtime/bench/build/compare.
PROGS='fib tak hof qsort' RUNS=3 ITERS=8 techne run runtime/bench/compare.scm \
  /path/to/previous-binary target/release/techne-vm /tmp/follow-up.json

ITERS=3 TECHNE_JIT_SYNC=1 TECHNE_JIT_STATS=1 \
  target/release/techne-vm runtime/bench/build/compare/fib.scm

nix develop --command cargo test --release -p techne-vm \
  --features counting-allocator --test reclaim

# Attribution: add TECHNE_JIT_MEMORY=1 and TECHNE_JIT_SYNC=1.
# Control: TECHNE_JIT_CACHE=1 restores the known failing retention behavior.
```

## Subsequent numeric call kernel

The separately measured [FAST-CALLS.md](FAST-CALLS.md) follow-up now retains
a checked self-recursive integer ABI: fib 155.8 -> 100.5 ms, tak 231.4 ->
125.2 ms, with exact materialization instead of replay. This is not the
rejected keep-numbers experiment: it passes numeric arguments/results and
reduces context/setup pressure. Selected native compilations add about
3.3/5.7 ms and double their code mappings. Other functions retain the old
compiled entry/OSR behavior. The kernel is default-on, ablated with
`TECHNE_JIT_FAST=0`; the earlier tables above remain historical measurements.

New diagnostics tests check operand-class reporting, unchanged arithmetic,
bounded storage and UTF-8 truncation. The release run passed 116 library tests,
all 15 reclamation tests, and the selected language/memory/accounting/frame and
raw-access-ratchet suites. The final differential fuzz run passed 40 seeds in
five JIT modes with no skips. Both new diagnostics tests also passed with full
GC stress, heap verification and diagnostics enabled.
