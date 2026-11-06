# Bounded string-native hillclimb

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
631,994,344 -> 554,301,229 instructions, **12.3% fewer**. Final comparisons and the more conservative default admission are below.

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

Final three alternating pairs / eight calls: orgparse **75.66 -> 62.88 ms**,
**16.9% faster**. Five-pair / ten-call same-binary all-off/default control:
**75.60 -> 62.68 ms**, **17.1% faster**. Other application workloads have no
claimed gain. Initial aggressive default measurements and fresh Node controls
are preserved separately as `aggressive-runtime.json` and
`aggressive-node-vs-vm.json`; they are not the final conservative policy.

Mandelbrot is repeatedly 2–5% slower cross-binary despite unchanged generated
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
