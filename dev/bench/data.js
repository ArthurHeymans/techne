window.BENCHMARK_DATA = {
  "lastUpdate": 1791450061162,
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
      }
    ]
  }
}