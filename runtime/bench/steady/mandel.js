function it(cr, ci, mx) {
  let zr = 0.0, zi = 0.0, i = 0;
  while (i !== mx && zr * zr + zi * zi <= 4.0) { const t = zr * zr - zi * zi + cr; zi = 2.0 * zr * zi + ci; zr = t; i++; }
  return i;
}
const S = 250;
function run() { let s = 0; for (let y = 0; y < S; y++) for (let x = 0; x < S; x++) s += it(3.0 * (x / S) - 2.0, 2.0 * (y / S) - 1.0, 200); return s; }
