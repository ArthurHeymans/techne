local function tak(x, y, z) if not (y < x) then return z end return tak(tak(x-1,y,z), tak(y-1,z,x), tak(z-1,x,y)) end
local s = 0 for _ = 1, 10 do s = s + tak(22, 16, 8) end print(s)
