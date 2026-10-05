# Runtime readiness implementation

> **History.** This documents the Steel modernization, which techne-vm replaced
> below the parser; see [TECHNE-VM.md](TECHNE-VM.md) for the current runtime.

Status: independent component re-review closed all five identified transfer
findings, including handler attachments and pre-return teardown. Collector/frame
and unwind reviews pass with notes. Final combined-source validation is running;
performance qualification and parent acceptance remain outstanding. This is not
whole-runtime acceptance.

**Final assembled candidate: `c7b804a7d26f87eaf8a54d72db8693d0b97cca26`**,
`ukppryvq`, merge of passed4a2bdd7c and final lifetime repair
**`f80290525bb587250bf984b7ea3e0e108b3190d5`**. Parent verified four file hashes,
component patch hash `430a3513…` and byte-identical integrated delta from4a2bdd7c;
merge had no conflicts and touched no VM/collector/resumable production seam.
Inventory/patches: `.local/evidence/readiness-parent/lifetime-final/`.

Reviewer **`929cd529-2028-4ef0-9fad-ed21cde57fad`** returned **OK with notes**,
no findings, for frozenf8029052 relativea294. Ordinary attachment-only handler
collection and single-unwind cleanup are exercised, with fresh live rescue roots
protecting caller values even after an obsolete root Vec unwinds. Published owners
remain intact; unreturned cycles expire. Original three transfer fixes preserved.
Ordinary VM unwind is NOT a safe resume/poison boundary; resumable deserialize
admission remains rejected, and supported host callback poison is tested separately.
Worker reports42 focused/502 default/502 sync plus downstream/doc/lint/format gates;
not reviewer/parent reruns.

**`proc_c7f9`** runs the full three-configuration parent library/API/downstream/
doc/lint/format matrix and both qualification/smoke configurations at unchangedc7b8.
Driver `.local/workflows/validate-readiness-lifetime-final.sh`; evidence
`.local/evidence/readiness-parent/lifetime-final/`. **Passed**, exit0 at unchanged
c7b804a7: **525 default/556 combined/546 sync** full-library cases (existing5/6/6
ignored), all API/downstream/custom-hash gates,24 default/22 sync doctests, both
lint modes, formatting and both qualification/smoke configurations (four workloads/
fifteen protocol checks each), plus three supervision negative controls. No timing
or whole-runtime acceptance claim.

**`proc_a190`** builds locked/offline release binaries for both consumer feature
configurations, preserves separate executables/hashes, and reruns raw smoke.
Driver `.local/workflows/build-readiness-qualified-release.sh`; evidence
`.local/evidence/readiness-parent/qualified-release/`. **Passed**, exit0 at unchanged
c7b804a7: both release builds and raw smoke. Release request-cap checks **`proc_eb54`**
also passed both configurations with valid/duplicate IDs and4,096-frame limits;
separate binary hashes bound to each report. This is preparation for measurements,
not comparative qualification. All worker/reviewer workflows ended. Original Steel
rechecked unchanged at13307cc3. Consolidated supported gate/boundary record:
[QUALIFICATION.md](QUALIFICATION.md).
Host busy loops are confirmed pinned toCPU0 (sibling8); potential timingCPU4 has
sibling12 and the same powersave/amd-pstate-epp policy. Affinity isolation can avoid
those loops but is not global host quiescence or an OS-exclusive core lease. Parent
requested approval for CPU4-scoped comparative measurements with explicit shared
host limitations; no quiet-host acknowledgement or campaign run yet. Temporary
analyzer mitigation remains until the measurement gate is resolved.

**Preserved preceding combined source: `4a2bdd7ce1cdc2ca1941f0e374ecee61fe66bb8b`**,
`oyollllz`, merge of tested parent `e0b966486f42a45b81bf1512bdb406b68294243e`
and transfer correction **`a2947076115882a14daa675015a1b8d5807f1404`**. Parent
verified six file hashes and byte-identical component/merged delta (`b718e16a…`),
including actual native caller snapshot/root protection. No conflicts or additional
unreported edits. Exact provenance: `.local/evidence/readiness-parent/transfer-final/`
`integration-inventory.json`, `integration.patch`, `source-verification.json`.

Correction workflow `ebc6070b` stopped during extension reload AFTER delivering
the writer; review child `199a56e6-abaf-4567-80c3-5897e48dd4b9` stopped and its
status explicitly reports non-resumable. New same-protocol review workflow
**`2252276a-ea69-474e-9a12-bbfaa5d4e49a`** uses a labeled fresh-context transfer
reviewer (`5b7ba713-91f6-445a-b583-40e1991c6e6e`) and retained lifetime reviewer
(`d772f5e9-b176-433a-b152-e79f8cec1aec`). No execution/provider bypass.

**`proc_dc63`** independently runs all three full library configurations, public
API/downstream tests, doctests, lint/format and both final qualification/smoke
configurations. Driver `.local/workflows/validate-readiness-transfer-final.sh`,
logs `.local/evidence/readiness-parent/transfer-final/`. Parent library reruns
passed **521 default/552 combined/542 sync**, API/downstream/custom-hash gates,
24 default/22 sync doctests, both lint configurations and formatting. The driver
then **failed** qualification-default with exit101: local node's first reply timed
out under the existing10-second deadline. Exact source remained4a2bdd7c; failed
log and all preceding passes are preserved, not an overall pass. Worker final
source separately reports498 default/498 sync,38 focused, downstream capability
tests and compile-fail privacy controls; not parent/reviewer reruns.

Host diagnostic observed load82–98 and58 busy shell loops outside these workspaces
(no cleanup/killing attempted). Bounded startup diagnostic **`proc_f4fb`** then
passed twice on the unchanged current-default binary and twice on the frozen
qualification-modern binary, each with the original10-second deadline and
shutdown/reap. Logs include wall/child CPU observations, not comparative performance
acceptance or proof of the original timeout's cause. No fixture/source/deadline
change. Evidence `transfer-final/node-startup-diagnostic.json` and
`qualification-timeout-host.json`.

