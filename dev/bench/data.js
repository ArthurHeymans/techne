window.BENCHMARK_DATA = {
  "lastUpdate": 1791362199466,
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
      }
    ]
  }
}