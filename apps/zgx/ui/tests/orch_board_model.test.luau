-- Where every card stands (ds/orch_board_model), through its public
-- functions alone: a card's place from its row, or from its old column
-- when it has no row or an older engine has moved the item past its row;
-- the board's grouping (today's Done apart from Earlier, a filter, the
-- tabs' counts); and the order within each place.

local model = require("@ds/orch_board_model")

-- Today at 8 PM here: what closed this afternoon is today's.
local base = os.time()
local here = os.date("*t", base) :: any
local MIDNIGHT = base - (here.hour * 3600 + here.min * 60 + here.sec)
local T = MIDNIGHT + 20 * 3600
local MIN, HOUR, DAY = 60, 3600, 86400

local function item(id: string, state: string, extra: { [string]: any }?): any
	local it: { [string]: any } = { id = id, orchestrator_id = "orc-a", title = id, state = state, priority = "normal", order = 1, threads = {}, created = T - DAY, updated = T - HOUR }
	for k, v in extra or {} do
		it[k] = v
	end
	return it
end

-- A card's row as an engine that works places out writes it.
local function row(it: any, place: string, status: string, extra: { [string]: any }?): any
	local c: { [string]: any } = { id = it.id, place = place, status = status, old_state = it.state, since = T - HOUR, status_since = T - HOUR, asks = {} }
	for k, v in extra or {} do
		c[k] = v
	end
	return c
end

local function ids(list: { any }): string
	local out: { string } = {}
	for _, e in list do
		table.insert(out, e.id)
	end
	return table.concat(out, ",")
end

local function board(items: { any }, cards: { any }?, asks: { any }?, filter: string?): any
	return model.board({ items = items, cards = cards or {}, asks = asks or {} }, { at = T, filter = filter })
end

