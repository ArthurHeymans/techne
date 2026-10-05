import sys; sys.setrecursionlimit(100000)
def tak(x, y, z): return z if not y < x else tak(tak(x-1, y, z), tak(y-1, z, x), tak(z-1, x, y))
print(sum(tak(22, 16, 8) for _ in range(10)))
