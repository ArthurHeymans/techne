# Steel runtime-readiness experiments

> **History.** This documents the Steel modernization, which techne-vm replaced
> below the parser; see [TECHNE-VM.md](TECHNE-VM.md) for the current runtime.

Status: first experimental wave and cross-lane review complete. Combined adoption
is **blocked**; async lifetime repairs are implemented and under parent validation.
Parent correctness reruns for GC, JIT and values passed. The narrow GC/JIT union
also passed in the isolated phase-2 base; no combined modern-runtime readiness
claim. See [MODERN-RUNTIME.md](MODERN-RUNTIME.md) for the active implementation.

Workflow: `72929e67-ffe9-41a1-b8c3-58eff8ba76b6`.
Mission: `4ce18d72-8e4b-49b6-a7b9-6135bf77f096`.
Launch script: `.local/workflows/runtime-experiments.js`.
The workflow runs four isolated writers, followed by a read-only cross-lane
review if all four finish successfully. Completion wakes the parent for synthesis
and integration decisions; launch acceptance is not evidence of passing tests.

The user explicitly requested parallel implementation experiments for safe
concurrency, efficient value representation, modern GC and modern JIT, preserving
excellent Rust embedding. A follow-up explicitly adds investment in a compiler IR
and optimization pipeline. These are runtime acceptance requirements, not merely
optional future optimizations. They must be evaluated together before adoption.

The compiler/IR addition belongs to the existing JIT writer, avoiding overlapping
compiler owners. Steering request `98a306c7-7ed1-4ba4-99df-f6d7eba56188` was queued
for child `2a312436-e67d-4d4a-a87b-c33f73f442ce`; its eventual report must establish
what was actually implemented. The original launch script records the initial
brief, not this later scope update.

## Source and isolation

Source checkout: `/home/arthur/src/steel`, clean at the initial inspection.
Experiment baseline: `dec633b9` (`master`, Steel workspace version 0.8.2).
The existing checkout's working change is `wuyyzzor`, initially empty.

Experiments use separate Jujutsu workspaces created with new working changes.
No child may mutate the original checkout, sibling workspace, shared bookmarks,
or Techne's plan files. The parent owns integration and product documents.
No pushes, PRs, installs into the live environment, or replacement of system Steel.

## Implementation topology and lane board

Classification: multi-seam. Four contracts are independently testable, but their
ABI, roots and safepoints must converge. Isolation permits alternatives; it does
not imply their patches can be merged without design review.

| Lane | Exclusive workspace | Owned decision/contract | Initial change | Gate | Handoff |
| --- | --- | --- | --- | --- | --- |
| async | `/home/arthur/src/techne-steel/async` | Suspend/resume, wake-driven execution, cancellation and safe concurrency boundaries | `zzurxnuq` | Actual Steel/Rust async embedding and scheduler correctness tests | Changes, tests, baseline comparison, limitations and next step |
| values | `/home/arthur/src/techne-steel/values` | Compact value layout and safe host/heap identity boundary | `momrqwun` | Representation correctness/property tests, layout and boundary measurements | Changes, tested value subset, Rust safety/GC/JIT implications |
| gc | `/home/arthur/src/techne-steel/gc` | Collector strategy, rooting and Rust resource ownership | `uxwpkrtq` | Root/cycle/lifetime stress tests and measured collection behavior | Changes, pause/allocation evidence, migration constraints |
| jit | `/home/arthur/src/techne-steel/jit` | Compiler IR/passes, existing Cranelift path, interpreter equivalence and runtime safepoints | `ltzkwnwm` | Verified IR subset/pass where feasible, differential tests, phase/cold/hot timing and host-call safety | Changes, actual compile path, supported subset, IR migration contract and integration hazards |

One writer per workspace. A fresh read-only cross-lane review follows the four
handoffs. Parent synthesis selects compatible changes; integration is a separate,
serial step after that review, never automatic cherry-picking of all experiments.

## Shared constraints

- Inspect current facilities first: Steel already has async host functions,
  continuations, native threads/interruption, `jit`/`jit2`, and several reference
  counting/collection configuration options. Do not reinvent them by assumption.
