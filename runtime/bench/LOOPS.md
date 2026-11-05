# Loop/callback hillclimb

Baseline is `17fb4311`, including the numeric recursive kernel. Frozen binary
`/tmp/techne-loops/before`, SHA-256
`cd027dd56791f3d062d8224bb1bf167db5593db0ae57ac3456475c12ebfd523a`.
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

Raw records `/tmp/techne-loops/fuel-{wall,ablation,bounded-wall,bounded-counts}.json`;
the unrestricted policy's counts are `fuel-counts.json`.

Initially opt-in with `TECHNE_JIT_FUEL=1`. The unit test in
`src/vm/fuel_tests.rs` observes the exact count from a native every iteration
(one inlined call plus one back-edge) and requests allocation-pressure hurry.
Both publication and respecting that notification are asserted, without
timing thresholds or process-global environment mutation. It is excluded
under Miri since it deliberately executes native code. The enabled prototype
passes 117 library tests, recursive-kernel fixtures and all stop/kill tests.
