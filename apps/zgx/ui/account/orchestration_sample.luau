-- A complete orchestration state in the shape `orchestration.now` publishes
-- (docs/design/orchestrators.md, "Bridge"), plus the chats, devices, loads
-- and comms it refers to. Fixtures, galleries and audit shots read this one
-- table so every screen is drawn against the same world.
--
--   local sample = require("@app/account/orchestration_sample")
--   sample.now        what orchestration.now:get() returns
--   sample.loads      what orchestration.loads:get() returns
--   sample.comms      what orchestration.comms:get() returns
--   sample.devices    sessions.now devices to pair with it
--   sample.chats      sessions.now chats (crew threads carry activity/todo)
--   sample.at(t)      a copy with every time shifted so "now" is `t`

local NOW = 1794470400 -- 2026-11-12 18:40 PST

local function ago(s)
	return NOW - s
end

local devices = {
	{ id = "dev-desk", name = "desk", platform = "linux", here = true, online = true },
	{ id = "dev-mac", name = "macbook", platform = "macos", here = false, online = true },
	{ id = "dev-cloud", name = "Cloud", platform = "linux", here = false, online = true, cloud = true },
}

local orchestrators = {
	{
		id = "orc-brain",
		kind = "brain_dump",
		name = "Brain dump",
		goal = "Route everything I say to the right place.",
		rules = "",
		crew_rules = "",
		notes = "- Ask Wing about the relay deploy window\n- Look at zgx memory after the tabs refactor",
		hue = 262,
		crew = { devices = {} },
		paused = false,
		archived = false,
		created = ago(86400 * 6),
		updated = ago(40),
	},
	{
		id = "orc-orch",
		kind = "standard",
		name = "Orchestrators",
		goal = "Ship Orchestrators and the Control room in zgx.",
		rules = "Opus for UI and engine design, Sonnet for plumbing. Ask before merging to zeron/zgx.",
		crew_rules = "Own worktree per thread. cargo -j 4. Never touch the production engine.",
		notes = "1. Contract (done)\n2. Engine state + flow\n3. Control room + orchestrator screens\n4. Integrate, live test",
		hue = 196,
		crew = { devices = { "dev-desk", "dev-mac" }, max_running = 4, model = "claude-opus-5-5" },
		paused = false,
		archived = false,
		created = ago(3600 * 2),
		updated = ago(90),
	},
	{
		id = "orc-vpn",
		kind = "standard",
		name = "Exnomic VPN",
		goal = "Phase 2 migration, safely.",
		rules = "Nothing touches prod without asking.",
		crew_rules = "",
		notes = "Phase 1 watch-only on DE + Moscow. Moscow needs a reboot.",
		hue = 32,
		crew = { devices = { "dev-desk" } },
		wake = { at = { "09:00" }, weekdays_only = true, prompt = "Check node health and report." },
		paused = false,
		archived = false,
		created = ago(86400 * 3),
		updated = ago(3600),
	},
	{
		id = "orc-ci",
		kind = "standard",
		name = "CI speed",
		goal = "Every PR green in under 10 minutes.",
		rules = "",
		crew_rules = "",
		notes = "",
		hue = 128,
		crew = { devices = { "dev-cloud" }, harness = "codex" },
		paused = true,
		archived = false,
		created = ago(86400 * 9),
		updated = ago(86400),
	},
}

local chats = {
	{ id = "orc-orch", title = "Orchestrators", device = "dev-desk", status = "working", updated = ago(20), created = ago(7200), agent = "claude", model = "claude-opus-5-5" },
	{ id = "orc-vpn", title = "Exnomic VPN", device = "dev-desk", updated = ago(3600), created = ago(259200), agent = "claude", model = "claude-sonnet-5-5" },
	{ id = "orc-ci", title = "CI speed", device = "dev-cloud", updated = ago(86400), created = ago(777600), agent = "codex", model = "gpt-5.5" },
	{ id = "orc-brain", title = "Brain dump", device = "dev-desk", updated = ago(40), created = ago(518400), agent = "claude", model = "claude-haiku-5-5" },
	{
		id = "th-engine-state", title = "Engine: orchestration state", device = "dev-desk", status = "working",
		parent = "orc-orch", child_kind = "crew", updated = ago(12), created = ago(5400), agent = "claude", model = "claude-opus-5-5",
		activity = { kind = "testing", label = "cargo test -p zeron-engine --lib orchestration", since = ago(48) },
		todo = { done = 5, total = 8, current = "WatchOrchestration stream" },
	},
	{
		id = "th-engine-flow", title = "Engine: reports + MCP tools", device = "dev-desk", status = "working",
		parent = "orc-orch", child_kind = "crew", updated = ago(5), created = ago(5400), agent = "claude", model = "claude-opus-5-5",
		activity = { kind = "editing", label = "Edit crates/mcp/src/tools.rs", since = ago(6) },
		todo = { done = 3, total = 9, current = "report tool" },
	},
	{
		id = "th-ui-control", title = "Control room UI", device = "dev-mac", status = "working",
		parent = "orc-orch", child_kind = "crew", updated = ago(30), created = ago(5300), agent = "claude", model = "claude-opus-5-5",
		activity = { kind = "building", label = "cargo build --release", since = ago(140) },
		todo = { done = 2, total = 7, current = "Map widget" },
	},
	{
		id = "th-ui-orch", title = "Orchestrator screen", device = "dev-mac", status = "input",
		parent = "orc-orch", child_kind = "crew", updated = ago(200), created = ago(5300), agent = "claude", model = "claude-opus-5-5",
		todo = { done = 4, total = 6, current = "Crew updates card" },
	},
	{
		id = "th-vpn-moscow", title = "Moscow node reboot plan", device = "dev-desk",
		parent = "orc-vpn", child_kind = "crew", updated = ago(3500), created = ago(7000), agent = "claude", model = "claude-sonnet-5-5",
		todo = { done = 3, total = 3 },
	},
	{
		id = "th-vpn-merge", title = "Merge phase-1 fixes", device = "dev-desk", status = "working",
		parent = "orc-vpn", child_kind = "crew", updated = ago(8), created = ago(1800), agent = "claude", model = "claude-sonnet-5-5",
		activity = { kind = "merging", label = "gh pr merge 412 --squash", since = ago(9) },
		todo = { done = 6, total = 7, current = "Merge #412" },
	},
	{
		id = "th-ci-cache", title = "sccache on the iOS lane", device = "dev-cloud", status = "failed",
		parent = "orc-ci", child_kind = "crew", updated = ago(90000), created = ago(100000), agent = "codex", model = "gpt-5.5",
	},
	{ id = "th-titlebar", title = "Linux titlebar toggle", device = "dev-desk", status = "done", updated = ago(26000), created = ago(30000), agent = "claude", model = "claude-opus-5-5" },
}

