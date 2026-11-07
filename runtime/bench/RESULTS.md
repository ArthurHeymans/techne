# Benchmark results

The tables are the record. A run's raw output lands in `results/`
(gitignored, like `http/results/`): rerun `run.sh` to reproduce a
table, and see gh-pages for CI's instruction-count history.

## Bounded runtime improvements, 2026-11-05

Against `eda95c01`, on an EPYC 3151: indirect-call control **28.9 → 8.6 ms**,
HOF **575 → 342 ms**, and HOF peak RSS over ten calls **738 → 97 MiB**.
The default nursery stays 8 MiB. Most other existing workloads remain
essentially flat. Native compilation remains close to baseline; no general
optimizing tier or recompilation was added.

See [HILLCLIMB.md](HILLCLIMB.md) for the full table, instruction-count
ablations, compilation costs, rejected allocation policy and reproduction
with the Scheme [compare.scm](compare.scm) harness. These are before/after
Techne measurements, not a new V8-parity claim.

The [follow-up investigation](INVESTIGATION.md) finds normal compiled-call
pressure rather than frequent deoptimization in fib/tak, rejects a numeric
register-retention experiment, and fixes the retained compiler-workspace test
without loosening it. The final runtime timings stay essentially unchanged.

## Bounded numeric recursive calls, 2026-11-05

A subsequent [numeric kernel](FAST-CALLS.md), keeping the ordinary JIT as
fallback: fib **155.8 -> 100.5 ms** (-35.5%), tak **231.4 -> 125.2 ms**
(-45.9%). Five alternating process pairs / ten calls; whole-process counted
instructions fall 38.0% / 45.6%. Other workloads admit no kernel.

Fresh matching Node 24.21 controls are 74.2 / 117.3 ms: these two Techne
workloads are now **1.35x / 1.07x** warmed Node, not a general parity claim.
The tradeoff is explicit: selected native compilations cost 5.0 -> 8.3 ms
and 8.5 -> 14.1 ms, code mappings 4 -> 8 and 8 -> 16 KiB. Admission, precise
GC/error/preemption exits, depth bounds, measurements and reproduction are
in [FAST-CALLS.md](FAST-CALLS.md). `TECHNE_JIT_FAST=0` is the same-binary control.

## Callback and scalar-loop follow-up, 2026-11-06

Against the numeric-kernel baseline, unchanged workloads: direct control
**6.03 -> 4.35 ms**, indirect **8.56 -> 6.04 ms**, HOF **338.5 -> 302.5 ms**,
qsort **178.0 -> 160.0 ms**. Three alternating pairs / eight calls; whole
process instructions fall 8.2%, 23.1%, 14.9%, 4.0% respectively. Fib/tak stay
flat. New scalar controls improve **72.7 -> 25.5 ms** (wide exact sum) and
**35.6 -> 15.8 ms** (inexact sum), not application-speedup claims.

The retained changes publish fuel only at boundaries of bounded inlined
loops, cache invariant callback code guards without caching captures, and
inline exact i64/fixnum/ordinary float addition through the original `+`
procedure. All fallback/GC/preemption/error contracts remain. Costs include
HOF native compilation **18.8 -> 22.5 ms**, peak RSS **99,248 -> 100,512 KiB**;
nqueens is about 2% slower by wall time despite flat instructions. No
parsing/vector-bound improvement is claimed. Wider regressing policies were
rejected. Full data, ablations, 117-unit/378-conformance validation, caveats
and reproduction: [LOOPS.md](LOOPS.md).

## Review: interpreter HOF cost, 2026-11-06

The full-nursery list policy has an interpreter-only cost as well as the
compiled HOF/RSS gains above. With `TECHNE_JIT=0`, HOF performs more minor
collections and promotion copying when its temporary lists occupy the
nursery. This is a real tradeoff, not timing noise or a universal speedup.

Review also found avoidable feedback-helper calls with JIT disabled or a
function's compilation already settled. Gating those calls in the dispatch
loop reduces the interpreter HOF count from **3,661,225,584** to
**3,588,194,406** instructions. Against main `1356a30`'s CI reference of
**3,388,697,697**, the remaining cost is **5.9%**. The review rebuild's
compiled HOF count is **963,016,976**. These are whole-process cachegrind
counts on the unchanged `programs/hof.scm`, via `PROGS=hof` and
`TECHNE_VM=... techne run runtime/bench/icount.scm` in `nix develop`.

Retain the list policy for its compiled HOF throughput and peak-RSS gains
rather than reverting it to hide this interpreter cost. PR #334 accepts
this specific regression using the repository's `accepted-regression`
mechanism; the 5% benchmark threshold remains unchanged.

