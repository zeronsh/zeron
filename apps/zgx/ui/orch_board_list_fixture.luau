-- The board's list view (ds/orch_board_list) as the Control room shows it on
-- a busy day: Needs you full of asks with their buttons (one question long
-- enough to wrap), stalled cards with Continue / Close it / Drop it, and a
-- run of cards whose PR waits for review. Those review cards all carry one
-- ask of the engine's, "12 PRs to review" (`prs-orc-fixes`: one for all of
-- an orchestrator's open PRs, naming no card), as the engine's rows do.
-- What the person does is noted in `calls`.
--
-- States a test or a picture sets: `board_view` ("list" | "board"),
-- `board_tab`, `compact`, `only_list` (the list by itself, as a side panel).

ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()
local OrchBoard = require("@ds/orch_board")
local List = require("@ds/orch_board_list")

local base = os.time()
local here = os.date("*t", base) :: any
local T = base - (here.hour * 3600 + here.min * 60 + here.sec) + 20 * 3600
local function ago(s: number): number
	return T - s
end
local MIN, HOUR = 60, 3600

for _, t in { "dark", "light" } do
	ui.command { id = `theme_{t}`, title = `{t} theme`, run = function()
		ui.theme.follow_system(false)
		ui.theme.current:set(`zeron-{t}`)
	end }
end

local orchestrators = ui.state("fx_orchestrators", {
	{ id = "orc-fixes", kind = "standard", name = "Zeron fixes", hue = 24, archived = false },
	{ id = "orc-vpn", kind = "standard", name = "VPN Orchestrator", hue = 280, archived = false },
	{ id = "orc-reins", kind = "standard", name = "Reins Orch", hue = 190, archived = false },
} :: { any })

local item_rows: { any } = {}
local card_rows: { any } = {}
local function card(item: any, row: any)
	item.links = item.links or {}
	item.threads = item.threads or { `th-{item.id}` }
	item.priority = item.priority or "normal"
	item.created = item.created or ago(2 * HOUR)
	item.updated = item.updated or row.since
	table.insert(item_rows, item)
	row.id = item.id
	row.old_state = item.state
	row.held = false
	row.unseen = row.unseen == true
	row.asks = row.asks or {}
	row.guests = {}
	row.events = row.events or {}
	row.seq = row.seq or 1
	row.status_since = row.status_since or row.since
	row.finish = row.finish or { line = "pr" }
	row.updated = row.since
	table.insert(card_rows, row)
end

local function opts(labels: { string }): { any }
	local out = {}
	for _, l in labels do
		table.insert(out, { label = l, images = {}, scores = {} })
	end
	return out
end

local ask_rows: { any } = {}
local function ask(a: any)
	a.state = "open"
	a.quiet = false
	a.urgent = a.urgent == true
	a.images = {}
	a.links = a.links or {}
	a.options = a.options or {}
	table.insert(ask_rows, a)
end

-- Needs you: a stalled card, then two that ask (one long question).
card(
	{ id = "it-1474", orchestrator_id = "orc-fixes", title = "PR #1474 ready to merge: CI green, new title and description", state = "blocked" },
	{ place = "needs_you", status = "stalled", since = ago(20 * HOUR), reason = "Nothing was moving on it", cause = "stalled", by = "engine", asks = { "ak-stall" } }
)
ask({ id = "ak-stall", from_chat = "orc-fixes", orchestrator_id = "orc-fixes", item_id = "it-1474", kind = "decision", origin = "stall", created = ago(20 * HOUR),
	question = "Nothing is moving on \"PR #1474 ready to merge: CI green, new title and description\". What now?",
	options = opts({ "Continue", "Close it", "Drop it" }) })