local crew = {
	{ chat_id = "th-engine-state", orchestrator_id = "orc-orch", item_id = "it-engine", role = "engine state", released = false, joined = ago(5400),
		last_report = { id = "rp-1", state = "progress", summary = "Registry kinds + WatchOrchestration in; ops next", at = ago(600) } },
	{ chat_id = "th-engine-flow", orchestrator_id = "orc-orch", item_id = "it-flow", role = "reports + tools", released = false, joined = ago(5400),
		last_report = { id = "rp-2", state = "progress", summary = "CrewReports part renders; coalesced wake works in tests", at = ago(300) } },
	{ chat_id = "th-ui-control", orchestrator_id = "orc-orch", item_id = "it-control", role = "control room", released = false, joined = ago(5300) },
	{ chat_id = "th-ui-orch", orchestrator_id = "orc-orch", item_id = "it-orch-screen", role = "orchestrator screen", released = false, joined = ago(5300),
		last_report = { id = "rp-3", state = "blocked", summary = "Needs a decision: tabs or split for the orchestrator page", at = ago(200) } },
	{ chat_id = "th-vpn-moscow", orchestrator_id = "orc-vpn", item_id = "it-moscow", role = "reboot plan", released = false, joined = ago(7000),
		last_report = { id = "rp-4", state = "done", summary = "Reboot plan ready; needs your OK for 02:00 MSK", at = ago(3500) } },
	{ chat_id = "th-vpn-merge", orchestrator_id = "orc-vpn", item_id = "it-phase1", role = "merge", released = false, joined = ago(1800) },
	{ chat_id = "th-ci-cache", orchestrator_id = "orc-ci", item_id = "it-sccache", role = "sccache", released = false, joined = ago(100000),
		last_report = { id = "rp-5", state = "failed", summary = "sccache can't reach the bucket from macOS runners", at = ago(90000) } },
}

local items = {
	{ id = "it-engine", orchestrator_id = "orc-orch", title = "Engine: orchestration state + RPC", detail = "", state = "active", priority = "high",
		threads = { "th-engine-state" }, links = { { kind = "branch", label = "orch/engine-state", target = "orch/engine-state" } }, order = 1, created = ago(5400), updated = ago(600) },
	{ id = "it-flow", orchestrator_id = "orc-orch", title = "Reports, asks, MCP tools", detail = "", state = "active", priority = "high",
		threads = { "th-engine-flow" }, links = {}, order = 2, created = ago(5400), updated = ago(300) },
	{ id = "it-control", orchestrator_id = "orc-orch", title = "Control room", detail = "", state = "active", priority = "urgent",
		threads = { "th-ui-control" }, links = {}, order = 0, created = ago(5300), updated = ago(30) },
	{ id = "it-orch-screen", orchestrator_id = "orc-orch", title = "Orchestrator screen", detail = "", state = "blocked", priority = "high",
		threads = { "th-ui-orch" }, links = {}, order = 3, created = ago(5300), updated = ago(200) },
	{ id = "it-live", orchestrator_id = "orc-orch", title = "Live end-to-end test on a dev engine", detail = "", state = "planned", priority = "normal",
		threads = {}, links = {}, order = 4, created = ago(5300), updated = ago(5300) },
	{ id = "it-moscow", orchestrator_id = "orc-vpn", title = "Reboot Moscow node", detail = "", state = "review", priority = "high", progress = 1,
		threads = { "th-vpn-moscow" }, links = {}, order = 0, created = ago(7000), updated = ago(3500),
		outcome = "Plan: drain at 01:45 MSK, reboot 02:00, verify wg + exporter by 02:10. Rollback: none needed." },
	{ id = "it-phase1", orchestrator_id = "orc-vpn", title = "Merge phase-1 fixes", detail = "", state = "active", priority = "normal",
		threads = { "th-vpn-merge" }, links = { { kind = "pr", label = "#412", target = "https://github.com/katulevskiy/exnomic/pull/412" } }, order = 1, created = ago(1800), updated = ago(8) },
	{ id = "it-phase2", orchestrator_id = "orc-vpn", title = "Phase 2 migration", detail = "", state = "planned", priority = "low",
		threads = {}, links = {}, order = 2, created = ago(200000), updated = ago(200000) },
	{ id = "it-sccache", orchestrator_id = "orc-ci", title = "sccache for iOS", detail = "", state = "blocked", priority = "normal",
		threads = { "th-ci-cache" }, links = {}, order = 0, created = ago(100000), updated = ago(90000) },
	{ id = "it-titlebar", title = "Linux titlebar toggle", detail = "", state = "done", priority = "normal", progress = 1,
		threads = { "th-titlebar" }, links = { { kind = "pr", label = "#1383", target = "https://github.com/zeronsh/zeron/pull/1383" } }, order = 0,
		created = ago(30000), updated = ago(26000), done = ago(26000), outcome = "PR #1383 open: 88px → 10px when no window buttons." },
	{ id = "it-omarchy", title = "Rerun omarchy-update (needs sudo)", detail = "", state = "inbox", priority = "normal",
		threads = {}, links = {}, order = 0, created = ago(40), updated = ago(40), source = { dump_id = "dp-2", route_id = "rt-6" } },
}

local asks = {
	{ id = "ak-1", from_chat = "th-ui-orch", orchestrator_id = "orc-orch", item_id = "it-orch-screen", kind = "decision",
		question = "Orchestrator page: conversation and live state side by side, or tabs?",
		options = { { label = "Side by side", detail = "Chat left, crew + board right" }, { label = "Tabs", detail = "Chat · Board · Crew" } },
		urgent = false, state = "open", created = ago(200) },
	{ id = "ak-2", from_chat = "orc-vpn", orchestrator_id = "orc-vpn", item_id = "it-moscow", kind = "approval",
		question = "Reboot the Moscow node at **02:00 MSK** tonight?",
		options = { { label = "Approve" }, { label = "Not tonight" } }, urgent = true, state = "open", created = ago(3400) },
	{ id = "ak-3", from_chat = "th-ci-cache", orchestrator_id = "orc-ci", kind = "question",
		question = "Which bucket should macOS runners use for sccache?", options = {}, urgent = false, state = "answered",
		answer = "zeron-ci-cache-us", created = ago(95000), answered = ago(91000) },
}

