#!/usr/bin/env python3
"""Render one or more results directories of hyperfine JSON files as a Markdown table (mean wall ms)."""
import json, pathlib, sys
dirs = [pathlib.Path(a) for a in sys.argv[1:]]
progs = [p for p in "startup fib tak nqueens bintrees hof qsort mandel hash orgparse ffi".split() if any((d / f"{p}.json").exists() for d in dirs)]
impls = ["chez", "guile", "luajit", "lua5.4", "python3", "emacs-elc", "emacs-native2", "emacs-native3", "techne", "steel-stock", "steel-fork",
         "fork-engine", "fork-engine-ir", "fork-resumable", "fork-native"]
data = {p: {r["command"]: r for d in dirs if (d / f"{p}.json").exists() for r in json.load(open(d / f"{p}.json"))["results"]} for p in progs}
def cell(p, i):
    r = data[p].get(i)
    if r is None: return "–"
    return f"{r['mean'] * 1000:.0f}" + (f" ±{r['stddev'] * 1000:.0f}" if r["stddev"] and r["stddev"] > 0.1 * r["mean"] else "")
print("| program | " + " | ".join(impls) + " |")
print("|---" * (len(impls) + 1) + "|")
for p in progs: print(f"| {p} | " + " | ".join(cell(p, i) for i in impls) + " |")
