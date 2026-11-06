function inc(x) { return x + 1; }
function drive(f, n) {
  let sum = 0;
  for (let i = 0; i < n; i++) sum += inc(i);
  return sum;
}
function run() { return drive(inc, 1000000); }
