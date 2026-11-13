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
		text = "ok so the orchestrator page should be side by side not tabs, also remind me to rerun omarchy update it needs sudo. and ask the vpn guy whether moscow rebooted fine. the CI thing can wait till next week, and is the engine state thread still on the registry part?",
		state = "routed",
		summary = "5 parts: 2 to Orchestrators, 1 to Exnomic VPN, 1 to CI speed, 1 to your inbox.",
		device = "dev-desk",
		created = ago(40),
		routes = {
			{ id = "rt-5", excerpt = "the orchestrator page should be side by side not tabs", span = { from = 6, to = 59 },
				target = { kind = "chat", id = "th-ui-orch" }, intent = "info", message = "Decision from the person: side by side (chat left, crew + board right), not tabs.", state = "delivered", candidates = {} },
			{ id = "rt-6", excerpt = "remind me to rerun omarchy update it needs sudo", span = { from = 66, to = 113 },
				target = { kind = "item", id = "it-omarchy" }, intent = "task", message = "Rerun omarchy-update (needs sudo).", state = "delivered", candidates = {}, item_id = "it-omarchy" },
			{ id = "rt-7", excerpt = "ask the vpn guy whether moscow rebooted fine", span = { from = 119, to = 163 },
				target = { kind = "orchestrator", id = "orc-vpn" }, intent = "question", message = "Did the Moscow node reboot cleanly? Check wg and the exporter.", state = "answered", candidates = {},
				answer = "Not yet — it's scheduled for 02:00 MSK and waiting on your approval." },
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
}

local function shifted(v, by)
	if type(v) ~= "table" then
		return v
	end
	local out = {}
	for k, x in v do
		-- (A todo's `done` is a count, not a time.)
		if k == "todo" then
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
	return copy
end

return sample
