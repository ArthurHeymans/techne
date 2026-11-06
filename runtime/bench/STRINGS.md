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
631,994,344 -> 554,301,229 instructions, **12.3% fewer**. Final cross-binary,
compilation and memory comparisons follow before default enablement.

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

Raw local records: `/tmp/techne-scan/{org.callgrind,org-exclusive.txt,
index-wall,strings-wall,sub-vs-both-wall,native-cache-wall,strings-counts}.json`
(the first two are profile files, not JSON), plus `prototype-tests.log`.
