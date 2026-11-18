// Steady state: run the program's `run` N times in one isolate, printing its
// result once and each iteration's microseconds on one line.
const N = Number(process.env.ITERS || 20), times = []; let r;
for (let i = 0; i < N; i++) {
  const start = process.hrtime(); r = run();
  const [seconds, ns] = process.hrtime(start); // also supported by Node 8
  times.push(seconds * 1e6 + Math.floor(ns / 1000));
}
console.log(String(r)); console.log(times.join(" "));
