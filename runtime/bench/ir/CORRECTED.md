# Rerunning the corrected IR passes

Measured 2026-10-23, runtime **`03bcf78d`** (jj `znpqwxkr`), after the
[review-driven correctness fixes](CORRECTNESS.md). All configurations use the
same frozen release binaries with the counting allocator enabled. No runtime
code or default pass selection changed during this campaign.

## Result

**The large structural-optimization gains survived the correctness fixes.**
On unchanged source, combined passes save roughly **4–11× instructions** and
**7–20× execution time** on the closure/rest/scalar kernels. Startup costs
about **2.5 ms** more. These remain targeted-kernel gains, not an application-
wide speedup: the existing steady-state programs, editor keys and hot SRFI
pipelines mostly stay flat.

The historical arithmetic DCE/CSE gains do **not** survive: those unsafe rules
now correctly do nothing. Keeping all passes off merely because startup is a
few milliseconds slower would not follow from these results. A structural
subset is a defensible initial default; enabling everything is a separate
policy decision. This measurement change leaves defaults untouched.

## Protocol

- AMD Ryzen 9 8945HS; all measured children pinned to CPU 4. Governor
  `powersave`, not a locked frequency. Treat small wall-time changes as noise,
  not as instruction-count evidence.
- Selections: `0`, each of the eight passes separately, a **structural** subset
  (`bindings,local,contify,scalar,dead,hoist`), and `all`. `all` still excludes
  the root boot module.
- Kernels: **only the original `before` source is timed**, across flag
  selections. No manually rewritten `after` program supplies a speedup.
  Both variants are checked against the Rust driver's independent oracles.
- Each kernel runs 20,000 iterations, repeated ten times after three warm-up
  calls. Seven process samples for wall time; one deterministic Callgrind
  instruction sample. Collection is restricted to `timed_kernel`, excluding
  reading, compilation, boot and warm-up. Both interpreter and synchronous
  first-call JIT are measured.
- Steady programs: existing `steady/*.scm` and `steady/driver.scm`, unchanged.
  Five fresh processes per configuration, six iterations each; take the median
  of the last three, then the median across processes. Compare off/structural/
  all in both engines, rotating selection order. JIT threshold 1,000 with
  synchronous compilation. All result strings agree across configurations.
- Editor: off/structural/all; eleven rotated startup samples and three runs
  of the existing keystroke example. Report the ordinary asynchronous JIT
  policy separately from synchronous measurements. Callgrind counts only
  `timed_keys`, with synchronous compilation for reproducibility.
- Cached SRFI pipelines: five processes, five snapshot worlds per process,
  size 128 and thirty hot requests. Existing in-process `Snapshot` only,
  **not** a disk compilation cache or lazy package loader. First-call JIT
  compilation is synchronous here, not the default editor policy.

## 1. Unchanged warmed kernels

Ratios are **off / all**, so higher is better. Instruction and elapsed-time
ratios need not agree: avoiding allocations, calls and cache traffic changes
cycles per instruction. All rows retain the independent value checks.

| Original kernel | JIT instruction speedup | JIT wall speedup | Interpreter instruction speedup |
|---|---:|---:|---:|
| constant-let | 1.04× | 1.02× | 1.38× |
| dead-arithmetic | 1.00× | 1.00× | 1.00× |
| local-closure | **5.51×** | **10.70×** | 1.93× |
| closure-hoist | **6.01×** | **11.90×** | 2.19× |
| mutual-tail | **5.90×** | **7.11×** | 1.17× |
| pair-scalar | **3.90×** | **15.73×** | 1.50× |
| vector-scalar | **8.84×** | **20.20×** | 2.75× |
| cse-arithmetic | 1.00× | 1.00× | 1.00× |
| cse-vector | 4.87× | 4.63× | 2.07× |
| licm | 2.12× | 2.30× | 1.79× |
| rest-list | **11.35×** | **18.70×** | 3.32× |
| beta-control | 1.00× | 0.97× | 1.09× |
| fold-control | 1.00× | 1.00× | 1.00× |

