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
-- The host's calls, noted in `calls` and answered at once.
local calls = ui.state("calls", "")
local function note(what: string)
	calls:set(if calls:peek() == "" then what else `{calls:peek()},{what}`)
end
local function settled(): any
	return { after = function(_self: any, done: any)
		if done then
			(done :: any)(nil)
		end
	end }
end
local stub = orchestration :: any
stub.loads_wanted = function(_on: boolean) end
stub.op = function(op: any): any
	note(`op:{op.op}:{op.chatId or op.id or ""}`)
	return settled()
end
local s_stub = sessions :: any
s_stub.archive = function(id: string)
	note(`archive:{id}`)
end
s_stub.stop = function(id: string): any
	note(`stop:{id}`)
	return settled()
end

-- A chat watched in a page's own composer (Capture's, the Brain dump's):
-- no host lanes here, its session as the host would give it.
local lanes = require("@app/screens/chat/lanes") :: any
lanes.watch = function(slot: string, id: string?)
	if not id then
		return
	end
	lanes.view(slot).session:set({ id = id, state = "ready", device = "dev-desk", running = false })
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
-- The live one's shape: three machines; the Brain dump and an orchestrator
-- on this one; threads the Brain dump started on the others (one with a
-- long name), and one it handed a part to.
local function map_live()
	local t = os.time()
	local here = { id = "dev-hp1", name = "honeypot-1", platform = "linux", here = true, online = true }
	local hp = { id = "dev-hp", name = "honeypot", platform = "linux", online = true }
	local win = { id = "dev-win", name = "DESKTOP-3NFV6JC", platform = "windows", online = true }
	-- (One with nothing at work, a rail running past it.)
	local m2a = { id = "dev-m2a", name = "M2A", platform = "macos", online = true }
	sessions.now:set({
		state = "ready",
		here = "dev-hp1",
		devices = { here, hp, win, m2a },
		projects = { { id = "p-zeron", name = "zeron", device = "dev-hp1", path = "/home/you/zeron", git = true } },
		chats = {
			{ id = "orc-brain", title = "Brain dump", device = "dev-hp1", status = "working", updated = t - 30, created = t - 9000, agent = "claude",
				activity = { kind = "thinking", label = "", since = t - 20 } },
			{ id = "orc-fixes", title = "Zeron fixes", device = "dev-hp1", project = "p-zeron", updated = t - 300, created = t - 5000, agent = "claude",
				started_by = "orc-brain" },
			{ id = "th-codex", title = "Codex Slash Commands Missing", device = "dev-hp", status = "done", updated = t - 200, created = t - 3000, agent = "openai",
				started_by = "orc-brain" },
			{ id = "th-capy", title = "Explaining Capy.ai Captains", device = "dev-hp", updated = t - 100, created = t - 2000, agent = "claude" },
			{ id = "th-installer", title = "Windows installer check", device = "dev-win", status = "working", updated = t - 10, created = t - 1500, agent = "claude",
				started_by = "orc-brain", activity = { kind = "running", label = "", since = t - 15 } },
			{ id = "th-notes", title = "Release notes draft", device = "dev-hp1", status = "done", updated = t - 400, created = t - 2500, agent = "claude",
				started_by = "orc-brain" },
		},
		archived = {},
		children = {},
		places = { pinned = {}, sections = {} },
		places_ready = true,
	} :: any)
	local function orch(id: string, name: string, hue: number, kind: string?): any
		return { id = id, kind = kind or "standard", name = name, goal = "", rules = "", crew_rules = "", notes = "", hue = hue,
			crew = { devices = {} }, paused = false, archived = false, created = t - 9000, updated = t - 60, project = if kind then nil else "p-zeron" }
	end
	orchestration.now:set({
		state = "ready",
		orchestrators = { orch("orc-brain", "Brain dump", 172, "brain_dump"), orch("orc-fixes", "Zeron fixes", 20) },
		crew = {},
		items = {},
		asks = {},
		dumps = { { id = "d1", routes = { { target = { kind = "chat", id = "th-capy" } } } } },
	})
	orchestration.loads:set({
		["dev-hp1"] = { cpu = 0.3, mem_used = 17e9, mem_total = 67e9, online = true, at = t },
		["dev-hp"] = { cpu = 0.1, mem_used = 7e9, mem_total = 32e9, online = true, at = t },
		["dev-win"] = { cpu = 0.05, mem_used = 7e9, mem_total = 32e9, online = true, at = t },
	})
