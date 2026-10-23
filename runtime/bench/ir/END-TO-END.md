# Trying the IR optimizations end to end

Follow-up to the [source-transformation study](README.md), 2026-10-23.
Base runtime: upstream `main` at `e2ffd21c`; implementation change follows
jj change `yxxryzln` (the study).

## Decision

**Keep the compiler prototypes opt-in, not enabled by default.** They
improve the targeted kernels, but this is not evidence for enabling a
whole extra optimizer during every compilation. The initial all-module
pipeline increased startup instructions by about **9.4%** and prelude-load
instructions/form by **11.6%**, without materially improving the existing
benchmark programs. Skipping boot-prelude optimization is a useful policy
change, but does not establish a broad application speedup: editor startup
still regresses about **12.8–12.9%** with the revised pipeline.

The original implementation also lost logical inline frames in error
traces (`lambda` became `toplevel`). The [cached-world follow-up](CACHED.md)
fixes this for lexical inlining and contification and compares complete
traces, including across snapshot reuse. The measurements below describe
the original campaign, not the subsequent debug-metadata implementation.

The roughly **2.2 ms** uncached startup penalty is modest for a long-lived
editor, and compilation caching can amortize optimization work. Startup
cost alone is not a reason to reject these passes. The unresolved concerns
are representative execution benefits and when cold compilation is
scheduled—not whether every optimization pays for itself on the first
startup. The callback GC-work increase measured below was subsequently
[fixed in JIT register-root handling](GC.md); the historical data is retained.

The final implementation defaults to `TECHNE_IR=0`. It can be exercised
with individual passes or `TECHNE_IR=all`. `all` skips the root boot module;
add `root` to include it. These are **experimental process-level controls**,
read once per process, not a supported optimization-level contract.

The [correctness follow-up](CORRECTNESS.md) fixes exception suppression,
boxed-number identity changes, and task-visible heap-read hoisting found by
independent review. Its more conservative rules supersede the original
arithmetic DCE/CSE eligibility below. Historical speedups are not measurements
of the corrected pipeline. The [corrected rerun](CORRECTED.md) now measures
that pipeline: large structural-kernel gains remain, with about 2.5 ms of
startup cost and little change in the existing application workloads.

## What was implemented

[`compiler/optimize.rs`](../../../crates/techne-vm/src/compiler/optimize.rs)
is an actual resolved-tree optimizer, invoked before register allocation.
It changes bytecode for the **original `*-before` kernels**; measurements
below do not substitute the manually transformed `*-after` programs.

| Selection | Implementation and boundary |
|---|---|
| `bindings` | Immutable immediate/local propagation and constant-branch selection. New folds produce only immediate values, not shared boxed constants. No propagation of mutable globals or mutable heap constants. |
| `local` | Small fixed/rest-arity lexical lambda calls are cloned with fresh bound-variable identities and evaluate-once argument staging. No copying nested closures, loops, handlers, escape operations or unresolved heap literals. At most four active inlining levels, 80 nodes per body, 16 arguments, and 10,000 copied nodes per compilation. Rest parameters become explicit construction, available to scalar replacement. |
| `contify` | A restricted nonescaping local `letrec` group with a common tail continuation becomes a dispatch loop with parallel parameter moves. Arity, operator-only use and no parameter mutation are checked. Escaping functions, non-tail recursion and handlers fall back to original compilation. |
| `scalar` | Fresh pairs and small `vector` calls whose object is used only for known local fields become scalar bindings. Local vector writes become local assignments. Unknown indices, identity/escape/alias uses, and capturing the object in another closure reject the rewrite. |
| `cse` | Adjacent identical nonallocating primitive operand trees share a temporary. Includes repeated reads with no intervening effect, but excludes arithmetic that may return fresh boxed numbers. This is not whole-function memory CSE or bounds-check elimination. |
| `licm` | Restricted numeric recurrences get guarded invariant motion. Initial values and the original exit test run first, so a zero-trip loop does not newly evaluate a raising invariant. Initial-value bindings use identities distinct from loop parameters. Invariants cannot read mutable heap fields across back-edge scheduling points; the original exit test is not rewritten. No unknown calls or competing body effects are allowed. |
| `dead` | Unused bindings/sequence values are dropped only when effect/error proofs permit it. Arithmetic is retained: exact-integer facts do not exclude number-size-limit exceptions. Explicit unused pair allocation is retained; see the weak-table failure below. |
| `hoist` | Operator-only local closures capturing only immutable dominating parameters/outer values can move out of a loop. Per-iteration/local captures and identity escape are rejected. This is a fallback; inlining plus dead-binding removal often does better. |

