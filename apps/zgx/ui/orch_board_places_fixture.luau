-- The board (ds/orch_board) with every place filled, as the engine's rows
-- give them (`cards`: each card's place, worked out by the engine), for
-- pictures and for tests of the board, its list and a card's detail. What
-- the person does is noted in `calls` and applied to the rows the way the
-- engine's next frame would.
--
-- States a test or a picture sets: `filter` (an orchestrator's id),
-- `compact`, `board_view` ("board" | "list"), `board_tab`, `board_selected`
-- (the open card), `board_drawers` (`{ planned = true }`), `only_list`
-- (the list view by itself, as an orchestrator's side panel shows it).

ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()
local OrchBoard = require("@ds/orch_board")
local List = require("@ds/orch_board_list")
local Detail = require("@ds/orch_board_detail")

-- "Now" is today at 8 PM here, so what was done this afternoon is today's.
local base = os.time()
local here = os.date("*t", base) :: any
local T = base - (here.hour * 3600 + here.min * 60 + here.sec) + 20 * 3600
local function ago(s: number): number
	return T - s
end
local MIN, HOUR, DAY = 60, 3600, 86400

for _, t in { "dark", "light" } do
	ui.command { id = `theme_{t}`, title = `{t} theme`, run = function()
		ui.theme.follow_system(false)
		ui.theme.current:set(`zeron-{t}`)
	end }
end

local orchestrators = ui.state("fx_orchestrators", {
	{ id = "orc-fixes", kind = "standard", name = "Zeron fixes", hue = 176, archived = false },
	{ id = "orc-vpn", kind = "standard", name = "Exnomic VPN", hue = 18, archived = false },
	{ id = "orc-ci", kind = "standard", name = "CI speed", hue = 96, archived = false },
} :: { any })

local chats: { [string]: any } = {
	["th-merge"] = { id = "th-merge", title = "Merge main 0.3.8", status = "done" },
	["th-admin"] = { id = "th-admin", title = "Admin app redesign", status = "failed" },
	["th-relay"] = { id = "th-relay", title = "RU-entry relay", status = "done" },
	["th-prs"] = { id = "th-prs", title = "PR sweep", status = "done" },
	["th-chat"] = { id = "th-chat", title = "Chat list rows", status = "working" },
	["th-tool"] = { id = "th-tool", title = "Tool output error", status = "waiting" },
	["th-tel-server"] = { id = "th-tel-server", title = "Telemetry server", status = "done" },
	["th-tel-android"] = { id = "th-tel-android", title = "Telemetry Android", status = "working" },
	["th-release"] = { id = "th-release", title = "Release 7bb4a19a3", status = "done" },
	["th-ios"] = { id = "th-ios", title = "iOS Live screen", status = "limited" },
	["th-exits"] = { id = "th-exits", title = "Per-app exits", status = "done" },
	["th-asks"] = { id = "th-asks", title = "Board cards answer asks", status = "done" },
	["th-token"] = { id = "th-token", title = "Token usage report", status = "done" },
	["th-msi"] = { id = "th-msi", title = "Windows MSI", status = "done" },
}
local progress: { [string]: any } = {
	["th-chat"] = { state = "working", note = "Measuring on 700 chats: one status change went from 1,680 recomputes to 374. Checking the sidebar next." },
	["th-tel-android"] = { state = "working", note = "Server side is done. Wiring the Android client to it." },
}

local function ev(at: number, kind: string, text: string, extra: { [string]: any }?): any
	local e: { [string]: any } = { at = at, kind = kind, text = text }
	for k, v in extra or {} do
		e[k] = v
	end
	return e
end

-- Each card as two rows, the item and where the engine says it stands.
local seed: { { item: any, card: any } } = {
	{
		item = { id = "it-merge", orchestrator_id = "orc-fixes", title = "Bring main (0.3.8) into orchestrators", state = "blocked", priority = "high", threads = { "th-merge" }, order = 1, created = ago(2 * HOUR + 10 * MIN), updated = ago(6 * MIN), links = {} },
		card = {
			place = "needs_you", status = "asked", since = ago(6 * MIN), status_since = ago(6 * MIN), seq = 4, from = "running", reason = "Asked you to unlock and approve the push", cause = "asked", by = "agent", asks = { "ak-push" },
			finish = { line = "merged", branch = "zeron/main-merge", into = "zeron/orchestrators" },
			word = { state = "blocked", at = ago(6 * MIN), from = "th-merge", summary = "The merge is committed and its tests match both sides. Two things are left that only you can do." },
			events = {
				ev(ago(2 * HOUR), "move", "Thread started", { from = "planned", to = "running", seq = 1 }),
				ev(ago(HOUR), "report", "Reported: 48 conflicts resolved, code compiles"),
				ev(ago(20 * MIN), "report", "Reported: merge committed, tests match"),
				ev(ago(6 * MIN + 20), "thread", "Thread stopped: signing failed (1Password)"),
				ev(ago(6 * MIN), "move", "Asked you to unlock and approve the push", { from = "running", to = "needs_you", seq = 4 }),
			},
		},
	},
	{
		item = { id = "it-admin", orchestrator_id = "orc-vpn", title = "Admin app redesign", state = "blocked", priority = "normal", threads = { "th-admin" }, order = 2, created = ago(3 * HOUR), updated = ago(14 * MIN), links = {} },
		card = {
			place = "needs_you", status = "failed", since = ago(14 * MIN), status_since = ago(14 * MIN), seq = 2, from = "running", reason = "Its thread stopped on an error", cause = "failed", by = "engine", asks = { "ak-failed" },
			finish = { line = "accept" },
			events = {
				ev(ago(3 * HOUR), "move", "Thread started", { from = "planned", to = "running", seq = 1 }),
				ev(ago(14 * MIN), "move", "Its thread stopped on an error", { from = "running", to = "needs_you", seq = 2 }),
			},
		},
	},
	{
		item = { id = "it-relay", orchestrator_id = "orc-vpn", title = "RU-entry relay: resist cascade detection", state = "blocked", priority = "normal", threads = { "th-relay" }, order = 3, created = ago(5 * HOUR), updated = ago(48 * MIN), links = {} },
		card = {
			place = "needs_you", status = "stalled", since = ago(48 * MIN), status_since = ago(48 * MIN), seq = 3, from = "running", reason = "Nothing was moving on it", cause = "stalled", by = "engine", asks = { "ak-stall" },
			finish = { line = "pr" },
			word = { state = "progress", at = ago(2 * HOUR), from = "th-relay", summary = "Probe traffic now looks like plain HTTPS. Two of five carriers still reset the stream." },
			events = {
				ev(ago(5 * HOUR), "move", "Thread started", { from = "planned", to = "running", seq = 1 }),
				ev(ago(2 * HOUR), "report", "Reported: two of five carriers still reset the stream"),
				ev(ago(48 * MIN), "move", "Nothing was moving on it", { from = "running", to = "needs_you", seq = 3 }),
			},
		},
	},
	{
		item = { id = "it-prs", orchestrator_id = "orc-fixes", title = "12 pull requests to review", state = "blocked", priority = "normal", threads = { "th-prs" }, order = 4, created = ago(DAY), updated = ago(3 * HOUR), links = {} },
		card = {
			place = "snoozed", status = "review", ["until"] = T + 17 * HOUR + 37 * MIN, since = ago(3 * HOUR), status_since = ago(3 * HOUR), seq = 5, from = "needs_you", reason = "You snoozed it", cause = "snoozed", by = "you", asks = { "ak-prs" },
			finish = { line = "pr" },
			events = { ev(ago(3 * HOUR), "move", "You snoozed it", { from = "needs_you", to = "snoozed", seq = 5 }) },
		},
	},
	{
		item = { id = "it-chat", orchestrator_id = "orc-fixes", title = "Chat list: only changed rows update", state = "active", priority = "normal", threads = { "th-chat" }, order = 5, created = ago(50 * MIN), updated = ago(40 * MIN), links = {} },
		card = {
			place = "running", status = "working", since = ago(40 * MIN + 12), status_since = ago(40 * MIN + 12), seq = 1, from = "planned", reason = "Thread started", cause = "started", by = "agent",
			finish = { line = "merged", into = "zeron/orchestrators" },
			events = { ev(ago(40 * MIN), "move", "Thread started", { from = "planned", to = "running", seq = 1 }) },
		},
	},
	{
		item = { id = "it-tool", orchestrator_id = "orc-fixes", title = "Tool output error (tool_detail)", state = "active", priority = "normal", threads = { "th-tool" }, order = 6, created = ago(70 * MIN), updated = ago(18 * MIN), links = {} },
		card = {
			place = "running", status = "waiting", note = "tests", since = ago(64 * MIN), status_since = ago(18 * MIN + 3), seq = 1, from = "planned", reason = "Thread started", cause = "started", by = "agent",
			finish = { line = "pr" },
			word = { state = "progress", at = ago(19 * MIN), from = "th-tool", summary = "The fix is committed. Two test suites are running." },
			chip = { kind = "pr", label = "PR #1503", target = "https://github.com/zeronsh/zeron/pull/1503" },
			events = {
				ev(ago(64 * MIN), "move", "Thread started", { from = "planned", to = "running", seq = 1 }),
				ev(ago(19 * MIN), "report", "Reported: the fix is committed, two test suites are running"),
			},
		},
	},
	{
		item = { id = "it-telemetry", orchestrator_id = "orc-vpn", title = "Telemetry v2", state = "active", priority = "normal", threads = { "th-tel-android", "th-tel-server" }, order = 7, created = ago(2 * HOUR + 30 * MIN), updated = ago(9 * MIN), links = {} },
		card = {
			place = "running", status = "working", since = ago(2 * HOUR + 5 * MIN), status_since = ago(2 * HOUR + 5 * MIN), seq = 1, from = "planned", reason = "Thread started", cause = "started", by = "agent",
			finish = { line = "report" },
			events = { ev(ago(2 * HOUR + 5 * MIN), "move", "Thread started", { from = "planned", to = "running", seq = 1 }) },
		},
	},
	{
		item = { id = "it-release", orchestrator_id = "orc-fixes", title = "Release 7bb4a19a3", state = "active", priority = "normal", threads = { "th-release" }, order = 8, created = ago(55 * MIN), updated = ago(12), links = {} },
		card = {
			place = "running", status = "checking", since = ago(50 * MIN), status_since = ago(12), seq = 1, from = "planned", reason = "Thread started", cause = "started", by = "agent",
			finish = { line = "report" },
			word = { state = "done", at = ago(14), from = "th-release", summary = "Its thread finished: live check clean, data counts match. The orchestrator is checking the result." },
			checking = { since = ago(12), why = "report" },
			events = {
				ev(ago(50 * MIN), "move", "Thread started", { from = "planned", to = "running", seq = 1 }),
				ev(ago(14), "report", "Reported done: live check clean, data counts match"),
			},
		},
	},
	{
		item = { id = "it-ios", orchestrator_id = "orc-ci", title = "iOS Live screen", state = "active", priority = "normal", threads = { "th-ios" }, order = 9, created = ago(4 * HOUR), updated = ago(25 * MIN), links = {} },
		card = {
			place = "running", status = "limited", ["until"] = T + 100 * MIN, since = ago(3 * HOUR), status_since = ago(25 * MIN), seq = 1, from = "planned", reason = "Thread started", cause = "started", by = "agent",
			finish = { line = "pr" },
			word = { state = "progress", at = ago(30 * MIN), from = "th-ios", summary = "The Live screen lists rooms. The join flow is next." },
			events = { ev(ago(3 * HOUR), "move", "Thread started", { from = "planned", to = "running", seq = 1 }) },
		},
	},
	{
		item = { id = "it-keyed", orchestrator_id = "orc-fixes", title = "Keyed sidebar shape", detail = "Give every sidebar row a stable key so a push rebuilds only the rows it changed.", state = "planned", priority = "high", threads = {}, order = 1, created = ago(90 * MIN), updated = ago(90 * MIN), links = {} },
		card = { place = "planned", status = "next", since = ago(90 * MIN), status_since = ago(90 * MIN), seq = 0, cause = "created", by = "agent", finish = { line = "merged", into = "zeron/orchestrators" }, events = {} },
	},
	{
		item = { id = "it-win", orchestrator_id = "orc-ci", title = "Windows installer smoke test", state = "planned", priority = "normal", threads = {}, order = 2, created = ago(80 * MIN), updated = ago(80 * MIN), links = {}, depends_on = { "it-release" } },
		card = { place = "planned", status = "after", note = "Release 7bb4a19a3", since = ago(80 * MIN), status_since = ago(80 * MIN), seq = 0, cause = "waiting", by = "engine", finish = { line = "report" }, events = {} },
	},
	{
		item = { id = "it-exits", orchestrator_id = "orc-vpn", title = "Per-app exit preferences", state = "blocked", paused_at = ago(2 * HOUR), priority = "normal", threads = { "th-exits" }, order = 3, created = ago(DAY), updated = ago(2 * HOUR), links = {} },
		card = {
			place = "planned", status = "paused", since = ago(2 * HOUR), status_since = ago(2 * HOUR), seq = 2, from = "running", reason = "You paused it", cause = "paused", by = "you", finish = { line = "pr" },
			events = { ev(ago(2 * HOUR), "move", "You paused it", { from = "running", to = "planned", seq = 2 }) },
		},
	},
	{
		item = { id = "it-probes", orchestrator_id = "orc-vpn", title = "Speed probes on the admin page", detail = "Someday: show each exit's measured speed beside its name.", state = "inbox", priority = "low", threads = {}, order = 4, created = ago(3 * DAY), updated = ago(3 * DAY), links = {} },
		card = { place = "planned", status = "held", held = true, since = ago(3 * DAY), status_since = ago(3 * DAY), seq = 0, cause = "held", by = "agent", finish = { line = "report" }, events = {} },
	},
	{
		item = { id = "it-asks", orchestrator_id = "orc-fixes", title = "Board cards on the orchestrator page answer asks", state = "done", priority = "normal", threads = { "th-asks" }, order = 10, created = ago(62 * MIN), updated = ago(31 * MIN), done = ago(31 * MIN), links = {} },
		card = {
			place = "done", status = "done", since = ago(31 * MIN), status_since = ago(31 * MIN), seq = 3, from = "running", reason = "Merged in e99f817", cause = "merged", by = "engine", unseen = true,
			finish = { line = "merged", branch = "zeron/card-asks", into = "zeron/orchestrators" },
			closed = { at = ago(31 * MIN), by = "engine", how = "merged" },
			chip = { kind = "merged", label = "merged e99f817" },
			word = { state = "done", at = ago(33 * MIN), from = "th-asks", summary = "Merged into zeron/orchestrators and live in the dev app." },
			events = {
				ev(ago(62 * MIN), "move", "Thread started", { from = "planned", to = "running", seq = 1 }),
				ev(ago(33 * MIN), "report", "Reported done: merged and live in the dev app"),
				ev(ago(31 * MIN), "move", "Merged in e99f817", { from = "running", to = "done", seq = 3 }),
			},
		},
	},
	{
		item = { id = "it-token", orchestrator_id = "orc-fixes", title = "Token usage breakdown", state = "done", priority = "normal", threads = { "th-token" }, order = 11, created = ago(2 * HOUR), updated = ago(108 * MIN), done = ago(108 * MIN), links = {} },
		card = {
			place = "done", status = "done", since = ago(108 * MIN), status_since = ago(108 * MIN), seq = 2, from = "running", reason = "Its thread reported done", cause = "reported", by = "engine", unseen = true,
			finish = { line = "report" },
			closed = { at = ago(108 * MIN), by = "engine", how = "report" },
			chip = { kind = "report", label = "report" },
			word = { state = "done", at = ago(109 * MIN), from = "th-token", summary = "$7,840 in 14 days; 23% is orchestration overhead. The answer is in Q&A." },
			events = {
				ev(ago(2 * HOUR), "move", "Thread started", { from = "planned", to = "running", seq = 1 }),
				ev(ago(108 * MIN), "move", "Its thread reported done", { from = "running", to = "done", seq = 2 }),
			},
		},
	},
	{
		item = { id = "it-msi", orchestrator_id = "orc-ci", title = "Deploy lane: Windows MSI 65621", state = "done", priority = "normal", threads = { "th-msi" }, order = 12, created = ago(4 * HOUR), updated = ago(3 * HOUR + 12 * MIN), done = ago(3 * HOUR + 12 * MIN), links = {} },
		card = {
			place = "done", status = "done", since = ago(3 * HOUR + 12 * MIN), status_since = ago(3 * HOUR + 12 * MIN), seq = 2, from = "running", reason = "Its thread reported done", cause = "reported", by = "engine", unseen = false, seen_at = ago(3 * HOUR),
			finish = { line = "report" },
			closed = { at = ago(3 * HOUR + 12 * MIN), by = "engine", how = "report" },
			chip = { kind = "report", label = "report" },
			word = { state = "done", at = ago(3 * HOUR + 13 * MIN), from = "th-msi", summary = "Published as sequence 8." },
			events = { ev(ago(3 * HOUR + 12 * MIN), "move", "Its thread reported done", { from = "running", to = "done", seq = 2 }) },
		},
	},
}
-- Earlier: what closed before today.
local EARLIER = {
	{ "Sidebar badge counts deferred asks", "merged", "merged 4c1a9e2", 1 },
	{ "Engine restart keeps crew threads", "report", "report", 1 },
	{ "Invite-page downloads throttled in Russia", "report", "report", 2 },
	{ "Map: machines view", "merged", "merged a7709bd", 2 },
	{ "Old relay cleanup", "dropped", "", 3 },
}
for i, row in EARLIER do
	local dropped = row[2] == "dropped"
	local at = ago(row[4] * DAY + i * 20 * MIN)
	table.insert(seed, {
		item = { id = `it-old-{i}`, orchestrator_id = if i % 2 == 0 then "orc-vpn" else "orc-fixes", title = row[1], state = if dropped then "dropped" else "done", priority = "normal", threads = {}, order = 20 + i, created = at - HOUR, updated = at, done = at, links = {} },
		card = {
			place = if dropped then "dropped" else "done", status = if dropped then "dropped" else "done", since = at, status_since = at, seq = 1, cause = if dropped then "dropped" else "reported", by = if dropped then "you" else "engine", unseen = false,
			finish = { line = if row[2] == "merged" then "merged" else "report" },
			closed = { at = at, by = if dropped then "you" else "engine", how = row[2] },
			chip = if row[3] ~= "" then { kind = row[2], label = row[3] } else nil,
			events = {},
		},
	})
end

local item_rows: { any } = {}
local card_rows: { any } = {}
for _, s in seed do
	table.insert(item_rows, s.item)
	local c = s.card
	c.id = s.item.id
	c.old_state = s.item.state
	c.held = c.held == true
	c.unseen = c.unseen == true
	c.asks = c.asks or {}
	c.guests = {}
	c.updated = c.since
	table.insert(card_rows, c)
end

local ask_rows: { any } = {
	{
		id = "ak-push", from_chat = "th-merge", orchestrator_id = "orc-fixes", item_id = "it-merge", kind = "approval", origin = "tool",
		question = "Unlock 1Password and allow the push to your zgx fork?",
		options = { { label = "Unlocked + push the fork", images = {}, scores = {} }, { label = "Unlocked, don't push yet", images = {}, scores = {} }, { label = "Wait", images = {}, scores = {} } },
		urgent = false, state = "open", created = ago(6 * MIN), quiet = false, images = {}, links = {},
	},
	{
		id = "ak-failed", from_chat = "orc-vpn", orchestrator_id = "orc-vpn", item_id = "it-admin", kind = "decision", origin = "board_check",
		question = "\"Admin app redesign\" stopped on an error. What now?",
		options = { { label = "Retry", images = {}, scores = {} }, { label = "Drop it", images = {}, scores = {} } },
		urgent = false, state = "open", created = ago(14 * MIN), quiet = false, images = {}, links = {},
	},
	{
		id = "ak-stall", from_chat = "orc-vpn", orchestrator_id = "orc-vpn", item_id = "it-relay", kind = "decision", origin = "stall",
		question = "Nothing is moving on \"RU-entry relay: resist cascade detection\". What now?",
		options = { { label = "Continue", images = {}, scores = {} }, { label = "Close it", images = {}, scores = {} }, { label = "Drop it", images = {}, scores = {} } },
		urgent = false, state = "open", created = ago(48 * MIN), quiet = false, images = {}, links = {},
	},
	{
		id = "ak-prs", from_chat = "orc-fixes", orchestrator_id = "orc-fixes", item_id = "it-prs", kind = "decision", origin = "review",
		question = "12 pull requests are waiting for your review.",
		options = { { label = "Looks good", images = {}, scores = {} } },
		urgent = false, state = "open", created = ago(5 * HOUR), deferred_until = T + 17 * HOUR + 37 * MIN, quiet = false, images = {}, links = {},
	},
}

local items = ui.state("fx_items", item_rows)
local cards = ui.state("fx_cards", card_rows)
local asks = ui.state("fx_asks", ask_rows)
local said = ui.state("calls", "")
local function note(what: string)
	said:set(if said:peek() == "" then what else `{said:peek()},{what}`)
end
-- What the engine says to every action while set (a test of refusals): the
-- action is noted, the rows are left, and its promise is rejected with it.
local refuse = ui.state("refuse", "")
local function refused(): any
	local why = refuse:peek()
	if why == "" then
		return nil
	end
	return {
		after = function(self: any, _done: any, failed: ((any) -> ())?): any
			if failed then
				failed({ message = why })
			end
			return self
		end,
	}
end
local filter = ui.state("filter", nil :: string?)
local compact = ui.state("compact", false)
local only_list = ui.state("only_list", false)
local view = ui.state("board_view", "board")
local tab = ui.state("board_tab", "all")
local selected = ui.state("board_selected", nil :: string?)
-- (The list by itself: the card its host shows in the list's place.)
local list_open = ui.state("list_open", nil :: string?)
local drawers = ui.state("board_drawers", {} :: { [string]: boolean })

-- A card's rows changed as the engine's next frame would change them.
local function patch(list: any, id: string, fields: { [string]: any })
	local rows = table.clone(list:peek())
	for i, row in rows do
		if row.id == id then
			local copy = table.clone(row)
			for k, v in fields do
				copy[k] = v
			end
			rows[i] = copy
		end
	end
	list:set(rows)
end
local function move(id: string, place: string, status: string, reason: string, extra: { [string]: any }?)
	local now = T
	local card: any = nil
	for _, c in cards:peek() do
		if c.id == id then
			card = c
		end
	end
	if not card then
		return
	end
	local events = table.clone(card.events or {})
	table.insert(events, ev(now, "move", reason, { from = card.place, to = place, seq = (card.seq or 0) + 1 }))
	local fields: { [string]: any } = { place = place, status = status, since = now, status_since = now, seq = (card.seq or 0) + 1, from = card.place, reason = reason, by = "you", unseen = false, events = events }
	for k, v in extra or {} do
		fields[k] = v
	end
	patch(cards, id, fields)
end

-- Every drawer open (for pictures: a state's JSON can't go in a shot's spec).
ui.command { id = "drawers_open", title = "Open every drawer", run = function()
	drawers:set({ snoozed = true, planned = true, earlier = true })
end }

-- Moves the engine makes by itself, with no action of the person's: its
-- next frame changes the rows, as these do.
ui.command { id = "engine_done", title = "The engine closes a running card", run = function()
	patch(items, "it-chat", { state = "done", done = T })
	move("it-chat", "done", "done", "Its thread reported done", {
		by = "engine", cause = "reported", unseen = true, old_state = "done",
		closed = { at = T, by = "engine", how = "report" },
	})
end }
ui.command { id = "engine_start", title = "The engine starts a planned card", run = function()
	patch(items, "it-keyed", { state = "active", threads = { "th-keyed" } })
	move("it-keyed", "running", "starting", "Thread started", { by = "engine", cause = "started", old_state = "active" })
end }

-- Deletes waiting for their Undo, as the host keeps them.
local removing: { [string]: () -> () } = {}
local function send_remove(id: string)
	note(`sent:{id}`)
	patch(items, id, { removed = true })
end
-- The window closing (the app quitting) while a delete waits: sent now.
ui.command { id = "close_window", title = "Close the window", run = function()
	for id, cancel in removing do
		cancel()
		removing[id] = nil
		send_remove(id)
	end
end }

local function board_props(): { [string]: any }
	return {
		items = items,
		cards = cards,
		asks = asks,
		orchestrators = orchestrators,
		filter = filter,
		now = function(): number
			return T
		end,
		thread_of = function(chat: string): any
			return chats[chat]
		end,
		progress_of = function(chat: string): any
			return progress[chat]
		end,
		view = view,
		tab = tab,
		selected = selected,
		drawers = drawers,
		on_open_thread = function(chat: string)
			note(`open:{chat}`)
		end,
		on_link = function(l: any)
			note(`link:{l.label}`)
		end,
		on_source = function(src: any)
			note(`source:{src.dump_id}`)
		end,
		on_answer = function(ask: any, choice: any): any
			note(`answer:{ask.id}:{choice.option or ""}:{choice.text or ""}`)
			local no = refused()
			if no then
				return no
			end
			patch(asks, ask.id, { state = "answered" })
			if ask.item_id then
				move(ask.item_id, "running", "working", "You answered", { asks = {} })
			end
			return nil
		end,
		on_snooze = function(ask_id: string, until_ms: number?): any
			note(`snooze:{ask_id}:{if until_ms then "later" else "now"}`)
			local no = refused()
			if no then
				return no
			end
			for _, a in asks:peek() do
				if a.id == ask_id and a.item_id then
					if until_ms then
						patch(asks, ask_id, { deferred_until = until_ms / 1000 })
						move(a.item_id, "snoozed", "asked", "You snoozed it", { ["until"] = until_ms / 1000 })
					else
						patch(asks, ask_id, { deferred_until = false })
						move(a.item_id, "needs_you", "asked", "Back from snooze")
					end
				end
			end
			return nil
		end,
		on_close = function(id: string, dropped: boolean): any
			note(`close:{id}:{dropped}`)
			local no = refused()
			if no then
				return no
			end
			patch(items, id, { state = if dropped then "dropped" else "done", done = T })
			move(id, if dropped then "dropped" else "done", if dropped then "dropped" else "done", if dropped then "You dropped it" else "You marked it done", {
				old_state = if dropped then "dropped" else "done",
				closed = { at = T, by = "you", how = if dropped then "dropped" else "marked" },
				asks = {},
			})
			return nil
		end,
		on_reopen = function(id: string): any
			note(`reopen:{id}`)
			local no = refused()
			if no then
				return no
			end
			patch(items, id, { state = "planned" })
			move(id, "planned", "next", "You reopened it", { old_state = "planned", closed = false })
			return nil
		end,
		on_hold = function(id: string, held: boolean): any
			note(`hold:{id}:{held}`)
			local no = refused()
			if no then
				return no
			end
			patch(items, id, { state = if held then "inbox" else "planned" })
			move(id, "planned", if held then "held" else "next", if held then "You kept it for later" else "You let it go", { old_state = if held then "inbox" else "planned", held = held })
			return nil
		end,
		on_pause = function(id: string): any
			note(`pause:{id}`)
			local no = refused()
			if no then
				return no
			end
			patch(items, id, { state = "blocked", paused_at = T })
			move(id, "planned", "paused", "You paused it", { old_state = "blocked" })
			return nil
		end,
		on_resume = function(id: string): any
			note(`resume:{id}`)
			local no = refused()
			if no then
				return no
			end
			patch(items, id, { state = "active", paused_at = false })
			move(id, "running", "working", "You resumed it", { old_state = "active" })
			return nil
		end,
		on_start = function(id: string): any
			note(`start:{id}`)
			local no = refused()
			if no then
				return no
			end
			patch(items, id, { state = "active", threads = { `th-{id}` } })
			move(id, "running", "queued", "You started it", { old_state = "active" })
			return nil
		end,
		-- (As the host does: the delete sent once its wait is over, unless
		-- undone; `sent:<id>` noted then. `close_window` sends what waits.)
		on_remove = function(id: string, wait_ms: number)
			note(`remove:{id}`)
			if refuse:peek() ~= "" then
				error(refuse:peek(), 0)
			end
			removing[id] = ui.after(wait_ms, function()
				removing[id] = nil
				send_remove(id)
			end)
		end,
		on_undo_remove = function(id: string)
			note(`undo:{id}`)
			local cancel = removing[id]
			removing[id] = nil
			if cancel then
				cancel()
			end
		end,
		on_seen = function(id: string): any
			note(`seen:{id}`)
			local no = refused()
			if no then
				return no
			end
			patch(cards, id, { unseen = false, seen_at = T })
			return nil
		end,
		on_new_task = function(text: string, orchestrator_id: string?): any
			note(`new:{orchestrator_id or ""}:{text}`)
			local no = refused()
			if no then
				return no
			end
			return nil
		end,
	}
end

-- (The toasts over the board, as a window has them.)
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
				-- (As an orchestrator's side panel shows it: 360 wide, a card
				-- opened noted for the host.)
				props.tab = nil
				props.on_open_card = function(id: string)
					note(`card:{id}`)
					list_open:set(id)
				end
				local detail = table.clone(props)
				detail.card = list_open
				detail.back = true
				detail.flat = true
				detail.on_open_card = nil
				detail.on_back = function()
					list_open:set(nil)
				end
				-- (A panel's look: its own surface, the list or a card in it.)
				return ui.column {
					id = "fixture-list",
					w = 360,
					p = "$space.2",
					rounded = "$radius.xl",
					border = 1,
					border_color = "$border.subtle",
					ui.show(function(): boolean
						return list_open:get() == nil
					end, List.List(props :: any), Detail.Card(detail :: any)),
				}
			end
			props.show_filter = true
			return OrchBoard(props :: any)
		end),
	},
}, ds.toast.Host {} }
