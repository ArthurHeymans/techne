# Loop/callback hillclimb

Baseline is `17fb4311`, including the numeric recursive kernel.
Workloads and arguments stay unchanged. CPU 4, release/counting-allocator,
paired alternating processes, synchronous compilation at threshold 1000.

## 1. Publish loop fuel at boundaries, not every inlined call

Loops already count in an SSA register, but wrote through to memory after
**every** call/back-edge. A loop with an inlined Scheme callback often writes
twice per turn even though no other execution mechanism runs between them.

The bounded change retains every decrement, branch, poll location and resume
pc. It publishes before native/compiled calls and native returns/side exits,
then reloads after helpers that may count or shorten the slice. In particular:

- ordinary and inlined exits publish their exact SSA count;
- REDO publishes **before** refunding its call, never over the refund;
- native/compiled callees see current fuel, and their updated fuel survives;
- even storage/collection helpers can trigger allocation-pressure hurry;
- tail transitions and successful returns publish too;
- no extra guest roots, callbacks, recompilation or per-loop instrumentation.

This is a VM/native-boundary invariant, not Scheme policy: Scheme cannot
publish its own compiled counter or repair its native exit metadata.

### Reject the wider policy

Deferring for all loops saved instructions but sometimes lengthened native
live ranges enough to lose wall time: same-binary Mandelbrot +1.4%, nqueens
+2.4% in one three-pair run (an earlier Mandelbrot sample +3.5%). Do not keep
that policy merely because instruction counts improve. Restrict to loops
with actual bounded Scheme inlinees; pure loops retain the original path.

### Retained policy measurements

Three pairs / eight calls, medians of the last four, then across processes:

| Workload | Before ms | Boundary publication ms |
|---|---:|---:|
| direct control | 6.026 | 4.357 |
| indirect control | 8.561 | 7.741 |
| hof | 333.153 | 326.591 |
| qsort | 180.438 | 163.642 |
| mandel | 43.927 | 43.995 |
| nqueens | 255.045 | 255.622 |
| hash | 323.121 | 320.430 |
| orgparse | 74.391 | 73.594 |

One Cachegrind pair / two calls, **whole process**, including compilation:

| Workload | Before instructions | After instructions |
|---|---:|---:|
| direct | 94,045,214 | 86,312,660 |
| indirect | 124,421,388 | 116,823,225 |
| qsort | 1,304,550,124 | 1,252,941,093 |
| mandel | 288,653,445 | 288,641,181 |
| nqueens | 1,801,161,864 | 1,801,165,552 |

Initially opt-in with `TECHNE_JIT_FUEL=1`. The unit test in
`src/vm/fuel_tests.rs` observes the exact count from a native every iteration
(one inlined call plus one back-edge) and requests allocation-pressure hurry.
Both publication and respecting that notification are asserted, without
timing thresholds or process-global environment mutation. It is excluded
under Miri since it deliberately executes native code. The enabled prototype
passes 117 library tests, recursive-kernel fixtures and all stop/kill tests.

## 2. Cache invariant indirect guard results

A repeated indirect inline site can take its callee from a copy of an
immutable formal argument. Follow copies backwards only within a unique
straight-line predecessor chain; reject joins, non-copy definitions and
arguments defined anywhere in the body. Only cyclic, already monomorphic
inline sites qualify. Feedback remains bounded to four sites.

Compute a **boolean code-identity guard** at each native/OSR entry. False
uses the existing generic call path, not interpreter-only execution, and
must not raise for a non-procedure before the call is reached. Refresh after
self-tail jumps implicitly rebind parameters. Code identity is immutable
across collection; actual closures/captures are read from the current
registers, never memoized. There are no sampled-environment roots.

Initially opt-in `TECHNE_JIT_CALL_CACHE=1`; `TECHNE_JIT_LOG=1` reports selected
guards. With fuel publication retained, a five-pair / ten-call same-binary
control gives indirect **7.720 -> 6.031 ms** (-21.9%), HOF 333.706 -> 331.160
ms, qsort 160.209 -> 160.502 ms. Do not claim the latter two small differences
as reliable new gains. Whole-process counts (two calls) fall 116,824,190 ->
95,700,613 for indirect, 2,213,628,530 -> 2,155,900,791 for HOF.