`local`, `contify` and scalar rewrites compose with the other passes. For
example, rest elimination is not just a change to calling convention:
inlining exposes the list, scalar replacement removes it, and unused
bindings disappear. Independent pass measurements are not additive.

Captured-variable environments are rebuilt after rewriting. Eliminating a
call does **not** remove its scheduling checkpoint: an internal `Tick`
expression emits a counted forward `Loop` edge, using the existing VM/JIT
fuel and preemption mechanism rather than another opcode. This is one
reason actual inlining is slower than the ideal manually rewritten loop.

Large generated forms (more than 1,024 resolved variables) use the original
compiler, preserving linear behavior and register-limit diagnostics rather
than exposing them to repeated tree scans. Traversal/cloning uses the
compiler meter and stack-growth guard. The copy budget is not a claim
that every tree analysis is linear; this is another reason to keep the
pipeline experimental.

## Measurements

All instruction work uses the repository's existing Cachegrind/Callgrind
protocols. The machine is the Ryzen 9 8945HS used in the original study.
Binaries were frozen into `.local/ir-prototype` and `.local/ir-final` so
rebuilding/tests could not replace executables during a campaign.

### A. First complete pipeline: includes boot prelude

Measured all eight families together, then every family independently.
The first prototype optimized the root prelude too. The baseline is the
saved unmodified runtime; a disabled-pass run of the same prototype also
checked startup, compile and HTTP work.

| Existing workload | JIT instruction change | Interpreter instruction change |
|---|---:|---:|
| startup | +9.37% | +9.59% |
| fib | +0.20% | +0.08% |
| tak | +0.15% | +0.06% |
| nqueens | +0.16% | +0.04% |
| bintrees | +0.04% | +0.02% |
| higher-order functions | +0.10% | +0.05% |
| qsort | +0.24% | +0.04% |
| mandel | +0.91% | +0.06% |
| hash | +0.30% | +0.20% |
| orgparse | +0.50% | +0.11% |
| callbacks | +0.96% | −0.51% |
| number/string conversion | +0.63% | +0.22% |
| tasks | +0.54% | +1.40% |

Prelude-load work went **66,742 → 74,454 instructions/form**; reading was
unchanged (20,013). Much of the program regression is the fixed startup
cost. The longest-pause work of callbacks also rose **327,538 → 371,278
words (+13.35%)**, even though some allocation/call work was removed. A
transformation changing allocation timing/root lifetimes needs GC-pause
measurement, not just a faster arithmetic loop.

Each pass alone paid substantial common traversal/environment-analysis
cost when applied to the prelude: startup generally rose 5.8–6.0%, with
`dead` about 7.9%; prelude load generally rose 7.1–7.4%, with `dead` about
9.8%. Those numbers include pipeline infrastructure, not solely the cost
of the named rewrite. The targeted source patterns are largely absent
from these existing programs or already handled by Techne's existing
root-procedure/lambda and guarded JIT inlining.

### B. Revised pipeline: skip boot, compare the same binary

The final variant optimizes user/library modules only unless `root` is
explicitly selected. Baseline and experimental runs use the **same final
binaries**, `TECHNE_IR=0` versus `TECHNE_IR=all`, to avoid attributing
unrelated build differences to passes.

The campaign covers all 13 existing language benchmarks, reading/prelude
loading, HTTP plaintext and `/work` requests, editor startup, and all ten
keystroke workloads. Counts below are enabled versus disabled, including
both source compilation and execution where the existing harness does.
Positive is worse.

| Metric | Change |
|---|---:|
| language startup, JIT / interpreter | +0.080% / +0.080% |
| fib, tak, nqueens, bintrees, hof, qsort, mandel, hash, orgparse, numstr | JIT +0.000% to +0.065%; interpreter +0.000% to +0.010% |
| callbacks, JIT / interpreter | +0.640% / −0.595% |
| callbacks longest GC-pause work | **+13.354%** |
| tasks, JIT / interpreter | −0.048% / +0.125% |
| editor startup, JIT / interpreter | **+12.765% / +12.933%** |
| typing / long-line typing, instructions/key | −0.100% / −0.098% |
| movement / modal movement, instructions/key | −0.212% / −0.143% |
| search opening, instructions/key | −0.908% |
| first search key / narrowing, instructions/key | +0.003% / +0.004% |
| search deletion / line yank, instructions/key | −0.156% / −0.046% |
| first command key, instructions/key | −0.043% |
| HTTP plaintext requests, JIT / interpreter | +0.025% / −0.011% |
| HTTP `/work` requests, JIT / interpreter | +0.001% / +0.000% |
| prelude reading / loading, instructions/form | +0.000% / +0.010% |

