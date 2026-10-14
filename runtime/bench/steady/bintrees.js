function make(d) { return d === 0 ? [null, null] : [make(d - 1), make(d - 1)]; }
function check(t) { return t[0] === null ? 1 : 1 + check(t[0]) + check(t[1]); }
const M = 16;
function run() {
  let s = check(make(M + 1));
  const long = make(M);
  for (let d = 4; d <= M; d += 2) for (let i = 0, n = 2 ** (M - d + 4); i < n; i++) s += check(make(d));
  return s + check(long);
}
