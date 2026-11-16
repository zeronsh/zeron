-- The Control room (ui/orch_map_fixture.luau) on a big fleet, for what one
-- change costs: four machines, 100 threads (the VPN orchestrator with 51
-- crew), a Board card for each crew thread, every working one in Progress
-- (all shown) and the Board expanded. For tests/orch_perf.test.luau.
--
--   perf_big           the fleet (deterministic)
--   perf_status        one thread's status changes (working <-> done), as the
--                      engine's push gives it: sessions.now replaced whole
--   perf_progress      one thread's progress row moves (a step)
--   perf_only <id>     only that widget shown (`perf_only_map`, …)
--   perf_read          the run counts (ds/orch_perf_trace) into the state
--                      `perf_counts`; perf_reset sets them to nothing

local root = require("./orch_map_fixture")
local sessions = require("@account/sessions")
local orchestration = require("@account/orchestration")
local page = require("@app/screens/control/page")

local WORDS = { "Exporter image", "WireGuard keys", "Peer sync", "Admin app login", "Moscow relay", "DE node health", "Backup nightly", "Route table", "DNS split", "Metrics push", "Cert rotate", "Mesh join", "Firewall rules", "Client config", "UI polish", "Throughput test" }
local STATES = { "working", "idle", "working", "waiting", "idle", "done", "working", "failed", "idle", "input" }
local AGENTS = { { "claude", "claude-opus-5-5" }, { "openai", "gpt-5.5-codex" }, { "claude", "claude-sonnet-5-5" } }

local T = os.time()
local flips = 0

local function big()
	local t = T
	local devices = {
		{ id = "dev-desk", name = "dev-desk", platform = "linux", here = true, online = true },
		{ id = "dev-mac", name = "macbook", platform = "macos", online = true },
		{ id = "dev-win", name = "winbox", platform = "windows", online = true },
		{ id = "dev-cloud", name = "Cloud", platform = "cloud", cloud = true, online = true },
	}
	local chats, crew, items = {}, {}, {}
	local k = 0
	local function chat(id: string, title: string, device: string): any
		k += 1
		local agent = AGENTS[(k - 1) % #AGENTS + 1]
		local c = { id = id, title = title, device = device, status = STATES[(k - 1) % #STATES + 1], updated = t - k, created = t - 9000,
			agent = agent[1], model = agent[2], todo = { done = k % 5, total = 5 } }
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
			local c = chat(id, WORDS[(i - 1) % #WORDS + 1] .. (if i > #WORDS then ` {i // #WORDS + 1}` else ""), device)
			table.insert(crew, { orchestrator_id = o, chat_id = id, released = false })
			local state = if c.status == "done" then "review" elseif c.status == "input" or c.status == "failed" then "blocked" else "active"
			table.insert(items, { id = `it-{id}`, orchestrator_id = o, title = c.title, state = state, priority = if i % 7 == 0 then "high" else "normal",
				order = i, threads = { id }, created = t - 9000 + i, updated = t - i * 10 })
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
	orchestration.now:set({ state = "ready", orchestrators = os_, crew = crew, items = items, asks = {},
		dumps = { { id = "d1", routes = { { id = "r1", target = { kind = "orchestrator", id = "orc-eng" } } } } } })
	orchestration.loads:set({
		["dev-desk"] = { cpu = 0.38, mem_used = 17e9, mem_total = 67e9, online = true, at = t },
		["dev-mac"] = { cpu = 0.83, mem_used = 7e9, mem_total = 32e9, online = true, at = t },
		["dev-win"] = { cpu = 0.77, mem_used = 7e9, mem_total = 32e9, online = true, at = t },
		["dev-cloud"] = { cpu = 0.24, mem_used = 7e9, mem_total = 32e9, online = true, at = t },
	})
	page.board_expanded:set(true)
end
ui.command { id = "perf_big", title = "Perf: a big fleet", run = big }

-- One thread's status changes: the whole session list pushed again, every
-- other chat equal (as the host's push is reconciled).
ui.command { id = "perf_status", title = "Perf: one status change", run = function()
	flips += 1
	local n: any = table.clone(sessions.now:peek())
	local list = {}
	for _, c in n.chats do
		local c2 = c
		if c.id == "vpn3" then
			c2 = table.clone(c)
			c2.status = if flips % 2 == 1 then "done" else "working"
			c2.updated = T + flips
		end
		table.insert(list, c2)
	end
	n.chats = list
	sessions.now:set(n)
end }

-- One thread's progress row moves a step (the engine's rows, pushed whole).
ui.command { id = "perf_progress", title = "Perf: one progress step", run = function()
	flips += 1
	local p: any = table.clone(orchestration.progress:peek() or {})
	p["vpn1"] = { chat_id = "vpn1", device_id = "dev-desk", state = "working", started = T - 600, last_step = T + flips, updated = T + flips,
		steps = { done = flips % 5, total = 5 }, stalled = false, refreshing = false, note = `step {flips}` }
	orchestration.progress:set(p)
end }

for _, id in page.WIDGETS do
	ui.command { id = `perf_only_{id}`, title = `Perf: only {id}`, run = function()
		local hidden = {}
		for _, other in page.WIDGETS do
			if other ~= id then
				hidden[other] = true
			end
		end
		page.widgets:set({ order = table.clone(page.WIDGETS), hidden = hidden :: any })
	end }
end

local counts = ui.state("perf_counts", {} :: any)
ui.command { id = "perf_read", title = "Perf: read the run counts", run = function()
	counts:set(table.clone(require("@ds/orch_perf_trace").counts))
end }

ui.command { id = "perf_reset", title = "Perf: reset the run counts", run = function()
	require("@ds/orch_perf_trace").reset()
end }

return root
