-- The main window on an orchestrator's page, in orchestration's sample world
-- (account/orchestration_sample): the shell (ui/shell_fixture.luau's, its
-- host stubbed), its sessions and devices the sample's, the Orchestrators
-- orchestrator open, its conversation holding a Crew updates card. For
-- tests/orchestrator_page.test.luau and the audit's pictures.
--
-- The `Orchestrate` ops asked are noted in `orch_calls` (one a line,
-- `op:key=value,…`) and answered at once. Commands:
--   orch.dark / orch.light      the theme
--   orch.vpn                    Exnomic VPN's page instead
--   orch.rich                   replies in its conversation too (markdown)

local shell = require("./shell_fixture")
local sessions = require("@account/sessions")
local orchestration = require("@account/orchestration")
-- (The sample as of now, so its times read as it means them; its todo
-- counts kept as they are, which `at` would shift with the times.)
local sample = (function(): any
	local base = require("@account/orchestration_sample")
	local moved = base.at(os.time())
	for i, c in moved.chats do
		c.todo = base.chats[i].todo
	end
	return moved
end)()
local route = require("@app/screens/shell/route")
local activity = require("@app/screens/shell/activity")

local NOW = sample.NOW
local function ago(s: number): number
	return NOW - s
end

-- The chats as the host has them: crew threads under their orchestrator
-- (`children`), the rest top-level; a few of the person's own beside them.
local chats, children = {}, {}
for _, c in sample.chats do
	if c.parent then
		children[c.parent] = children[c.parent] or {}
		table.insert(children[c.parent], c)
	else
		table.insert(chats, table.clone(c))
	end
end
for _, c in chats do
	if c.id == "th-titlebar" then
		c.project = "zeron"
	end
end
table.insert(chats, { id = "own-relay", title = "Relay reconnect flake", project = "zeron", device = "dev-desk", status = "working", updated = ago(70), created = ago(4000), agent = "claude", model = "claude-sonnet-5-5" })
table.insert(chats, { id = "own-docs", title = "Plugin docs pass", project = "zeron", device = "dev-mac", updated = ago(5400), created = ago(9000), agent = "openai", harness = "codex" })
table.insert(chats, { id = "own-vpn", title = "wg exporter dashboards", project = "exnomic", device = "dev-desk", status = "done", updated = ago(8000), created = ago(12000), agent = "claude" })
activity.on:set(true)
sessions.now:set({
	state = "ready",
	here = "dev-desk",
	devices = sample.devices,
	projects = {
		{ id = "zeron", name = "zeron", device = "dev-desk", path = "/home/me/zeron", git = true },
		{ id = "exnomic", name = "exnomic", device = "dev-desk", path = "/home/me/exnomic", git = true },
	},
	chats = chats,
	archived = {},
	children = children,
	places = { pinned = {}, sections = {} },
	places_ready = true,
} :: any)
orchestration.now:set(sample.now)
orchestration.comms:set(sample.comms)
orchestration.loads:set(sample.loads)

-- Its conversation: the person's ask, its plan, the crew's reports filed
-- as one Crew updates card, and its answer to them.
local function report(id: string, from: string, state: string, summary: string, at: number): any
	return { id = id, from_chat = from, state = state, summary = summary, at = at }