**`proc_8143`** reruns both qualification configurations plus raw smoke, supervision,
lint/format on the same source, keeping logs separately in
`transfer-final/qualification-retry-1/`. Driver
`.local/workflows/retry-readiness-final-qualification.sh`. **Passed**, exit0 at
unchanged4a2bdd7c: both test/smoke configurations (four workloads/fifteen protocol
checks each), three supervisor controls, lint and formatting. No deadline/source
change; original timeout remains preserved. Request-cap rerun **`proc_120f`** also
passed both4,096-frame cases (valid and duplicate IDs), request-limit event and
exit/reap, with the modern binary hash recorded in `qualification-retry-1/request-cap.json`.
Final handler/teardown corrections and their re-review remain independently required.

Final collector reviewer **`d772f5e9-b176-433a-b152-e79f8cec1aec`** returned
**OK with notes**; collector-unwind evidence note is closed. Ordinary native
caller-root handler attachments remain a source concern for the transfer reviewer,
not a reproduced failure. Transfer reviewer is also auditing commit-before-teardown:
a recoverable destructor unwind before final VM publication must not exempt
unescaped fresh cycles from cleanup. Rooting is not publication; previously exposed
owners remain protected. Double panic/process abort is not promised recoverable.
Final transfer review `5b7ba713-91f6-445a-b583-40e1991c6e6e` confirmed both as
P1 source-backed blockers, while closing repeated-export/global-write/ordinary
error-cleanup findings. No reviewer test reruns. Follow-up workflow
**`708a3761-440c-4514-bf55-291d8895a678`**, mission
`ead6d6e4-2a60-4ae1-9ed2-cca8a91e4012`, resumes isolated writer then reviewer.
Writer `cf1dedc4-6b2c-4cb4-8cdc-cfa336337385` received approval for an armed
outer finalization guard and weak fresh-owner inventory. Drop serializer, borrowed
caches, consumed wire and construction roots before caller/publication roots;
keep prospective result independently rooted, not classified as escaped. Disarm
only after these potentially panicking phases succeed. Weak inventory must not
postpone unused acyclic payload destruction until after commit. Audit protection
remaining if root-group release itself panics; no unsafe lifetime/handle shortcut
or recoverable second-panic guarantee. Narrow source/tests stay worker-owned.

Preliminary measurement-host observation recorded load about59 on16 affinity
CPUs during the one-job validation build, with many unrelated busy shell processes.
Evidence `transfer-final/measurement-host-preflight.json`. No other processes were
stopped; no quiet-host acknowledgement or comparative measurements made. Recheck
when builds finish; inability to allocate a quiet host remains a timing limitation.

Latest combined candidate: **`f74d28e75d06a9c0c870012faea51a428befce52`**,
`pzsrnowl`, merge of passed `789725c9` with descriptor correction
**`821c3af0ba0d8669213bd961765e26ee090d2f83`**. Parent verified all thirteen
component file hashes and exact patch (`5ddc21a8…`); merge had no conflicts and
retained unconditional frame-contract enumeration. Evidence and full parent
matrix: `.local/evidence/readiness-parent/descriptor-merge/`,
`.local/workflows/validate-readiness-descriptor-merge.sh`, **`proc_716a`**. Parent
library reruns passed **510 default / 541 combined / 531 sync** (existing ignored
counts 5/6/6); all API/custom-hash gates, default22/sync20 doctests and both lint
configurations passed. Final formatting failed on inherited module declaration
ordering in `src/lib.rs`; the complete driver therefore exited1, not success.
Source remained frozen at f74d28e7; logs and failure remain preserved.
Worker final matrix separately reports 487 default/487 sync, 27 focused and24
sweep cases; these are not parent reruns.

Parent follow-up `znznszrr`, candidate **`e0b966486f42a45b81bf1512bdb406b68294243e`**,
normalizes that module ordering and adds two watchdog-isolated unwind scenarios:
armed Custom tracing during collection/mark start, and dead-payload destruction
during sweep. Each checks the expected panic, poisoned execution/collection/native
controls, unchanged future polls, and cancellation releasing the waiting future
without clearing poison. No production behavior change. **`proc_2a46`** runs the
three API configurations, targeted lint and all changed-core-file formatting;
driver `.local/workflows/validate-readiness-collector-unwind.sh`, evidence
`.local/evidence/readiness-parent/collector-unwind/`. **Passed**, exit0, source
unchanged: default2/combined4/sync2 scenarios, targeted lint and full changed-core
formatting. Both expected panic paths were caught and cancellation remained usable.

Independent review workflow **`070b88a0-b7f7-41f8-9e86-9b816f507d34`** completed:
- Lifetime reviewer `4c2ceca9-e16d-4acd-9060-f043f695f8b9`: **OK with notes**;
  prior collector/frame findings closed. Parent will add explicit collector
  tracing/destructor unwind and poisoned-runtime rejection/cancellation checks.
- Transfer reviewer `6a1391ae-093c-4dae-9b8c-2444ff6ca805`: **BLOCK**. Cached
  metadata closure exports omit fresh constants/global dependencies; global
  `SET` is absent from discovery/remapping; failures after successful metadata
  initialization bypass fresh-cycle cleanup. The last requirement includes later
  globals/result errors, but must preserve pre-existing/builtin/published owners.

Correction/re-review workflow **`ebc6070b-2a93-47be-8935-ea5025918ee9`**, mission
`b8959516-3068-4f2f-a73f-d3771f953f3e`, resumes the isolated heap writer then the
retained transfer reviewer. Writer owns transfer sources/tests in `next-heap`;
parent owns panic integration tests in `readiness-integrated`. Retained writer
is `a2d4cb72-827b-459a-93da-01929800ed59`. Approved compatibility direction:
restrict native `HeapSerializer` capabilities (no unrestricted thread/cache
access), provide checked/root-safe operations and tracked exposure, and journal
fresh owners through the entire import. Preserve transitive dependencies of
published/exposed values; detach only unescaped fresh owners on failure. Audit
ambient lookup/returned handles and later-filled exposed placeholders. Merely
collecting then returning an error must not count as escaping every owner.
External API/migration tests and published-owner controls are required. Approved
implementation uses a cycle-safe snapshot of transfer-local wire dependencies
before consumption, not a new runtime/RC graph collector. Publication/exposure
roots include constants, globals, native tracked conversions and the final result;
opaque callbacks must obtain fresh aliases through tracked conversion. Preserve
transitive dependencies even when placeholders fill later. Conservative retention
is allowed only for genuinely exposed dependency closures, not all callback use.
No public raw heap/thread handles; the existing internal teardown fixture remains
separate. This is not general RC cycle collection or whole-engine rollback.
Implementation workflow `a1949622-b54e-43b0-af3b-e16c8c458758`, mission
`48c50c62-3882-42bc-b0f4-a8610818bc64`, stopped before its reviewers because async
acceptance rejected an earlier final report: `commands-run evidence missing from
child report`. Current saved artifact includes commands, but that does not erase
the rejected receipt. Source is preserved at `7c1503c2`; parent captured its diff
in `.local/evidence/readiness-recovery/async-preserved.patch`.

