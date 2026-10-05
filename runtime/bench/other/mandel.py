def it(cr, ci, mx):
    zr = zi = 0.0; i = 0
    while i != mx and zr*zr + zi*zi <= 4.0: zr, zi = zr*zr - zi*zi + cr, 2.0*zr*zi + ci; i += 1
    return i
S = 250
print(sum(it(3.0 * (x / S) - 2.0, 2.0 * (y / S) - 1.0, 200) for y in range(S) for x in range(S)))
