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
# starting, TECHNE_EDITOR_KEYS the instructions per key of its keystroke
# workloads). The JSON is the format of github-action-benchmark's
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

# Instructions per key of the editor's keystroke workloads, if
# TECHNE_EDITOR_KEYS names techne-editor's `keys` example: callgrind counts
# only in the function typing a workload's timed keys, and the count is
# divided by their number. A workload over the budget per key fails (after
# the JSON is written), unless it is a known violation (PLAN.md, Stage 1,
# slice 7).
KEYS_BUDGET=28000000 # instructions, some 4 ms on the daily hardware
known_slow=()        # workload names, each with the PLAN.md item fixing it
over_budget=()
if [[ -n ${TECHNE_EDITOR_KEYS:-} ]]; then
  for w in $("$TECHNE_EDITOR_KEYS" --list); do
    out=$(TECHNE_JIT_SYNC=1 valgrind --tool=callgrind --callgrind-out-file=/dev/null --toggle-collect='*timed_keys*' \
      "$TECHNE_EDITOR_KEYS" "$w" 2>&1)
    keys=$(grep -v '^==' <<<"$out" | tail -1)
    per_key=$(( $(sed -n 's/.*Collected : *//p' <<<"$out") / keys ))
    printf '%-18s %16s instr per key\n' "keys $w" "$per_key" >&2
    entries+=("{\"name\": \"keys: $w (jit)\", \"unit\": \"instructions per key\", \"value\": $per_key}")
    if ((per_key > KEYS_BUDGET)) && [[ " ${known_slow[*]} " != *" $w "* ]]; then
      over_budget+=("$w")
    fi
  done
fi

{ echo "["; (IFS=$'\n'; echo "${entries[*]}" | sed '$!s/$/,/'); echo "]"; } > "$OUT"

if ((${#over_budget[@]})); then
  echo "over the keystroke budget of $KEYS_BUDGET instructions per key: ${over_budget[*]}" >&2
  exit 1
fi
