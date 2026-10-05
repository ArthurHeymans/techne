# Modern runtime implementation — phase 2

Status: **bounded integrated milestone accepted by parent**. Component and final
integration reviews returned **OK with notes**; the combined test matrix and
focused lint checks passed. This is not full runtime readiness.
Workflow `3d088a8d-723a-4854-a23f-1f9671b96819`, mission
`4ce18d72-8e4b-49b6-a7b9-6135bf77f096`, launcher
`.local/workflows/modern-runtime.js`. The workflow failed in report emission after
all writers completed, before review launch. Review-only recovery:
`28628d60-b0dd-46e0-a1fd-c4b9ed7c3c3e`, launched using
`.local/workflows/review-modern-runtime-recovery.js`. No writer was rerun.
The parent owns acceptance and integration. Async lifetime repair
`6851bd53-8e99-455a-a2e8-a9fffe5d8c7c` also passed independent parent reruns;
its repaired sources are in the integrated tree, but its continuation-based
scheduler remains separate from budgeted execution.
This is not a production-readiness claim. See [EXPERIMENTS.md](EXPERIMENTS.md)
for phase 1.

## Decision and scope

The next deliverable is an executable Steel-based vertical slice: resumable VM
execution, a real incremental collector subset, and a verified semantic IR with
an effective optimization pass. Design documents or another retention fix do not
satisfy this milestone. Keep the original `/home/arthur/src/steel` unchanged.

Initial runtime policy: one owner thread mutates each live Lisp world. Rust
futures, workers and remote services communicate through owned results/messages;
arbitrary shared-heap parallel mutation is not a requirement for this slice.
This does not make blocking Rust callbacks preemptible.

Use `gc-arena` as the first incremental collector implementation, subject to a
bounded API/version check. Its exact incremental cycle tracing, branded lifetimes
and mutation-versus-collection separation match this owner-thread policy. It is
single-threaded and non-moving: this phase does not claim moving, generational or
concurrent-mutator collection. Evaluate its actual limitations, not an invented
collector. Upstream reference inspected:
https://github.com/kyren/gc-arena (README, 2026-10-04).

Do not replace SteelVal or the native i128 JIT ABI in parallel with these changes.
Compact layout remains a subsequent choice informed by complete ownership costs.
Continue using Cranelift as the machine-code backend, not a second custom JIT.

## Shared contract, version 1

1. **Scheduling:** VM execution returns an explicit completion, failure or
   resumable budget-yield. A yield is not an error/abort/re-execution. Owned frame,
   stack, instruction position and roots persist; no RefCell/RwLock borrow or
   borrowed VM slice survives the return to Rust. Cancellation drops suspended
   state without replaying host effects. Instruction/work budgets are not
   wall-clock guarantees for native calls, tracing a large object, or allocation.
2. **Safepoints:** scheduler and collector work occurs only with all live values
   published in owned enumerable state, outside mutation/borrow guards. Every
   executing path admitted to budgeted mode must cooperate, including native
   backedges, or be rejected/fall back before execution. It is acceptable to
   disable native JIT execution specifically in the first budgeted mode.
3. **Ownership:** host handles retain their owner or are lifetime-scoped. Runtime
   identity and object generation/identity must reject stale and cross-owner
   use. Root lifetime alone does not keep a weak-handle heap alive. Handles and
   arena pointers do not become wire formats, capabilities or Send/Sync by fiat.
4. **Incremental heap subset:** introduce managed cells/vectors and scalar payloads
   through a genuine Rust/Steel allocation and mutation boundary. Internal graph
   edges must be traced, not individually pinned as permanent external roots.
   External handles root only their owned reachability. Collection happens in
   explicit increments outside arena mutation; all writes use library barriers.
   Rust-affine resources remain outside collector payloads with explicit close.