The structural subset matches `all`'s instruction gains on the closure,
contification, pair/vector scalar, and rest kernels. Its vector-read CSE
opportunity improves **3.86×**, versus 4.87× with all passes; it does not
perform numeric LICM, so that kernel stays at 1.00×.

Single-pass attribution matters. `local` alone improves local-closure
instructions **1.55×**, versus 5.51× combined, and rest-list **5.83×**, versus
11.35× combined. Removing exposed allocations and dead closure bindings is
where composition pays. `hoist` alone gives 1.93× on closure-hoist; `contify`
alone gives 5.90× on mutual-tail. Scalar replacement alone gives 3.90× on
pair-scalar and 8.84× on vector-scalar. Read CSE alone gives 1.16× on cse-vector;
numeric LICM alone gives 2.12×. Do not credit all combined gains to one pass.

The beta control is already reduced by the original compiler. Its interpreter
still benefits from subsequent binding/move cleanup; no extra JIT beta-
reduction gain is claimed. Small wall-time differences on unchanged controls
illustrate the timing noise floor.

Raw: [2,288 kernel records](results/corrected-kernels.json).

## 2. Existing steady-state programs

Hot milliseconds, medians across five processes. These are ordinary existing
benchmark programs, not new patterns designed for the passes.

| Program | JIT off | JIT structural | JIT all | Interpreter off | Interpreter structural | Interpreter all |
|---|---:|---:|---:|---:|---:|---:|
| fib | 27.200 | 27.222 | 27.434 | 100.173 | 100.180 | 101.013 |
| tak | 38.970 | 39.587 | 39.155 | 124.177 | 124.186 | 125.646 |
| nqueens | 44.236 | 44.569 | 44.332 | 166.486 | 167.218 | 165.901 |
| bintrees | 159.233 | 157.492 | 159.640 | 563.440 | 574.287 | 565.233 |
| hof | 98.916 | 99.097 | 100.078 | 187.711 | 189.326 | 188.387 |
| qsort | 33.420 | 33.241 | 33.253 | 209.973 | 209.773 | 213.633 |
| mandel | 8.023 | 8.153 | 8.062 | 136.069 | 135.462 | 136.255 |
| hash | 67.447 | 67.004 | 67.789 | 82.770 | 84.979 | 84.117 |
| orgparse | 11.505 | 11.512 | 11.542 | 74.623 | 73.810 | 75.949 |

**No broad application speedup is established.** The first tak sample appeared
about 8% faster with passes, but that disappeared after five-process repeats:
its baseline first sample was unusually slow. Whole-process Cachegrind counts
confirm practically unchanged tak work, rather than an 8% compiler win.

The independent existing `icount.sh` check includes startup and compilation,
so it is not substituted for warmed timing:

| Program | JIT structural instruction change | JIT all | Interpreter structural | Interpreter all |
|---|---:|---:|---:|---:|
| tak | +0.0009% | +0.0008% | +0.0013% | +0.0013% |
| hof | +0.0046% | +0.0052% | +0.0021% | +0.0021% |
| qsort | +0.0108% | +0.0091% | +0.0022% | +0.0022% |
| callbacks | −0.8482% | −0.8313% | −0.6119% | −0.6116% |

Raw: [270 steady records](results/corrected-steady.json),
[whole-process instruction counts](results/corrected-application-instructions.json).

## 3. Editor startup, keystrokes and GC

Whole-process startup medians, milliseconds; boot and teardown included:

| JIT policy | Off | Structural | All |
|---|---:|---:|---:|
| Ordinary asynchronous, threshold 1,000 | 20.59 | 23.10 | 23.18 |
| Synchronous, threshold 1,000 | 22.01 | 24.15 | 24.16 |

The ordinary-policy cost is **+2.51 ms** structural, **+2.58 ms** all. This is
not a claim that compilation dominates a long-lived editor's total runtime.

