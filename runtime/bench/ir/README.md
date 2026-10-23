# Language-IR optimization study

Baseline: upstream `main` **e2ffd21c** (fetched 2026-10-23).

**Follow-up:** [actual compiler passes tested end to end](END-TO-END.md).
The prototypes reproduce many kernel gains, but do not justify enabling the
pipeline by default: editor startup regresses and logical inline error
frames need preservation. The original source-rewrite measurements below
remain a separate baseline, not the actual-pass results.

## Conclusion

Prioritize **known lexical calls and closure/rest-list elimination**, then
**contification** and **small-object scalar replacement**. Immutable-local
propagation is a cheap companion, particularly for the interpreter. Add
local CSE and conservative loop-invariant motion after there is a sound
language-level effect model. Do not begin by replacing Cranelift or copying
a whole-program Scheme compiler.

This is a broad, source-backed survey of 16 Scheme/Lisp implementations,
not an exhaustive census of every implementation. Only **Techne** was
performance-tested. This original study did not modify the compiler;
the subsequent opt-in compiler experiments are documented separately.
The benchmarks below measure the opportunity from explicit source rewrites,
**not speedups delivered by an implemented optimizer pass**. They justify
experiments, not an application-wide performance claim.

## What Techne already does

Read alongside [`../../TECHNE-VM.md`](../../TECHNE-VM.md):

- `compiler.rs`: resolved lexical variable/function identities; mutation and
  capture tracking; boxes only for assigned-and-captured variables;
  tail-only named lets become `Loop`/`LoopCall`. Never-redefined primitives
  become bytecode, comparisons fuse with branches, globals fuse with calls.
- `Compiler::apply`: immediate fixed-arity lambda calls already become lets.
  `Compiler::inline_call`: guarded source inlining of root-module procedures
  when passed explicit lambdas (including passing them through nested
  inlining). This is **not** general known-lexical-function inlining.
- `fold`: literal integer arithmetic/comparisons and some literal predicates.
  `let_form`/`let_star` still retain lexical bindings; they are not an
  immutable-local propagation pass. `Dest::Effect` omits some unused values,
  not arbitrary possibly-raising primitive computations.
- `jit.rs`: guarded small global-call inlining, call-site specialization,
  liveness/dirty-register analysis, observed fixnum/flonum specialization,
  vector-kind facts, nursery allocation, and interpreter side exits. It
  invokes Cranelift with `opt_level = speed`. Machine optimization already
  happens; it cannot generally recover Scheme nonescape/effect facts from
  allocating helpers, frame publication, error exits and generic calls.
- A lexical callee still follows the generic `Call`/`TailCall` route. Local
  `letrec` groups are closures/boxes rather than one shared local CFG.
  Fresh pairs/vectors still allocate even when used only for local fields.

The beta-reduction and literal-folding **negative controls** below check
that this campaign does not recommend work already done.

## Survey: what to borrow, and what not to borrow

The entries describe the cited implementation/version, not a guarantee
that every optimization is always enabled. Backend optimizations are
separated from language optimizations; an unsafe compiler setting is not
equivalent to a safe optimization.

