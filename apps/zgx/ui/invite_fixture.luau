-- Invite your teammates, mounted on its own, for
-- tests/invite_in_place.test.luau: who was sent one is the screen's own
-- state (`invited`), set here as a send that worked sets it. Here, beside
-- app.luau, as a mounted script loads modules from its own directory down.

ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()
-- Addresses plain to start (hiding is the app's default): the tests read
-- them, and private_text's turn hiding on themselves.
ds.PrivateText.hidden:set(false)
local invite = ui.component(require("@screens/onboarding/invite"))

return ui.column {
	w_full = true,
	h_full = true,
	ds.Onboarding.Frame {
		invite {
			step = 3,
			count = 4,
			on_back = function() end,
			on_continue = function() end,
		},
	},
}
