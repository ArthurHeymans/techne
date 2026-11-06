# Bounded string and real-native hillclimb

Baseline: PR #334 tip (`739462e6`), frozen as `/tmp/techne-scan/before`,
SHA-256 `a58d853271955ecf6dab7d58f2a729a7644f8990d17412a87076ff68012c8665`.
Workloads/arguments unchanged; no comparison against an older VM baseline.

## Profile and bounded mechanisms

Callgrind on three orgparse calls reports 888.6 million whole-process
instructions. Generated code accounts for most execution; `substring`
accounts for 14.0% inclusively. Those costs overlap. The per-character loop
also retags and decodes an already-proven fixnum index at every string read.

The initial, independently ablatable prototype adds:

- `TECHNE_JIT_INDEX=1`: reuse the existing proven shifted fixnum at byte/string
  native indices. Keep unsigned bounds checks; no inferred range or unchecked
  read. The disabled branch retains its original combined tag/range guard.
- `TECHNE_JIT_SUBSTRING=1`: inline three-argument substring of flat ASCII
  sources when the result is at most 64 bytes. Validate tags and the complete
  range first. Reserve the exact rounded size in the already-admitted nursery,
  initialize the original header, bytes and padding, and return a *fresh mutable*
  string. Allocation pressure, unsupported values and representations take the
  original native with unchanged arguments; no collection during construction.
- `TECHNE_JIT_STRING_EQ=1`: inline two-string byte equality for flat UTF-8
  strings of at most 64 bytes (or unequal lengths). Validate both string kinds
  before returning false. Redirected/long strings use the original native.
- `TECHNE_JIT_NATIVE_CACHE=1`: extend the existing 16-global cache to repeated
  inline character/byte/string-read natives. Cache a boolean identity fact,
  refresh with the existing anticipated-global reload machinery at entries and
  after actual calls, and reject globals assigned anywhere in the combined
  body. A failed guard retains the generic call. No cached string pointers or
  mutable ASCII/length facts.

The 64-byte loops are bounded implementation work inside an existing native
case, not unbounded guest loops with missing polls. No original guest poll or
fuel decrement moves. Source addresses are cage-masked at every byte access.
The shared nursery reservation mechanism handles both fixed and bounded
variable sizes. No global inlining limit, nursery size or optimizer tier grows.

These changes belong in Rust because they lower existing native operations
and preserve the VM's allocation/confinement/exit invariants; Scheme cannot
remove its own generated ABI/tag traffic. Parsing policy and reusable
measurement machinery remain Scheme. No parsing workload is rewritten.

## Initial measurements (opt-in checkpoint)

EPYC 3151, CPU 4; alternating fresh processes, eight calls, medians of the last
four, then across three processes. Same-binary independent comparisons:

- Proven indices: orgparse 76.33 -> 73.69 ms; mandel/qsort effectively flat.
- Substring and equality together, indices held enabled: 73.77 -> 68.57 ms.
- Equality on top of substring, five pairs / ten calls: 66.81 -> 63.33 ms.
- Native guard cache on top of the other cases: 62.88 -> 61.05 ms;
  qsort 159.83 -> 159.86 ms, HOF 303.54 -> 301.97 ms (not claimed gains).

Different sessions show frequency/timing variability: do not add these
percentages or treat them as a fresh Node comparison. A same-binary combined
Cachegrind comparison (two calls, includes startup and compilation) falls
631,994,344 -> 554,301,229 instructions, **12.3% fewer**. Final comparisons
and the more conservative default admission are below.

## Correctness

The opt-in checkpoint passes 117 library tests, the numeric/callback/scalar
fixtures, all 15 stop and 15 reclamation tests unchanged, and four new string
fixtures. Independent ASCII-oracle cases span empty/word-boundary/64-byte/long
ranges and comparisons. Differential cases cover UTF-8, redirected strings,
mutation of fresh copies, changed indirect targets, exact error/source traces,
GC stress, collection inside inlined callers, and native guard refresh after a
collecting non-inline call or direct global assignment. A 64 KiB nursery run
keeps 30,000 copied strings alive through nursery exhaustion and full collection.

A first stress fixture retaining 20,000 strings under full-GC-on-every-allocation
exceeded its five-minute outer budget; the bounded stress fixture uses 512 and
the separate nursery-pressure fixture uses 30,000 without quadratic full-GC
stress. No existing test or memory bound was weakened. No timed-out child remains.