Same-protocol recovery workflow **`b1993d73-fc81-44c5-a322-46d9f0c3a36b`**, mission
`b7118bd0-5a1e-4693-9b8a-980e422e0a77`, completed. Retained async child
`9daecfe2-77e5-48a6-a22d-c24439bf7c4e` reconciled the command logs, all 751 tracked
files and preserved patch without source changes or build/test reruns. The
original rejection was from later guidance-only responses with empty command
arrays, not missing implementation logs. Both fresh reviews then ran and BLOCKED.
Reports are under `subagent-artifacts/outputs/b1993d73-fc81-44c5-a322-46d9f0c3a36b/runtime-readiness/`
in the Techne session artifact directory.

Corrective workflow **`6721dbd7-33f0-4b49-8f22-6085331ef023`**, mission
`6788814b-e489-4093-916d-24b07d162a77`, ran four retained writers in their original
isolated workspaces, each in a new described change. Script:
`.local/workflows/fix-readiness-review.js`. Native child
`f404a435-5642-411d-94c1-55239b46fcc8` encountered provider rejection after compaction:
`Codex error: This content was flagged for possible cybersecurity risk.` No
file-only handoff was produced. Parent captured its two-file exploratory-test
diff at `.local/evidence/readiness-recovery/native-provider/partial.patch`,
revision `9f06194f` / change `qpylwkry`, then resumed the same agent/protocol as
**`ce2e1cd4-6b5b-4a17-b332-ac999c20f444`**, clarifying defensive compiler-disable
maintenance. No provider, privilege or execution-mode fallback; continued
rejection must remain a blocker. Other corrective writers are unaffected.

The original workflow completed with that failed child receipt, so its automatic
reviews were skipped. Revived native run completed successfully with a durable
handoff; no execution-mode fallback occurred. IR/native correction re-review is
completed separately in workflow **`9f6b748a-df30-42e7-a5f2-2a55f6de3e0a`**,
mission `fc5f6d1c-ca82-40b8-b17e-857633ab7a76`. Reviewer
`2fa3bd0c-c425-49a1-b6a4-f8120af1b537` returned **OK with notes** for production
corrections: IR finding closed and legacy exclusion comprehensive. It identified
a P1 fixture migration: IR tests still expected legacy native installation, and
one scheduler test relied on automatic legacy compilation to test rejection.
Parent adapted only those assertions: preserve interpreter effects/results and
use a panic-only synthetic legacy entry for admission rejection; actual native
positive gates remain in scalar tests. Parent must separately resume the lifetime
reviewer once frame/descriptor repairs arrive. This sequencing overlaps read-only
compiler review with remaining lifetime writers without accepting their output.
Parent continuation: resolve findings, validate and integrate corrections,
finish combined qualification; do not stop at component reports.
This continues the accepted bounded milestone in `MODERN-RUNTIME.md`. The user
requested the remainder, continued execution, and parallel work. Parent owns
architecture decisions, finding disposition, integration and acceptance. Child
reports are intermediate evidence, not the end of this work.

## Frozen source and authority

Base: `7f25d97e46c2d4134f52d37065d6c5319393f72b` in
`/home/arthur/src/techne-steel/integrated`. Do not edit that reviewed change.
Original `/home/arthur/src/steel` remains untouched. Each writer receives a new
JJ working change above this base in a distinct workspace. No Git mutations,
shared-ancestry rewrites, pushes, PRs, live installs, or nested delegation.

This is multi-seam work: execution, heap, semantic compilation, native execution,
and qualification have distinct contracts and gates. Each has one writer;
shared files have function/contract ownership below, not competing broad owners.
Parent resolves integration glue only after durable component handoffs.

## Lane board

All workspaces are under `/home/arthur/src/techne-steel/`.

| Lane / workspace | Exclusive contract and primary source | Focused gate | Why independent |
| --- | --- | --- | --- |
| `next-async` | Budgeted task wait/wake/cancel; `steel_vm/resumable.rs` and its tests; VM FutureFunc/polling call paths and task lifecycle only | Real delayed host futures plus CPU task, rooted values, errors, late wakes, no replay, collection while waiting | Extends interpreter scheduler, not compiler/native/collector |
| `next-heap` | Built-in cell/vector heap ownership, tracing/barriers and RootToken thread affinity; `values/closed.rs`, collector support, audited mutation sites | Ordinary Scheme boxes/vectors and captured graphs; mutation during collection; cycles reclaimed; rooted survivors | Changes actual allocation/tracing, not the managed host-type demonstration |
| `next-ir` | `compiler/semantic_ir.rs`, its tests, symbolic compiler integration | Captures/lets/local mutation admitted with verified semantics; differential effect/error/live-global tests | Produces existing bytecode; does not redesign execution or native ABI |
| `next-native` | `jit2/`, native state/safepoint helper module; VM DynSuperInstruction/JIT construction branches only | Native loop actually runs, returns on budget, resumes exactly once, preserves roots/errors/redefinition and code owner | Builds native safepoint protocol; async lane owns task/wait policy |
| `next-qualification` | New `experiments/runtime-readiness/` embedding testbed and measurement/remote probe tooling only | Runnable local mixed workload and two-process explicit invocation; reproducible raw metrics with configuration | Consumes public APIs; no production-core edits or competing benchmark runs |

Cargo/module declarations receive only minimal lane-specific additions. Async
owns `resumable.rs`; native must request parent coordination for any changes
there. Heap owns existing core root-token/tracing definitions; async and native
consume their public semantics and escalate necessary changes. Native and async
may touch `vm.rs` only in their named, distinct dispatch contracts; neither owns
the entire file. IR must preserve opcode semantics consumed by both. Qualification
must adapt to published handoffs later, not invent hypothetical finished APIs.

