# HOF allocation decomposition after the native-cache fix

This is an investigation, not another performance optimization. The current
self-tail invalidation fix is described in [STRINGS.md](STRINGS.md). No
application workload, nursery size, arithmetic contract or benchmark limit
changes here.

## Measure the allocations the JIT actually makes

Counting Rust allocation helpers misses inline nursery allocation. The optional
`allocation-census` Cargo feature adds a fixed, owner-accounted census, enabled
by `TECHNE_ALLOC_CENSUS=1`. It scans completed nursery objects **before minor-GC
forwarding**, and adds the current residual nursery when sampled. Snapshots do
not collect, clear, retain roots or double-count promotion. Direct old-generation
allocation requests are counted separately, excluding GC copies.

The feature is compiled out of normal builds: no census field, allocation
probes, old-allocation counter branches, or collector scan is present there.
Even in the diagnostic build, generated allocation instructions are unchanged.
`gc-stats` prints a cumulative census alongside its existing statistics, under
its existing clock capability. The CLI prints a final census too.

Pairs and immutable integer layouts make their nursery counts exact. Other
object counts can include synthetic bytevector filler after string redirection.
The census is not an allocation trace for arbitrary mutation-heavy workloads,
and direct old allocations do not have kind attribution. Unparseable header kinds or extents set `truncated`, and the
Scheme runner refuses those snapshots; this does not replace full heap
verification. All
HOF probes below had **zero direct-old allocation requests** during the measured
phase, so their pair/integer byte breakdown is complete. Bytes mean requested
guest object words, not RSS, allocator size-class slack or temporary Rust/backend
allocations.

The small Rust mechanism reads collector state unavailable to Scheme. The
reusable driver, warmup, phase selection, subtraction and JSON reporting are
Scheme: [allocations.scm](allocations.scm). It assembles the unchanged steady
source with the ordinary prelude and a measurement-only driver.

## Results

Release/counting-allocator diagnostic build, synchronous JIT threshold 1000,
CPU 4. Three fresh processes, each warming with three unchanged `run` calls,
then probing `round 1`, `round 2`, `round 3` and `run`. Counts and results agree
exactly across all three processes. A one-warmup repeat gives identical phase
counts. An empty `(run)` returning 42 records zero guest allocation words,
including the measurement-only I/O. No wall-time inference is made from
census-enabled runs.

| Probe | Pairs | Pair bytes | Wide integers | Integer bytes |
|---|---:|---:|---:|---:|
| `(round 1)` | 400,000 | 9,600,000 | 0 | 0 |
| `(round 2)` | 666,666 | 15,999,984 | 76,083 | 1,217,328 |
| `(round 3)` | 533,334 | 12,800,016 | 30,602 | 489,632 |
| `(run)` — ten rounds | **5,200,000** | **124,800,000** | **320,061** | **5,120,976** |

A complete `run` requests **129,921,376 guest bytes**:

- pairs: **96.058%**;
- wide integers: **3.942%**;
- other objects: **400 bytes** (40 per round);
- direct-old allocation: **zero** in this phase.

The different rounds matter: filtering accepts nothing for `k mod 3 = 1`,
two thirds for `k mod 3 = 2`, and one third for `k mod 3 = 0`. The ordinary
`run` contains four/three/three such rounds, not ten copies of one phase.
Answers remain `0`, `1777764444777777`, `888882222388890`, and
`7999940003300001`, respectively.

The pair counts agree with the source: map constructs and reverses 200,000
pairs every round; filter constructs and reverses its accepted elements. Of
5.2 million pairs per `run`, **2.6 million are reversal copies**. This is an
attribution observation, not permission to replace `reverse` with destructive
mutation or transparently fuse arbitrary callbacks.

With `TECHNE_JIT_WIDE_ADD=0`, the unchanged `run` still returns the same answer
and allocates the same 5.2 million pairs, but requests **640,107 wide integers /
10,241,712 integer bytes**. The existing scalar fast path has already removed
roughly half the wide-integer allocations; remaining boxing is not the largest
allocation-byte target.

## Fresh CPU-side corroboration

A separate **non-census** fixed binary was profiled on two unchanged HOF calls
with Callgrind. Whole-process total: **1,933,703,631 instructions**. Representative
inclusive symbols:

- `Heap::minor`: **870,786,453 (45.03%)**;
- `reverse`: **592,366,058 (30.63%)**;
- `OldSpace::alloc`: **169,444,929 (8.76%)**.

These costs **overlap**, include startup/warmup/compilation, and must not be
summed. Inline attribution can be split across source files. The principal
`num::add` symbol is only 92,666 instructions: arithmetic now runs mostly in
generated code, not those Rust symbols. That does not establish zero integer
decoding/boxing cost.

## What to try next, and what not to claim

1. **Prioritize pair lifetime/promotion and reversal work.** Measure where
   minor collections promote temporary map/filter chains, especially when a
   large reverse needs another nursery span. Then try one narrow allocation/
   collector policy, with unchanged workloads and independent ablation. Keep
   exact roots, budgets, pauses, weak observations and the interpreter HOF
   tradeoff visible. Do not simply enlarge the nursery.
2. A **reusable Scheme list-builder/pipeline API** can avoid reversal copies
   or intermediate lists explicitly. Measure it as an application/API variant,
   not as a speedup of the unchanged HOF VM workload. Arbitrary callbacks and
   observable identities preclude an automatic fusion assumption.
3. **Defer a general unboxed wide-loop-state project.** Removing every remaining
   integer box eliminates at most about 3.94% of guest allocation bytes here,
   not necessarily 3.94% of CPU time. Its exact overflow, side-exit/callback
   materialization and GC obligations are substantial. A later CPU-grounded,
   narrowly bounded experiment may still justify it.

No new throughput improvement is claimed from this investigation.

## Calibration and reproduction

Feature-only unit tests check snapshots across collection, no double-counting
of promotion, direct old requests, and safe refusal of malformed nursery data.
An end-to-end test forces ordinary JIT execution and 64 KiB nursery exhaustion:
50,000 loop steps produce exactly 50,002 pairs and **50,000 inline integer
objects**, versus **100,000** with scalar addition disabled. It checks both
allocation counts and emitted selection, not selection markers alone. A second
test checks absent/zero/one opt-in behavior. Full-GC stress is deliberately not
used to claim inline-allocation coverage: stress disables the bump pointers.

```sh
nix develop --command cargo build --release -p techne-vm \
  --features counting-allocator,allocation-census --bin techne-vm
cp target/release/techne-vm /tmp/techne-census

PROBES='((round 1) (round 2) (round 3) (run))' WARMUP=3 \
  techne run runtime/bench/allocations.scm /tmp/techne-census \
  runtime/bench/steady/hof.scm /tmp/hof-allocations.json
# A custom steady source and PROBES can measure another workload unchanged.
# Direct-old bytes are explicitly unclassified and produce a warning.

nix develop --command cargo test --release -p techne-vm \
  --features counting-allocator,allocation-census --lib heap::census
nix develop --command cargo test --release -p techne-vm \
  --features counting-allocator,allocation-census --test allocation_census
```

Validation: **120 normal library tests**, all regular counting-allocator
integration targets, **378** non-benchmark conformance trials and **13** hostile
tests. The diagnostic build passes **122 library tests**, both calibration
integration tests, and feature-enabled memory/budget/reclamation/string checks.
Strict Clippy passes both configurations, as do no-default-features library
checks with and without the census feature. Formatting/diff checks pass.

Frozen diagnostic binary `/tmp/techne-hof/census-feature-final`, SHA-256
`e29ce0ea87479e5dbdb0cbb07df0c4f98fbb1349a740475f9241723b249fd253`.
Non-census CPU-profile binary `/tmp/techne-hof/fixed-edge`, SHA-256
`81b697958cf89c2755a9cff6f98248b851c86e7321e6105c19180ab68ba48668`.
The final normal binary has no census symbols; its frozen copy is
`/tmp/techne-hof/final`, SHA-256
`b1cdfa7e165d719e502ddee4134ca0e12106a5c3d7ce643957e2df195f72a585`.
Raw records: `/tmp/techne-hof/final-allocations-{0,1,2,warm1,wide-off}.json`,
`census-control.json`, `final-{default-tests,census-unit,conformance,hostile}.log`,
`fixed-hof.callgrind`, `fixed-hof-{inclusive,exclusive}.txt` and
`normal-census-off.json`. Earlier census binaries and `allocations*.json`
precede final validation and are superseded.
