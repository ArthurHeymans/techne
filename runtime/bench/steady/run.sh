#!/usr/bin/env bash
# Steady-state campaign: each program's work, run ITERS times in one process
# per implementation; prints the median of the second half of the iterations
# (ms). Pinned to one core like ../run.sh.
set -euo pipefail
cd "$(dirname "$0")"
TECHNE_VM=${TECHNE_VM:-$(realpath ../../../target/release/techne-vm)}
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
    med=$(tail -1 <<<"$out" | python3 -c 'import sys,statistics; t=list(map(int,sys.stdin.read().split())); print("%.1f %.1f" % (statistics.median(t[len(t)//2:])/1000, t[0]/1000))')
    printf '%-9s %-14s median %-8s first %-8s -> %s\n' "$p" "$impl" ${med% *} ${med#* } "$result"
  done
done