## Reviewed blockers and corrective ownership

- **P1, async regression:** persistent saved-state publication is emptied during
  dispatch, exposing missing live-frame contract roots in default/sync collection.
  Async owns contract enumeration in `closed.rs` live functions/continuations,
  including recycler/sync visitors, and the surgical parked/current-frame loops
  in `vm.rs::Synchronizer::enumerate_stacks`. Heap must not duplicate that seam.
- **P1, IR canonical metadata:** nested scopes can hide a missing outer `LetVar`
  from a self-tail completeness check. IR owns complete enclosing-prefix checks,
  with valid initializer temporaries preserved and pre-mutation lowering tests.
- **P1, descriptors and foreign marking:** heap owns descriptor owner handles,
  weak registry/callable identity indices, traced generated-function metadata,
  explicit registration leases, and serialization identity/import updates.
  Approved narrow FFI conversion/literal and `vm/threads.rs` metadata-import
  changes; no scheduling/contract-enumeration changes. Dynamic same-name types
  must not alias. Serialization always creates fresh dynamic destination owners,
even on the same thread, preserving aliases through its explicit transfer map;
ordinary local lookup/clone preserves live identity. Thread identity is not a
heap-owner witness. Builtin reuse needs factory-assigned kind and canonical
metadata validation, not names; noncanonical builtin metadata is rejected.
Non-Copy
  descriptor/API changes must be documented. No permanent registry roots or
  hidden unsafe Send bridges. Default/sync foreign marking may use serial
  owner-local tracing with the synchronizer intact, without parallel performance
  claims. All these repairs remain pending implementation/validation.
- **P1, legacy native roots/code ownership:** native audit found multiple unsafe
  collecting-helper classes and bare executable pointers without owned teardown.
  Parent approved rejecting **all legacy compilation** before effects, including
  explicit JIT requests returning unchanged bytecode closures. This deliberately
  disables unsafe legacy optimization; it does not make heap-native execution
  safe. The new scalar safepoint backend remains the actual native implementation
  and must retain native execution, fuel and code-lifetime tests.
- **P2:** heap corrects the stale summary claiming ordinary tracing is unchanged.

Parent owns `/home/arthur/src/techne-steel/readiness-integrated`, change `quvryyrt`,
assembled from the five frozen handoffs (using parent root correction `189b50b7`).
The initial merge was conflict-free. This is corrective assembly, **not accepted
integration**. Parent alone owns combined `resumable.rs` native setter/stat and
between-slice sweep forwarding, combined tests, exhaustive Waiting match glue,
and qualification API glue. The substantive qualification consumer follow-up is now
owned solely by the retained qualification worker in `next-qualification` (see
below); parent will integrate its package-only patch. Future corrective handoffs
will be merged as new parents/changes without rewriting delivered history. Core fixes above
remain owned by their component workers, not duplicated in this workspace.

Parent assembly glue now adds public health/borrow-checked native setter/stats,
between-slice sweep begin/step, and collector unwind poisoning. New bounded
subprocess scenarios combine real registered Pending/Ready waits, IR modes,
ordinary/built-in collection, cross-thread readiness-only wakes, exact effects,
native disabling/re-enabling/cache counters, cancellation/payload reclamation,
and owner-retaining completion after runtime drop. Existing IR/native fixtures
explicitly reject unexpected Waiting; qualification matches now recognize it.
Actual async/native qualification workloads and measurement relabeling remain
pending. The first assembly gates run as **`proc_1a20`** via
`.local/workflows/validate-readiness-assembly.sh`, serialized with component
builds and using a distinct cache copied (not hard-linked) from frozen next-roots.
Initial `proc_1a20` compiled default mode but failed a parent fixture assumption:
zero-fuel calls on a waiting task return `Waiting`, not `Yielded`, even after a
wake. `run_task` establishes this before polling; no production defect was found.
Original source `f78af497` and logs are preserved in
`.local/evidence/readiness-parent/assembly-zero-fuel-before/`, including the
source-backed disposition. New child change `znmmwyrv` corrects only that variant
assertion; zero work/polls/transfers/native-counter checks remain intact.
Rerun `proc_974c` passed the default combined scenario, then the combined feature
build hit an older pruning fixture's exact one-region admission assumption
(actual: two). Preserved source `ffd74006` and logs/disposition are in
`.local/evidence/readiness-parent/assembly-admission-before/`. New change
`yyqvvlwt` permits additional admitted regions. Third run `proc_bbae` passed the
exact one-pruned-branch assertion, then found two removed CFG blocks rather than
one. Source `14218281` and logs/disposition are preserved in
`.local/evidence/readiness-parent/assembly-cfg-before/`. `optimize_constants`
counts unreachable CFG blocks independently of branches; returning calls have
separate continuation blocks. New change `onpmpkxz` requires actual block removal
without fixed cardinality, retaining exactly one pruned branch and all runtime
results/effects/reclamation checks. Fourth run `proc_d322` reached the expected
legacy boundary: `modern_runtime` creates a native closure before consuming the
Engine, and budgeted admission rejects it as `UnsupportedExecution: precompiled
native closure`. That safety rejection is preserved, not weakened. Logs remain
in `.local/evidence/readiness-parent/assembly/`. The native worker's pending
legacy-compiler exclusion should remove this precompiled path.

Focused run **`proc_c990`** executes the new combined API scenarios (definitions
compiled inside budgeted mode), sync variants, unit-test compilation and the
qualification package, without the currently blocked old modern-runtime fixture.
Script argument: `validate-readiness-assembly.sh api`; separate logs:
`.local/evidence/readiness-parent/assembly-api-focused/`. **Passed** (`proc_c990`,
exit 0) at unchanged **`1ddc10e4223a80ba40f1e70047e5e8d1d4c941b4`**: default one
parent scenario; combined non-sync two; sync one (subprocess outputs not double
counted); unit-test compilation only; qualification package Cargo/smoke tests.
The full-mode script still runs the old modern-runtime configurations; no full
matrix or runtime safety acceptance is claimed.