test("board model: a card with a row stands where its row says", function()
	local it = item("it-a", "active")
	local place, status = model.place(it, row(it, "needs_you", "stalled"), false)
	expect(place).to_be("needs_you")
	expect(status).to_be("stalled")
	-- (A row that names no old column is the item's still.)
	local bare = row(it, "running", "checking")
	bare.old_state = nil
	place, status = model.place(it, bare, false)
	expect(place).to_be("running")
	expect(status).to_be("checking")
end)

test("board model: a card with no row stands by its old column", function()
	local cases = {
		{ item("i1", "inbox"), false, "planned", "held" },
		{ item("i2", "planned"), false, "planned", "next" },
		{ item("i3", "planned", { waiting = "Ops's crew is full" }), false, "planned", "queued" },
		{ item("i4", "active"), false, "running", "working" },
		{ item("i5", "blocked"), false, "needs_you", "asked" },
		{ item("i6", "blocked", { paused_at = T - HOUR }), false, "planned", "paused" },
		{ item("i7", "review"), true, "needs_you", "review" },
		{ item("i8", "review"), false, "done", "done" },
		{ item("i9", "done"), false, "done", "done" },
		{ item("i10", "dropped"), false, "dropped", "dropped" },
		{ item("i11", "something new"), false, "planned", "next" },
	}
	for _, c in cases do
		local place, status = model.place(c[1], nil, c[2])
		expect(`{c[1].state}: {place} {status}`).to_be(`{c[1].state}: {c[3]} {c[4]}`)
	end
end)

test("board model: a row an older engine has overtaken gives way to the old column", function()
	-- (Its row says Running; the item has since been closed by a writer
	-- that knows no places: its old column says Done.)
	local it = item("it-a", "done", { done = T - 10 * MIN })
	local stale = row(it, "running", "working", { old_state = "active" })
	local place, status = model.place(it, stale, false)
	expect(place).to_be("done")
	expect(status).to_be("done")
	local b = board({ it }, { stale })
	expect(ids(b.running)).to_be("")
	expect(ids(b.done)).to_be("it-a")
	-- (Its entry carries no row: nothing of the stale one is read.)
	expect(b.by_id["it-a"].card).to_be_nil()
	expect(b.by_id["it-a"].closed_at).to_be(T - 10 * MIN)
end)

test("board model: every card in its place; today's Done apart from Earlier; drawers not counted", function()
	local items = {
		item("n1", "blocked"),
		item("s1", "blocked"),
		item("r1", "active"),
		item("p1", "planned"),
		item("d1", "done", { done = MIDNIGHT }),
		item("e1", "done", { done = MIDNIGHT - 60 }),
		item("x1", "dropped", { done = T - HOUR }),
		item("gone", "active", { removed = true }),
	}
	local cards = {
		row(items[1], "needs_you", "asked"),
		row(items[2], "snoozed", "asked", { ["until"] = T + HOUR }),
	}
	local b = board(items, cards)
	expect(ids(b.needs_you)).to_be("n1")
	expect(ids(b.snoozed)).to_be("s1")
	expect(ids(b.running)).to_be("r1")
	expect(ids(b.planned)).to_be("p1")
	-- Closed at midnight: today's. A minute before: Earlier, with the dropped.
	expect(ids(b.done)).to_be("d1")
	expect(ids(b.earlier)).to_be("x1,e1")
	-- Taken off the board: nowhere.
	expect(b.by_id["gone"]).to_be_nil()
	-- The tabs' figures: the three columns alone.
	expect(b.counts.needs_you).to_be(1)
	expect(b.counts.running).to_be(1)
	expect(b.counts.done).to_be(1)
	expect(b.counts.all).to_be(3)
	-- A place by name; one card by id.
	expect(ids(model.of(b, "planned"))).to_be("p1")
	expect(#model.of(b, "nowhere")).to_be(0)
	local e = model.entry({ items = items, cards = cards, asks = {} }, "s1", T)
	expect(e and e.place).to_be("snoozed")
	expect(e and e.until_at).to_be(T + HOUR)
	expect(model.entry({ items = items }, nil, T)).to_be_nil()
end)

test("board model: a filter keeps one orchestrator's cards", function()
	local items = { item("a1", "active"), item("b1", "active", { orchestrator_id = "orc-b" }), item("b2", "done", { orchestrator_id = "orc-b", done = T - HOUR }) }
	local b = board(items, {}, {}, "orc-b")
	expect(ids(b.running)).to_be("b1")
	expect(ids(b.done)).to_be("b2")
	expect(b.counts.all).to_be(2)
	expect(board(items).counts.all).to_be(3)
end)

test("board model: the order within each place", function()
	local items = {
		-- Needs you: urgent first, then the one that came last.
		item("n-old", "blocked"),
		item("n-new", "blocked"),
		item("n-urgent", "blocked"),
		-- Snoozed: the one back first.
		item("s-late", "blocked"),
		item("s-soon", "blocked"),
		-- Running: the one that came last.
		item("r-old", "active"),
		item("r-new", "active"),
		-- Planned: next by priority then order, then waiting on another,
		-- paused, held.
		item("p-held", "inbox", { priority = "urgent" }),
		item("p-paused", "blocked", { paused_at = T - HOUR, priority = "urgent" }),
		item("p-after", "planned", { priority = "urgent" }),
		item("p-normal-2", "planned", { order = 2 }),
		item("p-normal-1", "planned", { order = 1 }),
		item("p-high", "planned", { priority = "high", order = 9 }),
		item("p-queued", "planned", { priority = "low", waiting = "a slot" }),
		-- Done and Earlier: closed last first.
		item("d-old", "done", { done = T - 3 * HOUR }),
		item("d-new", "done", { done = T - HOUR }),
		item("e-old", "done", { done = T - 3 * DAY }),
		item("e-new", "dropped", { done = T - DAY }),
	}
	local cards = {
		row(items[1], "needs_you", "asked", { since = T - 3 * HOUR }),
		row(items[2], "needs_you", "failed", { since = T - MIN }),
		row(items[3], "needs_you", "asked", { since = T - 5 * HOUR, asks = { "ak-u" } }),
		row(items[4], "snoozed", "asked", { ["until"] = T + 5 * HOUR }),
		row(items[5], "snoozed", "asked", { ["until"] = T + HOUR }),
		row(items[6], "running", "working", { since = T - 2 * HOUR }),
		row(items[7], "running", "working", { since = T - 2 * MIN }),
		row(items[10], "planned", "after"),
	}
	local asks = { { id = "ak-u", item_id = "n-urgent", state = "open", urgent = true, created = T - 5 * HOUR } }
	local b = board(items, cards, asks)
	expect(ids(b.needs_you)).to_be("n-urgent,n-new,n-old")
	expect(ids(b.snoozed)).to_be("s-soon,s-late")
	expect(ids(b.running)).to_be("r-new,r-old")
	expect(ids(b.planned)).to_be("p-high,p-normal-1,p-normal-2,p-queued,p-after,p-paused,p-held")
	expect(ids(b.done)).to_be("d-new,d-old")
	expect(ids(b.earlier)).to_be("e-new,e-old")
	-- What starts next: Planned's first, while it is next or queued.
	local first = model.next_up(b)
	expect(first and first.id).to_be("p-high")
	local only_held = board({ item("h", "inbox") })
	expect(model.next_up(only_held)).to_be_nil()
end)

test("board model: an escalated ask is urgent too; a put-off one is no ask to answer now", function()
	local items = { item("a", "blocked"), item("b", "blocked"), item("c", "blocked") }
	local cards = {
		row(items[1], "needs_you", "asked", { since = T - HOUR, asks = { "ak-a" } }),
		row(items[2], "needs_you", "asked", { since = T - 5 * HOUR, asks = { "ak-b" } }),
		row(items[3], "snoozed", "asked", { asks = { "ak-c" } }),
	}
	local asks = {
		{ id = "ak-a", item_id = "a", state = "open", created = T - HOUR },
		{ id = "ak-b", item_id = "b", state = "open", escalated = T - MIN, created = T - 5 * HOUR },
		-- (In milliseconds, as the wire may have it.)
		{ id = "ak-c", item_id = "c", state = "open", deferred_until = (T + 2 * HOUR) * 1000, created = T - DAY },
	}
	local b = board(items, cards, asks)
	expect(ids(b.needs_you)).to_be("b,a")
	expect(b.by_id["b"].urgent).to_be(true)
	expect(b.by_id["a"].urgent).to_be(false)
	expect(b.by_id["a"].ask.id).to_be("ak-a")
	-- Snoozed: its ask put off; back when the ask is (its row says no time).
	local c = b.by_id["c"]
	expect(c.ask).to_be_nil()
	expect(#c.asks).to_be(1)
	expect(c.until_at).to_be(T + 2 * HOUR)
	expect(model.deferred_until(asks[3])).to_be(T + 2 * HOUR)
end)

test("board model: a card with no row finds its asks by its id and by its threads'", function()
	local items = {
		item("named", "blocked"),
		item("threaded", "review", { threads = { "th-x" } }),
		item("quiet", "review", { threads = { "th-q" } }),
	}
	local asks = {
		{ id = "ak-2", item_id = "named", state = "open", created = T - MIN },
		{ id = "ak-1", item_id = "named", state = "open", created = T - HOUR },
		{ id = "ak-done", item_id = "named", state = "answered", created = T - DAY },
		{ id = "ak-x", from_chat = "th-x", state = "open", created = T - MIN },
		-- (A quiet one, "hasn't answered", waits on no card.)
		{ id = "ak-q", from_chat = "th-q", state = "open", quiet = true, created = T - MIN },
	}
	local b = board(items, {}, asks)
	local named = b.by_id["named"]
	-- Oldest first; an answered one isn't open.
	expect(ids(named.asks)).to_be("ak-1,ak-2")
	expect(named.ask.id).to_be("ak-1")
	-- A thread's open ask makes its Review card the person's.
	expect(b.by_id["threaded"].place).to_be("needs_you")
	expect(b.by_id["threaded"].status).to_be("review")
	expect(b.by_id["quiet"].place).to_be("done")
end)