card(
	{ id = "it-deploy", orchestrator_id = "orc-vpn", title = "Deploy lane (control + nodes from main)", state = "blocked" },
	{ place = "needs_you", status = "asked", since = ago(21 * HOUR), reason = "Asked you", cause = "asked", by = "agent", asks = { "ak-deploy" } }
)
ask({ id = "ak-deploy", from_chat = "th-it-deploy", orchestrator_id = "orc-vpn", item_id = "it-deploy", kind = "approval", origin = "tool", created = ago(21 * HOUR),
	question = "The new Mac build (macOS 123, draft 454985954) is built, but downloading it from GitHub needs a Reins approval on your phone: open the Reins app and approve \"read github.com/katulevskiy/exnomic\". Done?",
	options = opts({ "Approved", "Not now" }), recommendation = { option = 1 } })
card(
	{ id = "it-plugin", orchestrator_id = "orc-fixes", title = "Orchestrators as a built-in plugin, off by default (Settings, Plugins)", state = "blocked" },
	{ place = "needs_you", status = "asked", since = ago(18 * HOUR), reason = "Asked you", cause = "asked", by = "agent", asks = { "ak-plugin" } }
)
ask({ id = "ak-plugin", from_chat = "th-it-plugin", orchestrator_id = "orc-fixes", item_id = "it-plugin", kind = "decision", origin = "tool", created = ago(18 * HOUR),
	question = "1Password is refusing to sign commits (\"failed to fill whole buffer\", then \"agent returned an error\"), probably locked or waiting for an approval on your screen. It also blocks ssh to zephyrka. The plugin work is waiting on it, with a commit half done, and the release after it waits too. Can you unlock or approve 1Password, then tell me to carry on?",
	options = opts({ "Unlocked, carry on", "Not now" }) })
-- A second stalled one, further down.
card(
	{ id = "it-usb", orchestrator_id = "orc-reins", title = "Linux USB-C Ethernet failover", state = "blocked" },
	{ place = "needs_you", status = "stalled", since = ago(170 * HOUR), reason = "Nothing was moving on it", cause = "stalled", by = "engine", asks = { "ak-usb" } }
)
ask({ id = "ak-usb", from_chat = "orc-reins", orchestrator_id = "orc-reins", item_id = "it-usb", kind = "decision", origin = "stall", created = ago(170 * HOUR),
	question = "Nothing is moving on \"Linux USB-C Ethernet failover\". What now?",
	options = opts({ "Continue", "Close it", "Drop it" }) })

-- Review: each card's PR open and its work resting; the engine's one PR
-- ask for the orchestrator stands on every one of them.
local REVIEWS: { any } = {
	{ "Registry: updates over 16 KB are refused and resent forever (fix as a PR to main)", 1570, 23 },
	{ "Android app hangs and crashes when sending to an orchestrator chat", 1571, 24 },
	{ "Cursor missing from the model picker (stale catalog), PR + orchestrators", 1503, 143 },
	{ "Per-chat Cloud machine size (picker, MCP param, engine)", 1491, 162 },
	{ "zgx PR: over-limit canvas skips only itself; frame loop re-arms", 2269, 162 },
	{ "Worktree mode fails on a repo with no commits (unborn HEAD)", 1488, 162 },
	{ "Cloud chats: can't start (GitHub repo not detected; first-turn error)", 1487, 162 },
}
local pr_links: { any } = {}
for i, r in REVIEWS do
	local url = `https://github.com/zeronsh/zeron/pull/{r[2]}`
	table.insert(pr_links, { kind = "pr", label = `PR #{r[2]}`, target = url })
	card(
		{ id = `it-review-{i}`, orchestrator_id = "orc-fixes", title = r[1], state = "review", links = { { kind = "pr", label = `PR #{r[2]}`, target = url } } },
		{
			place = "needs_you", status = "review", since = ago(r[3] * HOUR), reason = "Ready to look at", cause = "review", by = "engine",
			asks = { "prs-orc-fixes" }, finish = { line = "pr", pr = url },
			chip = { kind = "pr", label = `PR #{r[2]}`, target = url },
		}
	)
end
ask({ id = "prs-orc-fixes", from_chat = "orc-fixes", orchestrator_id = "orc-fixes", kind = "approval", origin = "review", created = ago(24 * HOUR),
	question = "12 PRs to review", links = pr_links,
	context = "- Registry: updates over 16 KB are refused\n- Android app hangs" })