Parent has now merged reviewed `e50eeb83` and `7144c72a` into
`readiness-integrated` in new change `vprvuwxv`, preserving `1ddc10e4` and both
component revisions. Merge was conflict-free; parent fixture migration changes
only IR/scheduler tests. Full parent matrix **`proc_ea04`** completed with
combined/default/sync API gates, old modern-runtime fixtures, IR and scheduler
suites in default/jit2+sync, scalar/legacy-exclusion native tests, and qualification.
**Passed** (`proc_ea04`, exit 0) at unchanged
**`de558453c62d99fb4f92acca3549e58c1e1363e8`**: API scenarios default1/combined2/sync1;
old modern-runtime combined2/sync1; IR22 each default/sync; scheduler19/20;
native/fallback15; unit-test compilation and qualification Cargo/smoke. Child
subprocess outputs are not extra tests. Logs:
`.local/evidence/readiness-parent/assembly/`; old legacy-boundary failure is in
`assembly-legacy-boundary-before/`. This matrix predates frame/descriptor repairs.

Parent then provisionally merged frame fix `27481cbd` into new change `uonoktlp`.
The only conflict was an existing non-sync builtin-sweep-only live-contract edge
versus the new unconditional edge; resolved to unconditional contract enumeration
in every mode. No duplicated edges or foreign-owner policy changes. Parent read
the full correction and inspected sync reference-queue keepalive handling;
independent lifetime re-review remains pending. **`proc_de6e`** runs the scheduler
and two new actual-GC/continuation regressions in default, non-sync
builtin-sweep+jit2+incremental-gc, and sync+jit2+rooted-instructions. Driver:
`.local/workflows/validate-readiness-frame-merge.sh`; logs:
`.local/evidence/readiness-parent/frame-merge/`. **Passed** (`proc_de6e`, exit 0)
at unchanged **`e08a57297d63823730126dba7d8af4cf2717d454`**: scheduler/default21,
non-sync combined22, sync22, including both new regressions in each configuration.
This closes parent execution coverage of the frame-contract fix across supported
modes; independent lifetime review and the descriptor correction remain pending.

### Approved descriptor follow-up seams

Heap worker additionally owns these source-backed lifecycle corrections:

- Transfer-local visited descriptor IDs for finite self/mutual metadata export,
  including the narrow `threads.rs` context initializer. No ambient visited state.
- Scoped `HeapSerializer` roots for completed recursive values and destination
  descriptors during import, including callback-triggered collection. Final
  destination ownership must be established before those roots drop. This is
  transfer-sized temporary rooting, not permanent registration.
- On failed imports, detach metadata only from newly created dynamic destination
  owners, breaking partially built cycles without altering prior/builtin owners.
- Global-slot recycling uses its own visited identities and does not mutate either
  heap's marks/counters/sweep epoch. Preserve distinct views such as list slices,
  early-exit cleanup and the async lane's separate contract enumeration.
- Serializer/deserializer registries copy the selected function pointer and
  release their guard before user callbacks. Replacement affects later lookups.
- Root removal unlinks under the root-table guard but destroys the removed value
  only after releasing that guard in default and sync. Audit every caller.

Additional approved seams: descriptor-specific Custom equality/hash both use
immutable runtime owner IDs (generic Custom and eq? wrapper semantics unchanged);
actual `deserialize-value` releases its input Custom guard before callbacks and
roots the final result through reconstruction teardown, with an explicit VM
handoff audit. These too require targeted bounded tests and final independent
lifetime review; interim descriptor-suite passes are not complete acceptance.

### Qualification continuation

Workflow **`0e64bdef-8d82-4a96-a145-f06fb160210a`**, mission
`87c3a824-7d37-484f-aa6c-3f367c8be774`, resumes the original qualification worker.
Parent prepared a conflict-free provisional merge `ff089e41` / `wwrslnsn` in
`next-qualification` from passed assembly `1ddc10e4`, native `7144c72a`, and IR
`e50eeb83`; worker must freeze it with a new child change before package edits.
Original `5344a941` remains preserved. This is a consumer preparation branch,
not acceptance of the under-review corrections. Worker owns ONLY
`experiments/runtime-readiness/**`: actual waits/wakes/cancel, native counters and
controls, built-in sweep, truthful bounded two-process await, and zero-fuel
measurement relabeling plus positive-fuel publication samples. No comparative
campaign while other builds run; parent repeats qualification after final safety
corrections. Core source changes/merge conflict resolution remain parent-owned.

Delivered **`3124a2b5ac6ec8132db1b8954db9e3c136150909`** / `nmovzltl` above frozen
`ff089e416958ff8ffc69a3788341b54eb5085b3b`: exactly eleven package paths. Optional
`modern` enables jit2+builtin-sweep; default keeps the non-sync managed-arena
configuration. Worker reports both test/smoke/clippy configurations passing,
with four workloads and fifteen protocol checks each. Real futures/files and
joined readiness-only threads, explicit protocol2 await/wake/ready, native
controls/counters, actual bounded sweep, and separately labeled zero/positive
fuel/publication/full-collection observations are implemented. No comparative
campaign ran.

Parent independently verified the candidate revision, all eleven source hashes,
exact JJ/package patch hash, modern binary hash, eleven worker exit records,
and preservation of all 161 old dependency pins (38 additions). Evidence:
`.local/evidence/readiness-parent/combined-qualification/source-verification.json`.
Reviewer **`e957cac8-8439-4036-aa89-5107cbdfbf99`**, workflow
**`88b822b6-cb4e-4505-bda8-0f4982278b86`**, returned **OK with notes**, no findings,
for this consumer only. Request-count exhaustion is source-verified rather than
probe-exercised; six cache entries are not eviction stress; full disconnect
proves exit/reap, not cleanup acknowledgement.

Parent provisionally merged the package into frame-corrected assembly, new
change `xpkwxpuw` (initial merge `789725c9`), with no conflicts. **`proc_2a27`**
runs independent locked default/modern Cargo tests, raw smoke, supervision,
modern clippy and formatting, binding each binary hash to its configuration.
Driver: `.local/workflows/validate-readiness-combined-qualification.sh`; logs:
`.local/evidence/readiness-parent/combined-qualification/`. **Passed** (`proc_2a27`,
exit 0), revision unchanged at **`789725c958743f0b7c14e28e3de5907e91fe2d49`**.
Parent reran both feature configurations and raw smoke (four workloads/fifteen
protocol checks each), three supervision negative controls, modern clippy and
formatting. Final descriptor integration/review remains necessary.

