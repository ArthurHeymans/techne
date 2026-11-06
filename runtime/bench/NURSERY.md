# Where the remaining gap is not: GC attribution and the nursery capacity

Status: measurement only. No code, policy, default or workload changed. It
reorders the next steps, so the numbers and the caveats are the deliverable.

## Method, and what these counters can and cannot say

The collector's counters come from `TECHNE_GC_STATS=1`. They are **cumulative
over the whole process** (boot, compilation and every call), and their
durations are elapsed `Instant` times, so they include descheduling. Counts
and promoted words are not history-free either: asynchronous compilation
changes when the engine switches, and job construction captures run-time
observations (`vm.rs:5378`).

What the tables do, therefore:

- **Differenced counters.** Each row runs the same workload twice, in
  separate processes, at `ITERS=N0` and `ITERS=N1`, and reports
  `n = N1 - N0` calls' worth: ΔC = C(N1) − C(N0) for each counter, and
  GC ms/call = Δ(minor_time + full_time) / n, GC share = GC ms/call divided by
  the wall median. Counts and promoted words are divided by n the same way.
  This approximates the calls in `(N0, N1]`; startup and compilation elapsed
  times and collection-boundary effects need not cancel exactly. Call counts
  and the raw per-call wall vectors are in
  `/tmp/techne-nursery/results.jsonl`.
- **Wall** is the median of the last four calls *in call order* (not the four
  slowest) of the `N1` process.
- All rows use `TECHNE_JIT_SYNC=1` (synchronous compilation, the repository's
  attribution convention), `taskset -c 4`, `ITERS` 2→6 (2→10 for orgparse),
  release build with `counting-allocator`. Peak RSS is the median Linux
  `wait4` `ru_maxrss` over three fresh `ITERS=6` processes for every RSS row
  shown, converted from KiB to MiB; it grows with call count, so the value
  means nothing without them. The host was at load average 3–5 from unrelated
  agent tooling: wall figures are ±a few percent and are quoted for the share,
  not as a quiet-machine campaign.

## At the default 8 MiB nursery

| Workload | wall ms/call | GC ms/call | GC share | minors/call | full/call | promoted kw/call | max pause | max slice |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| hof | 316.1 | **189.4** | **59.9%** | 16.00 | 0.75 | 6659.3 | 25.6 ms | 7.13 ms |
| hash | 317.5 | 63.2 | 19.9% | 2.00 | 0 | 563.9 | 50.6 ms | 2.35 ms |
| bintrees | 841.3 | 90.0 | 10.7% | 43.00 | 0.25 | 2926.5 | 18.6 ms | 4.66 ms |
| orgparse | 66.9 | 1.2 | 1.8% | 0.38 | 0 | 63.8 | 9.70 ms | 410 ns |
| nqueens | 262.0 | 0.1 | 0.1% | 5.50 | 0 | 1.1 | 47 µs | 420 ns |
| fib | 100.7 | 0 | 0% | 0 | 0 | 0 | – | – |
| tak | 128.2 | 0 | 0% | 0 | 0 | 0 | – | – |
| qsort | 161.6 | 0 | 0% | 0 | 0 | 0 | – | – |
| mandel | 41.9 | 0 | 0% | 0 | 0 | 0 | – | – |

Two different populations:

- **hof is mostly collection.** hof's differenced recorded GC time divided by
  its wall median is 59.9%. Subtracting these estimates leaves about 127 ms per
  call; this is not an instrumented decomposition of warm execution. hash
  (19.9%) and bintrees (10.7%) are intermediate.
- **nqueens collects frequently but promotes almost nothing**: 5.5 minor
  collections per call, 1.1 kw promoted, 47 µs of pause.
- **Recorded collection cannot explain the gaps of qsort, mandel, fib, tak and
  orgparse** (0–1.8%). Allocation throughput, call overhead, generated-code
  operations and checks are candidate targets there; each needs its own
  profile and ablation, and these counters say nothing about which.

## Raising the capacity

Same binaries and sources, only `TECHNE_NURSERY_KB`:

| Workload | Nursery | wall ms/call | GC ms/call | GC share | minors/call | full/call | promoted kw/call | max pause | max slice | peak RSS |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| hof | 8 MiB | 316.1 | 189.4 | 59.9% | 16.00 | 0.75 | 6659.3 | 25.6 ms | 7.13 ms | 97.2 MiB |
| hof | 32 MiB | 156.6 | 56.7 | 36.2% | 4.00 | 0.25 | 1980.9 | 29.3 ms | 6.92 ms | 117.4 MiB |
| hof | 64 MiB | **123.2** | 19.8 | 16.1% | 2.00 | 0 | 800.0 | 19.5 ms | 630 ns | 118.1 MiB |
| bintrees | 8 MiB | 841.3 | 90.0 | 10.7% | 43.00 | 0.25 | 2926.5 | 18.6 ms | 4.66 ms | 88.4 MiB |
| bintrees | 32 MiB | 770.0 | 31.1 | 4.0% | 10.75 | 0 | 1046.7 | 15.2 ms | 1.00 µs | 90.9 MiB |
| bintrees | 64 MiB | 765.1 | 20.0 | 2.6% | 5.50 | 0 | 638.7 | 12.5 ms | 1.02 µs | 108.1 MiB |
| hash | 8 MiB | 317.5 | 63.2 | 19.9% | 2.00 | 0 | 563.9 | 50.6 ms | 2.35 ms | 190.2 MiB |
| hash | 32 MiB | 279.2 | 64.3 | 23.0% | 1.25 | 0.25 | 2680.8 | 63.9 ms | 19.9 ms | 129.7 MiB |
| hash | 64 MiB | 256.4 | 34.3 | 13.4% | 0.50 | 0 | 1546.9 | **96.3 ms** | 869 ns | 140.1 MiB |

