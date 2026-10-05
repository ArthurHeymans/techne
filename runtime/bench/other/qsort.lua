local n = 300000 local v = {} local seed = 42
for i = 0, n - 1 do v[i] = seed seed = (seed * 75 + 74) % 65537 end
local function partition(lo, hi)
  local p = v[hi] local i = lo
  for j = lo, hi - 1 do if v[j] < p then v[i], v[j] = v[j], v[i] i = i + 1 end end
  v[i], v[hi] = v[hi], v[i] return i
end
local function qs(lo, hi) if lo < hi then local p = partition(lo, hi) qs(lo, p - 1) qs(p + 1, hi) end end
qs(0, n - 1)
local ok = true for i = 0, n - 2 do if v[i] > v[i+1] then ok = false end end
print(ok and "#t" or "#f") print(v[0]) print(v[math.floor(n / 2)]) print(v[n - 1])
