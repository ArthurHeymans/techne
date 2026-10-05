local tags, levels, todos, opn, done, words, ntags = {}, {0,0,0,0,0,0,0}, 0, 0, 0, 0, 0
local sub, byte = string.sub, string.byte
local STAR, SP, COLON = byte("*"), byte(" "), byte(":")
for line in io.lines("org-sample.org") do
  local len = #line local stars = 0
  while stars < len and byte(line, stars + 1) == STAR do stars = stars + 1 end
  if stars > 0 and stars < 8 and stars < len and byte(line, stars + 1) == SP then
    levels[stars] = levels[stars] + 1
    if sub(line, stars + 2, stars + 6) == "TODO " then todos = todos + 1 end
    if byte(line, len) == COLON then
      local i = len
      while i >= 1 and byte(line, i) ~= SP do i = i - 1 end
      if i >= 1 then
        local start = i + 2
        for j = i + 2, len do
          if byte(line, j) == COLON then
            if j > start then local t = sub(line, start, j - 1)
              if not tags[t] then ntags = ntags + 1 end tags[t] = (tags[t] or 0) + 1 end
            start = j + 1
          end
        end
      end
    end
  elseif sub(line, 1, 6) == "- [ ] " then opn = opn + 1
  elseif sub(line, 1, 6) == "- [X] " then done = done + 1
  else
    local inw = false
    for i = 1, len do local sp = byte(line, i) == SP if not sp and not inw then words = words + 1 end inw = not sp end
  end
end
for _, x in ipairs({levels[1], levels[2], levels[3], todos, opn, done, words, ntags, tags["work"] or 0}) do print(x) end
