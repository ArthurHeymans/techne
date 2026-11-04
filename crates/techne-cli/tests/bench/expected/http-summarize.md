### Plaintext, 256 connections, by server threads (req/s)

| impl | 1 | 2 | 4 | 8 |
|---|---|---|---|---|
| techne | 154k |  |  |  |

### /work, 256 connections, by server threads (req/s)

| impl | 1 | 4 | 8 |
|---|---|---|---|
| techne |  | 9007199254741k |  |

### Plaintext, 1 thread, by connections (req/s / p99 ms)

| impl | 16 | 256 | 1024 | 4096 |
|---|---|---|---|---|
| techne |  | 154k / 2.9 |  |  |
| go | 2k / 1.1 |  |  | 0k / 1000.5 |

### Plaintext, 1 thread, 16 busy connections, by idle connections (req/s / RSS)

| impl | 0 | 1000 | 10000 |
|---|---|---|---|
| go | 2k / 9 MB | 2k / 12 MB |  |

