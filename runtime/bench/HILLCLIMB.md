# Bounded VM hillclimb, 2026-11-05

This is the frozen first-round record. The [follow-up investigation](INVESTIGATION.md)
adds diagnostics and fixes the retained-workspace failure described below.

Baseline: `eda95c0132c0c5d90ce345556f147ccb342a5aae`, release build with
`counting-allocator`. Same source programs, arguments and expected outputs
in both binaries. No hand-rewritten workload supplies an optimization win.

## What was kept

1. **List-specific nursery bulk allocation.** `list`, `reverse`, `append`
   and the VM's list builder can reserve up to the entire existing nursery
   for their small pairs. Above it, they still allocate old pairs as before.
   General bulk allocation retains its half-nursery cutoff. The default
   nursery remains 8 MiB, and budget admission/rooting rules are unchanged.
2. **Monomorphic indirect calls.** At most four sites per function record
   one target before its one compilation. A second target makes a site
   generic. Code ids and native indices do not keep sampled closures alive.
   Jobs resolve live ids and retain code dependencies before taking any
   address. A closure's code-id guard permits different instances of the
   same lambda, always using the actual instance's captures. Non-tail sites
   reuse bounded inlining; tail sites reuse guarded compiled calls. Native
   sites reuse known-native paths. Guard failures make ordinary calls.
3. **Reusable kind facts.** Successful pair accesses and `cons` results
   eliminate redundant checks, with independent OSR validation. Existing
   argument-kind propagation now reaches indirect inlinees. Fixnum and float
   inline return kinds propagate to callers, with result checks on fallback.
   Entering an inlinee does not prematurely assign its call's result kind to
   the base register, which still holds the actual closure until return.
4. **Small compilation cleanup.** Set-bit iteration replaces scanning all
   64 positions of each register-set word. Use/definition metadata is built
   once and shared by kind, liveness and dirtiness analyses rather than
   allocating it repeatedly within kind-analysis fixed points.

These mechanisms belong in the existing Rust VM/JIT/collector: a guest Scheme
rewrite cannot remove its own generated call/check overhead or safely change
root/deoptimization metadata. The comparison harness and new control programs
are Scheme; no new native subsystem or heavyweight compiler tier was added.

There is no recompilation, polymorphic-version growth, increased inline limit,
new global range solver or whole-CFG optimizer. Those larger projects are not
claimed as completed optimizations here.

## Warm execution

AMD EPYC 3151, x86-64, CPU 4. Three fresh processes per binary per workload,
alternating order; ten calls per process, median of the last five, then median
across processes. Both use threshold 1000 with synchronous JIT compilation.
Wall times are milliseconds; the host frequency is not locked. Small changes
are not reliable optimization evidence.

| Workload | Before | After | After / before |
|---|---:|---:|---:|
| indirect-call control | 28.905 | 8.575 | **0.297** |
| direct-call control | 6.041 | 6.029 | 0.998 |
| fib | 160.525 | 157.269 | 0.980 |
| tak | 231.585 | 230.251 | 0.994 |
| nqueens | 270.269 | 259.884 | 0.962 |
| bintrees | 839.867 | 844.096 | 1.005 |
| hof | 575.076 | 342.365 | **0.595** |
| qsort | 179.043 | 179.129 | 1.000 |
| mandel | 46.812 | 42.605 | 0.910 |
| hash | 315.564 | 316.244 | 1.002 |
| orgparse | 74.975 | 76.101 | 1.015 |

The indirect and direct controls do the same million additions and return
`500000500000`; only calling through the parameter versus the global differs.
They measure the opportunity, not a universal 3.4x application speedup.

### Instructions and attribution

Cachegrind counts the **whole process**, including reading, compilation, boot
and two workload calls. They are not hot-only instruction counts. Representative
paired measurements:

| Workload / comparison | Before instructions | After instructions | Change |
|---|---:|---:|---:|
| indirect: baseline vs candidate | 330,849,638 | 124,379,656 | -62.4% |
| hof: baseline vs candidate | 3,075,919,810 | 2,275,658,722 | -26.0% |
| qsort: baseline vs candidate | 1,304,372,568 | 1,304,402,593 | +0.0023% |
| fib: baseline vs candidate | 1,485,410,216 | 1,485,408,372 | -0.0001% |
| hash: baseline vs final list-only policy | 1,100,967,999 | 1,100,925,101 | -0.0039% |
| hof: pair facts disabled vs enabled | 2,308,140,746 | 2,275,658,722 | -1.4% |
| bintrees: pair facts disabled vs enabled | 7,075,294,985 | 6,911,603,403 | -2.3% |

With indirect feedback disabled but the list policy and pair facts retained,
HOF took 432.0 ms versus 341.4 ms with feedback: about 21% less time from
feedback on top of the other changes. The indirect control was 28.85 versus
8.55 ms in that ablation.

Mandel's instruction count was essentially unchanged (+0.17% in an earlier
pair), despite its faster wall sample. Do not credit the entire wall difference
to an algorithmic improvement. Likewise, the sub-percent instruction changes
of fib, hash and qsort are effectively flat.

