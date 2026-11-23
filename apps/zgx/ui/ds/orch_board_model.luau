--!strict
-- Where every card stands, for everything that shows it: the board's
-- columns and drawers, the list view's sections, and Needs you (its
-- widget, its float, the sidebar's badge: ds/orch_data's `needs`). The
-- engine works each card's place out from facts and keeps it in the card's
-- row (`orchestration.cards`, by the item's id); nothing here decides one.
-- A card whose engine predates places (no row, or a row an older engine
-- has since moved the item past) shows by its old column, mapped as the
-- engine maps it (crates/proto `board::place_of_old`).
--
--   local model = require("@ds/orch_board_model")
--   model.board(now, { filter = id?, at = seconds? })   -> Board (memoized
--                              on the rows it reads and the minute; a card
--                              that says the same keeps its Entry table)
--   model.entry(now, id)       -> Entry?  one card, whoever's it is
--   model.place(item, card?, asked) -> place, status
--   model.PLACES               the columns, in order
--   model.DRAWER[place]        the drawer at a column's foot
--
-- A Board is its places' cards, each an Entry, in the order they show:
--   needs_you   urgent first, then the one that came last
--   snoozed     the one that comes back first
--   running     the one that came last
--   planned     what starts next first (its priority, then its order), then
--               what waits on another card, what's paused, what's held
--   done        closed today, the last first
--   earlier     closed before today (dropped ones among them), the last first

local M = {}

export type Entry = {
	id: string,
	item: any,
	-- Its row, when its engine works places out and nothing older moved it
	-- since.
	card: any?,
	place: string,
	status: string,
	-- Its open asks, the one to answer first first (put-off ones last).
	asks: { any },
	-- The one to answer now (not put off), if any.
	ask: any?,
	urgent: boolean,
	-- When it came to this place; when its status last changed (its timer).
	since: number,
	status_since: number,
	-- When it closed (Done, Earlier).
	closed_at: number?,
	-- Until when it's snoozed, or its status ends by itself.
	until_at: number?,
}

export type Board = {
	needs_you: { Entry },
	snoozed: { Entry },
	running: { Entry },
	planned: { Entry },
	done: { Entry },
	earlier: { Entry },
	by_id: { [string]: Entry },
	-- The tabs' figures: what's on the board today.
	counts: { all: number, needs_you: number, running: number, done: number },
}

-- The columns, and the drawer at each one's foot.
M.PLACES = { "needs_you", "running", "done" }
M.DRAWER = { needs_you = "snoozed", running = "planned", done = "earlier" } :: { [string]: string }
-- Each drawer's column.
M.COLUMN = { snoozed = "needs_you", planned = "running", earlier = "done" } :: { [string]: string }

local PRIORITY = { urgent = 0, high = 1, normal = 2, low = 3 }
-- Planned's groups, in order.
local PLANNED = { queued = 0, next = 0, after = 1, paused = 2, held = 3 }

-- Until when an ask is put off (seconds), if it is.
local function deferred_until(ask: any): number?
	local u = ask and ask.deferred_until
	if type(u) == "number" then
		return if u > 1e11 then u / 1000 else u
	end
	return nil
end

local function deferred(ask: any, at: number): boolean
	local u = deferred_until(ask)
	return u ~= nil and u > at
end
M.deferred_until = deferred_until

-- Local midnight of `t`.
local function day_start(t: number): number
	local d = os.date("*t", t) :: any
	return t - (d.hour * 3600 + d.min * 60 + d.sec)
end
M.day_start = day_start

-- Where a card with no row shows: its old column, mapped. `asked`: it has
-- an open ask.
local function place_of_old(item: any, asked: boolean): (string, string)
	local s = item.state
	if s == "inbox" then
		return "planned", "held"
	elseif s == "planned" then
		return "planned", if (item.waiting or "") ~= "" then "queued" else "next"
	elseif s == "active" then
		return "running", "working"
	elseif s == "blocked" then
		if item.paused_at ~= nil then
			return "planned", "paused"
		end
		return "needs_you", "asked"
	elseif s == "review" then
		if asked then
			return "needs_you", "review"
		end
		return "done", "done"
	elseif s == "done" then
		return "done", "done"
	elseif s == "dropped" then
		return "dropped", "dropped"
	end
	return "planned", "next"
end

-- Whether `card` is `item`'s row still: an engine that works places out
-- writes the item's old column with every move, so a row whose old column
-- isn't the item's was overtaken by an older writer.
local function current(item: any, card: any?): boolean
	return card ~= nil and (card.old_state == nil or card.old_state == item.state)
end

-- Where a card shows, and how it stands there.
function M.place(item: any, card: any?, asked: boolean): (string, string)
	if current(item, card) then
		return (card :: any).place, (card :: any).status
	end
	return place_of_old(item, asked)
end

-- A function of one table, remembered for the last one given (by
-- identity: the bridge keeps a list that didn't change the same table).
local function memo1<T>(f: (any) -> T): (any) -> T
	local last_key: any = nil
	local last: T? = nil
	return function(key: any): T
		if last ~= nil and key == last_key then
			return last :: T
		end
		last_key = key
		local made = f(key)
		last = made
		return made
	end
end

-- The cards' rows by their item's id.
M.cards_by_id = memo1(function(cards: any): { [string]: any }
	local out: { [string]: any } = {}
	for _, c in cards or {} do
		out[c.id] = c
	end
	return out
end)

-- The open asks: by id, and those that name no card by the chat that asked.
local asks_index = memo1(function(asks: any): { by_id: { [string]: any }, by_item: { [string]: { any } }, by_chat: { [string]: { any } } }
	local by_id: { [string]: any } = {}
	local by_item: { [string]: { any } } = {}
	local by_chat: { [string]: { any } } = {}
	for _, a in asks or {} do
		if a.state == "open" then
			by_id[a.id] = a
			if a.item_id then
				by_item[a.item_id] = by_item[a.item_id] or {}
				table.insert(by_item[a.item_id], a)
			elseif a.from_chat then
				by_chat[a.from_chat] = by_chat[a.from_chat] or {}
				table.insert(by_chat[a.from_chat], a)
			end
		end
	end
	return { by_id = by_id, by_item = by_item, by_chat = by_chat }
end)
M.asks_index = asks_index

-- A card's open asks: its row's (the engine's order), else those that name
-- it or that one of its threads raised on no card.
local function asks_of(item: any, card: any?, index: any, at: number): { any }
	local out: { any } = {}
	if card and card.asks then
		for _, id in card.asks do
			local a = index.by_id[id]
			if a then
				table.insert(out, a)
			end
		end
		return out
	end
	for _, a in index.by_item[item.id] or {} do
		table.insert(out, a)
	end
	for _, chat in item.threads or {} do
		for _, a in index.by_chat[chat] or {} do
			if not a.quiet then
				table.insert(out, a)
			end
		end
	end
	-- (Put-off ones last, then the oldest first.)
	table.sort(out, function(a: any, b: any): boolean
		local da, db = deferred(a, at), deferred(b, at)
		if da ~= db then
			return db
		end
		if (a.created or 0) ~= (b.created or 0) then
			return (a.created or 0) < (b.created or 0)
		end
		return a.id < b.id
	end)
	return out
end

-- Whether two rows say the same (their fields, and the tables in them a few
-- levels down): a push handing an unchanged row back as a new table (the
-- whole list written again) changes no card.
local function same(a: any, b: any, depth: number): boolean
	if rawequal(a, b) then
		return true
	end
	if type(a) ~= "table" or type(b) ~= "table" or depth <= 0 then
		return false
	end
	for k, v in a do
		if not same(v, b[k], depth - 1) then
			return false
		end
	end
	for k in b do
		if a[k] == nil then
			return false
		end
	end
	return true
end
M.same = same

-- Whether a card's new Entry says what its last one did: then the last one
-- is kept, the same table, and nothing that reads the card wakes.
local function same_entry(a: Entry, b: Entry): boolean
	if
		a.place ~= b.place
		or a.status ~= b.status
		or a.urgent ~= b.urgent
		or a.since ~= b.since
		or a.status_since ~= b.status_since
		or a.closed_at ~= b.closed_at
		or a.until_at ~= b.until_at
		or #a.asks ~= #b.asks
	then
		return false
	end
	for i, ask in a.asks do
		if not same(ask, b.asks[i], 4) then
			return false
		end
	end
	return same(a.ask, b.ask, 4) and same(a.item, b.item, 4) and same(a.card, b.card, 5)
end

local function entry_of(item: any, card_row: any?, index: any, at: number): Entry
	local card = if current(item, card_row) then card_row else nil
	local asks = asks_of(item, card, index, at)
	local ask: any? = nil
	local urgent = false
	for _, a in asks do
		if not deferred(a, at) then
			ask = ask or a
			urgent = urgent or a.urgent == true or a.escalated ~= nil
		end
	end
	local place, status = M.place(item, card, ask ~= nil)
	local closed_at: number? = nil
	if place == "done" or place == "dropped" then
		closed_at = (card and card.closed and card.closed.at) or item.done or item.updated or 0
	end
	-- (`until` is the row's key, and Luau's word.)
	local until_at: number? = card and (card :: any)["until"]
	if place == "snoozed" and not until_at then
		for _, a in asks do
			local u = deferred_until(a)
			if u and (not until_at or u < until_at) then
				until_at = u
			end
		end
	end
	local since = (card and card.since) or closed_at or item.updated or item.created or 0
	return {
		id = item.id,
		item = item,
		card = card,
		place = place,
		status = status,
		asks = asks,
		ask = ask,
		urgent = urgent,
		since = since,
		status_since = (card and card.status_since) or since,
		closed_at = closed_at,
		until_at = until_at,
	}
end

local function latest_first(a: Entry, b: Entry): boolean
	if a.since ~= b.since then
		return a.since > b.since
	end
	return a.id < b.id
end

local function build(now: any, filter: string?, at: number, prev: Board?): Board
	local was = prev and prev.by_id
	local cards = M.cards_by_id(now and now.cards)
	local index = asks_index(now and now.asks)
	local today = day_start(at)
	local b: Board = {
		needs_you = {},
		snoozed = {},
		running = {},
		planned = {},
		done = {},
		earlier = {},
		by_id = {},
		counts = { all = 0, needs_you = 0, running = 0, done = 0 },
	}
	for _, item in (now and now.items) or {} do
		if not item.removed and (filter == nil or item.orchestrator_id == filter) then
			local e = entry_of(item, cards[item.id], index, at)
			local last = was and was[e.id]
			if last and same_entry(e, last) then
				e = last
			end
			b.by_id[e.id] = e
			if e.place == "needs_you" then
				table.insert(b.needs_you, e)
			elseif e.place == "snoozed" then
				table.insert(b.snoozed, e)
			elseif e.place == "running" then
				table.insert(b.running, e)
			elseif e.place == "done" and (e.closed_at or 0) >= today then
				table.insert(b.done, e)
			elseif e.place == "done" or e.place == "dropped" then
				table.insert(b.earlier, e)
			else
				table.insert(b.planned, e)
			end
		end
	end
	table.sort(b.needs_you, function(x: Entry, y: Entry): boolean
		if x.urgent ~= y.urgent then
			return x.urgent
		end
		return latest_first(x, y)
	end)
	table.sort(b.snoozed, function(x: Entry, y: Entry): boolean
		local ux, uy = x.until_at or math.huge, y.until_at or math.huge
		if ux ~= uy then
			return ux < uy
		end
		return x.id < y.id
	end)
	table.sort(b.running, latest_first)
	table.sort(b.planned, function(x: Entry, y: Entry): boolean
		local gx, gy = PLANNED[x.status] or 0, PLANNED[y.status] or 0
		if gx ~= gy then
			return gx < gy
		end
		local px, py = PRIORITY[x.item.priority] or 2, PRIORITY[y.item.priority] or 2
		if px ~= py then
			return px < py
		end
		local ox, oy = x.item.order or 0, y.item.order or 0
		if ox ~= oy then
			return ox < oy
		end
		return x.id < y.id
	end)
	local function closed_last(x: Entry, y: Entry): boolean
		local cx, cy = x.closed_at or 0, y.closed_at or 0
		if cx ~= cy then
			return cx > cy
		end
		return x.id < y.id
	end
	table.sort(b.done, closed_last)
	table.sort(b.earlier, closed_last)
	b.counts.needs_you = #b.needs_you
	b.counts.running = #b.running
	b.counts.done = #b.done
	b.counts.all = #b.needs_you + #b.running + #b.done
	return b
end

-- The board of `now` (the bridge's `orchestration`): every card's, or one
-- orchestrator's (`filter`). Remembered while the rows it reads, the
-- filter and the day stay the same (a few boards: the Control room's, a
-- page's, Needs you's).
local kept: { { items: any, cards: any, asks: any, filter: string?, day: number, minute: number, board: Board } } = {}
local KEPT = 6

function M.board(now: any, opts: { filter: string?, at: number? }?): Board
	local at = (opts and opts.at) or os.time()
	local filter = opts and opts.filter
	local day = day_start(at)
	-- (A put-off ask comes back with time alone: asked again each minute.)
	local minute = at // 60
	local items, cards, asks = now and now.items, now and now.cards, now and now.asks
	for _, k in kept do
		if k.items == items and k.cards == cards and k.asks == asks and k.filter == filter and k.day == day and k.minute == minute then
			return k.board
		end
	end
	-- (The last board of this filter: its cards that say the same are kept.)
	local prev: Board? = nil
	for _, k in kept do
		if k.filter == filter then
			prev = k.board
			break
		end
	end
	local board = build(now, filter, at, prev)
	table.insert(kept, 1, { items = items, cards = cards, asks = asks, filter = filter, day = day, minute = minute, board = board })
	if #kept > KEPT then
		table.remove(kept)
	end
	return board
end

-- One card's entry, whoever's it is.
function M.entry(now: any, id: string?, at: number?): Entry?
	if not id then
		return nil
	end
	return M.board(now, { at = at }).by_id[id]
end

-- A place's cards of a board (a column's or a drawer's).
function M.of(board: Board, place: string): { Entry }
	return ((board :: any)[place] :: { Entry }?) or {}
end

-- The card to start next, of Planned's.
function M.next_up(board: Board): Entry?
	local first = board.planned[1]
	if first and (first.status == "next" or first.status == "queued") then
		return first
	end
	return nil
end

return M
