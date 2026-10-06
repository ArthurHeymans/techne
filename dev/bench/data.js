window.BENCHMARK_DATA = {
  "lastUpdate": 1791306394014,
  "repoUrl": "https://github.com/ArthurHeymans/techne",
  "entries": {
    "techne-vm": [
      {
        "commit": {
          "author": {
            "email": "arthur@aheymans.xyz",
            "name": "Arthur Heymans",
            "username": "ArthurHeymans"
          },
          "committer": {
            "email": "noreply@github.com",
            "name": "GitHub",
            "username": "web-flow"
          },
          "distinct": true,
          "id": "fbec14a54f3b728b6ed2ec0cbe2e86470b53d4ae",
          "message": "Merge pull request #1 from ArthurHeymans/fix-ci-arm64\n\nFix ARM64 closure addresses and a compositor focus test",
          "timestamp": "2026-10-06T15:14:04+02:00",
          "tree_id": "49809b4dbac445a0687797aaf473c84741fe0d0a",
          "url": "https://github.com/ArthurHeymans/techne/commit/fbec14a54f3b728b6ed2ec0cbe2e86470b53d4ae"
        },
        "date": 1791292732641,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "startup (jit)",
            "value": 11963913,
            "unit": "instructions"
          },
          {
            "name": "startup (interp)",
            "value": 11770491,
            "unit": "instructions"
          },
          {
            "name": "fib (jit)",
            "value": 935850342,
            "unit": "instructions"
          },
          {
            "name": "fib (interp)",
            "value": 1743206069,
            "unit": "instructions"
          },
          {
            "name": "tak (jit)",
            "value": 1172810678,
            "unit": "instructions"
          },
          {
            "name": "tak (interp)",
            "value": 2118695877,
            "unit": "instructions"
          },
          {
            "name": "nqueens (jit)",
            "value": 1239091036,
            "unit": "instructions"
          },
          {
            "name": "nqueens (interp)",
            "value": 2806676719,
            "unit": "instructions"
          },
          {
            "name": "nqueens GC pause",
            "value": 546,
            "unit": "words"
          },
          {
            "name": "bintrees (jit)",
            "value": 4300217291,
            "unit": "instructions"
          },
          {
            "name": "bintrees (interp)",
            "value": 8961198798,
            "unit": "instructions"
          },
          {
            "name": "bintrees GC pause",
            "value": 392904,
            "unit": "words"
          },
          {
            "name": "hof (jit)",
            "value": 1793066335,
            "unit": "instructions"
          },
          {
            "name": "hof (interp)",
            "value": 3160843741,
            "unit": "instructions"
          },
          {
            "name": "hof GC pause",
            "value": 1579648,
            "unit": "words"
          },
          {
            "name": "qsort (jit)",
            "value": 1510599340,
            "unit": "instructions"
          },
          {
            "name": "qsort (interp)",
            "value": 3667689706,
            "unit": "instructions"
          },
          {
            "name": "mandel (jit)",
            "value": 1042411034,
            "unit": "instructions"
          },
          {
            "name": "mandel (interp)",
            "value": 2734940953,
            "unit": "instructions"
          },
          {
            "name": "hash (jit)",
            "value": 764667436,
            "unit": "instructions"
          },
          {
            "name": "hash (interp)",
            "value": 839502452,
            "unit": "instructions"
          },
          {
            "name": "hash GC pause",
            "value": 229623,
            "unit": "words"
          },
          {
            "name": "orgparse (jit)",
            "value": 1244142981,
            "unit": "instructions"
          },
          {
            "name": "orgparse (interp)",
            "value": 1500610253,
            "unit": "instructions"
          }
        ]
      },
      {
        "commit": {
          "author": {
            "email": "arthur@aheymans.xyz",
            "name": "Arthur Heymans",
            "username": "ArthurHeymans"
          },
          "committer": {
            "email": "noreply@github.com",
            "name": "GitHub",
            "username": "web-flow"
          },
          "distinct": true,
          "id": "7a2e52de0535c2d1742a9ebe675d19add7e21e0c",
          "message": "Merge pull request #3 from ArthurHeymans/gc-promoted-words\n\nFix the collector's count of promoted words",
          "timestamp": "2026-10-06T17:13:47+02:00",
          "tree_id": "ebe01c53f4c29e91af1ff674f69d91646be6fc13",
          "url": "https://github.com/ArthurHeymans/techne/commit/7a2e52de0535c2d1742a9ebe675d19add7e21e0c"
        },
        "date": 1791299809357,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "startup (jit)",
            "value": 11979734,
            "unit": "instructions"
          },
          {
            "name": "startup (interp)",
            "value": 11774145,
            "unit": "instructions"
          },
          {
            "name": "fib (jit)",
            "value": 935851879,
            "unit": "instructions"
          },
          {
            "name": "fib (interp)",
            "value": 1743194541,
            "unit": "instructions"
          },
          {
            "name": "tak (jit)",
            "value": 1172812333,
            "unit": "instructions"
          },
          {
            "name": "tak (interp)",
            "value": 2118695410,
            "unit": "instructions"
          },
          {
            "name": "nqueens (jit)",
            "value": 1251662439,
            "unit": "instructions"
          },
          {
            "name": "nqueens (interp)",
            "value": 2819236853,
            "unit": "instructions"
          },
          {
            "name": "nqueens GC pause",
            "value": 546,
            "unit": "words"
          },
          {
            "name": "bintrees (jit)",
            "value": 4301051722,
            "unit": "instructions"
          },
          {
            "name": "bintrees (interp)",
            "value": 8962022477,
            "unit": "instructions"
          },
          {
            "name": "bintrees GC pause",
            "value": 392904,
            "unit": "words"
          },
          {
            "name": "hof (jit)",
            "value": 1793183042,
            "unit": "instructions"
          },
          {
            "name": "hof (interp)",
            "value": 3160947374,
            "unit": "instructions"
          },
          {
            "name": "hof GC pause",
            "value": 1579648,
            "unit": "words"
          },
          {
            "name": "qsort (jit)",
            "value": 1510625194,
            "unit": "instructions"
          },
          {
            "name": "qsort (interp)",
            "value": 3667690869,
            "unit": "instructions"
          },
          {
            "name": "mandel (jit)",
            "value": 1042438595,
            "unit": "instructions"
          },
          {
            "name": "mandel (interp)",
            "value": 2734944704,
            "unit": "instructions"
          },
          {
            "name": "hash (jit)",
            "value": 764702231,
            "unit": "instructions"
          },
          {
            "name": "hash (interp)",
            "value": 839555932,
            "unit": "instructions"
          },
          {
            "name": "hash GC pause",
            "value": 229623,
            "unit": "words"
          },
          {
            "name": "orgparse (jit)",
            "value": 1244067539,
            "unit": "instructions"
          },
          {
            "name": "orgparse (interp)",
            "value": 1500597003,
            "unit": "instructions"
          }
        ]
      },
      {
        "commit": {
          "author": {
            "email": "arthur@aheymans.xyz",
            "name": "Arthur Heymans",
            "username": "ArthurHeymans"
          },
          "committer": {
            "email": "noreply@github.com",
            "name": "GitHub",
            "username": "web-flow"
          },
          "distinct": true,
          "id": "c8b7c823df0f4c54170061070c02c38e4a5c880d",
          "message": "Merge pull request #4 from ArthurHeymans/editor-slice-2\n\nRun the editor in a window (slice 2)",
          "timestamp": "2026-10-06T17:30:02+02:00",
          "tree_id": "5594b6665641ea7fc591b1ddb126305fad6befa8",
          "url": "https://github.com/ArthurHeymans/techne/commit/c8b7c823df0f4c54170061070c02c38e4a5c880d"
        },
        "date": 1791300771139,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "startup (jit)",
            "value": 11980555,
            "unit": "instructions"
          },
          {
            "name": "startup (interp)",
            "value": 11774465,
            "unit": "instructions"
          },
          {
            "name": "fib (jit)",
            "value": 935854043,
            "unit": "instructions"
          },
          {
            "name": "fib (interp)",
            "value": 1743194518,
            "unit": "instructions"
          },
          {
            "name": "tak (jit)",
            "value": 1172814571,
            "unit": "instructions"
          },
          {
            "name": "tak (interp)",
            "value": 2118699776,
            "unit": "instructions"
          },
          {
            "name": "nqueens (jit)",
            "value": 1251657157,
            "unit": "instructions"
          },
          {
            "name": "nqueens (interp)",
            "value": 2819236354,
            "unit": "instructions"
          },
          {
            "name": "nqueens GC pause",
            "value": 546,
            "unit": "words"
          },
          {
            "name": "bintrees (jit)",
            "value": 4301069533,
            "unit": "instructions"
          },
          {
            "name": "bintrees (interp)",
            "value": 8962022863,
            "unit": "instructions"
          },
          {
            "name": "bintrees GC pause",
            "value": 392904,
            "unit": "words"
          },
          {
            "name": "hof (jit)",
            "value": 1793205614,
            "unit": "instructions"
          },
          {
            "name": "hof (interp)",
            "value": 3160947567,
            "unit": "instructions"
          },
          {
            "name": "hof GC pause",
            "value": 1579648,
            "unit": "words"
          },
          {
            "name": "qsort (jit)",
            "value": 1510608532,
            "unit": "instructions"
          },
          {
            "name": "qsort (interp)",
            "value": 3667685885,
            "unit": "instructions"
          },
          {
            "name": "mandel (jit)",
            "value": 1042419511,
            "unit": "instructions"
          },
          {
            "name": "mandel (interp)",
            "value": 2734940867,
            "unit": "instructions"
          },
          {
            "name": "hash (jit)",
            "value": 764680307,
            "unit": "instructions"
          },
          {
            "name": "hash (interp)",
            "value": 839550995,
            "unit": "instructions"
          },
          {
            "name": "hash GC pause",
            "value": 229623,
            "unit": "words"
          },
          {
            "name": "orgparse (jit)",
            "value": 1244103204,
            "unit": "instructions"
          },
          {
            "name": "orgparse (interp)",
            "value": 1500596563,
            "unit": "instructions"
          }
        ]
      },
      {
        "commit": {
          "author": {
            "email": "arthur@aheymans.xyz",
            "name": "Arthur Heymans",
            "username": "ArthurHeymans"
          },
          "committer": {
            "email": "noreply@github.com",
            "name": "GitHub",
            "username": "web-flow"
          },
          "distinct": true,
          "id": "12a6fa55915fc856b4826b70880e78ceaaf81e6d",
          "message": "Merge pull request #7 from ArthurHeymans/editor-slice-3\n\nRun the editor in a terminal (slice 3), onto main",
          "timestamp": "2026-10-06T19:04:35+02:00",
          "tree_id": "7e5a495147e7292e138e74cc8688db9684074ed6",
          "url": "https://github.com/ArthurHeymans/techne/commit/12a6fa55915fc856b4826b70880e78ceaaf81e6d"
        },
        "date": 1791306393402,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "startup (jit)",
            "value": 11980125,
            "unit": "instructions"
          },
          {
            "name": "startup (interp)",
            "value": 11774145,
            "unit": "instructions"
          },
          {
            "name": "fib (jit)",
            "value": 935850160,
            "unit": "instructions"
          },
          {
            "name": "fib (interp)",
            "value": 1743198949,
            "unit": "instructions"
          },
          {
            "name": "tak (jit)",
            "value": 1172809718,
            "unit": "instructions"
          },
          {
            "name": "tak (interp)",
            "value": 2118700202,
            "unit": "instructions"
          },
          {
            "name": "nqueens (jit)",
            "value": 1251654847,
            "unit": "instructions"
          },
          {
            "name": "nqueens (interp)",
            "value": 2819237168,
            "unit": "instructions"
          },
          {
            "name": "nqueens GC pause",
            "value": 546,
            "unit": "words"
          },
          {
            "name": "bintrees (jit)",
            "value": 4301027792,
            "unit": "instructions"
          },
          {
            "name": "bintrees (interp)",
            "value": 8962022197,
            "unit": "instructions"
          },
          {
            "name": "bintrees GC pause",
            "value": 392904,
            "unit": "words"
          },
          {
            "name": "hof (jit)",
            "value": 1793205829,
            "unit": "instructions"
          },
          {
            "name": "hof (interp)",
            "value": 3160957402,
            "unit": "instructions"
          },
          {
            "name": "hof GC pause",
            "value": 1579648,
            "unit": "words"
          },
          {
            "name": "qsort (jit)",
            "value": 1510599963,
            "unit": "instructions"
          },
          {
            "name": "qsort (interp)",
            "value": 3667687019,
            "unit": "instructions"
          },
          {
            "name": "mandel (jit)",
            "value": 1042407773,
            "unit": "instructions"
          },
          {
            "name": "mandel (interp)",
            "value": 2734944913,
            "unit": "instructions"
          },
          {
            "name": "hash (jit)",
            "value": 764688228,
            "unit": "instructions"
          },
          {
            "name": "hash (interp)",
            "value": 839551404,
            "unit": "instructions"
          },
          {
            "name": "hash GC pause",
            "value": 229623,
            "unit": "words"
          },
          {
            "name": "orgparse (jit)",
            "value": 1244254788,
            "unit": "instructions"
          },
          {
            "name": "orgparse (interp)",
            "value": 1500597003,
            "unit": "instructions"
          }
        ]
      }
    ]
  }
}