## Reproduce

```sh
nix develop --command cargo build --release -p techne-vm \
  --features counting-allocator --bin techne-vm

PROGS=orgparse RUNS=5 ITERS=10 \
  AFTER_ENV='TECHNE_JIT_INDEX=1 TECHNE_JIT_SUBSTRING=1 TECHNE_JIT_STRING_EQ=1 TECHNE_JIT_NATIVE_CACHE=1' \
  techne run runtime/bench/compare.scm \
  /path/to/frozen/before target/release/techne-vm /tmp/strings-wall.json
# Run COUNTS=1 RUNS=1 ITERS=2 separately inside nix develop.
```

Raw local records in `/tmp/techne-scan/`: `org.callgrind`, `org-exclusive.txt`,
`{index-wall,strings-wall,sub-vs-both-wall,native-cache-wall,strings-counts}.json`,
plus `prototype-tests.log`.

## Default admission and rejected experiments

The four mechanisms are enabled by default; independently set
`TECHNE_JIT_INDEX=0`, `TECHNE_JIT_SUBSTRING=0`, `TECHNE_JIT_STRING_EQ=0` or
`TECHNE_JIT_NATIVE_CACHE=0` to ablate them.

Bounded string loops now count against the existing one-complex-native-call
inlining budget. A prefix predicate containing both substring and equality
therefore stays a separate compiled function instead of duplicating the two
byte loops at three caller sites. Compared with the aggressive prototype,
this loses about 2% of parsing speed (61.20 -> 62.38 ms) but reduces native
compilation **82.09 -> 64.24 ms**. `TECHNE_JIT_STRING_INLINE=1` restores that
aggressive bytecode-only admission for measurement; the default is conservative.
No general inline size or depth limit increases.

String-only checkpoint, three alternating pairs / eight calls: orgparse **75.66 -> 62.88 ms**,
**16.9% faster**. Five-pair / ten-call same-binary all-off/default control:
**75.60 -> 62.68 ms**, **17.1% faster**. Other application workloads have no
claimed gain. Initial aggressive default measurements and fresh Node controls
are preserved separately as `aggressive-runtime.json` and
`aggressive-node-vs-vm.json`; they are not the final conservative policy.

That string-only binary repeatedly ran Mandelbrot 2–5% slower despite unchanged generated
instructions (normalized native row/iter assembly agrees) and essentially
identical whole-process counts. In the same-binary all-off/default control it
is flat (44.284 -> 44.292 ms). This is a real wall-time caveat, not a credited
optimization or evidence that unchanged instruction counts guarantee equal
speed. Native block emission currently depends on randomized hash-table order.

Two further prototypes were rejected and removed:

- Straight-line vector access reuse, with separate mutability proofs, removed
  two guards in partition but regressed qsort **1.5–1.8%** in independent paired
  runs. No bounds or immutability check is removed in the retained code.
- Sorting entry/exit block emission to stabilize hash-dependent IR order did
  not improve Mandelbrot and regressed qsort **2.3%**. Determinism alone is not
  performance evidence; the ordering change is not retained.

The first full default validation passed all regular counting-allocator
integration targets, 117 library tests, 378 conformance trials, and separately
all 13 hostile/confinement tests. Hostile probes now include substring, string
equality and a forged huge-header bounded copy. The final conservative policy's
four string fixtures pass, including absent/on/off overrides. Broader final
revalidation, counted instructions and RSS are recorded in the follow-up below.

## Explicit inexact conversion and genuinely inexact division

A separate Mandelbrot profile finds 8.6% of whole-process instructions inside
native builtins, plus generated code's spill/reload/setup around their calls.
Its row function converts integer coordinates explicitly and divides them by
an integer size; its existing arithmetic bytecodes already handle the other
floating operations.

Add two scalar native cases, with independent ablations:

- `TECHNE_JIT_INEXACT=0` disables inline `inexact` and `exact->inexact` on
  fixnums/floats. A fixnum uses the same signed integer-to-f64 conversion;
  an already-inexact value is returned verbatim, including signed zero and
  NaN bits. Other number kinds and type errors retain the original native.
