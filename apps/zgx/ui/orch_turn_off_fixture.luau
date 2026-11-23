-- The Control room's page, with each of account/orchestration's parts read
-- on the page: for turning Orchestrators off while they're all live. Off,
-- the window sets the shared orchestration value to nil
-- (src/orchestrators.rs), and nothing reading it may fail.

ui.theme.strict("warn")
require("@account/plugins").set({ orchestrators = true })
local ds = require("@ds")
ds.setup()
require("@app/screens/shell/route").launched:set(true)
require("@app/control_world")()
local orchestration = require("@account/orchestration")
local page = require("@app/screens/control/page")
return ui.column {
	w_full = true,
	h_full = true,
	bg = "$background",
	ui.text { id = "orch-parts", function(): string
		local s = orchestration.settings:get()
		return `{#orchestration.orchestrators:get()} {#orchestration.dumps:get()} {#orchestration.asks:get()} {s.crew_idle_hours}`
	end },
	page.Page(),
}
