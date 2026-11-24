-- The Control room's page alone (screens/control/page) on the window's
-- material, drawn from account/orchestration_sample: for the tests that
-- look at its widgets without the shell around them.

ui.theme.strict("warn")
-- (Orchestrators on, as these are its: off is the default everywhere.)
require("@account/plugins").set({ orchestrators = true })
local ds = require("@ds")
ds.setup()
require("@app/screens/shell/route").launched:set(true)
require("@app/control_world")()
local page = require("@app/screens/control/page")
-- A toast, as the board's actions say one (tests of where the stack stands).
ui.command { id = "say_toast", title = "Show a toast", run = function()
	ds.toast.show { title = "Marked done", body = "A card", tone = "success" }
end }
return ui.column {
	w_full = true,
	h_full = true,
	bg = "$background",
	page.Page(),
	ds.toast.Host {},
}
