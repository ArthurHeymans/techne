xs = list(range(200000))
def rnd(k): return sum(filter(lambda x: x % 3 == 0, map(lambda x: x * x + k, xs)))
print(sum(rnd(n) for n in range(10, 0, -1)))
