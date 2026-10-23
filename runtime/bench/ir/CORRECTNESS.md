# Correctness follow-up to independent IR review

Independent Opus 5.5 and Muse Wonder Full Contributor source reviews prompted
four focused reproductions against the GC-fix runtime (`dfca0186`). All four
changed behavior with the relevant pass and with `all`, in both interpreter
and first-call JIT execution. These fixes initially kept optimization opt-in;
the [corrected rerun](CORRECTED.md) subsequently justified enabling all eight
passes by default, excluding the root boot module.

## Findings and fixes

- **Dead arithmetic:** an unused exact multiplication can exceed the VM's
  `MAX_BITS` limit. Being an integer does not prove an operation cannot raise.
  DCE now retains arithmetic, and its inductive integer analysis is removed.
- **Literal identity:** `Sexp::Int` spans all of `i64`, not just immediate
  fixnums. Propagating a boxed integer literal evaluates it again and changes
  `(eq? x x)`. Literal propagation now checks `Value::fixnum`.
  Additional regressions exposed related cases: newly folding a product into
  a shared boxed constant, copying a callee's boxed literal, and duplicating
  a loop exit literal. Optimizer-introduced folds are immediate-only; cloning
  declines unresolved heap literals, including strings and quoted objects.
  The pre-existing compiler's literal-folding policy is unchanged.
- **CSE identity:** separate arithmetic results may be freshly boxed objects.
  CSE rejects allocating subtrees, including numeric operations, rather than
  equating numerical purity with absence of observable allocation identity.
- **LICM and tasks:** immutable local bindings can reference mutable objects.
  Loop back edges admit other tasks, so invariants cannot contain pair/vector
  field reads. Replacements are confined to recurrence operands, never the
  original exit test.

A shared, exhaustive primitive-effect classification supplies the DCE,
CSE and LICM checks. It distinguishes total predicates, raising comparisons,
mutable heap reads, allocating operations and writes. This is a conservative
primitive classification, not a complete effect system or a new IR.
CSE scans effects only after its depth-bounded structural key succeeds.
LICM also caps each recurrence at the existing 80-node small-tree limit:
first-iteration parameters are `i64`s, so arithmetic preceding the first
invariant stays below `MAX_BITS` and cannot contribute a competing size-limit
exception. Larger recurrences keep their original evaluation order.

R7RS leaves numerical `eq?` unspecified. The identity regressions deliberately
preserve Techne's existing behavior rather than silently changing it under an
optimization flag.

## Regression coverage

`crates/techne-vm/tests/suites/lang/ir.scm` covers:

- the exact number-limit exception, both for an unused sequence value and an
  unused binding initializer;
- positive and negative boxed literal identity;
- positive and negative separately allocated arithmetic-result identity;
- per-invocation arithmetic allocation after immediate propagation, shared
  callee literals, and shared literals across zero/nonzero-trip loop exits;
- task-visible vector and pair mutations during numeric loops;
- a heap-dependent loop exit test that must observe another task's mutation.

The regression suite was first run against the unfixed compiler: unoptimized
interpreter/JIT passed; optimized interpreter/JIT failed. The original tests
failed on exception suppression, both identity changes, and all three task
cases. The unused-binding case was added alongside the sequence-value case.
The three additional folding/cloning identity regressions were also observed
failing before their fixes, in both optimized engines.

The normal suite runner and CI matrix exercise all passes through the default
`interp` and `jit` modes. `no-ir-interp` and `no-ir-jit` explicitly set
`TECHNE_IR=0` to preserve the unoptimized reference in both engines. These
modes run the whole Scheme suite collection, not just the new regressions.
Existing zero-trip, error-ordering, mutation, escape, logical-frame and GC
tests remain in place. The former `ir-interp`/`ir-jit` staging modes were
replaced rather than adding redundant jobs.

```sh
cargo test --release -p techne-vm --features counting-allocator \
  --test suites -- lang/ir

# Isolate each affected pass in both execution engines.
for passes in bindings cse licm dead; do
  TECHNE_IR=$passes TECHNE_TEST_MODES=interp,jit \
    cargo test --release -p techne-vm --features counting-allocator \
      --test suites -- lang/ir
done
```

## Validation results

Before the default switch, the corrected compiler passed:

- the VM/editor library and integration suites, with all passes enabled;
- the IR semantic suite with passes off and each affected pass independently,
  in both interpreter and first-call JIT modes;
- 100 differential-fuzz seeds against the unoptimized interpreter;
- the IR semantic suite with full-GC stress and heap verification in both
  optimized execution engines;
- 237 pinned Scheme/conformance/benchmark trials across `ir-interp` and
  `ir-jit`, including the documented-deviations check;
- warnings-denied Clippy and workspace formatting checks.

After enabling all passes by default, validation with `TECHNE_IR` unset also
passed the complete workspace release library/integration tests, 473 pinned
trials across `interp,jit,no-ir-interp,no-ir-jit`, 100 differential-fuzz seeds
comparing the actual default against `TECHNE_IR=0`, full-GC stress/verification
of the IR suite in all four modes, and workspace-wide warnings-denied Clippy
and formatting. A configuration regression checks that an unset selection
enables all eight passes but excludes boot, while explicit `0` still disables
them. Unoptimized CI jobs replace the former explicit opt-in jobs.

## Interpreting the measurements

The earlier instruction and cached-world campaigns predate these eligibility
fixes. In particular, arithmetic DCE/CSE gains may no longer apply. Their raw
data remains historical evidence, not an updated performance claim. The
subsequent [corrected rerun](CORRECTED.md) supplies current kernel,
steady-state, editor, GC and cached-world measurements and explains the
default-enablement decision.
