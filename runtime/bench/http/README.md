# HTTP server benchmark

The same minimal HTTP/1.1 keep-alive server in techne and other languages,
loaded with wrk, to see how techne's tasks (async) and worlds-on-threads
(SMP) hold up as a network server.

- `techne/`: the server is Lisp (`server.scm`), one task per connection,
  over `crates/techne-net`'s TCP procedures; a small Rust host starts it.
  `THREADS` VMs run on their own threads, each with its own listener on the
  port (`#:reuse-port`); they share nothing.
  - `techne`: each VM's sockets are on a current-thread tokio runtime its
    thread drives (`techne_net::install_on` and `techne_net::run_tasks`,
    which calls `run_tasks_for` when woken, as a host event loop would).
  - `techne-iothread`: techne-net's default, one shared tokio I/O thread
    (`techne_net::install`) waking VMs parked in `run_tasks`.
  - `techne-interp`: `techne`, with `TECHNE_JIT=0`.
- The others do exactly what `server.scm` does, on raw sockets: `rust`
  (tokio, multi-thread), `go` (goroutine per connection), `node` (`node:net`),
  `python` (asyncio Protocol), `guile-fibers` (a fiber per connection,
  work-stealing schedulers). For reference, the standard servers too:
  `go-nethttp` and `node-http`, which parse requests fully.
  Single-threaded runtimes (node, python) run one process per thread
  sharing the port.
- `GET /` returns "Hello, World!", which is mostly kernel time. `GET /work`
  computes `sum of (string-length (number->string (* i i)))` for i < 1000
  in the language.

`./run.sh` builds everything and runs every experiment (about 40 min);
`python3 summarize.py results/DIR/results.tsv` makes the tables below.

For CI, `../icount.sh` counts the instructions per request of `server.scm`
on in-memory connections (`crates/techne-vm/examples/serve.rs`), and the
nightly workflow runs techne's server for a few seconds per experiment.

## Results 2026-10-10

AMD Ryzen 9 8945HS (8 cores, 16 threads), servers pinned to 4 cores (CPUs
0-3 and their hyperthreads 8-11), wrk with 8 threads on the other 4 cores. A
laptop with a desktop session running (load ~2-3): repeat runs differ by up to
about 10%. Raw data: `results/full/`.

These numbers were taken with the harness's own throwaway TCP natives,
before techne-net. With techne-net, quick runs (`IMPLS='^techne'
EXPERIMENTS='smp idle' DURATION=4`), alternating with the old harness on
the same machine, give the same throughput within the noise: 157k against
155k req/s at 1 thread and 16 connections, and 4.4 µs of CPU per request
(user and kernel) at 256 connections in both. User-space instructions per
request rose about 2% (8,400 to 8,600: UTF-8 joined across reads, the
checks per read).

### Plaintext, 256 connections, by server threads (req/s)

| impl | 1 | 2 | 4 | 8 |
|---|---|---|---|---|
| techne | 143k | 276k | 512k | 787k |
| techne-iothread | 140k | 282k | 512k | 672k |
| techne-interp | 145k | 283k | 544k | 776k |
| rust-tokio | 161k | 355k | 595k | 784k |
| go | 155k | 301k | 567k | 789k |
| go-nethttp | 93k | 177k | 316k | 429k |
| node | 121k | 276k | 470k | 679k |
| node-http | 76k | 156k | 309k | 294k |
| python | 119k | 221k | 514k | 497k |
| guile-fibers | 92k | 167k | 253k | 301k |

At 8 threads rust, go and techne all stop at about 785k, which is probably
wrk on its 4 cores rather than the servers.

### /work, 256 connections, by server threads (req/s)

| impl | 1 | 4 | 8 |
|---|---|---|---|
| techne | 22k | 77k | 85k |
| techne-iothread | 22k | 73k | 86k |
| techne-interp | 18k | 66k | 77k |
| rust-tokio | 74k | 247k | 300k |
| go | 44k | 158k | 182k |
| go-nethttp | 35k | 117k | 133k |
| node | 66k | 251k | 303k |
| node-http | 52k | 193k | 189k |
| python | 9k | 37k | 42k |
| guile-fibers | 13k | 32k | 22k |

### Plaintext, 1 thread, by connections (req/s / p99 ms)