local dumps = {
	{
		id = "dp-2",
		attachments = { { path = "assets/favicons/zeron.png", name = "board.png" }, { path = "pending://up-1/notes.txt", name = "notes.txt" } },
		text = "ok so the orchestrator page should be side by side not tabs, also remind me to rerun omarchy update it needs sudo. and ask the vpn guy whether moscow rebooted fine. the CI thing can wait till next week, and is the engine state thread still on the registry part?",
		state = "routed",
		summary = "5 parts: 2 to Orchestrators, 1 to Exnomic VPN, 1 to CI speed, 1 to your inbox.",
		device = "dev-desk",
		created = ago(40),
		routes = {
			{ id = "rt-5", excerpt = "the orchestrator page should be side by side not tabs", span = { from = 6, to = 59 },
				target = { kind = "chat", id = "th-ui-orch" }, intent = "info", message = "Decision from the person: side by side (chat left, crew + board right), not tabs.", state = "done", candidates = {},
				answer = "Switched to side by side; the tabs are gone.", answer_full = "Done: the orchestrator page is **side by side** now (chat left, crew and board right). Tabs removed in `ui/screens/orchestrator/page.luau`." },
			{ id = "rt-6", excerpt = "remind me to rerun omarchy update it needs sudo", span = { from = 66, to = 113 },
				target = { kind = "item", id = "it-omarchy" }, intent = "task", message = "Rerun omarchy-update (needs sudo).", state = "delivered", candidates = {}, item_id = "it-omarchy" },
			{ id = "rt-7", excerpt = "ask the vpn guy whether moscow rebooted fine", span = { from = 119, to = 163 },
				target = { kind = "orchestrator", id = "orc-vpn" }, intent = "question", message = "Did the Moscow node reboot cleanly? Check wg and the exporter.", state = "answered", candidates = {},
				answer = "Not yet — scheduled for 02:00 MSK, waiting on your approval.",
				answer_full = "Not yet. The reboot is **scheduled for 02:00 MSK** and waits on your approval (the ask in Needs you).\n\nOnce it runs I'll check:\n\n- `wg show` on the node: the peers back\n- the exporter's `/metrics`: scraping again\n\nI'll report either way." },
			{ id = "rt-8", excerpt = "the CI thing can wait till next week", span = { from = 165, to = 201 },
				target = { kind = "orchestrator", id = "orc-ci" }, intent = "priority", message = "Deprioritise: nothing before next week.", state = "delivered", candidates = {} },
			{ id = "rt-9", excerpt = "is the engine state thread still on the registry part?", span = { from = 207, to = 261 },
				target = { kind = "chat", id = "th-engine-state" }, intent = "question", message = "Status check from the person: are you still on the registry layer?", state = "delivered", candidates = {} },
		},
	},
	{
		id = "dp-1",
		text = "make the board show PR checks and look at why zgx eats 2gb after the tabs refactor",
		state = "needs_you",
		summary = "1 part placed, 1 unsure.",
		device = "dev-mac",
		created = ago(7200),
		routes = {
			{ id = "rt-1", excerpt = "make the board show PR checks", span = { from = 0, to = 29 },
				target = { kind = "orchestrator", id = "orc-orch" }, intent = "task", message = "Board cards should show PR check state (via the github plugin slot).", state = "delivered", candidates = {} },
			{ id = "rt-2", excerpt = "look at why zgx eats 2gb after the tabs refactor", span = { from = 34, to = 82 },
				target = { kind = "unsure" }, intent = "task", message = "Investigate zgx memory (~2 GB) after the tabs refactor.", state = "pending",
				candidates = { { kind = "orchestrator", id = "orc-orch" }, { kind = "new_thread" } } },
		},
	},
}

local loads = {
	["dev-desk"] = { cpu = 0.71, mem_used = 41.2e9, mem_total = 62.0e9, load1 = 11.4, cores = 16, online = true, at = NOW },
	["dev-mac"] = { cpu = 0.38, mem_used = 22.1e9, mem_total = 36.0e9, load1 = 4.2, cores = 12, online = true, at = NOW },
	["dev-cloud"] = { cpu = 0.04, mem_used = 1.1e9, mem_total = 8.0e9, load1 = 0.1, cores = 4, online = true, at = NOW },
}

local comms = {
	{ id = "cm-1", kind = "route", from = "orc-brain", to = "th-ui-orch", summary = "Side by side, not tabs", at = ago(38) },
	{ id = "cm-2", kind = "route", from = "orc-brain", to = "orc-vpn", summary = "Did Moscow reboot cleanly?", at = ago(37) },
	{ id = "cm-3", kind = "message", from = "orc-vpn", to = "you", summary = "Not yet — waiting on your approval", at = ago(30) },
	{ id = "cm-4", kind = "report", from = "th-engine-flow", to = "orc-orch", summary = "Coalesced wake works in tests", at = ago(300) },
	{ id = "cm-5", kind = "ask", from = "th-ui-orch", to = "you", summary = "Side by side or tabs?", at = ago(200) },
	{ id = "cm-6", kind = "message", from = "orc-orch", to = "th-ui-control", summary = "Map first, then board", at = ago(150) },
	{ id = "cm-7", kind = "spawn", from = "orc-vpn", to = "th-vpn-merge", summary = "Merge phase-1 fixes", at = ago(1800) },
}

-- Each device's CPU over the last six minutes, one sample every 5 s, as
-- the bridge keeps it from `WatchDeviceLoads` (`history`, oldest first):
-- a wander around where it stands now, with a burst or two.
local function history_of(cpu: number, seed: number): { { at: number, cpu: number } }
	local out = {}
	local n = 72
	for i = 1, n do
		local t = i + seed * 13
		local wave = math.sin(t * 0.31) * 0.09 + math.sin(t * 1.13) * 0.05 + math.sin(t * 2.7) * 0.025
		local burst = if (i + seed * 7) % 29 < 5 then 0.18 else 0
		local v = cpu + wave + burst - 0.06 + (cpu - 0.3) * (i / n - 1) * 0.4
		table.insert(out, { at = NOW - (n - i) * 5, cpu = math.clamp(v, 0.02, 0.98) })
	end
	out[n].cpu = cpu
	return out
