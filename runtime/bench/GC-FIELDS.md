# Checked minor-GC field scanning — quiet-machine follow-up

Status: the quiet-machine recheck and remaining integrations are complete.
HOF/hash improve reproducibly, but tak/nqueens have a small cross-binary cost
that is absent in scanner ablation. **Keep this as a separate checkpoint pending
that tradeoff's resolution or approval**, not an unconditional no-regression
win. PR #337 merged at `d0e723d7` during this work; it was not modified.

The initial experiment was interrupted by unrelated compiler load and external
removal of `target/`. After the machine became quiet, the Techne harness was
rebuilt, source hashes checked, and the rebuilt VM verified byte-for-byte equal
to the saved candidate. No unrelated jobs were stopped. The resumed timing
comparisons each had a once-per-second competing-build audit; all three audit
logs are empty. Fresh pre/post samples also showed 99% CPU idle.

## Attribution

The opt-in census separately counts promotion by pair/integer kind and
successful `reverse` preparations (admission plus bulk reservation). Its
checkpoints read cumulative scalar counters only: a reserved nursery span is
unfinished and **must not be scanned**. A unit test exercises that interval.
For one unchanged HOF `run`, after three warmups and probes of rounds 1/2/3:

| Counter | Observed |
|---|---:|
| All promoted words | 6,853,581 |
| Promoted pairs / bytes | 2,284,509 / 54,828,216 |
| Promoted integer objects / bytes | 8 / 128 |
| Successful reverse preparations | 20 |
| Minors during reverse preparation / whole phase | 8 / 16 |
| Pair bytes promoted during reverse preparation | 30,399,984 |

Reverse preparation causes roughly 55% of pair promotion in this sampled
phase. This attributes the collection site, not every copied pair to a source
variable. Nursery phase and preceding probes matter; it is not a universal
promotion ratio. Almost all promoted bytes are pairs. The original/default
scanner's allocation, promotion, reverse counters and answers matched exactly
for all four probes in the final process-selector recheck
(`resume/promotion-{reference,fast}.json`).

## Candidate mechanism

`scan_field` does not rewrite an immediate: it cannot move or shade anything.
A pair with exactly two fields uses two direct visits, avoiding the length loop
and closure/ephemeron dispatch. Pointer fields retain the original evacuation,
forwarding and shading; car-before-cdr order, closure handles and weak treatment
are unchanged. Malformed pair lengths use the original generic path.

**Cage extents remain checked.** Review caught that the early unrolled
prototype bypassed `len_of`'s extent check. Those timings are not acceptable
evidence. The corrected code shares `checked_field_len` between `len_of` and
the fixed-pair guard, before forming raw field addresses. Boundary coverage
checks valid and rejected pair extents near a cage end without dereferencing
synthetic pointers. Do not restore the unchecked prototype to recover speed.

`TECHNE_GC_FIELD_SCAN=0` selects the original scanner. The final selector is
fixed per process, initialized outside collection and read once per minor,
not per object or field. It adds **no Heap/Vm layout field**. Compile-time
fast/reference variants each support marking/non-marking and ephemeron
continuation. Rust is necessary for the collector's memory-safety mechanism;
measurement orchestration and reporting stay Scheme.

## Quiet-machine recheck

CPU 4, synchronous JIT threshold 1000, alternating process pairs. Seven fresh
process pairs per application, twelve unchanged `run` calls per process,
median of the final six calls, then median across processes:

| Application | Baseline ms | Candidate ms | Change |
|---|---:|---:|---:|
| HOF | 316.621 | 306.2685 | -3.27% |
| Hash | 321.891 | 308.685 | -4.10% |
| Qsort | 161.891 | 161.2295 | -0.41% |

Seven-pair same-binary original/default scanner ablation:

| Application | Original scanner ms | Default ms | Change |
|---|---:|---:|---:|
| HOF | 316.229 | 306.199 | -3.17% |
| Hash | 319.7185 | 308.1975 | -3.60% |
| Qsort | 162.446 | 160.413 | -1.25% |

A separate five-pair, ten-call broad pass gave HOF **310.178 -> 303.389 ms
(-2.19%)**, hash **322.008 -> 306.838 ms (-4.71%)**, and qsort -1.03%.
Report a modest **2–3% HOF / roughly 4% hash** warmed improvement, not startup
wins or a new Node parity claim. Qsort shows no regression in these repeats;
no qsort gain is claimed.

The other eight controls have no claimed gain. Broad-pass tak was +1.07%
and nqueens +0.76%; targeted seven-pair repeats gave **+1.18% / +1.16%**.
Their same-binary scanner ablations are **+0.06% / -0.14%**, essentially flat.
This separates the scanner mechanism from cross-binary layout effects, but
**does not erase the observed cross-binary costs** or establish zero regression.
The remaining broad controls range from -0.62% to +0.23%.

Native compilation is effectively flat in the broad logs: HOF **22.38 / 22.20
ms**, hash **17.73 / 17.64 ms**. Ordinary benchmark GC counts/promoted words
match (HOF 160 minors / 8 full / 67,193,004 words; hash 19 / 0 / 5,304,378).
The gain is less tracing work, not fewer allocations or promotions.

