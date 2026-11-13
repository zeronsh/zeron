-- The Control room with its Q&A (control_qa_world), loaded as the shell
-- loads a screen: required while a component is being built
-- (screens/shell/boundary), so what the page makes at its top level is
-- made inside that build. `qa_screen_shown` takes the screen away and
-- brings it back, as going to a thread and back does.

ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()
require("@app/screens/shell/route").launched:set(true)
require("@app/control_world")()
require("@app/control_qa_world")()

local shown = ui.state("qa_screen_shown", true)

local Screen = ui.component(function(): Node
	local page = require("@app/screens/control/page")
	return page.Page()
end)

return ui.column {
	w_full = true,
	h_full = true,
	bg = "$background",
	ui.show(function(): boolean
		return shown:get()
	end, ui.column { w_full = true, h_full = true, Screen {} }),
	ds.toast.Host {},
}
