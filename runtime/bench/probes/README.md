# Isolating steady-state costs

From the repository root (choose an available CPU):

```sh
cargo build --release -p techne-vm --features counting-allocator --bin techne-vm
vm=target/release/techne-vm
probe=runtime/bench/probes
(cd runtime/bench && python3 gen-org.py)
taskset -c 3 "$vm" "$probe/parse-org.scm" runtime/bench/org-sample.org
taskset -c 3 "$vm" "$probe/qsort.scm"
for keys in integer reused fresh; do taskset -c 3 "$vm" "$probe/hash.scm" "$keys"; done
```

Times are microseconds. `parse-org` measures the actual document parser with lines already in memory; heading count and TODO states are printed outside the timer. To compare parser revisions, place the same probe in both source trees and use the same VM, alternately running each revision first. On an Intel N100, five process pairs, median of calls 4–6 per process, then median across processes: 46.819 ms on `5c57f2f`, 42.290 ms with the TODO directive scan change. All parses returned 12,000 headings and the default TODO state. This file has no TODO directive; an early directive avoids most of this scan even before the change.

`qsort` reports fill / sort / sortedness check. One N100 run (medians of calls 6–10): 2.7 / 58.9 / 2.9 ms. `hash` reports 200,000 successful lookups into an already populated table: 11.6 ms integer, 20.3 ms reused string, 37.3 ms freshly converted string. The reused mode includes fetching keys from a vector; the difference isn't a pure conversion cost. These are diagnostic controls, not before/after speedups. Keep the unchanged whole workloads in `steady/` as the acceptance benchmarks.