- Investigate suitable existing Rust crates before building a new mechanism.
  Existing Cranelift support is the JIT starting point, not a new code generator.
- Steel already has AST optimization passes and Cranelift IR; establish whether
  its intermediate language-level contracts are sufficient instead of claiming
  no compiler pipeline exists. Investigate a minimal verified CFG/SSA-style IR
  preserving effects, source identity, tail calls/continuations and live globals.
  Unknown Rust calls are effectful by default. Prefer one real lowering/pass slice
  over a disconnected demonstration optimizer; full migration remains explicit.
- Rust must not acquire unsound `Send`/`Sync`, dangling borrows, or unrooted values
  to make benchmarks pass. No borrowed VM state across suspension/collection.
- Define host handles, roots, safepoints and code generations explicitly. Moving
  objects, compact tags and generated code cannot independently choose incompatible
  representations and then claim a working combined runtime.
- Scheduler fairness, cancellation and GC coordination must cover compiled loops
  as well as interpreted code. Blocking Rust calls need a documented boundary.
- Preserve interpreter semantics for supported operations; reject/fall back for
  unsupported ones. Live redefinition cannot silently keep using invalid assumptions.
- Experiments can be feature-gated. A standalone model is evidence for a specific
  mechanism, not a claim of integration into Steel. Prefer one real integration
  seam over a broad disconnected toy VM.
- Benchmarks distinguish cold compilation, warm execution, GC pauses, allocations,
  host-call costs and scheduler latency. Record build flags, versions and workloads.
  Parallel-wave timing is preliminary; accepted performance comparisons require
  serial reruns under comparable conditions.
- Run focused tests, formatting and clippy for changed targets when feasible;
  report pre-existing failures and environmental blockers separately. New standalone
  Rust packages use edition 2024 and resolver 3; do not churn Steel's existing edition.
- Limit each concurrent lane to two Cargo jobs and separate target directories to
  avoid turning the user's desktop into a benchmark casualty.

## Intermediate evidence and decisions

### Async handoff

Child `7668b7e2-d65c-4787-b4b1-041c7e9f4136` completed its experiment in
`experiments/async-runtime/` of the async workspace, revision
`b0ab4e3cb6f623832436066281e434c9512e0278`. The parent read the report and core
adapter/protocol. Independent review subsequently identified the lifetime gaps
below; parent reruns of the repaired adapter remain pending.

Reported evidence: 15 default tests and 16 sync-feature tests passed, with focused
formatting/clippy checks. The adapter uses real Steel continuations, explicit roots
and wake-driven Rust futures. It supports cooperative suspension and cancellation,
not resumable instruction budgets or proven shared-heap parallel execution. Active
task redefinition and structured recovery into suspended Lisp remain gaps.

Timings were recorded under severe host contention (reported load average about
80 on 16 logical CPUs). They demonstrate workload execution, not reliable latency
budgets or comparative speed conclusions. No additional performance runs should be
used for adoption decisions until contention is controlled.

Managed report:
`/home/arthur/.pi/agent/sessions/--home-arthur-src-techne--/subagent-artifacts/outputs/72929e67-ffe9-41a1-b8c3-58eff8ba76b6/steel-runtime/async.md`.

### JIT error-comparison decision

Supervisor request `9bc97032-1acf-466f-8fd3-9ac2c81c6715` was answered: the bounded
safepoint experiment may compare structured error kinds rather than exact message
text, provided the corresponding failing operation, available source context and
observable effects are checked. Record both diagnostics and establish whether
the divergence predates the patch. Do not hide the mismatch, claim full diagnostic
equivalence, or widen this fix into unrelated error-message normalization.

## First-wave deliverable

Each lane delivers a bounded executable experiment and an evidence-backed next
step, not a declaration that a production language runtime was completed in one
pass. Review must distinguish implemented behavior, tested behavior, prototypes,
and unresolved risks. Full readiness requires a subsequently integrated Rust
embedding testbed and application-shaped workloads.

Managed report bindings: `steel-runtime/async.md`, `steel-runtime/values.md`,
`steel-runtime/gc.md`, `steel-runtime/jit.md`, and
`steel-runtime/cross-lane-review.md`. These are artifact-relative bindings, not
files in this repository. Actual paths and retained child IDs are returned by the
workflow and are recorded below.