Editor instruction changes per key stay between **−0.88% and +0.67%** across
the two enabled selections. Most are below 0.2%. Typing's ordinary-policy
median remains approximately **0.06 ms**; movement approximately **0.05 ms**.
There is no convincing general keystroke speedup or material instruction
regression. Tail wall latency varies with compilation/scheduling; opening
search occasionally exceeds 4 ms **even with passes off**, and also in enabled
runs. This campaign does not claim to fix that existing responsiveness issue.

The callback oracle is **5,362,247,396** in all configurations. Across all
individual passes, structural, all, and the historical `bindings,dead`
interaction, JIT maximum copying stays **327,535 words** and promotion
**862,852 words**. Interpreter maximum copying stays **327,538 words**;
promotion is 862,864 or 862,867 words. No GC-pause-work regression returns.
The independent whole-program HOF check likewise keeps its maximum pause
unchanged at 1,579,649 words.

Raw: [ordinary-policy wall measurements](results/corrected-applications-default-wall.json),
[synchronous measurements, key instructions and GC](results/corrected-applications-sync.json).

## 4. Existing cached SRFI workloads

Hot JIT request microseconds:

| Pipeline | Off | Structural | All |
|---|---:|---:|---:|
| SRFI 1 lists | 10.512 | 10.417 | 10.412 |
| SRFI 133 vectors | 26.375 | 26.462 | 26.105 |
| SRFI 113/128/69 sets | 877.798 | 875.368 | 879.001 |

Again, these are essentially flat, not the structural kernels' large gains.
Source/library compilation rises from 0.843/2.269/3.828 ms off to
0.952/2.583/4.194 ms all for list/vector/set respectively. Snapshot world
creation remains around 0.1–0.8 ms. Synchronous first-request native compilation
still costs about **7.8/17.5/43.5 ms**; snapshot reuse does not cache native code
or demonstrate a persistent file cache. Interpreter hot times also remain
close to baseline; full cold/warm breakdowns are retained in the raw data.

Raw: [90 cached-world records](results/corrected-cached.json).

## Reproducing

Build the current examples and freeze them in `.local/ir-corrected/bin`:

```sh
cargo build --release -p techne-vm -p techne-editor \
  --features techne-vm/counting-allocator --bin techne-vm \
  --example ir --example ir_cache --example startup --example keys
mkdir -p .local/ir-corrected/bin .local/ir-corrected/results
cp target/release/techne-vm .local/ir-corrected/bin/
cp target/release/examples/{ir,ir_cache,startup,keys} .local/ir-corrected/bin/
```

The exact small campaign drivers are archived with the data. They drive the
existing examples and Scheme benchmarks; they are not additional optimizers.
The recorded revision is deliberately fixed to the measured `03bcf78d`;
update that provenance if measuring different runtime code.

```sh
cp runtime/bench/ir/results/corrected-campaign.py.txt \
  .local/ir-corrected/campaign.py
cp runtime/bench/ir/results/corrected-repeat-steady.py.txt \
  .local/ir-corrected/repeat-steady.py
cp runtime/bench/ir/results/corrected-default-wall.py.txt \
  .local/ir-corrected/default-wall.py
nix develop --command bash -c '
  set -e
  export PYTHONDONTWRITEBYTECODE=1
  for phase in kernels steady applications cache; do
    python3 .local/ir-corrected/campaign.py "$phase"
  done
  python3 .local/ir-corrected/repeat-steady.py
  python3 .local/ir-corrected/default-wall.py
'
```

The standard instruction check was run separately, pinned to CPU 4, with
`PROGS="tak hof qsort callbacks"`, the frozen `TECHNE_VM`, and each of
`TECHNE_IR=0`, the structural selection, and `all`, using
`runtime/bench/icount.sh`. Binary hashes, full samples, source hashes and
measurement metadata are retained in the JSON artifacts. No cross-runtime
Node results are used to attribute gains to IR passes.