5. **Cross-heap boundary:** existing Steel values cannot be hidden inside the new
   arena without tracing both ownership domains. Initially copy supported scalars
   and use explicit owner-checked handles; reject unsupported opaque payloads.
   Cross-heap cycles are not supported by assumption. Distinguish an Engine-hosted
   new managed type from replacing Steel's existing box/vector allocator.
6. **Compiler IR:** one per-function semantic CFG, stable value/block IDs, block
   parameters or equivalent verified merges, source origins, explicit conservative
   effects and terminators. Verify dominance/use-def, edges/arity and relevant
   tail-position/safepoint invariants. Unknown host calls and mutable global loads
   are never treated as pure constants. Keep dynamic lookup/redefinition semantics.
7. **Optimization:** start with safe constant-branch pruning/unreachable-block
   elimination, optionally scalar constant propagation when errors/overflow are
   preserved. The pass must demonstrably run on executable code. Retain checked
   Steel arithmetic, evaluation order, effects and source information. Verify
   before and after transformation. Unsupported functions keep the existing
   compiler path before execution; do not fall back by replaying partial effects.
8. **Lowering:** execute the IR through existing Steel bytecode/VM; preserve the
   existing Cranelift backend downstream. No disconnected toy interpreter or
   unrelated textual optimizer presented as compiler integration. Full direct
   semantic-IR-to-Cranelift migration is a later step unless the same narrow slice
   can be wired safely without duplicating backend ownership.

Cross-contract changes require a supervisor decision before implementation.
Concrete API names may follow existing Steel conventions; report the signatures
and ownership rules needed by the integration owner.

### VM admission decision

Supervisor decision `3873113b-fcb9-497b-94bc-de2cb629989b` approves a
`ResumableRuntime` owning one Engine with shared-world tasks, not one Engine per
task. Proposed API: conversion from Engine, `start(source)`, `run_slice(task,
fuel)`/resume, and cancel. Concrete signatures remain subject to implementation
and safety review; conversion should be fallible if it needs admission checks.

Compilation/macro expansion in start is outside execution fuel. Zero fuel runs no
VM instruction. The initial subset disables new native compilation and rejects
already-native closures before invocation. Unsupported VM-context builtins,
continuations, async/custom callable paths and reentrant host behavior must be
explicitly identified. Ordinary supported Rust callbacks are atomic.

Known unsupported entries reject before execution. A dynamically encountered
unsupported callable may terminate the task with an explicit unsupported-execution
error after prior effects; those effects remain exactly once. It must never fall
back by restarting execution. This is not a claim that every unsupported program
can be rejected before any effect. Saved task state and completed owned values
must keep their roots and heap owner alive; foreign/stale task IDs are rejected.

Decision `a8c497d9-782b-44e5-8369-b716c7ae334b` permits narrowly admitting the
existing box/mutable-vector allocator builtins after auditing non-reentry and
argument rooting during allocation-triggered GC. Admission identifies the actual
callable, not a mutable Scheme binding name. Other context/control builtins remain
rejected. Allocator calls are atomic, with no wall-clock budget guarantee for large
vectors. Require alias/redefinition, heap-capture, forced-GC and rejected-control
regressions. This exception enables real heap tests; it does not broaden admission
to arbitrary context builtins.

## Base and lane ownership

Classification: **multi-seam**. These three contracts have independently testable
source boundaries. Parallel writers use distinct JJ workspaces and deliver
separately; the parent owns the later serial integration step.

The parent selected only the reviewed, independently rerun GC queue-release and
jit2 dispatch/arity fixes as prerequisites. They are combined, still experimental
and feature-gated as originally supplied, in a separate base workspace:

