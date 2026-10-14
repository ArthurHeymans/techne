// Steady state: run the program's `run` N times in one isolate, printing its
// result once and each iteration's microseconds on one line.
const N = Number(process.env.ITERS || 20), times = []; let r;
for (let i = 0; i < N; i++) { const t0 = process.hrtime.bigint(); r = run(); times.push(Number((process.hrtime.bigint() - t0) / 1000n)); }
console.log(String(r)); console.log(times.join(" "));
