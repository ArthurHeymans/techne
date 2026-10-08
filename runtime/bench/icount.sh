#!/usr/bin/env bash
# Deterministic performance numbers for techne-vm, for CI and for comparing
# builds on a busy machine: wall time depends on the load, these do not.
#
# For each program in programs/ it reports
#   - instructions executed with the JIT (compiling synchronously, so the
#     result does not depend on thread timing) and with the interpreter,
#     counted by valgrind's cachegrind (repeat runs agree to ~0.001%);
#   - the most words one GC pause copied or marked, an upper bound on the
#     longest pause's work (from TECHNE_GC_STATS).
#
# Usage: icount.sh [OUT.json]   (inside `nix develop`; TECHNE_VM overrides
# the binary, PROGS the program list, TECHNE_EDITOR_STARTUP adds the editor
# starting). The JSON is the format of github-action-benchmark's
# customSmallerIsBetter tool; a table goes to stderr.
set -euo pipefail
OUT=$(realpath "${1:-/dev/stdout}")
cd "$(dirname "$0")"
VM=${TECHNE_VM:-$(realpath ../../target/release/techne-vm)}
PROGS=${PROGS:-startup fib tak nqueens bintrees hof qsort mandel hash orgparse callbacks}

mkdir -p build/techne
[[ -f org-sample.org ]] || python3 gen-org.py  # input of orgparse
icount() { # env... -- program
  env "$@" valgrind --tool=cachegrind --cache-sim=no --cachegrind-out-file=/dev/null \
    "$VM" "$prog_file" 2>&1 >/dev/null | sed -n 's/.*I *refs: *//p' | tr -d ,
}

entries=()
for prog in $PROGS; do
  prog_file=build/techne/$prog.scm
  cat prelude/techne.scm "programs/$prog.scm" > "$prog_file"
  jit=$(icount TECHNE_JIT_SYNC=1)
  interp=$(icount TECHNE_JIT=0)
  stats=$(TECHNE_GC_STATS=1 "$VM" "$prog_file" 2>&1 >/dev/null | grep GcStats || true)
  copied=$(sed -n 's/.*max_copied_words: \([0-9]*\).*/\1/p' <<<"$stats")
  marked=$(sed -n 's/.*max_slice_words: \([0-9]*\).*/\1/p' <<<"$stats")
  pause=$(( ${copied:-0} + ${marked:-0} ))
  printf '%-10s %16s instr (jit) %16s instr (interp) %10s words/pause\n' "$prog" "$jit" "$interp" "$pause" >&2
  entries+=("{\"name\": \"$prog (jit)\", \"unit\": \"instructions\", \"value\": $jit}")
  entries+=("{\"name\": \"$prog (interp)\", \"unit\": \"instructions\", \"value\": $interp}")
  # Programs that never pause have nothing to compare (and 0 breaks ratios).
  if ((pause > 0)); then
    entries+=("{\"name\": \"$prog GC pause\", \"unit\": \"words\", \"value\": $pause}")
  fi
done

# The editor starting (its Lisp compiled and loaded), if
# TECHNE_EDITOR_STARTUP names techne-editor's `startup` example.
if [[ -n ${TECHNE_EDITOR_STARTUP:-} ]]; then
  count() { env "$@" valgrind --tool=cachegrind --cache-sim=no --cachegrind-out-file=/dev/null \
    "$TECHNE_EDITOR_STARTUP" 2>&1 >/dev/null | sed -n 's/.*I *refs: *//p' | tr -d ,; }
  jit=$(count TECHNE_JIT_SYNC=1)
  interp=$(count TECHNE_JIT=0)
  printf '%-10s %16s instr (jit) %16s instr (interp)\n' editor "$jit" "$interp" >&2
  entries+=("{\"name\": \"editor startup (jit)\", \"unit\": \"instructions\", \"value\": $jit}")
  entries+=("{\"name\": \"editor startup (interp)\", \"unit\": \"instructions\", \"value\": $interp}")
fi

{ echo "["; (IFS=$'\n'; echo "${entries[*]}" | sed '$!s/$/,/'); echo "]"; } > "$OUT"