end
loads["dev-desk"].history = history_of(loads["dev-desk"].cpu, 1)
loads["dev-mac"].history = history_of(loads["dev-mac"].cpu, 2)
loads["dev-cloud"].history = history_of(loads["dev-cloud"].cpu, 3)

-- More comms, for a feed with bursts in it.
for _, c in {
	{ id = "cm-8", kind = "report", from = "th-engine-state", to = "orc-orch", summary = "Registry kinds in; ops next", at = ago(600) },
	{ id = "cm-9", kind = "message", from = "orc-orch", to = "th-engine-state", summary = "Good. Ops, then the stream", at = ago(590) },
	{ id = "cm-10", kind = "report", from = "th-vpn-moscow", to = "orc-vpn", summary = "Reboot plan ready", at = ago(3500) },
	{ id = "cm-11", kind = "ask", from = "orc-vpn", to = "you", summary = "Reboot Moscow at 02:00 MSK?", at = ago(3400) },
	{ id = "cm-12", kind = "message", from = "orc-orch", to = "th-engine-flow", summary = "Coalesce wakes within 2 s", at = ago(5) },
	{ id = "cm-13", kind = "report", from = "th-vpn-merge", to = "orc-vpn", summary = "Checks green, merging #412", at = ago(10) },
	{ id = "cm-14", kind = "answer", from = "th-ui-orch", to = "you", summary = "Switched to side by side; the tabs are gone.", at = ago(12) },
} do
	table.insert(comms, c)
end
table.sort(comms, function(a: any, b: any): boolean
	return a.at < b.at
end)

local sample = {
	NOW = NOW,
	now = {
		state = "ready",
		orchestrators = orchestrators,
		crew = crew,
		items = items,
		asks = asks,
		dumps = dumps,
	},
	loads = loads,
	comms = comms,
	devices = devices,
	chats = chats,
}

