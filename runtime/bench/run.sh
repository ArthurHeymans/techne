#!/usr/bin/env bash
# Timing campaign. Run inside: nix-shell -p chez hyperfine luajit lua5_4
# Every command is pinned to one core (CPU 4, sibling 12) to limit interference.
set -euo pipefail
cd "$(dirname "$0")"
./assemble.sh
elisp/compile.sh >/dev/null
B=${BENCH_BIN:-$(realpath ../../.local/bench-target)}
CPU=${CPU:-4}
TECHNE_VM=${TECHNE_VM:-$(realpath ../../target/release/techne-vm)}
# luajit also provides `lua`, so resolve PUC Lua 5.4 explicitly.
LUA54=${LUA54:-$(nix-build "<nixpkgs>" -A lua5_4 --no-out-link)/bin/lua}
OUT=results/$(date +%Y%m%d-%H%M)
mkdir -p "$OUT"
{ echo "date: $(date -Is)"; echo "cpu: $CPU siblings $(cat /sys/devices/system/cpu/cpu$CPU/topology/thread_siblings_list)";
  echo "governor: $(cat /sys/devices/system/cpu/cpu$CPU/cpufreq/scaling_governor)"; uptime;
  sha256sum $B/stock/release/steel $B/fork/release/steel $B/driver-default $B/driver-modern; } > "$OUT/env.txt"
cd build
P="taskset -c $CPU"
# IMPLS: optional regex selecting implementations by name.
add() { [[ $1 =~ ${IMPLS:-.} ]] && cmds+=(-n "$1" "$2") || true; }
for prog in ${PROGS:-startup fib tak nqueens bintrees hof qsort mandel hash orgparse ffi}; do
  cmds=()
  if [[ $prog != ffi ]]; then
    add chez "$P scheme --script chez/$prog.scm"
    add guile "env GUILE_AUTO_COMPILE=1 $P guile guile/$prog.scm"
    add luajit "$P luajit $prog.lua"
    add lua5.4 "$P $LUA54 $prog.lua"
    add python3 "$P python3 $prog.py"
    add emacs-elc "$P emacs -Q --batch -l elisp/$prog.elc"
    add emacs-native2 "$P emacs -Q --batch -l elisp/s2/$prog.eln"
    add emacs-native3 "$P emacs -Q --batch -l elisp/s3/$prog.eln"
    add techne "$P $TECHNE_VM techne/$prog.scm"
    add techne-interp "env TECHNE_JIT=0 $P $TECHNE_VM techne/$prog.scm"
  fi
  if [[ $prog != hash ]]; then  # Steel's hash pathology makes it run >5 minutes
    if [[ $prog != ffi ]]; then
      add steel-stock "$P $B/stock/release/steel steel/$prog.scm"
      add steel-fork "$P $B/fork/release/steel steel/$prog.scm"
    fi
    add fork-engine "$P $B/driver-default engine steel/$prog.scm"
    add fork-engine-ir "$P $B/driver-default engine-ir steel/$prog.scm"
    add fork-resumable "$P $B/driver-default resumable steel-resumable/$prog.scm 4096"
    add fork-native "$P $B/driver-modern resumable-native steel-resumable/$prog.scm 4096"
  fi
  [[ ${#cmds[@]} -eq 0 ]] && continue
  hyperfine -N --warmup 1 --runs ${RUNS:-5} --export-json "../$OUT/$prog.json" "${cmds[@]}" --output=null 2>&1 | grep -E "^Benchmark|Time \(mean" || true
done
echo "$OUT"
