window.BENCHMARK_DATA = {
  "lastUpdate": 1791498988178,
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
      }
    ]
  }
}