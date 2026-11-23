-- The Control room's shell (control_fixture, control_world's sample) with a
-- fleet as big as a person's, or a small one, chosen by a command: what the
-- caps on its lists are tested on (tests/list_caps.test.luau).
--
--   fleet_dense    the sample plus ~700 chats: 7 orchestrators, ~100 crew
--                  threads, ~200 board items, a dozen open asks, five
--                  machines
--   fleet_sparse   the Brain dump and one orchestrator, four threads, one ask
--   fleet_nocrew   fleet_sparse, its orchestrator's crew released
--   fleet_empty    a new account: one chat, no orchestrator
--   fleet_orch_<id> / fleet_control   where the window is

local root = require("./control_fixture")
local sessions = require("@account/sessions")
local orchestration = require("@account/orchestration")
local route = require("@app/screens/shell/route")

local T = os.time()
local STATES = { "done", "working", "done", "failed", "working", "done", "waiting", "done", "input", "done" }
local WORDS = { "Exporter image", "WireGuard keys", "Peer sync", "Admin app login", "Moscow relay", "DE node health", "Backup nightly",
	"Route table", "DNS split", "Metrics push", "Cert rotate", "Mesh join", "Firewall rules", "Client config", "UI polish", "Throughput test",
	"Board cards", "Map legend", "Hover cards", "Sidebar badges", "Engine reports", "Ask routing", "Release notes", "Docs search" }
local ITEM_STATES = { "active", "active", "review", "blocked", "done", "planned", "active", "inbox", "review", "done" }
local PRIORITY = { "normal", "normal", "high", "normal", "low", "urgent", "normal", "high" }

local function clone(v: any): any
	return table.clone(v or {})
end

