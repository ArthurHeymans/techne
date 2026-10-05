local function it(cr, ci, mx)
  local zr, zi, i = 0.0, 0.0, 0
  while i ~= mx and zr*zr + zi*zi <= 4.0 do zr, zi = zr*zr - zi*zi + cr, 2.0*zr*zi + ci i = i + 1 end
  return i
end
local S, s = 250, 0
for y = 0, S - 1 do for x = 0, S - 1 do s = s + it(3.0 * (x / S) - 2.0, 2.0 * (y / S) - 1.0, 200) end end
print(s)