- `/home/arthur/src/techne-steel/modern-base`
- Change `uqokxyqt`, revision `945694d2ac903e78b3b37265afacea3be05accbd`.
- Parents `2fff3861` (GC) and `00358ed5` (JIT); conflict-free merge.
- Parent reran the union with `jit2,sync,gc-root-experiment`: all 6 GC correctness
  tests and 4 JIT parent tests passed (`proc_4b80`, exit 0). Measurement/helper
  tests retain their documented top-level ignore behavior. Log:
  `.local/evidence/runtime-modern-base/tests.log`. This verifies the prerequisite
  union, not the new phase-2 components. Original Steel remains empty at `13307cc3`.
- The completed async lifetime repair is **not** part of this frozen base.

| Lane | Exclusive workspace / new change | Ownership | Gate and handoff | Why independent |
| --- | --- | --- | --- | --- |
| VM | `/home/arthur/src/techne-steel/vm`, `opkoqtkq` | Owned suspended execution, dispatch fuel, Engine slice API and VM tests; no compiler/collector rewrite | Real CPU-loop slicing/resume, exact-once effects, cancellation, roots and fairness tests; API and supported-path report | Existing bytecode is its input; new IR/heap are not needed to test suspension |
| GC | `/home/arthur/src/techne-steel/incremental-gc`, `sulunmku` | Incremental arena module/adapter, dependency and Engine-hosted managed-type tests; no VM/compiler edits | Reclamation/cycles, incremental write barriers, stale/foreign handles, owner lifetime, real Scheme allocation fixture | Uses existing finite Engine calls and explicit host collection increments |
| IR | `/home/arthur/src/techne-steel/semantic-ir`, `tunmmvsx` | Compiler semantic IR, verifier, pass and bytecode lowering/admission; no GC or VM-dispatch changes | Optimized/unoptimized/reference Engine differential tests, pass evidence, effects/errors/redefinition/fallback | Executes through the current bytecode VM without the other new components |
| Existing async repair | `/home/arthur/src/techne-steel/async` | Completed-future tracing and result lifetime, exclusively its retained writer | Repeated await/GC/reclamation, shutdown and bounded watchdog tests | Addresses first-wave ownership blockers; integrate only after its own review |
| Integration | Parent-owned, allocated after handoffs | Reconcile the component APIs and approved async fixes; application-shaped tests | Budgeted Scheme allocation loop, host wait/cancel, GC between slices, IR on/off differential behavior | Strictly dependent on the component evidence; never concurrent writes in their workspaces |

Component writers may add their own Cargo feature/dependency/module export but
must keep such shared-file edits surgical. They do not modify each other's work,
shared ancestry/bookmarks, Techne plans or the original checkout. No pushes,
PRs or live installs. Existing first-wave changes are preserved; these are new
working changes. One writer per workspace.

## Evidence and acceptance

- Use library/VM integration tests that fail when the mechanism is removed.
- Infinite loops and deadlock-risk stress run in subprocesses with kill/reap
  deadlines. Report tested configurations and exact revisions.
- Test actual collection/reuse, not merely equal values that survive accidental
  retention. Include mutations during incremental cycles and unreachable cycles.
- Budget resumption must preserve effects, locals, source/error context and roots;
  prove two tasks make progress without Scheme-level explicit yield calls for the
  supported slice. Do not claim native-call preemption.
- IR tests must show a transformation took place and the transformed code ran.
  Check wrong types, overflow, mutable globals, source spans, effect order and
  rejected forms. A no-op verifier-only path is not the optimization milestone.
- Up to two Cargo jobs per lane, separate target directories. Avoid broad
  performance runs on the contended host. Instrument work counts, allocations,
  collections and yield counts first; timings remain observations.
- Fresh read-only cross-component review follows durable component handoffs.
  Parent validates and fixes blockers before serial integration and re-review.
- Feature-disabled behavior remains supported. Unsupported semantics and migration
  gaps are explicit. A real bounded slice is progress, not the finished runtime.

## Component handoffs and parent validation

