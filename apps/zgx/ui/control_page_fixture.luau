-- The Control room's page alone (screens/control/page) on the window's
-- material, drawn from account/orchestration_sample: for the tests that
-- look at its widgets without the shell around them.

ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()
require("@app/screens/shell/route").launched:set(true)
require("@app/control_world")()
local page = require("@app/screens/control/page")
return ui.column {
	w_full = true,
	h_full = true,
	bg = "$background",
	page.Page(),
}
