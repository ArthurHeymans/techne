# Narrow experiments on the remaining V8 gap

These are historical results from `81f9f72`, not measurements of current
`main`. The short `append` and indentation changes have since shipped.
The JIT instrumentation and other experimental drivers mentioned below
are not part of this recovery; keep the negative results as evidence,
not as reasons to change JIT policy.

## Reproduce the editor startup diagnostic

Build the headless probe with `cargo build --release -p techne-editor
--example profile_syntax`. Use the same binary for both sides. Make two
scratch roots (`before` and `diagnostic`), each with sibling copies of
`lisp/` and `packages/`: the editor loads shipped packages beside its Lisp.
In **only** `diagnostic/lisp/editor/syntax.scm`:

1. Add `#:validate-definitions? [validate-definitions? #t]` to the
   `register-language!` keyword arguments.
2. In that function, pass `(and validate-definitions? definitions)` as
   the final argument of `syntax-check-language`. Leave the original
   `definitions` query text in the registry.
3. Pass `#:validate-definitions? #f` at the bottom of the file, in the
   `register-language!` call inside the `builtin-languages` loop. Do not
   change the default for any other caller.

```sh
techne run runtime/bench/editor-syntax-startup.scm \
  target/release/examples/profile_syntax /path/to/before/lisp /path/to/diagnostic/lisp samples.scm
```

The runner alternates execution order and reports median open, first
snapshot, first edit, first definition jump, and open-plus-snapshot times
in milliseconds. `RUNS` sets the number of pairs (default 7); set `CPU`
to pin both sides with `taskset`. The probe checks the edited buffer and
navigation target. It does not measure a painted frame. The copied Lisp
change is an ablation, not a production patch. See the historical results
under “Tree-sitter startup” below, including the slower first jump.

Baseline `81f9f72`, EPYC 3151, CPU 4. Release, counting allocator, default
asynchronous JIT and nursery policy. Wall experiments use five alternating
process sets, 12 calls per process, medians of the last six calls and then
of the process medians. Answers match. No startup time is included; some
processes were interrupted by host load, so use the raw samples, not a
single process or the first-call result, to judge the magnitude.

## Short `append`: retain as a VM candidate

Callgrind on three baseline nqueens calls (synchronous JIT, includes startup
and compilation) attributed **19.4%** of 2.69 billion instructions to `append`
inclusively. The largest single generated-code symbol was 49.1% exclusively;
that is not a count of instructions spent in call setup. Minor GC was 0.03%
inclusively. This points to `append` first, not a nursery change.

Use a 16-value stack buffer for a short copied prefix instead of allocating a
Rust `Vec`. A Scheme caller cannot remove this temporary inside the native
primitive. The allocator and admission policy, list validation, copying order,
and shared final tail are unchanged. `Bulk::list` may collect; gather values
only after it returns. `Bulk::pair` cannot collect during the copy. The large
case still uses the original `Vec` path. No JIT or guest allocation policy
change is involved.

| Workload | Baseline warm ms | Stack warm ms | Change |
|---|---:|---:|---:|
| nqueens | 254.43 | 238.45 | **−6.3%** |
| qsort (control) | 161.45 | 162.48 | +0.6% |
| HOF (control) | 161.95 | 161.19 | −0.5% |

The 6.3% is the change between campaign medians. Nqueens improved in four of
five pairs, or three of four after dropping the 437 ms baseline pair. It
lost the last pair (254.43 → 259.37 ms). The control deltas are not claimed
as changes. Relative to the earlier warmed
Node 24.21.0 nqueens control (142.91 ms on this host), the candidate is still
about **1.67×** as slow; this is not a new paired V8 measurement.

## Org prefix checks: application variant, not a VM win

All variants use the same baseline VM, input, and result. The first replaces
length/substr/equality with the existing `string-prefix?`. The second also
checks the fixed `TODO ` marker in place at the heading offset instead of
copying the entire suffix. The benchmark's original source is unchanged.

