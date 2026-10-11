window.BENCHMARK_DATA = {
  "lastUpdate": 1791737507561,
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
          "id": "28694238080daec7b829f91710dbc8ed355aaad0",
          "message": "Merge pull request #8 from ArthurHeymans/language-step-0\n\nR7RS conformance: fixes, missing procedures, libraries (language step 0)",
          "timestamp": "2026-10-06T19:34:23+02:00",
          "tree_id": "0968c5a47d84825ff157c362084914feb071e434",
          "url": "https://github.com/ArthurHeymans/techne/commit/28694238080daec7b829f91710dbc8ed355aaad0"
        },
        "date": 1791308231722,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "startup (jit)",
            "value": 12044403,
            "unit": "instructions"
          },
          {
            "name": "startup (interp)",
            "value": 11884661,
            "unit": "instructions"
          },
          {
            "name": "startup GC pause",
            "value": 294,
            "unit": "words"
          },
          {
            "name": "fib (jit)",
            "value": 935921043,
            "unit": "instructions"
          },
          {
            "name": "fib (interp)",
            "value": 1743297966,
            "unit": "instructions"
          },
          {
            "name": "fib GC pause",
            "value": 294,
            "unit": "words"
          },
          {
            "name": "tak (jit)",
            "value": 1172858686,
            "unit": "instructions"
          },
          {
            "name": "tak (interp)",
            "value": 2118774250,
            "unit": "instructions"
          },
          {
            "name": "tak GC pause",
            "value": 294,
            "unit": "words"
          },
          {
            "name": "nqueens (jit)",
            "value": 1254226471,
            "unit": "instructions"
          },
          {
            "name": "nqueens (interp)",
            "value": 2790764039,
            "unit": "instructions"
          },
          {
            "name": "nqueens GC pause",
            "value": 345,
            "unit": "words"
          },
          {
            "name": "bintrees (jit)",
            "value": 4301553853,
            "unit": "instructions"
          },
          {
            "name": "bintrees (interp)",
            "value": 8902787248,
            "unit": "instructions"
          },
          {
            "name": "bintrees GC pause",
            "value": 393138,
            "unit": "words"
          },
          {
            "name": "hof (jit)",
            "value": 1794573751,
            "unit": "instructions"
          },
          {
            "name": "hof (interp)",
            "value": 3143342072,
            "unit": "instructions"
          },
          {
            "name": "hof GC pause",
            "value": 1579648,
            "unit": "words"
          },
          {
            "name": "qsort (jit)",
            "value": 1510543601,
            "unit": "instructions"
          },
          {
            "name": "qsort (interp)",
            "value": 3667644163,
            "unit": "instructions"
          },
          {
            "name": "qsort GC pause",
            "value": 294,
            "unit": "words"
          },
          {
            "name": "mandel (jit)",
            "value": 1042401870,
            "unit": "instructions"
          },
          {
            "name": "mandel (interp)",
            "value": 2734926390,
            "unit": "instructions"
          },
          {
            "name": "mandel GC pause",
            "value": 294,
            "unit": "words"
          },
          {
            "name": "hash (jit)",
            "value": 766744141,
            "unit": "instructions"
          },
          {
            "name": "hash (interp)",
            "value": 841644506,
            "unit": "instructions"
          },
          {
            "name": "hash GC pause",
            "value": 229389,
            "unit": "words"
          },
          {
            "name": "orgparse (jit)",
            "value": 1213750779,
            "unit": "instructions"
          },
          {
            "name": "orgparse (interp)",
            "value": 1470152949,
            "unit": "instructions"
          },
          {
            "name": "orgparse GC pause",
            "value": 294,
            "unit": "words"
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
          "id": "1515ef507d23c59d506cc2ee77e3e309ff534557",
          "message": "Merge pull request #9 from ArthurHeymans/ci-faster\n\nFaster CI: a cache per job, nextest, no debug info",
          "timestamp": "2026-10-06T19:34:42+02:00",
          "tree_id": "f1ada0ee53862e9745910ea0776b8b68e23dfb25",
          "url": "https://github.com/ArthurHeymans/techne/commit/1515ef507d23c59d506cc2ee77e3e309ff534557"
        },
        "date": 1791309117487,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "startup (jit)",
            "value": 12050825,
            "unit": "instructions"
          },
          {
            "name": "startup (interp)",
            "value": 11873655,
            "unit": "instructions"
          },
          {
            "name": "startup GC pause",
            "value": 294,
            "unit": "words"
          },
          {
            "name": "fib (jit)",
            "value": 935913761,
            "unit": "instructions"
          },
          {
            "name": "fib (interp)",
            "value": 1743289645,
            "unit": "instructions"
          },
          {
            "name": "fib GC pause",
            "value": 294,
            "unit": "words"
          },
          {
            "name": "tak (jit)",
            "value": 1172845798,
            "unit": "instructions"
          },
          {
            "name": "tak (interp)",
            "value": 2118765882,
            "unit": "instructions"
          },
          {
            "name": "tak GC pause",
            "value": 294,
            "unit": "words"
          },
          {
            "name": "nqueens (jit)",
            "value": 1254215859,
            "unit": "instructions"
          },
          {
            "name": "nqueens (interp)",
            "value": 2790757654,
            "unit": "instructions"
          },
          {
            "name": "nqueens GC pause",
            "value": 345,
            "unit": "words"
          },
          {
            "name": "bintrees (jit)",
            "value": 4301546637,
            "unit": "instructions"
          },
          {
            "name": "bintrees (interp)",
            "value": 8902775839,
            "unit": "instructions"
          },
          {
            "name": "bintrees GC pause",
            "value": 393138,
            "unit": "words"
          },
          {
            "name": "hof (jit)",
            "value": 1794572584,
            "unit": "instructions"
          },
          {
            "name": "hof (interp)",
            "value": 3143328968,
            "unit": "instructions"
          },
          {
            "name": "hof GC pause",
            "value": 1579648,
            "unit": "words"
          },
          {
            "name": "qsort (jit)",
            "value": 1510542931,
            "unit": "instructions"
          },
          {
            "name": "qsort (interp)",
            "value": 3667632300,
            "unit": "instructions"
          },
          {
            "name": "qsort GC pause",
            "value": 294,
            "unit": "words"
          },
          {
            "name": "mandel (jit)",
            "value": 1042289569,
            "unit": "instructions"
          },
          {
            "name": "mandel (interp)",
            "value": 2734919485,
            "unit": "instructions"
          },
          {
            "name": "mandel GC pause",
            "value": 294,
            "unit": "words"
          },
          {
            "name": "hash (jit)",
            "value": 766753430,
            "unit": "instructions"
          },
          {
            "name": "hash (interp)",
            "value": 841635988,
            "unit": "instructions"
          },
          {
            "name": "hash GC pause",
            "value": 229389,
            "unit": "words"
          },
          {
            "name": "orgparse (jit)",
            "value": 1213848033,
            "unit": "instructions"
          },
          {
            "name": "orgparse (interp)",
            "value": 1470141494,
            "unit": "instructions"
          },
          {
            "name": "orgparse GC pause",
            "value": 294,
            "unit": "words"
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
          "id": "56d4c8c367ec05614e0884c587a8608fc7b7bf2a",
          "message": "Merge pull request #10 from ArthurHeymans/live-loop\n\nLive editing in two views (slice 4) on owned scopes and packages, replacing the parallel slice 3 and step 0",
          "timestamp": "2026-10-07T10:22:18+02:00",
          "tree_id": "0f70e2522c841027c14486cdb0bf826dcbbfa90a",
          "url": "https://github.com/ArthurHeymans/techne/commit/56d4c8c367ec05614e0884c587a8608fc7b7bf2a"
        },
        "date": 1791361605819,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "startup (jit)",
            "value": 11773961,
            "unit": "instructions"
          },
          {
            "name": "startup (interp)",
            "value": 11613120,
            "unit": "instructions"
          },
          {
            "name": "fib (jit)",
            "value": 935646248,
            "unit": "instructions"
          },
          {
            "name": "fib (interp)",
            "value": 1693413461,
            "unit": "instructions"
          },
          {
            "name": "tak (jit)",
            "value": 1172590758,
            "unit": "instructions"
          },
          {
            "name": "tak (interp)",
            "value": 2129449843,
            "unit": "instructions"
          },
          {
            "name": "nqueens (jit)",
            "value": 1245379517,
            "unit": "instructions"
          },
          {
            "name": "nqueens (interp)",
            "value": 2817265399,
            "unit": "instructions"
          },
          {
            "name": "nqueens GC pause",
            "value": 345,
            "unit": "words"
          },
          {
            "name": "bintrees (jit)",
            "value": 4320505163,
            "unit": "instructions"
          },
          {
            "name": "bintrees (interp)",
            "value": 8936006066,
            "unit": "instructions"
          },
          {
            "name": "bintrees GC pause",
            "value": 393138,
            "unit": "words"
          },
          {
            "name": "hof (jit)",
            "value": 1800624251,
            "unit": "instructions"
          },
          {
            "name": "hof (interp)",
            "value": 3216713706,
            "unit": "instructions"
          },
          {
            "name": "hof GC pause",
            "value": 1579648,
            "unit": "words"
          },
          {
            "name": "qsort (jit)",
            "value": 1510187863,
            "unit": "instructions"
          },
          {
            "name": "qsort (interp)",
            "value": 3746886671,
            "unit": "instructions"
          },
          {
            "name": "mandel (jit)",
            "value": 1034789435,
            "unit": "instructions"
          },
          {
            "name": "mandel (interp)",
            "value": 2582779916,
            "unit": "instructions"
          },
          {
            "name": "hash (jit)",
            "value": 746504022,
            "unit": "instructions"
          },
          {
            "name": "hash (interp)",
            "value": 823084373,
            "unit": "instructions"
          },
          {
            "name": "hash GC pause",
            "value": 229391,
            "unit": "words"
          },
          {
            "name": "orgparse (jit)",
            "value": 1118373527,
            "unit": "instructions"
          },
          {
            "name": "orgparse (interp)",
            "value": 1389745248,
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
          "id": "56d4c8c367ec05614e0884c587a8608fc7b7bf2a",
          "message": "Merge pull request #10 from ArthurHeymans/live-loop\n\nLive editing in two views (slice 4) on owned scopes and packages, replacing the parallel slice 3 and step 0",
          "timestamp": "2026-10-07T10:22:18+02:00",
          "tree_id": "0f70e2522c841027c14486cdb0bf826dcbbfa90a",
          "url": "https://github.com/ArthurHeymans/techne/commit/56d4c8c367ec05614e0884c587a8608fc7b7bf2a"
        },
        "date": 1791362198947,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "startup (jit)",
            "value": 11757991,
            "unit": "instructions"
          },
          {
            "name": "startup (interp)",
            "value": 11613120,
            "unit": "instructions"
          },
          {
            "name": "fib (jit)",
            "value": 935643614,
            "unit": "instructions"
          },
          {
            "name": "fib (interp)",
            "value": 1693409020,
            "unit": "instructions"
          },
          {
            "name": "tak (jit)",
            "value": 1172586361,
            "unit": "instructions"
          },
          {
            "name": "tak (interp)",
            "value": 2129449843,
            "unit": "instructions"
          },
          {
            "name": "nqueens (jit)",
            "value": 1245378530,
            "unit": "instructions"
          },
          {
            "name": "nqueens (interp)",
            "value": 2817265399,
            "unit": "instructions"
          },
          {
            "name": "nqueens GC pause",
            "value": 345,
            "unit": "words"
          },
          {
            "name": "bintrees (jit)",
            "value": 4320553319,
            "unit": "instructions"
          },
          {
            "name": "bintrees (interp)",
            "value": 8936001625,
            "unit": "instructions"
          },
          {
            "name": "bintrees GC pause",
            "value": 393138,
            "unit": "words"
          },
          {
            "name": "hof (jit)",
            "value": 1800624636,
            "unit": "instructions"
          },
          {
            "name": "hof (interp)",
            "value": 3216709265,
            "unit": "instructions"
          },
          {
            "name": "hof GC pause",
            "value": 1579648,
            "unit": "words"
          },
          {
            "name": "qsort (jit)",
            "value": 1510225659,
            "unit": "instructions"
          },
          {
            "name": "qsort (interp)",
            "value": 3746886671,
            "unit": "instructions"
          },
          {
            "name": "mandel (jit)",
            "value": 1034800633,
            "unit": "instructions"
          },
          {
            "name": "mandel (interp)",
            "value": 2582779916,
            "unit": "instructions"
          },
          {
            "name": "hash (jit)",
            "value": 746507967,
            "unit": "instructions"
          },
          {
            "name": "hash (interp)",
            "value": 823084373,
            "unit": "instructions"
          },
          {
            "name": "hash GC pause",
            "value": 229391,
            "unit": "words"
          },
          {
            "name": "orgparse (jit)",
            "value": 1118439479,
            "unit": "instructions"
          },
          {
            "name": "orgparse (interp)",
            "value": 1389745248,
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
          "id": "504fa340232fb96cbc3e2acd0c8b877270957411",
          "message": "Merge pull request #12 from ArthurHeymans/editor-session\n\nBring the whole session back after a runtime crash; faster narrowing in the minibuffer",
          "timestamp": "2026-10-07T11:18:13+02:00",
          "tree_id": "5defc6534e926bd71af761af1fd16ab9c1a8e008",
          "url": "https://github.com/ArthurHeymans/techne/commit/504fa340232fb96cbc3e2acd0c8b877270957411"
        },
        "date": 1791364852088,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "startup (jit)",
            "value": 11757991,
            "unit": "instructions"
          },
          {
            "name": "startup (interp)",
            "value": 11613120,
            "unit": "instructions"
          },
          {
            "name": "fib (jit)",
            "value": 935642776,
            "unit": "instructions"
          },
          {
            "name": "fib (interp)",
            "value": 1693409020,
            "unit": "instructions"
          },
          {
            "name": "tak (jit)",
            "value": 1172582450,
            "unit": "instructions"
          },
          {
            "name": "tak (interp)",
            "value": 2129454284,
            "unit": "instructions"
          },
          {
            "name": "nqueens (jit)",
            "value": 1245381136,
            "unit": "instructions"
          },
          {
            "name": "nqueens (interp)",
            "value": 2817260958,
            "unit": "instructions"
          },
          {
            "name": "nqueens GC pause",
            "value": 345,
            "unit": "words"
          },
          {
            "name": "bintrees (jit)",
            "value": 4320518271,
            "unit": "instructions"
          },
          {
            "name": "bintrees (interp)",
            "value": 8936006066,
            "unit": "instructions"
          },
          {
            "name": "bintrees GC pause",
            "value": 393138,
            "unit": "words"
          },
          {
            "name": "hof (jit)",
            "value": 1800635835,
            "unit": "instructions"
          },
          {
            "name": "hof (interp)",
            "value": 3216709265,
            "unit": "instructions"
          },
          {
            "name": "hof GC pause",
            "value": 1579648,
            "unit": "words"
          },
          {
            "name": "qsort (jit)",
            "value": 1510204728,
            "unit": "instructions"
          },
          {
            "name": "qsort (interp)",
            "value": 3746886671,
            "unit": "instructions"
          },
          {
            "name": "mandel (jit)",
            "value": 1034792048,
            "unit": "instructions"
          },
          {
            "name": "mandel (interp)",
            "value": 2582779916,
            "unit": "instructions"
          },
          {
            "name": "hash (jit)",
            "value": 746501045,
            "unit": "instructions"
          },
          {
            "name": "hash (interp)",
            "value": 823088814,
            "unit": "instructions"
          },
          {
            "name": "hash GC pause",
            "value": 229391,
            "unit": "words"
          },
          {
            "name": "orgparse (jit)",
            "value": 1118460026,
            "unit": "instructions"
          },
          {
            "name": "orgparse (interp)",
            "value": 1389745248,
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
          "id": "ccdfb9fceb337b8e0eb4366700351057174cc691",
          "message": "Merge pull request #27 from ArthurHeymans/close-crash\n\nFix a segmentation fault when closing the window",
          "timestamp": "2026-10-07T13:59:07+02:00",
          "tree_id": "433ba079e9440d34a64e93bf4c433fd306d69350",
          "url": "https://github.com/ArthurHeymans/techne/commit/ccdfb9fceb337b8e0eb4366700351057174cc691"
        },
        "date": 1791374606750,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "startup (jit)",
            "value": 11762657,
            "unit": "instructions"
          },
          {
            "name": "startup (interp)",
            "value": 11616465,
            "unit": "instructions"
          },
          {
            "name": "fib (jit)",
            "value": 935643066,
            "unit": "instructions"
          },
          {
            "name": "fib (interp)",
            "value": 1693413800,
            "unit": "instructions"
          },
          {
            "name": "tak (jit)",
            "value": 1172591570,
            "unit": "instructions"
          },
          {
            "name": "tak (interp)",
            "value": 2129459999,
            "unit": "instructions"
          },
          {
            "name": "nqueens (jit)",
            "value": 1245374561,
            "unit": "instructions"
          },
          {
            "name": "nqueens (interp)",
            "value": 2817312813,
            "unit": "instructions"
          },
          {
            "name": "nqueens GC pause",
            "value": 345,
            "unit": "words"
          },
          {
            "name": "bintrees (jit)",
            "value": 4320534139,
            "unit": "instructions"
          },
          {
            "name": "bintrees (interp)",
            "value": 8936009507,
            "unit": "instructions"
          },
          {
            "name": "bintrees GC pause",
            "value": 393138,
            "unit": "words"
          },
          {
            "name": "hof (jit)",
            "value": 1800645742,
            "unit": "instructions"
          },
          {
            "name": "hof (interp)",
            "value": 3216706272,
            "unit": "instructions"
          },
          {
            "name": "hof GC pause",
            "value": 1579648,
            "unit": "words"
          },
          {
            "name": "qsort (jit)",
            "value": 1510222112,
            "unit": "instructions"
          },
          {
            "name": "qsort (interp)",
            "value": 3746888154,
            "unit": "instructions"
          },
          {
            "name": "mandel (jit)",
            "value": 1034788138,
            "unit": "instructions"
          },
          {
            "name": "mandel (interp)",
            "value": 2582780044,
            "unit": "instructions"
          },
          {
            "name": "hash (jit)",
            "value": 746542835,
            "unit": "instructions"
          },
          {
            "name": "hash (interp)",
            "value": 823089779,
            "unit": "instructions"
          },
          {
            "name": "hash GC pause",
            "value": 229391,
            "unit": "words"
          },
          {
            "name": "orgparse (jit)",
            "value": 1118473365,
            "unit": "instructions"
          },
          {
            "name": "orgparse (interp)",
            "value": 1389745997,
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
          "id": "c1f4ada901a3430421ae8fdf36423050efe11e38",
          "message": "Merge pull request #34 from ArthurHeymans/buffer-completion\n\nCompletion popup in itl and Scheme buffers, as Arthur's Corfu",
          "timestamp": "2026-10-07T15:53:54+02:00",
          "tree_id": "842a19d6686d16f225e9492149deed4a63a5e4e1",
          "url": "https://github.com/ArthurHeymans/techne/commit/c1f4ada901a3430421ae8fdf36423050efe11e38"
        },
        "date": 1791381411175,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "startup (jit)",
            "value": 11766228,
            "unit": "instructions"
          },
          {
            "name": "startup (interp)",
            "value": 11615072,
            "unit": "instructions"
          },
          {
            "name": "fib (jit)",
            "value": 935644378,
            "unit": "instructions"
          },
          {
            "name": "fib (interp)",
            "value": 1693412051,
            "unit": "instructions"
          },
          {
            "name": "tak (jit)",
            "value": 1172598950,
            "unit": "instructions"
          },
          {
            "name": "tak (interp)",
            "value": 2129454564,
            "unit": "instructions"
          },
          {
            "name": "nqueens (jit)",
            "value": 1245369091,
            "unit": "instructions"
          },
          {
            "name": "nqueens (interp)",
            "value": 2817318399,
            "unit": "instructions"
          },
          {
            "name": "nqueens GC pause",
            "value": 345,
            "unit": "words"
          },
          {
            "name": "bintrees (jit)",
            "value": 4320512284,
            "unit": "instructions"
          },
          {
            "name": "bintrees (interp)",
            "value": 8936007628,
            "unit": "instructions"
          },
          {
            "name": "bintrees GC pause",
            "value": 393138,
            "unit": "words"
          },
          {
            "name": "hof (jit)",
            "value": 1800617843,
            "unit": "instructions"
          },
          {
            "name": "hof (interp)",
            "value": 3216707228,
            "unit": "instructions"
          },
          {
            "name": "hof GC pause",
            "value": 1579648,
            "unit": "words"
          },
          {
            "name": "qsort (jit)",
            "value": 1510234545,
            "unit": "instructions"
          },
          {
            "name": "qsort (interp)",
            "value": 3746886698,
            "unit": "instructions"
          },
          {
            "name": "mandel (jit)",
            "value": 1034781355,
            "unit": "instructions"
          },
          {
            "name": "mandel (interp)",
            "value": 2582782658,
            "unit": "instructions"
          },
          {
            "name": "hash (jit)",
            "value": 746503564,
            "unit": "instructions"
          },
          {
            "name": "hash (interp)",
            "value": 823090906,
            "unit": "instructions"
          },
          {
            "name": "hash GC pause",
            "value": 229391,
            "unit": "words"
          },
          {
            "name": "orgparse (jit)",
            "value": 1118457570,
            "unit": "instructions"
          },
          {
            "name": "orgparse (interp)",
            "value": 1389743580,
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
          "id": "3524a25b3831148f76f46a59c64416df69f3fec9",
          "message": "Merge pull request #36 from ArthurHeymans/terminal-redraw\n\nMake terminal redraws smaller on large consoles",
          "timestamp": "2026-10-07T16:19:03+02:00",
          "tree_id": "686e31371274c71f9e2d119ff4cd9fa0b88a3147",
          "url": "https://github.com/ArthurHeymans/techne/commit/3524a25b3831148f76f46a59c64416df69f3fec9"
        },
        "date": 1791382907578,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "startup (jit)",
            "value": 11761787,
            "unit": "instructions"
          },
          {
            "name": "startup (interp)",
            "value": 11615072,
            "unit": "instructions"
          },
          {
            "name": "fib (jit)",
            "value": 935647220,
            "unit": "instructions"
          },
          {
            "name": "fib (interp)",
            "value": 1693407610,
            "unit": "instructions"
          },
          {
            "name": "tak (jit)",
            "value": 1172598896,
            "unit": "instructions"
          },
          {
            "name": "tak (interp)",
            "value": 2129454564,
            "unit": "instructions"
          },
          {
            "name": "nqueens (jit)",
            "value": 1245373631,
            "unit": "instructions"
          },
          {
            "name": "nqueens (interp)",
            "value": 2817322840,
            "unit": "instructions"
          },
          {
            "name": "nqueens GC pause",
            "value": 345,
            "unit": "words"
          },
          {
            "name": "bintrees (jit)",
            "value": 4320565944,
            "unit": "instructions"
          },
          {
            "name": "bintrees (interp)",
            "value": 8936003187,
            "unit": "instructions"
          },
          {
            "name": "bintrees GC pause",
            "value": 393138,
            "unit": "words"
          },
          {
            "name": "hof (jit)",
            "value": 1800637177,
            "unit": "instructions"
          },
          {
            "name": "hof (interp)",
            "value": 3216702787,
            "unit": "instructions"
          },
          {
            "name": "hof GC pause",
            "value": 1579648,
            "unit": "words"
          },
          {
            "name": "qsort (jit)",
            "value": 1510224075,
            "unit": "instructions"
          },
          {
            "name": "qsort (interp)",
            "value": 3746886698,
            "unit": "instructions"
          },
          {
            "name": "mandel (jit)",
            "value": 1034784885,
            "unit": "instructions"
          },
          {
            "name": "mandel (interp)",
            "value": 2582782658,
            "unit": "instructions"
          },
          {
            "name": "hash (jit)",
            "value": 746500530,
            "unit": "instructions"
          },
          {
            "name": "hash (interp)",
            "value": 823086465,
            "unit": "instructions"
          },
          {
            "name": "hash GC pause",
            "value": 229391,
            "unit": "words"
          },
          {
            "name": "orgparse (jit)",
            "value": 1118428701,
            "unit": "instructions"
          },
          {
            "name": "orgparse (interp)",
            "value": 1389743580,
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
          "id": "9a86143271fe76ff3a02d7014952e14bb43314d3",
          "message": "Merge pull request #37 from ArthurHeymans/r7rs-gaps\n\nRun portable Scheme: bytes, exact fractions, complex numbers, SRFI libraries",
          "timestamp": "2026-10-07T23:57:24+02:00",
          "tree_id": "0a1e980a82ee75fa313d43f8a06d687805d4fb19",
          "url": "https://github.com/ArthurHeymans/techne/commit/9a86143271fe76ff3a02d7014952e14bb43314d3"
        },
        "date": 1791410403404,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "startup (jit)",
            "value": 12037227,
            "unit": "instructions"
          },
          {
            "name": "startup (interp)",
            "value": 11893494,
            "unit": "instructions"
          },
          {
            "name": "fib (jit)",
            "value": 935919935,
            "unit": "instructions"
          },
          {
            "name": "fib (interp)",
            "value": 1693691003,
            "unit": "instructions"
          },
          {
            "name": "tak (jit)",
            "value": 1172872886,
            "unit": "instructions"
          },
          {
            "name": "tak (interp)",
            "value": 2129738418,
            "unit": "instructions"
          },
          {
            "name": "nqueens (jit)",
            "value": 1245671126,
            "unit": "instructions"
          },
          {
            "name": "nqueens (interp)",
            "value": 2816936119,
            "unit": "instructions"
          },
          {
            "name": "nqueens GC pause",
            "value": 345,
            "unit": "words"
          },
          {
            "name": "bintrees (jit)",
            "value": 4326608164,
            "unit": "instructions"
          },
          {
            "name": "bintrees (interp)",
            "value": 8942119661,
            "unit": "instructions"
          },
          {
            "name": "bintrees GC pause",
            "value": 393138,
            "unit": "words"
          },
          {
            "name": "hof (jit)",
            "value": 1869587946,
            "unit": "instructions"
          },
          {
            "name": "hof (interp)",
            "value": 3285611432,
            "unit": "instructions"
          },
          {
            "name": "hof GC pause",
            "value": 1579648,
            "unit": "words"
          },
          {
            "name": "qsort (jit)",
            "value": 1519381638,
            "unit": "instructions"
          },
          {
            "name": "qsort (interp)",
            "value": 3753381894,
            "unit": "instructions"
          },
          {
            "name": "mandel (jit)",
            "value": 1025308118,
            "unit": "instructions"
          },
          {
            "name": "mandel (interp)",
            "value": 2573319741,
            "unit": "instructions"
          },
          {
            "name": "hash (jit)",
            "value": 764086341,
            "unit": "instructions"
          },
          {
            "name": "hash (interp)",
            "value": 840651309,
            "unit": "instructions"
          },
          {
            "name": "hash GC pause",
            "value": 229391,
            "unit": "words"
          },
          {
            "name": "orgparse (jit)",
            "value": 1130449150,
            "unit": "instructions"
          },
          {
            "name": "orgparse (interp)",
            "value": 1401814711,
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
          "id": "f5de7701b76ff4bee0cb8cf9c2cbea271d8afc60",
          "message": "Merge pull request #40 from ArthurHeymans/srfi-reference\n\nCheck SRFI reference implementations and fuzz exact and complex arithmetic",
          "timestamp": "2026-10-08T06:33:10+02:00",
          "tree_id": "6336535091acc1facc6172a501cb8d5f76fbda42",
          "url": "https://github.com/ArthurHeymans/techne/commit/f5de7701b76ff4bee0cb8cf9c2cbea271d8afc60"
        },
        "date": 1791434257861,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "startup (jit)",
            "value": 12057418,
            "unit": "instructions"
          },
          {
            "name": "startup (interp)",
            "value": 11894097,
            "unit": "instructions"
          },
          {
            "name": "fib (jit)",
            "value": 935924410,
            "unit": "instructions"
          },
          {
            "name": "fib (interp)",
            "value": 1693692477,
            "unit": "instructions"
          },
          {
            "name": "tak (jit)",
            "value": 1172873512,
            "unit": "instructions"
          },
          {
            "name": "tak (interp)",
            "value": 2129743448,
            "unit": "instructions"
          },
          {
            "name": "nqueens (jit)",
            "value": 1245671013,
            "unit": "instructions"
          },
          {
            "name": "nqueens (interp)",
            "value": 2816936722,
            "unit": "instructions"
          },
          {
            "name": "nqueens GC pause",
            "value": 345,
            "unit": "words"
          },
          {
            "name": "bintrees (jit)",
            "value": 4326604379,
            "unit": "instructions"
          },
          {
            "name": "bintrees (interp)",
            "value": 8942124691,
            "unit": "instructions"
          },
          {
            "name": "bintrees GC pause",
            "value": 393138,
            "unit": "words"
          },
          {
            "name": "hof (jit)",
            "value": 1869591706,
            "unit": "instructions"
          },
          {
            "name": "hof (interp)",
            "value": 3285612021,
            "unit": "instructions"
          },
          {
            "name": "hof GC pause",
            "value": 1579648,
            "unit": "words"
          },
          {
            "name": "qsort (jit)",
            "value": 1519422160,
            "unit": "instructions"
          },
          {
            "name": "qsort (interp)",
            "value": 3753378045,
            "unit": "instructions"
          },
          {
            "name": "mandel (jit)",
            "value": 1025290247,
            "unit": "instructions"
          },
          {
            "name": "mandel (interp)",
            "value": 2573324797,
            "unit": "instructions"
          },
          {
            "name": "hash (jit)",
            "value": 764066667,
            "unit": "instructions"
          },
          {
            "name": "hash (interp)",
            "value": 840651884,
            "unit": "instructions"
          },
          {
            "name": "hash GC pause",
            "value": 229391,
            "unit": "words"
          },
          {
            "name": "orgparse (jit)",
            "value": 1130365649,
            "unit": "instructions"
          },
          {
            "name": "orgparse (interp)",
            "value": 1401815308,
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
          "id": "c1eef3ed11c2b8092497ced0973948621adaf4ec",
          "message": "Merge pull request #38 from ArthurHeymans/architecture-review\n\nFirmer extension contracts: owned overrides, revision-checked edits, keyed rows",
          "timestamp": "2026-10-08T06:54:05+02:00",
          "tree_id": "473dfb720e5ba26a6806d524dfa87532ddfadb3b",
          "url": "https://github.com/ArthurHeymans/techne/commit/c1eef3ed11c2b8092497ced0973948621adaf4ec"
        },
        "date": 1791435516711,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "startup (jit)",
            "value": 12296461,
            "unit": "instructions"
          },
          {
            "name": "startup (interp)",
            "value": 12137691,
            "unit": "instructions"
          },
          {
            "name": "fib (jit)",
            "value": 936179711,
            "unit": "instructions"
          },
          {
            "name": "fib (interp)",
            "value": 1693939605,
            "unit": "instructions"
          },
          {
            "name": "tak (jit)",
            "value": 1173121425,
            "unit": "instructions"
          },
          {
            "name": "tak (interp)",
            "value": 2129983993,
            "unit": "instructions"
          },
          {
            "name": "nqueens (jit)",
            "value": 1245925030,
            "unit": "instructions"
          },
          {
            "name": "nqueens (interp)",
            "value": 2817875581,
            "unit": "instructions"
          },
          {
            "name": "nqueens GC pause",
            "value": 345,
            "unit": "words"
          },
          {
            "name": "bintrees (jit)",
            "value": 4326927889,
            "unit": "instructions"
          },
          {
            "name": "bintrees (interp)",
            "value": 8942385645,
            "unit": "instructions"
          },
          {
            "name": "bintrees GC pause",
            "value": 393138,
            "unit": "words"
          },
          {
            "name": "hof (jit)",
            "value": 1869946680,
            "unit": "instructions"
          },
          {
            "name": "hof (interp)",
            "value": 3285952992,
            "unit": "instructions"
          },
          {
            "name": "hof GC pause",
            "value": 1579648,
            "unit": "words"
          },
          {
            "name": "qsort (jit)",
            "value": 1519644856,
            "unit": "instructions"
          },
          {
            "name": "qsort (interp)",
            "value": 3753617437,
            "unit": "instructions"
          },
          {
            "name": "mandel (jit)",
            "value": 1025537933,
            "unit": "instructions"
          },
          {
            "name": "mandel (interp)",
            "value": 2573567571,
            "unit": "instructions"
          },
          {
            "name": "hash (jit)",
            "value": 764333584,
            "unit": "instructions"
          },
          {
            "name": "hash (interp)",
            "value": 840893557,
            "unit": "instructions"
          },
          {
            "name": "hash GC pause",
            "value": 229391,
            "unit": "words"
          },
          {
            "name": "orgparse (jit)",
            "value": 1130690036,
            "unit": "instructions"
          },
          {
            "name": "orgparse (interp)",
            "value": 1402093922,
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
          "id": "ebb12063dbddc04fbe0c825c73908e6bd0dca5b1",
          "message": "Merge pull request #39 from ArthurHeymans/vc-status\n\nVersion control status as a package, with background refresh",
          "timestamp": "2026-10-08T06:54:19+02:00",
          "tree_id": "d1f2eef4ff1692190721b32ac3b9b92899e8c7f7",
          "url": "https://github.com/ArthurHeymans/techne/commit/ebb12063dbddc04fbe0c825c73908e6bd0dca5b1"
        },
        "date": 1791435807349,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "startup (jit)",
            "value": 12311537,
            "unit": "instructions"
          },
          {
            "name": "startup (interp)",
            "value": 12142139,
            "unit": "instructions"
          },
          {
            "name": "fib (jit)",
            "value": 936180097,
            "unit": "instructions"
          },
          {
            "name": "fib (interp)",
            "value": 1693939605,
            "unit": "instructions"
          },
          {
            "name": "tak (jit)",
            "value": 1173120895,
            "unit": "instructions"
          },
          {
            "name": "tak (interp)",
            "value": 2129983993,
            "unit": "instructions"
          },
          {
            "name": "nqueens (jit)",
            "value": 1245925718,
            "unit": "instructions"
          },
          {
            "name": "nqueens (interp)",
            "value": 2817875581,
            "unit": "instructions"
          },
          {
            "name": "nqueens GC pause",
            "value": 345,
            "unit": "words"
          },
          {
            "name": "bintrees (jit)",
            "value": 4326897151,
            "unit": "instructions"
          },
          {
            "name": "bintrees (interp)",
            "value": 8942385645,
            "unit": "instructions"
          },
          {
            "name": "bintrees GC pause",
            "value": 393138,
            "unit": "words"
          },
          {
            "name": "hof (jit)",
            "value": 1869948315,
            "unit": "instructions"
          },
          {
            "name": "hof (interp)",
            "value": 3285952992,
            "unit": "instructions"
          },
          {
            "name": "hof GC pause",
            "value": 1579648,
            "unit": "words"
          },
          {
            "name": "qsort (jit)",
            "value": 1519656242,
            "unit": "instructions"
          },
          {
            "name": "qsort (interp)",
            "value": 3753621885,
            "unit": "instructions"
          },
          {
            "name": "mandel (jit)",
            "value": 1025574601,
            "unit": "instructions"
          },
          {
            "name": "mandel (interp)",
            "value": 2573567571,
            "unit": "instructions"
          },
          {
            "name": "hash (jit)",
            "value": 764318947,
            "unit": "instructions"
          },
          {
            "name": "hash (interp)",
            "value": 840893557,
            "unit": "instructions"
          },
          {
            "name": "hash GC pause",
            "value": 229391,
            "unit": "words"
          },
          {
            "name": "orgparse (jit)",
            "value": 1130695457,
            "unit": "instructions"
          },
          {
            "name": "orgparse (interp)",
            "value": 1402093922,
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
          "id": "62235a4c3c8357eb23782096749546f01e1becf7",
          "message": "Merge pull request #41 from ArthurHeymans/self-documentation\n\nA self-documenting editor: help for everything, checked by CI",
          "timestamp": "2026-10-08T08:12:17+02:00",
          "tree_id": "0cc174dfd8bef74fed680dad7aa62b8f8b2f775b",
          "url": "https://github.com/ArthurHeymans/techne/commit/62235a4c3c8357eb23782096749546f01e1becf7"
        },
        "date": 1791440106130,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "startup (jit)",
            "value": 12169207,
            "unit": "instructions"
          },
          {
            "name": "startup (interp)",
            "value": 12022160,
            "unit": "instructions"
          },
          {
            "name": "fib (jit)",
            "value": 936053343,
            "unit": "instructions"
          },
          {
            "name": "fib (interp)",
            "value": 1693816853,
            "unit": "instructions"
          },
          {
            "name": "tak (jit)",
            "value": 1172992008,
            "unit": "instructions"
          },
          {
            "name": "tak (interp)",
            "value": 2129858520,
            "unit": "instructions"
          },
          {
            "name": "nqueens (jit)",
            "value": 1246136647,
            "unit": "instructions"
          },
          {
            "name": "nqueens (interp)",
            "value": 2817383968,
            "unit": "instructions"
          },
          {
            "name": "nqueens GC pause",
            "value": 345,
            "unit": "words"
          },
          {
            "name": "bintrees (jit)",
            "value": 4326821446,
            "unit": "instructions"
          },
          {
            "name": "bintrees (interp)",
            "value": 8942264128,
            "unit": "instructions"
          },
          {
            "name": "bintrees GC pause",
            "value": 393138,
            "unit": "words"
          },
          {
            "name": "hof (jit)",
            "value": 1870281242,
            "unit": "instructions"
          },
          {
            "name": "hof (interp)",
            "value": 3286328268,
            "unit": "instructions"
          },
          {
            "name": "hof GC pause",
            "value": 1579648,
            "unit": "words"
          },
          {
            "name": "qsort (jit)",
            "value": 1519511761,
            "unit": "instructions"
          },
          {
            "name": "qsort (interp)",
            "value": 3753482897,
            "unit": "instructions"
          },
          {
            "name": "mandel (jit)",
            "value": 1025685822,
            "unit": "instructions"
          },
          {
            "name": "mandel (interp)",
            "value": 2573679699,
            "unit": "instructions"
          },
          {
            "name": "hash (jit)",
            "value": 765402883,
            "unit": "instructions"
          },
          {
            "name": "hash (interp)",
            "value": 841971802,
            "unit": "instructions"
          },
          {
            "name": "hash GC pause",
            "value": 229391,
            "unit": "words"
          },
          {
            "name": "orgparse (jit)",
            "value": 1134696124,
            "unit": "instructions"
          },
          {
            "name": "orgparse (interp)",
            "value": 1406011757,
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
          "id": "b4f2b3260e540d7c8c7ae70f4e11690108bfe869",
          "message": "Merge pull request #42 from ArthurHeymans/srfi-69-hash-tables\n\nHash tables with any equivalence, as SRFI 69 specifies",
          "timestamp": "2026-10-08T10:05:27+02:00",
          "tree_id": "a99f0fa37450b4b4bcf8be630ed0e576ce4ad6a5",
          "url": "https://github.com/ArthurHeymans/techne/commit/b4f2b3260e540d7c8c7ae70f4e11690108bfe869"
        },
        "date": 1791447023076,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "startup (jit)",
            "value": 12475388,
            "unit": "instructions"
          },
          {
            "name": "startup (interp)",
            "value": 12321065,
            "unit": "instructions"
          },
          {
            "name": "fib (jit)",
            "value": 936361964,
            "unit": "instructions"
          },
          {
            "name": "fib (interp)",
            "value": 1694116448,
            "unit": "instructions"
          },
          {
            "name": "tak (jit)",
            "value": 1173301007,
            "unit": "instructions"
          },
          {
            "name": "tak (interp)",
            "value": 2130155266,
            "unit": "instructions"
          },
          {
            "name": "nqueens (jit)",
            "value": 1246446511,
            "unit": "instructions"
          },
          {
            "name": "nqueens (interp)",
            "value": 2817704906,
            "unit": "instructions"
          },
          {
            "name": "nqueens GC pause",
            "value": 345,
            "unit": "words"
          },
          {
            "name": "bintrees (jit)",
            "value": 4327143700,
            "unit": "instructions"
          },
          {
            "name": "bintrees (interp)",
            "value": 8942593211,
            "unit": "instructions"
          },
          {
            "name": "bintrees GC pause",
            "value": 393138,
            "unit": "words"
          },
          {
            "name": "hof (jit)",
            "value": 1870394346,
            "unit": "instructions"
          },
          {
            "name": "hof (interp)",
            "value": 3286479298,
            "unit": "instructions"
          },
          {
            "name": "hof GC pause",
            "value": 1579648,
            "unit": "words"
          },
          {
            "name": "qsort (jit)",
            "value": 1519820796,
            "unit": "instructions"
          },
          {
            "name": "qsort (interp)",
            "value": 3753787126,
            "unit": "instructions"
          },
          {
            "name": "mandel (jit)",
            "value": 1025991056,
            "unit": "instructions"
          },
          {
            "name": "mandel (interp)",
            "value": 2573982521,
            "unit": "instructions"
          },
          {
            "name": "hash (jit)",
            "value": 738713527,
            "unit": "instructions"
          },
          {
            "name": "hash (interp)",
            "value": 815259204,
            "unit": "instructions"
          },
          {
            "name": "hash GC pause",
            "value": 229395,
            "unit": "words"
          },
          {
            "name": "orgparse (jit)",
            "value": 1134753462,
            "unit": "instructions"
          },
          {
            "name": "orgparse (interp)",
            "value": 1406057252,
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
          "id": "be2c074c66ea81ae37b10992212ac1a6c4059bdf",
          "message": "Merge pull request #43 from ArthurHeymans/srfi-27-random\n\nRandom numbers (SRFI 27), and SRFI 132 sorting checked",
          "timestamp": "2026-10-08T10:29:44+02:00",
          "tree_id": "bf72911842e0cac75504f159031c39ffcda87c82",
          "url": "https://github.com/ArthurHeymans/techne/commit/be2c074c66ea81ae37b10992212ac1a6c4059bdf"
        },
        "date": 1791448451890,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "startup (jit)",
            "value": 12470663,
            "unit": "instructions"
          },
          {
            "name": "startup (interp)",
            "value": 12322152,
            "unit": "instructions"
          },
          {
            "name": "fib (jit)",
            "value": 936349871,
            "unit": "instructions"
          },
          {
            "name": "fib (interp)",
            "value": 1694117554,
            "unit": "instructions"
          },
          {
            "name": "tak (jit)",
            "value": 1173291705,
            "unit": "instructions"
          },
          {
            "name": "tak (interp)",
            "value": 2130160839,
            "unit": "instructions"
          },
          {
            "name": "nqueens (jit)",
            "value": 1246443334,
            "unit": "instructions"
          },
          {
            "name": "nqueens (interp)",
            "value": 2817706106,
            "unit": "instructions"
          },
          {
            "name": "nqueens GC pause",
            "value": 345,
            "unit": "words"
          },
          {
            "name": "bintrees (jit)",
            "value": 4327086288,
            "unit": "instructions"
          },
          {
            "name": "bintrees (interp)",
            "value": 8942594375,
            "unit": "instructions"
          },
          {
            "name": "bintrees GC pause",
            "value": 393138,
            "unit": "words"
          },
          {
            "name": "hof (jit)",
            "value": 1870380811,
            "unit": "instructions"
          },
          {
            "name": "hof (interp)",
            "value": 3286480567,
            "unit": "instructions"
          },
          {
            "name": "hof GC pause",
            "value": 1579648,
            "unit": "words"
          },
          {
            "name": "qsort (jit)",
            "value": 1519796500,
            "unit": "instructions"
          },
          {
            "name": "qsort (interp)",
            "value": 3753788288,
            "unit": "instructions"
          },
          {
            "name": "mandel (jit)",
            "value": 1025951661,
            "unit": "instructions"
          },
          {
            "name": "mandel (interp)",
            "value": 2573979263,
            "unit": "instructions"
          },
          {
            "name": "hash (jit)",
            "value": 738675105,
            "unit": "instructions"
          },
          {
            "name": "hash (interp)",
            "value": 815264788,
            "unit": "instructions"
          },
          {
            "name": "hash GC pause",
            "value": 229395,
            "unit": "words"
          },
          {
            "name": "orgparse (jit)",
            "value": 1134637128,
            "unit": "instructions"
          },
          {
            "name": "orgparse (interp)",
            "value": 1406058517,
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
          "id": "28b64e3914d5d2deab4a2bfebec110c56f6ef8f5",
          "message": "Merge pull request #45 from ArthurHeymans/srfi-130-cursors\n\nString cursors: walk non-ASCII text in linear time (SRFI 130)",
          "timestamp": "2026-10-08T10:56:45+02:00",
          "tree_id": "718ebe2d710faeaa95c45a09b72a4365d0188915",
          "url": "https://github.com/ArthurHeymans/techne/commit/28b64e3914d5d2deab4a2bfebec110c56f6ef8f5"
        },
        "date": 1791450060446,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "startup (jit)",
            "value": 12546819,
            "unit": "instructions"
          },
          {
            "name": "startup (interp)",
            "value": 12381059,
            "unit": "instructions"
          },
          {
            "name": "fib (jit)",
            "value": 936414497,
            "unit": "instructions"
          },
          {
            "name": "fib (interp)",
            "value": 1694176471,
            "unit": "instructions"
          },
          {
            "name": "tak (jit)",
            "value": 1173356773,
            "unit": "instructions"
          },
          {
            "name": "tak (interp)",
            "value": 2130215549,
            "unit": "instructions"
          },
          {
            "name": "nqueens (jit)",
            "value": 1245806424,
            "unit": "instructions"
          },
          {
            "name": "nqueens (interp)",
            "value": 2818428459,
            "unit": "instructions"
          },
          {
            "name": "nqueens GC pause",
            "value": 345,
            "unit": "words"
          },
          {
            "name": "bintrees (jit)",
            "value": 4327165913,
            "unit": "instructions"
          },
          {
            "name": "bintrees (interp)",
            "value": 8942671913,
            "unit": "instructions"
          },
          {
            "name": "bintrees GC pause",
            "value": 393138,
            "unit": "words"
          },
          {
            "name": "hof (jit)",
            "value": 1870491515,
            "unit": "instructions"
          },
          {
            "name": "hof (interp)",
            "value": 3286514623,
            "unit": "instructions"
          },
          {
            "name": "hof GC pause",
            "value": 1579648,
            "unit": "words"
          },
          {
            "name": "qsort (jit)",
            "value": 1519884758,
            "unit": "instructions"
          },
          {
            "name": "qsort (interp)",
            "value": 3753842796,
            "unit": "instructions"
          },
          {
            "name": "mandel (jit)",
            "value": 1026011094,
            "unit": "instructions"
          },
          {
            "name": "mandel (interp)",
            "value": 2574038586,
            "unit": "instructions"
          },
          {
            "name": "hash (jit)",
            "value": 738737504,
            "unit": "instructions"
          },
          {
            "name": "hash (interp)",
            "value": 815320218,
            "unit": "instructions"
          },
          {
            "name": "hash GC pause",
            "value": 229395,
            "unit": "words"
          },
          {
            "name": "orgparse (jit)",
            "value": 1134893933,
            "unit": "instructions"
          },
          {
            "name": "orgparse (interp)",
            "value": 1406258098,
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
          "id": "a758f05295161175d140b1d3a5a878e028f74a15",
          "message": "Merge pull request #52 from ArthurHeymans/reliability-file-aliases\n\nShare file documents across aliases without losing recovery journals",
          "timestamp": "2026-10-08T12:45:07+02:00",
          "tree_id": "8605180aaa4b0af86f8b2a05768734188c585837",
          "url": "https://github.com/ArthurHeymans/techne/commit/a758f05295161175d140b1d3a5a878e028f74a15"
        },
        "date": 1791456588365,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "startup (jit)",
            "value": 12773333,
            "unit": "instructions"
          },
          {
            "name": "startup (interp)",
            "value": 12603833,
            "unit": "instructions"
          },
          {
            "name": "fib (jit)",
            "value": 936642948,
            "unit": "instructions"
          },
          {
            "name": "fib (interp)",
            "value": 1694398176,
            "unit": "instructions"
          },
          {
            "name": "tak (jit)",
            "value": 1173590742,
            "unit": "instructions"
          },
          {
            "name": "tak (interp)",
            "value": 2130440717,
            "unit": "instructions"
          },
          {
            "name": "nqueens (jit)",
            "value": 1246079897,
            "unit": "instructions"
          },
          {
            "name": "nqueens (interp)",
            "value": 2818660481,
            "unit": "instructions"
          },
          {
            "name": "nqueens GC pause",
            "value": 345,
            "unit": "words"
          },
          {
            "name": "bintrees (jit)",
            "value": 4327426398,
            "unit": "instructions"
          },
          {
            "name": "bintrees (interp)",
            "value": 8942897014,
            "unit": "instructions"
          },
          {
            "name": "bintrees GC pause",
            "value": 393138,
            "unit": "words"
          },
          {
            "name": "hof (jit)",
            "value": 1870739364,
            "unit": "instructions"
          },
          {
            "name": "hof (interp)",
            "value": 3286730922,
            "unit": "instructions"
          },
          {
            "name": "hof GC pause",
            "value": 1579648,
            "unit": "words"
          },
          {
            "name": "qsort (jit)",
            "value": 1520101020,
            "unit": "instructions"
          },
          {
            "name": "qsort (interp)",
            "value": 3754064264,
            "unit": "instructions"
          },
          {
            "name": "mandel (jit)",
            "value": 1026284622,
            "unit": "instructions"
          },
          {
            "name": "mandel (interp)",
            "value": 2574261383,
            "unit": "instructions"
          },
          {
            "name": "hash (jit)",
            "value": 741798418,
            "unit": "instructions"
          },
          {
            "name": "hash (interp)",
            "value": 818342210,
            "unit": "instructions"
          },
          {
            "name": "hash GC pause",
            "value": 229395,
            "unit": "words"
          },
          {
            "name": "orgparse (jit)",
            "value": 1135149028,
            "unit": "instructions"
          },
          {
            "name": "orgparse (interp)",
            "value": 1406464044,
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
          "id": "d7c95fb922bd021a2add3ff6a0c6c8dacba594ec",
          "message": "Merge pull request #54 from ArthurHeymans/editor-review-fixes\n\nFix editor and SRFI bugs found in review: stuck evaluations, stale edits, private journals",
          "timestamp": "2026-10-08T17:33:33+02:00",
          "tree_id": "d3ad6a8ff2d9829ccf550d5638097c6cd407f71b",
          "url": "https://github.com/ArthurHeymans/techne/commit/d7c95fb922bd021a2add3ff6a0c6c8dacba594ec"
        },
        "date": 1791473892719,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "startup (jit)",
            "value": 12909146,
            "unit": "instructions"
          },
          {
            "name": "startup (interp)",
            "value": 12730508,
            "unit": "instructions"
          },
          {
            "name": "fib (jit)",
            "value": 936773311,
            "unit": "instructions"
          },
          {
            "name": "fib (interp)",
            "value": 1694524780,
            "unit": "instructions"
          },
          {
            "name": "tak (jit)",
            "value": 1173773992,
            "unit": "instructions"
          },
          {
            "name": "tak (interp)",
            "value": 2130626583,
            "unit": "instructions"
          },
          {
            "name": "nqueens (jit)",
            "value": 1246271464,
            "unit": "instructions"
          },
          {
            "name": "nqueens (interp)",
            "value": 2818241191,
            "unit": "instructions"
          },
          {
            "name": "nqueens GC pause",
            "value": 345,
            "unit": "words"
          },
          {
            "name": "bintrees (jit)",
            "value": 4327601854,
            "unit": "instructions"
          },
          {
            "name": "bintrees (interp)",
            "value": 8943090479,
            "unit": "instructions"
          },
          {
            "name": "bintrees GC pause",
            "value": 393138,
            "unit": "words"
          },
          {
            "name": "hof (jit)",
            "value": 1871161489,
            "unit": "instructions"
          },
          {
            "name": "hof (interp)",
            "value": 3287213171,
            "unit": "instructions"
          },
          {
            "name": "hof GC pause",
            "value": 1579648,
            "unit": "words"
          },
          {
            "name": "qsort (jit)",
            "value": 1520300632,
            "unit": "instructions"
          },
          {
            "name": "qsort (interp)",
            "value": 3754249589,
            "unit": "instructions"
          },
          {
            "name": "mandel (jit)",
            "value": 1026441009,
            "unit": "instructions"
          },
          {
            "name": "mandel (interp)",
            "value": 2574447920,
            "unit": "instructions"
          },
          {
            "name": "hash (jit)",
            "value": 741900929,
            "unit": "instructions"
          },
          {
            "name": "hash (interp)",
            "value": 818469428,
            "unit": "instructions"
          },
          {
            "name": "hash GC pause",
            "value": 229395,
            "unit": "words"
          },
          {
            "name": "orgparse (jit)",
            "value": 1135226639,
            "unit": "instructions"
          },
          {
            "name": "orgparse (interp)",
            "value": 1406591290,
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
          "id": "33aa32015ee78e4a4b110484b7b21054282da194",
          "message": "Merge pull request #55 from ArthurHeymans/vm-safety-fixes\n\ntechne-vm: fix input handling and resource lifetime bugs",
          "timestamp": "2026-10-08T21:38:04+02:00",
          "tree_id": "05e77e597a7a57f8b243af0f070d7f9ce955ad93",
          "url": "https://github.com/ArthurHeymans/techne/commit/33aa32015ee78e4a4b110484b7b21054282da194"
        },
        "date": 1791488485496,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "startup (jit)",
            "value": 13809921,
            "unit": "instructions"
          },
          {
            "name": "startup (interp)",
            "value": 13638110,
            "unit": "instructions"
          },
          {
            "name": "fib (jit)",
            "value": 937681327,
            "unit": "instructions"
          },
          {
            "name": "fib (interp)",
            "value": 1695437508,
            "unit": "instructions"
          },
          {
            "name": "tak (jit)",
            "value": 1174690790,
            "unit": "instructions"
          },
          {
            "name": "tak (interp)",
            "value": 2131541412,
            "unit": "instructions"
          },
          {
            "name": "nqueens (jit)",
            "value": 1247158390,
            "unit": "instructions"
          },
          {
            "name": "nqueens (interp)",
            "value": 2819091168,
            "unit": "instructions"
          },
          {
            "name": "nqueens GC pause",
            "value": 345,
            "unit": "words"
          },
          {
            "name": "bintrees (jit)",
            "value": 4328544767,
            "unit": "instructions"
          },
          {
            "name": "bintrees (interp)",
            "value": 8944013628,
            "unit": "instructions"
          },
          {
            "name": "bintrees GC pause",
            "value": 393138,
            "unit": "words"
          },
          {
            "name": "hof (jit)",
            "value": 1872117829,
            "unit": "instructions"
          },
          {
            "name": "hof (interp)",
            "value": 3288131271,
            "unit": "instructions"
          },
          {
            "name": "hof GC pause",
            "value": 1579648,
            "unit": "words"
          },
          {
            "name": "qsort (jit)",
            "value": 1521263128,
            "unit": "instructions"
          },
          {
            "name": "qsort (interp)",
            "value": 3755190235,
            "unit": "instructions"
          },
          {
            "name": "mandel (jit)",
            "value": 1027406460,
            "unit": "instructions"
          },
          {
            "name": "mandel (interp)",
            "value": 2575377280,
            "unit": "instructions"
          },
          {
            "name": "hash (jit)",
            "value": 742858421,
            "unit": "instructions"
          },
          {
            "name": "hash (interp)",
            "value": 819395283,
            "unit": "instructions"
          },
          {
            "name": "hash GC pause",
            "value": 229395,
            "unit": "words"
          },
          {
            "name": "orgparse (jit)",
            "value": 1136202140,
            "unit": "instructions"
          },
          {
            "name": "orgparse (interp)",
            "value": 1407547432,
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
          "id": "31b5020130b4d2adb29eaf437b988a85c85b1860",
          "message": "Merge pull request #56 from ArthurHeymans/runtime-fast-paths\n\ntechne-vm: faster arithmetic on large integers and long lists",
          "timestamp": "2026-10-08T21:58:49+02:00",
          "tree_id": "1b48ded2151663fffb01be8cff14107e7f579f1f",
          "url": "https://github.com/ArthurHeymans/techne/commit/31b5020130b4d2adb29eaf437b988a85c85b1860"
        },
        "date": 1791489768012,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "startup (jit)",
            "value": 13790043,
            "unit": "instructions"
          },
          {
            "name": "startup (interp)",
            "value": 13638485,
            "unit": "instructions"
          },
          {
            "name": "fib (jit)",
            "value": 937680882,
            "unit": "instructions"
          },
          {
            "name": "fib (interp)",
            "value": 1695437883,
            "unit": "instructions"
          },
          {
            "name": "tak (jit)",
            "value": 1174695200,
            "unit": "instructions"
          },
          {
            "name": "tak (interp)",
            "value": 2131541787,
            "unit": "instructions"
          },
          {
            "name": "nqueens (jit)",
            "value": 1248025831,
            "unit": "instructions"
          },
          {
            "name": "nqueens (interp)",
            "value": 2819954355,
            "unit": "instructions"
          },
          {
            "name": "nqueens GC pause",
            "value": 345,
            "unit": "words"
          },
          {
            "name": "bintrees (jit)",
            "value": 4328518959,
            "unit": "instructions"
          },
          {
            "name": "bintrees (interp)",
            "value": 8944014003,
            "unit": "instructions"
          },
          {
            "name": "bintrees GC pause",
            "value": 393138,
            "unit": "words"
          },
          {
            "name": "hof (jit)",
            "value": 1656723340,
            "unit": "instructions"
          },
          {
            "name": "hof (interp)",
            "value": 3072695284,
            "unit": "instructions"
          },
          {
            "name": "hof GC pause",
            "value": 1579648,
            "unit": "words"
          },
          {
            "name": "qsort (jit)",
            "value": 1521214746,
            "unit": "instructions"
          },
          {
            "name": "qsort (interp)",
            "value": 3755186162,
            "unit": "instructions"
          },
          {
            "name": "mandel (jit)",
            "value": 1028624823,
            "unit": "instructions"
          },
          {
            "name": "mandel (interp)",
            "value": 2576627655,
            "unit": "instructions"
          },
          {
            "name": "hash (jit)",
            "value": 742820883,
            "unit": "instructions"
          },
          {
            "name": "hash (interp)",
            "value": 819391210,
            "unit": "instructions"
          },
          {
            "name": "hash GC pause",
            "value": 229395,
            "unit": "words"
          },
          {
            "name": "orgparse (jit)",
            "value": 1136196990,
            "unit": "instructions"
          },
          {
            "name": "orgparse (interp)",
            "value": 1407547805,
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
          "id": "5e576e3e56d38a4bf88ec6af0287bf637347c6eb",
          "message": "Merge pull request #57 from ArthurHeymans/jit-fast-calls\n\ntechne-vm: faster calls and fixnum arithmetic in compiled code",
          "timestamp": "2026-10-08T22:08:12+02:00",
          "tree_id": "d40fc229c49b8fb408b59698753903555a0a22c7",
          "url": "https://github.com/ArthurHeymans/techne/commit/5e576e3e56d38a4bf88ec6af0287bf637347c6eb"
        },
        "date": 1791490258934,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "startup (jit)",
            "value": 13795551,
            "unit": "instructions"
          },
          {
            "name": "startup (interp)",
            "value": 13634919,
            "unit": "instructions"
          },
          {
            "name": "fib (jit)",
            "value": 856429341,
            "unit": "instructions"
          },
          {
            "name": "fib (interp)",
            "value": 1695434331,
            "unit": "instructions"
          },
          {
            "name": "tak (jit)",
            "value": 1083969166,
            "unit": "instructions"
          },
          {
            "name": "tak (interp)",
            "value": 2131542703,
            "unit": "instructions"
          },
          {
            "name": "nqueens (jit)",
            "value": 1198899474,
            "unit": "instructions"
          },
          {
            "name": "nqueens (interp)",
            "value": 2819955240,
            "unit": "instructions"
          },
          {
            "name": "nqueens GC pause",
            "value": 345,
            "unit": "words"
          },
          {
            "name": "bintrees (jit)",
            "value": 3923502126,
            "unit": "instructions"
          },
          {
            "name": "bintrees (interp)",
            "value": 8944014912,
            "unit": "instructions"
          },
          {
            "name": "bintrees GC pause",
            "value": 393138,
            "unit": "words"
          },
          {
            "name": "hof (jit)",
            "value": 1605985019,
            "unit": "instructions"
          },
          {
            "name": "hof (interp)",
            "value": 3072696181,
            "unit": "instructions"
          },
          {
            "name": "hof GC pause",
            "value": 1579648,
            "unit": "words"
          },
          {
            "name": "qsort (jit)",
            "value": 1462505052,
            "unit": "instructions"
          },
          {
            "name": "qsort (interp)",
            "value": 3755187103,
            "unit": "instructions"
          },
          {
            "name": "mandel (jit)",
            "value": 1033523686,
            "unit": "instructions"
          },
          {
            "name": "mandel (interp)",
            "value": 2576624206,
            "unit": "instructions"
          },
          {
            "name": "hash (jit)",
            "value": 739282231,
            "unit": "instructions"
          },
          {
            "name": "hash (interp)",
            "value": 819392123,
            "unit": "instructions"
          },
          {
            "name": "hash GC pause",
            "value": 229395,
            "unit": "words"
          },
          {
            "name": "orgparse (jit)",
            "value": 1125874904,
            "unit": "instructions"
          },
          {
            "name": "orgparse (interp)",
            "value": 1407548758,
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
          "id": "8a35c7169527244310df6366dd6fed749fbafdd3",
          "message": "Merge pull request #58 from ArthurHeymans/fuzz-exact-ints\n\nCheck integer arithmetic exactly; comment every PR's benchmarks",
          "timestamp": "2026-10-08T22:25:53+02:00",
          "tree_id": "c04dbd8c513a1396e169d5527fb7ad30797b4c40",
          "url": "https://github.com/ArthurHeymans/techne/commit/8a35c7169527244310df6366dd6fed749fbafdd3"
        },
        "date": 1791491307819,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "startup (jit)",
            "value": 13791103,
            "unit": "instructions"
          },
          {
            "name": "startup (interp)",
            "value": 13634919,
            "unit": "instructions"
          },
          {
            "name": "fib (jit)",
            "value": 856430613,
            "unit": "instructions"
          },
          {
            "name": "fib (interp)",
            "value": 1695438779,
            "unit": "instructions"
          },
          {
            "name": "tak (jit)",
            "value": 1083968976,
            "unit": "instructions"
          },
          {
            "name": "tak (interp)",
            "value": 2131542703,
            "unit": "instructions"
          },
          {
            "name": "nqueens (jit)",
            "value": 1198899747,
            "unit": "instructions"
          },
          {
            "name": "nqueens (interp)",
            "value": 2819950792,
            "unit": "instructions"
          },
          {
            "name": "nqueens GC pause",
            "value": 345,
            "unit": "words"
          },
          {
            "name": "bintrees (jit)",
            "value": 3923468650,
            "unit": "instructions"
          },
          {
            "name": "bintrees (interp)",
            "value": 8944014912,
            "unit": "instructions"
          },
          {
            "name": "bintrees GC pause",
            "value": 393138,
            "unit": "words"
          },
          {
            "name": "hof (jit)",
            "value": 1605983276,
            "unit": "instructions"
          },
          {
            "name": "hof (interp)",
            "value": 3072696181,
            "unit": "instructions"
          },
          {
            "name": "hof GC pause",
            "value": 1579648,
            "unit": "words"
          },
          {
            "name": "qsort (jit)",
            "value": 1462445687,
            "unit": "instructions"
          },
          {
            "name": "qsort (interp)",
            "value": 3755191551,
            "unit": "instructions"
          },
          {
            "name": "mandel (jit)",
            "value": 1033549564,
            "unit": "instructions"
          },
          {
            "name": "mandel (interp)",
            "value": 2576628654,
            "unit": "instructions"
          },
          {
            "name": "hash (jit)",
            "value": 739298985,
            "unit": "instructions"
          },
          {
            "name": "hash (interp)",
            "value": 819392123,
            "unit": "instructions"
          },
          {
            "name": "hash GC pause",
            "value": 229395,
            "unit": "words"
          },
          {
            "name": "orgparse (jit)",
            "value": 1125961856,
            "unit": "instructions"
          },
          {
            "name": "orgparse (interp)",
            "value": 1407548758,
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
          "id": "326e7f54cdf5bbeec4c30ef6b8dbf8a2023b3c9f",
          "message": "Merge pull request #59 from ArthurHeymans/jit-call-identity\n\ntechne-vm: cheaper checks on compiled calls to global functions",
          "timestamp": "2026-10-08T22:37:38+02:00",
          "tree_id": "5d3ef22641911eb1ddbff5517e19073dc7be0881",
          "url": "https://github.com/ArthurHeymans/techne/commit/326e7f54cdf5bbeec4c30ef6b8dbf8a2023b3c9f"
        },
        "date": 1791492007644,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "startup (jit)",
            "value": 13801022,
            "unit": "instructions"
          },
          {
            "name": "startup (interp)",
            "value": 13644744,
            "unit": "instructions"
          },
          {
            "name": "fib (jit)",
            "value": 778543547,
            "unit": "instructions"
          },
          {
            "name": "fib (interp)",
            "value": 1695444147,
            "unit": "instructions"
          },
          {
            "name": "tak (jit)",
            "value": 985849235,
            "unit": "instructions"
          },
          {
            "name": "tak (interp)",
            "value": 2131546996,
            "unit": "instructions"
          },
          {
            "name": "nqueens (jit)",
            "value": 1139807668,
            "unit": "instructions"
          },
          {
            "name": "nqueens (interp)",
            "value": 2819937577,
            "unit": "instructions"
          },
          {
            "name": "nqueens GC pause",
            "value": 345,
            "unit": "words"
          },
          {
            "name": "bintrees (jit)",
            "value": 3629292018,
            "unit": "instructions"
          },
          {
            "name": "bintrees (interp)",
            "value": 8944075788,
            "unit": "instructions"
          },
          {
            "name": "bintrees GC pause",
            "value": 393138,
            "unit": "words"
          },
          {
            "name": "hof (jit)",
            "value": 1605882325,
            "unit": "instructions"
          },
          {
            "name": "hof (interp)",
            "value": 3072732629,
            "unit": "instructions"
          },
          {
            "name": "hof GC pause",
            "value": 1579648,
            "unit": "words"
          },
          {
            "name": "qsort (jit)",
            "value": 1424572082,
            "unit": "instructions"
          },
          {
            "name": "qsort (interp)",
            "value": 3755196736,
            "unit": "instructions"
          },
          {
            "name": "mandel (jit)",
            "value": 1032735978,
            "unit": "instructions"
          },
          {
            "name": "mandel (interp)",
            "value": 2576639249,
            "unit": "instructions"
          },
          {
            "name": "hash (jit)",
            "value": 736271528,
            "unit": "instructions"
          },
          {
            "name": "hash (interp)",
            "value": 819401878,
            "unit": "instructions"
          },
          {
            "name": "hash GC pause",
            "value": 229395,
            "unit": "words"
          },
          {
            "name": "orgparse (jit)",
            "value": 1121837020,
            "unit": "instructions"
          },
          {
            "name": "orgparse (interp)",
            "value": 1407557745,
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
          "id": "ed81a7efb53a18592360ce4308f68df0cb422807",
          "message": "Merge pull request #60 from ArthurHeymans/bench-callbacks\n\nbench: measure lambdas passed to higher-order procedures",
          "timestamp": "2026-10-08T22:48:47+02:00",
          "tree_id": "aae90d039ff21f8c18c4aa0091dc3c55f3e12085",
          "url": "https://github.com/ArthurHeymans/techne/commit/ed81a7efb53a18592360ce4308f68df0cb422807"
        },
        "date": 1791492705926,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "startup (jit)",
            "value": 13820599,
            "unit": "instructions"
          },
          {
            "name": "startup (interp)",
            "value": 13644744,
            "unit": "instructions"
          },
          {
            "name": "fib (jit)",
            "value": 778541716,
            "unit": "instructions"
          },
          {
            "name": "fib (interp)",
            "value": 1695448595,
            "unit": "instructions"
          },
          {
            "name": "tak (jit)",
            "value": 985846161,
            "unit": "instructions"
          },
          {
            "name": "tak (interp)",
            "value": 2131551444,
            "unit": "instructions"
          },
          {
            "name": "nqueens (jit)",
            "value": 1139800638,
            "unit": "instructions"
          },
          {
            "name": "nqueens (interp)",
            "value": 2819937577,
            "unit": "instructions"
          },
          {
            "name": "nqueens GC pause",
            "value": 345,
            "unit": "words"
          },
          {
            "name": "bintrees (jit)",
            "value": 3629288899,
            "unit": "instructions"
          },
          {
            "name": "bintrees (interp)",
            "value": 8944075788,
            "unit": "instructions"
          },
          {
            "name": "bintrees GC pause",
            "value": 393138,
            "unit": "words"
          },
          {
            "name": "hof (jit)",
            "value": 1605940204,
            "unit": "instructions"
          },
          {
            "name": "hof (interp)",
            "value": 3072737077,
            "unit": "instructions"
          },
          {
            "name": "hof GC pause",
            "value": 1579648,
            "unit": "words"
          },
          {
            "name": "qsort (jit)",
            "value": 1424559493,
            "unit": "instructions"
          },
          {
            "name": "qsort (interp)",
            "value": 3755201184,
            "unit": "instructions"
          },
          {
            "name": "mandel (jit)",
            "value": 1032638417,
            "unit": "instructions"
          },
          {
            "name": "mandel (interp)",
            "value": 2576639249,
            "unit": "instructions"
          },
          {
            "name": "hash (jit)",
            "value": 736269973,
            "unit": "instructions"
          },
          {
            "name": "hash (interp)",
            "value": 819401878,
            "unit": "instructions"
          },
          {
            "name": "hash GC pause",
            "value": 229395,
            "unit": "words"
          },
          {
            "name": "orgparse (jit)",
            "value": 1121860635,
            "unit": "instructions"
          },
          {
            "name": "orgparse (interp)",
            "value": 1407557745,
            "unit": "instructions"
          },
          {
            "name": "callbacks (jit)",
            "value": 948975661,
            "unit": "instructions"
          },
          {
            "name": "callbacks (interp)",
            "value": 2166653590,
            "unit": "instructions"
          },
          {
            "name": "callbacks GC pause",
            "value": 327540,
            "unit": "words"
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
          "id": "ee13c2f6e26b9fb737ebc2bd8bc33e762cb3a3e0",
          "message": "Merge pull request #62 from ArthurHeymans/inline-higher-order\n\ntechne-vm: inline lambdas passed to map, filter and co.",
          "timestamp": "2026-10-08T23:26:04+02:00",
          "tree_id": "3d10da1ded3c6225afb904d8fe4cf9c54d70766a",
          "url": "https://github.com/ArthurHeymans/techne/commit/ee13c2f6e26b9fb737ebc2bd8bc33e762cb3a3e0"
        },
        "date": 1791494935777,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "startup (jit)",
            "value": 13960706,
            "unit": "instructions"
          },
          {
            "name": "startup (interp)",
            "value": 13787034,
            "unit": "instructions"
          },
          {
            "name": "fib (jit)",
            "value": 778687575,
            "unit": "instructions"
          },
          {
            "name": "fib (interp)",
            "value": 1695586466,
            "unit": "instructions"
          },
          {
            "name": "tak (jit)",
            "value": 985989656,
            "unit": "instructions"
          },
          {
            "name": "tak (interp)",
            "value": 2131686876,
            "unit": "instructions"
          },
          {
            "name": "nqueens (jit)",
            "value": 1140662604,
            "unit": "instructions"
          },
          {
            "name": "nqueens (interp)",
            "value": 2820776467,
            "unit": "instructions"
          },
          {
            "name": "nqueens GC pause",
            "value": 345,
            "unit": "words"
          },
          {
            "name": "bintrees (jit)",
            "value": 3629455723,
            "unit": "instructions"
          },
          {
            "name": "bintrees (interp)",
            "value": 8944219368,
            "unit": "instructions"
          },
          {
            "name": "bintrees GC pause",
            "value": 393138,
            "unit": "words"
          },
          {
            "name": "hof (jit)",
            "value": 1606140372,
            "unit": "instructions"
          },
          {
            "name": "hof (interp)",
            "value": 3072856451,
            "unit": "instructions"
          },
          {
            "name": "hof GC pause",
            "value": 1579648,
            "unit": "words"
          },
          {
            "name": "qsort (jit)",
            "value": 1424726623,
            "unit": "instructions"
          },
          {
            "name": "qsort (interp)",
            "value": 3755345063,
            "unit": "instructions"
          },
          {
            "name": "mandel (jit)",
            "value": 1032772871,
            "unit": "instructions"
          },
          {
            "name": "mandel (interp)",
            "value": 2576781267,
            "unit": "instructions"
          },
          {
            "name": "hash (jit)",
            "value": 736417557,
            "unit": "instructions"
          },
          {
            "name": "hash (interp)",
            "value": 819545951,
            "unit": "instructions"
          },
          {
            "name": "hash GC pause",
            "value": 229395,
            "unit": "words"
          },
          {
            "name": "orgparse (jit)",
            "value": 1122009024,
            "unit": "instructions"
          },
          {
            "name": "orgparse (interp)",
            "value": 1407709884,
            "unit": "instructions"
          },
          {
            "name": "callbacks (jit)",
            "value": 633270806,
            "unit": "instructions"
          },
          {
            "name": "callbacks (interp)",
            "value": 1542569733,
            "unit": "instructions"
          },
          {
            "name": "callbacks GC pause",
            "value": 327538,
            "unit": "words"
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
          "id": "ae10154af738bf9a7f8dc7395c7e54c1a185dc70",
          "message": "Merge pull request #63 from ArthurHeymans/cheaper-inlining\n\nCheaper inlining, and count the editor starting",
          "timestamp": "2026-10-08T23:51:06+02:00",
          "tree_id": "6deba2287e4c1a786b6d20ef930bfcd71cfb2470",
          "url": "https://github.com/ArthurHeymans/techne/commit/ae10154af738bf9a7f8dc7395c7e54c1a185dc70"
        },
        "date": 1791496519117,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "startup (jit)",
            "value": 13927846,
            "unit": "instructions"
          },
          {
            "name": "startup (interp)",
            "value": 13771932,
            "unit": "instructions"
          },
          {
            "name": "fib (jit)",
            "value": 778672083,
            "unit": "instructions"
          },
          {
            "name": "fib (interp)",
            "value": 1695571354,
            "unit": "instructions"
          },
          {
            "name": "tak (jit)",
            "value": 985978224,
            "unit": "instructions"
          },
          {
            "name": "tak (interp)",
            "value": 2131676671,
            "unit": "instructions"
          },
          {
            "name": "nqueens (jit)",
            "value": 1140650459,
            "unit": "instructions"
          },
          {
            "name": "nqueens (interp)",
            "value": 2820763991,
            "unit": "instructions"
          },
          {
            "name": "nqueens GC pause",
            "value": 345,
            "unit": "words"
          },
          {
            "name": "bintrees (jit)",
            "value": 3629439257,
            "unit": "instructions"
          },
          {
            "name": "bintrees (interp)",
            "value": 8944205918,
            "unit": "instructions"
          },
          {
            "name": "bintrees GC pause",
            "value": 393138,
            "unit": "words"
          },
          {
            "name": "hof (jit)",
            "value": 1606134135,
            "unit": "instructions"
          },
          {
            "name": "hof (interp)",
            "value": 3072834340,
            "unit": "instructions"
          },
          {
            "name": "hof GC pause",
            "value": 1579648,
            "unit": "words"
          },
          {
            "name": "qsort (jit)",
            "value": 1424719740,
            "unit": "instructions"
          },
          {
            "name": "qsort (interp)",
            "value": 3755332393,
            "unit": "instructions"
          },
          {
            "name": "mandel (jit)",
            "value": 1032774259,
            "unit": "instructions"
          },
          {
            "name": "mandel (interp)",
            "value": 2576769492,
            "unit": "instructions"
          },
          {
            "name": "hash (jit)",
            "value": 736407067,
            "unit": "instructions"
          },
          {
            "name": "hash (interp)",
            "value": 819530298,
            "unit": "instructions"
          },
          {
            "name": "hash GC pause",
            "value": 229395,
            "unit": "words"
          },
          {
            "name": "orgparse (jit)",
            "value": 1122061420,
            "unit": "instructions"
          },
          {
            "name": "orgparse (interp)",
            "value": 1407695535,
            "unit": "instructions"
          },
          {
            "name": "callbacks (jit)",
            "value": 632529424,
            "unit": "instructions"
          },
          {
            "name": "callbacks (interp)",
            "value": 1542570452,
            "unit": "instructions"
          },
          {
            "name": "callbacks GC pause",
            "value": 327538,
            "unit": "words"
          },
          {
            "name": "editor startup (jit)",
            "value": 120368713,
            "unit": "instructions"
          },
          {
            "name": "editor startup (interp)",
            "value": 117530452,
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
          "id": "a465833d41d4ef80e5473ac9b1fb4e82df2b3b46",
          "message": "Merge pull request #61 from ArthurHeymans/editor-attachments\n\nServe several frontends from one runtime; open files in it as emacsclient does",
          "timestamp": "2026-10-09T00:32:59+02:00",
          "tree_id": "e1adf244404ab981edba436246d72e8c97beb779",
          "url": "https://github.com/ArthurHeymans/techne/commit/a465833d41d4ef80e5473ac9b1fb4e82df2b3b46"
        },
        "date": 1791498987365,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "startup (jit)",
            "value": 13947396,
            "unit": "instructions"
          },
          {
            "name": "startup (interp)",
            "value": 13776380,
            "unit": "instructions"
          },
          {
            "name": "fib (jit)",
            "value": 778674082,
            "unit": "instructions"
          },
          {
            "name": "fib (interp)",
            "value": 1695571354,
            "unit": "instructions"
          },
          {
            "name": "tak (jit)",
            "value": 985979963,
            "unit": "instructions"
          },
          {
            "name": "tak (interp)",
            "value": 2131680234,
            "unit": "instructions"
          },
          {
            "name": "nqueens (jit)",
            "value": 1140656189,
            "unit": "instructions"
          },
          {
            "name": "nqueens (interp)",
            "value": 2820763991,
            "unit": "instructions"
          },
          {
            "name": "nqueens GC pause",
            "value": 345,
            "unit": "words"
          },
          {
            "name": "bintrees (jit)",
            "value": 3629440621,
            "unit": "instructions"
          },
          {
            "name": "bintrees (interp)",
            "value": 8944205932,
            "unit": "instructions"
          },
          {
            "name": "bintrees GC pause",
            "value": 393138,
            "unit": "words"
          },
          {
            "name": "hof (jit)",
            "value": 1606119300,
            "unit": "instructions"
          },
          {
            "name": "hof (interp)",
            "value": 3072838788,
            "unit": "instructions"
          },
          {
            "name": "hof GC pause",
            "value": 1579648,
            "unit": "words"
          },
          {
            "name": "qsort (jit)",
            "value": 1424725058,
            "unit": "instructions"
          },
          {
            "name": "qsort (interp)",
            "value": 3755332339,
            "unit": "instructions"
          },
          {
            "name": "mandel (jit)",
            "value": 1032839911,
            "unit": "instructions"
          },
          {
            "name": "mandel (interp)",
            "value": 2576769478,
            "unit": "instructions"
          },
          {
            "name": "hash (jit)",
            "value": 736404386,
            "unit": "instructions"
          },
          {
            "name": "hash (interp)",
            "value": 819534760,
            "unit": "instructions"
          },
          {
            "name": "hash GC pause",
            "value": 229395,
            "unit": "words"
          },
          {
            "name": "orgparse (jit)",
            "value": 1121990877,
            "unit": "instructions"
          },
          {
            "name": "orgparse (interp)",
            "value": 1407695549,
            "unit": "instructions"
          },
          {
            "name": "callbacks (jit)",
            "value": 632530306,
            "unit": "instructions"
          },
          {
            "name": "callbacks (interp)",
            "value": 1542574861,
            "unit": "instructions"
          },
          {
            "name": "callbacks GC pause",
            "value": 371278,
            "unit": "words"
          },
          {
            "name": "editor startup (jit)",
            "value": 123621683,
            "unit": "instructions"
          },
          {
            "name": "editor startup (interp)",
            "value": 120724460,
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
          "id": "ea9759ba6cbbe683b0c9ca2bbc93919a80303b70",
          "message": "Merge pull request #66 from ArthurHeymans/reclaim-code\n\nFree code, source text and package generations nothing uses any more",
          "timestamp": "2026-10-09T07:30:33+02:00",
          "tree_id": "026c06d0ed802fc1f72dd5cc47bcb2c082b02645",
          "url": "https://github.com/ArthurHeymans/techne/commit/ea9759ba6cbbe683b0c9ca2bbc93919a80303b70"
        },
        "date": 1791524074440,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "startup (jit)",
            "value": 14369715,
            "unit": "instructions"
          },
          {
            "name": "startup (interp)",
            "value": 14209530,
            "unit": "instructions"
          },
          {
            "name": "fib (jit)",
            "value": 779103772,
            "unit": "instructions"
          },
          {
            "name": "fib (interp)",
            "value": 1696008277,
            "unit": "instructions"
          },
          {
            "name": "tak (jit)",
            "value": 986347190,
            "unit": "instructions"
          },
          {
            "name": "tak (interp)",
            "value": 2132053452,
            "unit": "instructions"
          },
          {
            "name": "nqueens (jit)",
            "value": 1136059751,
            "unit": "instructions"
          },
          {
            "name": "nqueens (interp)",
            "value": 2816189359,
            "unit": "instructions"
          },
          {
            "name": "nqueens GC pause",
            "value": 345,
            "unit": "words"
          },
          {
            "name": "bintrees (jit)",
            "value": 3629460334,
            "unit": "instructions"
          },
          {
            "name": "bintrees (interp)",
            "value": 8944223577,
            "unit": "instructions"
          },
          {
            "name": "bintrees GC pause",
            "value": 393138,
            "unit": "words"
          },
          {
            "name": "hof (jit)",
            "value": 1605219643,
            "unit": "instructions"
          },
          {
            "name": "hof (interp)",
            "value": 3072021293,
            "unit": "instructions"
          },
          {
            "name": "hof GC pause",
            "value": 1579648,
            "unit": "words"
          },
          {
            "name": "qsort (jit)",
            "value": 1425084992,
            "unit": "instructions"
          },
          {
            "name": "qsort (interp)",
            "value": 3755711860,
            "unit": "instructions"
          },
          {
            "name": "mandel (jit)",
            "value": 1033188229,
            "unit": "instructions"
          },
          {
            "name": "mandel (interp)",
            "value": 2577144436,
            "unit": "instructions"
          },
          {
            "name": "hash (jit)",
            "value": 735923102,
            "unit": "instructions"
          },
          {
            "name": "hash (interp)",
            "value": 819051022,
            "unit": "instructions"
          },
          {
            "name": "hash GC pause",
            "value": 229395,
            "unit": "words"
          },
          {
            "name": "orgparse (jit)",
            "value": 1122546467,
            "unit": "instructions"
          },
          {
            "name": "orgparse (interp)",
            "value": 1408237035,
            "unit": "instructions"
          },
          {
            "name": "callbacks (jit)",
            "value": 632445613,
            "unit": "instructions"
          },
          {
            "name": "callbacks (interp)",
            "value": 1542620615,
            "unit": "instructions"
          },
          {
            "name": "callbacks GC pause",
            "value": 327538,
            "unit": "words"
          },
          {
            "name": "editor startup (jit)",
            "value": 125538464,
            "unit": "instructions"
          },
          {
            "name": "editor startup (interp)",
            "value": 122678388,
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
          "id": "531b03a319d229a77427201e1006b0e1990156c4",
          "message": "Merge pull request #65 from ArthurHeymans/keystroke-budget\n\nMeasure keystrokes against the 4 ms budget, in CI, and plan fuzzing them",
          "timestamp": "2026-10-09T07:31:32+02:00",
          "tree_id": "e2214c89aceb2da2f9d56443479a779f07a3ae81",
          "url": "https://github.com/ArthurHeymans/techne/commit/531b03a319d229a77427201e1006b0e1990156c4"
        },
        "date": 1791524416507,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "startup (jit)",
            "value": 14365611,
            "unit": "instructions"
          },
          {
            "name": "startup (interp)",
            "value": 14205277,
            "unit": "instructions"
          },
          {
            "name": "fib (jit)",
            "value": 779100920,
            "unit": "instructions"
          },
          {
            "name": "fib (interp)",
            "value": 1696008468,
            "unit": "instructions"
          },
          {
            "name": "tak (jit)",
            "value": 986348334,
            "unit": "instructions"
          },
          {
            "name": "tak (interp)",
            "value": 2132053643,
            "unit": "instructions"
          },
          {
            "name": "nqueens (jit)",
            "value": 1136057320,
            "unit": "instructions"
          },
          {
            "name": "nqueens (interp)",
            "value": 2816185106,
            "unit": "instructions"
          },
          {
            "name": "nqueens GC pause",
            "value": 345,
            "unit": "words"
          },
          {
            "name": "bintrees (jit)",
            "value": 3629452693,
            "unit": "instructions"
          },
          {
            "name": "bintrees (interp)",
            "value": 8944223856,
            "unit": "instructions"
          },
          {
            "name": "bintrees GC pause",
            "value": 393138,
            "unit": "words"
          },
          {
            "name": "hof (jit)",
            "value": 1605192102,
            "unit": "instructions"
          },
          {
            "name": "hof (interp)",
            "value": 3072017030,
            "unit": "instructions"
          },
          {
            "name": "hof GC pause",
            "value": 1579648,
            "unit": "words"
          },
          {
            "name": "qsort (jit)",
            "value": 1425085298,
            "unit": "instructions"
          },
          {
            "name": "qsort (interp)",
            "value": 3755712128,
            "unit": "instructions"
          },
          {
            "name": "mandel (jit)",
            "value": 1033163535,
            "unit": "instructions"
          },
          {
            "name": "mandel (interp)",
            "value": 2577140260,
            "unit": "instructions"
          },
          {
            "name": "hash (jit)",
            "value": 735915827,
            "unit": "instructions"
          },
          {
            "name": "hash (interp)",
            "value": 819046851,
            "unit": "instructions"
          },
          {
            "name": "hash GC pause",
            "value": 229395,
            "unit": "words"
          },
          {
            "name": "orgparse (jit)",
            "value": 1122585323,
            "unit": "instructions"
          },
          {
            "name": "orgparse (interp)",
            "value": 1408237312,
            "unit": "instructions"
          },
          {
            "name": "callbacks (jit)",
            "value": 632457469,
            "unit": "instructions"
          },
          {
            "name": "callbacks (interp)",
            "value": 1542625337,
            "unit": "instructions"
          },
          {
            "name": "callbacks GC pause",
            "value": 327538,
            "unit": "words"
          },
          {
            "name": "editor startup (jit)",
            "value": 125538397,
            "unit": "instructions"
          },
          {
            "name": "editor startup (interp)",
            "value": 122678847,
            "unit": "instructions"
          },
          {
            "name": "keys: type (jit)",
            "value": 3074149,
            "unit": "instructions per key"
          },
          {
            "name": "keys: type-long-line (jit)",
            "value": 3164368,
            "unit": "instructions per key"
          },
          {
            "name": "keys: move (jit)",
            "value": 4326820,
            "unit": "instructions per key"
          },
          {
            "name": "keys: move-modal (jit)",
            "value": 3158707,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-first-key (jit)",
            "value": 570646231,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-narrow (jit)",
            "value": 258686370,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-delete (jit)",
            "value": 352764677,
            "unit": "instructions per key"
          },
          {
            "name": "keys: command-first-key (jit)",
            "value": -676830,
            "unit": "instructions per key"
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
          "id": "36905300fde8a5394d11c7220abee27a48d21e67",
          "message": "Merge pull request #68 from ArthurHeymans/journal-lock-fork\n\nFix spurious \"journal is already open\" when a process is spawned meanwhile",
          "timestamp": "2026-10-09T08:46:03+02:00",
          "tree_id": "8970f4ec663e499b3aafd919d81af70b8d420643",
          "url": "https://github.com/ArthurHeymans/techne/commit/36905300fde8a5394d11c7220abee27a48d21e67"
        },
        "date": 1791528846296,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "startup (jit)",
            "value": 14383936,
            "unit": "instructions"
          },
          {
            "name": "startup (interp)",
            "value": 14205277,
            "unit": "instructions"
          },
          {
            "name": "fib (jit)",
            "value": 779102394,
            "unit": "instructions"
          },
          {
            "name": "fib (interp)",
            "value": 1696008468,
            "unit": "instructions"
          },
          {
            "name": "tak (jit)",
            "value": 986348770,
            "unit": "instructions"
          },
          {
            "name": "tak (interp)",
            "value": 2132053643,
            "unit": "instructions"
          },
          {
            "name": "nqueens (jit)",
            "value": 1136055746,
            "unit": "instructions"
          },
          {
            "name": "nqueens (interp)",
            "value": 2816185106,
            "unit": "instructions"
          },
          {
            "name": "nqueens GC pause",
            "value": 345,
            "unit": "words"
          },
          {
            "name": "bintrees (jit)",
            "value": 3629458228,
            "unit": "instructions"
          },
          {
            "name": "bintrees (interp)",
            "value": 8944223856,
            "unit": "instructions"
          },
          {
            "name": "bintrees GC pause",
            "value": 393138,
            "unit": "words"
          },
          {
            "name": "hof (jit)",
            "value": 1605216932,
            "unit": "instructions"
          },
          {
            "name": "hof (interp)",
            "value": 3072017030,
            "unit": "instructions"
          },
          {
            "name": "hof GC pause",
            "value": 1579648,
            "unit": "words"
          },
          {
            "name": "qsort (jit)",
            "value": 1425060984,
            "unit": "instructions"
          },
          {
            "name": "qsort (interp)",
            "value": 3755712128,
            "unit": "instructions"
          },
          {
            "name": "mandel (jit)",
            "value": 1033157701,
            "unit": "instructions"
          },
          {
            "name": "mandel (interp)",
            "value": 2577144708,
            "unit": "instructions"
          },
          {
            "name": "hash (jit)",
            "value": 735913431,
            "unit": "instructions"
          },
          {
            "name": "hash (interp)",
            "value": 819046851,
            "unit": "instructions"
          },
          {
            "name": "hash GC pause",
            "value": 229395,
            "unit": "words"
          },
          {
            "name": "orgparse (jit)",
            "value": 1122586667,
            "unit": "instructions"
          },
          {
            "name": "orgparse (interp)",
            "value": 1408237312,
            "unit": "instructions"
          },
          {
            "name": "callbacks (jit)",
            "value": 632444455,
            "unit": "instructions"
          },
          {
            "name": "callbacks (interp)",
            "value": 1542625337,
            "unit": "instructions"
          },
          {
            "name": "callbacks GC pause",
            "value": 327538,
            "unit": "words"
          },
          {
            "name": "editor startup (jit)",
            "value": 125534814,
            "unit": "instructions"
          },
          {
            "name": "editor startup (interp)",
            "value": 122675110,
            "unit": "instructions"
          },
          {
            "name": "keys: type (jit)",
            "value": 3070929,
            "unit": "instructions per key"
          },
          {
            "name": "keys: type-long-line (jit)",
            "value": 3165282,
            "unit": "instructions per key"
          },
          {
            "name": "keys: move (jit)",
            "value": 4326925,
            "unit": "instructions per key"
          },
          {
            "name": "keys: move-modal (jit)",
            "value": 3154444,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-first-key (jit)",
            "value": 570951268,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-narrow (jit)",
            "value": 258618195,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-delete (jit)",
            "value": 352577301,
            "unit": "instructions per key"
          },
          {
            "name": "keys: command-first-key (jit)",
            "value": -683856,
            "unit": "instructions per key"
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
          "id": "519409a72e6c5ab0a626561c81f6c46abf85fb3c",
          "message": "Merge pull request #67 from ArthurHeymans/native-matching\n\nKeep the line search within the keystroke budget, and hold every key to it in CI",
          "timestamp": "2026-10-09T08:46:12+02:00",
          "tree_id": "d7d5af234329ef52f76a106d245b00d3f6cfbf4b",
          "url": "https://github.com/ArthurHeymans/techne/commit/519409a72e6c5ab0a626561c81f6c46abf85fb3c"
        },
        "date": 1791529257821,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "startup (jit)",
            "value": 14379433,
            "unit": "instructions"
          },
          {
            "name": "startup (interp)",
            "value": 14209725,
            "unit": "instructions"
          },
          {
            "name": "fib (jit)",
            "value": 779102813,
            "unit": "instructions"
          },
          {
            "name": "fib (interp)",
            "value": 1696008468,
            "unit": "instructions"
          },
          {
            "name": "tak (jit)",
            "value": 986348191,
            "unit": "instructions"
          },
          {
            "name": "tak (interp)",
            "value": 2132053643,
            "unit": "instructions"
          },
          {
            "name": "nqueens (jit)",
            "value": 1136054909,
            "unit": "instructions"
          },
          {
            "name": "nqueens (interp)",
            "value": 2816189554,
            "unit": "instructions"
          },
          {
            "name": "nqueens GC pause",
            "value": 345,
            "unit": "words"
          },
          {
            "name": "bintrees (jit)",
            "value": 3629467807,
            "unit": "instructions"
          },
          {
            "name": "bintrees (interp)",
            "value": 8944223856,
            "unit": "instructions"
          },
          {
            "name": "bintrees GC pause",
            "value": 393138,
            "unit": "words"
          },
          {
            "name": "hof (jit)",
            "value": 1605213331,
            "unit": "instructions"
          },
          {
            "name": "hof (interp)",
            "value": 3072021478,
            "unit": "instructions"
          },
          {
            "name": "hof GC pause",
            "value": 1579648,
            "unit": "words"
          },
          {
            "name": "qsort (jit)",
            "value": 1425120106,
            "unit": "instructions"
          },
          {
            "name": "qsort (interp)",
            "value": 3755712128,
            "unit": "instructions"
          },
          {
            "name": "mandel (jit)",
            "value": 1033158161,
            "unit": "instructions"
          },
          {
            "name": "mandel (interp)",
            "value": 2577144708,
            "unit": "instructions"
          },
          {
            "name": "hash (jit)",
            "value": 735914605,
            "unit": "instructions"
          },
          {
            "name": "hash (interp)",
            "value": 819046851,
            "unit": "instructions"
          },
          {
            "name": "hash GC pause",
            "value": 229395,
            "unit": "words"
          },
          {
            "name": "orgparse (jit)",
            "value": 1122561037,
            "unit": "instructions"
          },
          {
            "name": "orgparse (interp)",
            "value": 1408237312,
            "unit": "instructions"
          },
          {
            "name": "callbacks (jit)",
            "value": 632493023,
            "unit": "instructions"
          },
          {
            "name": "callbacks (interp)",
            "value": 1542625337,
            "unit": "instructions"
          },
          {
            "name": "callbacks GC pause",
            "value": 327538,
            "unit": "words"
          },
          {
            "name": "editor startup (jit)",
            "value": 126222872,
            "unit": "instructions"
          },
          {
            "name": "editor startup (interp)",
            "value": 123313382,
            "unit": "instructions"
          },
          {
            "name": "keys: type (jit)",
            "value": 1211083,
            "unit": "instructions per key"
          },
          {
            "name": "keys: type-long-line (jit)",
            "value": 1213184,
            "unit": "instructions per key"
          },
          {
            "name": "keys: move (jit)",
            "value": 1115676,
            "unit": "instructions per key"
          },
          {
            "name": "keys: move-modal (jit)",
            "value": 982607,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-open (jit)",
            "value": 19061671,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-first-key (jit)",
            "value": 21927436,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-narrow (jit)",
            "value": 13709751,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-delete (jit)",
            "value": 1354752,
            "unit": "instructions per key"
          },
          {
            "name": "keys: command-first-key (jit)",
            "value": 1445748,
            "unit": "instructions per key"
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
          "id": "cccbda7c6f86671abd110a9559c8c16f966dbef8",
          "message": "Merge pull request #71 from ArthurHeymans/stop-executions\n\nStop the command C-g is for, and kill one that ignores it",
          "timestamp": "2026-10-09T11:18:15+02:00",
          "tree_id": "a9c16195da51465c57e601463210e66847feae2f",
          "url": "https://github.com/ArthurHeymans/techne/commit/cccbda7c6f86671abd110a9559c8c16f966dbef8"
        },
        "date": 1791537754836,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "startup (jit)",
            "value": 14361246,
            "unit": "instructions"
          },
          {
            "name": "startup (interp)",
            "value": 14205630,
            "unit": "instructions"
          },
          {
            "name": "fib (jit)",
            "value": 779096724,
            "unit": "instructions"
          },
          {
            "name": "fib (interp)",
            "value": 1696192687,
            "unit": "instructions"
          },
          {
            "name": "tak (jit)",
            "value": 986341869,
            "unit": "instructions"
          },
          {
            "name": "tak (interp)",
            "value": 2132296369,
            "unit": "instructions"
          },
          {
            "name": "nqueens (jit)",
            "value": 1136063725,
            "unit": "instructions"
          },
          {
            "name": "nqueens (interp)",
            "value": 2816355957,
            "unit": "instructions"
          },
          {
            "name": "nqueens GC pause",
            "value": 345,
            "unit": "words"
          },
          {
            "name": "bintrees (jit)",
            "value": 3629458505,
            "unit": "instructions"
          },
          {
            "name": "bintrees (interp)",
            "value": 8945038886,
            "unit": "instructions"
          },
          {
            "name": "bintrees GC pause",
            "value": 393138,
            "unit": "words"
          },
          {
            "name": "hof (jit)",
            "value": 1605286500,
            "unit": "instructions"
          },
          {
            "name": "hof (interp)",
            "value": 3072242149,
            "unit": "instructions"
          },
          {
            "name": "hof GC pause",
            "value": 1579648,
            "unit": "words"
          },
          {
            "name": "qsort (jit)",
            "value": 1425067593,
            "unit": "instructions"
          },
          {
            "name": "qsort (interp)",
            "value": 3756007070,
            "unit": "instructions"
          },
          {
            "name": "mandel (jit)",
            "value": 1033152927,
            "unit": "instructions"
          },
          {
            "name": "mandel (interp)",
            "value": 2577236492,
            "unit": "instructions"
          },
          {
            "name": "hash (jit)",
            "value": 735907893,
            "unit": "instructions"
          },
          {
            "name": "hash (interp)",
            "value": 819063566,
            "unit": "instructions"
          },
          {
            "name": "hash GC pause",
            "value": 229395,
            "unit": "words"
          },
          {
            "name": "orgparse (jit)",
            "value": 1122509458,
            "unit": "instructions"
          },
          {
            "name": "orgparse (interp)",
            "value": 1408284062,
            "unit": "instructions"
          },
          {
            "name": "callbacks (jit)",
            "value": 632524591,
            "unit": "instructions"
          },
          {
            "name": "callbacks (interp)",
            "value": 1542771010,
            "unit": "instructions"
          },
          {
            "name": "callbacks GC pause",
            "value": 327538,
            "unit": "words"
          },
          {
            "name": "editor startup (jit)",
            "value": 126230233,
            "unit": "instructions"
          },
          {
            "name": "editor startup (interp)",
            "value": 123337119,
            "unit": "instructions"
          },
          {
            "name": "keys: type (jit)",
            "value": 1229575,
            "unit": "instructions per key"
          },
          {
            "name": "keys: type-long-line (jit)",
            "value": 1231509,
            "unit": "instructions per key"
          },
          {
            "name": "keys: move (jit)",
            "value": 1131824,
            "unit": "instructions per key"
          },
          {
            "name": "keys: move-modal (jit)",
            "value": 1006820,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-open (jit)",
            "value": 19079254,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-first-key (jit)",
            "value": 21949819,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-narrow (jit)",
            "value": 13717278,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-delete (jit)",
            "value": 1419816,
            "unit": "instructions per key"
          },
          {
            "name": "keys: command-first-key (jit)",
            "value": 1454779,
            "unit": "instructions per key"
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
          "id": "5e39b1292c24c746bbedf7587827ea823d7828db",
          "message": "Merge pull request #74 from ArthurHeymans/stuck-runtime\n\nGive up a runtime that C-g cannot stop",
          "timestamp": "2026-10-09T13:23:29+02:00",
          "tree_id": "49383bb4f6714731e82732801ef8582fc34f09af",
          "url": "https://github.com/ArthurHeymans/techne/commit/5e39b1292c24c746bbedf7587827ea823d7828db"
        },
        "date": 1791545280799,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "startup (jit)",
            "value": 14379574,
            "unit": "instructions"
          },
          {
            "name": "startup (interp)",
            "value": 14201177,
            "unit": "instructions"
          },
          {
            "name": "fib (jit)",
            "value": 779099495,
            "unit": "instructions"
          },
          {
            "name": "fib (interp)",
            "value": 1696193572,
            "unit": "instructions"
          },
          {
            "name": "tak (jit)",
            "value": 986341125,
            "unit": "instructions"
          },
          {
            "name": "tak (interp)",
            "value": 2132296369,
            "unit": "instructions"
          },
          {
            "name": "nqueens (jit)",
            "value": 1136057163,
            "unit": "instructions"
          },
          {
            "name": "nqueens (interp)",
            "value": 2816355957,
            "unit": "instructions"
          },
          {
            "name": "nqueens GC pause",
            "value": 345,
            "unit": "words"
          },
          {
            "name": "bintrees (jit)",
            "value": 3629417526,
            "unit": "instructions"
          },
          {
            "name": "bintrees (interp)",
            "value": 8945043339,
            "unit": "instructions"
          },
          {
            "name": "bintrees GC pause",
            "value": 393138,
            "unit": "words"
          },
          {
            "name": "hof (jit)",
            "value": 1605292519,
            "unit": "instructions"
          },
          {
            "name": "hof (interp)",
            "value": 3072246602,
            "unit": "instructions"
          },
          {
            "name": "hof GC pause",
            "value": 1579648,
            "unit": "words"
          },
          {
            "name": "qsort (jit)",
            "value": 1425052197,
            "unit": "instructions"
          },
          {
            "name": "qsort (interp)",
            "value": 3756007070,
            "unit": "instructions"
          },
          {
            "name": "mandel (jit)",
            "value": 1033180484,
            "unit": "instructions"
          },
          {
            "name": "mandel (interp)",
            "value": 2577236492,
            "unit": "instructions"
          },
          {
            "name": "hash (jit)",
            "value": 735912354,
            "unit": "instructions"
          },
          {
            "name": "hash (interp)",
            "value": 819059113,
            "unit": "instructions"
          },
          {
            "name": "hash GC pause",
            "value": 229395,
            "unit": "words"
          },
          {
            "name": "orgparse (jit)",
            "value": 1122470532,
            "unit": "instructions"
          },
          {
            "name": "orgparse (interp)",
            "value": 1408284062,
            "unit": "instructions"
          },
          {
            "name": "callbacks (jit)",
            "value": 632499576,
            "unit": "instructions"
          },
          {
            "name": "callbacks (interp)",
            "value": 1542770125,
            "unit": "instructions"
          },
          {
            "name": "callbacks GC pause",
            "value": 327538,
            "unit": "words"
          },
          {
            "name": "editor startup (jit)",
            "value": 126231931,
            "unit": "instructions"
          },
          {
            "name": "editor startup (interp)",
            "value": 123638852,
            "unit": "instructions"
          },
          {
            "name": "keys: type (jit)",
            "value": 1255031,
            "unit": "instructions per key"
          },
          {
            "name": "keys: type-long-line (jit)",
            "value": 1255112,
            "unit": "instructions per key"
          },
          {
            "name": "keys: move (jit)",
            "value": 1150723,
            "unit": "instructions per key"
          },
          {
            "name": "keys: move-modal (jit)",
            "value": 1022300,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-open (jit)",
            "value": 19084104,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-first-key (jit)",
            "value": 21956632,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-narrow (jit)",
            "value": 13704820,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-delete (jit)",
            "value": 1370646,
            "unit": "instructions per key"
          },
          {
            "name": "keys: command-first-key (jit)",
            "value": 1459771,
            "unit": "instructions per key"
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
          "id": "3a0198d414920712e3806ec22143e4f42a647395",
          "message": "Merge pull request #76 from ArthurHeymans/memory-admission\n\nCount what the heap holds and refuse allocations past a limit",
          "timestamp": "2026-10-09T14:26:58+02:00",
          "tree_id": "94174b1edf992ac8ab78e6f96c06fd1053a0de03",
          "url": "https://github.com/ArthurHeymans/techne/commit/3a0198d414920712e3806ec22143e4f42a647395"
        },
        "date": 1791549086136,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "startup (jit)",
            "value": 14309074,
            "unit": "instructions"
          },
          {
            "name": "startup (interp)",
            "value": 14144506,
            "unit": "instructions"
          },
          {
            "name": "fib (jit)",
            "value": 779041938,
            "unit": "instructions"
          },
          {
            "name": "fib (interp)",
            "value": 1696139656,
            "unit": "instructions"
          },
          {
            "name": "tak (jit)",
            "value": 986284945,
            "unit": "instructions"
          },
          {
            "name": "tak (interp)",
            "value": 2132237709,
            "unit": "instructions"
          },
          {
            "name": "nqueens (jit)",
            "value": 1136004899,
            "unit": "instructions"
          },
          {
            "name": "nqueens (interp)",
            "value": 2816300879,
            "unit": "instructions"
          },
          {
            "name": "nqueens GC pause",
            "value": 345,
            "unit": "words"
          },
          {
            "name": "bintrees (jit)",
            "value": 3629395863,
            "unit": "instructions"
          },
          {
            "name": "bintrees (interp)",
            "value": 8944985669,
            "unit": "instructions"
          },
          {
            "name": "bintrees GC pause",
            "value": 393138,
            "unit": "words"
          },
          {
            "name": "hof (jit)",
            "value": 1605217428,
            "unit": "instructions"
          },
          {
            "name": "hof (interp)",
            "value": 3072184281,
            "unit": "instructions"
          },
          {
            "name": "hof GC pause",
            "value": 1579648,
            "unit": "words"
          },
          {
            "name": "qsort (jit)",
            "value": 1425027372,
            "unit": "instructions"
          },
          {
            "name": "qsort (interp)",
            "value": 3755941782,
            "unit": "instructions"
          },
          {
            "name": "mandel (jit)",
            "value": 1033089925,
            "unit": "instructions"
          },
          {
            "name": "mandel (interp)",
            "value": 2577175629,
            "unit": "instructions"
          },
          {
            "name": "hash (jit)",
            "value": 735851024,
            "unit": "instructions"
          },
          {
            "name": "hash (interp)",
            "value": 819000759,
            "unit": "instructions"
          },
          {
            "name": "hash GC pause",
            "value": 229395,
            "unit": "words"
          },
          {
            "name": "orgparse (jit)",
            "value": 1122534715,
            "unit": "instructions"
          },
          {
            "name": "orgparse (interp)",
            "value": 1408221453,
            "unit": "instructions"
          },
          {
            "name": "callbacks (jit)",
            "value": 632493390,
            "unit": "instructions"
          },
          {
            "name": "callbacks (interp)",
            "value": 1542714127,
            "unit": "instructions"
          },
          {
            "name": "callbacks GC pause",
            "value": 327538,
            "unit": "words"
          },
          {
            "name": "editor startup (jit)",
            "value": 125871131,
            "unit": "instructions"
          },
          {
            "name": "editor startup (interp)",
            "value": 123301103,
            "unit": "instructions"
          },
          {
            "name": "keys: type (jit)",
            "value": 1247622,
            "unit": "instructions per key"
          },
          {
            "name": "keys: type-long-line (jit)",
            "value": 1249625,
            "unit": "instructions per key"
          },
          {
            "name": "keys: move (jit)",
            "value": 1146351,
            "unit": "instructions per key"
          },
          {
            "name": "keys: move-modal (jit)",
            "value": 1019340,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-open (jit)",
            "value": 19077223,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-first-key (jit)",
            "value": 21957456,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-narrow (jit)",
            "value": 13701392,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-delete (jit)",
            "value": 1378160,
            "unit": "instructions per key"
          },
          {
            "name": "keys: command-first-key (jit)",
            "value": 1456798,
            "unit": "instructions per key"
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
          "id": "08f3e6b9e3353e22c3fbb93f9142fb4ca99f424c",
          "message": "Merge pull request #77 from ArthurHeymans/memory-held\n\nCount what the VM holds, not only the heap",
          "timestamp": "2026-10-09T19:06:32+02:00",
          "tree_id": "7f7b15da45cab7dd36c520b73c3e100a81ccbdba",
          "url": "https://github.com/ArthurHeymans/techne/commit/08f3e6b9e3353e22c3fbb93f9142fb4ca99f424c"
        },
        "date": 1791565863034,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "startup (jit)",
            "value": 14428544,
            "unit": "instructions"
          },
          {
            "name": "startup (interp)",
            "value": 14253110,
            "unit": "instructions"
          },
          {
            "name": "fib (jit)",
            "value": 779145155,
            "unit": "instructions"
          },
          {
            "name": "fib (interp)",
            "value": 1696244024,
            "unit": "instructions"
          },
          {
            "name": "tak (jit)",
            "value": 986392550,
            "unit": "instructions"
          },
          {
            "name": "tak (interp)",
            "value": 2132346721,
            "unit": "instructions"
          },
          {
            "name": "nqueens (jit)",
            "value": 1136817539,
            "unit": "instructions"
          },
          {
            "name": "nqueens (interp)",
            "value": 2816407297,
            "unit": "instructions"
          },
          {
            "name": "nqueens GC pause",
            "value": 345,
            "unit": "words"
          },
          {
            "name": "bintrees (jit)",
            "value": 3629473396,
            "unit": "instructions"
          },
          {
            "name": "bintrees (interp)",
            "value": 8945105509,
            "unit": "instructions"
          },
          {
            "name": "bintrees GC pause",
            "value": 393138,
            "unit": "words"
          },
          {
            "name": "hof (jit)",
            "value": 1606524092,
            "unit": "instructions"
          },
          {
            "name": "hof (interp)",
            "value": 3072287018,
            "unit": "instructions"
          },
          {
            "name": "hof GC pause",
            "value": 1579648,
            "unit": "words"
          },
          {
            "name": "qsort (jit)",
            "value": 1425121952,
            "unit": "instructions"
          },
          {
            "name": "qsort (interp)",
            "value": 3756051603,
            "unit": "instructions"
          },
          {
            "name": "mandel (jit)",
            "value": 1033680720,
            "unit": "instructions"
          },
          {
            "name": "mandel (interp)",
            "value": 2577289355,
            "unit": "instructions"
          },
          {
            "name": "hash (jit)",
            "value": 738351063,
            "unit": "instructions"
          },
          {
            "name": "hash (interp)",
            "value": 819110076,
            "unit": "instructions"
          },
          {
            "name": "hash GC pause",
            "value": 229395,
            "unit": "words"
          },
          {
            "name": "orgparse (jit)",
            "value": 1130742613,
            "unit": "instructions"
          },
          {
            "name": "orgparse (interp)",
            "value": 1408332113,
            "unit": "instructions"
          },
          {
            "name": "callbacks (jit)",
            "value": 632639240,
            "unit": "instructions"
          },
          {
            "name": "callbacks (interp)",
            "value": 1542826324,
            "unit": "instructions"
          },
          {
            "name": "callbacks GC pause",
            "value": 327538,
            "unit": "words"
          },
          {
            "name": "editor startup (jit)",
            "value": 126317418,
            "unit": "instructions"
          },
          {
            "name": "editor startup (interp)",
            "value": 123790992,
            "unit": "instructions"
          },
          {
            "name": "keys: type (jit)",
            "value": 1248478,
            "unit": "instructions per key"
          },
          {
            "name": "keys: type-long-line (jit)",
            "value": 1251089,
            "unit": "instructions per key"
          },
          {
            "name": "keys: move (jit)",
            "value": 1146261,
            "unit": "instructions per key"
          },
          {
            "name": "keys: move-modal (jit)",
            "value": 1022413,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-open (jit)",
            "value": 19338371,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-first-key (jit)",
            "value": 21761170,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-narrow (jit)",
            "value": 13741631,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-delete (jit)",
            "value": 1379807,
            "unit": "instructions per key"
          },
          {
            "name": "keys: command-first-key (jit)",
            "value": 1457525,
            "unit": "instructions per key"
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
          "id": "95f17179b403510e851f0f8ec7fc91a7ae573848",
          "message": "Merge pull request #78 from ArthurHeymans/memory-allocator\n\nCount what a world holds in the allocator",
          "timestamp": "2026-10-09T22:37:56+02:00",
          "tree_id": "b39e817590673533cc80937d5720da804fb3a9bd",
          "url": "https://github.com/ArthurHeymans/techne/commit/95f17179b403510e851f0f8ec7fc91a7ae573848"
        },
        "date": 1791578549234,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "startup (jit)",
            "value": 14939833,
            "unit": "instructions"
          },
          {
            "name": "startup (interp)",
            "value": 14785665,
            "unit": "instructions"
          },
          {
            "name": "fib (jit)",
            "value": 779711856,
            "unit": "instructions"
          },
          {
            "name": "fib (interp)",
            "value": 1714375050,
            "unit": "instructions"
          },
          {
            "name": "tak (jit)",
            "value": 986970917,
            "unit": "instructions"
          },
          {
            "name": "tak (interp)",
            "value": 2153224686,
            "unit": "instructions"
          },
          {
            "name": "nqueens (jit)",
            "value": 1138701696,
            "unit": "instructions"
          },
          {
            "name": "nqueens (interp)",
            "value": 2819818500,
            "unit": "instructions"
          },
          {
            "name": "nqueens GC pause",
            "value": 345,
            "unit": "words"
          },
          {
            "name": "bintrees (jit)",
            "value": 3630141603,
            "unit": "instructions"
          },
          {
            "name": "bintrees (interp)",
            "value": 9020470091,
            "unit": "instructions"
          },
          {
            "name": "bintrees GC pause",
            "value": 393138,
            "unit": "words"
          },
          {
            "name": "hof (jit)",
            "value": 1606577991,
            "unit": "instructions"
          },
          {
            "name": "hof (interp)",
            "value": 3075868905,
            "unit": "instructions"
          },
          {
            "name": "hof GC pause",
            "value": 1579648,
            "unit": "words"
          },
          {
            "name": "qsort (jit)",
            "value": 1425812578,
            "unit": "instructions"
          },
          {
            "name": "qsort (interp)",
            "value": 3762087352,
            "unit": "instructions"
          },
          {
            "name": "mandel (jit)",
            "value": 1033855628,
            "unit": "instructions"
          },
          {
            "name": "mandel (interp)",
            "value": 2581198254,
            "unit": "instructions"
          },
          {
            "name": "hash (jit)",
            "value": 735047030,
            "unit": "instructions"
          },
          {
            "name": "hash (interp)",
            "value": 816920395,
            "unit": "instructions"
          },
          {
            "name": "hash GC pause",
            "value": 229395,
            "unit": "words"
          },
          {
            "name": "orgparse (jit)",
            "value": 1123902735,
            "unit": "instructions"
          },
          {
            "name": "orgparse (interp)",
            "value": 1407665982,
            "unit": "instructions"
          },
          {
            "name": "callbacks (jit)",
            "value": 633306598,
            "unit": "instructions"
          },
          {
            "name": "callbacks (interp)",
            "value": 1547657096,
            "unit": "instructions"
          },
          {
            "name": "callbacks GC pause",
            "value": 327538,
            "unit": "words"
          },
          {
            "name": "editor startup (jit)",
            "value": 130479826,
            "unit": "instructions"
          },
          {
            "name": "editor startup (interp)",
            "value": 127926015,
            "unit": "instructions"
          },
          {
            "name": "keys: type (jit)",
            "value": 1259697,
            "unit": "instructions per key"
          },
          {
            "name": "keys: type-long-line (jit)",
            "value": 1261528,
            "unit": "instructions per key"
          },
          {
            "name": "keys: move (jit)",
            "value": 1154712,
            "unit": "instructions per key"
          },
          {
            "name": "keys: move-modal (jit)",
            "value": 1022202,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-open (jit)",
            "value": 18718655,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-first-key (jit)",
            "value": 22064392,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-narrow (jit)",
            "value": 13864513,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-delete (jit)",
            "value": 1400430,
            "unit": "instructions per key"
          },
          {
            "name": "keys: command-first-key (jit)",
            "value": 1505581,
            "unit": "instructions per key"
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
          "id": "198e1b0f4a78222a001dd2f75b9e21ec5c3fa624",
          "message": "Merge pull request #79 from ArthurHeymans/memory-pressure\n\nRefuse what grows a world past its limit, kill what goes on",
          "timestamp": "2026-10-10T08:45:55+02:00",
          "tree_id": "11e18feb8a635e56fced130ca6cff99ce396659a",
          "url": "https://github.com/ArthurHeymans/techne/commit/198e1b0f4a78222a001dd2f75b9e21ec5c3fa624"
        },
        "date": 1791615025608,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "startup (jit)",
            "value": 15342411,
            "unit": "instructions"
          },
          {
            "name": "startup (interp)",
            "value": 15186104,
            "unit": "instructions"
          },
          {
            "name": "fib (jit)",
            "value": 780128421,
            "unit": "instructions"
          },
          {
            "name": "fib (interp)",
            "value": 1697456995,
            "unit": "instructions"
          },
          {
            "name": "tak (jit)",
            "value": 987392120,
            "unit": "instructions"
          },
          {
            "name": "tak (interp)",
            "value": 2133639848,
            "unit": "instructions"
          },
          {
            "name": "nqueens (jit)",
            "value": 1143756671,
            "unit": "instructions"
          },
          {
            "name": "nqueens (interp)",
            "value": 2824662736,
            "unit": "instructions"
          },
          {
            "name": "nqueens GC pause",
            "value": 345,
            "unit": "words"
          },
          {
            "name": "bintrees (jit)",
            "value": 3630575138,
            "unit": "instructions"
          },
          {
            "name": "bintrees (interp)",
            "value": 8947253354,
            "unit": "instructions"
          },
          {
            "name": "bintrees GC pause",
            "value": 393138,
            "unit": "words"
          },
          {
            "name": "hof (jit)",
            "value": 1607041674,
            "unit": "instructions"
          },
          {
            "name": "hof (interp)",
            "value": 3074281723,
            "unit": "instructions"
          },
          {
            "name": "hof GC pause",
            "value": 1579648,
            "unit": "words"
          },
          {
            "name": "qsort (jit)",
            "value": 1426304710,
            "unit": "instructions"
          },
          {
            "name": "qsort (interp)",
            "value": 3757451740,
            "unit": "instructions"
          },
          {
            "name": "mandel (jit)",
            "value": 1034274512,
            "unit": "instructions"
          },
          {
            "name": "mandel (interp)",
            "value": 2578382858,
            "unit": "instructions"
          },
          {
            "name": "hash (jit)",
            "value": 741801318,
            "unit": "instructions"
          },
          {
            "name": "hash (interp)",
            "value": 824875339,
            "unit": "instructions"
          },
          {
            "name": "hash GC pause",
            "value": 229395,
            "unit": "words"
          },
          {
            "name": "orgparse (jit)",
            "value": 1124221859,
            "unit": "instructions"
          },
          {
            "name": "orgparse (interp)",
            "value": 1409741289,
            "unit": "instructions"
          },
          {
            "name": "callbacks (jit)",
            "value": 633912381,
            "unit": "instructions"
          },
          {
            "name": "callbacks (interp)",
            "value": 1544077513,
            "unit": "instructions"
          },
          {
            "name": "callbacks GC pause",
            "value": 327538,
            "unit": "words"
          },
          {
            "name": "editor startup (jit)",
            "value": 134639842,
            "unit": "instructions"
          },
          {
            "name": "editor startup (interp)",
            "value": 132091283,
            "unit": "instructions"
          },
          {
            "name": "keys: type (jit)",
            "value": 1289858,
            "unit": "instructions per key"
          },
          {
            "name": "keys: type-long-line (jit)",
            "value": 1291756,
            "unit": "instructions per key"
          },
          {
            "name": "keys: move (jit)",
            "value": 1183149,
            "unit": "instructions per key"
          },
          {
            "name": "keys: move-modal (jit)",
            "value": 1046921,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-open (jit)",
            "value": 18778353,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-first-key (jit)",
            "value": 22116181,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-narrow (jit)",
            "value": 13839945,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-delete (jit)",
            "value": 1448493,
            "unit": "instructions per key"
          },
          {
            "name": "keys: command-first-key (jit)",
            "value": 1557052,
            "unit": "instructions per key"
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
          "id": "d3c86be0141c3720f38be5958fa6d1c9267b6d36",
          "message": "Merge pull request #80 from ArthurHeymans/flaky-tests\n\nMake the tests that flaked under load wait for what they check",
          "timestamp": "2026-10-10T11:33:56+02:00",
          "tree_id": "72261cdcc4cec9e3a8090c5e8eb60f9d09e46122",
          "url": "https://github.com/ArthurHeymans/techne/commit/d3c86be0141c3720f38be5958fa6d1c9267b6d36"
        },
        "date": 1791625103413,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "startup (jit)",
            "value": 15356839,
            "unit": "instructions"
          },
          {
            "name": "startup (interp)",
            "value": 15186104,
            "unit": "instructions"
          },
          {
            "name": "fib (jit)",
            "value": 780126924,
            "unit": "instructions"
          },
          {
            "name": "fib (interp)",
            "value": 1697456995,
            "unit": "instructions"
          },
          {
            "name": "tak (jit)",
            "value": 987394126,
            "unit": "instructions"
          },
          {
            "name": "tak (interp)",
            "value": 2133644349,
            "unit": "instructions"
          },
          {
            "name": "nqueens (jit)",
            "value": 1143759835,
            "unit": "instructions"
          },
          {
            "name": "nqueens (interp)",
            "value": 2824662736,
            "unit": "instructions"
          },
          {
            "name": "nqueens GC pause",
            "value": 345,
            "unit": "words"
          },
          {
            "name": "bintrees (jit)",
            "value": 3630579541,
            "unit": "instructions"
          },
          {
            "name": "bintrees (interp)",
            "value": 8947253354,
            "unit": "instructions"
          },
          {
            "name": "bintrees GC pause",
            "value": 393138,
            "unit": "words"
          },
          {
            "name": "hof (jit)",
            "value": 1607048858,
            "unit": "instructions"
          },
          {
            "name": "hof (interp)",
            "value": 3074286224,
            "unit": "instructions"
          },
          {
            "name": "hof GC pause",
            "value": 1579648,
            "unit": "words"
          },
          {
            "name": "qsort (jit)",
            "value": 1426290303,
            "unit": "instructions"
          },
          {
            "name": "qsort (interp)",
            "value": 3757451740,
            "unit": "instructions"
          },
          {
            "name": "mandel (jit)",
            "value": 1034277199,
            "unit": "instructions"
          },
          {
            "name": "mandel (interp)",
            "value": 2578387359,
            "unit": "instructions"
          },
          {
            "name": "hash (jit)",
            "value": 741811941,
            "unit": "instructions"
          },
          {
            "name": "hash (interp)",
            "value": 824870838,
            "unit": "instructions"
          },
          {
            "name": "hash GC pause",
            "value": 229395,
            "unit": "words"
          },
          {
            "name": "orgparse (jit)",
            "value": 1124351398,
            "unit": "instructions"
          },
          {
            "name": "orgparse (interp)",
            "value": 1409741289,
            "unit": "instructions"
          },
          {
            "name": "callbacks (jit)",
            "value": 633909101,
            "unit": "instructions"
          },
          {
            "name": "callbacks (interp)",
            "value": 1544077513,
            "unit": "instructions"
          },
          {
            "name": "callbacks GC pause",
            "value": 327538,
            "unit": "words"
          },
          {
            "name": "editor startup (jit)",
            "value": 134639711,
            "unit": "instructions"
          },
          {
            "name": "editor startup (interp)",
            "value": 132091283,
            "unit": "instructions"
          },
          {
            "name": "keys: type (jit)",
            "value": 1289723,
            "unit": "instructions per key"
          },
          {
            "name": "keys: type-long-line (jit)",
            "value": 1291497,
            "unit": "instructions per key"
          },
          {
            "name": "keys: move (jit)",
            "value": 1183137,
            "unit": "instructions per key"
          },
          {
            "name": "keys: move-modal (jit)",
            "value": 1046976,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-open (jit)",
            "value": 18778389,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-first-key (jit)",
            "value": 22116211,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-narrow (jit)",
            "value": 13839916,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-delete (jit)",
            "value": 1448472,
            "unit": "instructions per key"
          },
          {
            "name": "keys: command-first-key (jit)",
            "value": 1557167,
            "unit": "instructions per key"
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
          "id": "b33a2430e565eccd9edc6ace8b5094c4681a465f",
          "message": "Merge pull request #81 from ArthurHeymans/memory-natives\n\nAdmit what natives make of far more than they are given",
          "timestamp": "2026-10-10T11:41:01+02:00",
          "tree_id": "75f76678565139f1038801f92915f1479bd5e204",
          "url": "https://github.com/ArthurHeymans/techne/commit/b33a2430e565eccd9edc6ace8b5094c4681a465f"
        },
        "date": 1791625477302,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "startup (jit)",
            "value": 15296996,
            "unit": "instructions"
          },
          {
            "name": "startup (interp)",
            "value": 15123999,
            "unit": "instructions"
          },
          {
            "name": "fib (jit)",
            "value": 780062245,
            "unit": "instructions"
          },
          {
            "name": "fib (interp)",
            "value": 1697399177,
            "unit": "instructions"
          },
          {
            "name": "tak (jit)",
            "value": 987332132,
            "unit": "instructions"
          },
          {
            "name": "tak (interp)",
            "value": 2133577192,
            "unit": "instructions"
          },
          {
            "name": "nqueens (jit)",
            "value": 1154421499,
            "unit": "instructions"
          },
          {
            "name": "nqueens (interp)",
            "value": 2836782038,
            "unit": "instructions"
          },
          {
            "name": "nqueens GC pause",
            "value": 345,
            "unit": "words"
          },
          {
            "name": "bintrees (jit)",
            "value": 3630560021,
            "unit": "instructions"
          },
          {
            "name": "bintrees (interp)",
            "value": 8962175178,
            "unit": "instructions"
          },
          {
            "name": "bintrees GC pause",
            "value": 393138,
            "unit": "words"
          },
          {
            "name": "hof (jit)",
            "value": 1620955293,
            "unit": "instructions"
          },
          {
            "name": "hof (interp)",
            "value": 3091017932,
            "unit": "instructions"
          },
          {
            "name": "hof GC pause",
            "value": 1579648,
            "unit": "words"
          },
          {
            "name": "qsort (jit)",
            "value": 1426199318,
            "unit": "instructions"
          },
          {
            "name": "qsort (interp)",
            "value": 3757384070,
            "unit": "instructions"
          },
          {
            "name": "mandel (jit)",
            "value": 1034217170,
            "unit": "instructions"
          },
          {
            "name": "mandel (interp)",
            "value": 2578319445,
            "unit": "instructions"
          },
          {
            "name": "hash (jit)",
            "value": 740940370,
            "unit": "instructions"
          },
          {
            "name": "hash (interp)",
            "value": 824008927,
            "unit": "instructions"
          },
          {
            "name": "hash GC pause",
            "value": 229395,
            "unit": "words"
          },
          {
            "name": "orgparse (jit)",
            "value": 1136948770,
            "unit": "instructions"
          },
          {
            "name": "orgparse (interp)",
            "value": 1422399557,
            "unit": "instructions"
          },
          {
            "name": "callbacks (jit)",
            "value": 633827135,
            "unit": "instructions"
          },
          {
            "name": "callbacks (interp)",
            "value": 1545338849,
            "unit": "instructions"
          },
          {
            "name": "callbacks GC pause",
            "value": 327538,
            "unit": "words"
          },
          {
            "name": "editor startup (jit)",
            "value": 134220722,
            "unit": "instructions"
          },
          {
            "name": "editor startup (interp)",
            "value": 131685658,
            "unit": "instructions"
          },
          {
            "name": "keys: type (jit)",
            "value": 1294637,
            "unit": "instructions per key"
          },
          {
            "name": "keys: type-long-line (jit)",
            "value": 1296346,
            "unit": "instructions per key"
          },
          {
            "name": "keys: move (jit)",
            "value": 1187730,
            "unit": "instructions per key"
          },
          {
            "name": "keys: move-modal (jit)",
            "value": 1050837,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-open (jit)",
            "value": 18786037,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-first-key (jit)",
            "value": 22106842,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-narrow (jit)",
            "value": 13830342,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-delete (jit)",
            "value": 1438628,
            "unit": "instructions per key"
          },
          {
            "name": "keys: command-first-key (jit)",
            "value": 1551199,
            "unit": "instructions per key"
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
          "id": "f8843895dc63eb0f2750d04fedd01aeecfb1c4d1",
          "message": "Merge pull request #82 from ArthurHeymans/lopsided-gcd\n\nReduce ratios with a gcd that suits lopsided numbers",
          "timestamp": "2026-10-10T21:08:30+02:00",
          "tree_id": "585fd96de31912a42a4288a306cfc706b1a81176",
          "url": "https://github.com/ArthurHeymans/techne/commit/f8843895dc63eb0f2750d04fedd01aeecfb1c4d1"
        },
        "date": 1791659573919,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "startup (jit)",
            "value": 15286216,
            "unit": "instructions"
          },
          {
            "name": "startup (interp)",
            "value": 15132173,
            "unit": "instructions"
          },
          {
            "name": "fib (jit)",
            "value": 780067322,
            "unit": "instructions"
          },
          {
            "name": "fib (interp)",
            "value": 1697405957,
            "unit": "instructions"
          },
          {
            "name": "tak (jit)",
            "value": 987328572,
            "unit": "instructions"
          },
          {
            "name": "tak (interp)",
            "value": 2133590042,
            "unit": "instructions"
          },
          {
            "name": "nqueens (jit)",
            "value": 1154428336,
            "unit": "instructions"
          },
          {
            "name": "nqueens (interp)",
            "value": 2836792537,
            "unit": "instructions"
          },
          {
            "name": "nqueens GC pause",
            "value": 345,
            "unit": "words"
          },
          {
            "name": "bintrees (jit)",
            "value": 3630566710,
            "unit": "instructions"
          },
          {
            "name": "bintrees (interp)",
            "value": 8962184255,
            "unit": "instructions"
          },
          {
            "name": "bintrees GC pause",
            "value": 393138,
            "unit": "words"
          },
          {
            "name": "hof (jit)",
            "value": 1620988114,
            "unit": "instructions"
          },
          {
            "name": "hof (interp)",
            "value": 3091026178,
            "unit": "instructions"
          },
          {
            "name": "hof GC pause",
            "value": 1579648,
            "unit": "words"
          },
          {
            "name": "qsort (jit)",
            "value": 1426255823,
            "unit": "instructions"
          },
          {
            "name": "qsort (interp)",
            "value": 3757400410,
            "unit": "instructions"
          },
          {
            "name": "mandel (jit)",
            "value": 1034207306,
            "unit": "instructions"
          },
          {
            "name": "mandel (interp)",
            "value": 2578328764,
            "unit": "instructions"
          },
          {
            "name": "hash (jit)",
            "value": 740948954,
            "unit": "instructions"
          },
          {
            "name": "hash (interp)",
            "value": 824018733,
            "unit": "instructions"
          },
          {
            "name": "hash GC pause",
            "value": 229395,
            "unit": "words"
          },
          {
            "name": "orgparse (jit)",
            "value": 1136903410,
            "unit": "instructions"
          },
          {
            "name": "orgparse (interp)",
            "value": 1422415846,
            "unit": "instructions"
          },
          {
            "name": "callbacks (jit)",
            "value": 633853962,
            "unit": "instructions"
          },
          {
            "name": "callbacks (interp)",
            "value": 1545342877,
            "unit": "instructions"
          },
          {
            "name": "callbacks GC pause",
            "value": 327538,
            "unit": "words"
          },
          {
            "name": "editor startup (jit)",
            "value": 134292169,
            "unit": "instructions"
          },
          {
            "name": "editor startup (interp)",
            "value": 131756762,
            "unit": "instructions"
          },
          {
            "name": "keys: type (jit)",
            "value": 1294640,
            "unit": "instructions per key"
          },
          {
            "name": "keys: type-long-line (jit)",
            "value": 1296343,
            "unit": "instructions per key"
          },
          {
            "name": "keys: move (jit)",
            "value": 1187780,
            "unit": "instructions per key"
          },
          {
            "name": "keys: move-modal (jit)",
            "value": 1050917,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-open (jit)",
            "value": 18786156,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-first-key (jit)",
            "value": 22106895,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-narrow (jit)",
            "value": 13830397,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-delete (jit)",
            "value": 1440225,
            "unit": "instructions per key"
          },
          {
            "name": "keys: command-first-key (jit)",
            "value": 1552454,
            "unit": "instructions per key"
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
          "id": "e5915404d4ab3b7fd78aebaa4c5eb30269c361e4",
          "message": "Merge pull request #83 from ArthurHeymans/memory-meter\n\nMeter reading, macro expansion and compilation",
          "timestamp": "2026-10-11T00:02:25+02:00",
          "tree_id": "74ed5b38368a9921a4eebfbb7492a3509a7d0777",
          "url": "https://github.com/ArthurHeymans/techne/commit/e5915404d4ab3b7fd78aebaa4c5eb30269c361e4"
        },
        "date": 1791669966736,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "startup (jit)",
            "value": 15663834,
            "unit": "instructions"
          },
          {
            "name": "startup (interp)",
            "value": 15499569,
            "unit": "instructions"
          },
          {
            "name": "fib (jit)",
            "value": 780436142,
            "unit": "instructions"
          },
          {
            "name": "fib (interp)",
            "value": 1697771179,
            "unit": "instructions"
          },
          {
            "name": "tak (jit)",
            "value": 987705970,
            "unit": "instructions"
          },
          {
            "name": "tak (interp)",
            "value": 2133954452,
            "unit": "instructions"
          },
          {
            "name": "nqueens (jit)",
            "value": 1153623131,
            "unit": "instructions"
          },
          {
            "name": "nqueens (interp)",
            "value": 2836862921,
            "unit": "instructions"
          },
          {
            "name": "nqueens GC pause",
            "value": 345,
            "unit": "words"
          },
          {
            "name": "bintrees (jit)",
            "value": 3630912166,
            "unit": "instructions"
          },
          {
            "name": "bintrees (interp)",
            "value": 8962557080,
            "unit": "instructions"
          },
          {
            "name": "bintrees GC pause",
            "value": 393138,
            "unit": "words"
          },
          {
            "name": "hof (jit)",
            "value": 1621370941,
            "unit": "instructions"
          },
          {
            "name": "hof (interp)",
            "value": 3091414966,
            "unit": "instructions"
          },
          {
            "name": "hof GC pause",
            "value": 1579648,
            "unit": "words"
          },
          {
            "name": "qsort (jit)",
            "value": 1426630583,
            "unit": "instructions"
          },
          {
            "name": "qsort (interp)",
            "value": 3757780304,
            "unit": "instructions"
          },
          {
            "name": "mandel (jit)",
            "value": 1034598683,
            "unit": "instructions"
          },
          {
            "name": "mandel (interp)",
            "value": 2578703556,
            "unit": "instructions"
          },
          {
            "name": "hash (jit)",
            "value": 741322260,
            "unit": "instructions"
          },
          {
            "name": "hash (interp)",
            "value": 824395139,
            "unit": "instructions"
          },
          {
            "name": "hash GC pause",
            "value": 229395,
            "unit": "words"
          },
          {
            "name": "orgparse (jit)",
            "value": 1137371258,
            "unit": "instructions"
          },
          {
            "name": "orgparse (interp)",
            "value": 1422813927,
            "unit": "instructions"
          },
          {
            "name": "callbacks (jit)",
            "value": 634243908,
            "unit": "instructions"
          },
          {
            "name": "callbacks (interp)",
            "value": 1545735727,
            "unit": "instructions"
          },
          {
            "name": "callbacks GC pause",
            "value": 327538,
            "unit": "words"
          },
          {
            "name": "editor startup (jit)",
            "value": 137717106,
            "unit": "instructions"
          },
          {
            "name": "editor startup (interp)",
            "value": 135175699,
            "unit": "instructions"
          },
          {
            "name": "keys: type (jit)",
            "value": 1294232,
            "unit": "instructions per key"
          },
          {
            "name": "keys: type-long-line (jit)",
            "value": 1295925,
            "unit": "instructions per key"
          },
          {
            "name": "keys: move (jit)",
            "value": 1187492,
            "unit": "instructions per key"
          },
          {
            "name": "keys: move-modal (jit)",
            "value": 1051067,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-open (jit)",
            "value": 18801872,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-first-key (jit)",
            "value": 22107268,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-narrow (jit)",
            "value": 13832240,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-delete (jit)",
            "value": 1437297,
            "unit": "instructions per key"
          },
          {
            "name": "keys: command-first-key (jit)",
            "value": 1549823,
            "unit": "instructions per key"
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
          "id": "05d7025dea7f4a3775502fafbfe15bece74849eb",
          "message": "Merge pull request #85 from ArthurHeymans/http-bench\n\nBenchmark an HTTP server in techne against other languages",
          "timestamp": "2026-10-11T00:03:38+02:00",
          "tree_id": "e534d7faaddc87f1ac7b9660a8f7e9e5d22029f8",
          "url": "https://github.com/ArthurHeymans/techne/commit/05d7025dea7f4a3775502fafbfe15bece74849eb"
        },
        "date": 1791670336311,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "startup (jit)",
            "value": 15668392,
            "unit": "instructions"
          },
          {
            "name": "startup (interp)",
            "value": 15495068,
            "unit": "instructions"
          },
          {
            "name": "fib (jit)",
            "value": 780430304,
            "unit": "instructions"
          },
          {
            "name": "fib (interp)",
            "value": 1697766678,
            "unit": "instructions"
          },
          {
            "name": "tak (jit)",
            "value": 987704572,
            "unit": "instructions"
          },
          {
            "name": "tak (interp)",
            "value": 2133954452,
            "unit": "instructions"
          },
          {
            "name": "nqueens (jit)",
            "value": 1153620988,
            "unit": "instructions"
          },
          {
            "name": "nqueens (interp)",
            "value": 2836862921,
            "unit": "instructions"
          },
          {
            "name": "nqueens GC pause",
            "value": 345,
            "unit": "words"
          },
          {
            "name": "bintrees (jit)",
            "value": 3630946389,
            "unit": "instructions"
          },
          {
            "name": "bintrees (interp)",
            "value": 8962557080,
            "unit": "instructions"
          },
          {
            "name": "bintrees GC pause",
            "value": 393138,
            "unit": "words"
          },
          {
            "name": "hof (jit)",
            "value": 1621364536,
            "unit": "instructions"
          },
          {
            "name": "hof (interp)",
            "value": 3091414966,
            "unit": "instructions"
          },
          {
            "name": "hof GC pause",
            "value": 1579648,
            "unit": "words"
          },
          {
            "name": "qsort (jit)",
            "value": 1426614703,
            "unit": "instructions"
          },
          {
            "name": "qsort (interp)",
            "value": 3757775803,
            "unit": "instructions"
          },
          {
            "name": "mandel (jit)",
            "value": 1034588069,
            "unit": "instructions"
          },
          {
            "name": "mandel (interp)",
            "value": 2578708057,
            "unit": "instructions"
          },
          {
            "name": "hash (jit)",
            "value": 741309099,
            "unit": "instructions"
          },
          {
            "name": "hash (interp)",
            "value": 824390638,
            "unit": "instructions"
          },
          {
            "name": "hash GC pause",
            "value": 229395,
            "unit": "words"
          },
          {
            "name": "orgparse (jit)",
            "value": 1137365470,
            "unit": "instructions"
          },
          {
            "name": "orgparse (interp)",
            "value": 1422813927,
            "unit": "instructions"
          },
          {
            "name": "callbacks (jit)",
            "value": 634265726,
            "unit": "instructions"
          },
          {
            "name": "callbacks (interp)",
            "value": 1545735727,
            "unit": "instructions"
          },
          {
            "name": "callbacks GC pause",
            "value": 327538,
            "unit": "words"
          },
          {
            "name": "editor startup (jit)",
            "value": 137717481,
            "unit": "instructions"
          },
          {
            "name": "editor startup (interp)",
            "value": 135175699,
            "unit": "instructions"
          },
          {
            "name": "keys: type (jit)",
            "value": 1294156,
            "unit": "instructions per key"
          },
          {
            "name": "keys: type-long-line (jit)",
            "value": 1296007,
            "unit": "instructions per key"
          },
          {
            "name": "keys: move (jit)",
            "value": 1187434,
            "unit": "instructions per key"
          },
          {
            "name": "keys: move-modal (jit)",
            "value": 1051104,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-open (jit)",
            "value": 18801872,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-first-key (jit)",
            "value": 22107232,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-narrow (jit)",
            "value": 13832240,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-delete (jit)",
            "value": 1437318,
            "unit": "instructions per key"
          },
          {
            "name": "keys: command-first-key (jit)",
            "value": 1549513,
            "unit": "instructions per key"
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
          "id": "cadfefb5f1df9133e46cf560a6591160497d7fc3",
          "message": "Merge pull request #87 from ArthurHeymans/bench-ci\n\nCatch regressions in the HTTP server's tasks, async waits and native calls",
          "timestamp": "2026-10-11T00:51:30+02:00",
          "tree_id": "696d4409d60ddb4383fafd60102c1eeed27042ed",
          "url": "https://github.com/ArthurHeymans/techne/commit/cadfefb5f1df9133e46cf560a6591160497d7fc3"
        },
        "date": 1791672982859,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "startup (jit)",
            "value": 15649845,
            "unit": "instructions"
          },
          {
            "name": "startup (interp)",
            "value": 15495506,
            "unit": "instructions"
          },
          {
            "name": "fib (jit)",
            "value": 780435600,
            "unit": "instructions"
          },
          {
            "name": "fib (interp)",
            "value": 1697767127,
            "unit": "instructions"
          },
          {
            "name": "tak (jit)",
            "value": 987705836,
            "unit": "instructions"
          },
          {
            "name": "tak (interp)",
            "value": 2133959402,
            "unit": "instructions"
          },
          {
            "name": "nqueens (jit)",
            "value": 1153621937,
            "unit": "instructions"
          },
          {
            "name": "nqueens (interp)",
            "value": 2836863361,
            "unit": "instructions"
          },
          {
            "name": "nqueens GC pause",
            "value": 345,
            "unit": "words"
          },
          {
            "name": "bintrees (jit)",
            "value": 3630946297,
            "unit": "instructions"
          },
          {
            "name": "bintrees (interp)",
            "value": 8962557534,
            "unit": "instructions"
          },
          {
            "name": "bintrees GC pause",
            "value": 393138,
            "unit": "words"
          },
          {
            "name": "hof (jit)",
            "value": 1621365623,
            "unit": "instructions"
          },
          {
            "name": "hof (interp)",
            "value": 3091415413,
            "unit": "instructions"
          },
          {
            "name": "hof GC pause",
            "value": 1579648,
            "unit": "words"
          },
          {
            "name": "qsort (jit)",
            "value": 1426608083,
            "unit": "instructions"
          },
          {
            "name": "qsort (interp)",
            "value": 3757780742,
            "unit": "instructions"
          },
          {
            "name": "mandel (jit)",
            "value": 1034632842,
            "unit": "instructions"
          },
          {
            "name": "mandel (interp)",
            "value": 2578703982,
            "unit": "instructions"
          },
          {
            "name": "hash (jit)",
            "value": 741325733,
            "unit": "instructions"
          },
          {
            "name": "hash (interp)",
            "value": 824391101,
            "unit": "instructions"
          },
          {
            "name": "hash GC pause",
            "value": 229395,
            "unit": "words"
          },
          {
            "name": "orgparse (jit)",
            "value": 1137368458,
            "unit": "instructions"
          },
          {
            "name": "orgparse (interp)",
            "value": 1422814377,
            "unit": "instructions"
          },
          {
            "name": "callbacks (jit)",
            "value": 634232205,
            "unit": "instructions"
          },
          {
            "name": "callbacks (interp)",
            "value": 1545740670,
            "unit": "instructions"
          },
          {
            "name": "callbacks GC pause",
            "value": 327538,
            "unit": "words"
          },
          {
            "name": "numstr (jit)",
            "value": 838002794,
            "unit": "instructions"
          },
          {
            "name": "numstr (interp)",
            "value": 922279874,
            "unit": "instructions"
          },
          {
            "name": "tasks (jit)",
            "value": 1318480344,
            "unit": "instructions"
          },
          {
            "name": "tasks (interp)",
            "value": 1220264232,
            "unit": "instructions"
          },
          {
            "name": "editor startup (jit)",
            "value": 137717035,
            "unit": "instructions"
          },
          {
            "name": "editor startup (interp)",
            "value": 135175883,
            "unit": "instructions"
          },
          {
            "name": "keys: type (jit)",
            "value": 1294190,
            "unit": "instructions per key"
          },
          {
            "name": "keys: type-long-line (jit)",
            "value": 1295867,
            "unit": "instructions per key"
          },
          {
            "name": "keys: move (jit)",
            "value": 1187431,
            "unit": "instructions per key"
          },
          {
            "name": "keys: move-modal (jit)",
            "value": 1051003,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-open (jit)",
            "value": 18801940,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-first-key (jit)",
            "value": 22107232,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-narrow (jit)",
            "value": 13832253,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-delete (jit)",
            "value": 1437358,
            "unit": "instructions per key"
          },
          {
            "name": "keys: command-first-key (jit)",
            "value": 1551071,
            "unit": "instructions per key"
          },
          {
            "name": "http: plaintext request (jit)",
            "value": 8523,
            "unit": "instructions per request"
          },
          {
            "name": "http: plaintext request (interp)",
            "value": 8806,
            "unit": "instructions per request"
          },
          {
            "name": "http: /work request (jit)",
            "value": 819906,
            "unit": "instructions per request"
          },
          {
            "name": "http: /work request (interp)",
            "value": 936396,
            "unit": "instructions per request"
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
          "id": "9b5aa67359fc3e35e384afc995bba0ddfb8ce205",
          "message": "Merge pull request #88 from ArthurHeymans/fast-number-string\n\nMake strings from numbers and strings without temporary copies",
          "timestamp": "2026-10-11T00:57:29+02:00",
          "tree_id": "c01c074c29f208d85bb8bcfb7e40f40b7737bd21",
          "url": "https://github.com/ArthurHeymans/techne/commit/9b5aa67359fc3e35e384afc995bba0ddfb8ce205"
        },
        "date": 1791673341608,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "startup (jit)",
            "value": 15650464,
            "unit": "instructions"
          },
          {
            "name": "startup (interp)",
            "value": 15497032,
            "unit": "instructions"
          },
          {
            "name": "fib (jit)",
            "value": 780434267,
            "unit": "instructions"
          },
          {
            "name": "fib (interp)",
            "value": 1697764132,
            "unit": "instructions"
          },
          {
            "name": "tak (jit)",
            "value": 987703911,
            "unit": "instructions"
          },
          {
            "name": "tak (interp)",
            "value": 2133956386,
            "unit": "instructions"
          },
          {
            "name": "nqueens (jit)",
            "value": 1153621076,
            "unit": "instructions"
          },
          {
            "name": "nqueens (interp)",
            "value": 2836860300,
            "unit": "instructions"
          },
          {
            "name": "nqueens GC pause",
            "value": 345,
            "unit": "words"
          },
          {
            "name": "bintrees (jit)",
            "value": 3630891768,
            "unit": "instructions"
          },
          {
            "name": "bintrees (interp)",
            "value": 8962554474,
            "unit": "instructions"
          },
          {
            "name": "bintrees GC pause",
            "value": 393138,
            "unit": "words"
          },
          {
            "name": "hof (jit)",
            "value": 1621376617,
            "unit": "instructions"
          },
          {
            "name": "hof (interp)",
            "value": 3091407862,
            "unit": "instructions"
          },
          {
            "name": "hof GC pause",
            "value": 1579648,
            "unit": "words"
          },
          {
            "name": "qsort (jit)",
            "value": 1426636520,
            "unit": "instructions"
          },
          {
            "name": "qsort (interp)",
            "value": 3757774033,
            "unit": "instructions"
          },
          {
            "name": "mandel (jit)",
            "value": 1034582716,
            "unit": "instructions"
          },
          {
            "name": "mandel (interp)",
            "value": 2578700963,
            "unit": "instructions"
          },
          {
            "name": "hash (jit)",
            "value": 658144483,
            "unit": "instructions"
          },
          {
            "name": "hash (interp)",
            "value": 741212279,
            "unit": "instructions"
          },
          {
            "name": "hash GC pause",
            "value": 229395,
            "unit": "words"
          },
          {
            "name": "orgparse (jit)",
            "value": 1136606608,
            "unit": "instructions"
          },
          {
            "name": "orgparse (interp)",
            "value": 1422025646,
            "unit": "instructions"
          },
          {
            "name": "callbacks (jit)",
            "value": 634282869,
            "unit": "instructions"
          },
          {
            "name": "callbacks (interp)",
            "value": 1545733370,
            "unit": "instructions"
          },
          {
            "name": "callbacks GC pause",
            "value": 327538,
            "unit": "words"
          },
          {
            "name": "numstr (jit)",
            "value": 627721147,
            "unit": "instructions"
          },
          {
            "name": "numstr (interp)",
            "value": 711992445,
            "unit": "instructions"
          },
          {
            "name": "tasks (jit)",
            "value": 1318361055,
            "unit": "instructions"
          },
          {
            "name": "tasks (interp)",
            "value": 1220236030,
            "unit": "instructions"
          },
          {
            "name": "editor startup (jit)",
            "value": 137702693,
            "unit": "instructions"
          },
          {
            "name": "editor startup (interp)",
            "value": 135162922,
            "unit": "instructions"
          },
          {
            "name": "keys: type (jit)",
            "value": 1295803,
            "unit": "instructions per key"
          },
          {
            "name": "keys: type-long-line (jit)",
            "value": 1297551,
            "unit": "instructions per key"
          },
          {
            "name": "keys: move (jit)",
            "value": 1189367,
            "unit": "instructions per key"
          },
          {
            "name": "keys: move-modal (jit)",
            "value": 1052498,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-open (jit)",
            "value": 18798743,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-first-key (jit)",
            "value": 22106886,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-narrow (jit)",
            "value": 13831730,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-delete (jit)",
            "value": 1436841,
            "unit": "instructions per key"
          },
          {
            "name": "keys: command-first-key (jit)",
            "value": 1547788,
            "unit": "instructions per key"
          },
          {
            "name": "http: plaintext request (jit)",
            "value": 8464,
            "unit": "instructions per request"
          },
          {
            "name": "http: plaintext request (interp)",
            "value": 8746,
            "unit": "instructions per request"
          },
          {
            "name": "http: /work request (jit)",
            "value": 608958,
            "unit": "instructions per request"
          },
          {
            "name": "http: /work request (interp)",
            "value": 725448,
            "unit": "instructions per request"
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
          "id": "e86e3291d7c39862f19fc96618300a02c89c3cb1",
          "message": "Merge pull request #89 from ArthurHeymans/task-slots\n\nGive finished tasks' and closed channels' slots back",
          "timestamp": "2026-10-11T02:26:02+02:00",
          "tree_id": "dc05f7cfc4ce19a3185fca578f042b163949c29f",
          "url": "https://github.com/ArthurHeymans/techne/commit/e86e3291d7c39862f19fc96618300a02c89c3cb1"
        },
        "date": 1791678612927,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "startup (jit)",
            "value": 15663205,
            "unit": "instructions"
          },
          {
            "name": "startup (interp)",
            "value": 15492312,
            "unit": "instructions"
          },
          {
            "name": "fib (jit)",
            "value": 780441301,
            "unit": "instructions"
          },
          {
            "name": "fib (interp)",
            "value": 1747140022,
            "unit": "instructions"
          },
          {
            "name": "tak (jit)",
            "value": 987712255,
            "unit": "instructions"
          },
          {
            "name": "tak (interp)",
            "value": 2190596883,
            "unit": "instructions"
          },
          {
            "name": "nqueens (jit)",
            "value": 1153638834,
            "unit": "instructions"
          },
          {
            "name": "nqueens (interp)",
            "value": 2870530051,
            "unit": "instructions"
          },
          {
            "name": "nqueens GC pause",
            "value": 345,
            "unit": "words"
          },
          {
            "name": "bintrees (jit)",
            "value": 3630941708,
            "unit": "instructions"
          },
          {
            "name": "bintrees (interp)",
            "value": 9180447254,
            "unit": "instructions"
          },
          {
            "name": "bintrees GC pause",
            "value": 393138,
            "unit": "words"
          },
          {
            "name": "hof (jit)",
            "value": 1621413941,
            "unit": "instructions"
          },
          {
            "name": "hof (interp)",
            "value": 3148843937,
            "unit": "instructions"
          },
          {
            "name": "hof GC pause",
            "value": 1579648,
            "unit": "words"
          },
          {
            "name": "qsort (jit)",
            "value": 1426679570,
            "unit": "instructions"
          },
          {
            "name": "qsort (interp)",
            "value": 3821313661,
            "unit": "instructions"
          },
          {
            "name": "mandel (jit)",
            "value": 1034639114,
            "unit": "instructions"
          },
          {
            "name": "mandel (interp)",
            "value": 2611329167,
            "unit": "instructions"
          },
          {
            "name": "hash (jit)",
            "value": 658160584,
            "unit": "instructions"
          },
          {
            "name": "hash (interp)",
            "value": 743215778,
            "unit": "instructions"
          },
          {
            "name": "hash GC pause",
            "value": 229395,
            "unit": "words"
          },
          {
            "name": "orgparse (jit)",
            "value": 1136629263,
            "unit": "instructions"
          },
          {
            "name": "orgparse (interp)",
            "value": 1440629547,
            "unit": "instructions"
          },
          {
            "name": "callbacks (jit)",
            "value": 634342204,
            "unit": "instructions"
          },
          {
            "name": "callbacks (interp)",
            "value": 1573935891,
            "unit": "instructions"
          },
          {
            "name": "callbacks GC pause",
            "value": 327538,
            "unit": "words"
          },
          {
            "name": "numstr (jit)",
            "value": 627727038,
            "unit": "instructions"
          },
          {
            "name": "numstr (interp)",
            "value": 714002803,
            "unit": "instructions"
          },
          {
            "name": "tasks (jit)",
            "value": 1324615755,
            "unit": "instructions"
          },
          {
            "name": "tasks (interp)",
            "value": 1228492138,
            "unit": "instructions"
          },
          {
            "name": "editor startup (jit)",
            "value": 137935465,
            "unit": "instructions"
          },
          {
            "name": "editor startup (interp)",
            "value": 135292957,
            "unit": "instructions"
          },
          {
            "name": "keys: type (jit)",
            "value": 1287677,
            "unit": "instructions per key"
          },
          {
            "name": "keys: type-long-line (jit)",
            "value": 1289318,
            "unit": "instructions per key"
          },
          {
            "name": "keys: move (jit)",
            "value": 1182898,
            "unit": "instructions per key"
          },
          {
            "name": "keys: move-modal (jit)",
            "value": 1048664,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-open (jit)",
            "value": 19193457,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-first-key (jit)",
            "value": 22040993,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-narrow (jit)",
            "value": 13763709,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-delete (jit)",
            "value": 1429608,
            "unit": "instructions per key"
          },
          {
            "name": "keys: command-first-key (jit)",
            "value": 1550033,
            "unit": "instructions per key"
          },
          {
            "name": "http: plaintext request (jit)",
            "value": 8448,
            "unit": "instructions per request"
          },
          {
            "name": "http: plaintext request (interp)",
            "value": 8699,
            "unit": "instructions per request"
          },
          {
            "name": "http: /work request (jit)",
            "value": 608957,
            "unit": "instructions per request"
          },
          {
            "name": "http: /work request (interp)",
            "value": 710392,
            "unit": "instructions per request"
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
          "id": "5a8eb8406fc907106aa1f9d67e321b4e0b540eea",
          "message": "Merge pull request #90 from ArthurHeymans/ready-queue\n\nKeep idle tasks from slowing down busy ones",
          "timestamp": "2026-10-11T02:26:19+02:00",
          "tree_id": "591265f51a40541bd4bb305413776cc65529dec0",
          "url": "https://github.com/ArthurHeymans/techne/commit/5a8eb8406fc907106aa1f9d67e321b4e0b540eea"
        },
        "date": 1791678933779,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "startup (jit)",
            "value": 15680496,
            "unit": "instructions"
          },
          {
            "name": "startup (interp)",
            "value": 15512566,
            "unit": "instructions"
          },
          {
            "name": "fib (jit)",
            "value": 780450367,
            "unit": "instructions"
          },
          {
            "name": "fib (interp)",
            "value": 1747160373,
            "unit": "instructions"
          },
          {
            "name": "tak (jit)",
            "value": 987730023,
            "unit": "instructions"
          },
          {
            "name": "tak (interp)",
            "value": 2190617270,
            "unit": "instructions"
          },
          {
            "name": "nqueens (jit)",
            "value": 1153656227,
            "unit": "instructions"
          },
          {
            "name": "nqueens (interp)",
            "value": 2870550554,
            "unit": "instructions"
          },
          {
            "name": "nqueens GC pause",
            "value": 345,
            "unit": "words"
          },
          {
            "name": "bintrees (jit)",
            "value": 3630937246,
            "unit": "instructions"
          },
          {
            "name": "bintrees (interp)",
            "value": 9180467744,
            "unit": "instructions"
          },
          {
            "name": "bintrees GC pause",
            "value": 393138,
            "unit": "words"
          },
          {
            "name": "hof (jit)",
            "value": 1621444952,
            "unit": "instructions"
          },
          {
            "name": "hof (interp)",
            "value": 3148858666,
            "unit": "instructions"
          },
          {
            "name": "hof GC pause",
            "value": 1579648,
            "unit": "words"
          },
          {
            "name": "qsort (jit)",
            "value": 1426688367,
            "unit": "instructions"
          },
          {
            "name": "qsort (interp)",
            "value": 3821329914,
            "unit": "instructions"
          },
          {
            "name": "mandel (jit)",
            "value": 1034609617,
            "unit": "instructions"
          },
          {
            "name": "mandel (interp)",
            "value": 2611349684,
            "unit": "instructions"
          },
          {
            "name": "hash (jit)",
            "value": 658180449,
            "unit": "instructions"
          },
          {
            "name": "hash (interp)",
            "value": 743231914,
            "unit": "instructions"
          },
          {
            "name": "hash GC pause",
            "value": 229395,
            "unit": "words"
          },
          {
            "name": "orgparse (jit)",
            "value": 1136619778,
            "unit": "instructions"
          },
          {
            "name": "orgparse (interp)",
            "value": 1440650410,
            "unit": "instructions"
          },
          {
            "name": "callbacks (jit)",
            "value": 634293521,
            "unit": "instructions"
          },
          {
            "name": "callbacks (interp)",
            "value": 1573954027,
            "unit": "instructions"
          },
          {
            "name": "callbacks GC pause",
            "value": 327538,
            "unit": "words"
          },
          {
            "name": "numstr (jit)",
            "value": 627743806,
            "unit": "instructions"
          },
          {
            "name": "numstr (interp)",
            "value": 714018693,
            "unit": "instructions"
          },
          {
            "name": "tasks (jit)",
            "value": 380537641,
            "unit": "instructions"
          },
          {
            "name": "tasks (interp)",
            "value": 284994292,
            "unit": "instructions"
          },
          {
            "name": "editor startup (jit)",
            "value": 138054236,
            "unit": "instructions"
          },
          {
            "name": "editor startup (interp)",
            "value": 135448035,
            "unit": "instructions"
          },
          {
            "name": "keys: type (jit)",
            "value": 1288101,
            "unit": "instructions per key"
          },
          {
            "name": "keys: type-long-line (jit)",
            "value": 1289639,
            "unit": "instructions per key"
          },
          {
            "name": "keys: move (jit)",
            "value": 1183765,
            "unit": "instructions per key"
          },
          {
            "name": "keys: move-modal (jit)",
            "value": 1049007,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-open (jit)",
            "value": 19193957,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-first-key (jit)",
            "value": 22041569,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-narrow (jit)",
            "value": 13760979,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-delete (jit)",
            "value": 1430863,
            "unit": "instructions per key"
          },
          {
            "name": "keys: command-first-key (jit)",
            "value": 1549805,
            "unit": "instructions per key"
          },
          {
            "name": "http: plaintext request (jit)",
            "value": 7252,
            "unit": "instructions per request"
          },
          {
            "name": "http: plaintext request (interp)",
            "value": 7493,
            "unit": "instructions per request"
          },
          {
            "name": "http: /work request (jit)",
            "value": 607728,
            "unit": "instructions per request"
          },
          {
            "name": "http: /work request (interp)",
            "value": 709159,
            "unit": "instructions per request"
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
          "id": "ed62e3ad671f5b855d0507138a0da3677a1da0d9",
          "message": "Merge pull request #91 from ArthurHeymans/native-calls\n\nCall natives from compiled code without the generic path",
          "timestamp": "2026-10-11T02:31:02+02:00",
          "tree_id": "2c5cc02553f79745bd0a9bd85fbff535a145325a",
          "url": "https://github.com/ArthurHeymans/techne/commit/ed62e3ad671f5b855d0507138a0da3677a1da0d9"
        },
        "date": 1791679296100,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "startup (jit)",
            "value": 15730476,
            "unit": "instructions"
          },
          {
            "name": "startup (interp)",
            "value": 15581570,
            "unit": "instructions"
          },
          {
            "name": "fib (jit)",
            "value": 780555409,
            "unit": "instructions"
          },
          {
            "name": "fib (interp)",
            "value": 1747225072,
            "unit": "instructions"
          },
          {
            "name": "tak (jit)",
            "value": 987858886,
            "unit": "instructions"
          },
          {
            "name": "tak (interp)",
            "value": 2190682334,
            "unit": "instructions"
          },
          {
            "name": "nqueens (jit)",
            "value": 1128842765,
            "unit": "instructions"
          },
          {
            "name": "nqueens (interp)",
            "value": 2868312932,
            "unit": "instructions"
          },
          {
            "name": "nqueens GC pause",
            "value": 345,
            "unit": "words"
          },
          {
            "name": "bintrees (jit)",
            "value": 3631816046,
            "unit": "instructions"
          },
          {
            "name": "bintrees (interp)",
            "value": 9180535719,
            "unit": "instructions"
          },
          {
            "name": "bintrees GC pause",
            "value": 393138,
            "unit": "words"
          },
          {
            "name": "hof (jit)",
            "value": 1614744030,
            "unit": "instructions"
          },
          {
            "name": "hof (interp)",
            "value": 3144728536,
            "unit": "instructions"
          },
          {
            "name": "hof GC pause",
            "value": 1579648,
            "unit": "words"
          },
          {
            "name": "qsort (jit)",
            "value": 1426859488,
            "unit": "instructions"
          },
          {
            "name": "qsort (interp)",
            "value": 3821401605,
            "unit": "instructions"
          },
          {
            "name": "mandel (jit)",
            "value": 1016687861,
            "unit": "instructions"
          },
          {
            "name": "mandel (interp)",
            "value": 2609670087,
            "unit": "instructions"
          },
          {
            "name": "hash (jit)",
            "value": 567084033,
            "unit": "instructions"
          },
          {
            "name": "hash (interp)",
            "value": 734901633,
            "unit": "instructions"
          },
          {
            "name": "hash GC pause",
            "value": 229395,
            "unit": "words"
          },
          {
            "name": "orgparse (jit)",
            "value": 399655354,
            "unit": "instructions"
          },
          {
            "name": "orgparse (interp)",
            "value": 1411557562,
            "unit": "instructions"
          },
          {
            "name": "callbacks (jit)",
            "value": 639295366,
            "unit": "instructions"
          },
          {
            "name": "callbacks (interp)",
            "value": 1573748763,
            "unit": "instructions"
          },
          {
            "name": "callbacks GC pause",
            "value": 327538,
            "unit": "words"
          },
          {
            "name": "numstr (jit)",
            "value": 390649906,
            "unit": "instructions"
          },
          {
            "name": "numstr (interp)",
            "value": 700083882,
            "unit": "instructions"
          },
          {
            "name": "tasks (jit)",
            "value": 376856718,
            "unit": "instructions"
          },
          {
            "name": "tasks (interp)",
            "value": 282964492,
            "unit": "instructions"
          },
          {
            "name": "editor startup (jit)",
            "value": 138618060,
            "unit": "instructions"
          },
          {
            "name": "editor startup (interp)",
            "value": 135907426,
            "unit": "instructions"
          },
          {
            "name": "keys: type (jit)",
            "value": 1208108,
            "unit": "instructions per key"
          },
          {
            "name": "keys: type-long-line (jit)",
            "value": 1207710,
            "unit": "instructions per key"
          },
          {
            "name": "keys: move (jit)",
            "value": 1128416,
            "unit": "instructions per key"
          },
          {
            "name": "keys: move-modal (jit)",
            "value": 1005440,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-open (jit)",
            "value": 19160923,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-first-key (jit)",
            "value": 22015720,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-narrow (jit)",
            "value": 13741396,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-delete (jit)",
            "value": 1412884,
            "unit": "instructions per key"
          },
          {
            "name": "keys: command-first-key (jit)",
            "value": 1526338,
            "unit": "instructions per key"
          },
          {
            "name": "http: plaintext request (jit)",
            "value": 6688,
            "unit": "instructions per request"
          },
          {
            "name": "http: plaintext request (interp)",
            "value": 7451,
            "unit": "instructions per request"
          },
          {
            "name": "http: /work request (jit)",
            "value": 368797,
            "unit": "instructions per request"
          },
          {
            "name": "http: /work request (interp)",
            "value": 695090,
            "unit": "instructions per request"
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
          "id": "dba00bab4496cc4c2ceecdb7cd82a6f70686fb1d",
          "message": "Merge pull request #92 from ArthurHeymans/keys-no-deadline\n\ntechne-editor: count keystrokes without the parts' deadline",
          "timestamp": "2026-10-11T02:44:25+02:00",
          "tree_id": "d0f50de1b67f0c7e81826b9ad3314bf9818992a7",
          "url": "https://github.com/ArthurHeymans/techne/commit/dba00bab4496cc4c2ceecdb7cd82a6f70686fb1d"
        },
        "date": 1791679724386,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "startup (jit)",
            "value": 15730476,
            "unit": "instructions"
          },
          {
            "name": "startup (interp)",
            "value": 15577069,
            "unit": "instructions"
          },
          {
            "name": "fib (jit)",
            "value": 780557421,
            "unit": "instructions"
          },
          {
            "name": "fib (interp)",
            "value": 1747220571,
            "unit": "instructions"
          },
          {
            "name": "tak (jit)",
            "value": 987856425,
            "unit": "instructions"
          },
          {
            "name": "tak (interp)",
            "value": 2190677833,
            "unit": "instructions"
          },
          {
            "name": "nqueens (jit)",
            "value": 1128847293,
            "unit": "instructions"
          },
          {
            "name": "nqueens (interp)",
            "value": 2868312932,
            "unit": "instructions"
          },
          {
            "name": "nqueens GC pause",
            "value": 345,
            "unit": "words"
          },
          {
            "name": "bintrees (jit)",
            "value": 3631829902,
            "unit": "instructions"
          },
          {
            "name": "bintrees (interp)",
            "value": 9180535719,
            "unit": "instructions"
          },
          {
            "name": "bintrees GC pause",
            "value": 393138,
            "unit": "words"
          },
          {
            "name": "hof (jit)",
            "value": 1614744740,
            "unit": "instructions"
          },
          {
            "name": "hof (interp)",
            "value": 3144724035,
            "unit": "instructions"
          },
          {
            "name": "hof GC pause",
            "value": 1579648,
            "unit": "words"
          },
          {
            "name": "qsort (jit)",
            "value": 1426867730,
            "unit": "instructions"
          },
          {
            "name": "qsort (interp)",
            "value": 3821401605,
            "unit": "instructions"
          },
          {
            "name": "mandel (jit)",
            "value": 1016661074,
            "unit": "instructions"
          },
          {
            "name": "mandel (interp)",
            "value": 2609665586,
            "unit": "instructions"
          },
          {
            "name": "hash (jit)",
            "value": 567093603,
            "unit": "instructions"
          },
          {
            "name": "hash (interp)",
            "value": 734901633,
            "unit": "instructions"
          },
          {
            "name": "hash GC pause",
            "value": 229395,
            "unit": "words"
          },
          {
            "name": "orgparse (jit)",
            "value": 399659629,
            "unit": "instructions"
          },
          {
            "name": "orgparse (interp)",
            "value": 1411557562,
            "unit": "instructions"
          },
          {
            "name": "callbacks (jit)",
            "value": 639303569,
            "unit": "instructions"
          },
          {
            "name": "callbacks (interp)",
            "value": 1573744262,
            "unit": "instructions"
          },
          {
            "name": "callbacks GC pause",
            "value": 327538,
            "unit": "words"
          },
          {
            "name": "numstr (jit)",
            "value": 390648296,
            "unit": "instructions"
          },
          {
            "name": "numstr (interp)",
            "value": 700083882,
            "unit": "instructions"
          },
          {
            "name": "tasks (jit)",
            "value": 376817876,
            "unit": "instructions"
          },
          {
            "name": "tasks (interp)",
            "value": 282959991,
            "unit": "instructions"
          },
          {
            "name": "editor startup (jit)",
            "value": 138617929,
            "unit": "instructions"
          },
          {
            "name": "editor startup (interp)",
            "value": 135907426,
            "unit": "instructions"
          },
          {
            "name": "keys: type (jit)",
            "value": 1208015,
            "unit": "instructions per key"
          },
          {
            "name": "keys: type-long-line (jit)",
            "value": 1207649,
            "unit": "instructions per key"
          },
          {
            "name": "keys: move (jit)",
            "value": 1128397,
            "unit": "instructions per key"
          },
          {
            "name": "keys: move-modal (jit)",
            "value": 1005367,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-open (jit)",
            "value": 19160887,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-first-key (jit)",
            "value": 22015720,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-narrow (jit)",
            "value": 13741396,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-delete (jit)",
            "value": 1412884,
            "unit": "instructions per key"
          },
          {
            "name": "keys: command-first-key (jit)",
            "value": 1525056,
            "unit": "instructions per key"
          },
          {
            "name": "http: plaintext request (jit)",
            "value": 6691,
            "unit": "instructions per request"
          },
          {
            "name": "http: plaintext request (interp)",
            "value": 7451,
            "unit": "instructions per request"
          },
          {
            "name": "http: /work request (jit)",
            "value": 368796,
            "unit": "instructions per request"
          },
          {
            "name": "http: /work request (interp)",
            "value": 695090,
            "unit": "instructions per request"
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
          "id": "308baabfbe6853df6fd4925144da2ab0495c8800",
          "message": "Merge pull request #94 from ArthurHeymans/join-error-kind\n\nKeep an error's kind for whoever joins its task",
          "timestamp": "2026-10-11T02:46:33+02:00",
          "tree_id": "baa48ab7dfab8d34558e4e875f2b768a96c33a6a",
          "url": "https://github.com/ArthurHeymans/techne/commit/308baabfbe6853df6fd4925144da2ab0495c8800"
        },
        "date": 1791680083772,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "startup (jit)",
            "value": 15730476,
            "unit": "instructions"
          },
          {
            "name": "startup (interp)",
            "value": 15577069,
            "unit": "instructions"
          },
          {
            "name": "fib (jit)",
            "value": 780555342,
            "unit": "instructions"
          },
          {
            "name": "fib (interp)",
            "value": 1747220571,
            "unit": "instructions"
          },
          {
            "name": "tak (jit)",
            "value": 987847609,
            "unit": "instructions"
          },
          {
            "name": "tak (interp)",
            "value": 2190682334,
            "unit": "instructions"
          },
          {
            "name": "nqueens (jit)",
            "value": 1128839397,
            "unit": "instructions"
          },
          {
            "name": "nqueens (interp)",
            "value": 2868308431,
            "unit": "instructions"
          },
          {
            "name": "nqueens GC pause",
            "value": 345,
            "unit": "words"
          },
          {
            "name": "bintrees (jit)",
            "value": 3631797986,
            "unit": "instructions"
          },
          {
            "name": "bintrees (interp)",
            "value": 9180531218,
            "unit": "instructions"
          },
          {
            "name": "bintrees GC pause",
            "value": 393138,
            "unit": "words"
          },
          {
            "name": "hof (jit)",
            "value": 1614754056,
            "unit": "instructions"
          },
          {
            "name": "hof (interp)",
            "value": 3144728536,
            "unit": "instructions"
          },
          {
            "name": "hof GC pause",
            "value": 1579648,
            "unit": "words"
          },
          {
            "name": "qsort (jit)",
            "value": 1426846931,
            "unit": "instructions"
          },
          {
            "name": "qsort (interp)",
            "value": 3821401605,
            "unit": "instructions"
          },
          {
            "name": "mandel (jit)",
            "value": 1016715805,
            "unit": "instructions"
          },
          {
            "name": "mandel (interp)",
            "value": 2609665586,
            "unit": "instructions"
          },
          {
            "name": "hash (jit)",
            "value": 567087510,
            "unit": "instructions"
          },
          {
            "name": "hash (interp)",
            "value": 734901633,
            "unit": "instructions"
          },
          {
            "name": "hash GC pause",
            "value": 229395,
            "unit": "words"
          },
          {
            "name": "orgparse (jit)",
            "value": 399649194,
            "unit": "instructions"
          },
          {
            "name": "orgparse (interp)",
            "value": 1411557562,
            "unit": "instructions"
          },
          {
            "name": "callbacks (jit)",
            "value": 639276923,
            "unit": "instructions"
          },
          {
            "name": "callbacks (interp)",
            "value": 1573744262,
            "unit": "instructions"
          },
          {
            "name": "callbacks GC pause",
            "value": 327538,
            "unit": "words"
          },
          {
            "name": "numstr (jit)",
            "value": 390650478,
            "unit": "instructions"
          },
          {
            "name": "numstr (interp)",
            "value": 700083882,
            "unit": "instructions"
          },
          {
            "name": "tasks (jit)",
            "value": 376802672,
            "unit": "instructions"
          },
          {
            "name": "tasks (interp)",
            "value": 282957489,
            "unit": "instructions"
          },
          {
            "name": "editor startup (jit)",
            "value": 138622310,
            "unit": "instructions"
          },
          {
            "name": "editor startup (interp)",
            "value": 135911941,
            "unit": "instructions"
          },
          {
            "name": "keys: type (jit)",
            "value": 1207960,
            "unit": "instructions per key"
          },
          {
            "name": "keys: type-long-line (jit)",
            "value": 1207716,
            "unit": "instructions per key"
          },
          {
            "name": "keys: move (jit)",
            "value": 1128392,
            "unit": "instructions per key"
          },
          {
            "name": "keys: move-modal (jit)",
            "value": 1005386,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-open (jit)",
            "value": 19160887,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-first-key (jit)",
            "value": 22015720,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-narrow (jit)",
            "value": 13741396,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-delete (jit)",
            "value": 1412884,
            "unit": "instructions per key"
          },
          {
            "name": "keys: command-first-key (jit)",
            "value": 1524960,
            "unit": "instructions per key"
          },
          {
            "name": "http: plaintext request (jit)",
            "value": 6686,
            "unit": "instructions per request"
          },
          {
            "name": "http: plaintext request (interp)",
            "value": 7451,
            "unit": "instructions per request"
          },
          {
            "name": "http: /work request (jit)",
            "value": 368795,
            "unit": "instructions per request"
          },
          {
            "name": "http: /work request (interp)",
            "value": 695090,
            "unit": "instructions per request"
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
          "id": "3496f88c6a64e05efcec35fd2d62884b35c539ed",
          "message": "Merge pull request #95 from ArthurHeymans/jit-native-protocol\n\nMake compiled calls of Rust natives cheaper",
          "timestamp": "2026-10-11T06:24:13+02:00",
          "tree_id": "43fab0eb13894648d53b32f1acc3d6685e847cd1",
          "url": "https://github.com/ArthurHeymans/techne/commit/3496f88c6a64e05efcec35fd2d62884b35c539ed"
        },
        "date": 1791692843329,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "startup (jit)",
            "value": 15749552,
            "unit": "instructions"
          },
          {
            "name": "startup (interp)",
            "value": 15581807,
            "unit": "instructions"
          },
          {
            "name": "fib (jit)",
            "value": 780559436,
            "unit": "instructions"
          },
          {
            "name": "fib (interp)",
            "value": 1747225309,
            "unit": "instructions"
          },
          {
            "name": "tak (jit)",
            "value": 987855308,
            "unit": "instructions"
          },
          {
            "name": "tak (interp)",
            "value": 2190682571,
            "unit": "instructions"
          },
          {
            "name": "nqueens (jit)",
            "value": 1116085148,
            "unit": "instructions"
          },
          {
            "name": "nqueens (interp)",
            "value": 2868308668,
            "unit": "instructions"
          },
          {
            "name": "nqueens GC pause",
            "value": 345,
            "unit": "words"
          },
          {
            "name": "bintrees (jit)",
            "value": 3632446165,
            "unit": "instructions"
          },
          {
            "name": "bintrees (interp)",
            "value": 9180535956,
            "unit": "instructions"
          },
          {
            "name": "bintrees GC pause",
            "value": 393138,
            "unit": "words"
          },
          {
            "name": "hof (jit)",
            "value": 1614862698,
            "unit": "instructions"
          },
          {
            "name": "hof (interp)",
            "value": 3144724272,
            "unit": "instructions"
          },
          {
            "name": "hof GC pause",
            "value": 1579648,
            "unit": "words"
          },
          {
            "name": "qsort (jit)",
            "value": 1426858226,
            "unit": "instructions"
          },
          {
            "name": "qsort (interp)",
            "value": 3821401849,
            "unit": "instructions"
          },
          {
            "name": "mandel (jit)",
            "value": 1010526007,
            "unit": "instructions"
          },
          {
            "name": "mandel (interp)",
            "value": 2609670324,
            "unit": "instructions"
          },
          {
            "name": "hash (jit)",
            "value": 532112211,
            "unit": "instructions"
          },
          {
            "name": "hash (interp)",
            "value": 734897369,
            "unit": "instructions"
          },
          {
            "name": "hash GC pause",
            "value": 229395,
            "unit": "words"
          },
          {
            "name": "orgparse (jit)",
            "value": 402169302,
            "unit": "instructions"
          },
          {
            "name": "orgparse (interp)",
            "value": 1411557799,
            "unit": "instructions"
          },
          {
            "name": "callbacks (jit)",
            "value": 641417053,
            "unit": "instructions"
          },
          {
            "name": "callbacks (interp)",
            "value": 1573749000,
            "unit": "instructions"
          },
          {
            "name": "callbacks GC pause",
            "value": 327538,
            "unit": "words"
          },
          {
            "name": "numstr (jit)",
            "value": 361033133,
            "unit": "instructions"
          },
          {
            "name": "numstr (interp)",
            "value": 700088620,
            "unit": "instructions"
          },
          {
            "name": "tasks (jit)",
            "value": 375775215,
            "unit": "instructions"
          },
          {
            "name": "tasks (interp)",
            "value": 282871901,
            "unit": "instructions"
          },
          {
            "name": "editor startup (jit)",
            "value": 138649619,
            "unit": "instructions"
          },
          {
            "name": "editor startup (interp)",
            "value": 135932209,
            "unit": "instructions"
          },
          {
            "name": "keys: type (jit)",
            "value": 1178878,
            "unit": "instructions per key"
          },
          {
            "name": "keys: type-long-line (jit)",
            "value": 1177719,
            "unit": "instructions per key"
          },
          {
            "name": "keys: move (jit)",
            "value": 1110640,
            "unit": "instructions per key"
          },
          {
            "name": "keys: move-modal (jit)",
            "value": 992276,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-open (jit)",
            "value": 19159604,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-first-key (jit)",
            "value": 22015308,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-narrow (jit)",
            "value": 13740548,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-delete (jit)",
            "value": 1407674,
            "unit": "instructions per key"
          },
          {
            "name": "keys: command-first-key (jit)",
            "value": 1524490,
            "unit": "instructions per key"
          },
          {
            "name": "http: plaintext request (jit)",
            "value": 6533,
            "unit": "instructions per request"
          },
          {
            "name": "http: plaintext request (interp)",
            "value": 7452,
            "unit": "instructions per request"
          },
          {
            "name": "http: /work request (jit)",
            "value": 338547,
            "unit": "instructions per request"
          },
          {
            "name": "http: /work request (interp)",
            "value": 695090,
            "unit": "instructions per request"
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
          "id": "3e20f184661e5ebbc311f6104297563c33c5aa81",
          "message": "Merge pull request #96 from ArthurHeymans/jit-loop-fixnums\n\nRun fixnum loops in compiled code twice as fast",
          "timestamp": "2026-10-11T08:21:20+02:00",
          "tree_id": "97faa55099f3c5126d9887f12d9fa5732c84a137",
          "url": "https://github.com/ArthurHeymans/techne/commit/3e20f184661e5ebbc311f6104297563c33c5aa81"
        },
        "date": 1791699964678,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "startup (jit)",
            "value": 15683974,
            "unit": "instructions"
          },
          {
            "name": "startup (interp)",
            "value": 15526067,
            "unit": "instructions"
          },
          {
            "name": "fib (jit)",
            "value": 766461148,
            "unit": "instructions"
          },
          {
            "name": "fib (interp)",
            "value": 1747173898,
            "unit": "instructions"
          },
          {
            "name": "tak (jit)",
            "value": 947271484,
            "unit": "instructions"
          },
          {
            "name": "tak (interp)",
            "value": 2190630721,
            "unit": "instructions"
          },
          {
            "name": "nqueens (jit)",
            "value": 1043341513,
            "unit": "instructions"
          },
          {
            "name": "nqueens (interp)",
            "value": 2868256053,
            "unit": "instructions"
          },
          {
            "name": "nqueens GC pause",
            "value": 345,
            "unit": "words"
          },
          {
            "name": "bintrees (jit)",
            "value": 3610272409,
            "unit": "instructions"
          },
          {
            "name": "bintrees (interp)",
            "value": 9180483958,
            "unit": "instructions"
          },
          {
            "name": "bintrees GC pause",
            "value": 393138,
            "unit": "words"
          },
          {
            "name": "hof (jit)",
            "value": 1584338753,
            "unit": "instructions"
          },
          {
            "name": "hof (interp)",
            "value": 3144675660,
            "unit": "instructions"
          },
          {
            "name": "hof GC pause",
            "value": 1579648,
            "unit": "words"
          },
          {
            "name": "qsort (jit)",
            "value": 1362113048,
            "unit": "instructions"
          },
          {
            "name": "qsort (interp)",
            "value": 3821345013,
            "unit": "instructions"
          },
          {
            "name": "mandel (jit)",
            "value": 930456927,
            "unit": "instructions"
          },
          {
            "name": "mandel (interp)",
            "value": 2609613644,
            "unit": "instructions"
          },
          {
            "name": "hash (jit)",
            "value": 525600764,
            "unit": "instructions"
          },
          {
            "name": "hash (interp)",
            "value": 734845718,
            "unit": "instructions"
          },
          {
            "name": "hash GC pause",
            "value": 229395,
            "unit": "words"
          },
          {
            "name": "orgparse (jit)",
            "value": 384320300,
            "unit": "instructions"
          },
          {
            "name": "orgparse (interp)",
            "value": 1411504703,
            "unit": "instructions"
          },
          {
            "name": "callbacks (jit)",
            "value": 644332360,
            "unit": "instructions"
          },
          {
            "name": "callbacks (interp)",
            "value": 1573690252,
            "unit": "instructions"
          },
          {
            "name": "callbacks GC pause",
            "value": 327538,
            "unit": "words"
          },
          {
            "name": "numstr (jit)",
            "value": 335366915,
            "unit": "instructions"
          },
          {
            "name": "numstr (interp)",
            "value": 700032425,
            "unit": "instructions"
          },
          {
            "name": "tasks (jit)",
            "value": 385446668,
            "unit": "instructions"
          },
          {
            "name": "tasks (interp)",
            "value": 283051006,
            "unit": "instructions"
          },
          {
            "name": "editor startup (jit)",
            "value": 138761802,
            "unit": "instructions"
          },
          {
            "name": "editor startup (interp)",
            "value": 135993333,
            "unit": "instructions"
          },
          {
            "name": "keys: type (jit)",
            "value": 1176667,
            "unit": "instructions per key"
          },
          {
            "name": "keys: type-long-line (jit)",
            "value": 1178202,
            "unit": "instructions per key"
          },
          {
            "name": "keys: move (jit)",
            "value": 1104684,
            "unit": "instructions per key"
          },
          {
            "name": "keys: move-modal (jit)",
            "value": 985447,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-open (jit)",
            "value": 19150713,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-first-key (jit)",
            "value": 22010283,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-narrow (jit)",
            "value": 13733398,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-delete (jit)",
            "value": 1400586,
            "unit": "instructions per key"
          },
          {
            "name": "keys: command-first-key (jit)",
            "value": 1516616,
            "unit": "instructions per key"
          },
          {
            "name": "http: plaintext request (jit)",
            "value": 6545,
            "unit": "instructions per request"
          },
          {
            "name": "http: plaintext request (interp)",
            "value": 7454,
            "unit": "instructions per request"
          },
          {
            "name": "http: /work request (jit)",
            "value": 312576,
            "unit": "instructions per request"
          },
          {
            "name": "http: /work request (interp)",
            "value": 695092,
            "unit": "instructions per request"
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
          "id": "4d56eefbdef86136be81974399bf8d3ba6bdc471",
          "message": "Merge pull request #97 from ArthurHeymans/fast-digits\n\nTurn fixnums into strings faster",
          "timestamp": "2026-10-11T08:23:31+02:00",
          "tree_id": "3f103e518de8ee68d194466de23d8a4c4ad4cd7c",
          "url": "https://github.com/ArthurHeymans/techne/commit/4d56eefbdef86136be81974399bf8d3ba6bdc471"
        },
        "date": 1791700273086,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "startup (jit)",
            "value": 15684172,
            "unit": "instructions"
          },
          {
            "name": "startup (interp)",
            "value": 15526081,
            "unit": "instructions"
          },
          {
            "name": "fib (jit)",
            "value": 766461221,
            "unit": "instructions"
          },
          {
            "name": "fib (interp)",
            "value": 1747169411,
            "unit": "instructions"
          },
          {
            "name": "tak (jit)",
            "value": 947269215,
            "unit": "instructions"
          },
          {
            "name": "tak (interp)",
            "value": 2190626234,
            "unit": "instructions"
          },
          {
            "name": "nqueens (jit)",
            "value": 1043318434,
            "unit": "instructions"
          },
          {
            "name": "nqueens (interp)",
            "value": 2868256067,
            "unit": "instructions"
          },
          {
            "name": "nqueens GC pause",
            "value": 345,
            "unit": "words"
          },
          {
            "name": "bintrees (jit)",
            "value": 3610283053,
            "unit": "instructions"
          },
          {
            "name": "bintrees (interp)",
            "value": 9180483958,
            "unit": "instructions"
          },
          {
            "name": "bintrees GC pause",
            "value": 393138,
            "unit": "words"
          },
          {
            "name": "hof (jit)",
            "value": 1584346146,
            "unit": "instructions"
          },
          {
            "name": "hof (interp)",
            "value": 3144671173,
            "unit": "instructions"
          },
          {
            "name": "hof GC pause",
            "value": 1579648,
            "unit": "words"
          },
          {
            "name": "qsort (jit)",
            "value": 1362112255,
            "unit": "instructions"
          },
          {
            "name": "qsort (interp)",
            "value": 3821349500,
            "unit": "instructions"
          },
          {
            "name": "mandel (jit)",
            "value": 930477839,
            "unit": "instructions"
          },
          {
            "name": "mandel (interp)",
            "value": 2609618145,
            "unit": "instructions"
          },
          {
            "name": "hash (jit)",
            "value": 499920548,
            "unit": "instructions"
          },
          {
            "name": "hash (interp)",
            "value": 709139884,
            "unit": "instructions"
          },
          {
            "name": "hash GC pause",
            "value": 229395,
            "unit": "words"
          },
          {
            "name": "orgparse (jit)",
            "value": 384237994,
            "unit": "instructions"
          },
          {
            "name": "orgparse (interp)",
            "value": 1411504703,
            "unit": "instructions"
          },
          {
            "name": "callbacks (jit)",
            "value": 644342085,
            "unit": "instructions"
          },
          {
            "name": "callbacks (interp)",
            "value": 1573690238,
            "unit": "instructions"
          },
          {
            "name": "callbacks GC pause",
            "value": 327538,
            "unit": "words"
          },
          {
            "name": "numstr (jit)",
            "value": 271481651,
            "unit": "instructions"
          },
          {
            "name": "numstr (interp)",
            "value": 636155424,
            "unit": "instructions"
          },
          {
            "name": "tasks (jit)",
            "value": 385443256,
            "unit": "instructions"
          },
          {
            "name": "tasks (interp)",
            "value": 283050992,
            "unit": "instructions"
          },
          {
            "name": "editor startup (jit)",
            "value": 138761277,
            "unit": "instructions"
          },
          {
            "name": "editor startup (interp)",
            "value": 135993419,
            "unit": "instructions"
          },
          {
            "name": "keys: type (jit)",
            "value": 1176569,
            "unit": "instructions per key"
          },
          {
            "name": "keys: type-long-line (jit)",
            "value": 1178114,
            "unit": "instructions per key"
          },
          {
            "name": "keys: move (jit)",
            "value": 1104641,
            "unit": "instructions per key"
          },
          {
            "name": "keys: move-modal (jit)",
            "value": 986846,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-open (jit)",
            "value": 19149811,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-first-key (jit)",
            "value": 22009959,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-narrow (jit)",
            "value": 13733144,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-delete (jit)",
            "value": 1400430,
            "unit": "instructions per key"
          },
          {
            "name": "keys: command-first-key (jit)",
            "value": 1514732,
            "unit": "instructions per key"
          },
          {
            "name": "http: plaintext request (jit)",
            "value": 6545,
            "unit": "instructions per request"
          },
          {
            "name": "http: plaintext request (interp)",
            "value": 7454,
            "unit": "instructions per request"
          },
          {
            "name": "http: /work request (jit)",
            "value": 248598,
            "unit": "instructions per request"
          },
          {
            "name": "http: /work request (interp)",
            "value": 631120,
            "unit": "instructions per request"
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
          "id": "9a19d0196a978a3e93c1e237a54f2ef6e3769eb2",
          "message": "Merge pull request #98 from ArthurHeymans/miri\n\nCheck the VM under Miri, fix what it found",
          "timestamp": "2026-10-11T10:22:16+02:00",
          "tree_id": "6f981ff27276bcc61c5cb2c58ecbd16b0fcf9f4c",
          "url": "https://github.com/ArthurHeymans/techne/commit/9a19d0196a978a3e93c1e237a54f2ef6e3769eb2"
        },
        "date": 1791707173914,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "startup (jit)",
            "value": 15717867,
            "unit": "instructions"
          },
          {
            "name": "startup (interp)",
            "value": 15544181,
            "unit": "instructions"
          },
          {
            "name": "fib (jit)",
            "value": 766476204,
            "unit": "instructions"
          },
          {
            "name": "fib (interp)",
            "value": 1747187537,
            "unit": "instructions"
          },
          {
            "name": "tak (jit)",
            "value": 947289695,
            "unit": "instructions"
          },
          {
            "name": "tak (interp)",
            "value": 2190644312,
            "unit": "instructions"
          },
          {
            "name": "nqueens (jit)",
            "value": 1043333058,
            "unit": "instructions"
          },
          {
            "name": "nqueens (interp)",
            "value": 2868274156,
            "unit": "instructions"
          },
          {
            "name": "nqueens GC pause",
            "value": 345,
            "unit": "words"
          },
          {
            "name": "bintrees (jit)",
            "value": 3610284904,
            "unit": "instructions"
          },
          {
            "name": "bintrees (interp)",
            "value": 9180493988,
            "unit": "instructions"
          },
          {
            "name": "bintrees GC pause",
            "value": 393138,
            "unit": "words"
          },
          {
            "name": "hof (jit)",
            "value": 1584338428,
            "unit": "instructions"
          },
          {
            "name": "hof (interp)",
            "value": 3144596739,
            "unit": "instructions"
          },
          {
            "name": "hof GC pause",
            "value": 1579648,
            "unit": "words"
          },
          {
            "name": "qsort (jit)",
            "value": 1362117101,
            "unit": "instructions"
          },
          {
            "name": "qsort (interp)",
            "value": 3821362191,
            "unit": "instructions"
          },
          {
            "name": "mandel (jit)",
            "value": 930524451,
            "unit": "instructions"
          },
          {
            "name": "mandel (interp)",
            "value": 2609627227,
            "unit": "instructions"
          },
          {
            "name": "hash (jit)",
            "value": 499934954,
            "unit": "instructions"
          },
          {
            "name": "hash (interp)",
            "value": 709153686,
            "unit": "instructions"
          },
          {
            "name": "hash GC pause",
            "value": 229395,
            "unit": "words"
          },
          {
            "name": "orgparse (jit)",
            "value": 384360642,
            "unit": "instructions"
          },
          {
            "name": "orgparse (interp)",
            "value": 1411518089,
            "unit": "instructions"
          },
          {
            "name": "callbacks (jit)",
            "value": 644377174,
            "unit": "instructions"
          },
          {
            "name": "callbacks (interp)",
            "value": 1573687820,
            "unit": "instructions"
          },
          {
            "name": "callbacks GC pause",
            "value": 327538,
            "unit": "words"
          },
          {
            "name": "numstr (jit)",
            "value": 271509864,
            "unit": "instructions"
          },
          {
            "name": "numstr (interp)",
            "value": 636168966,
            "unit": "instructions"
          },
          {
            "name": "tasks (jit)",
            "value": 385567083,
            "unit": "instructions"
          },
          {
            "name": "tasks (interp)",
            "value": 283076195,
            "unit": "instructions"
          },
          {
            "name": "editor startup (jit)",
            "value": 138819677,
            "unit": "instructions"
          },
          {
            "name": "editor startup (interp)",
            "value": 136016429,
            "unit": "instructions"
          },
          {
            "name": "keys: type (jit)",
            "value": 1176719,
            "unit": "instructions per key"
          },
          {
            "name": "keys: type-long-line (jit)",
            "value": 1175765,
            "unit": "instructions per key"
          },
          {
            "name": "keys: move (jit)",
            "value": 1104998,
            "unit": "instructions per key"
          },
          {
            "name": "keys: move-modal (jit)",
            "value": 986894,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-open (jit)",
            "value": 19150149,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-first-key (jit)",
            "value": 22010992,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-narrow (jit)",
            "value": 13731318,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-delete (jit)",
            "value": 1401663,
            "unit": "instructions per key"
          },
          {
            "name": "keys: command-first-key (jit)",
            "value": 1515209,
            "unit": "instructions per key"
          },
          {
            "name": "http: plaintext request (jit)",
            "value": 6547,
            "unit": "instructions per request"
          },
          {
            "name": "http: plaintext request (interp)",
            "value": 7454,
            "unit": "instructions per request"
          },
          {
            "name": "http: /work request (jit)",
            "value": 248596,
            "unit": "instructions per request"
          },
          {
            "name": "http: /work request (interp)",
            "value": 631121,
            "unit": "instructions per request"
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
          "id": "91908766e607071f9f6346934ec067082838c4cd",
          "message": "Merge pull request #99 from ArthurHeymans/coverage-timeout\n\nCI: give the nightly coverage run's suites more time",
          "timestamp": "2026-10-11T10:22:59+02:00",
          "tree_id": "a5164d19687019e502c4d4b2960fbe41ac90c372",
          "url": "https://github.com/ArthurHeymans/techne/commit/91908766e607071f9f6346934ec067082838c4cd"
        },
        "date": 1791707527267,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "startup (jit)",
            "value": 15698895,
            "unit": "instructions"
          },
          {
            "name": "startup (interp)",
            "value": 15544181,
            "unit": "instructions"
          },
          {
            "name": "fib (jit)",
            "value": 766478084,
            "unit": "instructions"
          },
          {
            "name": "fib (interp)",
            "value": 1747183036,
            "unit": "instructions"
          },
          {
            "name": "tak (jit)",
            "value": 947291154,
            "unit": "instructions"
          },
          {
            "name": "tak (interp)",
            "value": 2190644312,
            "unit": "instructions"
          },
          {
            "name": "nqueens (jit)",
            "value": 1043363044,
            "unit": "instructions"
          },
          {
            "name": "nqueens (interp)",
            "value": 2868269655,
            "unit": "instructions"
          },
          {
            "name": "nqueens GC pause",
            "value": 345,
            "unit": "words"
          },
          {
            "name": "bintrees (jit)",
            "value": 3610293427,
            "unit": "instructions"
          },
          {
            "name": "bintrees (interp)",
            "value": 9180498489,
            "unit": "instructions"
          },
          {
            "name": "bintrees GC pause",
            "value": 393138,
            "unit": "words"
          },
          {
            "name": "hof (jit)",
            "value": 1584353678,
            "unit": "instructions"
          },
          {
            "name": "hof (interp)",
            "value": 3144596739,
            "unit": "instructions"
          },
          {
            "name": "hof GC pause",
            "value": 1579648,
            "unit": "words"
          },
          {
            "name": "qsort (jit)",
            "value": 1362093857,
            "unit": "instructions"
          },
          {
            "name": "qsort (interp)",
            "value": 3821362191,
            "unit": "instructions"
          },
          {
            "name": "mandel (jit)",
            "value": 930463060,
            "unit": "instructions"
          },
          {
            "name": "mandel (interp)",
            "value": 2609631728,
            "unit": "instructions"
          },
          {
            "name": "hash (jit)",
            "value": 499914747,
            "unit": "instructions"
          },
          {
            "name": "hash (interp)",
            "value": 709153686,
            "unit": "instructions"
          },
          {
            "name": "hash GC pause",
            "value": 229395,
            "unit": "words"
          },
          {
            "name": "orgparse (jit)",
            "value": 384188159,
            "unit": "instructions"
          },
          {
            "name": "orgparse (interp)",
            "value": 1411518089,
            "unit": "instructions"
          },
          {
            "name": "callbacks (jit)",
            "value": 644407041,
            "unit": "instructions"
          },
          {
            "name": "callbacks (interp)",
            "value": 1573687820,
            "unit": "instructions"
          },
          {
            "name": "callbacks GC pause",
            "value": 327538,
            "unit": "words"
          },
          {
            "name": "numstr (jit)",
            "value": 271511883,
            "unit": "instructions"
          },
          {
            "name": "numstr (interp)",
            "value": 636168966,
            "unit": "instructions"
          },
          {
            "name": "tasks (jit)",
            "value": 385375964,
            "unit": "instructions"
          },
          {
            "name": "tasks (interp)",
            "value": 283076195,
            "unit": "instructions"
          },
          {
            "name": "editor startup (jit)",
            "value": 138820474,
            "unit": "instructions"
          },
          {
            "name": "editor startup (interp)",
            "value": 136016429,
            "unit": "instructions"
          },
          {
            "name": "keys: type (jit)",
            "value": 1176721,
            "unit": "instructions per key"
          },
          {
            "name": "keys: type-long-line (jit)",
            "value": 1175731,
            "unit": "instructions per key"
          },
          {
            "name": "keys: move (jit)",
            "value": 1104916,
            "unit": "instructions per key"
          },
          {
            "name": "keys: move-modal (jit)",
            "value": 986963,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-open (jit)",
            "value": 19150230,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-first-key (jit)",
            "value": 22011030,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-narrow (jit)",
            "value": 13734551,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-delete (jit)",
            "value": 1400458,
            "unit": "instructions per key"
          },
          {
            "name": "keys: command-first-key (jit)",
            "value": 1515811,
            "unit": "instructions per key"
          },
          {
            "name": "http: plaintext request (jit)",
            "value": 6545,
            "unit": "instructions per request"
          },
          {
            "name": "http: plaintext request (interp)",
            "value": 7454,
            "unit": "instructions per request"
          },
          {
            "name": "http: /work request (jit)",
            "value": 248596,
            "unit": "instructions per request"
          },
          {
            "name": "http: /work request (interp)",
            "value": 631121,
            "unit": "instructions per request"
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
          "id": "0a923855d2f521a3dbdf086cf7018648e361f937",
          "message": "Merge pull request #100 from ArthurHeymans/jit-fixnum-natives\n\nKeep the results of string-length and similar natives as known fixnums",
          "timestamp": "2026-10-11T12:40:20+02:00",
          "tree_id": "6e591a190ff30143cef6d2a7eed3cf9375d8b7bc",
          "url": "https://github.com/ArthurHeymans/techne/commit/0a923855d2f521a3dbdf086cf7018648e361f937"
        },
        "date": 1791715509653,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "startup (jit)",
            "value": 15714957,
            "unit": "instructions"
          },
          {
            "name": "startup (interp)",
            "value": 15541085,
            "unit": "instructions"
          },
          {
            "name": "fib (jit)",
            "value": 766473917,
            "unit": "instructions"
          },
          {
            "name": "fib (interp)",
            "value": 1747188923,
            "unit": "instructions"
          },
          {
            "name": "tak (jit)",
            "value": 947283444,
            "unit": "instructions"
          },
          {
            "name": "tak (interp)",
            "value": 2190641240,
            "unit": "instructions"
          },
          {
            "name": "nqueens (jit)",
            "value": 1043325653,
            "unit": "instructions"
          },
          {
            "name": "nqueens (interp)",
            "value": 2868275620,
            "unit": "instructions"
          },
          {
            "name": "nqueens GC pause",
            "value": 345,
            "unit": "words"
          },
          {
            "name": "bintrees (jit)",
            "value": 3602956415,
            "unit": "instructions"
          },
          {
            "name": "bintrees (interp)",
            "value": 9180499982,
            "unit": "instructions"
          },
          {
            "name": "bintrees GC pause",
            "value": 393138,
            "unit": "words"
          },
          {
            "name": "hof (jit)",
            "value": 1584345591,
            "unit": "instructions"
          },
          {
            "name": "hof (interp)",
            "value": 3144592109,
            "unit": "instructions"
          },
          {
            "name": "hof GC pause",
            "value": 1579648,
            "unit": "words"
          },
          {
            "name": "qsort (jit)",
            "value": 1362140225,
            "unit": "instructions"
          },
          {
            "name": "qsort (interp)",
            "value": 3821363615,
            "unit": "instructions"
          },
          {
            "name": "mandel (jit)",
            "value": 930733079,
            "unit": "instructions"
          },
          {
            "name": "mandel (interp)",
            "value": 2609633148,
            "unit": "instructions"
          },
          {
            "name": "hash (jit)",
            "value": 497948670,
            "unit": "instructions"
          },
          {
            "name": "hash (interp)",
            "value": 709155101,
            "unit": "instructions"
          },
          {
            "name": "hash GC pause",
            "value": 229395,
            "unit": "words"
          },
          {
            "name": "orgparse (jit)",
            "value": 358612570,
            "unit": "instructions"
          },
          {
            "name": "orgparse (interp)",
            "value": 1411519574,
            "unit": "instructions"
          },
          {
            "name": "callbacks (jit)",
            "value": 642213675,
            "unit": "instructions"
          },
          {
            "name": "callbacks (interp)",
            "value": 1573692947,
            "unit": "instructions"
          },
          {
            "name": "callbacks GC pause",
            "value": 327538,
            "unit": "words"
          },
          {
            "name": "numstr (jit)",
            "value": 265250621,
            "unit": "instructions"
          },
          {
            "name": "numstr (interp)",
            "value": 636170397,
            "unit": "instructions"
          },
          {
            "name": "tasks (jit)",
            "value": 385626286,
            "unit": "instructions"
          },
          {
            "name": "tasks (interp)",
            "value": 283055176,
            "unit": "instructions"
          },
          {
            "name": "editor startup (jit)",
            "value": 138789053,
            "unit": "instructions"
          },
          {
            "name": "editor startup (interp)",
            "value": 136018760,
            "unit": "instructions"
          },
          {
            "name": "keys: type (jit)",
            "value": 1177705,
            "unit": "instructions per key"
          },
          {
            "name": "keys: type-long-line (jit)",
            "value": 1177078,
            "unit": "instructions per key"
          },
          {
            "name": "keys: move (jit)",
            "value": 1104506,
            "unit": "instructions per key"
          },
          {
            "name": "keys: move-modal (jit)",
            "value": 985072,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-open (jit)",
            "value": 19150460,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-first-key (jit)",
            "value": 22011436,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-narrow (jit)",
            "value": 13731855,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-delete (jit)",
            "value": 1398669,
            "unit": "instructions per key"
          },
          {
            "name": "keys: command-first-key (jit)",
            "value": 1516317,
            "unit": "instructions per key"
          },
          {
            "name": "http: plaintext request (jit)",
            "value": 6545,
            "unit": "instructions per request"
          },
          {
            "name": "http: plaintext request (interp)",
            "value": 7454,
            "unit": "instructions per request"
          },
          {
            "name": "http: /work request (jit)",
            "value": 242607,
            "unit": "instructions per request"
          },
          {
            "name": "http: /work request (interp)",
            "value": 631121,
            "unit": "instructions per request"
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
          "id": "df2af1f7e47de335474c5ca4be6ce4229f601057",
          "message": "Merge pull request #103 from ArthurHeymans/rust-scopes\n\nKeep scopes in a table in Rust",
          "timestamp": "2026-10-11T14:09:10+02:00",
          "tree_id": "ac62933824da5e22c540f51ad0135883258cb00f",
          "url": "https://github.com/ArthurHeymans/techne/commit/df2af1f7e47de335474c5ca4be6ce4229f601057"
        },
        "date": 1791720814798,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "startup (jit)",
            "value": 14969847,
            "unit": "instructions"
          },
          {
            "name": "startup (interp)",
            "value": 14809958,
            "unit": "instructions"
          },
          {
            "name": "fib (jit)",
            "value": 765722308,
            "unit": "instructions"
          },
          {
            "name": "fib (interp)",
            "value": 1746452034,
            "unit": "instructions"
          },
          {
            "name": "tak (jit)",
            "value": 946538186,
            "unit": "instructions"
          },
          {
            "name": "tak (interp)",
            "value": 2189909328,
            "unit": "instructions"
          },
          {
            "name": "nqueens (jit)",
            "value": 1044187481,
            "unit": "instructions"
          },
          {
            "name": "nqueens (interp)",
            "value": 2867726426,
            "unit": "instructions"
          },
          {
            "name": "nqueens GC pause",
            "value": 345,
            "unit": "words"
          },
          {
            "name": "bintrees (jit)",
            "value": 3602281498,
            "unit": "instructions"
          },
          {
            "name": "bintrees (interp)",
            "value": 9179818447,
            "unit": "instructions"
          },
          {
            "name": "bintrees GC pause",
            "value": 393138,
            "unit": "words"
          },
          {
            "name": "hof (jit)",
            "value": 1583629631,
            "unit": "instructions"
          },
          {
            "name": "hof (interp)",
            "value": 3143988659,
            "unit": "instructions"
          },
          {
            "name": "hof GC pause",
            "value": 1579648,
            "unit": "words"
          },
          {
            "name": "qsort (jit)",
            "value": 1361453036,
            "unit": "instructions"
          },
          {
            "name": "qsort (interp)",
            "value": 3820688143,
            "unit": "instructions"
          },
          {
            "name": "mandel (jit)",
            "value": 930058924,
            "unit": "instructions"
          },
          {
            "name": "mandel (interp)",
            "value": 2608955781,
            "unit": "instructions"
          },
          {
            "name": "hash (jit)",
            "value": 497192083,
            "unit": "instructions"
          },
          {
            "name": "hash (interp)",
            "value": 708418880,
            "unit": "instructions"
          },
          {
            "name": "hash GC pause",
            "value": 229395,
            "unit": "words"
          },
          {
            "name": "orgparse (jit)",
            "value": 357972397,
            "unit": "instructions"
          },
          {
            "name": "orgparse (interp)",
            "value": 1410770486,
            "unit": "instructions"
          },
          {
            "name": "callbacks (jit)",
            "value": 641469550,
            "unit": "instructions"
          },
          {
            "name": "callbacks (interp)",
            "value": 1573024926,
            "unit": "instructions"
          },
          {
            "name": "callbacks GC pause",
            "value": 327538,
            "unit": "words"
          },
          {
            "name": "numstr (jit)",
            "value": 264494724,
            "unit": "instructions"
          },
          {
            "name": "numstr (interp)",
            "value": 635438276,
            "unit": "instructions"
          },
          {
            "name": "tasks (jit)",
            "value": 250538600,
            "unit": "instructions"
          },
          {
            "name": "tasks (interp)",
            "value": 129686005,
            "unit": "instructions"
          },
          {
            "name": "editor startup (jit)",
            "value": 135328271,
            "unit": "instructions"
          },
          {
            "name": "editor startup (interp)",
            "value": 132570056,
            "unit": "instructions"
          },
          {
            "name": "keys: type (jit)",
            "value": 1184957,
            "unit": "instructions per key"
          },
          {
            "name": "keys: type-long-line (jit)",
            "value": 1181766,
            "unit": "instructions per key"
          },
          {
            "name": "keys: move (jit)",
            "value": 1117281,
            "unit": "instructions per key"
          },
          {
            "name": "keys: move-modal (jit)",
            "value": 997482,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-open (jit)",
            "value": 18316575,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-first-key (jit)",
            "value": 22089965,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-narrow (jit)",
            "value": 13828525,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-delete (jit)",
            "value": 1416235,
            "unit": "instructions per key"
          },
          {
            "name": "keys: command-first-key (jit)",
            "value": 1531209,
            "unit": "instructions per key"
          },
          {
            "name": "http: plaintext request (jit)",
            "value": 6485,
            "unit": "instructions per request"
          },
          {
            "name": "http: plaintext request (interp)",
            "value": 7343,
            "unit": "instructions per request"
          },
          {
            "name": "http: /work request (jit)",
            "value": 242556,
            "unit": "instructions per request"
          },
          {
            "name": "http: /work request (interp)",
            "value": 631008,
            "unit": "instructions per request"
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
          "id": "4d80ac3ac372558309e8d0ca91ebe485b23870a5",
          "message": "Merge pull request #102 from ArthurHeymans/jit-inline-callees\n\nRun small functions inline in compiled code",
          "timestamp": "2026-10-11T14:13:16+02:00",
          "tree_id": "db875d52f359ff8498e2e6ca99514c9e06a8d15d",
          "url": "https://github.com/ArthurHeymans/techne/commit/4d80ac3ac372558309e8d0ca91ebe485b23870a5"
        },
        "date": 1791721018595,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "startup (jit)",
            "value": 15013297,
            "unit": "instructions"
          },
          {
            "name": "startup (interp)",
            "value": 14847380,
            "unit": "instructions"
          },
          {
            "name": "fib (jit)",
            "value": 765781054,
            "unit": "instructions"
          },
          {
            "name": "fib (interp)",
            "value": 1746484989,
            "unit": "instructions"
          },
          {
            "name": "tak (jit)",
            "value": 946589334,
            "unit": "instructions"
          },
          {
            "name": "tak (interp)",
            "value": 2189947205,
            "unit": "instructions"
          },
          {
            "name": "nqueens (jit)",
            "value": 1038520961,
            "unit": "instructions"
          },
          {
            "name": "nqueens (interp)",
            "value": 2867408810,
            "unit": "instructions"
          },
          {
            "name": "nqueens GC pause",
            "value": 345,
            "unit": "words"
          },
          {
            "name": "bintrees (jit)",
            "value": 3602285886,
            "unit": "instructions"
          },
          {
            "name": "bintrees (interp)",
            "value": 9179852280,
            "unit": "instructions"
          },
          {
            "name": "bintrees GC pause",
            "value": 393138,
            "unit": "words"
          },
          {
            "name": "hof (jit)",
            "value": 1579652987,
            "unit": "instructions"
          },
          {
            "name": "hof (interp)",
            "value": 3142821395,
            "unit": "instructions"
          },
          {
            "name": "hof GC pause",
            "value": 1579648,
            "unit": "words"
          },
          {
            "name": "qsort (jit)",
            "value": 1183351593,
            "unit": "instructions"
          },
          {
            "name": "qsort (interp)",
            "value": 3820725926,
            "unit": "instructions"
          },
          {
            "name": "mandel (jit)",
            "value": 927278555,
            "unit": "instructions"
          },
          {
            "name": "mandel (interp)",
            "value": 2608493652,
            "unit": "instructions"
          },
          {
            "name": "hash (jit)",
            "value": 483611713,
            "unit": "instructions"
          },
          {
            "name": "hash (interp)",
            "value": 707251982,
            "unit": "instructions"
          },
          {
            "name": "hash GC pause",
            "value": 229395,
            "unit": "words"
          },
          {
            "name": "orgparse (jit)",
            "value": 360823044,
            "unit": "instructions"
          },
          {
            "name": "orgparse (interp)",
            "value": 1416785976,
            "unit": "instructions"
          },
          {
            "name": "callbacks (jit)",
            "value": 558177260,
            "unit": "instructions"
          },
          {
            "name": "callbacks (interp)",
            "value": 1572996362,
            "unit": "instructions"
          },
          {
            "name": "callbacks GC pause",
            "value": 327538,
            "unit": "words"
          },
          {
            "name": "numstr (jit)",
            "value": 240966038,
            "unit": "instructions"
          },
          {
            "name": "numstr (interp)",
            "value": 635471344,
            "unit": "instructions"
          },
          {
            "name": "tasks (jit)",
            "value": 256340235,
            "unit": "instructions"
          },
          {
            "name": "tasks (interp)",
            "value": 129625321,
            "unit": "instructions"
          },
          {
            "name": "editor startup (jit)",
            "value": 135649521,
            "unit": "instructions"
          },
          {
            "name": "editor startup (interp)",
            "value": 132727222,
            "unit": "instructions"
          },
          {
            "name": "keys: type (jit)",
            "value": 1153953,
            "unit": "instructions per key"
          },
          {
            "name": "keys: type-long-line (jit)",
            "value": 1150836,
            "unit": "instructions per key"
          },
          {
            "name": "keys: move (jit)",
            "value": 1100899,
            "unit": "instructions per key"
          },
          {
            "name": "keys: move-modal (jit)",
            "value": 986802,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-open (jit)",
            "value": 19165821,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-first-key (jit)",
            "value": 22021808,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-narrow (jit)",
            "value": 13759928,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-delete (jit)",
            "value": 1410762,
            "unit": "instructions per key"
          },
          {
            "name": "keys: command-first-key (jit)",
            "value": 1528159,
            "unit": "instructions per key"
          },
          {
            "name": "http: plaintext request (jit)",
            "value": 6435,
            "unit": "instructions per request"
          },
          {
            "name": "http: plaintext request (interp)",
            "value": 7348,
            "unit": "instructions per request"
          },
          {
            "name": "http: /work request (jit)",
            "value": 218458,
            "unit": "instructions per request"
          },
          {
            "name": "http: /work request (interp)",
            "value": 631011,
            "unit": "instructions per request"
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
          "id": "00fb26750309c1a63998c69c3baabb9a873fbcd7",
          "message": "Merge pull request #109 from ArthurHeymans/fast-optional\n\nParse no arguments where none are optional",
          "timestamp": "2026-10-11T15:43:37+02:00",
          "tree_id": "971ca4624bb992e5dbe05c4e927590b1aad96892",
          "url": "https://github.com/ArthurHeymans/techne/commit/00fb26750309c1a63998c69c3baabb9a873fbcd7"
        },
        "date": 1791726422483,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "startup (jit)",
            "value": 15039989,
            "unit": "instructions"
          },
          {
            "name": "startup (interp)",
            "value": 14873475,
            "unit": "instructions"
          },
          {
            "name": "fib (jit)",
            "value": 765808907,
            "unit": "instructions"
          },
          {
            "name": "fib (interp)",
            "value": 1746515345,
            "unit": "instructions"
          },
          {
            "name": "tak (jit)",
            "value": 946616904,
            "unit": "instructions"
          },
          {
            "name": "tak (interp)",
            "value": 2189974961,
            "unit": "instructions"
          },
          {
            "name": "nqueens (jit)",
            "value": 1037129865,
            "unit": "instructions"
          },
          {
            "name": "nqueens (interp)",
            "value": 2867434961,
            "unit": "instructions"
          },
          {
            "name": "nqueens GC pause",
            "value": 345,
            "unit": "words"
          },
          {
            "name": "bintrees (jit)",
            "value": 3602267536,
            "unit": "instructions"
          },
          {
            "name": "bintrees (interp)",
            "value": 9179819122,
            "unit": "instructions"
          },
          {
            "name": "bintrees GC pause",
            "value": 393138,
            "unit": "words"
          },
          {
            "name": "hof (jit)",
            "value": 1579655267,
            "unit": "instructions"
          },
          {
            "name": "hof (interp)",
            "value": 3142706394,
            "unit": "instructions"
          },
          {
            "name": "hof GC pause",
            "value": 1579648,
            "unit": "words"
          },
          {
            "name": "qsort (jit)",
            "value": 1183368352,
            "unit": "instructions"
          },
          {
            "name": "qsort (interp)",
            "value": 3820691168,
            "unit": "instructions"
          },
          {
            "name": "mandel (jit)",
            "value": 927246108,
            "unit": "instructions"
          },
          {
            "name": "mandel (interp)",
            "value": 2608460641,
            "unit": "instructions"
          },
          {
            "name": "hash (jit)",
            "value": 483641926,
            "unit": "instructions"
          },
          {
            "name": "hash (interp)",
            "value": 707284100,
            "unit": "instructions"
          },
          {
            "name": "hash GC pause",
            "value": 229395,
            "unit": "words"
          },
          {
            "name": "orgparse (jit)",
            "value": 360710685,
            "unit": "instructions"
          },
          {
            "name": "orgparse (interp)",
            "value": 1416676653,
            "unit": "instructions"
          },
          {
            "name": "callbacks (jit)",
            "value": 558117994,
            "unit": "instructions"
          },
          {
            "name": "callbacks (interp)",
            "value": 1573023353,
            "unit": "instructions"
          },
          {
            "name": "callbacks GC pause",
            "value": 327538,
            "unit": "words"
          },
          {
            "name": "numstr (jit)",
            "value": 240991763,
            "unit": "instructions"
          },
          {
            "name": "numstr (interp)",
            "value": 635502285,
            "unit": "instructions"
          },
          {
            "name": "tasks (jit)",
            "value": 254722066,
            "unit": "instructions"
          },
          {
            "name": "tasks (interp)",
            "value": 125574544,
            "unit": "instructions"
          },
          {
            "name": "editor startup (jit)",
            "value": 134315636,
            "unit": "instructions"
          },
          {
            "name": "editor startup (interp)",
            "value": 131403542,
            "unit": "instructions"
          },
          {
            "name": "keys: type (jit)",
            "value": 827127,
            "unit": "instructions per key"
          },
          {
            "name": "keys: type-long-line (jit)",
            "value": 825712,
            "unit": "instructions per key"
          },
          {
            "name": "keys: move (jit)",
            "value": 784599,
            "unit": "instructions per key"
          },
          {
            "name": "keys: move-modal (jit)",
            "value": 718173,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-open (jit)",
            "value": 18193401,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-first-key (jit)",
            "value": 21907282,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-narrow (jit)",
            "value": 13645405,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-delete (jit)",
            "value": 1229373,
            "unit": "instructions per key"
          },
          {
            "name": "keys: command-first-key (jit)",
            "value": 1346141,
            "unit": "instructions per key"
          },
          {
            "name": "http: plaintext request (jit)",
            "value": 6438,
            "unit": "instructions per request"
          },
          {
            "name": "http: plaintext request (interp)",
            "value": 7352,
            "unit": "instructions per request"
          },
          {
            "name": "http: /work request (jit)",
            "value": 218468,
            "unit": "instructions per request"
          },
          {
            "name": "http: /work request (interp)",
            "value": 631017,
            "unit": "instructions per request"
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
          "id": "4911b321ba8424085ce8d9b306ea1127ff85de8e",
          "message": "Merge pull request #107 from ArthurHeymans/child-tasks\n\nKill an execution's children with it",
          "timestamp": "2026-10-11T15:55:26+02:00",
          "tree_id": "2a370637d6caaed0c7c24f4864d7f9717a110ead",
          "url": "https://github.com/ArthurHeymans/techne/commit/4911b321ba8424085ce8d9b306ea1127ff85de8e"
        },
        "date": 1791727143764,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "startup (jit)",
            "value": 15113076,
            "unit": "instructions"
          },
          {
            "name": "startup (interp)",
            "value": 14945934,
            "unit": "instructions"
          },
          {
            "name": "fib (jit)",
            "value": 765882521,
            "unit": "instructions"
          },
          {
            "name": "fib (interp)",
            "value": 1746587951,
            "unit": "instructions"
          },
          {
            "name": "tak (jit)",
            "value": 946687707,
            "unit": "instructions"
          },
          {
            "name": "tak (interp)",
            "value": 2190045635,
            "unit": "instructions"
          },
          {
            "name": "nqueens (jit)",
            "value": 1038636816,
            "unit": "instructions"
          },
          {
            "name": "nqueens (interp)",
            "value": 2866747061,
            "unit": "instructions"
          },
          {
            "name": "nqueens GC pause",
            "value": 345,
            "unit": "words"
          },
          {
            "name": "bintrees (jit)",
            "value": 3602433383,
            "unit": "instructions"
          },
          {
            "name": "bintrees (interp)",
            "value": 9179956965,
            "unit": "instructions"
          },
          {
            "name": "bintrees GC pause",
            "value": 393138,
            "unit": "words"
          },
          {
            "name": "hof (jit)",
            "value": 1579651870,
            "unit": "instructions"
          },
          {
            "name": "hof (interp)",
            "value": 3142841803,
            "unit": "instructions"
          },
          {
            "name": "hof GC pause",
            "value": 1579648,
            "unit": "words"
          },
          {
            "name": "qsort (jit)",
            "value": 1183475886,
            "unit": "instructions"
          },
          {
            "name": "qsort (interp)",
            "value": 3820822418,
            "unit": "instructions"
          },
          {
            "name": "mandel (jit)",
            "value": 927375393,
            "unit": "instructions"
          },
          {
            "name": "mandel (interp)",
            "value": 2608593268,
            "unit": "instructions"
          },
          {
            "name": "hash (jit)",
            "value": 483685920,
            "unit": "instructions"
          },
          {
            "name": "hash (interp)",
            "value": 707357204,
            "unit": "instructions"
          },
          {
            "name": "hash GC pause",
            "value": 229395,
            "unit": "words"
          },
          {
            "name": "orgparse (jit)",
            "value": 360785033,
            "unit": "instructions"
          },
          {
            "name": "orgparse (interp)",
            "value": 1416798486,
            "unit": "instructions"
          },
          {
            "name": "callbacks (jit)",
            "value": 558221299,
            "unit": "instructions"
          },
          {
            "name": "callbacks (interp)",
            "value": 1573100637,
            "unit": "instructions"
          },
          {
            "name": "callbacks GC pause",
            "value": 327538,
            "unit": "words"
          },
          {
            "name": "numstr (jit)",
            "value": 241073158,
            "unit": "instructions"
          },
          {
            "name": "numstr (interp)",
            "value": 635570034,
            "unit": "instructions"
          },
          {
            "name": "tasks (jit)",
            "value": 253368440,
            "unit": "instructions"
          },
          {
            "name": "tasks (interp)",
            "value": 127054891,
            "unit": "instructions"
          },
          {
            "name": "editor startup (jit)",
            "value": 134180573,
            "unit": "instructions"
          },
          {
            "name": "editor startup (interp)",
            "value": 131227330,
            "unit": "instructions"
          },
          {
            "name": "keys: type (jit)",
            "value": 832676,
            "unit": "instructions per key"
          },
          {
            "name": "keys: type-long-line (jit)",
            "value": 829627,
            "unit": "instructions per key"
          },
          {
            "name": "keys: move (jit)",
            "value": 787079,
            "unit": "instructions per key"
          },
          {
            "name": "keys: move-modal (jit)",
            "value": 722476,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-open (jit)",
            "value": 18199385,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-first-key (jit)",
            "value": 21909327,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-narrow (jit)",
            "value": 13645795,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-delete (jit)",
            "value": 1228061,
            "unit": "instructions per key"
          },
          {
            "name": "keys: command-first-key (jit)",
            "value": 1348763,
            "unit": "instructions per key"
          },
          {
            "name": "http: plaintext request (jit)",
            "value": 6477,
            "unit": "instructions per request"
          },
          {
            "name": "http: plaintext request (interp)",
            "value": 7379,
            "unit": "instructions per request"
          },
          {
            "name": "http: /work request (jit)",
            "value": 218504,
            "unit": "instructions per request"
          },
          {
            "name": "http: /work request (interp)",
            "value": 631048,
            "unit": "instructions per request"
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
          "id": "d032e03c78f44ca51017952ebdf75f39f862ca7c",
          "message": "Merge pull request #104 from ArthurHeymans/dicts\n\nPersistent dicts and sets with literal syntax",
          "timestamp": "2026-10-11T16:47:49+02:00",
          "tree_id": "0c9287ad159a9e2cd42af2ce643e8b47797fa1f3",
          "url": "https://github.com/ArthurHeymans/techne/commit/d032e03c78f44ca51017952ebdf75f39f862ca7c"
        },
        "date": 1791730363288,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "startup (jit)",
            "value": 15815068,
            "unit": "instructions"
          },
          {
            "name": "startup (interp)",
            "value": 15645631,
            "unit": "instructions"
          },
          {
            "name": "fib (jit)",
            "value": 766578624,
            "unit": "instructions"
          },
          {
            "name": "fib (interp)",
            "value": 1747293850,
            "unit": "instructions"
          },
          {
            "name": "tak (jit)",
            "value": 947391249,
            "unit": "instructions"
          },
          {
            "name": "tak (interp)",
            "value": 2190752701,
            "unit": "instructions"
          },
          {
            "name": "nqueens (jit)",
            "value": 1038217889,
            "unit": "instructions"
          },
          {
            "name": "nqueens (interp)",
            "value": 2868160792,
            "unit": "instructions"
          },
          {
            "name": "nqueens GC pause",
            "value": 345,
            "unit": "words"
          },
          {
            "name": "bintrees (jit)",
            "value": 3603093877,
            "unit": "instructions"
          },
          {
            "name": "bintrees (interp)",
            "value": 9180638709,
            "unit": "instructions"
          },
          {
            "name": "bintrees GC pause",
            "value": 393138,
            "unit": "words"
          },
          {
            "name": "hof (jit)",
            "value": 1580384250,
            "unit": "instructions"
          },
          {
            "name": "hof (interp)",
            "value": 3143557352,
            "unit": "instructions"
          },
          {
            "name": "hof GC pause",
            "value": 1579648,
            "unit": "words"
          },
          {
            "name": "qsort (jit)",
            "value": 1184118796,
            "unit": "instructions"
          },
          {
            "name": "qsort (interp)",
            "value": 3821473003,
            "unit": "instructions"
          },
          {
            "name": "mandel (jit)",
            "value": 928071212,
            "unit": "instructions"
          },
          {
            "name": "mandel (interp)",
            "value": 2609241985,
            "unit": "instructions"
          },
          {
            "name": "hash (jit)",
            "value": 483991823,
            "unit": "instructions"
          },
          {
            "name": "hash (interp)",
            "value": 707660997,
            "unit": "instructions"
          },
          {
            "name": "hash GC pause",
            "value": 229395,
            "unit": "words"
          },
          {
            "name": "orgparse (jit)",
            "value": 361210184,
            "unit": "instructions"
          },
          {
            "name": "orgparse (interp)",
            "value": 1417241580,
            "unit": "instructions"
          },
          {
            "name": "callbacks (jit)",
            "value": 558831359,
            "unit": "instructions"
          },
          {
            "name": "callbacks (interp)",
            "value": 1573775035,
            "unit": "instructions"
          },
          {
            "name": "callbacks GC pause",
            "value": 327538,
            "unit": "words"
          },
          {
            "name": "numstr (jit)",
            "value": 240777490,
            "unit": "instructions"
          },
          {
            "name": "numstr (interp)",
            "value": 635277358,
            "unit": "instructions"
          },
          {
            "name": "tasks (jit)",
            "value": 254905054,
            "unit": "instructions"
          },
          {
            "name": "tasks (interp)",
            "value": 128629491,
            "unit": "instructions"
          },
          {
            "name": "editor startup (jit)",
            "value": 135506487,
            "unit": "instructions"
          },
          {
            "name": "editor startup (interp)",
            "value": 132526865,
            "unit": "instructions"
          },
          {
            "name": "keys: type (jit)",
            "value": 831372,
            "unit": "instructions per key"
          },
          {
            "name": "keys: type-long-line (jit)",
            "value": 829573,
            "unit": "instructions per key"
          },
          {
            "name": "keys: move (jit)",
            "value": 787389,
            "unit": "instructions per key"
          },
          {
            "name": "keys: move-modal (jit)",
            "value": 721326,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-open (jit)",
            "value": 18523687,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-first-key (jit)",
            "value": 21911378,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-narrow (jit)",
            "value": 13649202,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-delete (jit)",
            "value": 1229410,
            "unit": "instructions per key"
          },
          {
            "name": "keys: command-first-key (jit)",
            "value": 1350230,
            "unit": "instructions per key"
          },
          {
            "name": "http: plaintext request (jit)",
            "value": 6490,
            "unit": "instructions per request"
          },
          {
            "name": "http: plaintext request (interp)",
            "value": 7397,
            "unit": "instructions per request"
          },
          {
            "name": "http: /work request (jit)",
            "value": 217508,
            "unit": "instructions per request"
          },
          {
            "name": "http: /work request (interp)",
            "value": 630060,
            "unit": "instructions per request"
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
          "id": "d9541555c3f5b7d3bcfa6e06412458cc7dbaf0e3",
          "message": "Merge pull request #105 from ArthurHeymans/json\n\nRead and write JSON",
          "timestamp": "2026-10-11T17:08:45+02:00",
          "tree_id": "1f63ddca8b8d15a31354faf37a8c2dd299278348",
          "url": "https://github.com/ArthurHeymans/techne/commit/d9541555c3f5b7d3bcfa6e06412458cc7dbaf0e3"
        },
        "date": 1791731633566,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "startup (jit)",
            "value": 15742401,
            "unit": "instructions"
          },
          {
            "name": "startup (interp)",
            "value": 15575686,
            "unit": "instructions"
          },
          {
            "name": "fib (jit)",
            "value": 766508167,
            "unit": "instructions"
          },
          {
            "name": "fib (interp)",
            "value": 1747217623,
            "unit": "instructions"
          },
          {
            "name": "tak (jit)",
            "value": 947315845,
            "unit": "instructions"
          },
          {
            "name": "tak (interp)",
            "value": 2190675572,
            "unit": "instructions"
          },
          {
            "name": "nqueens (jit)",
            "value": 1038132844,
            "unit": "instructions"
          },
          {
            "name": "nqueens (interp)",
            "value": 2868082816,
            "unit": "instructions"
          },
          {
            "name": "nqueens GC pause",
            "value": 345,
            "unit": "words"
          },
          {
            "name": "bintrees (jit)",
            "value": 3603010151,
            "unit": "instructions"
          },
          {
            "name": "bintrees (interp)",
            "value": 9180563213,
            "unit": "instructions"
          },
          {
            "name": "bintrees GC pause",
            "value": 393138,
            "unit": "words"
          },
          {
            "name": "hof (jit)",
            "value": 1580316546,
            "unit": "instructions"
          },
          {
            "name": "hof (interp)",
            "value": 3143464152,
            "unit": "instructions"
          },
          {
            "name": "hof GC pause",
            "value": 1579648,
            "unit": "words"
          },
          {
            "name": "qsort (jit)",
            "value": 1184012297,
            "unit": "instructions"
          },
          {
            "name": "qsort (interp)",
            "value": 3821393800,
            "unit": "instructions"
          },
          {
            "name": "mandel (jit)",
            "value": 927927468,
            "unit": "instructions"
          },
          {
            "name": "mandel (interp)",
            "value": 2609162566,
            "unit": "instructions"
          },
          {
            "name": "hash (jit)",
            "value": 483930312,
            "unit": "instructions"
          },
          {
            "name": "hash (interp)",
            "value": 707588369,
            "unit": "instructions"
          },
          {
            "name": "hash GC pause",
            "value": 229395,
            "unit": "words"
          },
          {
            "name": "orgparse (jit)",
            "value": 361138840,
            "unit": "instructions"
          },
          {
            "name": "orgparse (interp)",
            "value": 1417158344,
            "unit": "instructions"
          },
          {
            "name": "callbacks (jit)",
            "value": 558838590,
            "unit": "instructions"
          },
          {
            "name": "callbacks (interp)",
            "value": 1573695027,
            "unit": "instructions"
          },
          {
            "name": "callbacks GC pause",
            "value": 327538,
            "unit": "words"
          },
          {
            "name": "numstr (jit)",
            "value": 240703484,
            "unit": "instructions"
          },
          {
            "name": "numstr (interp)",
            "value": 635205156,
            "unit": "instructions"
          },
          {
            "name": "tasks (jit)",
            "value": 254369540,
            "unit": "instructions"
          },
          {
            "name": "tasks (interp)",
            "value": 128070695,
            "unit": "instructions"
          },
          {
            "name": "editor startup (jit)",
            "value": 135102607,
            "unit": "instructions"
          },
          {
            "name": "editor startup (interp)",
            "value": 132186780,
            "unit": "instructions"
          },
          {
            "name": "keys: type (jit)",
            "value": 823261,
            "unit": "instructions per key"
          },
          {
            "name": "keys: type-long-line (jit)",
            "value": 820496,
            "unit": "instructions per key"
          },
          {
            "name": "keys: move (jit)",
            "value": 777570,
            "unit": "instructions per key"
          },
          {
            "name": "keys: move-modal (jit)",
            "value": 713777,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-open (jit)",
            "value": 18505782,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-first-key (jit)",
            "value": 21903038,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-narrow (jit)",
            "value": 13635256,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-delete (jit)",
            "value": 1213684,
            "unit": "instructions per key"
          },
          {
            "name": "keys: command-first-key (jit)",
            "value": 1340403,
            "unit": "instructions per key"
          },
          {
            "name": "http: plaintext request (jit)",
            "value": 6439,
            "unit": "instructions per request"
          },
          {
            "name": "http: plaintext request (interp)",
            "value": 7348,
            "unit": "instructions per request"
          },
          {
            "name": "http: /work request (jit)",
            "value": 217458,
            "unit": "instructions per request"
          },
          {
            "name": "http: /work request (interp)",
            "value": 630010,
            "unit": "instructions per request"
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
          "id": "9d1b1df3d899b2a8c461613d1ee8849c09d2333c",
          "message": "Merge pull request #113 from ArthurHeymans/child-pruning\n\nAmortize child-list pruning",
          "timestamp": "2026-10-11T18:10:43+02:00",
          "tree_id": "3357e92850b8ef1e87df1be032b4d1cb196aa27c",
          "url": "https://github.com/ArthurHeymans/techne/commit/9d1b1df3d899b2a8c461613d1ee8849c09d2333c"
        },
        "date": 1791735343758,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "startup (jit)",
            "value": 15723408,
            "unit": "instructions"
          },
          {
            "name": "startup (interp)",
            "value": 15571203,
            "unit": "instructions"
          },
          {
            "name": "fib (jit)",
            "value": 766501780,
            "unit": "instructions"
          },
          {
            "name": "fib (interp)",
            "value": 1747213140,
            "unit": "instructions"
          },
          {
            "name": "tak (jit)",
            "value": 947312677,
            "unit": "instructions"
          },
          {
            "name": "tak (interp)",
            "value": 2190675590,
            "unit": "instructions"
          },
          {
            "name": "nqueens (jit)",
            "value": 1038133519,
            "unit": "instructions"
          },
          {
            "name": "nqueens (interp)",
            "value": 2868082834,
            "unit": "instructions"
          },
          {
            "name": "nqueens GC pause",
            "value": 345,
            "unit": "words"
          },
          {
            "name": "bintrees (jit)",
            "value": 3603027568,
            "unit": "instructions"
          },
          {
            "name": "bintrees (interp)",
            "value": 9180563231,
            "unit": "instructions"
          },
          {
            "name": "bintrees GC pause",
            "value": 393138,
            "unit": "words"
          },
          {
            "name": "hof (jit)",
            "value": 1580288990,
            "unit": "instructions"
          },
          {
            "name": "hof (interp)",
            "value": 3143464170,
            "unit": "instructions"
          },
          {
            "name": "hof GC pause",
            "value": 1579648,
            "unit": "words"
          },
          {
            "name": "qsort (jit)",
            "value": 1184026729,
            "unit": "instructions"
          },
          {
            "name": "qsort (interp)",
            "value": 3821393818,
            "unit": "instructions"
          },
          {
            "name": "mandel (jit)",
            "value": 927982140,
            "unit": "instructions"
          },
          {
            "name": "mandel (interp)",
            "value": 2609158083,
            "unit": "instructions"
          },
          {
            "name": "hash (jit)",
            "value": 483913394,
            "unit": "instructions"
          },
          {
            "name": "hash (interp)",
            "value": 707583886,
            "unit": "instructions"
          },
          {
            "name": "hash GC pause",
            "value": 229395,
            "unit": "words"
          },
          {
            "name": "orgparse (jit)",
            "value": 361223579,
            "unit": "instructions"
          },
          {
            "name": "orgparse (interp)",
            "value": 1417158362,
            "unit": "instructions"
          },
          {
            "name": "callbacks (jit)",
            "value": 558725152,
            "unit": "instructions"
          },
          {
            "name": "callbacks (interp)",
            "value": 1573695045,
            "unit": "instructions"
          },
          {
            "name": "callbacks GC pause",
            "value": 327538,
            "unit": "words"
          },
          {
            "name": "numstr (jit)",
            "value": 240690913,
            "unit": "instructions"
          },
          {
            "name": "numstr (interp)",
            "value": 635200673,
            "unit": "instructions"
          },
          {
            "name": "tasks (jit)",
            "value": 254237756,
            "unit": "instructions"
          },
          {
            "name": "tasks (interp)",
            "value": 128066212,
            "unit": "instructions"
          },
          {
            "name": "editor startup (jit)",
            "value": 135101856,
            "unit": "instructions"
          },
          {
            "name": "editor startup (interp)",
            "value": 132186780,
            "unit": "instructions"
          },
          {
            "name": "keys: type (jit)",
            "value": 821809,
            "unit": "instructions per key"
          },
          {
            "name": "keys: type-long-line (jit)",
            "value": 820550,
            "unit": "instructions per key"
          },
          {
            "name": "keys: move (jit)",
            "value": 777693,
            "unit": "instructions per key"
          },
          {
            "name": "keys: move-modal (jit)",
            "value": 713764,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-open (jit)",
            "value": 18505782,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-first-key (jit)",
            "value": 21903629,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-narrow (jit)",
            "value": 13635256,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-delete (jit)",
            "value": 1213684,
            "unit": "instructions per key"
          },
          {
            "name": "keys: command-first-key (jit)",
            "value": 1341554,
            "unit": "instructions per key"
          },
          {
            "name": "http: plaintext request (jit)",
            "value": 6436,
            "unit": "instructions per request"
          },
          {
            "name": "http: plaintext request (interp)",
            "value": 7348,
            "unit": "instructions per request"
          },
          {
            "name": "http: /work request (jit)",
            "value": 217458,
            "unit": "instructions per request"
          },
          {
            "name": "http: /work request (interp)",
            "value": 630010,
            "unit": "instructions per request"
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
          "id": "02a985f8537faab3eb478eb312ea150fb090a818",
          "message": "Merge pull request #106 from ArthurHeymans/jit-inline-frames\n\nCompile inlined calls more cheaply",
          "timestamp": "2026-10-11T18:31:51+02:00",
          "tree_id": "85049168f60e8440ebf0dc08a0714b89c98fc461",
          "url": "https://github.com/ArthurHeymans/techne/commit/02a985f8537faab3eb478eb312ea150fb090a818"
        },
        "date": 1791736552632,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "startup (jit)",
            "value": 15746932,
            "unit": "instructions"
          },
          {
            "name": "startup (interp)",
            "value": 15574355,
            "unit": "instructions"
          },
          {
            "name": "fib (jit)",
            "value": 766518265,
            "unit": "instructions"
          },
          {
            "name": "fib (interp)",
            "value": 1747220806,
            "unit": "instructions"
          },
          {
            "name": "tak (jit)",
            "value": 947327284,
            "unit": "instructions"
          },
          {
            "name": "tak (interp)",
            "value": 2190678768,
            "unit": "instructions"
          },
          {
            "name": "nqueens (jit)",
            "value": 1038130189,
            "unit": "instructions"
          },
          {
            "name": "nqueens (interp)",
            "value": 2868086045,
            "unit": "instructions"
          },
          {
            "name": "nqueens GC pause",
            "value": 345,
            "unit": "words"
          },
          {
            "name": "bintrees (jit)",
            "value": 3603070118,
            "unit": "instructions"
          },
          {
            "name": "bintrees (interp)",
            "value": 9180566474,
            "unit": "instructions"
          },
          {
            "name": "bintrees GC pause",
            "value": 393138,
            "unit": "words"
          },
          {
            "name": "hof (jit)",
            "value": 1580319813,
            "unit": "instructions"
          },
          {
            "name": "hof (interp)",
            "value": 3143471930,
            "unit": "instructions"
          },
          {
            "name": "hof GC pause",
            "value": 1579648,
            "unit": "words"
          },
          {
            "name": "qsort (jit)",
            "value": 1186888269,
            "unit": "instructions"
          },
          {
            "name": "qsort (interp)",
            "value": 3821397089,
            "unit": "instructions"
          },
          {
            "name": "mandel (jit)",
            "value": 928005874,
            "unit": "instructions"
          },
          {
            "name": "mandel (interp)",
            "value": 2609165782,
            "unit": "instructions"
          },
          {
            "name": "hash (jit)",
            "value": 483964812,
            "unit": "instructions"
          },
          {
            "name": "hash (interp)",
            "value": 707591598,
            "unit": "instructions"
          },
          {
            "name": "hash GC pause",
            "value": 229395,
            "unit": "words"
          },
          {
            "name": "orgparse (jit)",
            "value": 361232256,
            "unit": "instructions"
          },
          {
            "name": "orgparse (interp)",
            "value": 1417161742,
            "unit": "instructions"
          },
          {
            "name": "callbacks (jit)",
            "value": 541770474,
            "unit": "instructions"
          },
          {
            "name": "callbacks (interp)",
            "value": 1573698370,
            "unit": "instructions"
          },
          {
            "name": "callbacks GC pause",
            "value": 327538,
            "unit": "words"
          },
          {
            "name": "numstr (jit)",
            "value": 240709632,
            "unit": "instructions"
          },
          {
            "name": "numstr (interp)",
            "value": 635208352,
            "unit": "instructions"
          },
          {
            "name": "tasks (jit)",
            "value": 252184286,
            "unit": "instructions"
          },
          {
            "name": "tasks (interp)",
            "value": 128076784,
            "unit": "instructions"
          },
          {
            "name": "editor startup (jit)",
            "value": 135113582,
            "unit": "instructions"
          },
          {
            "name": "editor startup (interp)",
            "value": 132212419,
            "unit": "instructions"
          },
          {
            "name": "keys: type (jit)",
            "value": 821668,
            "unit": "instructions per key"
          },
          {
            "name": "keys: type-long-line (jit)",
            "value": 820213,
            "unit": "instructions per key"
          },
          {
            "name": "keys: move (jit)",
            "value": 777621,
            "unit": "instructions per key"
          },
          {
            "name": "keys: move-modal (jit)",
            "value": 714659,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-open (jit)",
            "value": 18505769,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-first-key (jit)",
            "value": 21903014,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-narrow (jit)",
            "value": 13635232,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-delete (jit)",
            "value": 1213660,
            "unit": "instructions per key"
          },
          {
            "name": "keys: command-first-key (jit)",
            "value": 1341375,
            "unit": "instructions per key"
          },
          {
            "name": "http: plaintext request (jit)",
            "value": 6435,
            "unit": "instructions per request"
          },
          {
            "name": "http: plaintext request (interp)",
            "value": 7348,
            "unit": "instructions per request"
          },
          {
            "name": "http: /work request (jit)",
            "value": 217461,
            "unit": "instructions per request"
          },
          {
            "name": "http: /work request (interp)",
            "value": 630009,
            "unit": "instructions per request"
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
          "id": "956ee004377dc0845b35aa4f6b4804cc5f04c3e6",
          "message": "Merge pull request #108 from ArthurHeymans/jit-inline-tail\n\nRun small functions inline at tail calls",
          "timestamp": "2026-10-11T18:32:05+02:00",
          "tree_id": "769b3fa93d300725ee752647c091ebe7b5f61000",
          "url": "https://github.com/ArthurHeymans/techne/commit/956ee004377dc0845b35aa4f6b4804cc5f04c3e6"
        },
        "date": 1791736847017,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "startup (jit)",
            "value": 15745103,
            "unit": "instructions"
          },
          {
            "name": "startup (interp)",
            "value": 15594631,
            "unit": "instructions"
          },
          {
            "name": "fib (jit)",
            "value": 766539217,
            "unit": "instructions"
          },
          {
            "name": "fib (interp)",
            "value": 1740191990,
            "unit": "instructions"
          },
          {
            "name": "tak (jit)",
            "value": 947340000,
            "unit": "instructions"
          },
          {
            "name": "tak (interp)",
            "value": 2138622194,
            "unit": "instructions"
          },
          {
            "name": "nqueens (jit)",
            "value": 1039532728,
            "unit": "instructions"
          },
          {
            "name": "nqueens (interp)",
            "value": 2858829063,
            "unit": "instructions"
          },
          {
            "name": "nqueens GC pause",
            "value": 345,
            "unit": "words"
          },
          {
            "name": "bintrees (jit)",
            "value": 3603096910,
            "unit": "instructions"
          },
          {
            "name": "bintrees (interp)",
            "value": 9052724581,
            "unit": "instructions"
          },
          {
            "name": "bintrees GC pause",
            "value": 393138,
            "unit": "words"
          },
          {
            "name": "hof (jit)",
            "value": 1579762335,
            "unit": "instructions"
          },
          {
            "name": "hof (interp)",
            "value": 3093904120,
            "unit": "instructions"
          },
          {
            "name": "hof GC pause",
            "value": 1579648,
            "unit": "words"
          },
          {
            "name": "qsort (jit)",
            "value": 1186912741,
            "unit": "instructions"
          },
          {
            "name": "qsort (interp)",
            "value": 3771617480,
            "unit": "instructions"
          },
          {
            "name": "mandel (jit)",
            "value": 928001946,
            "unit": "instructions"
          },
          {
            "name": "mandel (interp)",
            "value": 2721056550,
            "unit": "instructions"
          },
          {
            "name": "hash (jit)",
            "value": 485563705,
            "unit": "instructions"
          },
          {
            "name": "hash (interp)",
            "value": 705622636,
            "unit": "instructions"
          },
          {
            "name": "hash GC pause",
            "value": 229395,
            "unit": "words"
          },
          {
            "name": "orgparse (jit)",
            "value": 361277098,
            "unit": "instructions"
          },
          {
            "name": "orgparse (interp)",
            "value": 1394030432,
            "unit": "instructions"
          },
          {
            "name": "callbacks (jit)",
            "value": 541890413,
            "unit": "instructions"
          },
          {
            "name": "callbacks (interp)",
            "value": 1553308318,
            "unit": "instructions"
          },
          {
            "name": "callbacks GC pause",
            "value": 327538,
            "unit": "words"
          },
          {
            "name": "numstr (jit)",
            "value": 240733759,
            "unit": "instructions"
          },
          {
            "name": "numstr (interp)",
            "value": 639218064,
            "unit": "instructions"
          },
          {
            "name": "tasks (jit)",
            "value": 252231740,
            "unit": "instructions"
          },
          {
            "name": "tasks (interp)",
            "value": 127473572,
            "unit": "instructions"
          },
          {
            "name": "editor startup (jit)",
            "value": 135355755,
            "unit": "instructions"
          },
          {
            "name": "editor startup (interp)",
            "value": 132452994,
            "unit": "instructions"
          },
          {
            "name": "keys: type (jit)",
            "value": 813371,
            "unit": "instructions per key"
          },
          {
            "name": "keys: type-long-line (jit)",
            "value": 812227,
            "unit": "instructions per key"
          },
          {
            "name": "keys: move (jit)",
            "value": 766825,
            "unit": "instructions per key"
          },
          {
            "name": "keys: move-modal (jit)",
            "value": 704157,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-open (jit)",
            "value": 18486173,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-first-key (jit)",
            "value": 21887776,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-narrow (jit)",
            "value": 13620114,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-delete (jit)",
            "value": 1197908,
            "unit": "instructions per key"
          },
          {
            "name": "keys: command-first-key (jit)",
            "value": 1327136,
            "unit": "instructions per key"
          },
          {
            "name": "http: plaintext request (jit)",
            "value": 6429,
            "unit": "instructions per request"
          },
          {
            "name": "http: plaintext request (interp)",
            "value": 7336,
            "unit": "instructions per request"
          },
          {
            "name": "http: /work request (jit)",
            "value": 217448,
            "unit": "instructions per request"
          },
          {
            "name": "http: /work request (interp)",
            "value": 629998,
            "unit": "instructions per request"
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
          "id": "3cc5b13be5915547fd7f62a0376a33531a6c60ff",
          "message": "Merge pull request #110 from ArthurHeymans/techne-net\n\nTCP networking for the Lisp",
          "timestamp": "2026-10-11T18:48:17+02:00",
          "tree_id": "1d3c2b9c1bc40f2b26dc9f4d1d68cd8569300c3c",
          "url": "https://github.com/ArthurHeymans/techne/commit/3cc5b13be5915547fd7f62a0376a33531a6c60ff"
        },
        "date": 1791737506831,
        "tool": "customSmallerIsBetter",
        "benches": [
          {
            "name": "startup (jit)",
            "value": 15764112,
            "unit": "instructions"
          },
          {
            "name": "startup (interp)",
            "value": 15599131,
            "unit": "instructions"
          },
          {
            "name": "fib (jit)",
            "value": 766538219,
            "unit": "instructions"
          },
          {
            "name": "fib (interp)",
            "value": 1740191984,
            "unit": "instructions"
          },
          {
            "name": "tak (jit)",
            "value": 947347778,
            "unit": "instructions"
          },
          {
            "name": "tak (interp)",
            "value": 2138622188,
            "unit": "instructions"
          },
          {
            "name": "nqueens (jit)",
            "value": 1039532519,
            "unit": "instructions"
          },
          {
            "name": "nqueens (interp)",
            "value": 2858829057,
            "unit": "instructions"
          },
          {
            "name": "nqueens GC pause",
            "value": 345,
            "unit": "words"
          },
          {
            "name": "bintrees (jit)",
            "value": 3603058499,
            "unit": "instructions"
          },
          {
            "name": "bintrees (interp)",
            "value": 9052724575,
            "unit": "instructions"
          },
          {
            "name": "bintrees GC pause",
            "value": 393138,
            "unit": "words"
          },
          {
            "name": "hof (jit)",
            "value": 1579761914,
            "unit": "instructions"
          },
          {
            "name": "hof (interp)",
            "value": 3093908620,
            "unit": "instructions"
          },
          {
            "name": "hof GC pause",
            "value": 1579648,
            "unit": "words"
          },
          {
            "name": "qsort (jit)",
            "value": 1186925662,
            "unit": "instructions"
          },
          {
            "name": "qsort (interp)",
            "value": 3771617474,
            "unit": "instructions"
          },
          {
            "name": "mandel (jit)",
            "value": 928018059,
            "unit": "instructions"
          },
          {
            "name": "mandel (interp)",
            "value": 2721061050,
            "unit": "instructions"
          },
          {
            "name": "hash (jit)",
            "value": 485567399,
            "unit": "instructions"
          },
          {
            "name": "hash (interp)",
            "value": 705627136,
            "unit": "instructions"
          },
          {
            "name": "hash GC pause",
            "value": 229395,
            "unit": "words"
          },
          {
            "name": "orgparse (jit)",
            "value": 361415606,
            "unit": "instructions"
          },
          {
            "name": "orgparse (interp)",
            "value": 1394030426,
            "unit": "instructions"
          },
          {
            "name": "callbacks (jit)",
            "value": 541894032,
            "unit": "instructions"
          },
          {
            "name": "callbacks (interp)",
            "value": 1553312818,
            "unit": "instructions"
          },
          {
            "name": "callbacks GC pause",
            "value": 327538,
            "unit": "words"
          },
          {
            "name": "numstr (jit)",
            "value": 240739644,
            "unit": "instructions"
          },
          {
            "name": "numstr (interp)",
            "value": 639222564,
            "unit": "instructions"
          },
          {
            "name": "tasks (jit)",
            "value": 252219663,
            "unit": "instructions"
          },
          {
            "name": "tasks (interp)",
            "value": 127478072,
            "unit": "instructions"
          },
          {
            "name": "editor startup (jit)",
            "value": 135357497,
            "unit": "instructions"
          },
          {
            "name": "editor startup (interp)",
            "value": 132452994,
            "unit": "instructions"
          },
          {
            "name": "keys: type (jit)",
            "value": 813347,
            "unit": "instructions per key"
          },
          {
            "name": "keys: type-long-line (jit)",
            "value": 813622,
            "unit": "instructions per key"
          },
          {
            "name": "keys: move (jit)",
            "value": 766760,
            "unit": "instructions per key"
          },
          {
            "name": "keys: move-modal (jit)",
            "value": 703267,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-open (jit)",
            "value": 18486173,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-first-key (jit)",
            "value": 21887776,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-narrow (jit)",
            "value": 13620114,
            "unit": "instructions per key"
          },
          {
            "name": "keys: search-delete (jit)",
            "value": 1197908,
            "unit": "instructions per key"
          },
          {
            "name": "keys: command-first-key (jit)",
            "value": 1326763,
            "unit": "instructions per key"
          },
          {
            "name": "http: plaintext request (jit)",
            "value": 6458,
            "unit": "instructions per request"
          },
          {
            "name": "http: plaintext request (interp)",
            "value": 7449,
            "unit": "instructions per request"
          },
          {
            "name": "http: /work request (jit)",
            "value": 217497,
            "unit": "instructions per request"
          },
          {
            "name": "http: /work request (interp)",
            "value": 630131,
            "unit": "instructions per request"
          }
        ]
      }
    ]
  }
}