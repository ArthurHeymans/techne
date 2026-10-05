def ok(row, dist, placed):
    for q in placed:
        if q == row + dist or q == row - dist: return False
        dist += 1
    return True
def tr(x, y, z):
    if not x: return 1 if not y else 0
    return (tr(x[1:] + y, [], [x[0]] + z) if ok(x[0], 1, z) else 0) + tr(x[1:], [x[0]] + y, z)
print(sum(tr(list(range(1, 11)), [], []) for _ in range(10)))