All ten keystroke workloads remain below the existing instruction budget.
Tiny deltas are not compelling wins: instruction counts can vary with
hashing/allocation/code layout, and this sweep is one observation per
configuration. More importantly, the largest typing/movement savings are
only fractions of a percent, HTTP is effectively unchanged, and editor
**load/compilation** cost gets materially worse. Skipping root compilation
makes the existing prelude-load probe nearly unchanged by construction;
editor startup reveals the work shifted to compiling user/library modules.

This is why a warmed microbenchmark win is not a reason to enable the
pipeline at every source load.

### C. Actual compiler passes on original microbenchmarks

For each original `*-before` procedure, compare the final disabled and
combined-pass binaries/modes at the same iteration/repeat counts. The
`*-after` procedures are still measured as reference/control programs,
not used instead of the original workload.

Each timed kernel executes 20,000 iterations × 20 calls, in one
Callgrind sample per configuration/mode. These ratios are **disabled /
enabled** instruction work, not before-source / after-source ratios.

| Original kernel | JIT instructions/iteration, disabled → enabled | JIT ratio | Interpreter ratio |
|---|---:|---:|---:|
| constant bindings | 25.1 → 24.1 | 1.04x | 1.36x |
| dead arithmetic | 27.1 → 19.1 | 1.42x | 1.28x |
| local closure | 345.2 → 64.1 | **5.38x** | 1.91x |
| closure hoisting opportunity | 335.2 → 57.1 | **5.87x** | 2.17x |
| mutual-tail group | 148.1 → 25.1 | **5.90x** | 1.18x |
| pair scalar replacement | 98.2 → 24.1 | **4.07x** | 1.50x |
| vector scalar replacement | 250.2 → 29.1 | **8.60x** | 2.70x |
| arithmetic CSE opportunity | 41.1 → 32.1 | 1.28x | 1.18x |
| vector-read CSE opportunity | 94.1 → 19.1 | **4.93x** | 2.06x |
| invariant arithmetic | 51.1 → 24.1 | 2.12x | 1.79x |
| rest-list opportunity | 683.2 → 57.1 | **11.96x** | 3.43x |
| immediate-lambda control | 24.1 → 24.1 | 1.00x | 1.08x |
| literal-fold control | 19.1 → 19.1 | 1.00x | 1.00x |

The closure-hoisting and vector-read rows benefit from **other enabled
passes**, not solely the optimization named by the kernel. In particular,
scalar replacement plus invariant motion eliminates the vector/read in
that row; CSE alone has the smaller gain seen in the ablation data. The
immediate-lambda control's interpreter improvement is additional binding
cleanup, not newly implemented immediate-lambda beta reduction. Within
each configuration its two source variants still serve as controls.

Actual lexical-call and rest-list results fall short of the original
study's ideal 11.5x/28.3x JIT opportunities: the compiler retains scheduling
checks and has argument/dispatch overhead the manual rewrite omitted.
The interpreter's mutual-tail benefit is only 1.18x because its dispatch
loop is not free. These are real improvements on the manufactured
workloads, but neither implementation coverage nor application frequency
can be inferred from those ratios.

A separate seven-sample wall campaign (20,000 iterations × 100 calls,
pinned CPU 4) measured the original JIT kernels at:

| Kernel | Disabled → enabled, median ns/iteration |
|---|---:|
| local closure | 14.59 → 2.03 |
| mutual tail | 5.99 → 0.81 |
| pair scalar | 3.98 → 0.66 |
| vector scalar | 9.58 → 0.79 |
| rest-list opportunity | 28.60 → 1.81 |

For the application cross-check, **11 interleaved whole editor-startup
process samples** gave medians **20.76 → 22.96 ms (+10.61%)**. That agrees
with the instruction-work regression, not with extrapolating the kernel
speedups. These remain noisy machine-specific wall measurements; the
instruction results are the primary comparison. Kernel configurations
were separate campaigns, not interleaved with each other.

Raw observations, isolated-pass runs, workload counts and executable hashes
are in [`results/end-to-end.json`](results/end-to-end.json). The initial
prototype and final-policy data are separate in that artifact; do not mix
their baseline/configuration labels. Compilation cap/default/root-policy
changes and formatting occurred between prototype iterations. Binary hashes
identify the frozen measured builds; no claim of bit-for-bit source/build
reproducibility across those iterations is implied.

## Failures found and handled

- Combining binding propagation and LICM originally reused the same
  identities for dominating initial-value bindings and mutable loop
  parameters, turning a terminating loop into a constant-counter loop.
  Fixed by fresh initial-value identities; both combined kernel oracles
  and zero-trip/error tests pass. Testing passes only independently would
  have missed this.
