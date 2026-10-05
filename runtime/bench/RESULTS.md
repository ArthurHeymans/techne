# Benchmark results 2026-10-05

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

## Findings
- Stock Steel is 6–17× slower than Chez/Guile and 2–5× slower than CPython.
- The fork is 1.2–1.7× slower than stock: it disabled the legacy JIT that stock's
  CLI uses by default (visible in stock's profile), and nothing replaced it.
- Semantic IR (Optimize) gives no measurable speedup on any program.
- Resumable slicing costs 5–20% over plain Engine execution.
- jit2 "native" mode is 2–15× slower than interpreting: it copies frame slots into
  a 4 KB buffer per entry and exits at every call (fib: 28M entries, ~2 ops each).
- Startup (~150 ms) is the compiler re-analysing the prelude from source each run.

## Emacs 31.1 comparison (results/20261005-1536, host load ~60, noisy)

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