| Variant | Warm ms | First call ms |
|---|---:|---:|
| Original | 62.78 | 169.76 |
| Native prefix predicate | 59.07 | 157.73 |
| Also compare the TODO offset in place | **58.02** | 174.28 |

The prefix-only change saves about 6% warmed. Removing the suffix copy saves
another ~1 ms in these medians, but that is suggestive: the discovery run
always launched `prefix` second, first-call time did not improve, and there
were large outliers. The runner now rotates launch positions across six sets
by default for Org. This does not justify a new Rust
string primitive. Try the prefix predicate where an application actually
copies to test a prefix, and measure that application independently.

## Qsort and callbacks: next VM experiments

A diagnostic-only qsort variant timed before/after the fill, recursive sort,
and validation, preserving their original operations and checking the answer
on every iteration. Same five-process/12-call protocol, baseline VM vs Node.

| Phase | Node ms | Techne ms |
|---|---:|---:|
| Fill | 3.39 | 9.25 |
| Recursive sort (including partition) | 97.61 | 144.18 |
| Validate and format result | 1.08 | 7.40 |

The sort itself accounts for about **79% of the summed phase gap**. To split
that phase without timing every call, profile the unchanged workload with
Callgrind and log actual JIT address ranges. Count exclusive instructions,
not inclusive call arcs. Subtract a one-call process from a three-call
process (synchronous JIT, same threshold and input); the difference is a
proxy for two warm calls, not an isolated warm-up-free capture.

| Code range | One call Ir (M) | Three calls Ir (M) | Share of difference |
|---|---:|---:|---:|
| `partition` (both `swap!` sites inline) | 467.05 | 1,401.28 | **79.1%** |
| Recursive `qsort` driver | 51.77 | 155.52 | **8.8%** |
| `sorted?` | 39.17 | 117.77 | 6.7% |
| `run` | 28.11 | 84.52 | 4.8% |
| Out-of-line `swap!` | 0 | 0.04 | <0.1% |
| Outside named JIT ranges | 78.11 | 85.82 | 0.7% |

Among the generated instructions in the sort phase, about **90%** are in
`partition` including its inlined swaps. The recursive driver's share is
not all call overhead, and partition's range also includes its entry and
exit machinery. This is instruction attribution, not a wall-time profile
or an estimate of removable checks. A general call-ABI change is not the
first target for this workload.

### Inside `partition`

`TECHNE_JIT_PROFILE_OPS=partition` labels lowering operations using Cranelift
source locations, with no probes in generated code. Subtracting the same
one-call run from a three-call run gives the following distribution within
`partition` alone (934.23M exclusive instructions in the difference).
The function's three-call count matched the prior count exactly with and
without source locations.

| Lowering category | Exclusive Ir (M) | Share |
|---|---:|---:|
| Comparisons, including numeric dispatch | 121.35 | 13.0% |
| Arithmetic, including counter increments | 75.20 | 8.0% |
| Vector bounds and index tag guards | 77.65 | 8.3% |
| Index conversion | 52.67 | 5.6% |
| Element address and load/store | 60.98 | 6.5% |
| Fuel polls | 44.43 | 4.8% |
| Write-barrier lowering | 35.35 | 3.8% |
| Call lowering, excluding fuel | 17.20 | 1.8% |
| Vector kind/extent lowering | 1.41 | 0.2% |
| Other or no location | 448.00 | 48.0% |

These are labels on optimized machine instructions, not cycle shares or a
measure of removable work. Index conversion is also needed for element access;
counting that entire category as redundant bounds work would overstate the
opportunity. The large other/unmapped group includes control flow, moves,
and code without a useful origin. No claim is made about its breakdown.

Two probes did not justify a production change:

- **Write the swaps inline in Scheme.** This keeps the vector operations and
  their order but bypasses the guarded inline call. Seven alternating pairs,
  12 calls each, warmed medians **161.26 → 160.58 ms**. Five pairs improved and
  two regressed; the 0.4% median change is not a clear win. Not a VM change.
