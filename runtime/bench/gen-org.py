#!/usr/bin/env python3
"""Deterministic synthetic Org file for the orgparse benchmark."""
import random, sys
r = random.Random(7)
W = "alpha beta gamma delta firmware coreboot review patch email meeting agent build remote kernel register".split()
T = "work home urgent firmware review email agent remote".split()
out = []
for h in range(int(sys.argv[1]) if len(sys.argv) > 1 else 12000):
    lvl = r.choice([1, 2, 2, 3, 3, 3])
    kw = r.choice(["TODO ", "DONE ", "", ""])
    title = " ".join(r.choice(W) for _ in range(r.randint(2, 7)))
    tags = ":" + ":".join(r.sample(T, r.randint(1, 3))) + ":" if r.random() < 0.6 else ""
    out.append(f"{'*' * lvl} {kw}{title}" + (f" {tags}" if tags else ""))
    for _ in range(r.randint(0, 6)):
        k = r.random()
        if k < 0.15: out.append("- [ ] " + " ".join(r.choice(W) for _ in range(4)))
        elif k < 0.25: out.append("- [X] " + " ".join(r.choice(W) for _ in range(4)))
        else: out.append(" ".join(r.choice(W) for _ in range(r.randint(3, 14))))
open("org-sample.org", "w").write("\n".join(out) + "\n")
