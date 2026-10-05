local function make(d) if d == 0 then return {false, false} end return {make(d-1), make(d-1)} end
local function check(t) if not t[1] then return 1 end return 1 + check(t[1]) + check(t[2]) end
local M = 16
print(check(make(M + 1)))
local long = make(M)
for d = 4, M, 2 do local s = 0 for _ = 1, 2 ^ (M - d + 4) do s = s + check(make(d)) end print(s) end
print(check(long))
