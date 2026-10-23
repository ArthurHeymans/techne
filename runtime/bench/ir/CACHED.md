# Logical frames and compiled-world reuse

Follow-up to [the end-to-end experiment](END-TO-END.md).

## Scope

This change fixes the debugging blocker and tests compile-once reuse through
Techne's **existing in-process `Snapshot` mechanism**. It does **not** implement
the persistent per-file code cache or lazy package loader from the
`doc-startup` design. Snapshot timings are not predictions of warm file-cache
startup: a snapshot copies initialized state; a file-cache load must relocate
code and still run top-level initialization.

The roughly 2.2 ms uncached startup overhead previously measured is modest for
a long-lived editor. Caching makes optimization's compilation cost amortizable;
it need not earn that cost back during startup itself. Representative execution
benefits, GC work, and latency on the first interactive use remain the important
questions. The experimental passes remain opt-in.

## Logical frames

Lexical inlining and contification now attach an immutable `InlineMap` to
bytecode that actually needs one:

- Sparse PC intervals identify the executing source function and its lexical
  caller. Names, source byte positions and local indices contain no heap
  values or machine-code addresses.
- Source tail position is recorded **before** moving a function body into a
  caller's register destination. A logical tail call need not be a physical
  tail call after inlining.
- Failed calls retain their logical caller; saved callers omit a virtual
  frame only after entering the callee. Handler trampolines do not pretend to
  have entered that callee. Ordinary interpreter and JIT unwinding use the
  same metadata.
- Contified branches carry their original function names and inherit the
  entry call's continuation. Tail-recursive dispatch does not manufacture
  a growing history of frames.
- Trace limits count logical source frames rather than physical frames.
  Trace expansion happens on error, not on each instruction. Range generation
  happens at scope boundaries, not on ordinary instruction emission.

`Code` has one optional shared pointer; code without rewritten calls allocates
no inline table. The tables are immutable and shared by `Code::instance_copy`.
Current tree inlining is restricted to the same source file, so the existing
`Code::file` hold and relocation also cover these positions. JIT state remains
world-local and is not part of this metadata.

This preserves real frames for execution, suspension, GC and scheduling;
it does not create a second runtime stack. Existing fuel checkpoints stay.
It is not a claim of exhaustive equivalence for every introspection or
resource-exhaustion program.

## What was exercised

[`ir_frames.rs`](../../../crates/techne-vm/tests/ir_frames.rs) compares entire
stderr, stdout and exit status against an unoptimized interpreter, with
`local`, `contify` and `all`, each in interpreter and first-call JIT modes.
Cases cover tail/non-tail nesting, calls through other physical functions,
wrong arity/non-procedures, mutual recursion, deep truncated traces, re-raising
handlers and a task failure delivered after a wait.

The optimizer differential fuzzer now compares **complete diagnostics**, not
just the first error line. Snapshot tests additionally warm native code,
freeze it, destroy the original snapshot, collect sibling worlds, compare
traces in interpreter/JIT execution, redefine a global in one sibling, and
snapshot an instantiated world again. This checks both debug metadata reuse
and live redefinition rather than treating a cached global as immutable.

## Measurements

The new [`ir_cache`](../../../crates/techne-vm/examples/ir_cache.rs) example
loads unchanged shipped libraries and checks results against independent Rust
integer oracles:

- SRFI 1: map/filter/fold pipeline over lists.
- SRFI 133: vector map/fold pipeline.
- SRFI 113/128/69: comparator-backed set construction and union.

Each run separates source compilation/library loading, snapshot freeze,
world instantiation, first invocation, and warmed calls. It checks every call.
A first invocation includes any native compilation it triggers; that cost is
not hidden in the warmed-call column. The JIT configuration uses synchronous
first-call compilation (`TECHNE_JIT=1`) to make that distinction explicit;
it is not the ordinary asynchronous threshold of 1,000.

The campaign compares disabled passes, `bindings,local,scalar,dead`, and all
passes using the same frozen binaries. Five process samples rotate the pass
order. Each uses 128 elements, five fresh snapshot worlds, ten warm-up calls,
and thirty measured hot calls per world. The report takes medians across
worlds, then across processes. The existing editor startup and keystroke
workloads are also run; startup uses eleven interleaved process samples and
keys use three complete runs (each has its own five rounds). Editor runs use
the ordinary JIT threshold, not first-call synchronous compilation.

The callback GC workload is repeated with each pass individually and with
the two combinations, using synchronous installation at the ordinary
threshold, as in the original instruction campaign. Its pause-work bound is
`max_copied_words + max_slice_words`; the maxima need not be from one pause.
Do not confuse it with measured pause duration or total allocation.

Raw observations and binary hashes are in
[`results/cached.json`](results/cached.json). These are machine-specific wall
timings, not instruction-count results.

### Results

`selected` below means `bindings,local,scalar,dead`.

| SRFI workload | Source/library load, ms: off / selected / all | Warm world, ms: off / selected / all |
|---|---:|---:|
| list | 0.863 / 0.949 / 0.977 | 0.110 / 0.127 / 0.109 |
| vector | 2.335 / 2.659 / 2.696 | 0.170 / 0.191 / 0.184 |
| set | 3.816 / 4.201 / 4.249 | 0.771 / 0.826 / 0.935 |

