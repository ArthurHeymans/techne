#!/usr/bin/env bash
# HTTP server benchmark: the same minimal keep-alive server (server.scm and
# its ports) in techne and other languages, loaded by wrk.
#
#   ./run.sh                 # every experiment, results in results/DATE/
#   EXPERIMENTS=smp IMPLS='^techne' DURATION=5 ./run.sh   # builds only techne
#
# Experiments (name: path, server threads, connections):
#   smp:    /      threads 1 2 4 8, 256 connections   (scaling over cores)
#   work:   /work  threads 1 4 8,   256 connections   (CPU-bound handlers)
#   conns:  /      threads 1,       16..4096 conns    (many tasks per thread)
#   idle:   /      threads 1,       16 conns, 0..10000 idle ones besides
#
# The server runs on CPUS (4 cores, both hyperthreads), wrk on the other
# four (LOADCPUS). Each run is preceded by WARMUP seconds of load.
set -euo pipefail
cd "$(dirname "$0")"
HERE=$PWD

ALL="techne techne-iothread techne-interp rust-tokio go go-nethttp node node-http python guile-fibers"
IMPLS=${IMPLS:-.}
impls=$(tr ' ' '\n' <<<"$ALL" | grep -E "$IMPLS")
uses() { grep -q "$1" <<<"$impls"; }

if ! command -v wrk >/dev/null || { uses guile && ! guile -c '(use-modules (fibers))' 2>/dev/null; }; then
    exec nix-shell -p wrk guile guile-fibers --run "$(printf '%q ' "$0" "$@")"
fi

CPUS=${CPUS:-0-3,8-11}
LOADCPUS=${LOADCPUS:-4-7,12-15}
WRK_THREADS=${WRK_THREADS:-8}
DURATION=${DURATION:-8}
WARMUP=${WARMUP:-2}
PORT=${PORT:-8090}
EXPERIMENTS=${EXPERIMENTS:-smp work conns idle}
OUT=${OUT:-results/$(date +%Y%m%d-%H%M)}
mkdir -p "$OUT"
TECHNE=${TECHNE:-$HERE/../../../target/release/techne-http-bench}

# What the selected implementations need.
build() {
    if uses techne; then
        (cd techne && CARGO_TARGET_DIR=$HERE/../../../target cargo build --release -q)
    fi
    if uses rust; then
        (cd rust && CARGO_TARGET_DIR=$HERE/build/rust-target cargo build --release -q)
        cp build/rust-target/release/rust-http-bench build/
    fi
    if uses go; then
        (cd go/raw && go build -o ../../build/go-raw .)
        (cd go/nethttp && go build -o ../../build/go-nethttp .)
    fi
}

# Start implementation $1 with $2 threads in the background, on $CPUS.
# Single-threaded runtimes run $2 processes sharing the port (SO_REUSEPORT).
start() {
    local impl=$1 t=$2 pin="taskset -c $CPUS"
    PIDS=()
    case $impl in
        techne)        THREADS=$t IO=inline $pin "$TECHNE" techne/server.scm $PORT & ;;
        techne-iothread) THREADS=$t IO=thread IO_WORKERS=1 $pin "$TECHNE" techne/server.scm $PORT & ;;
        techne-interp) TECHNE_JIT=0 THREADS=$t IO=inline $pin "$TECHNE" techne/server.scm $PORT & ;;
        rust-tokio)    THREADS=$t $pin build/rust-http-bench $PORT & ;;
        go)            GOMAXPROCS=$t $pin build/go-raw $PORT & ;;
        go-nethttp)    GOMAXPROCS=$t $pin build/go-nethttp $PORT & ;;
        guile-fibers)  THREADS=$t $pin guile guile/raw.scm $PORT 2>/dev/null & ;;
        node)          for _ in $(seq "$t"); do $pin node node/raw.js $PORT & done ;;
        node-http)     for _ in $(seq "$t"); do $pin node node/http.js $PORT & done ;;
        python)        for _ in $(seq "$t"); do $pin python3 python/raw.py $PORT & done ;;
    esac
    PIDS=($(jobs -p))
}

stop() {
    pkill -P $$ 2>/dev/null || true
    wait 2>/dev/null || true
}
trap stop EXIT

ready() {
    for _ in $(seq 100); do
        curl -sf -o /dev/null "http://127.0.0.1:$PORT/" && return 0
        sleep 0.1
    done
    echo "server did not start" >&2
    return 1
}

# Resident memory of the server processes, in MB.
rss() {
    ps -o rss= -p "$(IFS=,; echo "${PIDS[*]}")" | awk '{s += $1} END {printf "%.0f", s / 1024}'
}

# One measurement: impl threads path connections [idle] -> a line of
# results.tsv. With idle, that many more connections stay open, unused.
measure() {
    local impl=$1 t=$2 path=$3 c=$4 idle=${5:-0} log
    start "$impl" "$t"
    ready
    if [ "$idle" -gt 0 ]; then
        coproc HOLD (exec taskset -c "$LOADCPUS" python3 idle.py "$PORT" "$idle")
        read -r _ <&"${HOLD[0]}"
    fi
    taskset -c "$LOADCPUS" wrk -t"$WRK_THREADS" -c"$c" -d"${WARMUP}s" "http://127.0.0.1:$PORT$path" >/dev/null
    log=$OUT/$impl-t$t-c$c-i$idle${path//\//-}.txt
    taskset -c "$LOADCPUS" wrk -t"$WRK_THREADS" -c"$c" -d"${DURATION}s" --latency "http://127.0.0.1:$PORT$path" >"$log" &
    local w=$! mem
    sleep $((DURATION / 2))
    mem=$(rss)
    wait $w
    stop
    python3 - "$impl" "$t" "$path" "$c" "$idle" "$mem" "$log" >>"$OUT/results.tsv" <<'EOF'
import re, sys
impl, t, path, c, idle, mem, log = sys.argv[1:]
text = open(log).read()
def lat(p):
    m = re.search(rf"^\s+{p}%\s+([\d.]+)(us|ms|s)\b", text, re.M)
    if not m: return ""
    return "%.3f" % (float(m[1]) * {"us": 1e-3, "ms": 1, "s": 1e3}[m[2]])
rps = re.search(r"Requests/sec:\s+([\d.]+)", text)[1]
errors = "".join(re.findall(r"(Socket errors:.*|Non-2xx.*)", text)).strip()
print("\t".join([impl, t, path, c, idle, "%.0f" % float(rps), lat(50), lat(99), mem, errors]))
EOF
    tail -1 "$OUT/results.tsv"
}

build
[ -f "$OUT/results.tsv" ] || printf 'impl\tthreads\tpath\tconns\tidle\trps\tp50_ms\tp99_ms\trss_mb\terrors\n' >"$OUT/results.tsv"
{ uname -a; lscpu | grep 'Model name'; uptime; echo "server cpus $CPUS, wrk cpus $LOADCPUS, -t$WRK_THREADS, ${DURATION}s"; } >"$OUT/host.txt"
for e in $EXPERIMENTS; do
    for impl in $impls; do
        case $e in
            smp)   for t in 1 2 4 8; do measure "$impl" $t / 256; done ;;
            work)  for t in 1 4 8; do measure "$impl" $t /work 256; done ;;
            # Long enough that the first requests, waiting on connection
            # setup, are under 1%.
            conns) for c in 16 256 1024 4096; do DURATION=$((DURATION > 20 ? DURATION : 20)) measure "$impl" 1 / $c; done ;;
            idle)  for i in 0 1000 10000; do measure "$impl" 1 / 16 $i; done ;;
        esac
    done
done