local TIME_KEYS = {
	created = true, updated = true, done = true, joined = true, at = true, since = true, answered = true,
	-- (A progress row's clocks.)
	started = true, last_step = true, note_at = true, tool_since = true,
}

local function shifted(v, by)
	if type(v) ~= "table" then
		return v
	end
	local out = {}
	for k, x in v do
		-- (A todo's `done` is a count, not a time; so are a progress row's
		-- steps.)
		if k == "todo" or k == "steps" then
			out[k] = x
		elseif TIME_KEYS[k] and type(x) == "number" then
			out[k] = x + by
		else
			out[k] = shifted(x, by)
		end
	end
	return out
end

function sample.at(t: number)
	local copy = shifted(sample, t - NOW)
	copy.NOW = t
	copy.at = sample.at
	copy.fleet_at = sample.fleet_at
	return copy
end

-- A busier world, for the Map's three views: four machines (this desk, a
-- macbook, a Windows box and a Cloud machine), four projects, five
-- orchestrators and their crews across them, one plain thread working on
-- its own, and the last hour of what each thread did
-- (`orchestration.timeline`). Shaped as the sample is, plus:
--
--   sample.fleet.projects     sessions.now projects
--   sample.fleet.timeline     what orchestration.timeline:get() returns
--   sample.fleet_at(t)        a copy with every time shifted so "now" is `t`
do
	local function crew_chat(id: string, title: string, device: string, project: string, parent: string, status: string?, activity: any, todo: any, extra: any?): any
		local c: any = {
			id = id, title = title, device = device, project = project, parent = parent, child_kind = "crew",
			status = status, updated = ago(30), created = ago(5400), agent = "claude", model = "claude-opus-5-5",
		}
		if activity then
			c.activity = { kind = activity[1], label = activity[2], since = ago(activity[3]) }
		end
		if todo then
			c.todo = { done = todo[1], total = todo[2], current = todo[3] }
		end
		for k, v in extra or {} do
			c[k] = v
		end
		return c
	end
	local function orch(id: string, name: string, hue: number, project: string?, goal: string, extra: any?): any
		local o: any = {
			id = id, kind = "standard", name = name, goal = goal, rules = "", crew_rules = "", notes = "", hue = hue,
			crew = { devices = {} }, paused = false, archived = false, created = ago(86400), updated = ago(60), project = project,
		}
		for k, v in extra or {} do
			o[k] = v
		end
		return o
	end

	local f_devices = {
		{ id = "dev-desk", name = "desk", platform = "linux", here = true, online = true },
		{ id = "dev-mac", name = "macbook", platform = "macos", here = false, online = true },
		{ id = "dev-forge", name = "winbox", platform = "windows", here = false, online = true },
		{ id = "dev-cloud", name = "Cloud", platform = "linux", here = false, online = true, cloud = true },
	}
	local f_projects = {
		{ id = "p-zeron-desk", name = "zeron", device = "dev-desk", path = "/home/you/zeron", git = true },
		{ id = "p-exnomic-desk", name = "exnomic", device = "dev-desk", path = "/home/you/exnomic", git = true },
		{ id = "p-zeron-mac", name = "zeron", device = "dev-mac", path = "/Users/you/zeron", git = true },
		{ id = "p-reins-mac", name = "reins", device = "dev-mac", path = "/Users/you/reins", git = true },
		{ id = "p-reins-forge", name = "reins", device = "dev-forge", path = "C:\\src\\reins", git = true },
		{ id = "p-exnomic-cloud", name = "exnomic", device = "dev-cloud", path = "/work/exnomic", git = true },
		{ id = "p-tally-cloud", name = "tally", device = "dev-cloud", path = "/work/tally", git = true },
	}
	local f_orchestrators = {
		orch("orc-brain", "Brain dump", 262, nil, "Route everything I say to the right place.", { kind = "brain_dump" }),
		orch("orc-orch", "Orchestrators", 196, "p-zeron-desk", "Ship Orchestrators and the Control room in zgx."),
		orch("orc-vpn", "Exnomic VPN", 32, "p-exnomic-desk", "Phase 2 migration, safely."),
		orch("orc-reins", "Reins release", 318, "p-reins-mac", "Ship Reins 2.4 on macOS and Windows."),
		orch("orc-tally", "Tally", 100, "p-tally-cloud", "Line counts that never lie."),
	}
	local f_chats = {
		{ id = "orc-brain", title = "Brain dump", device = "dev-desk", updated = ago(40), created = ago(518400), agent = "claude", model = "claude-haiku-5-5" },
		{ id = "orc-orch", title = "Orchestrators", device = "dev-desk", project = "p-zeron-desk", status = "working", updated = ago(20), created = ago(7200), agent = "claude", model = "claude-opus-5-5",
			activity = { kind = "delegating", label = "Message Engine: reports + MCP tools", since = ago(25) } },
		{ id = "orc-vpn", title = "Exnomic VPN", device = "dev-desk", project = "p-exnomic-desk", updated = ago(900), created = ago(259200), agent = "claude", model = "claude-sonnet-5-5" },
		{ id = "orc-reins", title = "Reins release", device = "dev-mac", project = "p-reins-mac", updated = ago(300), created = ago(90000), agent = "claude", model = "claude-opus-5-5" },
		{ id = "orc-tally", title = "Tally", device = "dev-cloud", project = "p-tally-cloud", updated = ago(340), created = ago(180000), agent = "codex", model = "gpt-5.5" },
		crew_chat("th-engine-state", "Engine: orchestration state", "dev-desk", "p-zeron-desk", "orc-orch", "working",
			{ "testing", "cargo test -p zeron-engine --lib orchestration", 48 }, { 5, 8, "WatchOrchestration stream" }),
		crew_chat("th-engine-flow", "Engine: reports + MCP tools", "dev-desk", "p-zeron-desk", "orc-orch", "working",
			{ "merging", "git merge orch/engine-state", 9 }, { 7, 9, "Merge engine-state" }),
		crew_chat("th-ui-control", "Control room UI", "dev-mac", "p-zeron-mac", "orc-orch", "working",
			{ "testing", "zgx test ui control_", 95 }, { 2, 7, "Map widget" }),
		crew_chat("th-ui-orch", "Orchestrator screen", "dev-mac", "p-zeron-mac", "orc-orch", "input",
			nil, { 4, 6, "Crew updates card" }, { updated = ago(200) }),
		crew_chat("th-vpn-moscow", "Moscow node reboot plan", "dev-desk", "p-exnomic-desk", "orc-vpn", nil,
			nil, { 3, 3 }, { updated = ago(2100) }),
		crew_chat("th-vpn-exporter", "Exporter image", "dev-cloud", "p-exnomic-cloud", "orc-vpn", "working",
			{ "building", "docker build -t exnomic/exporter .", 260 }, { 2, 5, "Image builds" }),
		crew_chat("th-reins-win", "Windows installer", "dev-forge", "p-reins-forge", "orc-reins", "working",
			{ "building", "cargo build --release --target x86_64-pc-windows-msvc", 410 }, { 3, 5, "MSI bundle" }),
		crew_chat("th-reins-notes", "Release notes", "dev-mac", "p-reins-mac", "orc-reins", "working",
			{ "editing", "Edit CHANGELOG.md", 30 }, { 1, 3, "2.4 highlights" }),
		crew_chat("th-reins-sign", "Code signing", "dev-forge", "p-reins-forge", "orc-reins", nil,
			nil, { 2, 2 }, { updated = ago(1400) }),
		crew_chat("th-tally-ci", "pytest on 3.13", "dev-cloud", "p-tally-cloud", "orc-tally", "failed",
			nil, { 4, 6, "Fix empty input" }, { agent = "codex", model = "gpt-5.5", updated = ago(340) }),
		crew_chat("th-tally-lines", "--lines flag", "dev-cloud", "p-tally-cloud", "orc-tally", "working",
			{ "thinking", "", 70 }, { 2, 4, "-l alias" }, { agent = "codex", model = "gpt-5.5" }),
		crew_chat("th-tally-docs", "README usage", "dev-cloud", "p-tally-cloud", "orc-tally", "done",
			nil, { 2, 2 }, { agent = "codex", model = "gpt-5.5", updated = ago(2600) }),
		-- A thread of the person's own, working, under no orchestrator.
		{ id = "th-zgx-mem", title = "zgx memory after tabs", device = "dev-desk", project = "p-zeron-desk", status = "working",
			updated = ago(10), created = ago(1600), agent = "claude", model = "claude-opus-5-5",
			activity = { kind = "thinking", label = "", since = ago(120) } },
		{ id = "th-titlebar", title = "Linux titlebar toggle", device = "dev-desk", project = "p-zeron-desk", status = "done", updated = ago(26000), created = ago(30000), agent = "claude", model = "claude-opus-5-5" },
		-- What the Brain dump started from the person's dumps: threads of
		-- their own, on whichever machine the project lives.
		{ id = "th-dev-names", title = "Verify unique device names", device = "dev-mac", project = "p-zeron-mac", status = "working",
			updated = ago(6), created = ago(700), agent = "claude", model = "claude-sonnet-5-5",
			activity = { kind = "testing", label = "cargo test -p zeron-engine devices::names", since = ago(80) }, todo = { done = 2, total = 4, current = "-1, -2 suffixes" } },
		{ id = "th-omarchy", title = "Rerun omarchy-update", device = "dev-desk", status = "done",
			updated = ago(1900), created = ago(2400), agent = "claude", model = "claude-haiku-5-5", todo = { done = 1, total = 1 } },
		{ id = "th-wg-audit", title = "Audit wg peers", device = "dev-cloud", project = "p-exnomic-cloud", status = "input",
			updated = ago(150), created = ago(1100), agent = "claude", model = "claude-sonnet-5-5", todo = { done = 3, total = 5, current = "Which peers to drop?" } },
	}
	local function member(chat: string, o: string, item: string, extra: any?): any
		local m: any = { chat_id = chat, orchestrator_id = o, item_id = item, role = "", released = false, joined = ago(5000) }
		for k, v in extra or {} do
			m[k] = v
		end
		return m
	end
	local f_crew = {
		member("th-engine-state", "orc-orch", "it-engine"),
		member("th-engine-flow", "orc-orch", "it-flow"),
		member("th-ui-control", "orc-orch", "it-control"),
		member("th-ui-orch", "orc-orch", "it-orch-screen"),
		member("th-vpn-moscow", "orc-vpn", "it-moscow"),
		member("th-vpn-exporter", "orc-vpn", "it-exporter"),
		member("th-reins-notes", "orc-reins", "it-notes"),
		member("th-reins-win", "orc-reins", "it-win"),
		member("th-reins-sign", "orc-reins", "it-sign"),
		member("th-tally-lines", "orc-tally", "it-lines"),
		member("th-tally-ci", "orc-tally", "it-ci"),
		member("th-tally-docs", "orc-tally", "it-docs"),
	}
	local function item(id: string, o: string, title: string, state: string, order: number, threads: { string }): any
		return { id = id, orchestrator_id = o, title = title, detail = "", state = state, priority = "normal", threads = threads, links = {}, order = order, created = ago(5000), updated = ago(300) }
	end
	local f_items = {
		item("it-engine", "orc-orch", "Engine: orchestration state", "active", 1, { "th-engine-state" }),
		item("it-flow", "orc-orch", "Reports, asks, MCP tools", "active", 2, { "th-engine-flow" }),
		item("it-control", "orc-orch", "Control room", "active", 3, { "th-ui-control" }),
		item("it-orch-screen", "orc-orch", "Orchestrator screen", "blocked", 4, { "th-ui-orch" }),
		item("it-moscow", "orc-vpn", "Reboot Moscow node", "review", 1, { "th-vpn-moscow" }),
		item("it-exporter", "orc-vpn", "Exporter image", "active", 2, { "th-vpn-exporter" }),
		item("it-notes", "orc-reins", "Release notes", "active", 1, { "th-reins-notes" }),
		item("it-win", "orc-reins", "Windows installer", "active", 2, { "th-reins-win" }),
		item("it-sign", "orc-reins", "Code signing", "done", 3, { "th-reins-sign" }),
		item("it-lines", "orc-tally", "--lines flag", "active", 1, { "th-tally-lines" }),
		item("it-ci", "orc-tally", "CI on 3.13", "blocked", 2, { "th-tally-ci" }),
		item("it-docs", "orc-tally", "README usage", "done", 3, { "th-tally-docs" }),
	}
	local f_asks = {
		{ id = "ak-f1", from_chat = "th-ui-orch", orchestrator_id = "orc-orch", item_id = "it-orch-screen", kind = "decision",
			question = "Orchestrator page: side by side, or tabs?", options = { { label = "Side by side" }, { label = "Tabs" } },
			urgent = false, state = "open", created = ago(200) },
	}
	-- The person's dumps, and where the Brain dump put each part.
	local function route(id: string, kind: string, target: string, excerpt: string): any
		return { id = id, excerpt = excerpt, target = { kind = kind, id = target }, intent = "task", message = excerpt, state = "delivered", candidates = {} }
	end
	local f_dumps = {
		{ id = "dp-f2", text = "make sure no two devices end up with the same name, and the orchestrator page goes side by side. also the reins notes before the msi",
			state = "routed", device = "dev-desk", created = ago(710), updated = ago(630), routes = {
				route("rf-1", "new_thread", "th-dev-names", "make sure no two devices end up with the same name"),
				route("rf-2", "chat", "th-ui-orch", "the orchestrator page goes side by side"),
				route("rf-3", "orchestrator", "orc-reins", "the reins notes before the msi"),
			} },
		{ id = "dp-f1", text = "rerun omarchy update, and someone audit the wg peers on the exnomic box",
			state = "routed", device = "dev-desk", created = ago(2420), updated = ago(1100), routes = {
				route("rf-4", "new_thread", "th-omarchy", "rerun omarchy update"),
				route("rf-5", "new_thread", "th-wg-audit", "audit the wg peers on the exnomic box"),
			} },
	}
	local f_loads = {
		["dev-desk"] = { cpu = 0.71, mem_used = 41.2e9, mem_total = 62.0e9, load1 = 11.4, cores = 16, online = true, at = NOW },
		["dev-mac"] = { cpu = 0.46, mem_used = 22.1e9, mem_total = 36.0e9, load1 = 5.2, cores = 12, online = true, at = NOW },
		["dev-forge"] = { cpu = 0.88, mem_used = 25.7e9, mem_total = 32.0e9, load1 = 14.1, cores = 16, online = true, at = NOW },
		["dev-cloud"] = { cpu = 0.34, mem_used = 5.9e9, mem_total = 16.0e9, load1 = 1.6, cores = 8, online = true, at = NOW },
	}
	-- What passed between them in the last hour (the Timeline's ticks, the
	-- Map's pulses).
	local f_comms = {
		{ id = "cf-1", kind = "spawn", from = "orc-vpn", to = "th-vpn-exporter", summary = "Exporter image", at = ago(1500) },
		{ id = "cf-2", kind = "report", from = "th-engine-state", to = "orc-orch", summary = "Registry kinds in", at = ago(1500) },
		{ id = "cf-3", kind = "report", from = "th-reins-win", to = "orc-reins", summary = "MSVC link error, retrying", at = ago(1700) },
		{ id = "cf-4", kind = "report", from = "th-ui-control", to = "orc-orch", summary = "2 tests fail at 900 wide", at = ago(1250) },
		{ id = "cf-5", kind = "report", from = "th-vpn-moscow", to = "orc-vpn", summary = "Reboot plan ready", at = ago(2100) },
		{ id = "cf-6", kind = "report", from = "th-tally-docs", to = "orc-tally", summary = "README pushed", at = ago(2600) },
		{ id = "cf-7", kind = "report", from = "th-engine-flow", to = "orc-orch", summary = "Coalesced wake works", at = ago(300) },
		{ id = "cf-8", kind = "report", from = "th-reins-notes", to = "orc-reins", summary = "Draft up", at = ago(300) },
		{ id = "cf-9", kind = "ask", from = "th-ui-orch", to = "you", summary = "Side by side or tabs?", at = ago(200) },
		{ id = "cf-10", kind = "report", from = "th-tally-ci", to = "orc-tally", summary = "pytest: 3 failed", at = ago(340) },
		{ id = "cf-11", kind = "message", from = "orc-orch", to = "th-engine-flow", summary = "Merge engine-state first", at = ago(25) },
		{ id = "cf-12", kind = "spawn", from = "orc-brain", to = "th-dev-names", summary = "Verify unique device names", at = ago(700) },
		{ id = "cf-13", kind = "spawn", from = "orc-brain", to = "th-wg-audit", summary = "Audit wg peers", at = ago(1100) },
		{ id = "cf-14", kind = "route", from = "orc-brain", to = "th-ui-orch", summary = "Side by side, not tabs", at = ago(640) },
		{ id = "cf-15", kind = "route", from = "orc-brain", to = "orc-reins", summary = "Ship notes before the MSI", at = ago(630) },
		{ id = "cf-16", kind = "ask", from = "th-wg-audit", to = "you", summary = "Which peers to drop?", at = ago(150) },
	}
	-- Each thread's last hour: from `at` on, its state and what it did.
	local function lane(list: { any }): { any }
		local out = {}
		for _, e in list do
			table.insert(out, { at = ago(e[1]), status = e[2], kind = e[3] })
		end
		return out
	end
	local f_timeline = {
		["orc-orch"] = lane { { 3600, "working", "delegating" }, { 3420, "idle" }, { 1260, "working", "thinking" }, { 1190, "idle" }, { 25, "working", "delegating" } },
		["orc-reins"] = lane { { 3150, "working", "delegating" }, { 3020, "idle" }, { 1700, "working", "thinking" }, { 1640, "idle" } },
		["orc-tally"] = lane { { 2100, "working", "delegating" }, { 2000, "idle" }, { 340, "working", "thinking" }, { 300, "idle" } },
		["th-engine-state"] = lane {
			{ 3300, "working", "thinking" }, { 3050, "working", "editing" }, { 2500, "working", "building" }, { 2300, "working", "testing" },
			{ 2050, "working", "editing" }, { 1500, "idle" }, { 1380, "working", "thinking" }, { 1200, "working", "editing" },
			{ 700, "working", "building" }, { 560, "working", "editing" }, { 48, "working", "testing" },
		},
		["th-engine-flow"] = lane {
			{ 3500, "working", "thinking" }, { 3200, "working", "editing" }, { 2200, "working", "testing" }, { 2000, "working", "editing" },
			{ 1000, "working", "testing" }, { 820, "working", "merging" }, { 760, "working", "editing" }, { 300, "working", "testing" }, { 140, "working", "editing" },
			{ 9, "working", "merging" },
		},
		["th-ui-control"] = lane {
			{ 2800, "working", "thinking" }, { 2600, "working", "editing" }, { 1600, "working", "building" }, { 1400, "working", "testing" },
			{ 1250, "failed" }, { 1180, "working", "editing" }, { 600, "working", "building" }, { 420, "working", "editing" },
			{ 95, "working", "testing" },
		},
		["th-ui-orch"] = lane {
			{ 3000, "working", "thinking" }, { 2700, "working", "editing" }, { 1800, "working", "testing" }, { 1650, "working", "editing" },
			{ 900, "working", "reading" }, { 620, "working", "editing" }, { 200, "input" },
		},
		["th-vpn-moscow"] = lane { { 3550, "working", "reading" }, { 2900, "working", "editing" }, { 2100, "done" } },
		["th-vpn-exporter"] = lane { { 1500, "working", "thinking" }, { 1300, "working", "editing" }, { 700, "working", "installing" }, { 520, "working", "editing" }, { 260, "working", "building" } },
		["th-reins-win"] = lane {
			{ 3100, "working", "thinking" }, { 2900, "working", "installing" }, { 2300, "working", "building" }, { 1700, "failed" },
			{ 1500, "working", "editing" }, { 1100, "working", "building" }, { 800, "working", "testing" }, { 410, "working", "building" },
		},
		["th-reins-notes"] = lane { { 900, "working", "reading" }, { 600, "working", "editing" }, { 300, "working", "reviewing" }, { 30, "working", "editing" } },
		["th-reins-sign"] = lane { { 2600, "working", "running" }, { 2200, "working", "pushing" }, { 2000, "done" }, { 1400, "idle" } },
		["th-tally-ci"] = lane { { 2000, "working", "thinking" }, { 1800, "working", "editing" }, { 900, "working", "testing" }, { 340, "failed" } },
		["th-tally-lines"] = lane { { 1000, "working", "reading" }, { 700, "working", "editing" }, { 450, "working", "testing" }, { 380, "working", "editing" }, { 70, "working", "thinking" } },
		["th-tally-docs"] = lane { { 3400, "working", "editing" }, { 2700, "working", "pushing" }, { 2600, "done" } },
		["th-dev-names"] = lane { { 700, "working", "reading" }, { 520, "working", "editing" }, { 80, "working", "testing" } },
		["th-omarchy"] = lane { { 2400, "working", "running" }, { 1900, "done" } },
		["th-wg-audit"] = lane { { 1100, "working", "reading" }, { 600, "working", "running" }, { 150, "input" } },
		["th-zgx-mem"] = lane { { 1500, "working", "reading" }, { 1100, "working", "running" }, { 600, "working", "reading" }, { 120, "working", "thinking" } },
	}
	-- Each device's CPU, a sample every five seconds for the last four minutes.
	local function wave(base: number, swing: number, n: number, seed: number): { number }
		local out = {}
		for i = 1, n do
			local v = base + swing * math.sin((i + seed) * 0.37) * math.cos((i + seed * 2) * 0.11) + swing * 0.35 * math.sin(i * 1.7 + seed)
			table.insert(out, math.clamp(v, 0.02, 0.99))
		end
		return out
	end
	-- (As the bridge keeps it: `history`, oldest first, ending on `cpu`.)
	local swings: { [string]: { number } } = { ["dev-desk"] = { 0.18, 1 }, ["dev-mac"] = { 0.14, 4 }, ["dev-forge"] = { 0.12, 7 }, ["dev-cloud"] = { 0.14, 2 } }
	for id, w in swings do
		local l: any = (f_loads :: any)[id]
		local line = wave(l.cpu, w[1], 60, w[2])
		local off = l.cpu - line[#line]
		local history = {}
		for i, v in line do
			table.insert(history, { at = NOW - (#line - i) * 5, cpu = math.clamp(v + off, 0.02, 0.99) })
		end
		l.history = history
	end
	(sample :: any).fleet = {
		NOW = NOW,
		now = { state = "ready", orchestrators = f_orchestrators, crew = f_crew, items = f_items, asks = f_asks, dumps = f_dumps },
		loads = f_loads,
		comms = f_comms,
		devices = f_devices,
		projects = f_projects,
		chats = f_chats,
		timeline = f_timeline,
	}
end

function sample.fleet_at(t: number)
	local copy = shifted((sample :: any).fleet, t - NOW)
	copy.NOW = t
	return copy
end

-- How far each chat's work has got, as orchestration.progress:get() gives
-- it (src/orchestration_progress.rs): one row of each kind.
--
--   sample.progress          by chat id, for sample.chats
--   sample.fleet.progress    by chat id, for sample.fleet.chats
do
	local function row(chat: string, device: string, state: string, extra: any): any
		local r: any = { chat_id = chat, device_id = device, state = state, stalled = false, refreshing = false, updated = ago(30) }
		for k, v in extra do
			r[k] = v
		end
		return r
	end
	local progress = {
		-- An orchestrator at work, no todo list of its own.
		["orc-orch"] = row("orc-orch", "dev-desk", "working", {
			started = ago(1500), last_step = ago(25), step = "Message Engine: reports + MCP tools",
			note = "Sequencing the merges: engine-state lands first, then flow.", note_at = ago(140), updated = ago(25),
		}),
		-- Working through steps, a long tool running, an ETA.
		["th-engine-state"] = row("th-engine-state", "dev-desk", "working", {
			started = ago(2460), last_step = ago(130), step = "cargo test -p zeron-engine --lib orchestration",
			steps = { done = 5, total = 8 }, tool = "cargo test -p zeron-engine --lib orchestration", tool_since = ago(540),
			eta_secs = 1260, note = "Registry kinds and the watch are in; running the engine tests before the ops.",
			note_at = ago(200), updated = ago(30),
		}),
		-- A fresh note on its way.
		["th-engine-flow"] = row("th-engine-flow", "dev-desk", "working", {
			started = ago(3300), last_step = ago(40), step = "report tool", steps = { done = 3, total = 9 }, eta_secs = 2700,
			note = "CrewReports part renders; wiring the report tool next.", note_at = ago(420), refreshing = true, updated = ago(12),
		}),
		-- Stalled: one command running far longer than a step takes.
		["th-ui-control"] = row("th-ui-control", "dev-mac", "working", {
			started = ago(4200), last_step = ago(1140), step = "Map widget", steps = { done = 2, total = 7 }, stalled = true,
			tool = "cargo build --release", tool_since = ago(2880),
			note = "Release build after the layout change; no new output for a while.", note_at = ago(1100), updated = ago(60),
		}),
		-- Asked the person something.
		["th-ui-orch"] = row("th-ui-orch", "dev-mac", "awaiting_input", {
			started = ago(3000), last_step = ago(200), step = "Crew updates card", steps = { done = 4, total = 6 },
			note = "Asked whether the orchestrator page is side by side or tabs.", note_at = ago(190), updated = ago(200),
		}),
		-- Waiting on background work that wakes it.
		["th-vpn-merge"] = row("th-vpn-merge", "dev-desk", "waiting", {
			started = ago(1700), last_step = ago(60), step = "Merge #412", steps = { done = 6, total = 7 },
			note = "Merge queued; waiting on CI for #412.", note_at = ago(55), updated = ago(55),
		}),
		["th-vpn-moscow"] = row("th-vpn-moscow", "dev-desk", "done", {
			started = ago(7000), last_step = ago(3500), steps = { done = 3, total = 3 },
			note = "Reboot plan ready for 02:00 MSK; needs your OK.", note_at = ago(3480), updated = ago(3480),
		}),
		["th-ci-cache"] = row("th-ci-cache", "dev-cloud", "failed", {
			started = ago(100000), last_step = ago(90100), step = "sccache on macOS runners",
			note = "sccache can't reach the bucket from the macOS runners.", note_at = ago(90000), updated = ago(90000),
		}),
	}
	sample.progress = progress

	-- The fleet's, from its chats: their todo lists, what they run, a few
	-- set apart.
	local f = (sample :: any).fleet
	local STATE = { working = "working", input = "awaiting_input", failed = "failed", done = "done" }
	local NOTES: { [string]: string } = {
		["th-engine-state"] = "Watch is in; running the engine tests before the ops.",
		["th-engine-flow"] = "Merging engine-state so the report tool builds on it.",
		["th-ui-control"] = "Map widget renders; two tests fail at 900 wide.",
		["th-ui-orch"] = "Asked whether the page is side by side or tabs.",
		["th-vpn-exporter"] = "Image builds; pushing to the registry next.",
		["th-reins-win"] = "MSVC link error fixed; rebuilding the MSI.",
		["th-reins-notes"] = "Draft up; tightening the 2.4 highlights.",
		["th-tally-ci"] = "pytest 3.13: empty input still fails.",
		["th-tally-lines"] = "Adding the -l alias and its test.",
		["th-zgx-mem"] = "Comparing heap snapshots before and after tabs.",
	}
	local out = {}
	for _, c in f.chats do
		local state = STATE[c.status or ""]
		if state and c.id ~= "orc-brain" then
			local r = row(c.id, c.device, state, {
				started = c.created + 600, last_step = if c.activity then c.activity.since else c.updated, note = NOTES[c.id],
				note_at = if NOTES[c.id] then ago(90) else nil,
			})
			if c.todo then
				r.steps = { done = c.todo.done, total = c.todo.total }
				r.step = c.todo.current
			end
			local a = c.activity
			if a and (a.kind == "testing" or a.kind == "building") and NOW - a.since >= 15 then
				r.tool, r.tool_since = a.label, a.since
			end
			if state == "working" and c.todo and c.todo.done >= 3 then
				r.eta_secs = (c.todo.total - c.todo.done) * 420
			end
			out[c.id] = r
		end
	end
	-- Stalled, and waiting on background work.
	out["th-zgx-mem"].stalled = true
	out["th-zgx-mem"].last_step = ago(980)
	out["th-vpn-exporter"].state = "waiting"
	out["th-reins-notes"].refreshing = true
	f.progress = out
end

return sample
