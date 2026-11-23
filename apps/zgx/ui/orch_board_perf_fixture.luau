-- The board (ds/orch_board) on 200 cards over every place, rows as the
-- engine gives them (ui/orch_board_places_fixture.luau's, made many), for
-- what one card's change costs (tests/orch_board_perf.test.luau). What one
-- command changes is pushed as the bridge pushes it: the list again, the
-- rows that didn't change the same tables.
--
--   perf_status        a Running card's status changes (working <-> waiting),
--                      in its column
--   perf_status_fresh  the same, every row handed back as a new table (the
--                      whole list written again)
--   perf_move          a Running card moves to Done, and back the next time
--   perf_same          nothing changed: the same rows in a new list;
--                      perf_same_fresh, each row a new table
--   perf_tick          the clock goes on a minute
--   perf_read          the run counts (ds/orch_perf_trace) into the state
--                      `perf_counts`; perf_reset sets them to nothing
--
-- States a test sets: `board_view` ("board" | "list"), `board_drawers`.

ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()
local OrchBoard = require("@ds/orch_board")
local trace = require("@ds/orch_perf_trace")

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

local ORCS = {
	{ id = "orc-fixes", kind = "standard", name = "Zeron fixes", hue = 176, archived = false },
	{ id = "orc-vpn", kind = "standard", name = "Exnomic VPN", hue = 18, archived = false },
	{ id = "orc-ci", kind = "standard", name = "CI speed", hue = 96, archived = false },
	{ id = "orc-ui", kind = "standard", name = "Control room UI", hue = 320, archived = false },
}
local orchestrators = ui.state("fx_orchestrators", ORCS :: { any })

local WORDS = { "Exporter image", "WireGuard keys", "Peer sync", "Admin app login", "Moscow relay", "DE node health", "Backup nightly", "Route table", "DNS split", "Metrics push", "Cert rotate", "Mesh join", "Firewall rules", "Client config", "UI polish", "Throughput test", "Chat list rows", "Telemetry Android", "Release notes", "Windows MSI" }
local SUMMARIES = {
	"The fix is committed. Two test suites are running.",
	"Probe traffic now looks like plain HTTPS. Two of five carriers still reset the stream.",
	"Measuring on 700 chats: one status change went from 1,680 recomputes to 374.",
	"Server side is done. Wiring the Android client to it.",
	"Merged into zeron/orchestrators and live in the dev app.",
	"$7,840 in 14 days; 23% is orchestration overhead.",
}
-- How many of each place, in order (200 in all).
local SPREAD: { { any } } = { { "needs_you", 28 }, { "snoozed", 8 }, { "running", 64 }, { "planned", 40 }, { "done", 30 }, { "earlier", 30 } }
local RUNNING = { "working", "waiting", "checking", "working", "limited", "working" }
local PLANNED = { "next", "after", "paused", "held", "next" }
local NEEDS = { "asked", "failed", "stalled", "review", "asked" }

local function ev(at: number, kind: string, text: string, extra: { [string]: any }?): any
	local e: { [string]: any } = { at = at, kind = kind, text = text }
	for k, v in extra or {} do
		e[k] = v
	end
	return e
end

local chats: { [string]: any } = {}
local progress: { [string]: any } = {}
local item_rows: { any } = {}
local card_rows: { any } = {}
local ask_rows: { any } = {}
local n = 0
for _, spread in SPREAD do
	local place, count = spread[1] :: string, spread[2] :: number
	for i = 1, count do
		n += 1
		local id = `it-{n}`
		local chat = `th-{n}`
		local o = ORCS[(n - 1) % #ORCS + 1]
		local title = WORDS[(n - 1) % #WORDS + 1] .. ` {n}`
		local since = ago(i * 7 * MIN + n)
		local item: { [string]: any } = {
			id = id, orchestrator_id = o.id, title = title, priority = if n % 9 == 0 then "high" else "normal",
			threads = { chat }, order = n, created = since - HOUR, updated = since, links = {},
			detail = "Give every row a stable key so a push rebuilds only the rows it changed.",
		}
		local card: { [string]: any } = {
			id = id, place = place, since = since, status_since = since, seq = 2, from = "running", reason = "Thread started", cause = "started", by = "agent",
			finish = { line = if n % 3 == 0 then "merged" else "pr", into = "zeron/orchestrators" },
			word = { state = "progress", at = since, from = chat, summary = SUMMARIES[(n - 1) % #SUMMARIES + 1] },
			events = {
				ev(since - HOUR, "move", "Thread started", { from = "planned", to = "running", seq = 1 }),
				ev(since - 20 * MIN, "report", "Reported: the fix is committed"),
				ev(since, "move", "Moved", { from = "running", to = place, seq = 2 }),
			},
			held = false, unseen = false, asks = {}, guests = {}, updated = since,
		}
		chats[chat] = { id = chat, title = title, status = "working" }
		if place == "needs_you" or place == "snoozed" then
			item.state = "blocked"
			card.status = if place == "snoozed" then "review" else NEEDS[(i - 1) % #NEEDS + 1]
			local ask: { [string]: any } = {
				id = `ak-{n}`, from_chat = chat, orchestrator_id = o.id, item_id = id, kind = "decision", origin = "tool",
				question = `{title}: unlock and approve the push to the fork?`,
				options = { { label = "Approve", images = {}, scores = {} }, { label = "Wait", images = {}, scores = {} } },
				urgent = i % 7 == 0, state = "open", created = since, quiet = false, images = {}, links = {},
			}
			if place == "snoozed" then
				card["until"] = T + (i + 1) * HOUR
				ask.deferred_until = T + (i + 1) * HOUR
			end
			table.insert(ask_rows, ask)
			card.asks = { ask.id }
		elseif place == "running" then
			item.state = "active"
			card.status = RUNNING[(i - 1) % #RUNNING + 1]
			if card.status == "waiting" then
				card.note = "tests"
			elseif card.status == "limited" then
				card["until"] = T + 100 * MIN
			end
			if i % 4 == 0 then
				card.chip = { kind = "pr", label = `PR #{1500 + n}`, target = `https://github.com/zeronsh/zeron/pull/{1500 + n}` }
			end
			if i % 2 == 0 then
				progress[chat] = { state = "working", note = `Step {i} of the plan: wiring the client.` }
			end
		elseif place == "planned" then
			card.status = PLANNED[(i - 1) % #PLANNED + 1]
			item.state = if card.status == "held" then "inbox" elseif card.status == "paused" then "blocked" else "planned"
			if card.status == "paused" then
				item.paused_at = since
			elseif card.status == "after" then
				card.note = "Release 7bb4a19a3"
			end
			card.held = card.status == "held"
			item.threads = {}
		else
			-- Done today, or earlier (some dropped).
			local closed = if place == "done" then ago(i * 9 * MIN) else ago((i % 5 + 1) * DAY + i * 20 * MIN)
			local dropped = place == "earlier" and i % 6 == 0
			item.state = if dropped then "dropped" else "done"
			item.done = closed
			item.updated = closed
			card.place = if dropped then "dropped" else "done"
			card.status = if dropped then "dropped" else "done"
			card.since, card.status_since = closed, closed
			card.closed = { at = closed, by = "engine", how = if n % 3 == 0 then "merged" else "report" }
			card.chip = if n % 3 == 0 then { kind = "merged", label = "merged e99f817" } else { kind = "report", label = "report" }
			card.unseen = place == "done" and i % 4 == 0
		end
		card.old_state = item.state
		table.insert(item_rows, item)
		table.insert(card_rows, card)
	end
end

local items = ui.state("fx_items", item_rows)
local cards = ui.state("fx_cards", card_rows)
local asks = ui.state("fx_asks", ask_rows)
local now = ui.state("fx_now", T)
local view = ui.state("board_view", "board")
local tab = ui.state("board_tab", "all")
local selected = ui.state("board_selected", nil :: string?)
local drawers = ui.state("board_drawers", {} :: { [string]: boolean })

-- A row of a list changed, the rest kept (or every row a new table).
local function patch(list: any, id: string, fields: { [string]: any }, fresh: boolean?)
	local was: { any } = list:peek()
	local rows = table.clone(was)
	for i, row in rows do
		if row.id == id then
			local copy = table.clone(row)
			for k, v in fields do
				copy[k] = v
			end
			rows[i] = copy
		elseif fresh then
			rows[i] = table.clone(row)
		end
	end
	list:set(rows)
end
local function row_of(list: any, id: string): any
	for _, row in list:peek() do
		if row.id == id then
			return row
		end
	end
	return nil
end

-- The card that changes: the latest of Running (at its top, shown).
local TARGET = "it-37"
local flips = 0
local function status_push(fresh: boolean)
	flips += 1
	local c = row_of(cards, TARGET)
	local waiting = c.status ~= "waiting"
	patch(cards, TARGET, { status = if waiting then "waiting" else "working", note = if waiting then "tests" else "", status_since = T - flips, updated = T + flips }, fresh)
end
ui.command { id = "perf_status", title = "Perf: one status change", run = function()
	status_push(false)
end }
ui.command { id = "perf_status_fresh", title = "Perf: one status change, every row new", run = function()
	status_push(true)
end }
ui.command { id = "perf_move", title = "Perf: a card moves between columns", run = function()
	flips += 1
	local c = row_of(cards, TARGET)
	ui.batch(function()
		if c.place == "running" then
			patch(items, TARGET, { state = "done", done = T - 1, updated = T + flips })
			patch(cards, TARGET, { place = "done", status = "done", old_state = "done", from = "running", seq = c.seq + 1, closed = { at = T - 1, by = "engine", how = "report" }, since = T - 1, status_since = T - 1, updated = T + flips })
		else
			patch(items, TARGET, { state = "active", done = false, updated = T + flips })
			patch(cards, TARGET, { place = "running", status = "working", old_state = "active", from = "done", seq = c.seq + 1, closed = false, since = T - 1, status_since = T - 1, updated = T + flips })
		end
	end)
end }
-- Nothing changed: the same rows again, in a new list (or each a new table).
ui.command { id = "perf_same", title = "Perf: the same rows again", run = function()
	local was: { any } = cards:peek()
	cards:set(table.clone(was))
end }
ui.command { id = "perf_same_fresh", title = "Perf: the same rows again, each a new table", run = function()
	local out: { any } = {}
	for _, row in cards:peek() do
		table.insert(out, table.clone(row))
	end
	cards:set(out)
end }
ui.command { id = "perf_tick", title = "Perf: a minute goes by", run = function()
	now:set(now:peek() + 60)
end }

local counts = ui.state("perf_counts", {} :: any)
ui.command { id = "perf_read", title = "Perf: read the run counts", run = function()
	counts:set(table.clone(trace.counts))
end }
ui.command { id = "perf_reset", title = "Perf: reset the run counts", run = function()
	trace.reset()
end }

return ui.div { w_full = true, h_full = true, ds.toast.Host {}, ui.scroll {
	id = "fixture-root",
	w_full = true,
	h_full = true,
	bg = "$background",
	ui.column {
		w_full = true,
		p = "$space.4",
		OrchBoard {
			items = items,
			cards = cards,
			asks = asks,
			orchestrators = orchestrators,
			show_filter = true,
			now = function(): number
				return now:get()
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
			on_answer = function(): any
				return nil
			end,
			on_snooze = function(): any
				return nil
			end,
			on_close = function(): any
				return nil
			end,
			on_pause = function(): any
				return nil
			end,
			on_resume = function(): any
				return nil
			end,
		} :: any,
	},
} }
