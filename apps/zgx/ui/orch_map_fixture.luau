-- The Control room's page alone, drawn from account/orchestration_sample's
-- busier world (`fleet`: four machines, four projects, five orchestrators,
-- the last hour of what each thread did): for the Map's tests and shots.
-- No engine; `ui.command`s choose the Map's view and play what the engine
-- does on its own.

ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()
require("@app/screens/shell/route").launched:set(true)

local sessions = require("@account/sessions")
local orchestration = require("@account/orchestration")
local sample = require("@account/orchestration_sample").fleet_at(os.time())

local top, children = {}, {}
for _, c0 in sample.chats do
	local c = table.clone(c0)
	if c.agent == "codex" then
		c.agent = "openai"
	end
	if c.parent then
		children[c.parent] = children[c.parent] or {}
		table.insert(children[c.parent], c)
	else
		table.insert(top, c)
	end
end
sessions.now:set({
	state = "ready",
	here = "dev-desk",
	devices = sample.devices,
	projects = sample.projects,
	chats = top,
	archived = {},
	children = children,
	places = { pinned = {}, sections = {} },
	places_ready = true,
} :: any)
orchestration.now:set(sample.now)
orchestration.loads:set(sample.loads)
orchestration.comms:set(sample.comms)
ui.shared("orchestration.timeline", {}):set(sample.timeline)
local stub = orchestration :: any
stub.loads_wanted = function(_on: boolean) end
stub.op = function(_op: any): any
	return { after = function(_self: any, done: any)
		if done then
			(done :: any)(nil)
		end
	end }
end

local page = require("@app/screens/control/page")
local OrchMap = require("@ds/orch_map")
for _, v in OrchMap.VARIANTS do
	ui.command { id = `map_{v}`, title = `Map: {v}`, run = function()
		OrchMap.set(page.map_view, { variant = v })
	end }
end
ui.command { id = "map_window_60", title = "Map: the last hour", run = function()
	OrchMap.set(page.map_view, { window = 60 })
end }
-- Only the Map shown (shots): the other widgets hidden.
ui.command { id = "map_only", title = "Only the Map", run = function()
	local hidden = {}
	for _, id in page.WIDGETS do
		if id ~= "map" then
			hidden[id] = true
		end
	end
	page.widgets:set({ order = table.clone(page.WIDGETS), hidden = hidden :: any })
end }
-- A report coming in (a pulse along a rail).
ui.command { id = "fixture.report", title = "Fixture: a crew report", run = function()
	local n = #orchestration.comms:peek()
	orchestration.comms:push({ id = `cf-x{n}`, kind = "report", from = "th-ui-control", to = "orc-orch", summary = "Map tests pass", at = os.time() })
end }
-- A thread's activity moving twice within 300 ms: the first is held.
ui.command { id = "fixture.flicker", title = "Fixture: a thread flickers", run = function()
	local function set(kind: string)
		local n = sessions.now:peek()
		local copy: any = table.clone(n)
		local kids: any = table.clone(copy.children or {})
		local list = {}
		for _, c in kids["orc-orch"] do
			if c.id == "th-engine-state" then
				c = table.clone(c)
				c.activity = { kind = kind, label = "", since = os.time() }
			end
			table.insert(list, c)
		end
		kids["orc-orch"] = list
		copy.children = kids
		sessions.now:set(copy)
	end
	set("building")
	ui.after(60, function()
		set("merging")
	end)
end }
ui.command { id = "theme_light", title = "Light theme", run = function()
	ui.theme.follow_system(false)
	ui.theme.current:set("zeron-light")
end }
ui.command { id = "theme_dark", title = "Dark theme", run = function()
	ui.theme.follow_system(false)
	ui.theme.current:set("zeron-dark")
end }

return ui.column {
	w_full = true,
	h_full = true,
	bg = "$background",
	page.Page(),
}