Static Rust executable text grows **27,244 bytes** (11,145,375 -> 11,172,619),
about 0.24%. Three-process median HOF peak RSS is **100,084 -> 100,624 KiB**,
an observed **540 KiB increase**, not a memory reduction. No larger nursery,
budget ceiling, memory ratchet or JIT admission bound was introduced.

## Instruction counts and earlier process-selector evidence

Normal release/counting-allocator binary, synchronous JIT threshold 1000,
unchanged source. Same-binary Cachegrind, two calls including boot and
compilation, one process per side (`layout-counts.json`):

| Application | Reference instructions | Candidate instructions | Change |
|---|---:|---:|---:|
| HOF | 1,943,153,900 | 1,863,082,301 | -4.12% |
| Hash | 1,101,184,907 | 1,090,104,275 | -1.01% |
| Qsort | 1,252,944,755 | 1,252,934,927 | effectively flat |

The first five-pair same-binary wall comparison (`layout-ablation.json`) gave
HOF **309.280 -> 304.837 ms (-1.44%)**, hash **316.767 -> 311.333 ms (-1.72%)**,
and qsort **160.409 -> 160.771 ms (+0.23%)**. Each process called `run` ten
times; samples are medians of its last five calls, CPU 4, alternating order.

However, its cross-binary comparison (`layout-runtime.json`) gave HOF
**313.267 -> 313.366 ms**, effectively flat, while hash improved more and
qsort moved +0.81%. A later repeat reached qsort 271–282 ms and timed out
before completing the second comparison. The machine then showed load above
8 with multiple unrelated `rustc` jobs. **Exclude `layout-repeat.json` and the
incomplete `layout-ablation-repeat.json` from throughput conclusions.**

Earlier `checked-*` trials retained the exact cage check but used a per-Heap
selector, hence a different layout. Their same-binary results were HOF -2.60%
and hash -3.32%, with counts -4.12% / -1.01%; the broad cross-binary qsort
movement was +2.30%. These are not relabeled as final process-selector results.
The still earlier `field-*`, `unroll-*` and `final-*` results precede restoration
of the extent check and must not support a performance claim.

The candidate reduces work, **not** allocation or promotion. It does not
change the nursery/window, roots, admission, promotion policy, collection
triggers, list semantics or arithmetic. There is no automatic callback fusion,
destructive reversal or new JIT tier. The executable-text/RSS costs are reported
above. Do not infer a memory or compilation improvement.

## Validation and next decision

On the final process-selector source:

- **121 library tests**, plus memory/cage/confinement/lifetime/budget/reclaim
  integration tests, pass in both default and reference processes.
- **378 conformance trials and 13 hostile tests pass in each mode**.
- Diagnostic build: **124 library tests** and both allocation calibration tests.
- Graph coverage includes shared/cyclic pairs, stable identity hashes, signed
  zero, NaN/infinities, fixnum boundaries and exact immediate bits over repeated
  full collections. The reference run is a separate process, not an unsafe
  environment mutation inside parallel tests.
- Strict Clippy in normal/diagnostic configurations, no-default-features checks
  with/without the census, and formatting pass. LSP is inconclusive.
- **All regular counting-allocator integrations now pass on the final
  process-selector source**, along with the 121 library tests. The first
  rebuild/test invocation hit its outer time budget after compilation;
  the cached complete rerun passed. No failed or partial run is counted as
  complete validation.

Next decision: resolve or explicitly approve the small cross-binary
recursive/vector-control costs before promoting the checkpoint. Do not widen
existing safety/memory ratchets, dismiss layout costs as nonexistent, or
rewrite workloads to obtain a win. Otherwise keep only the attribution
instrumentation.

The Scheme runner is `runtime/bench/allocations.scm`; `PROBES` selects phases.
Use a current `allocation-census` binary for the added promotion/reverse fields.
Do not sample a bulk reservation until initialized; reverse's checkpoint uses
only cumulative totals, not a residual nursery scan. Never use census-enabled
times as throughput evidence.

## Artifacts

All paths below are under `/tmp/techne-pairs/`:

- Frozen baseline `before`, SHA-256
  `b1cdfa7e165d719e502ddee4134ca0e12106a5c3d7ce643957e2df195f72a585`.
- Final process-selector `layout-final` (identical to `layout`), SHA-256
  `fabe99e7e21a6eee8b4b040ccc6831c58b4abeb0fbbfe7ff159cf60d375622d9`.
- Diagnostic `layout-census`, SHA-256
  `9679b43dad738ec6b3fde0a5839377feb868140acc152b717c56dead7bd2924c`.
- Final quiet runs: `resume/{runtime,ablation,broad,control-runtime,control-ablation}.json`.
- Audit logs: `resume/{competing-builds,broad-competing-builds,control-competing-builds}.log`.
- `resume/promotion-{reference,fast}.json`, `resume/rss-{before,candidate}-{0,1,2}.txt`.
- Complete validation: `resume/all-integrations-complete.log`; rebuilt candidate
  `resume/candidate` has the exact `layout-final` hash above.
- `layout-{runtime,ablation,counts}.json`, `layout-*.log` retain earlier evidence.
- `promotion.json`, `checked-promotion-{reference,fast}.json`.
- Historical/superseded prototypes and interrupted runs are explicitly not
  final throughput evidence; no benchmark child remains running.