Parent additionally **passed** the review's formerly source-only request cap:
`proc_1370`, `.local/workflows/check-readiness-request-cap.py`, drove 4,096 valid
requests and separately 4,096 frames with duplicate IDs. Both produced the
request-limit event and exited/reaped with status 0. Existing bounded supervision
and a whole-check alarm applied; no source changes, cleanup-acknowledgement or
benchmark claim. Evidence: `combined-qualification/request-cap.json`, including
binary hash.

Qualification observed the inherited default collector does not eagerly destroy
all dead slot payloads after cancel/full GC. Approved boundary: record that count
truthfully and require final release after all runtime/OwnedValue owners drop;
modern builtin-sweep retains exact after-sweep destruction. Deterministic
future/file guard cancellation remains mandatory in both modes—external resource
cleanup must not rely on eventual GC destruction. Worker preserves its failed
default assertion and source evidence rather than changing core policy.

## Shared contracts

- Owner-thread shared interactive world, isolated/message-passing workers. No
  arbitrary cross-thread Steel value mutation or unsafe Send/Sync assertions.
- Actual Steel compilation/execution; no alternate toy evaluator, continuation
  adapter passed off as budgeted await, or host-managed types passed off as
  built-in heap migration.
- VM/native exhaustion yields resumable owned state. Never recover by rerunning
  an effectful prefix. Zero fuel does no interpreter/native work. Host callbacks,
  macro expansion and compilation remain explicitly non-preemptible unless
  separately addressed.
- Future polling occurs only on the owner thread and outside collection. Wakers
  may signal readiness across threads but carry no unrooted SteelVal. Cancellation
  must unregister/drop owned wait state; stale/duplicate/late wakes cannot revive
  a task or run effects. Waiting, yielded, executing and completed roots have
  explicit ownership, including nested future-held values and callbacks.
- No raw SteelVal escape from resumable completion or future-completion public
  APIs. Owner-retaining handles or scalar copies only. Existing embedding APIs
  retain their documented caller rooting obligations; do not silently remove them.
- Heap work must audit actual mutable-edge publication, closure captures,
  globals/stacks, Rust roots, custom traversal and cached futures. Do not use
  permanent roots, conservative leaks, lifetime transmutation, or hidden strong
  handles for every internal edge to simulate success. Consult the parent when
  gc-arena cannot honestly preserve the existing embedding contract; an evidence-
  backed different collector strategy is permitted only by parent decision.
- Native suspension must retain code ownership, reconstruct logical values/PC
  and publish precise roots before GC. No collection over unreported registers.
  Unsupported native forms fall back *before execution*. Checked arithmetic,
  dynamic globals/redefinition and observable errors/effects remain intact.
- IR verifies before and after optimization; no opcode narrowing or tail-call
  regression. Do not erase mutation, allocation, exceptions, suspension, unknown
  Rust calls or dynamic global lookup. Unsupported semantics stay explicit.
- Compact-value ABI selection requires full ownership and workload-cost evidence;
  no sweeping ABI rewrite solely because an isolated eight-byte value is smaller.

## Component handoff notes

Async source delivered at `7c1503c2a8c62cd7334c44ff3f42010fd2d13aa9` in
`next-async`; report recovery succeeded as described above, but review found the
executing-contract regression. It adds
actual owner-thread `register_async`, `AsyncValue`, `SliceOutcome::Waiting`,
readiness inspection/parking, and constant-work saved-state root publication.
Child claims 19 default / 20 sync scheduler scenarios, 23 VM tests each,
2 non-sync / 1 sync existing combined scenarios and 6 ownership doctests each,
plus clippy/formatting; parent read the report but has not independently rerun
these source gates yet. Integration patch: `.local/native-safepoints.patch` in
that workspace. Staged host-call return must precede native, fuel and safepoint
hooks. Do not mistake a rejected handoff receipt for failed implementation tests,
or a later corrected report file for a successful lifecycle acceptance.

Frame-contract corrective handoff **`27481cbdc7c93e71a76cc70e72d1f1e4546f9072`**,
change `montynoz`, is delivered in next-async. Three files: contract enumeration
at live/continuation frame paths in `closed.rs`, four synchronizer frame paths
in `vm.rs`, and two bounded regressions. Worker reproduced missing marks during
actual automatic allocation GC in default and sync, then passed scheduler 21/22,
targeted GC 1/1, existing VM 23/23, clippy and formatting. Root-release validation
uses bounded slot reuse to observe payload destruction. Remote synchronizer and
recycler paths are source-audited, not separately forced at runtime; builtin-sweep
was absent from that lane. Evidence: `next-async/.local/frame-contracts/`.
Parent read the full patch, merged it, and reran all three scheduler variants
successfully at `e08a5729` as recorded above. Independent lifetime re-review remains
pending with the descriptor correction.

Qualification handoff received at **`5344a941d7005d1f4e2301a35e822b10e48cfc07`**,
new change `osoymnoo` above preserved initial `69c8f8a9`. Parent selects this
follow-up for review/integration, not the initial revision. Report:
`/home/arthur/.pi/agent/sessions/--home-arthur-src-techne--/subagent-artifacts/outputs/a1949622-b54e-43b0-af3b-e16c8c458758/runtime-readiness/qualification.md`.
Fourteen package-only files implement mixed workloads and a real bounded
two-process probe. Root-window tests use 1/8/32 distinct live box arguments;
zero-fuel observations measure the entire suspension/publication path, separately
from full legacy collection. New async/native/built-in-sweep APIs are NOT consumed
yet. Child reports Cargo gates, 3 supervision controls, smoke, clippy and formatting
passed. Parent independently checks source hashes and reruns Cargo/supervision/
smoke via `.local/workflows/validate-readiness-qualification.sh` (`proc_3ae6`);
logs go to `.local/evidence/readiness-parent/qualification/`. Parent reruns
**passed** (`proc_3ae6`, exit 0): exact 14-file source hashes, Cargo tests,
3 supervision negative controls, 4 mixed workloads and 12 two-process protocol
checks; revision unchanged. No performance conclusion or combined-runtime
acceptance follows from this component.