## First-wave synthesis and review disposition

All five children completed. Workflow state:
`experiments-reviewed-awaiting-parent-integration`. The independent reviewer read
reports, source and selected logs but **reran no tests**. Its verdict is
**BLOCK for combined adoption**, with selective narrow integration candidates.
The parent verified the reported workspace revisions, inspected the GC/JIT core
diffs, and confirmed the original Steel workspace remains empty at `13307cc3`.

| Lane | Final first-wave revision | Implemented outcome | Disposition |
| --- | --- | --- | --- |
| async | `b0ab4e3cb6f623832436066281e434c9512e0278` | Real continuations and wake-driven futures, with cooperative task boundaries | Repair lifetime bugs before adopting an owned embedding API |
| values | `a8cfb1da29b1b58bd33b325fd69151848fcea603` | Feature-gated 8-byte NaN/handle comparisons against 16-byte SteelVal | Research tooling only; no replacement ABI selected |
| gc | `2fff3861f3c81015da54a4e3ddef2578e5ef018a` | Release temporary sync-marker roots after worker acknowledgments | Narrow correctness fix suitable for subsequent integration review |
| jit | `00358ed5ef0d921087fe2055f9e11b4ef4f9e5e6` | Native module self-tail calls return to dispatch; unsupported arities fall back before symbol declaration | Narrow experimental jit2 fix; not approval to enable JIT generally |

The value experiment preserves NaN payloads and full-width integers through a
rooted fallback. Smaller words are not automatically smaller total storage:
all-rooted handles require a root/table entry even for each integer. The current
16-byte SteelVal and jit2's i128 encoding remain unchanged.

The GC result repairs a real retention bug, **not** latency or collector
architecture. No moving, generational, incremental or concurrent-mutator collector
was implemented. `gc-arena` is a candidate for a future owner-thread incremental
subset; MMTk needs a substantially larger precise-root/barrier/slot binding.
Neither was integrated or benchmarked.

The JIT result fixes a reachable unchecked native backedge, **not** resumable
instruction-budget scheduling. Steel already has AST optimization, bytecode
peepholes and Cranelift IR. A verified semantic CFG/SSA migration is documented in
`jit/experiments/jit-runtime.md` but remains **unimplemented**, so the requested IR
and optimization-pipeline milestone is still open.

### Review findings and corrective handoff

1. **P1: completion owner lifetime.** A public RootedSteelVal does not keep the
   Engine/weak-handle heap alive. Require an owner-retaining/scoped result API,
   without a public extraction path that silently drops its owner. Test a mutable
   vector result across Runtime shutdown. The values RootScope has the same
   underlying runtime-outlives-roots restriction and is not a production handle.
2. **P1: shared completed futures.** FutureResult caches a bare SteelVal while
   collection skips FutureV. Repeated await after root release and slot reuse can
   expose a reclaimed result. The original finding is source-derived, not an
   executed failure. Require forced-reclamation regressions and trace/retain cached
   results for the correct lifetime; avoid permanent roots that leak cycles.
3. **P2: unbounded interruption test.** An interrupt request is not a watchdog if
   dispatch stops checking it. Use a parent subprocess kill/reap deadline.

Retained async writer resumed as
`6851bd53-8e99-455a-a2e8-a9fffe5d8c7c`, exclusively owning the async workspace,
with instructions to start a new jj change above the completed experiment.
Its scope is these three findings, not a collector/representation rewrite.

Supervisor decision `4e51e237-f7ef-4974-8721-55f3c69c4fa6`: permit the async fix
to mirror the **same feature-gated post-ack queue clear** from the GC lane as an
explicit dependency. Otherwise baseline sync retention could mask the new
lifetime tests. Trace cached completed futures only from reachable FutureV
values, with per-pass identity handling; require cycle reclamation and state the
owner-thread polling/collection contract. This is approval to implement and test
a proposal, not acceptance of its eventual safety. No sibling workspace or
original checkout is modified by that child.

