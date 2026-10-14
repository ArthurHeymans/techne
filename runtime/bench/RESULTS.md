# Benchmark results

## techne against V8 (Node 24), 2026-10-14

Same programs, same machine. The JavaScript ports in `other/*.js` mirror
the Lua ports (arrays and `Map`s), and their outputs match. All runs are
pinned to one core, which also pins V8's background compiler and GC
threads.

**Whole process** (`run.sh`, hyperfine, 10 runs, mean wall ms including
startup; raw JSON in `results/20261014-1235/`):

| program | chez | guile | luajit | lua 5.4 | python3 | node | techne | techne interp |
|---|---|---|---|---|---|---|---|---|
| startup | 44 | 9 | 2 | 2 | 12 | 24 | 4 | 5 |
| fib | 53 | 39 | 13 | 90 | 187 | 42 | 32 | 112 |
| tak | 56 | 48 | 25 | 98 | 215 | 47 | 46 | 139 |
| nqueens | 67 | 99 | 80 | 290 | 345 | 62 | 59 | 200 |
| bintrees | 107 | 298 | 755 | 1548 | 1028 | 254 | 168 | 570 |
| hof | 82 | 115 | 38 | 112 | 217 | 90 | 109 | 199 |
| qsort | 81 | 80 | 47 | 120 | 424 | 55 | 71 | 233 |
| mandel | 139 | 720 | 9 | 96 | 408 | 36 | 50 | 149 |
| hash | 354 | 224 | 46 | 103 | 125 | 105 | 66 | 80 |
| orgparse | 71 | 89 | 14 | 68 | 187 | 51 | 35 | 80 |

**Steady state** (`steady/run.sh`): each program's work as one `run`
function, called 20 times in one process. Median ms of the last 10 calls,
and of the first call (`results/20261014-1235/steady.txt`):

| program | node | techne | techne interp | techne / node | node 1st | techne 1st |
|---|---|---|---|---|---|---|
| fib | 16 | 27 | 99 | 1.7× | 18 | 29 |
| tak | 20 | 45 | 133 | 2.2× | 22 | 47 |
| nqueens | 29 | 48 | 189 | 1.7× | 38 | 54 |
| bintrees | 116 | 156 | 563 | 1.3× | 232 | 161 |
| hof | 45 | 89 | 182 | 2.0× | 53 | 93 |
| qsort | 18 | 60 | 228 | 3.3× | 25 | 67 |
| mandel | 7 | 41 | 146 | 6.2× | 9 | 45 |
| hash | 50 | 62 | 107 | 1.2× | 76 | 59 |
| orgparse | 6 | 12 | 70 | 1.9× | 23 | 26 |

- Warmed up, V8 is 2.1× faster (geometric mean). The gap is smallest
  where allocation and hashing dominate (bintrees 1.3×, hash 1.2×) and
  largest in arithmetic and vector loops (qsort 3.3×, mandel 6.2×).
- Those loops pay for generic arithmetic: each operation checks its
  operands' tags and picks the fixnum or float path, and only fixnums are
  inferred. mandel's inner loop costs about 11 ns an iteration, against
  2 ns in V8. Type feedback, float inference and unboxed loop variables
  are what close it, not call or allocation speed.
- techne warms up less: its first call is within 12% of its steady
  state except orgparse's (2.2×), while V8's first call is up to 4× its
  own (orgparse, bintrees).
  For short scripts techne is ahead on the whole process: it starts in
  4 ms against Node's 24, and wins 6 of the 9 programs.

## Steel and other Schemes, 2026-10-05

Harness: `run.sh` (hyperfine, 5 runs, pinned to CPU 4; host load ~59, governor
powersave, so treat as ±10%). Programs in `programs/`, identical across Schemes
except `prelude/`. Raw JSON: `results/20261005-1146/`. All outputs cross-checked.
Wall ms, **including process startup** (Steel startup alone is ~150 ms).

| program | chez | guile | luajit | lua 5.4¹ | python3 | steel stock | steel fork | fork +IR | fork resumable | fork native |
|---|---|---|---|---|---|---|---|---|---|---|
| startup | 49 | 10 | 1 | 1 | 12 | 158 | 121 | 152 | 159 | 166 |
| fib | 59 | 42 | 14 | 91 | 205 | 395 | 627 | 668 | 714 | 5806 |
| tak | 60 | 51 | 26 | 104 | 237 | 546 | 809 | 933 | 1097 | 12923 |
| nqueens | 72 | 106 | 84 | 248 | 354 | 911 | 1426 | 1682 | 1832 | 5180 |
| bintrees | 111 | 308 | 823 | 1852 | 1069 | 1850 | 2879 | 2993 | 3320 | 15250 |
| hof | 92 | 126 | 44 | 116 | 238 | 1150 | 1349 | 1378 | 1532 | 3251 |
| qsort | 88 | 89 | 52 | 127 | 451 | 1123 | 1942 | 1798 | 2043 | 11551 |
| mandel | 148 | 782 | 9 | 107 | 392 | 938 | 1315 | 1515 | 1633 | 5131 |
| hash | 382 | 244 | 50 | 115 | 128 | >300 s² | – | – | – | – |
| orgparse | 78 | 97 | 15 | 74 | 184 | 485 | 641 | 678 | 707 | 2993 |
| ffi (2M Rust calls + 2M Scheme calls) | | | | | | | | 432 | 437 | 2906 |

¹ Re-measured separately (3 runs); the campaign's column accidentally ran LuaJIT.
² Quadratic: `IterativeDropHandler::visit_hash_map` drains a dropped persistent
map with `mem::take`, forcing copy-on-write of every node still shared with the
new version. Each insert into a rebound map costs O(n).

### Findings
- Stock Steel is 6–17× slower than Chez/Guile and 2–5× slower than CPython.
- The fork is 1.2–1.7× slower than stock: it disabled the legacy JIT that stock's
  CLI uses by default (visible in stock's profile), and nothing replaced it.
- Semantic IR (Optimize) gives no measurable speedup on any program.
- Resumable slicing costs 5–20% over plain Engine execution.
- jit2 "native" mode is 2–15× slower than interpreting: it copies frame slots into
  a 4 KB buffer per entry and exits at every call (fib: 28M entries, ~2 ops each).
- Startup (~150 ms) is the compiler re-analysing the prelude from source each run.

### Emacs 31.1 comparison (results/20261005-1536, host load ~60, noisy)

| program | emacs .elc | native speed 2 | native speed 3 | steel stock |
|---|---|---|---|---|
| startup | 56 | 77 | 55 | 158 |
| fib | 306 | 361 | 238 | 395 |
| tak | 436 | 374 | 136 | 546 |
| nqueens | 804 | 796 | 507 | 911 |
| bintrees | 1005 | 892 | 387 | 1850 |
| hof | 326 | 310 | 339 | 1150 |
| qsort | 288 | 216 | 230 | 1123 |
| mandel | 2125 | 2896 | 536 | 938 |
| hash | 361 | 297 | 264 | >300 s |
| orgparse | 166 | 149 | 146 | 485 |

Byte-compiled Elisp beats stock Steel everywhere except mandel (Elisp boxes floats).
Native speed 3 is 1.5–5× faster than stock Steel. Speed 2 barely beats bytecode:
it does not compile self-calls directly.
