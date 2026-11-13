-- The Control room in the main window (ui/control_fixture.luau's), for the
-- floating composer's pictures (screens/control/page `FloatingCapture`):
-- `fixture.float` scrolls Capture up out of view with a draft in its
-- composer and two files on it (the composer bare, floating at the page's
-- foot); `fixture.float_open` opens it out. (`theme_dark` and `theme_light`,
-- control_world's, wear either theme.)
--
--   control_float@1440x900|!theme_dark|!fixture.float|+400|~bare-dark
--   control_float@1440x900|!theme_light|!fixture.float|!fixture.float_open|+400|~open-light

local built = require("./control_fixture")

ui.command { id = "fixture.float", title = "Fixture: Capture's composer floating", run = function()
	require("@app/screens/chat/lane_tab").draft("orc-brain"):set("Ask vpn whether Moscow's exit is back, then")
	require("@app/screens/chat/shared").attach_files({
		{ name = "wg0.conf", path = "/Users/dev/wg0.conf" },
		{ name = "latency.csv", path = "/Users/dev/latency.csv" },
	}, "orc-brain")
	require("@app/screens/control/page").place:set(900)
end }
ui.command { id = "fixture.float_open", title = "Fixture: the floating composer opened out", run = function()
	require("@app/screens/control/page").float_open:set(true)
end }

return built
