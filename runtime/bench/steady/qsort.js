const n = 300000, v = new Array(n).fill(0);
function partition(lo, hi) {
  const p = v[hi]; let i = lo;
  for (let j = lo; j < hi; j++) if (v[j] < p) { const t = v[i]; v[i] = v[j]; v[j] = t; i++; }
  const t = v[i]; v[i] = v[hi]; v[hi] = t; return i;
}
function qs(lo, hi) { if (lo < hi) { const p = partition(lo, hi); qs(lo, p - 1); qs(p + 1, hi); } }
function run() {
  let seed = 42;
  for (let i = 0; i < n; i++) { v[i] = seed; seed = (seed * 75 + 74) % 65537; }
  qs(0, n - 1);
  let ok = true; for (let i = 0; i < n - 1; i++) if (v[i] > v[i + 1]) ok = false;
  return [ok, v[Math.floor(n / 2)], v[n - 1]].join(" ");
}