| Component | Revision | Parent-confirmed correctness |
| --- | --- | --- |
| VM | `ec2c8d61032895952d17e88313b8dc0d364e25da` | 7 default / 8 combined-feature scenarios; 23 existing VM tests |
| Incremental heap | `500c6afa7a494b9670b0c569f4b7e4571dda9206` | 7 integration + 2 unit + 3 doctests |
| Semantic IR | `47cb5ffa5b039c617b0e55ad57519bcccd4e960c` | 6 default / 6 jit2+sync parent tests; 5 existing codegen tests |

Parent ran all nine commands sequentially with locked offline dependencies via
`.local/workflows/validate-modern-components.sh`; process `proc_f2fc` exited zero.
Logs include exact tested revisions in `.local/evidence/modern-components-parent/`.
Watchdog subprocess children account for additional nested test result lines;
counts above are parent scenarios, not inflated by those nested invocations.

Implemented outcomes, still subject to review:

- VM: explicit fuel yield from existing dispatch, owned task/frame state, shared
  globals and exact-once effects. JIT is disabled in this mode; continuation,
  handler, async and other unsupported calls reject. Root publication still scans
  live state; this is not a final low-overhead scheduler design.
- GC: pinned `gc-arena 0.7.0`, traced cells/vectors and real Scheme-facing functions.
  Barriers and unreachable-cycle reclamation are exercised. This manages new host
  types, **not** built-in Steel objects. Non-sync only, with host-scheduled debt
  credits rather than strict pause/work bounds.
- IR: symbolic lambda bytecode reconstructed into a semantic stack-SSA CFG, verified
  before/after constant-branch pruning and unreachable-block removal, then lowered
  back into existing bytecode/Cranelift paths. A real fixture prunes one branch and
  removes one block while preserving its host effect. Captures/lets/mutable locals
  and several other forms fall back; earlier AST folding policy remains unchanged.

Reports are under
`/home/arthur/.pi/agent/sessions/--home-arthur-src-techne--/subagent-artifacts/outputs/3d088a8d-723a-4854-a23f-1f9671b96819/modern-runtime/`:
`vm.md`, `gc.md`, `ir.md`.

Infrastructure failure: `emit.handoffs[0].outputPathMapping` was undefined, which
violated the emitter's JSON-only contract. The launcher now normalizes optional
receipt fields to null/empty arrays. Workspace inventories and complete component
diffs were captured under `.local/evidence/modern-runtime-recovery/` before the
same-protocol, review-only recovery. The original failed workflow receipt remains
unchanged; its three completed child runs remain resumable. No implementation
acceptance or integrated-runtime claim follows from repairing this script.

## Independent review and disposition

Reviewer `290dd803-9021-4e61-ab62-3f54c59519ed` completed source/evidence review,
without rerunning tests. Report:
`/home/arthur/.pi/agent/sessions/--home-arthur-src-techne--/subagent-artifacts/outputs/28628d60-b0dd-46e0-a1fd-c4b9ed7c3c3e/modern-runtime/review.md`.

Parent inspected the named IR lowering and VM completion code and accepts:

- **P1, fix now:** a general TailCall in the reconstructed IR is not guaranteed to
  transfer permanently: Rust/native callbacks can return to the next bytecode.
  Lowering omits the branch-exit jump, permitting execution of the unchosen branch
  in Verify as well as Optimize mode. Reproduce both branch outcomes and repair
  the return-to-epilogue behavior without changing nonreturning self-tail calls.
- **P2, fix now:** public IR operands use usize but lowering narrows to u24 without
  complete range checks. Oversized immediates/operands/relocations must return an
  error, not debug-panic or release-truncate.
- **Integration contract, fix now:** VM CompletedValues lends cloneable raw
  SteelVals, unlike the repaired async opaque OwnedValue boundary. Make extracted
  and derived result handles retain their owner and roots, with no raw escape API.
  Documentation of the raw-clone restriction is insufficient for the selected
  owned-result contract.