### Memory and GC

For HOF's ten calls, Linux `wait4` reported median peak RSS over three paired
processes of **755,760 -> 99,164 KiB** (738 -> 97 MiB). Median total child CPU
time, including compilation, was **5.79 -> 3.40 seconds**. Both used the same
8 MiB nursery. Avoiding individually allocated old-space reverse results
matters for memory as well as time.

This is not a claim that all GC work decreases. In representative ten-call
logs, minor collections rose from 107 to 169, promoted words from 30,226,955
to 64,469,085, and completed old cycles from 1 to 8. Maximum copied words per
pause remained 1,048,544; maximum slice work rose from 650,052 to 731,078 words.
The policy avoids expensive old-pair allocation and allows temporary lists to
die without inflating old space, rather than simply minimizing promotion.

**Rejected experiment:** letting *all* bulk allocations use the full nursery
increased hash's whole-process instructions by 1.59%. The retained change
applies only to lists; the final hash instruction count is flat. Merely raising
the nursery to 16/32 MiB also helped HOF, but was not necessary and was not kept.

## Compilation cost

Compiler-thread elapsed time, summed over native jobs per process, median over
the three samples (not queue latency or whole-process startup):

| Workload | Before ms | After ms |
|---|---:|---:|
| indirect | 6.71 | 6.27 |
| hof | 18.83 | 19.06 |
| fib | 5.01 | 4.88 |
| qsort | 36.54 | 36.45 |
| bintrees | 19.49 | 19.32 |

These small differences are timing samples, not a claim of a large compiler
speedup. HOF's emitted pre-optimization IR grew from 1,569 to 1,817 instructions
as callbacks were inlined; qsort stayed at 4,606 and fib at 394. Feedback storage
and specialization are fixed-budget; all existing inline depth/size and
compilation-memory admission limits remain.

The existing `compile` example's Callgrind `*timed_forms*` region checked that
front-end compilation was not substantially made worse:

| Phase | Forms | Before instructions | After instructions |
|---|---:|---:|---:|
| read | 7,760 | 153,671,661 | 153,692,819 |
| load | 2,328 | 139,243,066 | 139,469,676 |

That is +0.014% and +0.163%, respectively. These are front-end workloads, not
measurements of native-code compilation. No native-code-size improvement is
claimed from IR counts.

## Correctness and known baseline failure

The final selected release run passed 114 library tests, 40 language tests,
46 memory tests, 14 reclamation tests, source-frame/input/unwind tests and the
raw-access ratchet. New tests cover target changes, actual immutable and boxed
captures, GC inside an inlinee, side effects before errors, logical error frames,
code retention without retaining captured weak keys, tail calls, fallback result
kinds, feedback limits and pair-fact invalidation.

The differential fuzzer passed 40 seeds against the interpreter in five JIT
modes, including background compilation and GC stress, with no skipped seeds.
Targeted feedback tests also passed with GC stress and heap verification.

`load_use_unload_cycles_plateau_and_retained_closures_stay_safe` fails on
**untouched `eda95c01` too**, reproduced from an independent source archive:
its later `other` plateau is about 3.9 MB versus about 2 MB in early samples,
exceeding its 1 MB slack. The candidate fails similarly, including with either
new JIT optimization disabled. The threshold-1 companion test passes. No test
limit was relaxed; the known failure was excluded from that first-round run.
The follow-up trims oversized compiler working buffers and now passes this
unchanged test too; the figures above remain the original frozen measurements.

## Reproduce

Freeze the baseline binary before rebuilding the modified source. Do not build
a second source tree into the same Cargo target directory: equal-named binary
artifacts can replace one another even when Cargo considers a cached build fresh.
Then, from the repository root:

```sh
nix develop --command cargo build --release -p techne-vm \
  --features counting-allocator --bin techne-vm --example compile

RUNS=3 ITERS=10 techne run runtime/bench/compare.scm \
  /absolute/path/to/before target/release/techne-vm /tmp/vm-wall.json

# Counts can take a while: save/compare a few workloads at a time.
nix develop --command env COUNTS=1 RUNS=1 ITERS=2 PROGS='indirect hof' \
  techne run runtime/bench/compare.scm \
  /absolute/path/to/before target/release/techne-vm /tmp/vm-counts.json

# Same binary on both sides, one optimization disabled only on the first.
nix develop --command env COUNTS=1 RUNS=1 ITERS=2 PROGS='hof bintrees' \
  BEFORE_ENV=TECHNE_JIT_PAIRS=0 techne run runtime/bench/compare.scm \
  target/release/techne-vm target/release/techne-vm /tmp/vm-pairs.json
```

`BEFORE_ENV=TECHNE_JIT_FEEDBACK=0` similarly isolates indirect feedback. The
harness verifies results, alternates process order, and checkpoints JSON with
raw iteration times, IR/native compilation logs and GC statistics after every
sample. `COUNTS=1` uses a native executable, not a shell wrapper around one
(Cachegrind does not follow wrapper execs by default).
