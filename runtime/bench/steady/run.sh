#!/usr/bin/env bash
# Steady-state campaign: each program's work, run ITERS times in one process
# per implementation; prints the median of the second half of the iterations
# (ms). Pinned to one core like ../run.sh.
set -euo pipefail
cd "$(dirname "$0")"
TECHNE_VM=${TECHNE_VM:-$(realpath ../../../target/release/techne-vm)}
# The techne command, which takes the medians (median.scm). Without
# TECHNE_CLI, the release build's, made if missing: without frontends,
# which this does not need (a build with them replaces it).
if [[ -z ${TECHNE_CLI:-} ]]; then
  TECHNE_CLI=$(realpath -m ../../../target/release/techne)
  [[ -x $TECHNE_CLI ]] || cargo build --release -q -p techne-cli --no-default-features
fi
[[ -x $TECHNE_CLI ]] || { echo "no techne command at $TECHNE_CLI" >&2; exit 1; }
CPU=${CPU:-4}
B=../build/steady; mkdir -p $B; cp ../org-sample.org ../build/
for p in ${PROGS:-fib tak nqueens bintrees hof qsort mandel hash orgparse}; do
  cat ../prelude/techne.scm $p.scm driver.scm > $B/$p.scm
  cat $p.js driver.js > $B/$p.js
  for impl in node techne techne-interp; do
    case $impl in
      node) out=$(cd ../build && taskset -c $CPU node steady/$p.js) ;;
      techne) out=$(cd ../build && taskset -c $CPU $TECHNE_VM steady/$p.scm) ;;
      techne-interp) out=$(cd ../build && TECHNE_JIT=0 taskset -c $CPU $TECHNE_VM steady/$p.scm) ;;
    esac
    result=$(head -1 <<<"$out")
    med=$(tail -1 <<<"$out" | "$TECHNE_CLI" run median.scm)
    printf '%-9s %-14s median %-8s first %-8s -> %s\n' "$p" "$impl" ${med% *} ${med#* } "$result"
  done
done
