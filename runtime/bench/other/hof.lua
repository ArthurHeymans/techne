local function map(f, l) local r = {} for i = 1, #l do r[i] = f(l[i]) end return r end
local function filter(p, l) local r = {} for i = 1, #l do if p(l[i]) then r[#r+1] = l[i] end end return r end
local function fold(f, a, l) for i = 1, #l do a = f(a, l[i]) end return a end
local xs = {} for i = 0, 199999 do xs[#xs+1] = i end
local s = 0
for k = 10, 1, -1 do
  s = s + fold(function(a, b) return a + b end, 0, filter(function(x) return x % 3 == 0 end, map(function(x) return x * x + k end, xs)))
end
print(string.format("%d", s))
