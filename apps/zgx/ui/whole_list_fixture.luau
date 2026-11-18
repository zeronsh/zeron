-- The shell on the Control room (ui/control_fixture.luau) with a fleet as
-- big as a person's: about 700 chats (four orchestrators and their ~100
-- crew, ~600 of their own across twelve projects, some archived, some with
-- side chats), the sidebar listing them all. What one sessions push costs
-- every whole-list reader: tests/whole_list.test.luau.
--
--   wl_big             the fleet (deterministic)
--   wl_status          one chat's status changes (working <-> done), the
--                      session list pushed again, every other row the same
--   wl_add / wl_remove one chat comes / goes
--   wl_rename          one chat's title changes
--   wl_read            the run counts (ds/orch_perf_trace) into the state
--                      `wl_counts`; wl_reset sets them to nothing

local root = require("./control_fixture")
local sessions = require("@account/sessions")
local orchestration = require("@account/orchestration")

local T = os.time()
local flips = 0
local STATES = { "done", "done", "failed", "working", "done", "done", "done", "waiting", "done", "input" }
local WORDS = { "Exporter image", "WireGuard keys", "Peer sync", "Admin app login", "Moscow relay", "DE node health", "Backup nightly", "Route table", "DNS split", "Metrics push", "Cert rotate", "Mesh join", "Firewall rules", "Client config", "UI polish", "Throughput test" }

