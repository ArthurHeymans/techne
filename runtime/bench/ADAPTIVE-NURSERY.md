# Opt-in survivor-sized nursery (`TECHNE_NURSERY_ADAPT=1`)

Status: opt-in checkpoint, **off by default**. No default, workload or limit
changes. Its effect on work and wall time is measured below; it is not
proposed as a default while a workload it does not help still grows (see
"Known weakness").

## Why

[NURSERY.md](NURSERY.md) measured where the gap is and is not: at the default
8 MiB nursery hof spends about 60% of its warm time in recorded collection and
allocates ~15 nurseries' worth per call, so each collection re-copies its live
young set. qsort, mandel, fib, tak and orgparse collect nothing or almost
nothing, and nqueens collects often but promotes ~nothing.

## What it does

`Heap` keeps a base capacity (`TECHNE_NURSERY_KB`, else 8 MiB) and, when
`TECHNE_NURSERY_ADAPT=1` is set *and* no explicit `TECHNE_NURSERY_KB` pins the
capacity, a ceiling (`TECHNE_NURSERY_ADAPT_KB`, default 4x base = 32 MiB; a parsed value is
clamped to 256 MiB, one that does not parse or overflows falls back). After a collection it aims the next window at
`8 x` the words that collection copied:

- more than the current target → take the estimate in one step;
- at most half the current target → halve it;
- otherwise leave it alone.

The target is clamped to `[base, ceiling]`.

**Only a collection caused by running out of window moves the target**
(`heap.rs`, `window_exhausted`). Every forced collection leaves it alone:
boot's, `collect-garbage`'s, one from GC stress (where no allocation ever has
room) or from `request_collection` (`Heap::forced_collection`, covered by a
unit test and by `tests/nursery_adapt.rs::gc_stress_does_not_grow_the_nursery`),
and a bulk reservation's own collection.
That is both what the policy is about — a nursery that is too small is one
that fills up — and half of a correctness requirement. A bulk reservation is
checked against the nursery capacity and then collects to make its span. The
gate stops that collection from changing the target; it does not stop an
*already-pending* shrink target (one an earlier resize was refused) from being
applied, so `Vm::reserve_nursery` also holds the span across that collection
(`Heap::hold_reserve`, honoured by `resize_nursery`), and asserts afterwards
that the capacity still covers it.
`heap::tests::a_bulk_reservation_holds_the_capacity_across_its_collection`
drives that path through `Vm::reserve_nursery`; it fails if `resize_nursery`
ignores the floor.

Only the target moves during the collection. The reservation is resized at the
one place collections already resize (`Vm::collect_with`), with the nursery
empty, through the existing `nursery_growth() <= room()` admission: growth is
refused rather than forced when the world's budget or cage has no room, and
the small nursery serves on. Shrinking allocates the smaller region before
releasing the old one, so a refused allocation leaves the current one intact.
`GcStats::nursery_words` and `nursery_target_words` report what the last
collection settled on, and are recorded after every collection, including one
whose resize was refused.

## Mechanism, not a policy result

- `heap::tests::a_survivor_sized_window_grows_to_the_ceiling_and_shrinks_back`
  covers the growth and shrink rules, the clamps, both resize directions, a
  collection that was not a window exhaustion, and a policy with no room to
  grow.
- `tests/nursery_adapt.rs` runs separate processes, because the policy is read
  from the environment when a heap is made, and the pin test fills the pinned
  16 MiB many times over so a policy that adapted would have to grow. It covers the bulk-reservation
  collection above (observed failing with the gate removed: "a bulk
  reservation outgrew the nursery it was checked against"), an explicit
  `TECHNE_NURSERY_KB` pinning the nursery and disabling adaptation, an
  `TECHNE_NURSERY_ADAPT_KB` that overflows when converted falling back instead
  of wrapping, and
  the default (off) nursery staying at its base after the prelude's forced
  collection, and GC stress not growing an opt-in one. A unit test covers the
  forced/requested distinction itself.

## Measurements

hof, unchanged steady workload, same binary, the environment the only
difference.

| | default 8 MiB | `TECHNE_NURSERY_ADAPT=1` |
|---|---:|---:|
| whole-process instructions, 2 calls (Callgrind) | 1,947,448,207 | **1,222,220,382** (**−37.2%**) |
| wall ms/call (3 alternating pairs, median of last four of 6) | 318.6 | **149.1** (**−53%**) |
| minors per call | 16 | 4 |
| recorded GC ms/call | 189.4 | 48.5 |
| peak RSS, median of three `ITERS=6` processes | 97.2 MiB | 117.9 MiB |

The capacity settled at its 32 MiB ceiling. Instruction counts are
load-insensitive and carry the mechanism claim; the wall figures happened to
be stable here (318.1–319.2 vs 148.8–149.3 ms) but this host was at load
average 2–5 from unrelated work, so treat them as one campaign, not a quiet
benchmark. Recorded GC time excludes the nursery replacement itself, which
happens after the timed collection.

Controls: nqueens (5.5 collections per call, ~nothing promoted), qsort,
mandel, fib and tak stayed at the base capacity — nqueens 243.8 vs 244.4 ms,
qsort 162.8 vs 159.9 ms, both inside this host's noise.

For context only: the warmed Node 24.21.0 control for the hof program in
[STRINGS.md](STRINGS.md) is 157 ms, from a different campaign whose JavaScript
port uses arrays where this source uses lists. This does not establish parity
or isolate arithmetic and call overhead; those are still owed
([NURSERY.md](NURSERY.md)).

### Known weakness

A rule based on survival alone also grows for a workload that allocates
little: orgparse reached 31 MiB with 1 instead of 3 collections in 10 calls,
no measured gain, and +7.8 MiB peak RSS. Growth should also require that the
window filled more than once since the last resize, i.e. collection
frequency, not survival alone. Until then the ceiling
(`TECHNE_NURSERY_ADAPT_KB`) is the knob, and the default off is the
recommendation.

## Environment

| Variable | Meaning |
|---|---|
| `TECHNE_NURSERY_ADAPT=1` | opt in; ignored when `TECHNE_NURSERY_KB` is set |
| `TECHNE_NURSERY_ADAPT_KB` | ceiling in KiB, default `4 x` base. A value that parses is clamped to 256 MiB; one that does not parse, or that overflows when converted to words, falls back to the default |
| `TECHNE_NURSERY_KB` | pins base and capacity, as before; disables adaptation |

## Reproduce

```sh
nix develop --command cargo build --release -p techne-vm --features counting-allocator

# Work, not wall time: same binary, the environment is the only difference.
# Assemble build/steady as in NURSERY.md first (gen-org.py, steady/run.sh).
nix develop --command sh -c '
  for a in 0 1; do
    if [ "$a" = 1 ]; then export TECHNE_NURSERY_ADAPT=1; else unset TECHNE_NURSERY_ADAPT; fi
    ITERS=2 TECHNE_JIT_SYNC=1 valgrind --tool=callgrind \
      --callgrind-out-file=/tmp/cg-hof-$a.out \
      target/release/techne-vm runtime/bench/build/steady/hof.scm >/dev/null 2>/tmp/cg-hof-$a.log
    echo "adapt=$a $(grep "I   refs" /tmp/cg-hof-$a.log)"
  done'

# Capacity and target of the last collection, with the rest of the statistics.
ITERS=6 TECHNE_GC_STATS=1 TECHNE_NURSERY_ADAPT=1 \
  target/release/techne-vm runtime/bench/build/steady/hof.scm
```