| impl | 16 | 256 | 1024 | 4096 |
|---|---|---|---|---|
| techne | 153k / 0.2 | 143k / 3.4 | 123k / 12.2 | 115k / 47.7 |
| techne-iothread | 151k / 0.2 | 140k / 3.1 | 125k / 12.6 | 112k / 53.5 |
| techne-interp | 146k / 0.2 | 145k / 3.1 | 118k / 13.4 | 107k / 54.1 |
| rust-tokio | 176k / 0.3 | 161k / 4.1 | 185k / 165.9 | 159k / 760.1 |
| go | 163k / 0.2 | 155k / 3.7 | 135k / 11.0 | 121k / 44.8 |
| go-nethttp | 104k / 0.4 | 93k / 6.2 | 52k / 53.1 | 69k / 79.7 |
| node | 137k / 0.2 | 121k / 4.0 | 101k / 205.0 | 99k / 27.0 |
| node-http | 85k / 0.5 | 76k / 24.8 | 58k / 54.2 | 60k / 32.6 |
| python | 147k / 0.2 | 119k / 5.5 | 123k / 11.8 | 96k / 91.0 |
| guile-fibers | 87k / 3.0 | 92k / 5.1 | 80k / 19.1 | 74k / 69.8 |

### Plaintext, 1 thread, 16 busy connections, by idle connections (req/s / RSS)

| impl | 0 | 1000 | 10000 |
|---|---|---|---|
| techne | 153k / 25 MB | 113k / 28 MB | 30k / 59 MB |
| techne-iothread | 151k / 24 MB | 129k / 29 MB | 54k / 60 MB |
| techne-interp | 146k / 20 MB | 109k / 22 MB | 30k / 55 MB |
| rust-tokio | 176k / 4 MB | 179k / 16 MB | 175k / 130 MB |
| go | 163k / 5 MB | 161k / 9 MB | 160k / 41 MB |
| go-nethttp | 104k / 13 MB | 95k / 32 MB | 92k / 233 MB |
| node | 137k / 72 MB | 143k / 79 MB | 146k / 116 MB |
| node-http | 85k / 80 MB | 86k / 94 MB | 86k / 223 MB |
| python | 147k / 23 MB | 143k / 25 MB | 144k / 49 MB |
| guile-fibers | 87k / 27 MB | 87k / 50 MB | 77k / 250 MB |

## What it shows

- **SMP works.** Worlds share nothing, so VMs on threads with their own
  listeners scale like Go and Rust: 3.5x on 4 cores for both plaintext and
  `/work` (Go 3.6x, Rust 3.7x), with no lock or GC shared between them. Only the variant that
  funnels all I/O through one tokio thread (`techne-iothread`,
  techne-net's default runtime) falls behind at 8 threads.
- **Per request, the runtime costs little.** Plaintext is mostly kernel:
  per request, techne spends 1.5 µs in user space and 3.1 µs in the
  kernel, against Rust's 0.5 and 2.9 and Python's 1.6 and 3.3. The rest of
  techne's time goes to async plumbing: per wait, an `Arc<Flag>`, a boxed
  `Error` ("task suspended") and a boxed future, plus `run_round` and
  `Foreign` lookups.
- **The scheduler is O(tasks) per round.** `run_round` clones `live_tasks`
  and checks every task, ready or not, so idle connections tax the busy
  ones: with 10,000 idle connections, throughput drops to a fifth, while
  every other runtime's stays flat (epoll hands them only what is ready).
  A ready queue that wakers push to would fix this. Under full load
  (4096 busy connections) it does not show, because nearly every task is
  ready each round. Also, `tasks` never shrinks (finished tasks keep their
  slot and result), so a long-running server leaks a slot per connection;
  PLAN.md's Stage 1 step 8.3f is meant to fix that.
- **`/work` measures natives, not the JIT.** The loop spends its time in
  `number->string` (a Rust `String` formatted, copied into the heap and
  freed: malloc/free and memmove are about half of the profile), called through
  `jit_call_slow`/`call_native`. So the JIT gains only 20% over the
  interpreter here. Techne is 2x behind Go and 3x behind V8 and Rust,
  but ahead of Python and Guile.
- Memory per connection is small: about 3.4 KB per idle connection
  (task, stack and socket), like Go (3.6 KB), less than Node (4.4 KB) or
  this Rust server (12.6 KB, a 16 KB buffer each).
- Not measured: connection churn (no keep-alive), large bodies, TLS.