- **Compare byte offsets for proven fixnum indices.** Compute the offset as
  `shifted >>s 13` and compare it unsigned against `checked_length << 3`,
  instead of `shifted >>s 16` against the word count. Both versions apply the
  same check, reject negative indices, and keep the same fallback. Seven
  alternating pairs with the synchronous `compare.scm` runner: qsort
  **160.34 → 167.50 ms (+4.5%)**; nqueens and mandel within 0.2%. The
  prototype was removed. This does not test a loop-level range proof.

A second annotation pass split the comparison category, again using the
three-call minus one-call counts. Of the partition's 934.23M instructions,
**66.89M (7.2%)** were attributed to numeric dispatch and **54.46M (5.8%)**
to the comparisons and their branches. No instructions landed in the
separate operand-conversion bucket: this is source attribution, not evidence
that Cranelift emits no shifts. The total partition count is unchanged.

Hoisting the pivot's type check can only remove a subset of that 7.2%: the
loaded element still needs a check and the mixed-number path still needs to
exist. That is not enough evidence to add a new version of the loop for qsort.

With `TECHNE_JIT_SYNC=1 TECHNE_JIT_IR=drive TECHNE_JIT_ASM=drive`, the
successful indirect inline path still tests the cached boolean each turn and
has more stack traffic in its loop than the direct control. Both have overflow
branches and fuel polls. A bounded guarded hot-loop version could avoid the
per-turn callee branch while preserving the existing generic path, fuel,
and deoptimization; the disassembly does not establish the size of any gain.

The same exclusive-instruction analysis on the unchanged direct and indirect
controls, using five calls minus one, gives `drive` **128.07M vs 144.07M Ir**.
That is about **four extra instructions per loop iteration** (`+12.5%`),
whereas the prior wall gap between the controls was about 38%. The fuel,
call, comparison and arithmetic labels counted the same on both sides;
about three of the four extra instructions are outside any lowering label.
For this profile use `TECHNE_JIT_PROFILE_OPS=drive` with `direct` and
`indirect`, at `ITERS=1` and `5`.

### Exact machine-code attribution for indirect callbacks

Joining the exclusive counts to decoded instruction offsets from the **same
process** accounts for the four hot-loop instructions on x86-64:

- Two loads from the same native stack slot feed the two operands of the
  guard's `test`. The direct control's guard uses a register, and both paths
  already have a conditional branch.
- The other two are a reload and a spill of a different live value around the
  successful inline path and the back-edge. There is no extra native call on
  this path. The duplicated load is an allocation/lowering detail, not a
  second Scheme-level guard.

The simple prototype compared the cached 0/1 value to one rather than testing
it for nonzero. It removed one stack load but the x86 code inserted an
immediate materialization, keeping the hot instruction count effectively
unchanged. In a synchronous screening run, indirect improved **9.40 → 8.19 ms**;
that run also moved the direct control by 3%. On the default asynchronous VM,
seven alternating pairs of 12 calls gave **6.0265 → 6.0305 ms** for indirect
and **4.354 → 4.356 ms** for direct. HOF and qsort medians were within 1%.
The prototype was removed. A loop-versioning experiment would need to show
that it reduces spill pressure as well as removes the guard, without losing
the normal fallback. We did not implement loop cloning.

## Real editor workloads: indirect calls were not the bottleneck

On CPU 4, seven fresh-process pairs in alternating order, release
counting-allocator builds with the default background JIT. `startup` opens
an empty editor and loads its shipped Lisp/packages; its time includes the
whole process. `profile_indent` selects a 7,168-line Scheme buffer and uses
its normal `indent-region` key binding in a headless editor. Commands are
checked against the complete expected text and undone between calls. Warm
command time is the median of the last two of four calls, then the median
across processes. Setup, result verification and undo are outside that time.

| Indirect-call cache | Startup (ms) | First indent (ms) | Warm indent (ms) |
|---|---:|---:|---:|
| On | 459.77 | 410.82 | 371.73 |
| Off | 462.12 | 411.17 | 371.41 |

No `jit-call-cache` admission appeared in either synchronous diagnostic log;
`jit-native-cache` is a different feature. The ablation and logs are evidence
against prototyping loop versioning for these two workloads, not a general
claim about callback cost in the editor.