Heap handoff: `df9d0b502852b5c9ac68bc17569596f8e720cb88`, frozen in
`next-heap`. Parent correction workspace `next-roots`, change `zyxzusyz`, starts
above it. The first correction makes every non-sync `builtin-sweep` collection
use owner-local traversal, including ordinary allocation-pressure/full GC, rather
than only explicit begin-sweep calls. The original foreign-bridge reproducer is
now a positive survival/release regression. Parent script
`.local/workflows/validate-readiness-roots.sh` **passed** (`proc_90cb`, exit 0) at
`189b50b78249097c8abfd4570120b70428c17f6a`: independently reproduced the original
failure, then passed 8 sweep scenarios (3 metadata repros remain ignored),
1 downstream Custom test, 9 default and 11 sync heap regressions. Its target
directory is separate, seeded by a non-hard-linked cache copy from the completed
heap lane. Default without `builtin-sweep` and sync are not claimed fixed by this
first correction. Descriptor metadata ownership remains pending separate source work.
The lifetime reviewer examined both frozen `df9d0b50` and this correction,
supporting its bounded non-sync fix while retaining descriptor/default/sync gaps.

IR handoff: `6e83512dd0990b42421e1a43fde91c680edd20f4`, frozen in `next-ir`.
Three compiler-only files add captures, closures, mutable lets, explicit returning
tail cleanup, conservative scalar propagation, and canonical lowering checks.
Child reports 19 IR scenarios and 5 existing codegen tests in each default and
jit2+sync configuration, plus clippy/formatting. Parent has read the handoff;
independent review found the nested self-tail metadata hole above. Corrective
handoff **`e50eeb834e1d8c3fa13e39cba0e21c406523f632`** (change `nvttuwxo` in
next-ir) now checks every enclosing initialized range at self-tail lowering,
without rejecting valid temporary values during nested initializer evaluation.
Worker reproduced the exact public-lowering failure in default and jit2+sync,
then passed 22 IR scenarios and 5 codegen tests in each, clippy and formatting.
Only two IR files changed; patch/evidence are in `next-ir/.local/ir-fixes/`.
Parent has read the report; independent corrective review and parent reruns are
still pending. This handoff is not yet merged into the assembly workspace.

Native corrective handoff **`7144c72a1e78cbcd4eb98697b8314784b6c86c54`** supersedes
the original legacy-availability policy. Its public legacy JIT is a zero-sized
rejecting admission object with no module construction; explicit and automatic
requests retain original bytecode. Scalar safepoint native execution remains.
Worker passed 15 native/fallback parent scenarios in each of three native modes,
10 default scheduler and 23 VM tests, clippy/formatting. Initial exploratory
forced-GC legacy test passed (no crash/reclamation failure reproduced). Evidence:
`next-native/.local/root-fixes/`. Reviewer cleared production with the fixture
migration noted above; parent has merged it and is validating.

Original native handoff: `e49ab6f628d7c2fce0bb9160ac1e36199eb90db2` in `next-native`,
above preserved implementation `f82fec2e`. Five changed source files. Child passed
13 parent native/legacy scenarios in non-sync, sync and sync+rooted-instructions,
plus 10 default resumable and 23 VM tests, clippy and formatting. Actual native
execution of the 2,000-decrement loop records 21,004 native dispatches, 6,001
native arithmetic operations and 999 lowered backedges; total 24,015 dispatches
matches interpreter. Parent read the report; this is mechanism evidence, not a
performance conclusion. Public setter/stats plumbing and combined validation are
pending. Ordinary legacy JIT NEWBOX/helper execution has a source-established
unreported-temporary GC path (no reproduced crash yet); budgeted admission rejects
that path. Evidence: `next-native/.local/evidence/legacy-gc-risk.md`.

## Parent decisions during implementation

The workers surfaced architectural boundaries rather than silently hiding
unsupported semantics. Approved directions:

1. **Budgeted async:** task-aware `register_async` accepts owner-retaining inputs
   and returns a future of copied scalars or owner-checked values. Registered calls
   auto-await; legacy arbitrary FutureFunc remains unadmitted. Factories, polls and
   result publication occur outside VM/Engine borrows. A persistent rooted Custom
   publisher traces complete task stack/frame state at collection, avoiding
   per-yield traversal/token churn. Await errors retain original call context;
   cancellation, lost-wake races, owner cycles and tail returns need direct tests.
2. **Built-in heap:** direct gc-arena replacement conflicts with unbranded SteelVal,
   arbitrary Custom interior mutation and existing unguarded mutable-vector writes.
   Approved actual built-in heap work is **atomic tracing plus incremental sweep**,
   retaining existing generic traversal. Mark/root/custom scans remain unbounded:
   this does not close the incremental-marking/GC-pause gate. No lifetime erasure
   or graph-rebuilding arena facade. Pending-sweep allocation/reuse, weak/finalizer
   behavior and rooted mutation require explicit correctness coverage. Core
   non-sync RootToken thread affinity is fixed in this lane as well.
3. **Native execution:** opt-in Cranelift scalar bytecode segments transfer typed
   logical stack/PC state back on every native exit; unsupported operations stop
   before their instruction and continue in the interpreter without replay.
   Heap values remain in the VM; no heap-capable native frame/GC claim. Native
   owner adds `RuntimeOptions.native_safepoints` (default false) and the coordinated
   pre-dispatch hook. Async owner reserves the public resumable setter; parent
   integrates it after both APIs exist. Legacy DynSuperInstruction remains
   rejected. Cache keys/lifetime must include owner and code identity, and avoid
   immortal TLS retention. The legacy Engine JIT owner did not provide the expected
   teardown boundary in tests, so the new cache must have independent runtime
   ownership (`RuntimeOptions` if compatible with existing thread contracts).
   Cloned worlds start with empty caches; no new unsafe Send/Sync assertions.
   The public native setter changes only admission after a healthy-runtime check:
   disabling stops native execution but retains the bounded cache/counters until
   owner release. Cache initialization is lazy; statistics use
   `native_cache.stats()`. Async supplies this setter as a parent-integration patch
   so its standalone workspace need not depend on unpublished native fields.
   Lifetime probes must prove native-module release independently of the old JIT
   Arc, accounting for completion handles legitimately retaining an Engine.
   Fuel charges actual executed work including backedges, not source-loop counts
   that compiler lowering/unrolling may change. The 2,000-decrement fixture lowers
   to an inlined initial iteration and two iterations per lambda body: 999 TCO
   backedges and 24,015 reference dispatches, not 2,000 native backedges.

   With `jit2`, existing compiler lowering leaves arithmetic as global calls.
   Approved narrow native specialization classifies the actual current binary
   `+`, `-`, `<=` FuncV targets before each batch, with code/PC/arity/classification
   guards and checked arithmetic. Wrong types/overflow stop before the call for
   interpreter handling. No name-based trust, helper callbacks, or global mutation
   inside a native batch; concurrent shared-global mutation must be excluded or
   the fast path rejected. Tests must prove native arithmetic, not just native
   stack/control work. Steel rejects `set!` of literal imported builtin names;
   test guards by binding each audited builtin object to fresh mutable aliases,
   replacing each alias via `set!` between slices, then restoring it. The unchanged
   alias must remain admitted while the changed alias executes its effectful
   replacement exactly once. Preserve the rejected literal-name fixture as
   reference-policy evidence; do not claim those imported names were mutated.

