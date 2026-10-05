local n, h, s = 200000, {}, {}
for i = 0, n - 1 do h[i] = 2 * i end
for i = 0, n - 1 do s[tostring(i)] = i end
local t = 0 for i = 0, n - 1 do t = t + h[i] + s[tostring(i)] end
local c = 0 for _ in pairs(h) do c = c + 1 end for _ in pairs(s) do c = c + 1 end
print(string.format("%d", t)) print(c)