local function dense()
	local s: any = clone(sessions.now:peek())
	local o: any = clone(orchestration.now:peek())
	local devices = clone(s.devices)
	for _, d in {
		{ id = "dev-win", name = "winbox", platform = "windows", here = false, online = true },
		{ id = "dev-zephyr", name = "zephyrka", platform = "linux", here = false, online = true },
		{ id = "dev-pi", name = "pi-lab", platform = "linux", here = false, online = false },
	} do
		table.insert(devices, d)
	end
	s.devices = devices
	local devs = { "dev-desk", "dev-mac", "dev-win", "dev-zephyr", "dev-cloud" }
	local projects = clone(s.projects)
	for p = 1, 12 do
		table.insert(projects, { id = `rp{p}`, name = `project-{p}`, device = devs[(p - 1) % 4 + 1], path = `/home/you/project-{p}`, git = true })
	end
	s.projects = projects
	local top, children, archived = clone(s.chats), {}, clone(s.archived)
	for key, v in s.children or {} do
		children[key] = clone(v)
	end
	local orchs, crew, items, asks = clone(o.orchestrators), clone(o.crew), clone(o.items), clone(o.asks)
	local k = 0
	local function chat(id: string, title: string, device: string, project: string?): any
		k += 1
		return { id = id, title = title, device = device, project = project, status = STATES[(k - 1) % #STATES + 1],
			updated = T - k * 37, created = T - 90000 + k, agent = if k % 4 == 0 then "openai" else "claude",
			model = if k % 4 == 0 then "gpt-5.5" else "claude-opus-5-5" }
	end
	local leads: { { any } } = { { "orc-ui", "Control room UI", 320, 34 }, { "orc-eng", "Engine", 40, 30 }, { "orc-docs", "Docs", 100, 26 } }
	for li, l in leads do
		local lead = chat(l[1], l[2], "dev-desk", `rp{li}`)
		lead.status = "working"
		table.insert(top, lead)
		table.insert(orchs, { id = l[1], kind = "standard", name = l[2], goal = `Ship {l[2]} work end to end.`, rules = "Small PRs. Tests first.",
			crew_rules = "Report at milestones.", notes = "- Weekly review Fridays", hue = l[3],
			crew = { devices = { "dev-desk", devs[li + 1] }, max_running = 6 }, paused = li == 3, archived = false, created = T - 90000, updated = T - 60 })
		children[l[1]] = {}
		for i = 1, l[4] do
			local id = `{l[1]}-{i}`
			local c = chat(id, WORDS[(i + li * 5 - 1) % #WORDS + 1] .. ` {i}`, devs[(i - 1) % #devs + 1], `rp{(li - 1) % 12 + 1}`)
			c.parent = l[1]
			c.child_kind = "crew"
			if c.status == "working" then
				c.todo = { done = i % 6, total = 7, current = "Wiring the last case" }
			end
			table.insert(children[l[1]], c)
			table.insert(crew, { orchestrator_id = l[1], chat_id = id, item_id = `it-{id}`, role = string.lower(WORDS[(i - 1) % #WORDS + 1]),
				released = false, joined = T - 9000 + i,
				last_report = if i % 3 == 0 then { id = `rp-{id}`, state = if i % 9 == 0 then "blocked" else "progress", summary = "Halfway; tests green so far", at = T - i * 60 } else nil })
			local st = ITEM_STATES[(i - 1) % #ITEM_STATES + 1]
			table.insert(items, { id = `it-{id}`, orchestrator_id = l[1], title = c.title, detail = "", state = st,
				priority = PRIORITY[(i - 1) % #PRIORITY + 1], order = i, threads = { id }, links = {},
				progress = if st == "done" or st == "review" then 1 else (i % 5) / 5,
				outcome = if st == "done" or st == "review" then "Merged behind a flag; follow-up filed." else nil,
				done = if st == "done" then T - i * 300 else nil,
				created = T - 9000 + i, updated = T - i * 10 })
		end
		-- Cards with no thread yet: planned, inbox, done long ago.
		for i = 1, 22 do
			local st = ({ "planned", "inbox", "done", "planned", "done" })[(i - 1) % 5 + 1]
			table.insert(items, { id = `it-{l[1]}-x{i}`, orchestrator_id = l[1], title = `{WORDS[(i * 3) % #WORDS + 1]} follow-up {i}`, detail = "",
				state = st, priority = PRIORITY[(i + 2) % #PRIORITY + 1], order = 100 + i, threads = {}, links = {},
				done = if st == "done" then T - 86400 - i * 600 else nil, created = T - 200000 + i, updated = T - 100000 + i })
		end
	end
	-- A dozen open asks (the sample's two among them).
	local qs = {
		{ "orc-ui-2", "orc-ui", "decision", "Board: group Review by orchestrator, or one column?", { { label = "By orchestrator" }, { label = "One column" } }, false },
		{ "orc-ui-6", "orc-ui", "approval", "Merge **zeron/board-cards-cheap** into orch-next?", { { label = "Merge" }, { label = "Hold" } }, true },
		{ "orc-ui-11", "orc-ui", "question", "Which accent should the Map's machine hubs use in light mode?", {}, false },
		{ "orc-eng-4", "orc-eng", "approval", "Restart the dev engine now? 3 chats are mid-turn.", { { label = "Restart" }, { label = "After they finish" } }, false },
		{ "orc-eng-9", "orc-eng", "decision", "Keep reports for 30 days or forever?", { { label = "30 days" }, { label = "Forever" } }, false },
		{ "orc-eng-13", "orc-eng", "question", "The bucket for macOS sccache?", {}, true },
		{ "orc-docs-3", "orc-docs", "decision", "Docs site: Astro or mdBook?", { { label = "Astro", detail = "Search built in" }, { label = "mdBook", detail = "Fewer deps" } }, false },
		{ "orc-docs-8", "orc-docs", "approval", "Publish the 0.3 changelog?", { { label = "Publish" }, { label = "Edit first" } }, false },
		{ "orc-ui-17", "orc-ui", "decision", "Hover cards: open on hover or on click?", { { label = "Hover" }, { label = "Click" } }, false },
		{ "orc-eng-21", "orc-eng", "question", "Is the winbox allowed to run GPU tests overnight?", {}, false },
		{ "orc-ui-23", "orc-ui", "approval", "Ship the Map legend change today?", { { label = "Ship" }, { label = "Tomorrow" } }, false },
		{ "orc-ui-29", "orc-ui", "decision", "Crew list: newest first or by state?", { { label = "Newest first" }, { label = "By state" } }, false },
	}
	for i, q in qs do
		table.insert(asks, { id = `ak-rv{i}`, from_chat = q[1], orchestrator_id = q[2], item_id = `it-{q[1]}`, kind = q[3], question = q[4],
			options = q[5], urgent = q[6], state = "open", created = T - i * 420, origin = "tool" })
	end
	-- ~560 of the person's own, every fifth archived, a few with side chats.
	for i = 1, 560 do
		local c = chat(`own{i}`, `Own thread {i}`, devs[(i - 1) % 4 + 1], `rp{(i - 1) % 12 + 1}`)
		if i % 5 == 0 then
			c.archived = true
			table.insert(archived, c)
		else
			table.insert(top, c)
			if i % 40 == 1 then
				local sc = chat(`side{i}`, `Side chat {i}`, c.device, c.project)
				sc.parent = c.id
				sc.side = true
				children[c.id] = { sc }
			end
		end
	end
	s.chats, s.children, s.archived = top, children, archived
	o.orchestrators, o.crew, o.items, o.asks = orchs, crew, items, asks
	sessions.now:set(s)
	orchestration.now:set(o)
	local loads = clone(orchestration.loads:peek())
	loads["dev-win"] = { cpu = 0.55, mem_used = 18.0e9, mem_total = 32.0e9, load1 = 6.1, cores = 12, online = true, at = T }
	loads["dev-zephyr"] = { cpu = 0.22, mem_used = 9.0e9, mem_total = 16.0e9, load1 = 1.9, cores = 8, online = true, at = T }
	orchestration.loads:set(loads)
	local comms = clone(orchestration.comms:peek())
	for i = 1, 40 do
		local l = leads[(i - 1) % 3 + 1]
		table.insert(comms, { id = `cm-rv{i}`, kind = ({ "report", "message", "spawn", "ask", "route" })[(i - 1) % 5 + 1],
			from = `{l[1]}-{(i % l[4]) + 1}`, to = l[1], summary = `{WORDS[i % #WORDS + 1]}: step {i}`, at = T - i * 45 })
	end
	orchestration.comms:set(comms)
end
ui.command { id = "fleet_dense", title = "Fleet: dense", run = dense }

-- What belongs to `keep` (chat / orchestrator ids) in a list.
local function only(list: { any }?, keep: { [string]: boolean }, fields: { string }): { any }
	local out = {}
	for _, v in list or {} do
		local ok = false
		for _, f in fields do
			if v[f] and keep[v[f]] then
				ok = true
			end
		end
		if ok then
			table.insert(out, v)
		end
	end
	return out
end

local function sparse(no_crew: boolean)
	local s: any = clone(sessions.now:peek())
	local o: any = clone(orchestration.now:peek())
	local keep: { [string]: boolean } = { ["orc-brain"] = true, ["orc-vpn"] = true, ["th-vpn-moscow"] = true, ["th-vpn-merge"] = true }
	local extra = { "th-vpn-dns", "th-vpn-keys" }
	s.devices = { s.devices[1] }
	local top = {}
	for _, c in s.chats do
		if keep[c.id] then
			local c2 = table.clone(c)
			c2.device = "dev-desk"
			table.insert(top, c2)
		end
	end
	for i, t in { "Fix the login redirect", "Read the release notes", "Plan the week" } do
		table.insert(top, { id = `own{i}`, title = t, device = "dev-desk", status = if i == 1 then "working" else "done",
			updated = T - i * 900, created = T - i * 4000, agent = "claude", model = "claude-opus-5-5" })
	end
	local kids = {}
	if not no_crew then
		for _, c in (s.children or {})["orc-vpn"] or {} do
			local c2 = table.clone(c)
			c2.device = "dev-desk"
			table.insert(kids, c2)
		end
		for i, id in extra do
			table.insert(kids, { id = id, title = if i == 1 then "DNS split for DE" else "Rotate WireGuard keys", device = "dev-desk",
				parent = "orc-vpn", child_kind = "crew", status = if i == 1 then "working" else "done", updated = T - i * 200,
				created = T - 5000, agent = "claude", model = "claude-opus-5-5" })
			keep[id] = true
		end
	end
	s.chats, s.children, s.archived = top, { ["orc-vpn"] = kids }, {}
	o.orchestrators = only(o.orchestrators, keep, { "id" })
	local crew = if no_crew then {} else only(o.crew, keep, { "chat_id" })
	local items = only(o.items, keep, { "orchestrator_id" })
	if no_crew then
		for i, it in items do
			local it2 = table.clone(it)
			it2.threads = {}
			items[i] = it2
		end
	else
		for i, id in extra do
			table.insert(crew, { orchestrator_id = "orc-vpn", chat_id = id, item_id = `it-sp{i}`, role = if i == 1 then "dns" else "keys", released = false, joined = T - 5000 })
		end
		table.insert(items, { id = "it-sp1", orchestrator_id = "orc-vpn", title = "DNS split for DE", detail = "", state = "active", priority = "normal",
			order = 3, threads = { "th-vpn-dns" }, links = {}, created = T - 5000, updated = T - 200 })
		table.insert(items, { id = "it-sp2", orchestrator_id = "orc-vpn", title = "Rotate WireGuard keys", detail = "", state = "review", priority = "normal",
			progress = 1, order = 4, threads = { "th-vpn-keys" }, links = {}, outcome = "Keys rotated on 3 nodes.", created = T - 5000, updated = T - 400 })
	end
	o.crew, o.items = crew, items
	-- One ask: the urgent reboot one.
	o.asks = only(o.asks, { ["orc-vpn"] = true }, { "from_chat" })
	local d2 = (o.dumps or {})[2]
	o.dumps = if d2 then { d2 } else {}
	sessions.now:set(s)
	orchestration.now:set(o)
	orchestration.loads:set({ ["dev-desk"] = (orchestration.loads:peek() :: any)["dev-desk"] } :: any)
	orchestration.comms:set(only(orchestration.comms:peek(), keep, { "from" }))
	local progress = {}
	for id, row in (orchestration.progress:peek() or {}) :: any do
		if keep[id] then
			progress[id] = row
		end
	end
	orchestration.progress:set(progress :: any)
end
ui.command { id = "fleet_sparse", title = "Fleet: sparse", run = function()
	sparse(false)
end }
ui.command { id = "fleet_nocrew", title = "Fleet: an orchestrator with no crew", run = function()
	sparse(true)
end }

-- A new account: one machine, one chat, no orchestrator yet.
ui.command { id = "fleet_empty", title = "Fleet: a new account", run = function()
	local s: any = clone(sessions.now:peek())
	s.devices = { s.devices[1] }
	s.chats = {
		{ id = "own1", title = "Hello Zeron", device = "dev-desk", status = "done", updated = T - 600, created = T - 700, agent = "claude", model = "claude-opus-5-5" },
	}
	s.children, s.archived = {}, {}
	sessions.now:set(s)
	orchestration.now:set({ state = "ready", orchestrators = {}, crew = {}, items = {}, asks = {}, dumps = {} } :: any)
	orchestration.comms:set({})
	orchestration.progress:set({} :: any)
end }

ui.command { id = "fleet_control", title = "Fleet: the Control room", run = function()
	route.control()
end }
for _, id in { "orc-vpn", "orc-ui" } do
	ui.command { id = `fleet_orch_{id}`, title = `Fleet: orchestrator {id}`, run = function()
		route.orchestrator(id)
	end }
end

return root
