function drive(f, seed) {
  let acc = seed;
  for (let i = 1; i <= 1000000; i++) acc = f(acc, i);
  return acc;
}
function run() { return drive((a, b) => a + b, 0.5); }
