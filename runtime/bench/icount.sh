#!/usr/bin/env bash
# Deterministic performance numbers for techne-vm, for CI and for comparing
# builds on a busy machine: wall time depends on the load, these do not.
#
# For each program in programs/ it reports
#   - instructions executed with the JIT (compiling synchronously, so the
#     result does not depend on thread timing) and with the interpreter,
#     counted by valgrind's cachegrind (repeat runs agree to ~0.001%);
#   - the most words one GC pause copied or marked, an upper bound on the
#     longest pause's work (from TECHNE_GC_STATS, the JIT compiling
#     synchronously too).
#
# Usage: icount.sh [OUT.json [FOOTPRINT.json]]   (inside `nix develop`;
# TECHNE_VM overrides the binary, PROGS the program list,
# TECHNE_EDITOR_STARTUP adds the editor starting: the instructions per
# datum of the Lisp it loads go to OUT, its whole count to FOOTPRINT,
# held to a budget rather than compared; TECHNE_EDITOR_KEYS adds the
# instructions per key of its keystroke workloads, TECHNE_SERVE the
# instructions per request of the HTTP server's, TECHNE_COMPILE the
# instructions per top-level form of reading and of loading the
# prelude). The JSON is the format of github-action-benchmark's
# customSmallerIsBetter tool; a table goes to stderr.
set -euo pipefail
OUT=$(realpath "${1:-/dev/stdout}")
FOOTPRINT=$(realpath "${2:-/dev/null}")
cd "$(dirname "$0")"
VM=${TECHNE_VM:-$(realpath ../../target/release/techne-vm)}
PROGS=${PROGS:-startup fib tak nqueens bintrees hof qsort mandel hash orgparse callbacks numstr tasks}

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
  # The JIT compiling synchronously, as for the counts: compiled in the
  # background, code is installed at different points of a run, which
  # moves the collections.
  stats=$(TECHNE_JIT_SYNC=1 TECHNE_GC_STATS=1 "$VM" "$prog_file" 2>&1 >/dev/null | grep GcStats || true)
  copied=$(sed -n 's/.*max_copied_words: \([0-9]*\).*/\1/p' <<<"$stats")
  marked=$(sed -n 's/.*max_slice_words: \([0-9]*\).*/\1/p' <<<"$stats")
  pause=$(( ${copied:-0} + ${marked:-0} ))
  printf '%-10s %16s instr (jit) %16s instr (interp) %10s words/pause\n' "$prog" "$jit" "$interp" "$pause" >&2
  entries+=("{\"name\": \"$prog (jit)\", \"unit\": \"instructions\", \"value\": $jit}")
  entries+=("{\"name\": \"$prog (interp)\", \"unit\": \"instructions\", \"value\": $interp}")
  # Pauses of a few words (a program that barely collects) are noise, not
  # a pause to compare; 0 would break ratios.
  if ((pause >= 64)); then
    entries+=("{\"name\": \"$prog GC pause\", \"unit\": \"words\", \"value\": $pause}")
  fi
done

# The editor starting (its Lisp compiled and loaded), if
# TECHNE_EDITOR_STARTUP names techne-editor's `startup` example. Its
# count grows with every package the editor ships, so it is not what is
# compared: the instructions per datum of the Lisp loaded are, which
# shipping more Lisp leaves alone and a slower reader, expander or
# compiler does not. The whole count goes to FOOTPRINT and is held to
# the startup budget instead.
STARTUP_BUDGET=700000000 # instructions, some 100 ms (REQUIREMENTS.md), as KEYS_BUDGET's 4 ms
footprint=()
if [[ -n ${TECHNE_EDITOR_STARTUP:-} ]]; then
  count() { env "$@" valgrind --tool=cachegrind --cache-sim=no --cachegrind-out-file=/dev/null \
    "$TECHNE_EDITOR_STARTUP" 2>&1 >/dev/null | sed -n 's/.*I *refs: *//p' | tr -d ,; }
  jit=$(count TECHNE_JIT_SYNC=1)
  interp=$(count TECHNE_JIT=0)
  data=$("$TECHNE_EDITOR_STARTUP" --data)
  printf '%-10s %16s instr (jit) %16s instr (interp) %10s data\n' editor "$jit" "$interp" "$data" >&2
  entries+=("{\"name\": \"editor load (jit)\", \"unit\": \"instructions per datum\", \"value\": $((jit / data))}")
  entries+=("{\"name\": \"editor load (interp)\", \"unit\": \"instructions per datum\", \"value\": $((interp / data))}")
  footprint+=("{\"name\": \"editor startup (jit)\", \"unit\": \"instructions\", \"value\": $jit}")
  footprint+=("{\"name\": \"editor startup (interp)\", \"unit\": \"instructions\", \"value\": $interp}")
  footprint+=("{\"name\": \"editor Lisp loaded\", \"unit\": \"data\", \"value\": $data}")
