def make(d): return (None, None) if d == 0 else (make(d-1), make(d-1))
def check(t): return 1 if t[0] is None else 1 + check(t[0]) + check(t[1])
M = 16
print(check(make(M + 1)))
long = make(M)
for d in range(4, M + 1, 2):
    print(sum(check(make(d)) for _ in range(2 ** (M - d + 4))))
print(check(long))
