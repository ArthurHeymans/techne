# Self-recursive numeric fast-call experiment

The reference is the bounded JIT in `INVESTIGATION.md`. Its six-argument C
entry point, VM register traffic and per-call frame setup dominate fib/tak;
merely retaining more SSA values increased spills. This experiment keeps
that implementation as the fallback, not as an ABI silently reinterpreted.

## Admission and ABI

Only small (at most 32 bytecodes / 64 registers), fixed-arity (1–3 arguments),
self-recursive integer functions qualify. The accepted instructions are
integer constants, moves, arithmetic, fused comparisons, branches and calls
to guarded globals holding this same code. No heap operations, handlers,
unknown calls, native calls or effects. Every live register must be proven
a fixnum under a checked fixnum-argument hypothesis. Self-call results can
be assumed fixnums only because every normal kernel return is proven one;
overflow or a changed procedure leaves the kernel, not a fabricated return.

A companion native function takes `(ctx, bp, numeric arguments..., remaining)`
and returns `(shifted fixnum result, status)`. There are no more than six
arguments, even for tak. Only the ordinary pc-0 loader can enter it, after
checking all arguments. Normal kernel status is zero; a high-bit marker
keeps every cold status distinct, including EXIT at pc 0. The entry strips
it before returning the original dispatch status. OSR, non-numeric inputs
and unsupported functions retain the original JIT and interpreter. The kernel uses one numeric SSA
form and does not carry both tags and unboxed values across recursive calls.

## Rooting and exact exits

The kernel cannot collect, grow registers or invoke arbitrary code. Calls
check the expected closure, available virtual-register capacity, native depth
and the same preemption fuel as the original JIT. Tail self-calls are jumps.
The outer entry also requires its actual closure and every expected self
callee to be the same object. This preserves the closure slot across tail
jumps, including weak-key observations of otherwise unused environments;
code identity alone is insufficient here. Distinct instances use the old path.
Capacity is bounded using the largest call-frame advance; it is not reserved
for an unconditional thousand frames in small worlds. A kernel chain has at
most 128 native activations (numeric live ranges can enlarge its frames),
and consumes the original native-depth budget including its initial call.

The outer entry retains normal frame initialization/publication. It records
its frame base in the native-run context. On an innermost exceptional exit:

1. Initialize the active kernel register span to safe zero values, retaining
   the original outer closure below that span. No collection occurs here.
2. Store the innermost live numeric registers in their tagged representation,
   publish the root window, and record the exact original pc/code/frame.
3. As native callers unwind, store only caller registers below the child's
   callee slot; never overwrite the materialized child's registers. Store
   the guarded closure in that slot and push the ordinary suspended frame.
4. Resume the existing interpreter dispatcher. It executes a slow arithmetic
   instruction once, or makes the deferred call/polls preemption as usual.

Each admitted function has the same frame size at every recursive level,
so the innermost materialized extent covers the ancestors. Liveness admission
also requires suspended caller values to lie below the call base, excluding
its not-yet-produced result. There is no replay of completed subtrees, no GC
with uninitialized frames, and no collecting "fallback helper" inside the
kernel. The ctx and register buffer stay valid throughout its native run.

## Compilation and lifetime budget

This is a bounded companion, not a general second optimizing tier. Admit
both functions' working/code memory, including separately rounded mappings.
Recheck the optional working set against current owner accounting; on
admission refusal try the ordinary compilation without the kernel.
Keep the pair in one isolated arena so
partial compilation failure can discard it whole. Charge both code mappings,
retain the same code dependencies, and reclaim both together. Reuse the same
Cranelift working contexts sequentially; do not create a new backend per call.

`tests/fast_calls.rs` compares values, errors and source frames against the
interpreter, including deep recursion, preemption, overflow in arguments and
results, non-fixnum arguments, redefined globals, and heap roots in an ordinary
caller during a collecting slow exit. Benchmark only with counters disabled.
The measured kernel is enabled by default; `TECHNE_JIT_FAST=0` restores the
ordinary call path, `=1` explicitly enables it. `TECHNE_JIT_LOG=1` reports
admitted kernel IR/code sizes, and `TECHNE_JIT_ASM=name` prints both functions.

Rust is necessary for this mechanism: it changes the native call convention
and must preserve root publication, code lifetime and exact deoptimization
whatever Scheme does. Workloads and measurement policy remain Scheme.

## Measurements

Same EPYC 3151, CPU 4, release/counting-allocator binaries. Five alternating
process pairs, ten calls each; median of each process's last five calls,
then median across processes. No execution counters. Results agree in every
pair. Before is the preceding runtime/workspace improvements, not the original
`eda95c01` baseline.

| Workload | Before ms | Kernel default ms |
|---|---:|---:|
| indirect | 8.554 | 8.550 |
| direct | 6.030 | 6.025 |
| fib | 155.761 | **100.456** |
| tak | 231.389 | **125.218** |
| nqueens | 250.456 | 254.364 |
| bintrees | 842.553 | 849.066 |
| hof | 338.738 | 342.903 |
| qsort | 179.827 | 177.184 |
| mandel | 41.492 | 41.546 |
| hash | 320.724 | 321.142 |
| orgparse | 75.637 | 74.852 |

