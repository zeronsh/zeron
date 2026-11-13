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
-- The live one's small world: one machine, the Brain dump, and Tally with
-- its two crew there.
ui.command { id = "map_small", title = "Map: a small world", run = function()
	local t = os.time()
	local desk = { id = "dev-desk", name = "honeypot", platform = "linux", here = true, online = true }
	sessions.now:set({
		state = "ready",
		here = "dev-desk",
		devices = { desk },
		projects = { { id = "p-tally", name = "tally", device = "dev-desk", path = "/home/you/tally", git = true } },
		chats = {
			{ id = "orc-brain", title = "Brain dump", device = "dev-desk", updated = t - 120, created = t - 9000, agent = "claude" },
			{ id = "orc-tally", title = "Tally", device = "dev-desk", project = "p-tally", status = "working", updated = t - 20, created = t - 3000, agent = "claude",
				activity = { kind = "delegating", label = "", since = t - 15 } },
		},
		archived = {},
		children = {
			["orc-tally"] = {
				{ id = "th-lines", parent = "orc-tally", title = "tally --lines", device = "dev-desk", project = "p-tally", child_kind = "crew", status = "done",
					updated = t - 400, created = t - 2400, agent = "claude", todo = { done = 3, total = 3 } },
				{ id = "th-tests", parent = "orc-tally", title = "tally tests", device = "dev-desk", project = "p-tally", child_kind = "crew", status = "working",
					updated = t - 5, created = t - 1200, agent = "claude", activity = { kind = "thinking", label = "", since = t - 40 }, todo = { done = 1, total = 4, current = "empty input" } },
			},
		},
		places = { pinned = {}, sections = {} },
		places_ready = true,
	} :: any)
	local function orch(id: string, name: string, hue: number, kind: string?): any
		return { id = id, kind = kind or "standard", name = name, goal = "", rules = "", crew_rules = "", notes = "", hue = hue,
			crew = { devices = {} }, paused = false, archived = false, created = t - 9000, updated = t - 60, project = if kind then nil else "p-tally" }
	end
	orchestration.now:set({
		state = "ready",
		orchestrators = { orch("orc-brain", "Brain dump", 172, "brain_dump"), orch("orc-tally", "Tally", 236) },
		crew = {
			{ chat_id = "th-lines", orchestrator_id = "orc-tally", role = "", released = false, joined = t - 2400 },
			{ chat_id = "th-tests", orchestrator_id = "orc-tally", role = "", released = false, joined = t - 1200 },
		},
		items = {},
		asks = {},
		dumps = {},
	})
	orchestration.loads:set({ ["dev-desk"] = { cpu = 0.07, mem_used = 17e9, mem_total = 67e9, online = true, at = t } })
end }
-- A report coming in (a pulse along a rail).
ui.command { id = "fixture.report", title = "Fixture: a crew report", run = function()
	local n = #orchestration.comms:peek()
	orchestration.comms:push({ id = `cf-x{n}`, kind = "report", from = "th-ui-control", to = "orc-orch", summary = "Map tests pass", at = os.time() })
end }
-- A thread's activity moving twice within 300 ms: the first is held.
-- A thread on another machine (an older host: no activity, no todo ever)
-- working, and later its turn over: `status` alone says so.
local function set_status(id: string, st: string?)
	local n: any = table.clone(sessions.now:peek())
	local function fix(list: { any }): { any }
		local out = {}
		for _, c in list do
			if c.id == id then
				c = table.clone(c)
				c.status = st
				c.activity = nil
				c.todo = nil
			end
			table.insert(out, c)
		end
		return out
	end
	n.chats = fix(n.chats or {})
	local kids = {}
	for k, list in n.children or {} do
		kids[k] = fix(list)
	end
	n.children = kids
	sessions.now:set(n)
end
ui.command { id = "fixture.remote_working", title = "Fixture: a remote thread works", run = function()
	set_status("th-dev-names", "working")
	set_status("orc-brain", "working")
end }
ui.command { id = "fixture.remote_idle", title = "Fixture: a remote thread's turn ends", run = function()
	set_status("th-dev-names", nil)
	set_status("orc-brain", nil)
end }
-- Its row flickering as its turn ends (the host's last frames out of
-- order): idle, working again for a blink, idle for good.
ui.command { id = "fixture.remote_flicker", title = "Fixture: a remote thread's turn ends, flickering", run = function()
	set_status("th-dev-names", nil)
	set_status("orc-brain", nil)
	ui.after(40, function()
		set_status("th-dev-names", "working")
		set_status("orc-brain", "working")
	end)
	ui.after(80, function()
		set_status("th-dev-names", nil)
		set_status("orc-brain", nil)
	end)
end }
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
