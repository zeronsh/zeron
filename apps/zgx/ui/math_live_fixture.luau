-- LaTeX math in a reply with the real layout (src/math.rs, zatex), for a
-- look in a window: `zeron-zgx fixture=math_live_fixture.luau --inspect`.
-- Commands switch the theme and the math size. (Tests use math_fixture,
-- which stands a box in for each layout: a test host has no `math.layout`.)

ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()
local Markdown = require("@ds/markdown")
local Math = require("@ds/math")

local REPLY = table.concat({
	"The roots of $ax^2 + bx + c$ are",
	"",
	"$$x = \\frac{-b \\pm \\sqrt{b^2 - 4ac}}{2a}$$",
	"",
	"and the Basel problem sums to",
	"",
	"$$\\sum_{n=1}^{\\infty} \\frac{1}{n^2} = \\frac{\\pi^2}{6}, \\quad \\begin{pmatrix} a & b \\\\ c & d \\end{pmatrix}$$",
	"",
	"This one can't be set, so it shows as code:",
	"",
	"$$x + \\nosuchcommand{y}$$",
	"",
	"Inline, $x$ shows as its TeX for now.",
}, "\n")

ui.command { id = "theme_light", title = "Light theme", run = function()
	ui.theme.follow_system(false)
	ui.theme.current:set("zeron-light")
end }
ui.command { id = "theme_dark", title = "Dark theme", run = function()
	ui.theme.follow_system(false)
	ui.theme.current:set("zeron-dark")
end }
ui.command { id = "size_100", title = "Math 100%", run = function()
	Math.size:set(100)
end }
ui.command { id = "size_135", title = "Math 135%", run = function()
	Math.size:set(135)
end }

Math.render:set(true)

return ui.scroll {
	w_full = true,
	h_full = true,
	bg = "$background",
	ui.column {
		id = "reply",
		selectable = true,
		w_full = true,
		p = "$space.6",
		Markdown { text = REPLY },
	},
}