Increasing nursery capacity reduces hof's collection frequency, its cumulative
evacuation/promotion and its measured GC time (316 → 157 → 123 ms per call;
189 → 57 → 20 ms of collection). These counters do **not** separate copying,
old-space allocation, scanning and old-generation collection: in this
collector `Heap::evacuate` *is* promotion — a nursery object is copied into old
space, counted as promoted, and the old space is allocated with it. So "copying"
and "promotion" are one operation here, not two explanations.

What the promotion column does and does not say: cumulative promotion per call
falls from 6659 to 800 kw (about 88%) for identical work. That is a strong
sensitivity to *when* collections happen; it is not an object-age
distribution. At the default, the 16 collections per call average 416 kw each
(~40% of the 1 MiB-word nursery), and one hof call allocates ~15 nurseries'
worth (5.2M pairs × 24 B ≈ 125 MB). An object that survives several small-window
collections can still die before one large-window collection, so the "share of
promoted bytes that would die before a second collection" cannot be read off
this table. Measuring it needs age/cohort survival at a fixed window.

Pause: whole-process maximum combined pauses for hof were 25.6, 29.3 and
19.5 ms at 8/32/64 MiB; hash's were 50.6, 63.9 and 96.3 ms. Differenced
completed old-generation cycles also vary across configurations (hof 0.75,
0.25 and 0 per call). These observations do not isolate causes or establish a
pause-per-window model, and do not test the `set_nursery_window` comment's
~0.8 ms/MiB estimate.

RSS: hof +21 MiB, bintrees +20 MiB, hash −50 MiB from 8 to 64 MiB, and hash is
non-monotonic (32 MiB is 10 MiB lower than 64 MiB) while its cumulative
promotion *rose*. These measurements do not identify the causes; net old-space
use, committed nursery and allocator slack would have to be measured, not
inferred from promotion.

For context only: the warmed Node 24.21.0 control for the hof program in
[STRINGS.md](STRINGS.md) is 157 ms, from a different campaign whose JavaScript
port uses arrays where the Scheme source uses lists. hof at 123 ms here shows
its sensitivity to collection policy within this campaign; it does not isolate
arithmetic or call overhead and does not establish performance relative to
that control.

## What to try next, in this order

1. **An adaptive young-generation capacity**, as a measured experiment: grow
   when survival is high and shrink when low (hof needs it, nqueens does not),
   admitted through the existing world budget, with RSS reported and the
   GC-free workloads as controls that must not move. Earlier *static* 16/32 MiB
   increases helped hof but were not retained
   ([HILLCLIMB.md](HILLCLIMB.md)); this is not a proposal to raise
   `NURSERY_WORDS`, and the rows above are one host.
2. **Per-operation and per-call work** for everything recorded collection does
   not explain. The compiler's call entry (six-argument C ABI, frame zero-fill,
   trampoline return) and per-element vector/string checks are candidates, not
   conclusions; [FAST-CALLS.md](FAST-CALLS.md) measured a narrower numeric call
   contract moving fib 35% and tak 46%, and each other workload needs its own
   profile and ablation.
3. **A survivor/aging space** could reduce premature old-space allocation and
   the later old-generation work that follows it, but it still evacuates and
   copies surviving objects, possibly repeatedly, and its throughput and peak
   RSS relative to more capacity are unmeasured. It needs age/to-space
   accounting and old→young references preserved across collections; the
   current remembered set is drained at every collection precisely because
   every live young object becomes old.

## Reproduce

```sh
nix develop --command cargo build --release -p techne-vm --features counting-allocator
cd runtime/bench
python3 gen-org.py            # the sample is generated, not tracked
PROGS='fib tak nqueens bintrees hof qsort mandel hash orgparse' ITERS=6 CPU=4 \
  ./steady/run.sh             # assembles build/steady/* and runs the engines

# One warm-phase row: the same workload at ITERS=2 and ITERS=6, counters
# differenced over calls 3-6, wall median of the last four calls of the second.
ITERS=2 TECHNE_JIT_SYNC=1 TECHNE_NURSERY_KB=8192 TECHNE_GC_STATS=1 \
  taskset -c 4 ../../target/release/techne-vm build/steady/hof.scm
ITERS=6 TECHNE_JIT_SYNC=1 TECHNE_NURSERY_KB=8192 TECHNE_GC_STATS=1 \
  taskset -c 4 ../../target/release/techne-vm build/steady/hof.scm
```

`steady/orgparse.scm` reads `org-sample.org` at run time, so regenerating the
sample needs no reassembly. The wall median must be of the last four values of
the driver's `µs` line *in order*, and peak RSS needs the same `ITERS` on both
sides of a comparison.