Fix workflow `89374e2b-ecf3-4a19-a385-06fddd2887ae`, launcher
`.local/workflows/fix-modern-runtime-review.js`, resumes only the original IR/VM
writers in their exclusive workspaces, on new changes above reviewed revisions.
Both writers completed: VM fix `3b7ebae56c8131cb60c3cf8dc2a39cab6dfe4755`
(change `rnuyrpu`), IR fix `847fa7cde2cfb51465fb6c40dd9935018812714b`
(change `xwosvsms`). Both reviewed parents are preserved. The IR callback fixture
reproduced the wrong branch/effects before repair; raw VM value escape likewise
reproduced before the opaque handle change.

Targeted reviewer `b47fae43-b956-4e25-a208-038860c83dc4` closed all three findings
with **Integration verdict: OK with notes**, approving parent-owned serial
integration, not combined-runtime adoption. Its `modern-runtime/fix-review.md`
report is under the fix workflow's managed output directory.
Because its diff tool lacks a usable HEAD baseline, the parent supplied exact
child-to-reviewed-parent JJ patches and full revision inventories under
`.local/evidence/modern-runtime-fixes/` (supervisor request
`321ae4cc-171e-4afe-aac7-dd08bdc05381`). Parent reruns, including release-mode
operand bounds and ownership compile-fail tests, **passed** as `proc_dd53` via
`.local/workflows/validate-modern-fixes.sh`: VM 10 default / 11 combined scenarios,
5 doctests and 23 existing tests in each configuration; IR 10 parent tests in each
mode, 5 codegen tests and 2 release-boundary tests. IR subprocess entry points
remain top-level ignored and are executed by their watchdog parents. All ten
commands exited zero; logs are in `.local/evidence/modern-fixes-parent/`.

GC and async patches are unchanged; no broad redesign or automatic integration
is authorized by this fix workflow.

Integration remains a separate parent-owned gate. Install managed bindings before
consuming Engine into the resumable runtime; step the arena only after slices
return. Add combined tiny-budget allocation/cycle/reclamation and IR-transformation
fixtures, including the callback-tail regression in budgeted execution. Keep
non-sync managed-GC tests separate from sync VM/IR/async configurations.
The existing continuation-based async executor is **not** schedulable inside the
new budgeted subset, which rejects continuations/async callables. Its tracing fix
is compatible; a real budgeted wait/resume boundary still requires implementation.

## Parent-owned integration

Workspace `/home/arthur/src/techne-steel/integrated`, new JJ change `nlqzsplr`.
Parents: reviewed VM `3b7ebae5`, IR `847fa7cd`, incremental heap `500c6afa`,
and repaired async `9fa6d406`. Original Steel and component workspaces remain
unchanged. The only merge conflict was Cargo.toml: the parent retained one copy
of the identical `gc-root-experiment` feature plus the new `incremental-gc`
feature. The queue-release source block merged without duplication.

New integration tests in `crates/steel-core/tests/modern_runtime.rs` exercise:

- Disabled/Verify/Optimize code in budgeted execution, both callback-tail branches,
  indirect calls, box/vector results, exact effects/errors and mutable globals.
- Real IR pruning counters, then two interleaved managed-allocation loops with
  tiny/zero fuel, existing Steel collection and arena increments between slices.
- Cyclic-graph reclamation, cancellation with suspended graph roots, stale task
  IDs, and opaque derived results retaining managed objects after runtime drop.

All scenarios have subprocess kill/reap deadlines. Initial harness checks exposed
two test-wiring mistakes: a crate-private error accessor and a nonexistent source
path passed to a canonicalizing API. Both were corrected without runtime changes;
error comparison now uses public Display/kind and source-span coordinates. The
managed GC/VM/IR scenario passed on that intermediate run.

