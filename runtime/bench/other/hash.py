n = 200000; h = {}; s = {}
for i in range(n): h[i] = 2 * i
for i in range(n): s[str(i)] = i
print(sum(h[i] + s[str(i)] for i in range(n))); print(len(h) + len(s))
