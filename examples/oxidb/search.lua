-- A mix of what a hundred people looking through a catalogue would ask for:
-- filtered pages, a page deeper in, one document, and the top of a sort.
local roles = {"eng", "ops", "sre", "design"}
local regions = {"eu", "us", "apac"}
local paths = {}
math.randomseed(42)
for i = 1, 400 do
  local r = math.random(4)
  if r == 1 then
    paths[i] = string.format("/?role=%s&limit=25&page=%d", roles[math.random(4)], math.random(50))
  elseif r == 2 then
    paths[i] = string.format("/?role=%s&region=%s&limit=25&page=%d", roles[math.random(4)], regions[math.random(3)], math.random(20))
  elseif r == 3 then
    paths[i] = string.format("/doc/%d", math.random(100000) - 1)
  else
    paths[i] = string.format("/?by=score&dir=desc&limit=25&page=%d", math.random(20))
  end
end
local i = 0
request = function()
  i = i + 1
  return wrk.format("GET", paths[(i % #paths) + 1])
end