Callgrind scoped to one indent command counted 1.43B exclusive instructions
in total. `mem_generic` accounted for **88.0% inclusively**, with `equal`
accounting for 48.5% inclusively inside that scan (do not add these). The
source sorts line positions and then applies generic `delete-duplicates`,
which scans all retained positions. Replace this quadratic scan with a
single Scheme pass over adjacent positions, preserving sorted order.

A same-binary comparison using the before/after Lisp trees and the same
seven-pair wall protocol gave first-command medians **410.88 → 127.65 ms**
and warmed medians **373.26 → 73.85 ms (5.05×)**. A command-scoped profile
under the same synchronous settings fell to **171.79M instructions**, down
88%. The production change is Scheme-only; the Rust example only hosts an
actual editor session for the measurement. No JIT specialization was added.
The indent test also checks multiple cursors sharing a line and undo.

Startup's largest exclusive hotspot was Tree-sitter query analysis: about
**34% of 687.70M instructions**, including the inlined rows attributed to
`language.h`. That makes query initialization a better startup lead than
callback inlining; first determine which queries are needed before the
first command before considering deferring them.

To reproduce the editor runs, build `cargo build --release -p techne-editor
--example startup --example profile_indent` and use:

```sh
techne run runtime/bench/editor-callbacks.scm target/release/examples/startup \
  target/release/examples/profile_indent /tmp/editor-pairs.scm
# Set LISP_BEFORE and LISP_AFTER to absolute Lisp directories to compare
# the indentation sources instead of ablating callback caching.
```

For the diagnostic profile use `TECHNE_JIT_SYNC=1 TECHNE_JIT_LOG=1
TECHNE_NURSERY_ADAPT=0` and `valgrind --tool=callgrind`; for indentation
also pass `--toggle-collect=profile_indent::indent_region` and run the
example with `1024 1`. The whole startup profile does not use the toggle.

## Tree-sitter startup: definition validation versus highlighting

The default grammars' highlight queries are already lazy. The empty editor
startup compiled 11 **built-in definition queries**, solely to validate them
at registration. No highlight or execution-query compilation appeared in
that log. The compiled definitions were discarded; first use goes through
`Syntax::compiled` and compiles the definition again on cache miss. One pinned
startup sample spent 121 ms in validation, with TypeScript and TSX the most
expensive at about 24 ms each, followed by Bash at 20 ms. The two TypeScript
entries are separate grammars even though their query text is the same.

For a diagnostic ablation, copy the Lisp tree and skip only built-in
**definition** `Query::new` during the final `builtin-languages` registration
loop in `lisp/editor/syntax.scm`. Pass `#f` in place of `definitions` to
`syntax-check-language` in that loop only; still store and use the real
query text. This does not skip custom-query validation and is **not** a
production change. Use the same release `profile_syntax` binary for both
Lisp trees. The probe opens a small Rust file, takes the first snapshot,
checks that an edit produces the expected text, and checks the first
`C-M-a` jump lands on the right definition.

| Phase | Before (ms) | Diagnostic skip (ms) |
|---|---:|---:|
| Open editor on Rust file | 460.77 | 344.47 |
| First snapshot | 94.27 | 95.67 |
| Open plus first snapshot (median of sums) | 554.70 | 440.05 |
| First edit plus snapshot | 1.15 | 1.13 |
| First definition jump | 11.58 | 16.38 |

Seven alternating process pairs on CPU 4, default JIT, timing logger off
for these measurements. The open-plus-first-snapshot win was present in
all seven pairs. The first edit did not regress in this probe; the first
jump **did** get slower here, even though both traces compile exactly one
Rust definition query at that point. Do not claim zero interaction cost
or ascribe that delta to moving compilation from startup: both paths
already compile it there. Rust highlighting alone took about 89–93 ms in
separate annotated first-snapshot samples. First-snapshot timing is not a
measurement of painted or fully highlighted output.

