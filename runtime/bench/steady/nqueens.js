function ok(row, dist, z) { while (z) { if (z[0] === row + dist || z[0] === row - dist) return false; dist++; z = z[1]; } return true; }
function append(a, b) { return a ? [a[0], append(a[1], b)] : b; }
function tr(x, y, z) {
  if (!x) return y ? 0 : 1;
  let r = 0;
  if (ok(x[0], 1, z)) r = tr(append(x[1], y), null, [x[0], z]);
  return r + tr(x[1], [x[0], y], z);
}
function run() { let s = 0; for (let k = 0; k < 10; k++) { let l = null; for (let i = 10; i >= 1; i--) l = [i, l]; s += tr(l, null, null); } return s; }