Additional open boundary: non-sync bare RootToken is currently auto-Send despite
its TLS-backed Drop behavior. Value-bearing roots and owner-local adapter state
do not remove this core type hazard; locality needs enforcement before a general
host API is exposed.

### Async repair handoff

Follow-up `6851bd53-8e99-455a-a2e8-a9fffe5d8c7c` completed at new change
`nosxytuy`, revision `9fa6d406e93f973aec4027c101633707a31f8cbf`, preserving the
original `b0ab4e3c` experiment. The configured `async.md` report now contains this
follow-up, superseding its original report text.

Implemented: opaque owner-retaining `OwnedValue` outputs; cached-result tracing
only through reachable FutureV graph edges with visited identities; the approved
sync queue-release dependency; subprocess kill/reap interruption tests. The
parent read the core/adapter diff and lifetime tests. The five regressions check
owner shutdown, repeated await, a future/result cycle, cancellation with another
consumer, and cancellation without another consumer. Drop counters **and actual
slot reuse** check reclamation, not only survival.

Worker evidence: 21 default / 22 sync integration tests plus 2 compile-fail tests
per configuration passed; red/green lifetime controls and sync-retention negative
controls were exercised. Parent independently reran both configurations:
**21 default / 22 sync integration tests and 2 compile-fail tests per configuration
passed**, including all five lifetime regressions in each. Process `proc_c08f`
exited zero; logs: `.local/evidence/runtime-async-repair/tests.log`. The ignored
subprocess helper is exercised by its parent tests, not omitted from coverage.
The supplementary root-workspace marker test timed out during compilation, not
test execution; it remains unverified. Owner-thread polling outside collection
is still a trusted host contract, not arbitrary concurrent-future safety.
Nothing from the async follow-up is yet in the phase-2 base.

### Parent validation and retained evidence

Parent correctness reruns **passed**, executed sequentially via
`.local/workflows/validate-runtime-wave1.sh`, with two Cargo jobs, per-workspace
target directories, CPU affinity 0–1, offline locked dependencies, and deadlines.
All eight commands exited zero at the first-wave revisions listed above:

| Configuration | Parent-confirmed result |
| --- | --- |
| GC default / sync / sync+biased, fix enabled | 5 / 6 / 6 correctness tests passed; measurement ignored in each |
| jit2+sync | 4 parent tests passed; subprocess helper ignored at top level but invoked by the parent tests |
| Values default / sync | 10 unit/property tests passed in each |
| Values default / sync doctests | 2 compile-fail tests passed in each |

Async repair reruns are tracked separately above. First-wave logs are in
`.local/evidence/runtime-wave1-parent/`; process `proc_af28` completed successfully.
No combined runtime, full workspace suite, or fresh benchmark matrix was run.

These are correctness reruns, not controlled performance trials. The host still
reported load near 59 on 16 logical CPUs at parent inspection; timing-based
adoption claims remain deferred. No matrix of separate lanes substitutes for an
integrated compiled-loop/forced-GC/async-cancellation test.

Durable report directory:
`/home/arthur/.pi/agent/sessions/--home-arthur-src-techne--/subagent-artifacts/outputs/72929e67-ffe9-41a1-b8c3-58eff8ba76b6/steel-runtime/`.

- `async.md`: originally child `7668b7e2-d65c-4787-b4b1-041c7e9f4136`;
  now superseded by follow-up `6851bd53-8e99-455a-a2e8-a9fffe5d8c7c`.
- `values.md`: child `45348a73-4023-4039-b784-b118a90f2ea8`.
- `gc.md`: child `d2fc8aff-9dad-4ead-9b1a-c8249aabf866`.
- `jit.md`: child `2a312436-e67d-4d4a-a87b-c33f73f442ce`.
- `cross-lane-review.md`: child `dfbe3f31-4fea-4196-aa8b-b299807cef01`.

Next decision after repair/validation: select a serial integration slice in a
separate workspace, keeping current SteelVal ABI and owner-thread VM mutation.
Then test a real incremental collector subset and an executable verified semantic
IR/pass slice against the agreed handle/safepoint contract. Those larger runtime
requirements are not satisfied by first-wave bug fixes or design documents.

