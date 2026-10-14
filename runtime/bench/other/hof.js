function map(f, l) { const r = new Array(l.length); for (let i = 0; i < l.length; i++) r[i] = f(l[i]); return r; }
function filter(p, l) { const r = []; for (let i = 0; i < l.length; i++) if (p(l[i])) r.push(l[i]); return r; }
function fold(f, a, l) { for (let i = 0; i < l.length; i++) a = f(a, l[i]); return a; }
const xs = []; for (let i = 0; i < 200000; i++) xs.push(i);
let s = 0;
for (let k = 10; k >= 1; k--) s += fold((a, b) => a + b, 0, filter(x => x % 3 === 0, map(x => x * x + k, xs)));
console.log(s);