`tests/call_cache.rs` asserts concrete expected values as well as matching
interpreter exit status/stdout/full source traces. It also verifies which
cases actually select/reject caching: distinct captures of one code, a new
target, unused/bad callbacks, self-tail rebinding, boxed assignment, CFG joins,
and a native collection inside an inlined callback with a captured pair.
Both ordinary and full-GC-stress variants pass.

## 3. Exact scalar addition through first-class `+`

`my-fold` still called the Rust native for every addition. Its accumulator
leaves the 48-bit fixnum range, but fits signed i64. Add a bounded two-argument
native inline case: decode fixnums or one-limb sign-magnitude integers, check
signed overflow, normalize back to a fixnum or allocate the original two-word
integer representation in the existing nursery. Mask payload addresses as well
as headers. Unsupported kinds, out-of-i64 operands, overflow and exhausted
nursery space use the existing collecting native path with original arguments.
No mutation of existing number objects or exact-to-float coercion.

Ordinary fixnum/float inputs reuse the existing floating arithmetic contract
rather than merely paying extra guards before calling Rust. Preserve the
native procedure's fold from zero: its signed-zero behavior differs from the
binary bytecode. A new differential test caught this; retain the initial
zero addition instead of weakening the expectation. Integer overflow never
falls into the floating path. Initially opt-in `TECHNE_JIT_WIDE_ADD=1`.

Independent i128-oracle tests exhaust 441 pairs around fixnum, i64 and larger
integer edges. Additional cases cover ratios, complex/inexact values,
infinities, NaNs, signed zero, immutable aliases, changed procedures, source
error traces and full-GC stress. The selection/ablation marker is checked.

New Scheme/JS `wide` and `flosum` controls make this mechanism measurable apart
from list allocation; they are not substitutes for the unchanged HOF workload.
Three same-binary pairs / eight calls with the earlier changes held enabled:

| Workload | Scalar case off ms | On ms |
|---|---:|---:|
| wide control | 72.029 | 21.739 |
| inexact sum control | 34.973 | 15.340 |
| hof | 321.683 | 301.293 |

The integer-only trial improved HOF but mildly regressed inexact sums; the
retained floating fallback avoids that regression. Its original HOF counts
fell 2,155,899,372 -> 1,922,429,247 (whole process, two calls); final combined
counts are measured separately.

This remains Rust because native number representation, allocation boundaries
and exact fallback/root publication belong to the VM mechanism. Workloads and
the comparison tool stay Scheme. The runner now labels each JSON record's
`side`, so same-binary ablations are not ambiguous from identical paths.

## Final default and combined validation

All three mechanisms are enabled by default. Developer ablations:
`TECHNE_JIT_FUEL=0`, `TECHNE_JIT_CALL_CACHE=0`, `TECHNE_JIT_WIDE_ADD=0`.
The last is read once per process; use separate processes for comparisons.
No inlining-limit growth, recompilation, nursery enlargement or workload
rewrite was retained.

A final review found one unrelated helper-fuel reload still emitted for
pure loops. That changed Mandelbrot's row function from 1,404 to 1,408 IR
instructions and lost about 5% in a cross-binary sample. Restricting that
reload to the deferred-counter case restores 1,404 instructions and the
old pure-loop behavior. Five process pairs then gave 41.735 -> 41.761 ms.
Do not keep unrelated cold-path IR changes on the theory that they are free.

Final full campaign: **three alternating pairs, eight calls**, medians of
the last four and then across processes. EPYC 3151 / CPU 4. Raw values:

| Workload | Before ms | Final ms |
|---|---:|---:|
| direct control | 6.029 | **4.353** |
| indirect control | 8.564 | **6.037** |
| fib | 100.668 | 100.720 |
| tak | 125.400 | 125.296 |
| nqueens | 254.642 | 259.612 |
| bintrees | 833.909 | 839.877 |
| hof | 338.471 | **302.526** |
| qsort | 177.955 | **160.017** |
| mandel | 43.681 | 41.791 |
| hash | 321.468 | 325.398 |
| orgparse | 75.223 | 75.442 |
| wide control | 72.666 | **25.479** |
| inexact sum control | 35.554 | **15.803** |

Retained gains: direct **27.8%**, indirect **29.5%**, HOF **10.6%**, qsort
**10.1%**. The new scalar controls improve 64.9% / 55.6%, not an application
speedup claim. Other workloads have no material instruction change. Nqueens
is about **2% slower by wall time** in this campaign; report it, not a gain.
Mandelbrot's process timings vary by several percent despite unchanged hot
work; the paired recheck above is flat, so credit no Mandelbrot improvement.

