const n = 200000, h = new Map(), s = new Map();
for (let i = 0; i < n; i++) h.set(i, 2 * i);
for (let i = 0; i < n; i++) s.set(String(i), i);
let t = 0; for (let i = 0; i < n; i++) t += h.get(i) + s.get(String(i));
console.log(t); console.log(h.size + s.size);