end
local function conversation(rich: boolean): { any }
	local out: { any } = {
		{ id = "o-1", role = "user", text = "Ship Orchestrators and the Control room in zgx. Engine first, then both screens; one thread each.", at = "Nov 12, 4:41 PM" },
	}
	if rich then
		table.insert(out, {
			id = "o-2",
			role = "assistant",
			text = "Four items on the board, one thread each:\n\n- **Engine state + RPC**: registry kinds, `WatchOrchestration`\n- **Reports, asks, MCP tools**\n- **Control room**\n- **Orchestrator screen**\n\nThe two UI threads build against the sample until the engine lands.",
			at = "Nov 12, 4:43 PM",
		})
	end
	table.insert(out, {
		id = "o-3",
		role = "crew_reports",
		text = "",
		at = "Nov 12, 6:37 PM",
		reports = {
			report("rp-1", "th-engine-state", "progress", "Registry kinds + WatchOrchestration in; ops next", ago(600)),
			report("rp-2", "th-engine-flow", "progress", "CrewReports part renders; coalesced wake works in tests", ago(300)),
			report("rp-3", "th-ui-orch", "blocked", "Needs a decision: tabs or split for the orchestrator page", ago(200)),
			report("rp-6", "th-ui-control", "message", "Map lanes in; board next", ago(150)),
		},
	})
	if rich then
		table.insert(out, {
			id = "o-4",
			role = "assistant",
			text = "Engine is moving. The orchestrator screen is waiting on you: **side by side or tabs** is in Needs you.",
			at = "Nov 12, 6:38 PM",
		})
	end
	return out
end
local function vpn_conversation(): { any }
	return {
		{ id = "v-1", role = "user", text = "Phase 2 migration, safely. Nothing touches prod without asking me.", at = "Nov 9, 10:02 AM" },
		{
			id = "v-2",
			role = "crew_reports",
			text = "",
			at = "Nov 12, 5:42 PM",
			reports = {
				report("rp-4", "th-vpn-moscow", "done", "Reboot plan ready; needs your OK for 02:00 MSK", ago(3500)),
				report("rp-7", "th-vpn-merge", "progress", "#412 green; merging", ago(40)),
			},
		},
	}
end
local fixture_chats = ui.shared("fixture.chats", {} :: { [string]: any })
local function show(rich: boolean)
	local c: { [string]: any } = {}
	for _, id in { "orc-orch", "orc-vpn" } do
		c[id] = {
			messages = if id == "orc-vpn" then vpn_conversation() else conversation(rich),
			session = { id = id, state = "ready", title = id, device = "dev-desk", harness = "claude-code", model = "claude-code/opus-5.5" },
		}
	end
	fixture_chats:set(c)
end
show(false)

local calls = ui.state("orch_calls", "")
local function settled(value: any): any
	return {
		after = function(_self: any, done: (any) -> (), _failed: ((any) -> ())?)
			done(value)
		end,
	}
end
local function said(op: { [string]: any }): string
	local keys: { string } = {}
	for k in op do
		if k ~= "op" then
			table.insert(keys, k)
		end
	end
	table.sort(keys)
	local parts: { string } = {}
	for _, k in keys do
		local v = op[k]
		table.insert(parts, `{k}={if type(v) == "table" then "{…}" else tostring(v)}`)
	end
	return `{op.op}:{table.concat(parts, ",")}`
end
;(orchestration :: any).op = function(op: { [string]: any }): any
	local line = said(op)
	calls:set(if calls:peek() == "" then line else `{calls:peek()}\n{line}`)
	if op.op == "promoteChat" then
		return settled({ id = op.chatId })
	end
	return settled({ id = "orc-orch" })
end

-- Where the route stands, for the tests to read.
local where = ui.state("orch_route", "")
ui.effect(function()
	local c = route.current:get()
	where:set(`{c.name}:{c.orchestrator or c.session or ""}`)
end)

ui.command { id = "orch.dark", title = "Dark", run = function()
	ui.theme.follow_system(false)
	ui.theme.current:set("zeron-dark")
end }
ui.command { id = "orch.light", title = "Light", run = function()
	ui.theme.follow_system(false)
	ui.theme.current:set("zeron-light")
end }
ui.command { id = "orch.vpn", title = "Exnomic VPN", run = function()
	route.orchestrator("orc-vpn")
end }
ui.command { id = "orch.orch", title = "Orchestrators", run = function()
	route.orchestrator("orc-orch")
end }
ui.command { id = "orch.rich", title = "Replies too", run = function()
	show(true)
end }

route.orchestrator("orc-orch")
return shell