A separate same-binary all-off/default ablation corroborates the retained
changes, and reverses the sign of nqueens' small cross-binary difference:

| Workload | All off ms | Default ms |
|---|---:|---:|
| direct | 6.027 | 4.352 |
| indirect | 8.557 | 6.025 |
| hof | 332.747 | 302.484 |
| qsort | 179.545 | 164.105 |
| mandel | 41.601 | 41.808 |
| nqueens | 262.640 | 258.437 |

These small unaffected-control differences are not evidence of an
optimization benefit or a reproducible policy regression.

Final Cachegrind, one pair / two calls, whole process including compilation:

| Workload | Before instructions | Final instructions |
|---|---:|---:|
| direct | 94,040,966 | 86,314,524 |
| indirect | 124,419,036 | 95,728,501 |
| hof | 2,271,834,220 | 1,932,739,593 |
| qsort | 1,304,543,325 | 1,252,921,434 |
| mandel | 288,657,361 | 288,657,905 |
| nqueens | 1,801,158,787 | 1,801,158,555 |
| wide | 800,714,897 | 225,689,194 |
| inexact sum | 416,513,640 | 161,440,861 |

Native compilation sums (medians): direct 6.09 -> 6.57 ms, indirect
6.27 -> 7.08 ms, HOF 18.78 -> 22.52 ms, qsort 36.42 -> 37.42 ms. Existing
fib/tak compilation stays flat. These are extra milliseconds, not zero
compilation cost. HOF's fold grows 198 -> 406 pre-opt IR instructions; the
callbacks add guarded entry blocks, not a new general optimizer tier.

HOF peak RSS over three alternating eight-call pairs: **99,248 -> 100,512
KiB** (about +1.3%), not a memory reduction. Total child CPU median falls
2.71 -> 2.51 seconds. GC scheduling shifts: a representative sample has
135 -> 128 minor collections but 51.7 -> 53.9 million promoted words, so
fewer allocations do not prove every collector cost falls. The 8 MiB nursery
and all original memory/reclamation assertions remain unchanged.

### Checks

Default settings pass **117 library tests**, all regular integration targets
for `counting-allocator` (including memory, budgets, snapshots/API, panic,
stop/kill, source frames, lifetime and cage/confinement checks), the new
cache/scalar fixtures, the 40-seed differential/exact-arithmetic fuzz
campaigns, and **378 language/R7RS/SRFI conformance trials** across modes.
The opt-in hostile feature was not part of that configuration. The very
large external R7RS benchmark campaign was not rerun in full. No tests,
expected failures, safety ratchets or plateau bounds were relaxed.

`cargo check --no-default-features`, formatting and diff checks pass.
LSP cannot confirm the two >5,000-line core modules; successful compilation
and active tests are the validation, not an empty diagnostic cache.

### Reproduce

```sh
nix develop --command cargo build --release -p techne-vm \
  --features counting-allocator --bin techne-vm

# Same-binary all-off versus default. Records identify each side explicitly.
PROGS='direct indirect hof qsort wide flosum' RUNS=3 ITERS=8 \
  BEFORE_ENV='TECHNE_JIT_FUEL=0 TECHNE_JIT_CALL_CACHE=0 TECHNE_JIT_WIDE_ADD=0' \
  techne run runtime/bench/compare.scm \
  target/release/techne-vm target/release/techne-vm /tmp/loops.json
# Run COUNTS=1 RUNS=1 ITERS=2 separately inside nix develop.

nix develop --command cargo test --release -p techne-vm \
  --features counting-allocator --test call_cache --test wide_add --test stop --test reclaim
nix develop --command cargo test --release -p techne-vm \
  --features counting-allocator --test suites -- --skip bench/
```

Run wall and counted campaigns separately, with a sufficient outer timeout:
Scheme process-scope cleanup cannot run after a hard process kill, so a
killed driver can leave its Cachegrind child behind.

### Still open

HOF is still list/allocation heavy and boxes its wide accumulator each
iteration. The scalar fast path removes the generic call, not those boxes.
The next larger step is bounded unboxed loop state with exact overflow and
root materialization, or measured allocation-policy changes—not changing
exact arithmetic to floats or transparently fusing arbitrary callbacks.
Parsing and local vector-bound reuse remain separate, unimplemented targets.
