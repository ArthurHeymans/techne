# Isolating steady-state costs

From the repository root (choose an available CPU):

```sh
cargo build --release -p techne-vm --features counting-allocator --bin techne-vm
vm=target/release/techne-vm
probe=runtime/bench/probes
(cd runtime/bench && python3 gen-org.py)
taskset -c 3 "$vm" "$probe/parse-org.scm" runtime/bench/org-sample.org
taskset -c 3 "$vm" "$probe/qsort.scm"
for keys in integer reused fresh convert; do taskset -c 3 "$vm" "$probe/hash.scm" "$keys"; done
```

Times are microseconds. `parse-org` measures the actual document parser with lines already in memory; heading count and TODO states are printed outside the timer. To compare parser revisions, place the same probe in both source trees and use the same VM, alternately running each revision first. On an Intel N100, five process pairs, median of calls 4–6 per process, then median across processes: 46.819 ms on `5c57f2f`, 42.290 ms with the TODO directive scan change. All parses returned 12,000 headings and the default TODO state. This file has no TODO directive; an early directive avoids most of this scan even before the change.

`qsort` reports fill / sort / sortedness check. One N100 run (medians of calls 6–10): 2.7 / 58.9 / 2.9 ms. `hash` reports 200,000 successful lookups into an already populated table: 11.6 ms integer, 20.3 ms reused string, 37.3 ms freshly converted string. The reused mode includes fetching keys from a vector; the difference isn't a pure conversion cost. These are diagnostic controls, not before/after speedups. Keep the unchanged whole workloads in `steady/` as the acceptance benchmarks.

## Follow-up on `0e47d8e` (Intel N100, CPU 3)

With synchronous compilation, medians of three process medians (calls 6–10): integer lookup 11.2 ms, reused 20.8 ms, fresh 36.7 ms, decimal conversion plus length checksum without a table 3.4 ms. For allocation counts, rebuild with `--features counting-allocator,allocation-census` and set `TECHNE_ALLOC_CENSUS=1 TECHNE_JIT_SYNC=1`. Subtract the first `allocation-census:` snapshot from the second, which bracket the tenth call. `convert` and `fresh` each allocated 200,000 nursery objects / 3,200,000 bytes, none promoted. `integer` and `reused` allocated zero. This doesn't assign the remaining difference entirely to equality: key locality and vector fetches also differ.

A rejected VM experiment specialized `equal?` table probes for string keys. Compare frozen ordinary release binaries with `CPU=3 PROGS=hash RUNS=6 ITERS=10 techne run runtime/bench/compare.scm BEFORE AFTER`. In six alternating pairs the unchanged hash result was identical; warmed median 160.68 → 154.46 ms, first call 146.27 → 139.81 ms. In a separate two-call run Cachegrind counted 1.090B → 0.994B whole-process instructions (includes boot and JIT). However, four alternating pairs on the integer-only lookup probe regressed from 11.33 to 15.26 ms (+35%), and whole-process instructions rose 12%. The specialization is not included; future candidates must pass the integer control as well as the whole workload. Concurrent system load affects absolute times. Two pairs on the other eight steady workloads were smoke checks, not evidence of further speedups.

Quicksort diagnostic on the baseline VM: keep `steady/qsort.scm` unchanged as the control. In temporary copies of `partition` only, first expand `(swap! i j)` in place while retaining both `v[j]` reads; then bind `x = v[j]` before the comparison and use `x` in the first store. Leave the pivot swap intact. For each of the six permutations of control/inline/reuse, run ten calls per process on CPU 3 (`TECHNE_JIT_SYNC=1`). Median warmed times: 64.98 / 66.42 / 63.75 ms; first calls 77.97 / 78.41 / 75.21 ms. All returned `(#t 32758 65535)`. `TECHNE_JIT_IR=1` reported 1484 / 1193 / 1106 instructions for `partition`: the baseline already inlines both swap calls. Fewer static instructions alone did not predict speed. No benchmark rewrite or JIT rule retained.
