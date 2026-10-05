# Mirrors the Scheme program's char-level algorithm (no regex/split shortcuts).
tags = {}; levels = [0] * 8; todos = opn = done = words = 0
for line in open("org-sample.org").read().split("\n")[:-1]:
    stars = 0
    while stars < len(line) and line[stars] == "*": stars += 1
    if 0 < stars < 8 and stars < len(line) and line[stars] == " ":
        levels[stars] += 1
        if line[stars + 1:].startswith("TODO "): todos += 1
        if line and line[-1] == ":":
            i = len(line) - 1
            while i >= 0 and line[i] != " ": i -= 1
            if i >= 0:
                start = j = i + 2
                while j < len(line):
                    if line[j] == ":":
                        if j > start: t = line[start:j]; tags[t] = tags.get(t, 0) + 1
                        start = j + 1
                    j += 1
    elif line.startswith("- [ ] "): opn += 1
    elif line.startswith("- [X] "): done += 1
    else:
        inw = False
        for c in line:
            sp = c == " "
            if not sp and not inw: words += 1
            inw = not sp
for x in (levels[1], levels[2], levels[3], todos, opn, done, words, len(tags), tags.get("work", 0)): print(x)