## Bounded string and real-native follow-up, 2026-11-06

Against reviewed PR #334 tip `fc4a2f34`: orgparse **74.86 -> 62.51 ms**
(-16.5%), Mandelbrot **41.40 -> 38.66 ms** (-6.6%). Earlier independent pairs
before the concurrent review fix gave -17.2% / -8.6%. Same-binary all-off/default controls
agree; whole-process instructions fall **16.3% / 9.1%**. Other application
workloads have no claimed algorithmic improvement, and exact first-class
ratio division stays flat. No workload rewrite or heavyweight optimizer.

The retained cases lower small substring/equality operations, avoid redundant
scalar index/native guards, and lower explicit inexact conversion and division
with an already-inexact operand. Native compilation grows **56.4 -> 63.7 ms**
for parsing, **19.1 -> 22.7 ms** for Mandelbrot. Parsing code mappings grow
56 -> 64 KiB; RSS is effectively flat. More aggressive inlining, local vector
bound reuse and sorted emission were rejected.

Fresh paired **Node 24.21.0** controls: parsing **37.49 / 63.35 ms** Node/Techne
(**1.69x**, previously about 2.03x); Mandelbrot **33.03 / 38.73 ms** (**1.17x**,
previously about 1.27x). Across nine application benchmarks, warmed Techne is
**1.45x** Node by geometric mean, not general parity. Tak is **1.04x**, HOF
remains **1.97x** and includes list-versus-array representation costs.

Full paired data, fresh Node table, compilation/RSS, independent ablations,
118-library/378-conformance/13-hostile validation and reproduction:
[STRINGS.md](STRINGS.md).

## Adversarial correction and HOF allocation census, 2026-11-06

The PR #337 review reproduced stale native identity after a compiled mutator
and implicit self-tail jump. Global anticipation now includes the possible
pc-0 edge, so the existing call-return reload points refresh needed globals.
The regression was observed failing before and passing after the fix, with
both functions compiled and the mutator not inlined. See [STRINGS.md](STRINGS.md)
for the correctness fix and the paired qsort wall-time caveat.

A separate, compile-time-optional nursery census profiles the unchanged warmed
HOF source: one ten-round `run` requests **5,200,000 pairs / 124,800,000 bytes**
and **320,061 wide integers / 5,120,976 bytes**. These are **96.058% / 3.942%**
of guest allocation bytes; other objects total 400 bytes, and direct-old
allocation requests are zero. Three processes and one/three warmups agree.

The calibration test distinguishes actual inline allocation from native
fallback under nursery exhaustion. Normal builds contain no census machinery.
A fresh non-census Callgrind profile still emphasizes minor GC and reverse
(overlapping inclusive costs). The next investigation should focus on pair
lifetime/promotion and reversal work, not assume wide-loop unboxing is the
largest win. No new throughput speedup is claimed. Methods, caveats, exact
counts, feature-gated tests and reproduction: [HOF.md](HOF.md).

## Less minor-collection work per promoted pair, 2026-11-06

A minor collection now leaves immediate fields unwritten (they cannot move
or shade) and visits a pair's two fields directly, without the generic
field loop or closure/ephemeron dispatch. The cage extent check that
`len_of` applies runs for pairs too. Allocation, promotion and collection
counts are unchanged: this is less tracing work, not less garbage.

Whole-process cachegrind counts against main `4db749c8`, JIT / interpreter:
HOF **-4.02% / -1.45%**, callbacks **-0.95% / -0.24%**, hash
**-0.63% / -0.44%**, bintrees **-0.38% / -0.13%**; every other program within
±0.11%. Wall time on an Intel N100 (CPU 3, three campaigns of 5-7
alternating process pairs, ten calls each): HOF **-2.1% to -4.4%**, hash
-3.4% to +1.1%, qsort, tak and nqueens within ±1% with no consistent sign.
Executable text grows 10 KiB.

The optional allocation census (`allocation-census` feature) now also counts
promotion by kind and attributes it to `reverse`'s preparation. In one
warmed HOF `run`, 2,284,509 pairs (54.8 MB) are promoted, almost all promoted
bytes; 30.4 MB of them during `reverse` preparing its result, whose
reservation can collect while the input list is still live. That is the
larger lever for HOF, not tracing speed.

## Native identity cache only in reload-free loops, 2026-11-06