Fib improves **35.5%**, tak **45.9%**. No other workload admits a kernel;
their wall deltas range from -1.5% to +1.6%, not additional claimed gains.
A same-binary three-pair ablation gives fib 155.738 -> 100.522 ms, tak
231.819 -> 124.985 ms, nqueens 255.145 -> 255.763 ms.

Cachegrind, one pair / two calls (whole process, including compilation):

| Workload | Before instructions | Default instructions | Ratio |
|---|---:|---:|---:|
| fib | 1,485,228,813 | 920,616,293 | 0.620 |
| tak | 1,900,349,522 | 1,034,190,480 | 0.544 |
| nqueens | 1,785,958,892 | 1,801,158,997 | 1.009 |
| hof | 2,271,438,445 | 2,271,829,102 | 1.000 |
| qsort | 1,304,085,984 | 1,304,550,815 | 1.000 |

Nqueens' small cross-binary instruction increase is not present in the
same-binary kernel-off/on control: 1,801,149,741 -> 1,801,157,132. These are
whole-process counts, not a claim to have isolated hot instructions.

Fresh Node **24.21.0** controls, same JavaScript workloads, CPU, five processes
and ten calls: fib **74.151 ms**, tak **117.309 ms**, matching answers
`2178309` and `90`. Techne is now **1.35x** / **1.07x** those two warmed Node
controls. This is progress on recursive calls, not general V8 parity.

### The compilation/memory tradeoff is real

| Workload | Native compile ms before -> default | Pre-opt IR before -> entry + kernel | Code pages before -> default |
|---|---:|---:|---:|
| fib | 5.03 -> 8.31 | 394 -> 441 + 257 | 4 -> 8 KiB |
| tak | 8.46 -> 14.13 | 884 -> 943 + 608 | 8 -> 16 KiB |

Only these two workloads pay for a companion. Other native-job compile sums
stay close (e.g. HOF 19.20 -> 19.19 ms, orgparse 57.55 -> 57.54 ms). This is
not a free compilation optimization: selected small functions compile about
65–67% longer, still milliseconds, with no recompilation. Syntactically
eligible functions with non-fixnum inputs may compile a kernel they do not
enter; the complete ordinary implementation remains available.

Tak's frequently recurring native frame shrinks from `0x90` to `0x60` bytes;
fib's kernel frame is `0x50`. The ordinary fallback/entry remains compiled,
but does not recur on successful kernel calls. This reduces setup/context
pressure, unlike the rejected keep-numbers change to the old ABI.

A synchronous owner-accounting sample for tak starts at 9,581,804 bytes in
both modes, ends at 10,228,072 without / 10,381,872 with the kernel, including
cached workspace and code. The extra 153,800 bytes are accounted, not hidden;
executable mappings grow 8 KiB. Oversized-workspace trimming still applies,
and both original reclamation tests pass unchanged.

### Validation and reproduction

Passed with the kernel enabled: 116 library tests; kernel/ablation cases;
40 differential-fuzz seeds in five genuinely separate modes (the harness now
clears inherited sync mode); exact integer campaigns; language, memory,
budget/counting, break/kill, reclamation, logical/unwind/loading frames and
raw-access ratchet. The kernel fixtures compare stdout, exit status and full
stderr/source traces with the interpreter, including full-GC stress. Dense
recursion keeps eight numeric locals alive across 4,000 logical frames.

With default settings (no FAST/SYNC override), all ordinary Rust integration
targets passed, plus **378** Scheme language/R7RS/SRFI conformance trials in
all selected modes (`--test suites -- --skip bench/`). The unrestricted
full-crate attempt exceeded its 15-minute budget; a separately logged run
confirmed continued progress into the much larger external R7RS benchmark
campaign, not a failing assertion. That full benchmark campaign is **not**
claimed complete. No tests, expected failures or reclamation bounds changed.

```sh
nix develop --command cargo build --release -p techne-vm \
  --features counting-allocator --bin techne-vm

# Same binary: original path versus the bounded kernel.
PROGS='fib tak nqueens hof qsort' RUNS=5 ITERS=10 \
  BEFORE_ENV=TECHNE_JIT_FAST=0 techne run runtime/bench/compare.scm \
  target/release/techne-vm target/release/techne-vm /tmp/fast-wall.json
# Add COUNTS=1 RUNS=1 ITERS=2 inside nix develop for Cachegrind.

nix develop --command cargo test --release -p techne-vm \
  --features counting-allocator --test fast_calls --test stop --test reclaim
```

Raw local records: `/tmp/techne-fast/{final-wall,final-counts,ablation-wall,
nqueens-ablation}.json`, generated assembly and synchronous memory logs.
Frozen binaries `/tmp/techne-fast/{before,after}`, SHA-256 respectively:
`7cd76e7ee4944e3672d139e952e705f10f5ee83a2faa019fa0876ab7cc9a3991` and
`cd027dd56791f3d062d8224bb1bf167db5593db0ae57ac3456475c12ebfd523a`.
Never build another source checkout into the same Cargo target directory.
