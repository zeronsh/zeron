-- Setup's way on with choices ("Continue in …"), for
-- tests/forward_in_place.test.luau: its words as wide as the widest, and
-- each choice's words following its text in place. The choices are
-- `test_forward_choices`, the one picked `test_forward_pick`. Here, beside
-- app.luau, as a mounted script loads modules from its own directory down.

ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()

local choices = ui.state("test_forward_choices", { "Continue in Halcyon Labs", "Continue in Northwind Robotics" })
local pick = ui.state("test_forward_pick", 1)

return ui.column {
	w_full = true,
	h_full = true,
	p = "$space.4",
	items = "start",
	ds.Onboarding.Forward {
		label = "Continue",
		choices = function(): { string }
			return choices:get()
		end,
		choice = function(): number
			return pick:get()
		end,
		max_w = 400,
	},
}
