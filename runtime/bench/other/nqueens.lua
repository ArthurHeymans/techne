local function ok(row, dist, z) while z do if z[1] == row + dist or z[1] == row - dist then return false end dist = dist + 1 z = z[2] end return true end
local function append(a, b) if not a then return b end return {a[1], append(a[2], b)} end
local function tr(x, y, z)
  if not x then return y and 0 or 1 end
  local r = 0
  if ok(x[1], 1, z) then r = tr(append(x[2], y), nil, {x[1], z}) end
  return r + tr(x[2], {x[1], y}, z)
end
local s = 0
for _ = 1, 10 do local l = nil for i = 10, 1, -1 do l = {i, l} end s = s + tr(l, nil, nil) end
print(s)
