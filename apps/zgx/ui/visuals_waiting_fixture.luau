-- A visual whose script is still being read (src/visuals_tests.rs): a test
-- has no engine, so the read waits, as a slow device's does.

local ds = require("@ds")
ds.setup()
local Visual = require("@ds/visual")

return ui.column {
	id = "reply",
	w_full = true,
	Visual { id = "v:waiting", title = "Latency", path = "/nowhere/v_waiting-visual.luau" },
}