Candidate: compile every shipped definition query against its actual
bundled grammar in CI, then omit runtime revalidation for those shipped
entries. Retain fail-fast registration for user/package queries, and
measure first navigation across more than Rust before shipping. Treat
Rust's cold highlight compilation as a separate first-frame issue; do not
move it onto the first edit. The worktree has only the opt-in attribution
hook, not the diagnostic ablation in production Lisp.

```sh
cargo build --release -p techne-editor --example startup --example profile_syntax
TECHNE_SYNTAX_QUERY_TIMING=1 target/release/examples/startup
TECHNE_SYNTAX_QUERY_TIMING=1 target/release/examples/profile_syntax
techne run runtime/bench/editor-syntax-startup.scm \
  target/release/examples/profile_syntax BEFORE_LISP DIAGNOSTIC_LISP /tmp/syntax-startup.scm
```

## Reproduce

Freeze a release/counting-allocator binary at `81f9f72` before building the
candidate. The Scheme driver assembles variants under `runtime/bench/build/`,
checks answers, and optionally writes individual sample times. For `org`,
place the fixture at `runtime/bench/org-sample.org` (see `gen-org.py`).

```sh
techne run runtime/bench/v8-gap.scm BEFORE AFTER append /tmp/append.scm
techne run runtime/bench/v8-gap.scm BEFORE BEFORE org /tmp/org.scm
techne run runtime/bench/v8-gap.scm BEFORE BEFORE qsort /tmp/qsort-phases.scm
RUNS=7 techne run runtime/bench/v8-gap.scm BEFORE BEFORE qsort-inline /tmp/qsort-inline.scm
```

`PROGS` controls the append campaign; `RUNS`, `ITERS`, and `CPU` work for
all wall-time modes. `callback-guard` reproduces the immediate-compare
ablation with the frozen prototype binary (which alone supports
`TECHNE_JIT_CALL_CACHE_IMM`). End-to-end Scheme tests exercise both sides of the small-buffer
cutoff, distinct heap cars in order, freshness of the copied spine, sharing
of the dotted tail, improper/cyclic prefix rejection, and normal,
minor-stress, and full-stress GC. Release library, language, nursery
adaptation, and reclamation tests also passed.

For instruction attribution, assemble each unchanged input by concatenating
`runtime/bench/prelude/techne.scm`, `runtime/bench/steady/NAME.scm`, and
`runtime/bench/steady/driver.scm` in that order. Run with a release binary
built with counting-allocator. Example for qsort, repeat with `N=3` using
separate output files. `VM` must include the opt-in range logger.

```sh
N=1
nix develop --command taskset -c 4 env ITERS=$N TECHNE_JIT_SYNC=1 \
  TECHNE_JIT_PROFILE_RANGES=1 valgrind --tool=callgrind --dump-instr=yes \
  --callgrind-out-file=/tmp/qsort-$N.callgrind "$VM" /tmp/qsort.scm 2>/tmp/qsort-$N.log
techne run runtime/bench/jit-callgrind.scm /tmp/qsort-$N.log /tmp/qsort-$N.callgrind
```

The range logger runs only at compilation and reports the ordinary generated
entry, not its optional numeric companion kernel. The analyzer requires the
log from the same process and rejects overlapping ranges. It reconciles
exclusive Ir with the profile total. To reproduce the partition breakdown,
add `TECHNE_JIT_PROFILE_OPS=partition` to the Valgrind process environment,
then pass `ops` as the last analyzer argument. Subtract the mapped totals
from the function's exclusive count to account for unmapped instructions.
For machine-code attribution, use `TECHNE_JIT_PROFILE_CODE=drive` and
`ITERS=1` and `5` with both callback controls, then run the analyzer as
`techne run runtime/bench/jit-callgrind.scm VM.log callgrind.out asm CODE.bin`.
This mode uses GNU objdump on x86-64. It verifies the decoded instruction
counts cover the recorded function; the bytes precede external relocations,
so use offsets for attribution, not absolute external call targets.

For the earlier nqueens profile, use `ITERS=3`, no range logging or
`--dump-instr`, the frozen baseline VM, and `callgrind_annotate --inclusive=yes
--auto=no` on the result.
