-- LaTeX math in a reply (ds/math through ds/markdown), mounted on its own
-- for tests/math_in_place.test.luau. A test host has no `math.layout`
-- (src/math.rs), so a box of the formula's size stands in for each layout:
-- what's under test is the page's side (which formulas, where, their size,
-- copy and the fallbacks), not zatex's.

ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()
local Markdown = require("@ds/markdown")
local Math = require("@ds/math")

local stub = Math :: any
stub.lay_out = function(tex: string, display: boolean?): any
	if string.find(tex, "\\nosuch", 1, true) then
		return { error = "UnknownCommand", at = 0 }
	end
	local w = #tex * 0.4
	-- An em box of the formula's width: move, three lines, close.
	return { w = w, ascent = if display then 1.2 else 0.8, descent = 0.3, left = 0, path = { 0, 0, 0, 1, w, 0, 1, w, 1, 1, 0, 1, 4 } }
end

local REPLY = table.concat({
	"Euler's identity, $e^{i\\pi} + 1 = 0$, ties five constants; it costs $5 and $10.",
	"",
	"$$\\frac{a}{b} \\\\ \\{x\\}$$",
	"",
	"```math",
	"x + \\nosuch",
	"```",
	"",
	"`$not math$` stays code.",
}, "\n")

ui.command {
	id = "math_off",
	title = "Turn math off",
	run = function()
		Math.render:set(false)
	end,
}

Math.render:set(true)
Math.size:set(100)

return ui.column {
	id = "reply",
	selectable = true,
	w_full = true,
	p = "$space.6",
	Markdown { text = REPLY },
}