These are the first-call JIT configuration's medians. `compile_us` in the
JSON means the **whole `eval_in` load**, including reading, expansion,
compilation and top-level initialization, not isolated optimizer time.
Snapshot reuse skips that work, but cloning initialized state is not free
and can itself grow with optimization. Freezing the snapshot once costs
about 0.21–0.43 ms in this configuration.

| Warm JIT invocation, µs | Off | Selected | All |
|---|---:|---:|---:|
| list | 10.62 | 10.69 | 10.52 |
| vector | 26.56 | 26.38 | 26.32 |
| set | 888.60 | 896.58 | 893.35 |

**No meaningful recurring speedup here:** differences are below 1%, with
interpreter differences similarly small. Caching amortizes compilation;
it does not create an execution benefit in code these passes barely change.
Synchronous native compilation makes the first invocations take roughly
7.6–7.7 ms, 17.2–17.3 ms and 42.2–42.3 ms respectively, even after source
compilation was cached. That is a useful stress demonstration of why
"already compiled" and "no first-use latency" are different claims, not a
measurement of the default editor's first keystroke.

Uncached editor startup is **19.98 / 22.23 / 22.57 ms** for off / selected /
all. The approximately 2.3–2.6 ms addition still is not the decisive objection.
Keystroke measurements have no consistent broad gain: median typing stays
0.06 ms, and search narrowing is 0.42 / 0.43 / 0.42 ms. The raw tail timings
vary enough across the three runs that individual improvements should not
be promoted as established application wins.

### GC interaction isolated

The smallest reproducer found is **`bindings,dead`**, not lexical inlining:

| Selection | Maximum copied + marked words |
|---|---:|
| off; each of the eight passes alone | 327,538 |
| `bindings,dead` | 371,278 |
| `bindings,local,scalar` | 327,538 |
| `local,scalar,dead` | 327,538 |
| selected; all | 371,278 |

All tested pairs/triples of the four selected passes reproduce the increase
only when both `bindings` and `dead` are present. The 21 interventions are
recorded in the artifact. Both baseline and reproducer perform seven minor
collections and no full collection; promoted words increase from 921,007 to
958,147. Thus this is not simply more collections. Allocation timing and
register-root liveness initially needed a focused follow-up. The subsequent
[JIT root-retention fix](GC.md) identifies stale register-memory pointers and
removes this increase without disabling either pass. The table and artifact
above remain the original, pre-fix measurements. The selected combination
still needs representative execution benefits before default enablement.

### Validation

The final implementation passes VM and editor release library/integration
tests with `TECHNE_IR=all`, the focused full-trace comparisons, a fresh
100-seed optimizer differential campaign, independent kernel oracles with
first-call JIT plus GC stress/verification, and snapshot-backed SRFI requests
under GC stress. VM library/examples/tests pass release Clippy with warnings
denied; Rust formatting also passes. These tests are stronger evidence than
the original message-only checks, not proof of all observable equivalences.

## Persistent-cache integration boundary

The startup design remains appropriate: optimize once while filling a cache,
not repeatedly on world startup. The new frame metadata fits that artifact
without retaining the source VM. For a persistent implementation:

1. Include the **normalized optimizer flags/budgets and debug-metadata format**
   in validity, alongside compiler/bytecode version, source, prelude and
   dependency macro/interface hashes. Never serve unoptimized or differently
   optimized code under the same key.
2. Serialize inline names, source offsets, local scope indices and PC ranges
   with the code. Relocate the code's source-file reference. Keep native PCs,
   JIT arenas, live frames and world addresses out of the artifact. Validate
   PC ranges and acyclic, in-bounds scope indices when decoding.
3. Preserve dynamic global lookup and the existing definition-identity guards;
   a cache hit is not permission to freeze mutable globals.
4. Treat source-cache misses and first-use native compilation as distinct
   latency events. Do heavier cache filling at installation/prewarming where
   possible; laziness must not move a cold optimizer onto the first keystroke.
5. Measure cold source loads, warm relocated file loads **including top-level
   initialization**, lazy first use and ordinary editor input separately.

No new cache invalidation, disk format, asynchronous compiler or autoload API
is being claimed by the snapshot experiment.

## Reproduce

```sh
cargo build --release -p techne-vm --features counting-allocator \
  --bin techne-vm --example ir_cache --example ir
cargo build --release -p techne-editor --example startup --example keys

taskset -c 4 python3 runtime/bench/ir/cached.py \
  --binary target/release/examples/ir_cache \
  --startup target/release/examples/startup \
  --keys target/release/examples/keys --vm target/release/techne-vm \
  --out .local/ir-cached.json

TECHNE_IR=all cargo test --release -p techne-vm \
  --features counting-allocator --lib --test ir_frames
TECHNE_FUZZ_SEEDS=100 cargo test --release -p techne-vm \
  --features counting-allocator --test fuzz \
  experimental_ir_matches_unoptimized_interpreter -- --ignored
```
