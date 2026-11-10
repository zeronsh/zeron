-- Each of `math.inputs` read by ds/math_source's `prepare`, into
-- `math.outputs`: src/math_source.rs holds its Rust read to the same.

local Source = require("@ds/math_source")

local inputs = ui.shared("math.inputs", {} :: { string })
local outputs = ui.shared("math.outputs", nil :: { string }?)
local made: { string } = {}
for i, text in inputs:peek() do
	made[i] = Source.prepare(text)
end
outputs:set(made)

return ui.column {}
