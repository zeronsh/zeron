-- The Control room in the main window (the shell on /control), drawn from
-- account/orchestration_sample with its chats and devices: what
-- tests/control_*.test.luau drive and audit shots take. No engine: the
-- host's calls are ui/control_world's.

ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()
-- (Where the window is: the Control room, as left there.)
ui.state("shell_route_path", "/control")
require("@app/screens/shell/route").launched:set(true)
require("@app/control_world")()
-- (No window chrome to restyle in a test host, as shell_fixture says.)
local actions = require("@app/screens/onboarding/actions");
(actions :: any).apply_scheme = function() end
-- (What the shell around the page asks of the host: Settings' calls, and the
-- composer's catalog, answered here.)
require("./settings_stub")
local store = require("@app/screens/chat/store") :: any
store.catalog_on = function() end
store.load_catalog = function() end
store.catalog:set({
	state = "ready",
	harnesses = { { id = "claude-code", label = "Claude", mark = "claude" } },
	complete = true,
	models = {
		{ value = "claude-code/claude-opus-5-5", label = "Opus 5.5", harness = "claude-code" },
		{ value = "claude-code/claude-haiku-5-5", label = "Haiku 5.5", harness = "claude-code" },
	},
})
local route = require("@app/screens/shell/route")
local shell = require("@app/screens/shell")
local built = shell()
-- Another page (Settings) opened from the page, and the Control room again
-- (as Back takes it there).
ui.command { id = "fixture.leave", title = "Fixture: open Settings", run = function()
	route.settings()
end }
ui.command { id = "fixture.control", title = "Fixture: the Control room", run = function()
	route.control()
end }
ui.after(0, function()
	route.control()
end)
return built
