-- Settings → Orchestration (screens/settings/orchestration) beside the
-- shell's sidebar (its view menu's Orchestrators group has the quick
-- toggle), for tests/settings_orch.test.luau and its pictures. No engine: an
-- `updateSettings` op is noted in `orch_ops` and saved to a stand-in for the
-- engine's synced row, kept across mounts (`ui.persist`), then published as
-- the engine would.

ui.theme.strict("warn")
-- (Orchestrators on, as these are its: off is the default everywhere.)
require("@account/plugins").set({ orchestrators = true })
local ds = require("@ds")
ds.setup()
local sessions = require("@account/sessions")
local orchestration = require("@account/orchestration")
local route = require("@app/screens/shell/route")
route.launched:set(true)
local Sidebar = require("@app/screens/shell/sidebar")
local page = require("@app/screens/settings/orchestration")

-- The engine's row, as the next launch finds it.
local engine = ui.persist("fake_engine_orch_settings", { crew_auto_archive = true, crew_idle_hours = 4 })

sessions.now:set({
	state = "ready",
	here = "dev-desk",
	devices = { { id = "dev-desk", name = "desk", platform = "linux", here = true, online = true } },
	projects = {},
	chats = {},
	archived = {},
	children = {},
	places = { pinned = {}, sections = {} },
	places_ready = true,
} :: any)

local function publish()
	orchestration.now:set({
		state = "ready",
		orchestrators = {
			{ id = "orc-fix", kind = "standard", name = "Zeron fixes", goal = "g", hue = 28, crew = { devices = {} }, paused = false, archived = false, created = os.time() - 3600, updated = os.time() },
		},
		crew = {},
		items = {},
		asks = {},
		dumps = {},
		settings = table.clone(engine:peek()),
	} :: any)
end
publish()

local ops = ui.state("orch_ops", "")
local function settled(value: any): any
	return {
		after = function(_self: any, done: (any) -> (), _failed: ((any) -> ())?)
			done(value)
		end,
	}
end
;(orchestration :: any).op = function(op: { [string]: any }): any
	local keys: { string } = {}
	for k in op do
		if k ~= "op" then
			table.insert(keys, k)
		end
	end
	table.sort(keys)
	local parts: { string } = {}
	for _, k in keys do
		table.insert(parts, `{k}={tostring(op[k])}`)
	end
	local line = `{op.op}:{table.concat(parts, ",")}`
	ops:set(if ops:peek() == "" then line else `{ops:peek()}\n{line}`)
	if op.op == "updateSettings" then
		local s = table.clone(engine:peek())
		if op.crewAutoArchive ~= nil then
			s.crew_auto_archive = op.crewAutoArchive
		end
		if op.crewIdleHours ~= nil then
			s.crew_idle_hours = op.crewIdleHours
		end
		engine:set(s)
		publish()
		return settled(s)
	end
	return settled({})
end

ui.command { id = "theme_dark", title = "Dark theme", run = function() ui.theme.follow_system(false); ui.theme.current:set("zeron-dark") end }
ui.command { id = "theme_light", title = "Light theme", run = function() ui.theme.follow_system(false); ui.theme.current:set("zeron-light") end }
ui.command { id = "activity_off", title = "Activity view off", run = function() require("@app/screens/shell/activity").on:set(false) end }
-- The window set elsewhere (another device), as the engine would publish it.
ui.command { id = "idle_24", title = "Window 24h", run = function()
	orchestration.idle_hours:set(24)
end }

return ui.row {
	w_full = true,
	h_full = true,
	bg = "$background",
	ui.div { h_full = true, Sidebar.Sidebar { collapsed = ui.state("orch_settings_sidebar_collapsed", false) } },
	ui.div { id = "settings-page", grow = 1, h_full = true, p = "$space.6", page.view() },
	ds.toast.Host {},
}