- Dropping unused `cons` expressions removed the deliberate collection
  churn in `lang/gc-shapes`, and a weak-table chain stayed live (expected
  0 entries, observed 300). Retained explicit allocation in dead-value
  cleanup. The suite passes without changing its expectations.
- The register-limit test exposed both a changed diagnostic (65,536
  unused source variables optimized away) and quadratic tree analysis.
  The large-form fallback preserves the existing diagnostic and avoids
  that analysis. The test was not weakened.
- Confirmed the logical-inline-frame trace change separately. This was a
  blocker in the original campaign; it is fixed in the [follow-up](CACHED.md),
  without weakening expectations. The production default remains off.

Focused [semantic tests](../../../crates/techne-vm/tests/suites/lang/ir.scm)
exercise argument staging, rest arguments, assigned parameters/captures,
escaped aliases/closures, mutable field reads/writes, bounds/errors,
bignums/floats, tail/non-tail/escaping recursive functions and zero-trip
invariant motion.

## Validation

- Combined pipeline: the 13-pair independent arithmetic oracles in
  interpreter and first-call JIT modes, plus JIT + GC stress + heap
  verification.
- All local language suites in default/interpreter/JIT/GC modes.
- **414 upstream/local conformance and benchmark trials passed** under
  `nix develop`, with the combined initial pipeline enabled, including
  pinned R7RS/SRFI/reference/benchmark sources.
- Full VM and editor library/integration tests passed with `TECHNE_IR=all`
  after the large-form fallback, including fuzzing, memory, budgets,
  cancellation and stop/preemption tests.
- A separate **100-seed differential optimizer fuzz campaign** compared
  disabled-pass interpreter output/status/error messages against enabled
  interpreter, first-call JIT and JIT + GC stress. It passed. This opt-in
  test originally excluded frame-list equality. The follow-up removes that
  exclusion and repeats the campaign with **complete diagnostic equality**;
  ordinary JIT fuzzing also compares complete diagnostics.
- Release Clippy for the VM library/examples passed with warnings denied;
  Rust formatting and the Python runner checks passed.
- Results do not establish full equivalence of debug frames or every
  handler/continuation/introspection program. No performance pass is
  enabled for ordinary users on the strength of those tests alone.

## Reproduce / continue

```sh
cargo build --release -p techne-vm --features counting-allocator \
  --bin techne-vm --example ir --example compile --example serve
cargo build --release -p techne-editor --example startup --example keys

TECHNE_IR=all TECHNE_JIT=0 target/release/examples/ir --check
TECHNE_IR=all TECHNE_JIT=1 TECHNE_GC_STRESS=1 TECHNE_GC_VERIFY=1 \
  target/release/examples/ir --check
nix develop --command env TECHNE_IR=all cargo test --release \
  -p techne-vm --features counting-allocator --test suites
TECHNE_IR=all cargo test --release -p techne-vm \
  --features counting-allocator --lib --tests
TECHNE_IR=all cargo test --release -p techne-editor --lib --tests
TECHNE_FUZZ_SEEDS=100 cargo test --release -p techne-vm \
  --features counting-allocator --test fuzz \
  experimental_ir_matches_unoptimized_interpreter -- --ignored

# Actual pass intervention; run again with --passes 0 for its baseline.
nix develop --command python3 runtime/bench/ir/run.py \
  --passes all --metric instructions --samples 1 \
  --iterations 20000 --repeats 20 --revision YOUR-REVISION \
  --out .local/ir-enabled.json

# Whole-program and application work. Repeat with TECHNE_IR=0.
TECHNE_IR=all \
TECHNE_COMPILE=$PWD/target/release/examples/compile \
TECHNE_SERVE=$PWD/target/release/examples/serve \
TECHNE_EDITOR_STARTUP=$PWD/target/release/examples/startup \
TECHNE_EDITOR_KEYS=$PWD/target/release/examples/keys \
  nix develop --command bash runtime/bench/icount.sh .local/ir-application.json
```

For ablation use `--passes bindings`, `local`, `contify`, `scalar`, `cse`,
`licm`, `dead` or `hoist`. Add dependencies explicitly for combinations;
use `--passes all,root` to deliberately reproduce the boot-optimization
policy. Do not sum independent percentage improvements or compare a
`*-after` kernel against the original and call it a compiler speedup.

The next useful step is **not enabling everything**. Logical inline frames
are now preserved. Integrate selected passes with the planned persistent
code cache and lazy-loading policy and profile representative
local-call/nonescaping-object patterns. The [GC-work regression is fixed](GC.md). Promote narrowly
justified transformations rather than importing an entire optimizing
compiler pipeline by default.