| Implementation | Language IR / verified optimization examples | Relevance to Techne |
|---|---|---|
| **Chez Scheme** | Expanded `Lsrc`, then nanopass IRs. CP0 integrates procedures, propagates bindings and folds/simplifies in value/test/effect contexts, with **size and effort limits**. Later passes optimize direct calls/closures and unbox floating-point variables. [Implementation](https://github.com/cisco/ChezScheme/blob/main/IMPLEMENTATION.md), [CP0](https://github.com/cisco/ChezScheme/blob/main/s/cp0.ss), [nanopass pipeline](https://github.com/cisco/ChezScheme/blob/main/s/cpnanopass.ss). | Model a bounded lexical simplifier on CP0, not an unbounded fixed-point inliner. Closure optimization belongs before low-level lowering. Techne already unboxes numeric JIT locals. |
| **Racket CS / BC** | The [performance guide](https://docs.racket-lang.org/guide/performance.html) documents propagation, folding, inlining, dead-code elimination, known-function/tail-loop optimization and numeric unboxing. **CS is the current Chez-based implementation**; BC's bytecode/JIT is a distinct older pipeline. Module immutability helps optimization. | Known lexical calls are transferable. Assuming module globals can never change is not: Techne is a live, reloadable environment. Retain global guards/dependencies. |
| **Guile** | Tree-IL partial evaluation precedes CPS. Higher-order CPS runs DCE, simplification, contification, integer devirtualization, peeling, CSE and type folding. First-order CPS adds numeric specialization, LICM, primitive specialization, arity/branch/return-type optimization, CSE and loop rotation. Closure conversion classifies operator-only uses. [Source](https://git.savannah.gnu.org/cgit/guile.git/tree/module/language/cps/optimize.scm); authoritative inspected local source: `~/src/guile/module/language/{tree-il,cps}` at `5bdfc6e609638af4d3f7f552d13926c4289da975`. | Best detailed reference for contification and effect-aware CSE/LICM. Borrow the analyses incrementally, not the entire CPS tower. Preserve allocations' identity and invalidate reads after clobbers. |
| **Gambit** | The [4.9.8 manual](https://gambitscheme.org/4.9.8/manual/) exposes primitive/user inlining, an expansion limit, lambda lifting, constant folding, dead-local optimization, separate/block compilation and safety declarations. | Lexical lifting and bounded inlining fit. Block compilation's assumption about global mutation and unsafe checks do not fit Techne's default semantics. |
| **CHICKEN** | The [5 compiler manual](https://wiki.call-cc.org/man/5/Using%20the%20compiler) documents local flow analysis, size-limited inlining, type-directed specialization and local-procedure clustering into dispatch loops. Higher optimization settings also enable `local`, `block`, `unsafe` or disabled interrupts. | Local call analysis and clustering are relevant. Do not conflate higher optimization levels with preserving redefinition, safety or scheduling. |
| **Gauche** | [Compiler source](https://github.com/shirok/Gauche/blob/master/src/compile.scm) explicitly describes IForm parsing/resolution, limited beta substitution, local closure elimination/tail loops, folding and branch/call cleanup, then lambda lifting. It emphasizes low runtime-compilation cost. | Particularly close to Techne's constraints: a small tree-IR pipeline can do useful lexical work without a wholesale SSA/CPS rewrite. |
| **Steel** | AST/semantic passes inline known functions, lower anonymous calls to lets, lift closures, evaluate constants and lower rest arguments. Some extra inlining is environment-controlled. Inspected `~/src/steel/crates/steel-core/src/compiler/compiler.rs` at `dec633b908afeafeaf62bab457a92e2bf873745a`; [upstream source](https://github.com/mattwparas/steel/blob/master/crates/steel-core/src/compiler/compiler.rs). | Extend Techne's existing explicit-lambda reduction to lexical bindings; inspect operator-only uses before making closures. Do not assume optional Steel passes are enabled by default. |
| **MIT/GNU Scheme** | SCode optimization (`sf`) and native compilation. The [12.1 manual](https://www.gnu.org/software/mit-scheme/documentation/stable/mit-scheme-user.html) documents full beta integration, operator-position integration, operator replacement/reduction and type/range-check controls. | Operator-position use analysis is a useful small first step. `usual-integrations` and disabled checks are semantic assumptions, not a free safe speedup. |
| **Bigloo** | [Compiler pipeline](https://github.com/manuel-serrano/bigloo/blob/master/comptime/Engine/compiler.scm) has effect/stackability analysis, inlining and beta reduction, type-test dataflow, globalization, CFA, integration, numeric specialization, repeated reductions and useless-cell removal. | Effects, escape/stackability and type facts should be explicit. A large CFA/AOT pipeline is not automatically worth its load-time cost for Techne. |
| **Kawa** | Expression-tree rewriting before JVM codegen: [`InlineCalls`](https://github.com/spurious/kawa-mirror/blob/master/gnu/expr/InlineCalls.java) handles types, procedure-specific folding/inlining and lambda-to-let reduction; [`FindTailCalls`](https://github.com/spurious/kawa-mirror/blob/master/gnu/expr/FindTailCalls.java) tracks uses and common return continuations for closure-free inlining/tail calls. These are inspected mirror sources, not a claim about every Kawa release. | The single-return-continuation analysis is directly relevant to contification. JVM optimizations are not themselves Scheme escape analysis. |
| **Chibi Scheme** | [`simplify.c`](https://github.com/ashinn/chibi-scheme/blob/master/simplify.c), conditional on `SEXP_USE_SIMPLIFY`: arithmetic constant evaluation, immutable literal substitution, conditional folding, dropping effect-free sequence terms, unused-rest analysis. Some more aggressive reference/let rewrites are explicitly disabled. | A useful deliberately small baseline. Preserve exception behavior and GC invariants; do not copy disabled transformations as proven optimizations. |
| **Stalin** | The [0.11 manual](https://carta.tech/man-pages/man1/stalin.1.html) exposes global flow-analysis precision, lightweight closure conversion and stack/region/heap allocation choices. It also documents safety limitations, including exact-integer-overflow checking. Historical evidence, not a modern safe baseline. | Local nonescape and representation analysis are interesting. Whole-program assumptions, long analysis and its numeric/safety tradeoffs are a poor default for a live editor VM. |
| **SBCL** | [`locall.lisp`](https://github.com/sbcl/sbcl/blob/master/src/compiler/locall.lisp) finds legal local calls and enables let conversion; [`ir1opt.lisp`](https://github.com/sbcl/sbcl/blob/master/src/compiler/ir1opt.lisp) folds/simplifies IR1. Separate constraint/environment/representation phases support type-check elimination and closure decisions. | Known-call conversion and use/type/effect information are more valuable than just another low-level peephole pass. Do not replace Scheme proper tail calls with Common Lisp policy. |
| **CMUCL** | [Compiler overview](https://cmucl.org/docs/internals/html/Compiler-Overview.html): ICR local-call analysis, once-called functions as lets, constant folding, unused-value elimination, binding substitution, type constraints, environment analysis, then VMR representation selection/copy propagation/register allocation. Phases interact. | Same useful separation between language reasoning and machine representation. This is SBCL's historical lineage, not independent evidence for twice as many ideas. |
| **Emacs Lisp** | `byte-opt.el` propagates lexical constants/aliases, folds and simplifies source/bytecode. Native `comp.el`'s LIMPLE pipeline includes forward value/type propagation, call optimization, interprocedural purity, constraints, type-check optimization and TCO before native emission. Inspected local Emacs source at `1f80e44c73d0ae33242d69da26334864109a221c`; [byte optimizer](https://github.com/emacs-mirror/emacs/blob/master/lisp/emacs-lisp/byte-opt.el), [native compiler](https://github.com/emacs-mirror/emacs/blob/master/lisp/emacs-lisp/comp.el). | Good evidence that the interactive system still needs language-level reasoning before its backend. Emacs's dynamic bindings/advice must not be mistaken for Techne's lexical semantics. |
| **Clojure** | JVM bytecode compiler with primitive locals/loops and type-directed interop; direct linking can remove Var dereferences. [Compilation](https://clojure.org/reference/compilation), [Java interop](https://clojure.org/reference/java_interop). JVM optimizations are a separate tier. | Numeric locals are largely covered by Techne's JIT. Unguarded direct linking explicitly loses redefinition visibility; Techne's guarded global specialization is the appropriate existing answer. |

Sources were inspected on 2026-10-23. Online `main`/`master` links can
change; versioned manuals and the listed local source revisions identify
what was consulted. This study does not build or benchmark those systems,
and makes no cross-implementation throughput ranking. Commercial Lisps,
every historical Scheme, tracing/meta-tracing variants and every LLVM/JVM
backend are outside this bounded survey.

## Experiment

[`kernels.scm`](kernels.scm) has **13 pairs**. Both members compute the
same bounded integer result from `n` and a runtime seed:

- immutable lexical constant propagation and branch pruning;
- unused arithmetic elimination;
- local closure inlining;
- closure hoisting, keeping a local call on both sides;
- local mutual-tail recursion converted to a loop;
- pair and mutable-vector scalar replacement;
- arithmetic CSE and vector-load CSE;
- total loop-invariant arithmetic motion;
- local rest-list elimination **together with** inlining;
- immediate-lambda and literal-folding negative controls.

The rest-list pair is **not** an isolated measurement of list allocation:
it also removes the generic call and captured environment. Likewise,
scalar replacement can expose numeric facts and eliminate checks. Those
are legitimate combined benefits, not additive independent speedups.
The mutual-tail pair has deliberately equivalent step arithmetic and a
common continuation; it is not a general algorithm for contifying any
mutually recursive group.

[`ir.rs`](../../../crates/techne-vm/examples/ir.rs) supplies the harness:

1. Boot one world and compile all definitions outside measurement.
2. Fetch/root the selected procedure and make immediate input values.
3. Warm it three times, checking results. JIT mode uses `TECHNE_JIT=1`,
   synchronous first-call compilation; interpreter mode uses `=0`.
4. Time repeated host calls into an entire Scheme loop. The host boundary
   is once per `n` iterations, not once per iteration.
5. Check the final result against an independent Rust closed-form oracle.
   No printing/parsing/compiling is in `timed_kernel`.

`ir --check` additionally checks **520 evaluations**: 13 kernels × two
variants × five lengths (including zero and odd/even lengths) × four
seeds (including negative and changing values). This validates the paired
kernels, **not** a general rewrite's correctness for arbitrary Scheme.
The measurement domain (`1 <= n <= 20000`, seed 17) keeps all arithmetic
intermediates/results in Techne fixnums. The semantic checks also use
bounded integer seeds; they do not prove numeric-tower generality.

[`run.py`](run.py) alternates before/after sample order, uses separate
warmed processes, pins each measured process to CPU 4 and clears inherited
`TECHNE_*` tuning. It records raw samples, runtime revision, binary/source
hashes, machine and tool details. Failure of a driver, an oracle, JSON
parsing or Callgrind collection fails the campaign.

Instruction counts use **Callgrind collection toggled only inside
`*timed_kernel*`**. JIT compilation and warm-up happen while collection is
off; native JIT instructions are counted. This avoids interpreting
startup/compiler work as loop performance. This is intentionally not the
whole-program cachegrind metric in `../icount.sh`.

## Measured results

Machine: AMD Ryzen 9 8945HS, Linux x86-64, CPU 4 (SMT sibling 12),
`amd-pstate-epp`, `powersave`, boost enabled. Rust 1.101.0-nightly
(`1d81eb4ad`, pinned by the project), release profile, counting-allocator
feature, Valgrind 3.27.1. Governor/boost were **not** changed.

- Instructions: 20,000 iterations × 20 calls, **two samples per variant**.
  All repeated instruction counts were identical, including both controls.
- Wall time: 20,000 iterations × 200 calls, **11 samples per variant**.
  Use medians; these are warmed loop costs, not end-to-end process times.
- Allocating variants run long enough for nursery turnover, unlike tiny
  one-shot allocation probes. No artificial collection/reset between calls.
  A separate `gc-stats` check of the pair kernel confirmed a minor collection
  during the 20-call workload, after none in its three-call warm-up.

Here `x` is **before / after** (larger means more potential); all values
are rounded. The primary signal is instruction work, not noisy wall time.

| Kernel | JIT instructions/iteration, before → after | JIT instruction ratio | Interpreter instruction ratio | JIT wall ratio | Interpreter wall ratio |
|---|---:|---:|---:|---:|---:|
| `constant-let` | 25.1 → 24.1 | 1.04x | 1.36x | 0.98x | 1.45x |
| `dead-arithmetic` | 27.1 → 19.1 | 1.42x | 1.28x | 1.58x | 1.21x |
| `local-closure` | 347.2 → 30.1 | **11.53x** | 2.07x | 16.22x | 2.01x |
| `closure-hoist` | 337.2 → 176.1 | 1.91x | 1.16x | 2.25x | 1.10x |
| `mutual-tail` | 148.1 → 19.1 | **7.76x** | 1.62x | 11.84x | 1.50x |
| `pair-scalar` | 98.2 → 24.1 | **4.07x** | 1.50x | 5.34x | 1.49x |
| `vector-scalar` | 250.2 → 29.1 | **8.60x** | 2.88x | 10.93x | 3.21x |
| `cse-arithmetic` | 41.1 → 32.1 | 1.28x | 1.18x | 1.28x | 1.92x |
| `cse-vector` | 94.1 → 80.1 | 1.17x | 1.24x | 1.08x | 1.22x |
| `licm` | 51.1 → 24.1 | 2.12x | 1.79x | 2.27x | 1.70x |
| `rest-list` (combined) | 683.2 → 24.1 | **28.35x** | 3.82x | 42.60x | 3.53x |
| `beta-control` | 24.1 → 24.1 | **1.00x** | **1.00x** | 0.99x | 1.00x |
| `fold-control` | 19.1 → 19.1 | **1.00x** | **1.00x** | 0.99x | 1.00x |

For scale, warmed JIT wall medians were 14.11 → 0.87 ns/iteration for
`local-closure`, 6.02 → 0.51 for `mutual-tail`, 3.52 → 0.66 for
`pair-scalar`, 9.03 → 0.83 for `vector-scalar`, and 28.06 → 0.66 for
`rest-list`. These extremely small loops are not application latencies.
Wall samples do contain outliers: the worst max–min spread relative to a
variant's median was about 95% (`pair-scalar` JIT); even a control had a
59% spread. CPU pinning is not machine isolation. Do not infer a small
improvement from a wall ratio near 1, or extrapolate those large kernel
ratios to the editor. In particular, lexical constants are essentially a
**JIT tie**, despite a useful interpreter reduction.

The arithmetic and invariant-motion results show that Cranelift does not
make every Scheme-level optimization redundant. But removing unknown
arithmetic indiscriminately would be wrong: error/overflow side exits
carry semantics the backend must preserve. The load-CSE gain is smaller
than the lexical/allocation opportunities and carries alias/GC proof cost.

See [`results/instructions.json`](results/instructions.json) and
[`results/wall.json`](results/wall.json) for all **104 instruction** and
**572 wall** samples, normalized from their recorded iteration/call counts.
The runtime driver and kernels' hashes in both files match the checked-in
sources; the binary hash identifies the measured executable.

## Which work is worth implementing?

### 1. Known lexical calls: highest priority

Introduce a bounded language-IR simplifier **after name resolution and
before register allocation**. Begin with fixed-arity, unassigned,
operator-only local lambda bindings with known call sites. Inline small
ones with evaluate-once argument lets; drop the now-unobservable closure.
Then handle nonescaping rest parameters when all supplied arguments are
known, before a rest list ever exists.

Use `VarId`/`FnId`, not symbol spelling or the current global value. Respect
`VarInfo::assigned`, lexical shadowing, captured mutation and argument
effects. Recompute captures/boxing/free-variable information when needed;
removing a call is not permission to retain stale capture indices.
A lambda called immediately already has this treatment; the missing case
is a lambda reached through its lexical binding.

Closure hoisting is a fallback when calls cannot be eliminated. It needs
proof that identity, captured per-iteration state and allocation lifetime
are unobserved. A source rewrite that hoists an immutable seed-capturing
closure is not permission to hoist every lambda.

### 2. Contification: high priority, separate from proper tail calls

Identify nonescaping local functions sharing a return continuation.
Convert their tail-only recursive SCC to local blocks/parallel parameter
moves. Techne already has proper tail calls; the win is removing
closure/box loads, generic calls and trampoline/frame work, not fixing
stack growth. Keep a polling/fuel check on loop back edges, and retain
source/handler/continuation behavior. Start with a restricted group,
not arbitrary recursion or handlers crossing new block boundaries.

### 3. Scalar replacement: high value, more proof work

Start with fresh small pairs/vectors or capture boxes with local,
non-identity-observing uses. Track fields as scalar IR values, including
local stores. Initially reject escape through returns, unknown calls,
stores into another object, identity/hash/weak references, or aliases the
analysis cannot resolve. Prefer **removing** allocation to introducing
Scheme stack objects with new rooting/continuation hazards.

This composes with lexical inlining: exposing a small constructor or
consumer can make an object demonstrably nonescaping. Do not double-count
the two benchmark gains as independently multiplicative.

### 4. Cheap binding simplification: useful companion

Propagate immutable scalar constants/copies and prune branches, then
reapply the existing folder. This benefits interpreter execution and
exposes opportunities to the other passes. Bound traversal/inlining work
with Techne's meter and a size/effort budget. Avoid copying mutable quoted
objects, datum-label cycles or arbitrary Scheme values as if they were
immutable scalars. Preserve the effects of discarded initializers.

### 5. CSE, dead values and LICM: stage behind effects/types

A minimal effect vocabulary should distinguish reads, writes, fresh
allocation/identity, possible raise, unknown call, collecting operation,
and control/scheduling effects. Start with local primitive CSE within a
basic block; use memory-version kills for loads. Unknown calls and alias
writes kill heap-read availability. CSE across a collecting operation
must retain rooted language values, never cached movable raw pointers.

Dead computation removal is safe only with an appropriate no-effect /
no-required-error proof. `(begin (car 0) 1)` must still raise. LICM must not
hoist a possibly raising expression out of a zero-trip loop; the measured
integer/remainder example is total on its stated domain. Numeric rewrites
must handle bignums, mixed exact/inexact arithmetic, NaNs, signed zero,
overflow side exits and error order, not just fixnum tests.

Arithmetic CSE and restricted LICM are worth prototyping. Broad global
load CSE, peeling, rotation, unrolling and bounds-check hoisting need more
representative evidence; they were not all independently measured here.
Repeated load elimination is not the same as proving an arbitrary loop's
bounds. Do not add general BBV, whole-program polyvariant CFA or a new
backend based on these measurements.

## Acceptance gates for an actual pass

These experiments establish opportunity, not implementation cost or
application coverage. Before landing a pass:

- Run bounded-domain oracles and adversarial semantic tests: mutation,
  escaped/identity-compared closures, aliased writes, wrong types, argument
  evaluation order, numeric overflow/mixed types, raising zero-trip loops,
  handlers/continuations, global redefinition, moving GC and stop/fuel.
- Exercise interpreter, synchronous/default JIT, first-call JIT and
  GC-stress modes. Preserve deoptimization/resume PCs and source spans.
- Compare `runtime/bench/icount.sh` on the existing real workloads, plus
  HTTP/editor work; a manufactured kernel is not a workload frequency.
- Count front-end work using `examples/compile.rs` / `TECHNE_COMPILE`, and
  measure compilation/code-size/memory costs. Keep boot and interactive
  load latency bounded. No optimizer implementation exists in this study,
  so **its compile-time cost, implementation effort and application-wide
  benefit have not been measured**.

## Reproduce

From the repository root:

```sh
cargo build --release -p techne-vm --features counting-allocator --example ir
TECHNE_JIT=0 target/release/examples/ir --check
TECHNE_JIT=1 target/release/examples/ir --check
TECHNE_JIT=1000 TECHNE_JIT_SYNC=1 target/release/examples/ir --check
TECHNE_JIT=0 TECHNE_GC_STRESS=1 target/release/examples/ir --check
TECHNE_JIT=1 TECHNE_GC_STRESS=1 target/release/examples/ir --check

python3 runtime/bench/ir/run.py --revision e2ffd21c \
  --samples 11 --iterations 20000 --repeats 200 \
  --out .local/ir-wall.json
nix shell nixpkgs#valgrind --command \
  python3 runtime/bench/ir/run.py --revision e2ffd21c \
    --metric instructions --samples 2 --iterations 20000 --repeats 20 \
    --out .local/ir-instructions.json
```

Use `--cases local-closure pair-scalar` and/or `--modes jit` for a short
run, and `--cpu`/`--binary` for another machine/build. Label the actual
runtime revision when testing a new optimizer. Do not overwrite baseline
results with a changed runtime and still label them `e2ffd21c`.
