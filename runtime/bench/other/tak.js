function tak(x, y, z) { return !(y < x) ? z : tak(tak(x - 1, y, z), tak(y - 1, z, x), tak(z - 1, x, y)); }
let s = 0; for (let k = 0; k < 10; k++) s += tak(22, 16, 8); console.log(s);