end
ui.command { id = "map_live", title = "Map: the live shape", run = map_live }

-- The Map's "Show chats" (ds/orch_map_visibility): one idle crew thread
-- (Code signing, on forge: idle, its card done) moved as the engine would move it.
local function touch(id: string, change: (any) -> ())
	local now = table.clone(sessions.now:peek() :: any)
	local function edit(list: { any }): { any }
		local out = {}
		for _, c in list do
			if c.id == id then
				c = table.clone(c)
				change(c)
			end
			table.insert(out, c)
		end
		return out
	end
	now.chats = edit(now.chats)
	local kids = {}
	for parent, list in now.children or {} do
		kids[parent] = edit(list)
	end
	now.children = kids
	sessions.now:set(now)
end
-- Its last message two hours ago: past "recent"'s window.
ui.command { id = "sign_old", title = "Signing: idle two hours", run = function()
	touch("th-reins-sign", function(c)
		c.status, c.updated = nil, os.time() - 7200
	end)
end }
-- A message in (or out): moved now.
ui.command { id = "sign_message", title = "Signing: a message", run = function()
	touch("th-reins-sign", function(c)
		c.updated = os.time()
	end)
end }
-- A turn starts.
ui.command { id = "sign_turn", title = "Signing: a turn", run = function()
	touch("th-reins-sign", function(c)
		c.status, c.updated = "working", os.time()
	end)
end }
-- Done, its output not read yet.
ui.command { id = "sign_unread", title = "Signing: unread", run = function()
	touch("th-reins-sign", function(c)
		c.status = "done"
	end)
end }
-- An open ask of its.
ui.command { id = "sign_ask", title = "Signing: asks", run = function()
	local now = table.clone(orchestration.now:peek() :: any)
	local asks = table.clone(now.asks or {})
	table.insert(asks, { id = "ask-sign", from_chat = "th-reins-sign", kind = "question", question = "Sign with the new key?", state = "open", created = os.time() })
	now.asks = asks
	orchestration.now:set(now)
end }
-- Its card in Review.
ui.command { id = "sign_review", title = "Signing: in Review", run = function()
	local now = table.clone(orchestration.now:peek() :: any)
	local items = {}
	for _, it in now.items or {} do
		if it.id == "it-sign" then
			it = table.clone(it)
			it.state = "review"
			it.threads = { "th-reins-sign" }
		end
		table.insert(items, it)
	end
	now.items = items
	orchestration.now:set(now)
end }
-- A big fleet (the new Graph's perf and shots): four machines, 104
-- agents (4 orchestrators, 100 threads), the VPN orchestrator with 51 crew (46 on the desk, 5 on the
-- MacBook), the Brain dump's on the cloud box. Deterministic.
local BIG_WORDS = { "Exporter image", "WireGuard keys", "Peer sync", "Admin app login", "Moscow relay", "DE node health", "Backup nightly", "Route table", "DNS split", "Metrics push", "Cert rotate", "Mesh join", "Firewall rules", "Client config", "UI polish", "Throughput test" }
local BIG_STATES = { "working", "idle", "working", "waiting", "idle", "done", "working", "failed", "idle", "input" }
local BIG_AGENTS = { { "claude", "claude-opus-5-5" }, { "openai", "gpt-5.5-codex" }, { "claude", "claude-sonnet-5-5" }, { "gemini", "gemini-3-pro" }, { "cursor", "composer-2" } }
local big_extra = 0
local big_stale = false
local function map_big()
	local t = os.time()
	local devices = {
		{ id = "dev-desk", name = "dev-desk", platform = "linux", here = true, online = true },
		{ id = "dev-mac", name = "macbook", platform = "macos", online = true },
		{ id = "dev-win", name = "winbox", platform = "windows", online = true },
		{ id = "dev-cloud", name = "Cloud", platform = "cloud", cloud = true, online = true },
	}
	local chats, crew = {}, {}
	local k = 0
	local function chat(id: string, title: string, device: string): any
		k += 1
		local agent = BIG_AGENTS[(k - 1) % #BIG_AGENTS + 1]
		local c = { id = id, title = title, device = device, status = BIG_STATES[(k - 1) % #BIG_STATES + 1], updated = t - k, created = t - 9000,
			agent = agent[1], model = agent[2] }
		-- (Stale: some finished ones last moved an hour or more ago, some
		-- forty minutes ago.)
		if big_stale and (c.status == "idle" or c.status == "done") then
			(c :: any).status = nil
			c.updated = if k % 2 == 0 then t - 2 * 3600 else t - 45 * 60
		end
		table.insert(chats, c)
		return c
	end
	local function orch(id: string, name: string, hue: number, device: string, kind: string?): any
		local c = chat(id, name, device)
		c.status = "working"
		return { id = id, kind = kind or "standard", name = name, goal = "", rules = "", crew_rules = "", notes = "", hue = hue,
			crew = { devices = {} }, paused = false, archived = false, created = t - 9000, updated = t - 60 }
	end
	local function members(o: string, prefix: string, n: number, device: string)
		for i = 1, n do
			local id = `{prefix}{i}`
			chat(id, BIG_WORDS[(i - 1) % #BIG_WORDS + 1] .. (if i > #BIG_WORDS then ` {i // #BIG_WORDS + 1}` else ""), device)
			table.insert(crew, { orchestrator_id = o, chat_id = id, released = false })
		end
	end
	local os_ = {
		orch("orc-vpn", "Exnomic VPN", 200, "dev-desk"),
		orch("orc-ui", "Control room UI", 320, "dev-mac"),
		orch("orc-eng", "Engine", 40, "dev-win"),
		orch("orc-brain", "Brain dump", 140, "dev-cloud", "brain_dump"),
	}
	members("orc-vpn", "vpn", 46, "dev-desk")
	members("orc-vpn", "vpnm", 5, "dev-mac")
	members("orc-ui", "ui", 16, "dev-mac")
	members("orc-eng", "eng", 12, "dev-win")
	members("orc-brain", "bd", 6, "dev-cloud")
	members("orc-eng", "engx", big_extra, "dev-win")
	for i = 1, 12 do
		chat(`own{i}`, `Own thread {i}`, "dev-desk").status = if i % 3 == 0 then "done" else "working"
	end
	for i = 1, 3 do
		chat(`ownw{i}`, `Side quest {i}`, "dev-win").status = "waiting"
	end
	sessions.now:set({
		state = "ready", here = "dev-desk", devices = devices, projects = {}, chats = chats, archived = {}, children = {},
		places = { pinned = {}, sections = {} }, places_ready = true,
	} :: any)
	orchestration.now:set({ state = "ready", orchestrators = os_, crew = crew, items = {}, asks = {},
		dumps = { { id = "d1", routes = { { target = { kind = "orchestrator", id = "orc-eng" } } } } } })
	orchestration.loads:set({
		["dev-desk"] = { cpu = 0.38, mem_used = 17e9, mem_total = 67e9, online = true, at = t },
		["dev-mac"] = { cpu = 0.83, mem_used = 7e9, mem_total = 32e9, online = true, at = t },
		["dev-win"] = { cpu = 0.77, mem_used = 7e9, mem_total = 32e9, online = true, at = t },
		["dev-cloud"] = { cpu = 0.24, mem_used = 7e9, mem_total = 32e9, online = true, at = t },
	})
end
-- The force Graph's fleet: the big one with 60 more Engine crew (160
-- threads, 164 agents), and messages between some of its chats lately
-- (weak links; the last in flight).
ui.command { id = "map_force_big", title = "Map: a big fleet that talks", run = function()
	big_extra = 60
	big_stale = false
	map_big()
	local t = os.time()
	local list = {}
	local pairs_ = { { "vpn3", "vpn9" }, { "ui4", "vpn12" }, { "eng2", "ui7" }, { "bd1", "eng5" }, { "vpn20", "orc-ui" }, { "engx4", "vpn33" } }
	for k, pr in pairs_ do
		table.insert(list, { id = `fc-{k}`, kind = "report", from = pr[1], to = pr[2], summary = "", at = if k == #pairs_ then t else t - 600 })
	end
	orchestration.comms:set(list)
end }
for _, z in { { "in", 1.25 }, { "out", 0.8 } } do
	ui.command { id = `map_force_zoom_{z[1]}`, title = `Map: zoom the force graph {z[1]}`, run = function()
		require("@ds/orch_force").zoom_by(z[2] :: number)
	end }
end
for _, by in { "orchestrator", "machine" } do
	ui.command { id = `map_force_color_{by}`, title = `Map: the force graph coloured by {by}`, run = function()
		require("@ds/orch_force").color_by:set(by)
	end }
end
-- A drag held a frame at a time: a hub (the Engine, 72 crew), a chat;
-- then let go.
for _, d in { { "hub", "orc-eng" }, { "chat", "vpn3" }, { "brainhub", "orc-orch" } } do
	ui.command { id = `map_force_drag_{d[1]}_step`, title = `Map: drag the force graph's {d[1]} a step`, run = function()
		require("@ds/orch_force").drag_by(d[2], 4, 2)
	end }
	ui.command { id = `map_force_drag_{d[1]}_end`, title = `Map: let go of the force graph's {d[1]}`, run = function()
		require("@ds/orch_force").drag_end(d[2])
	end }
end
ui.command { id = "map_force_drag_hub_hold", title = "Map: press on the force graph's hub", run = function()
	require("@ds/orch_force").drag_by("orc-eng", 0, 0)
end }
-- Every node's place now, into `force_places` (tests read it).
local force_places = ui.state("force_places", {} :: any)
ui.command { id = "map_force_read", title = "Map: read the force graph's places", run = function()
	local cur = require("@ds/orch_force").current
	local out = {}
	if cur then
		for i = 1, cur.sim.n do
			out[cur.sim.ids[i]] = { cur.sim.x[i], cur.sim.y[i] }
		end
	end
	force_places:set(out)
end }
-- A live push: one crew chat's state changes (input, then working, …).
local push_n = 0
ui.command { id = "map_force_push", title = "Map: a live push, one chat's state", run = function()
	push_n += 1
	local n: any = table.clone(sessions.now:peek())
	local list = {}
	for _, c in n.chats do
		local c2 = c
		if c.id == "vpn5" then
			c2 = table.clone(c)
			c2.status = if push_n % 2 == 1 then "input" else "working"
		end
		table.insert(list, c2)
	end
	n.chats = list
	sessions.now:set(n)
end }
-- Capture's composer's chips (the Brain dump's, "orc-brain"), into
-- `brain_cargo` (tests read them).
local brain_cargo = ui.state("brain_cargo", {} :: any)
ui.command { id = "map_read_cargo", title = "Map: read Capture's chips", run = function()
	local out = {}
	for _, item in require("@app/screens/chat/shared").peek_cargo("orc-brain") do
		table.insert(out, { kind = item.kind, name = item.name, id = item.ref and item.ref.id })
	end
	brain_cargo:set(out)
end }
ui.command { id = "map_force_pan", title = "Map: pan the force graph", run = function()
	require("@ds/orch_force").pan_by(-40, -25)
end }
-- A node dragged a little way (an orchestrator: its crew follow).
ui.command { id = "map_force_drag", title = "Map: drag a hub of the force graph", run = function()
	local F = require("@ds/orch_force")
	local cur = F.current
	if not cur then
		return
	end
	local i = cur.model.index["orc-ui"]
	if i then
		F.drag_to("orc-ui", cur.sim.x[i] + 60, cur.sim.y[i] + 40)
	end
end }
ui.command { id = "map_big", title = "Map: a big fleet", run = function()
	big_extra = 0
	big_stale = false
	map_big()
end }
-- The big fleet with its finished threads gone quiet: half last moved two
-- hours ago, half forty-five minutes ago (fading).
ui.command { id = "map_big_stale", title = "Map: a big fleet, gone quiet", run = function()
	big_stale = true
	map_big()
end }
-- A quiet thread of the big fleet (gone from the Map) at work again.
ui.command { id = "map_big_wake", title = "Map: a big fleet, a quiet one wakes", run = function()
	local n: any = table.clone(sessions.now:peek())
	local list = {}
	for _, c in n.chats do
		local c2 = table.clone(c)
		if c2.id == "vpn2" then
			c2.status = "working"
			c2.updated = os.time()
		end
		table.insert(list, c2)
	end
	n.chats = list
	sessions.now:set(n)
end }
for _, style in { "dots", "provider" } do
	ui.command { id = `map_graph2_{style}`, title = `Map: the new graph's nodes as {style}`, run = function()
		require("@ds/orch_graph2").node_style:set(style)
	end }
end
for _, mode in { "active", "recent", "all" } do
	ui.command { id = `map_show_{mode}`, title = `Map: show chats {mode}`, run = function()
		require("@ds/orch_map_visibility").mode:set(mode)
	end }
end
-- The machines' loads, moved on (the engine's tick, every few seconds).
local big_tick = 0
ui.command { id = "map_big_loads", title = "Map: a big fleet's loads tick", run = function()
	big_tick += 1
	local t = os.time()
	local out = {}
	for id, l in orchestration.loads:peek() or {} do
		local l2 = table.clone(l)
		l2.cpu = ((l.cpu or 0) + 0.07 * big_tick) % 1
		l2.at = t + big_tick
		out[id] = l2
	end
	orchestration.loads:set(out)
end }
-- One more Engine crew thread on the big fleet (a change of what's shown).
-- A huge fleet (165 threads, the Engine with 72 crew): the Map's bounds
-- and its canvases' bands (tests/orch_map_bounds).
ui.command { id = "map_huge", title = "Map: a huge fleet", run = function()
	big_extra = 60
	big_stale = false
	map_big()
end }
ui.command { id = "map_big_more", title = "Map: a big fleet, one more thread", run = function()
	big_extra += 1
	map_big()
end }
-- A crew report on the big fleet: a dot runs the VPN's comb.
ui.command { id = "map_big_report", title = "Map: a big fleet report", run = function()
	local n = #orchestration.comms:peek()
	orchestration.comms:push({ id = `big-x{n}`, kind = "report", from = "vpn12", to = "orc-vpn", summary = "done", at = os.time() })
end }
-- A copy of the live data, where one was put beside this fixture as
-- orch_live_snapshot.luau (`{ sessions, orchestration }` as the host had
-- them; never kept in the repository): the Graphs' bench on the real
-- shape.
ui.command { id = "map_snapshot", title = "Map: the live data's copy", run = function()
	local ok, snap = pcall(require, "@app/orch_live_snapshot")
	if not ok or type(snap) ~= "table" then
		return
	end
	sessions.now:set(snap.sessions)
	orchestration.now:set(snap.orchestration)
end }
-- The new Graph's view moved, as a drag or a ⌘/Ctrl-scroll would.
ui.command { id = "map_graph2_zoom_in", title = "Map: zoom the new graph in", run = function()
	require("@ds/orch_graph2").zoom_by(1.25)
end }
ui.command { id = "map_graph2_zoom_out", title = "Map: zoom the new graph out", run = function()
	require("@ds/orch_graph2").zoom_by(0.8)
end }
ui.command { id = "map_graph_zoom_out", title = "Map: zoom the graph out", run = function()
	require("@ds/orch_map_graph").zoom_by(0.8)
end }
ui.command { id = "map_graph2_pan", title = "Map: pan the new graph", run = function()
	require("@ds/orch_graph2").pan_by(-40, -25)
end }
-- Zeron fixes and its crew as the Map once showed them blank: one named
-- and done, one just created (its title given, its first turn still
-- running), one whose chat isn't heard yet (its role names it), one with
-- no title of its own (the host's placeholder, its item names it), and one
-- with nothing to go by.
ui.command { id = "map_crew_names", title = "Map: crew thread names", run = function()
	local t = os.time()
	local here = { id = "dev-hp1", name = "honeypot-1", platform = "linux", here = true, online = true }
	sessions.now:set({
		state = "ready",
		here = "dev-hp1",
		devices = { here },
		projects = { { id = "p-zeron", name = "zeron", device = "dev-hp1", path = "/home/you/zeron", git = true } },
		chats = {
			{ id = "orc-fixes", title = "Zeron fixes", device = "dev-hp1", project = "p-zeron", status = "working", updated = t - 60, created = t - 5000,
				agent = "claude", activity = { kind = "delegating", label = "", since = t - 30 } },
		},
		archived = {},
		children = {
			["orc-fixes"] = {
				{ id = "th-board", parent = "orc-fixes", title = "Board card declutter", name = "Board card declutter", device = "dev-hp1",
					project = "p-zeron", child_kind = "crew", status = "done", updated = t - 1100, created = t - 4000, agent = "claude" },
				{ id = "th-asks", parent = "orc-fixes", title = "End-of-turn question asks", name = "End-of-turn question asks", device = "dev-hp1",
					project = "p-zeron", child_kind = "crew", status = "working", updated = t - 40, created = t - 40, agent = "claude",
					activity = { kind = "reading", label = "", since = t - 20 } },
				{ id = "th-untitled", parent = "orc-fixes", title = "New side chat", device = "dev-hp1", project = "p-zeron",
					child_kind = "crew", status = "working", updated = t - 30, created = t - 30, agent = "claude" },
			},
		},
		places = { pinned = {}, sections = {} },
		places_ready = true,
	} :: any)
	orchestration.now:set({
		state = "ready",
		orchestrators = { { id = "orc-fixes", kind = "standard", name = "Zeron fixes", goal = "", rules = "", crew_rules = "", notes = "", hue = 20,
			crew = { devices = {} }, paused = false, archived = false, created = t - 5000, updated = t - 60, project = "p-zeron" } },
		crew = {
			{ chat_id = "th-board", orchestrator_id = "orc-fixes", role = "board cards", item_id = "it-board", released = false, joined = t - 4000 },
			{ chat_id = "th-asks", orchestrator_id = "orc-fixes", role = "end-of-turn asks", item_id = "it-asks", released = false, joined = t - 40 },
			{ chat_id = "th-hover", orchestrator_id = "orc-fixes", role = "hover card fix", item_id = "it-hover", released = false, joined = t - 35 },
			{ chat_id = "th-untitled", orchestrator_id = "orc-fixes", role = "", item_id = "it-sidebar", released = false, joined = t - 30 },
			{ chat_id = "9f3a1c7e-crew", orchestrator_id = "orc-fixes", role = "", released = false, joined = t - 25 },
		},
		items = {
			{ id = "it-board", title = "Board card declutter", state = "doing", orchestrator_id = "orc-fixes", order = 1 },
			{ id = "it-asks", title = "Turn-end asks", state = "doing", orchestrator_id = "orc-fixes", order = 2 },
			{ id = "it-hover", title = "Hover cards", state = "doing", orchestrator_id = "orc-fixes", order = 3 },
			{ id = "it-sidebar", title = "Crew threads in sidebar", state = "doing", orchestrator_id = "orc-fixes", order = 4 },
		},
		asks = {},
		dumps = {},
	})
	orchestration.loads:set({ ["dev-hp1"] = { cpu = 0.3, mem_used = 25e9, mem_total = 67e9, online = true, at = t } })
end }
-- The live shape, and Zeron fixes leading crew on M2A and honeypot: its
-- rail and the Brain dump's both run down past the other machines.
ui.command { id = "map_cross", title = "Map: two rails down", run = function()
	map_live()
	local t = os.time()
	local n: any = table.clone(sessions.now:peek())
	n.children = {
		["orc-fixes"] = {
			{ id = "th-diff", parent = "orc-fixes", title = "Diff view horizontal scroll", device = "dev-m2a", child_kind = "crew", status = "working",
				updated = t - 5, created = t - 900, agent = "claude", activity = { kind = "thinking", label = "", since = t - 20 } },
			{ id = "th-scroll", parent = "orc-fixes", title = "Scroll position reset", device = "dev-hp", child_kind = "crew", status = "working",
				updated = t - 8, created = t - 800, agent = "claude", activity = { kind = "thinking", label = "", since = t - 20 } },
		},
	}
	sessions.now:set(n)
	local o: any = table.clone(orchestration.now:peek())
	o.crew = {
		{ chat_id = "th-diff", orchestrator_id = "orc-fixes", role = "", released = false, joined = t - 900 },
		{ chat_id = "th-scroll", orchestrator_id = "orc-fixes", role = "", released = false, joined = t - 800 },
	}
	orchestration.now:set(o)
end }
-- A machine renamed (the live shape's Windows box), as the registry's
-- update after "Rename device" brings it.
ui.command { id = "fixture.rename_win", title = "Fixture: rename the Windows machine", run = function()
	local n: any = table.clone(sessions.now:peek())
	local list = {}
	for _, d in n.devices or {} do
		local d2 = table.clone(d)
		if d2.id == "dev-win" then
			d2.name = "x16-win"
		end
		table.insert(list, d2)
	end
	n.devices = list
	sessions.now:set(n)
end }
-- A zoom in, as a ⌘/Ctrl-scroll gives (the Graph's).
ui.command { id = "map_graph_zoom_in", title = "Map: zoom the graph in", run = function()
	require("@ds/orch_map_graph").zoom_by(1.25)
end }
-- The Topology's two ways to show work across machines (its … menu's
-- "Lines" and "Tags").
for _, mode in { "lines", "tags" } do
	ui.command { id = `map_topo_{mode}`, title = `Map: topology {mode}`, run = function()
		require("@ds/orch_map_topo_style").style:set(mode)
	end }
end
-- A secondary click's menu for the id in `map_menu_id`, as on the Map.
local menu_id = ui.state("map_menu_id", "")
ui.command { id = "map_menu", title = "Map: open a menu", run = function()
	OrchMap.open_menu(menu_id:peek())
end }
-- Choose `map_menu_pick` (a label, a submenu's after "/") from the menu
-- for `map_menu_id`, as a click there would.
local menu_pick = ui.state("map_menu_pick", "")
ui.command { id = "map_menu_choose", title = "Map: choose from a menu", run = function()
	local entries = require("@app/screens/control/map_menu").entries(menu_id:peek()) or {}
	for _, e in entries do
		if e and e.label == menu_pick:peek() and e.on_select then
			e.on_select()
		end
	end
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
-- A chat of no project on another machine, made after the page showed,
-- finished a minute ago; it writes to the Brain dump, and the Brain dump
-- routes a part to a thread not made yet and to one this window doesn't
-- know.
ui.command { id = "fixture.projectless", title = "Fixture: a chat of no project", run = function()
	local t = os.time()
	local n: any = table.clone(sessions.now:peek())
	local chats = table.clone(n.chats or {})
	table.insert(chats, { id = "th-hello", title = "Hello World Agent", device = "dev-forge", updated = t - 60, created = t - 300, agent = "claude", model = "claude-sonnet-5-5" })
	n.chats = chats
	sessions.now:set(n)
	local list = table.clone(orchestration.comms:peek())
	table.insert(list, { id = "cf-h1", kind = "message", from = "th-hello", to = "orc-brain", summary = "Hello World, started on winbox", at = t - 50 })
	table.insert(list, { id = "cf-h2", kind = "route", from = "orc-brain", to = "", summary = "Make device names unique", at = t - 40 })
	table.insert(list, { id = "cf-h3", kind = "route", from = "you", to = "th-not-here", summary = "123 hello world", at = t - 30 })
	orchestration.comms:set(list)
end }
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

-- The Map's "Show chats", for its pictures.
for _, m in { "active", "recent", "all" } do
	ui.command { id = `show_{m}`, title = `Show chats: {m}`, run = function()
		require("@ds/orch_map_visibility").mode:set(m)
	end }
end

return ui.column {
	w_full = true,
	h_full = true,
	bg = "$background",
	page.Page(),
}