fi

# Instructions per key of the editor's keystroke workloads, if
# TECHNE_EDITOR_KEYS names techne-editor's `keys` example: callgrind counts
# only in the function typing a workload's timed keys, and the count is
# divided by their number. A workload over the budget per key fails (after
# the JSON is written), unless it is a known violation (PLAN.md, Stage 1,
# slice 7).
KEYS_BUDGET=28000000 # instructions, some 4 ms on the daily hardware
# workload names, each with the PLAN.md item fixing it: typing in a
# syntax-coloured file collects its window's captures again (Stage 1,
# slice 7, "Known violation").
known_slow=(type-rust type-rust-big)
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

# Instructions per request of the HTTP benchmark's server on in-memory
# connections, if TECHNE_SERVE names techne-vm's `serve` example: callgrind
# counts only in the function serving the timed requests, after a warm-up
# in which the JIT compiled everything on first call (TECHNE_JIT=1).
# Compiling varies the order of earlier allocations from run to run
# (randomly seeded hash maps, a second thread), and with it the paths
# malloc takes during the requests (up to 1% of the count); unbounded
# per-thread caches make it take the same ones.
if [[ -n ${TECHNE_SERVE:-} ]]; then
  for w in $("$TECHNE_SERVE" --list); do
    for mode in jit:1 interp:0; do
      out=$(GLIBC_TUNABLES=glibc.malloc.tcache_count=65535 TECHNE_JIT=${mode#*:} valgrind --tool=callgrind \
        --callgrind-out-file=/dev/null --toggle-collect='*timed_requests*' "$TECHNE_SERVE" "$w" 2>&1)
      requests=$(grep -v '^==' <<<"$out" | tail -1)
      per_request=$(( $(sed -n 's/.*Collected : *//p' <<<"$out") / requests ))
      printf '%-18s %16s instr per request (%s)\n' "http $w" "$per_request" "${mode%:*}" >&2
      entries+=("{\"name\": \"http: $w request (${mode%:*})\", \"unit\": \"instructions per request\", \"value\": $per_request}")
    done
  done
fi

# Instructions per top-level form of the front end's phases, if
# TECHNE_COMPILE names techne-vm's `compile` example: callgrind counts
# only in the function passing over the prelude (reading it, or loading
# it into bare worlds made beforehand), and the count is divided by
# the forms it timed. As with the HTTP server's counts, unbounded
# per-thread caches keep malloc on the same paths between runs.
if [[ -n ${TECHNE_COMPILE:-} ]]; then
  for w in $("$TECHNE_COMPILE" --list); do
    out=$(GLIBC_TUNABLES=glibc.malloc.tcache_count=65535 valgrind --tool=callgrind \
      --callgrind-out-file=/dev/null --toggle-collect='*timed_forms*' "$TECHNE_COMPILE" "$w" 2>&1)
    forms=$(grep -v '^==' <<<"$out" | tail -1)
    per_form=$(( $(sed -n 's/.*Collected : *//p' <<<"$out") / forms ))
    printf '%-18s %16s instr per form\n' "compile $w" "$per_form" >&2
    entries+=("{\"name\": \"compile: $w\", \"unit\": \"instructions per form\", \"value\": $per_form}")
  done
fi

{ echo "["; (IFS=$'\n'; echo "${entries[*]}" | sed '$!s/$/,/'); echo "]"; } > "$OUT"
{ echo "["; (IFS=$'\n'; echo "${footprint[*]}" | sed '$!s/$/,/'); echo "]"; } > "$FOOTPRINT"

failed=0
if [[ -n ${TECHNE_EDITOR_STARTUP:-} ]] && ((jit > STARTUP_BUDGET || interp > STARTUP_BUDGET)); then
  echo "editor startup over its budget of $STARTUP_BUDGET instructions" >&2
  failed=1
fi
if ((${#over_budget[@]})); then
  echo "over the keystroke budget of $KEYS_BUDGET instructions per key: ${over_budget[*]}" >&2
  failed=1
fi
exit $failed
