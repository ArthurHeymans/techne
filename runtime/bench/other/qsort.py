import sys; sys.setrecursionlimit(1000000)
n = 300000; v = [0] * n; seed = 42
for i in range(n): v[i] = seed; seed = (seed * 75 + 74) % 65537
def partition(lo, hi):
    p = v[hi]; i = lo
    for j in range(lo, hi):
        if v[j] < p: v[i], v[j] = v[j], v[i]; i += 1
    v[i], v[hi] = v[hi], v[i]; return i
def qs(lo, hi):
    while lo < hi:
        p = partition(lo, hi); qs(lo, p - 1); lo = p + 1
qs(0, n - 1)
print("#t" if all(v[i] <= v[i+1] for i in range(n-1)) else "#f"); print(v[0]); print(v[n//2]); print(v[-1])
