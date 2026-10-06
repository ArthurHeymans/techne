# Supported runtime qualification

> **History.** This documents the Steel modernization, which techne-vm replaced
> below the parser; see [TECHNE-VM.md](../TECHNE-VM.md) for the current runtime.

**Status:** implementation, independent safety reviews and parent correctness
qualification passed for the supported subset. Comparative performance
qualification is pending. This is **not** full Stage 0A or production acceptance.

## Frozen implementation

- Combined Steel source: `c7b804a7d26f87eaf8a54d72db8693d0b97cca26`, change
  `ukppryvq`, workspace `/home/arthur/src/techne-steel/readiness-integrated`.
- Original `/home/arthur/src/steel` remains at
  `13307cc3aeef791d1ff462fcc81efebd5d627806`, unchanged.
- No installation, push, PR or application implementation was performed.
- Detailed chronology, failed evidence and ownership decisions:
  [READINESS.md](READINESS.md). Earlier accepted milestone:
  [MODERN-RUNTIME.md](MODERN-RUNTIME.md).

## Delivered

- Owner-thread, fuel-sliced Steel tasks with wake-driven host futures, persistent
  roots, cancellation and owner-retaining completion values.
- Executable verified semantic IR and optimization with checked fallback before
  unsupported execution; exact effects and live binding semantics retained.
- Real scalar Cranelift execution with safepoint/fuel accounting, interpreter
  fallback, bounded runtime-owned cache and enable/disable controls. The unsafe
  legacy i128 compilation path is disabled rather than presented as repaired.
- Actual built-in atomic marking and slot-budgeted sweep; the separate managed
  `gc-arena` heap still uses its own branded incremental protocol.
- Owned descriptors/generated callables and metadata-key/value tracing;
  owner-local foreign traversal; root removal outside user-destructor guards.
- Explicit graph transfer with fresh destination identity, alias preservation,
  checked global/constant reconstruction and restricted native import capabilities.
  Selective failure cleanup preserves published owners while breaking unescaped
  fresh metadata cycles, including recoverable pre-return teardown unwind.
- Native callback collection roots the actual ordinary VM caller, including
  exception handlers, constants, locals, captures, contracts and pending values.

## Parent-confirmed gates

Final driver: `.local/workflows/validate-readiness-lifetime-final.sh`.
Evidence: `.local/evidence/readiness-parent/lifetime-final/`.
Source revision remained unchanged throughout; driver exited 0 (`proc_c7f9`).

| Configuration / gate | Result |
| --- | --- |
| Default full library | 525 passed; 5 existing ignored |
| Non-sync builtin-sweep + jit2 + incremental-gc + dylibs + rooted-instructions | 556 passed; 6 existing ignored |
| Sync + gc-root-experiment + jit2 + rooted-instructions + dylibs | 546 passed; 6 existing ignored |
| Public runtime / downstream / managed-heap / custom-hash gates | Passed |
| Default / sync doctests | 24 / 22 passed |
| Both core lint configurations and formatting | Passed; baseline never_loop allowance and warnings documented |
| Default / modern consumer tests and raw smoke | Four workloads and fifteen protocol checks each |
| Supervisor timeout / overflow / disconnect negative controls | Three passed |

Independent component reviews closed the identified collector/frame and transfer
findings. Reviewers did not rerun tests. The final merge delta was independently
hash-verified by the parent and byte-identical to the reviewed component patch;
no integration glue or conflict resolution was added at that merge.

Release binaries were built with locked/offline one-job Cargo and separately
preserved/hashes recorded. Both passed raw smoke (`proc_a190`) and both passed
4,096-frame request-cap checks with valid and duplicate IDs (`proc_eb54`).
Evidence: `.local/evidence/readiness-parent/qualified-release/`.

One preceding-source qualification attempt timed out on the first node reply.
The failure remains preserved. Bounded startup diagnosis and unchanged-source
qualification retry passed without relaxing deadlines. Host contention was
observed, but the timeout's cause is not established. The final-source full run
passed without a retry.

## Established mechanisms, not timing claims

- Zero fuel performs no dispatch, polling, native work or saved-state transfer.
- Unsignalled waiting futures are not polled. Ready/cancelled file guards close
  deterministically; cancelled late wakers cannot revive tasks.
- Fuel bounds dispatched work, not macro expansion, compilation, host callbacks,
  marking, destructor work or wall time.
- Sweep step bounds examined slots; seven-slot probes cover the fixed snapshot.
  Slot counts are not object/byte counts. Marking is still atomic.
- Native cache capacity is 32 entries. Counters demonstrate execution, not speedup.
- Protocol limits: 4,096 requests/session, eight tasks, 32 retained results,
  4,096-byte frames, 4,096 fuel/slice and at most 100ms requested readiness wait.
  These do not establish a global memory bound for arbitrary Lisp programs.
- Default GC may defer destruction of dead payloads until slot reuse/owner drop;
  modern sweep checks eager release. External resource cancellation does not
  depend on eventual payload destruction.

## Remaining boundaries

- Raw foreign heaps must remain **alive and quiescent**. The synchronizer only
  stops registered threads in its world, not unrelated Engines or host threads.
- Native in-progress values retain explicit rooting obligations. Checked import
  capabilities do not expose unrestricted reconstruction caches or raw VM handles.
- Successful pure reference-counted metadata cycles need explicit breaking.
  Failure cleanup is not whole-engine rollback; published namespace/native effects
  remain observable and are never replayed.
- Ordinary VM host unwind is not a supported resume boundary. Budgeted admission
  still rejects context-dependent deserialization/handlers; admitted host unwind
  poisons execution while allowing cancellation. Double panic/process abort and
  panicking cleanup destructors are not promised recoverable.
- No incremental marking, bounded GC pause, general heap-native execution,
  complete IR admission, compact-value ABI or arbitrary hostile-wire qualification.
- Sync plus incremental-gc remains unsupported. Optional biased/triomphe/imbl
  configurations were not independently qualified in this final matrix.
- Timing, allocator bytes, exact root-edge cardinality and isolated marking cost
  are not inferred from mechanism counters or smoke durations.

## Performance gate

Two release configurations are ready for serial measurements across IR modes and
fuel budgets. No timing campaign has run. Busy shell workloads are pinned to
CPU 0 (SMT sibling 8); CPU 4 (sibling 12) is a possible affinity-isolated timing
resource, not an OS-exclusive core or globally quiet host. Approval of that scoped
measurement setup is pending. Preserve affinity, governor, contention, source and
binary hashes with raw samples, and report the limitations before claiming any
comparative result. Stage 0A remains open until qualification decisions are made.
