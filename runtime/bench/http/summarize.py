"""Markdown tables from a run.sh results.tsv: python3 summarize.py results/X/results.tsv"""

import csv
import sys
from collections import defaultdict

with open(sys.argv[1]) as f:
    rows = list(csv.DictReader(f, delimiter="\t"))
impls = list(dict.fromkeys(r["impl"] for r in rows))


def table(title, keep, column, values, cell):
    sel = defaultdict(dict)
    for r in filter(keep, rows):
        sel[r["impl"]][r[column]] = r
    print(f"### {title}\n")
    print("| impl | " + " | ".join(values) + " |")
    print("|---" * (len(values) + 1) + "|")
    for i in impls:
        if i in sel:
            print(f"| {i} | " + " | ".join(cell(sel[i][v]) if v in sel[i] else "" for v in values) + " |")
    print()


def k(r):
    return f"{int(r['rps']) / 1000:.0f}k"


def plain(r, threads=None, conns=None, idle="0"):
    return (
        r["path"] == "/"
        and r["idle"] == idle
        and (threads is None or r["threads"] == threads)
        and (conns is None or r["conns"] == conns)
    )


table("Plaintext, 256 connections, by server threads (req/s)", lambda r: plain(r, conns="256"), "threads", ["1", "2", "4", "8"], k)
table("/work, 256 connections, by server threads (req/s)", lambda r: r["path"] == "/work", "threads", ["1", "4", "8"], k)
table(
    "Plaintext, 1 thread, by connections (req/s / p99 ms)",
    lambda r: plain(r, threads="1"),
    "conns",
    ["16", "256", "1024", "4096"],
    lambda r: f"{k(r)} / {float(r['p99_ms']):.1f}",
)
table(
    "Plaintext, 1 thread, 16 busy connections, by idle connections (req/s / RSS)",
    lambda r: r["path"] == "/" and r["threads"] == "1" and r["conns"] == "16",
    "idle",
    ["0", "1000", "10000"],
    lambda r: f"{k(r)} / {r['rss_mb']} MB",
)