Process `proc_95eb` **passed** the corrected combined gate and all 17 commands in
`.local/workflows/validate-modern-integrated.sh`. Exact tested source revision:
`1327aea4f155cab1d025dd40ba5f7b855785ad89`. Current integrated revision
`7f25d97e46c2d4134f52d37065d6c5319393f72b` differs only by usage documentation,
verified with a JJ diff. Original Steel remains empty at `13307cc3`.

| Integrated configuration | Parent-confirmed result |
| --- | --- |
| Non-sync incremental heap + VM + IR | 2 combined scenarios; VM 10; IR 10; legacy GC 5; existing VM 23 |
| Incremental heap component | 7 bridge tests, 2 barrier tests, 3 doctests |
| Opaque VM completion boundary | 5 compile-fail doctests in each configuration |
| Sync + jit2 + rooted instructions + GC queue fix | 1 combined budgeted IR scenario; VM 11; IR 10; JIT 4; GC 6; parallel marker exact-count 1 |
| Separate continuation-based async adapter in the merged tree | 21 default / 22 sync integration tests, plus 2 compile-fail tests each |

Ignored top-level entries are documented measurement tests or subprocess entry
points invoked by their watchdog parents. No new timing/performance claim follows
from these correctness runs. LSP was inconclusive; Cargo and formatting checks
supply the validation evidence. Logs are under `.local/evidence/modern-integrated/`.
Final fresh-context integration-only reviewer
`ca4b4738-5081-46ae-aa38-fd2a3148093b` returned **OK with notes**, finding no
integration-caused runtime defect. Report:
`/home/arthur/.pi/agent/sessions/--home-arthur-src-techne--/subagent-artifacts/outputs/ca4b4738-5081-46ae-aa38-fd2a3148093b/modern-runtime/integration-review.md`.
The parent corrected its documentation P2: Mode variants are alternatives, not
bitflags. Documentation now also states that the managed cancellation fixture
stops before its named loop; separate VM tests cover cancellation after CPU-loop
backedges. Only documentation changed after the reviewed/tested Rust source.
Review patches/inventory are in the `review/` subdirectory. Focused integration-test clippy **passed** in both configurations
(`proc_afa1`, exit 0), with only the previously established `clippy::never_loop`
baseline lint allowed. No diagnostic referenced the new integration test. Offline
metadata encountered an uncached target dependency; an explicit platform-scoped
`cargo fetch --locked` succeeded without changing the lockfile. Usage and supported
boundaries are documented in the integrated checkout's
`experiments/modern-runtime.md`.

### Acceptance boundary and remaining engineering

Accepted: real budgeted Steel execution, executable verified IR optimization, and
incremental managed-object collection working together under the tested owner-thread
contract. All source changes remain isolated; no push, PR or live installation.

The follow-on work in [READINESS.md](READINESS.md) now implements task-aware
host waits, persistent root publication, actual built-in atomic-mark/incremental
sweep, scalar-native safepoints, expanded IR and owned descriptor/native-transfer
lifetimes. Independent component reviews closed the identified blockers. Final
combined source `c7b804a7d26f87eaf8a54d72db8693d0b97cca26` is undergoing parent
reruns; controlled performance qualification remains pending. This does not enlarge
the historical acceptance above or certify incremental marking/general heap-native
execution. No application development has been substituted for the runtime gate.

The remaining engineering identified at this first milestone was:

- A rooted host-wait/wakeup/cancellation protocol integrated with budgeted tasks,
  rather than the separate continuation-based executor.
- Migrating actual built-in heap objects and closure/capture ownership to the new
  collector, or evidence for a different collector strategy. Host managed types
  alone do not modernize the entire Steel heap.
- Cheaper persistent task root/state publication, native scheduling safepoints and
  precise roots/reconstruction metadata, broader semantic IR admission and explicit
  code-generation/redefinition lifetime rules.
- Controlled latency/allocation/throughput measurements, resource bounds and the
  remaining remote/runtime application-shaped probes. Current tests are correctness
  evidence, not state-of-the-art performance or production readiness.