4. **Live binding semantics:** retain Steel's distinction between top-level
   `define` shadowing and `set!` mutation. A task compiled before a shadowing
   `define` can still call the old slot; newly compiled requests resolve the new
   definition. Explicit `set!` updates the stable binding seen by pending calls,
   without rewriting active frames. Qualification must compare ordinary Engine
   behavior and test all three cases, not claim transparent rebinding/package
   reload. `SymbolMap::add` allocates a new slot and records the shadowed one;
   this is not an optimizer license to cache values across `set!`.
5. **Returning tail calls with lexical cleanup:** richer let support requires
   general `Terminator::TailCall` to carry an explicit verified returning edge
   and result, preserving existing LETENDSCOPE/POPPURE cleanup for returning Rust
   callbacks. Tail-transferring closures and nonreturning SelfTailCall retain
   their existing behavior. The IR owner may extend the representation/lowering
   using existing opcodes only; verifier, branch/effect/cleanup and codegen-bound
   tests must cover the change. A generic jump to the epilogue is insufficient
   when compiler-emitted scope cleanup remains. Public hand-built IR also needs
   complete LetVar initialization metadata before lowering for the existing
   bytecode/JIT consumer. Prefer backend-neutral semantic verification plus a
   consistent pre-emission lowering check; rejecting incomplete canonical IR in
   the verifier across all builds is also permitted. Do not defer missing-marker
   safety to the separate native lane or accept feature-dependent silent gaps.
6. **Cross-heap tracing:** opt-in built-in sweep uses per-traversal object identity,
   not pre-existing mark bits, to visit rooted graphs crossing owner-local heaps.
   All heap owners must remain alive; this does not make bare values owner-retaining
   or relax async/result foreign-owner checks. Sweep and accounting stay local,
   and alternating/pending foreign sweeps must remain sound. The worker identified
   a legacy mark-bit early-exit risk for a rooted B-to-A bridge when A collects;
   preserve its reproducer and track default/sync disposition separately rather
   than silently claiming those paths repaired.

7. **Public custom tracing:** re-export existing `MarkAndSweepContext` at the
   crate root so downstream Rust can name the parameter of public
   `Custom::gc_visit_children`. Keep the implementation module private. Require a
   public-path doctest and actual child-survival/reclamation test; document the
   synchronous, non-reentrant edge-reporting contract rather than demonstrating
   custom tracing only in privileged internal tests.

8. **Descriptor metadata is a required follow-on, not a waived safety gap:**
   inherited struct traversal visits instance fields but omits type-descriptor
   property values. Descriptors are VTABLE indices; generated Rust
   constructors/predicates/getters also capture those indices opaquely. The heap
   component may hand off tested boxes/vectors and enumerated graph coverage, but
   must preserve reproducers for instance-only, descriptor-only and constructor-only
   reachability. Parent must repair coherent descriptor/property/capture ownership
   and registration lifetime before claiming whole-heap readiness. Permanent
   VTABLE rooting or an instance-only patch does not close that contract.

9. **Non-sync native portability:** the native owner may repair five inherited
   compile errors in `steel_vm/vm/jit.rs`: use Shared rather than hard-coded Arc
   for instruction bodies, cfg-correct environment callback signatures, and
   unwrap the valid-index non-sync setter's unconditional Ok result consistently
   with existing local_set handling. No helper ABI or Send/Sync change. This
   enables the required non-sync heap+async+native integration; it does not certify
   legacy JIT root safety. Preserve the failing build and rerun both native feature
   configurations plus default non-JIT behavior on the corrected source.

These are implementation decisions, not acceptance. Full incremental marking
still needs an audited mutation/publication contract (including opaque custom
objects and futures) or a broader value/embedding migration. The final report
must keep that unresolved requirement distinct from incremental sweep.

## Validation and delivery

Each component supplies exact revision, changed files, machine-readable/patch
inventory under its workspace `.local/`, focused test commands/logs, public API,
known limitations and integration obligations. New Rust packages use edition 2024
and resolver 3. Hang-risk scenarios use subprocess deadlines with kill/reap.
Use one Cargo job and a distinct target directory per lane. Host memory is under
pressure: serialize Cargo build/test processes with
`flock /tmp/techne-runtime-readiness-cargo.lock`; code work remains parallel.
No comparative timing campaign during concurrent implementation. Known baseline
`clippy::never_loop` may be explicitly allowed, not silently hidden.

Infrastructure mitigation: automatic pi-lens Rust checks/tests were spawning Cargo
outside the shared lock in default target directories. Parent installed temporary
`/home/arthur/src/techne-steel/.pi-lens.json` (outside every source workspace),
with Rust/Cargo scan ignores, Rust LSP denial and autofix disabled. Actual pi-lens
config/ignore loaders confirmed both Rust and Cargo paths ignored in all five
workspaces. This is not passing LSP evidence; explicit locked Cargo/rustfmt gates
remain authoritative. Writers may terminate only re-verified automatic analyzer
processes belonging to their own workspace, never broad process-name matches or
explicit builds. Parent must remove this temporary config after the wave. User
machine-global settings were not changed.

The workflow collects handoffs and runs fresh read-only safety reviews; parent
then independently tests, dispositions findings, integrates, adapts the mixed
workload and remote probe to the real combined APIs, and runs serial measurements.
The runtime gate stays open until the combined evidence supports the claimed
semantics. Architectural blockers must be named, not hidden by reducing tests.