local function big()
	-- (Motion reduced: the status dots and rings still, so what a push costs
	-- isn't mixed with their steps.)
	require("@ds/motion").reduce:set(true)
	-- (Every widget shown, whatever a run before left kept.)
	local page = require("@app/screens/control/page")
	page.widgets:set({ order = table.clone(page.WIDGETS), hidden = {} :: any })
	local t = T
	local devices = {
		{ id = "dev-desk", name = "dev-desk", platform = "linux", here = true, online = true },
		{ id = "dev-mac", name = "macbook", platform = "macos", online = true },
		{ id = "dev-win", name = "winbox", platform = "windows", online = true },
	}
	local projects = {}
	for p = 1, 12 do
		table.insert(projects, { id = `p{p}`, name = `project-{p}`, device = devices[(p - 1) % 3 + 1].id, path = `/home/you/project-{p}`, git = true })
	end
	local top, archived, children = {}, {}, {}
	local crew, items, orchestrators = {}, {}, {}
	local k = 0
	local function chat(id: string, title: string, device: string, project: string?): any
		k += 1
		return { id = id, title = title, device = device, project = project, status = STATES[(k - 1) % #STATES + 1],
			updated = t - k * 30, created = t - 90000 + k, agent = "claude", model = "claude-opus-5-5" }
	end
	-- Four orchestrators, their crew threads under them.
	-- (The Brain dump first, as every person has one: the middle of the Map.)
	local brain = chat("orc-brain", "Brain dump", "dev-desk", "p1")
	brain.status = "working"
	table.insert(top, brain)
	table.insert(orchestrators, { id = "orc-brain", kind = "brain_dump", name = "Brain dump", goal = "", rules = "", crew_rules = "", notes = "", hue = 140,
		crew = { devices = {} }, paused = false, archived = false, created = t - 90000, updated = t - 60 })
	local leads = { { "orc-vpn", "Exnomic VPN", 200, 46 }, { "orc-ui", "Control room UI", 320, 24 }, { "orc-eng", "Engine", 40, 20 }, { "orc-docs", "Docs", 100, 10 } }
	for li, l in leads do
		local o = chat(l[1], l[2], "dev-desk", "p1")
		o.status = "working"
		table.insert(top, o)
		table.insert(orchestrators, { id = l[1], kind = "standard", name = l[2], goal = "", rules = "", crew_rules = "", notes = "", hue = l[3],
			crew = { devices = {} }, paused = false, archived = false, created = t - 90000, updated = t - 60 })
		children[l[1]] = {}
		for i = 1, l[4] do
			local id = `{l[1]}-{i}`
			local c = chat(id, WORDS[(i - 1) % #WORDS + 1] .. ` {i}`, devices[(i - 1) % 3 + 1].id, `p{(li - 1) % 12 + 1}`)
			c.parent = l[1]
			table.insert(children[l[1]], c)
			table.insert(crew, { orchestrator_id = l[1], chat_id = id, released = false })
			table.insert(items, { id = `it-{id}`, orchestrator_id = l[1], title = c.title, state = if c.status == "done" then "review" else "active",
				priority = "normal", order = i, threads = { id }, created = t - 9000 + i, updated = t - i * 10 })
		end
	end
	-- ~600 of the person's own, across the projects; a few with side chats;
	-- every fifth archived.
	for i = 1, 600 do
		local c = chat(`own{i}`, `Own thread {i}`, devices[(i - 1) % 3 + 1].id, `p{(i - 1) % 12 + 1}`)
		if i % 5 == 0 then
			c.archived = true
			table.insert(archived, c)
		else
			table.insert(top, c)
			if i % 40 == 1 then
				children[c.id] = { (function(): any
					local s = chat(`side{i}`, `Side chat {i}`, c.device, c.project)
					s.parent = c.id
					s.side = true
					return s
				end)() }
			end
		end
	end
	sessions.now:set({
		state = "ready", here = "dev-desk", devices = devices, projects = projects, chats = top, archived = archived, children = children,
		places = { pinned = {}, sections = {} }, places_ready = true,
	} :: any)
	orchestration.now:set({ state = "ready", orchestrators = orchestrators, crew = crew, items = items, asks = {}, dumps = {} })
end
ui.command { id = "wl_big", title = "Whole list: a big fleet", run = big }

-- The session list pushed again with `change` applied to its rows (one row
-- a new table, every other the same: the host's push is reconciled).
local function push(change: (list: { any }) -> { any })
	local n: any = table.clone(sessions.now:peek())
	n.chats = change(n.chats)
	sessions.now:set(n)
end

ui.command { id = "wl_status", title = "Whole list: one status change", run = function()
	flips += 1
	push(function(list)
		local out = table.clone(list)
		for i, c in out do
			if c.id == "own7" then
				local c2 = table.clone(c)
				c2.status = if flips % 2 == 1 then "working" else "done"
				out[i] = c2
			end
		end
		return out
	end)
end }

ui.command { id = "wl_rename", title = "Whole list: one chat renamed", run = function()
	flips += 1
	push(function(list)
		local out = table.clone(list)
		for i, c in out do
			if c.id == "own13" then
				local c2 = table.clone(c)
				c2.title = `Renamed thread {flips}`
				out[i] = c2
			end
		end
		return out
	end)
end }

ui.command { id = "wl_add", title = "Whole list: one chat added", run = function()
	flips += 1
	push(function(list)
		local out = table.clone(list)
		table.insert(out, 1, { id = `new{flips}`, title = `New thread {flips}`, device = "dev-desk", project = "p2", status = "working",
			updated = T + flips, created = T + flips, agent = "claude", model = "claude-opus-5-5" })
		return out
	end)
end }

ui.command { id = "wl_remove", title = "Whole list: one chat removed", run = function()
	push(function(list)
		local out = {}
		local gone = false
		for _, c in list do
			if not gone and c.id == "own21" then
				gone = true
			else
				table.insert(out, c)
			end
		end
		return out
	end)
end }

-- Only one of the Control room's widgets shown (`wl_only_map`, …), or none
-- (`wl_only_none`): what each one's readers cost a push.
local page = require("@app/screens/control/page")
for _, id in { "none", table.unpack(page.WIDGETS) } do
	ui.command { id = `wl_only_{id}`, title = `Whole list: only {id}`, run = function()
		local hidden = {}
		for _, other in page.WIDGETS do
			if other ~= id then
				hidden[other] = true
			end
		end
		page.widgets:set({ order = table.clone(page.WIDGETS), hidden = hidden :: any })
	end }
end

local counts = ui.state("wl_counts", {} :: any)
ui.command { id = "wl_read", title = "Whole list: read the run counts", run = function()
	counts:set(table.clone(require("@ds/orch_perf_trace").counts))
end }
ui.command { id = "wl_reset", title = "Whole list: reset the run counts", run = function()
	require("@ds/orch_perf_trace").reset()
end }

return root