PR #337's native identity cache also applied to loops that call another
procedure between uses, such as `numstr`'s `(string-length (number->string
...))`. Each such call reloads kept globals, so the guard was recomputed every
iteration for a single use: about 4 instructions per iteration, linear in
work (+1.07M / +4.07M / +8.07M instructions at 250 / 1000 / 2000 repeats).
Caching is now restricted to loops that can return to the call without a
reload. Whole-process cachegrind counts against main `4db749c8`: numstr (JIT)
**250,561,585 -> 246,491,803** (-1.62%, recovering the regression) and the HTTP `/work` request
**232,819 -> 228,807** instructions (-1.72%); orgparse
**345,825,732 -> 345,793,961**, keeping the cache's gain; every other
program within ±0.2%. The per-call-site JIT switches are now read once per
process instead of once per site.

## Native emission in a fixed order, 2026-11-06

The JIT writes some of a function's blocks (entry loaders, guard and exit
blocks, the numeric kernel's exits) in hash-map iteration order. `HashMap`
seeds SipHash per process, so two runs of one binary compiled the same
function differently: six runs of the steady Mandelbrot gave six IR block
orders, and qsort's `partition` two machine-code layouts. The JIT's maps now
use SipHash with fixed keys (`hash::Map`), so a given job compiles the same
way in every process; `tests/emission.rs` compares `partition`'s disassembly
across eight processes. This removes one source of run-to-run variance in
wall-time comparisons. It makes no throughput claim, and machine code still
depends on runtime state (sampled register kinds, call feedback, globals).

## techne against V8 (Node 24), 2026-10-14

Same programs, same machine. The JavaScript ports in `other/*.js` mirror
the Lua ports (arrays and `Map`s), and their outputs match. All runs are
pinned to one core, which also pins V8's background compiler and GC
threads.

**Whole process** (`run.sh`, hyperfine, 10 runs, mean wall ms including
startup):

| program | chez | guile | luajit | lua 5.4 | python3 | node | techne | techne interp |
|---|---|---|---|---|---|---|---|---|
| startup | 44 | 9 | 2 | 2 | 12 | 24 | 4 | 5 |
| fib | 53 | 39 | 13 | 90 | 187 | 42 | 32 | 112 |
| tak | 56 | 48 | 25 | 98 | 215 | 47 | 46 | 139 |
| nqueens | 67 | 99 | 80 | 290 | 345 | 62 | 59 | 200 |
| bintrees | 107 | 298 | 755 | 1548 | 1028 | 254 | 168 | 570 |
| hof | 82 | 115 | 38 | 112 | 217 | 90 | 109 | 199 |
| qsort | 81 | 80 | 47 | 120 | 424 | 55 | 71 | 233 |
| mandel | 139 | 720 | 9 | 96 | 408 | 36 | 50 | 149 |
| hash | 354 | 224 | 46 | 103 | 125 | 105 | 66 | 80 |
| orgparse | 71 | 89 | 14 | 68 | 187 | 51 | 35 | 80 |

**Steady state** (`steady/run.sh`): each program's work as one `run`
function, called 20 times in one process. Median ms of the last 10 calls,
and of the first call:

| program | node | techne | techne interp | techne / node | node 1st | techne 1st |
|---|---|---|---|---|---|---|
| fib | 16 | 27 | 99 | 1.7× | 18 | 29 |
| tak | 20 | 45 | 133 | 2.2× | 22 | 47 |
| nqueens | 29 | 48 | 189 | 1.7× | 38 | 54 |
| bintrees | 116 | 156 | 563 | 1.3× | 232 | 161 |
| hof | 45 | 89 | 182 | 2.0× | 53 | 93 |
| qsort | 18 | 60 | 228 | 3.3× | 25 | 67 |
| mandel | 7 | 41 | 146 | 6.2× | 9 | 45 |
| hash | 50 | 62 | 107 | 1.2× | 76 | 59 |
| orgparse | 6 | 12 | 70 | 1.9× | 23 | 26 |

- Warmed up, V8 is 2.1× faster (geometric mean). The gap is smallest
  where allocation and hashing dominate (bintrees 1.3×, hash 1.2×) and
  largest in arithmetic and vector loops (qsort 3.3×, mandel 6.2×).
- In those loops each operation checks its operands' tags and picks the
  fixnum or float path, and only fixnums are inferred. mandel's inner
  loop costs about 11 ns an iteration, against 2 ns in V8. Float
  inference and unboxed loop variables are the likely fix for mandel;
  qsort is integer work, so its gap needs a profile first. These are
  hypotheses until measured.
- Since then, float inference in compiled code (registers known to hold
  floats, kept as `f64`s) took mandel's steady state from 41 ms to 10 ms,
  against Node's 6.7 (same machine, one process each): 1.5× instead of
  6.2×. Then the kinds the interpreter sees at a loop head, assumed by
  the code compiled for it, took qsort from 60 ms to 50 (Node: 18) and
  mandel to 8.6 ms: 2.8× and 1.3×. Then keeping vectors' lengths, and
  in loops the globals read vectors through or called inline, took
  qsort to 32 ms (1.8×). (Its partition branch mispredicted as often
  in both, 124M times in 60 runs: the gap was in instructions, 3.5×
  V8's before.)
- techne warms up less: its first call is within 14% of its steady
  state except orgparse's (2.2×), while V8's first call is up to 4× its
  own (orgparse, bintrees). Each figure is one process; repeated runs
  and their spread are still to do.
  For short scripts techne is ahead on the whole process: it starts in
  4 ms against Node's 24, and wins 6 of the 9 programs.

## Steel and other Schemes, 2026-10-05

Harness: `run.sh` (hyperfine, 5 runs, pinned to CPU 4; host load ~59, governor
powersave, so treat as ±10%). Programs in `programs/`, identical across Schemes
except `prelude/`. All outputs cross-checked.
Wall ms, **including process startup** (Steel startup alone is ~150 ms).

| program | chez | guile | luajit | lua 5.4¹ | python3 | steel stock | steel fork | fork +IR | fork resumable | fork native |
|---|---|---|---|---|---|---|---|---|---|---|
| startup | 49 | 10 | 1 | 1 | 12 | 158 | 121 | 152 | 159 | 166 |
| fib | 59 | 42 | 14 | 91 | 205 | 395 | 627 | 668 | 714 | 5806 |
| tak | 60 | 51 | 26 | 104 | 237 | 546 | 809 | 933 | 1097 | 12923 |
| nqueens | 72 | 106 | 84 | 248 | 354 | 911 | 1426 | 1682 | 1832 | 5180 |
| bintrees | 111 | 308 | 823 | 1852 | 1069 | 1850 | 2879 | 2993 | 3320 | 15250 |
| hof | 92 | 126 | 44 | 116 | 238 | 1150 | 1349 | 1378 | 1532 | 3251 |
| qsort | 88 | 89 | 52 | 127 | 451 | 1123 | 1942 | 1798 | 2043 | 11551 |
| mandel | 148 | 782 | 9 | 107 | 392 | 938 | 1315 | 1515 | 1633 | 5131 |
| hash | 382 | 244 | 50 | 115 | 128 | >300 s² | – | – | – | – |
| orgparse | 78 | 97 | 15 | 74 | 184 | 485 | 641 | 678 | 707 | 2993 |
| ffi (2M Rust calls + 2M Scheme calls) | | | | | | | | 432 | 437 | 2906 |

¹ Re-measured separately (3 runs); the campaign's column accidentally ran LuaJIT.
² Quadratic: `IterativeDropHandler::visit_hash_map` drains a dropped persistent
map with `mem::take`, forcing copy-on-write of every node still shared with the
new version. Each insert into a rebound map costs O(n).

### Findings
- Stock Steel is 6–17× slower than Chez/Guile and 2–5× slower than CPython.
- The fork is 1.2–1.7× slower than stock: it disabled the legacy JIT that stock's
  CLI uses by default (visible in stock's profile), and nothing replaced it.
- Semantic IR (Optimize) gives no measurable speedup on any program.
- Resumable slicing costs 5–20% over plain Engine execution.
- jit2 "native" mode is 2–15× slower than interpreting: it copies frame slots into
  a 4 KB buffer per entry and exits at every call (fib: 28M entries, ~2 ops each).
- Startup (~150 ms) is the compiler re-analysing the prelude from source each run.

### Emacs 31.1 comparison (host load ~60, noisy)

| program | emacs .elc | native speed 2 | native speed 3 | steel stock |
|---|---|---|---|---|
| startup | 56 | 77 | 55 | 158 |
| fib | 306 | 361 | 238 | 395 |
| tak | 436 | 374 | 136 | 546 |
| nqueens | 804 | 796 | 507 | 911 |
| bintrees | 1005 | 892 | 387 | 1850 |
| hof | 326 | 310 | 339 | 1150 |
| qsort | 288 | 216 | 230 | 1123 |
| mandel | 2125 | 2896 | 536 | 938 |
| hash | 361 | 297 | 264 | >300 s |
| orgparse | 166 | 149 | 146 | 485 |

Byte-compiled Elisp beats stock Steel everywhere except mandel (Elisp boxes floats).
Native speed 3 is 1.5–5× faster than stock Steel. Speed 2 barely beats bytecode:
it does not compile self-calls directly.