-- Running and Done, a few each.
card(
	{ id = "it-chat", orchestrator_id = "orc-fixes", title = "Chat list: only changed rows update", state = "active" },
	{ place = "running", status = "working", since = ago(40 * MIN), reason = "Thread started", cause = "started", by = "agent", finish = { line = "merged", into = "zeron/orchestrators" } }
)
card(
	{ id = "it-tool", orchestrator_id = "orc-vpn", title = "Tool output error (tool_detail)", state = "active" },
	{ place = "running", status = "waiting", note = "tests", since = ago(64 * MIN), status_since = ago(18 * MIN), reason = "Thread started", cause = "started", by = "agent",
		chip = { kind = "pr", label = "PR #1503", target = "https://github.com/zeronsh/zeron/pull/1503" } }
)
card(
	{ id = "it-token", orchestrator_id = "orc-fixes", title = "Token usage breakdown", state = "done", done = ago(108 * MIN) },
	{ place = "done", status = "done", since = ago(108 * MIN), reason = "Its thread reported done", cause = "reported", by = "engine", unseen = true,
		finish = { line = "report" }, closed = { at = ago(108 * MIN), by = "engine", how = "report" }, chip = { kind = "report", label = "report" } }
)

local items = ui.state("fx_items", item_rows)
local cards = ui.state("fx_cards", card_rows)
local asks = ui.state("fx_asks", ask_rows)
local said = ui.state("calls", "")
local function note(what: string)
	said:set(if said:peek() == "" then what else `{said:peek()},{what}`)
end
local compact = ui.state("compact", false)
local only_list = ui.state("only_list", false)
local view = ui.state("board_view", "list")
local tab = ui.state("board_tab", "all")
local selected = ui.state("board_selected", nil :: string?)

local function board_props(): { [string]: any }
	return {
		items = items,
		cards = cards,
		asks = asks,
		orchestrators = orchestrators,
		now = function(): number
			return T
		end,
		thread_of = function(chat: string): any
			return { id = chat, title = chat, status = "done" }
		end,
		progress_of = function(_chat: string): any
			return nil
		end,
		view = view,
		tab = tab,
		selected = selected,
		on_open_thread = function(chat: string)
			note(`open:{chat}`)
		end,
		on_link = function(l: any)
			note(`link:{l.label}`)
		end,
		on_answer = function(a: any, choice: any): any
			note(`answer:{a.id}:{choice.option or ""}`)
			return nil
		end,
		on_expand_ask = function(a: any)
			note(`expand:{a.id}`)
		end,
		on_snooze = function(ask_id: string, until_ms: number?): any
			note(`snooze:{ask_id}:{if until_ms then "later" else "now"}`)
			return nil
		end,
		on_close = function(id: string, dropped: boolean): any
			note(`close:{id}:{dropped}`)
			return nil
		end,
		on_seen = function(id: string): any
			note(`seen:{id}`)
			return nil
		end,
	}
end

return ui.div { w_full = true, h_full = true, ui.scroll {
	id = "fixture-root",
	w_full = true,
	h_full = true,
	bg = "$background",
	ui.column {
		w_full = true,
		p = "$space.4",
		ui.presence(function(): string
			return `{if only_list:get() then "list" else "board"}:{if compact:get() then "compact" else "regular"}`
		end, function(k: string): Node
			local props = board_props()
			props.compact = string.find(k, "compact", 1, true) ~= nil
			if string.find(k, "list:", 1, true) == 1 then
				props.tab = nil
				props.on_open_card = function(id: string)
					note(`card:{id}`)
				end
				return ui.column {
					id = "fixture-list",
					w = 360,
					p = "$space.2",
					rounded = "$radius.xl",
					border = 1,
					border_color = "$border.subtle",
					List.List(props :: any),
				}
			end
			props.show_filter = true
			return OrchBoard(props :: any)
		end),
	},
}, ds.toast.Host {} }