- `TECHNE_JIT_FLOAT_DIV=0` disables two-argument `/` on scalar fixnum/float
  operands when at least one is **already inexact**. Exact/exact division
  retains exact integers/ratios and its zero-division error. Complex numbers,
  large integers, ratios, different arities and changed targets fall back.
  Floating zero, infinities and NaNs match the existing IEEE native fast path.

Neither case collects or retains pointers. Fallback retains original arguments
and logical frames. This is explicit conversion/native ABI lowering, not a
policy that silently coerces exact arithmetic to floats. It belongs in Rust
for the same reason as existing arithmetic lowering, not a new native subsystem.

Conversion alone looked flat (41.45 -> 41.25 ms), but the pair improves
Mandelbrot **41.71 -> 38.19 ms** in five same-binary pairs / ten calls. With
division held enabled, conversion improves **41.26 -> 38.24 ms**. Removing
both sets of native call boundaries matters; do not add individual speedups.
The unchanged-purpose `steady/exact-div.scm` negative control performs exact
first-class ratio divisions: 282.45 -> 281.73 ms in the prototype, essentially
flat. It is not a JavaScript-equivalent floating benchmark.

Two new fixtures check selection/ablation/default behavior and exact expected
answers, plus aliases, changed targets, collecting callbacks, exact/large/
complex/inexact values, nonfinite/signed-zero cases, arity/type errors and full
source traces under ordinary/full-GC execution. All pass. The cases are now
default-on, while retaining both independent zero-valued ablations.

## Final combined measurements

Frozen final binary `/tmp/techne-scan/final`, SHA-256
`8ecce4e1e0c0438da39dd5a86393247353bc49fa59d115bcb488eafcf80b0128`.
Three alternating pairs / eight calls, medians of the last four and then
across processes. Same EPYC 3151 / CPU 4 / release counting-allocator build.

| Workload | PR tip ms | Final ms |
|---|---:|---:|
| direct control | 4.351 | 4.355 |
| indirect control | 6.034 | 6.029 |
| fib | 100.611 | 100.568 |
| tak | 125.172 | 124.909 |
| nqueens | 259.001 | 251.510 |
| bintrees | 846.269 | 836.013 |
| hof | 305.814 | 302.410 |
| qsort | 164.186 | 163.869 |
| mandel | **41.828** | **38.247** |
| hash | 327.006 | 324.309 |
| orgparse | **75.366** | **62.369** |

Retained gains: parsing **17.2%**, Mandelbrot **8.6%**. The other controls
show wall-time/layout variability, not claimed algorithmic gains. In particular,
no vector-bound improvement is retained. Native block-order/hash and binary
layout effects mean unchanged instruction counts do not guarantee identical
wall samples; only the selected mechanisms have corroborating ablations/counts.

Same-binary all-six-off/default, five pairs / ten calls: orgparse **74.73 ->
62.51 ms** (-16.4%), mandel **42.01 -> 38.31 ms** (-8.8%), exact ratio-division
control **274.33 -> 275.16 ms** (+0.3%, effectively flat).

One Cachegrind pair / two calls, **whole process**, including boot/compilation:

| Workload | Before instructions | Final instructions | Change |
|---|---:|---:|---:|
| orgparse | 631,892,123 | 528,825,161 | **-16.3%** |
| mandel | 288,660,204 | 262,352,454 | **-9.1%** |
| qsort | 1,252,915,465 | 1,252,957,343 | +0.003% |
| nqueens | 1,801,162,234 | 1,801,172,700 | +0.0006% |
| exact-div control | 1,581,918,645 | 1,582,904,888 | +0.062% |

### Compilation and memory are not free

Native-job elapsed sums (process medians): orgparse **57.22 -> 64.51 ms**,
mandel **19.28 -> 22.76 ms**, HOF 22.86 -> 22.73 ms, qsort 37.86 -> 37.89 ms.
The conservative string admission avoids the aggressive prototype's 82 ms.
First-call medians, including warming/compilation: orgparse 139.94 -> 133.24 ms,
mandel 58.28 -> 58.28 ms. These samples are not whole-process startup figures.

Three alternating eight-call processes with owner diagnostics, median peak RSS:

| Workload | Before KiB | Final KiB | Machine mappings before -> final |
|---|---:|---:|---:|
| orgparse | 36,248 | 36,008 | 56 -> 64 KiB |
| mandel | 15,996 | 16,268 | 20 -> 20 KiB |
| hof | 100,648 | 100,408 | 32 -> 32 KiB |

RSS is effectively flat, not a memory-reduction claim. Mandelbrot's final
owner total after compilation is about 385 KiB larger, mostly reusable compiler
workspace; it remains accounted. Peak sizes, nursery and original reclamation
limits are unchanged. No general statement about reduced promotion/GC work.

### Fresh Node 24.21.0 controls

Three alternating Node/Techne process pairs / ten calls, CPU 4, medians of the
last five and then across processes. Results agree (normalizing printed list
syntax only). Unlike the preceding answer's reused Node figures, both sides
are fresh here. These are warmed times, not startup/compilation comparisons.

| Workload | Node ms | Techne ms | Techne / Node |
|---|---:|---:|---:|
| fib | 74.098 | 100.443 | 1.36x |
| tak | 119.860 | 124.317 | **1.04x** |
| nqueens | 142.026 | 250.913 | 1.77x |
| bintrees | 591.930 | 841.813 | 1.42x |
| hof | 157.618 | 309.182 | 1.96x |
| qsort | 103.180 | 164.324 | 1.59x |
| mandel | 32.998 | 38.664 | **1.17x** |
| hash | 249.405 | 326.152 | 1.31x |
| orgparse | 37.284 | 62.443 | **1.67x** |
| direct control | 2.774 | 4.353 | 1.57x |
| indirect control | 2.782 | 6.023 | 2.16x |

Geometric mean over the nine applications: **1.45x** Node, down from the
preceding approximate 1.50x comparison. HOF still includes Scheme lists versus
JS arrays. No universal V8 parity, list-fusion, or parsing-language rewrite.

### Final validation and raw records

Final defaults pass **117 library tests**, all regular counting-allocator
integration targets (including budgets, memory, reclamation, frames, stop/kill,
confinement, snapshots and safety ratchets), **378** language/R7RS/SRFI trials,
and separately **13 hostile tests**. The explicit 40-seed differential campaign
passes in five modes, **zero skipped**, alongside exact-integer fuzzing. New
string/real fixtures check absent/default, on and off behavior and source/GC
contracts. No test, expected failure, raw-access ratchet or memory plateau bound
was relaxed. The much larger external R7RS benchmark campaign is not claimed
complete. No heavyweight optimization tier or recompilation was introduced.

`cargo check --no-default-features`, formatting and diff checks pass. LSP cannot
confirm the >5,000-line core file; the small-file probes are inconclusive, not
proof of clean analysis. Compilation and active tests provide validation.

Final raw records in `/tmp/techne-scan/`: `final-runtime.json`,
`final-ablation.json`, `final-instructions.json`, `node-vs-vm.json`,
`rss-{orgparse,mandel,hof}-{before,final}-*.{txt,log,out}`,
`validated-{tests,conformance,hostile,fuzz,check,build}.log`. Historical
`strings-{runtime,ablation}.json` and `aggressive-*.json` are superseded by the
final combined campaign, not silently substituted.

```sh
# Same final binary; original mechanisms versus final defaults.
PROGS='orgparse mandel exact-div' RUNS=5 ITERS=10 \
  BEFORE_ENV='TECHNE_JIT_INDEX=0 TECHNE_JIT_SUBSTRING=0 TECHNE_JIT_STRING_EQ=0 TECHNE_JIT_NATIVE_CACHE=0 TECHNE_JIT_INEXACT=0 TECHNE_JIT_FLOAT_DIV=0' \
  techne run runtime/bench/compare.scm \
  target/release/techne-vm target/release/techne-vm /tmp/native-ablation.json
# Run counted campaigns separately with adequate outer timeout.

nix develop --command cargo test --release -p techne-vm \
  --features counting-allocator --test string_fast --test real_fast --test stop --test reclaim
nix develop --command cargo test --release -p techne-vm \
  --features counting-allocator,hostile --test hostile
```

Next targets remain HOF list intermediates/GC and wide-accumulator boxing,
callback overhead, and vector loops. The regressing local-proof prototype is
not evidence that every bounds strategy must fail, nor permission to weaken
immutability or exact arithmetic.
