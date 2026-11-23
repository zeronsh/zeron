--!strict
-- What the board's parts share: the rows a host gives it, read once, and
-- what the person can do to a card. The board (ds/orch_board), its list
-- view (ds/orch_board_list) and a card's detail (ds/orch_board_detail)
-- each take the same props and make one of these from them, so a host
-- wires a board once and its list or a detail the same way.
--
--   local ctx = Ctx.make(props)     -- inside a component
--   ctx.board()                     the cards by place (ds/orch_board_model);
--                                   wakes its reader on any change
--   ctx.entry(id)                   one card, wherever it stands; wakes its
--                                   reader when that card changes alone
--   ctx.ids(place)                  a place's cards' ids, in order; wakes its
--                                   reader when who is there, or their
--                                   order, changes
--   ctx.counts()                    the tabs' figures (Board.counts)
--   ctx.at()                        Unix seconds, ticking while a clock shows
--   ctx.clock(since)                the same, for a clock counting from
--                                   `since`: woken each second while it
--                                   shows seconds, each minute after
--   ctx.act.<what>(…)               what the person does (each a host's prop)
--
-- What one push costs: the board is built again once (ds/orch_board_model
-- keeps the Entry of each card that says the same), then compared with the
-- last one, and only what changed is woken: a card's own readers, a
-- place's list where its cards came or went or moved, the tabs' counts.
-- (A "presented" board that lags the rows, for the motion of step 3,
-- belongs between the rows and `ids`: `board()` and `entry` stay the
-- rows' own.)

local model = require("@self/orch_board_model")
local control = require("@self/control")
local trace = require("@self/orch_perf_trace")

export type Entry = model.Entry
export type Board = model.Board
export type Counts = { all: number, needs_you: number, running: number, done: number }

-- What a host gives: its rows, and the person's actions as it sends them.
export type Props = {
	id: string?,
	-- The bridge's rows (`orchestration.items`, `.cards`, `.asks`,
	-- `.orchestrators`): each a state, or a function of state.
	items: any,
	cards: any?,
	asks: any?,
	orchestrators: any?,
	-- One orchestrator read by itself (a page that follows rows so).
	orchestrator_of: ((id: string) -> any)?,
	-- A thread's chat row (`{ title, status, … }`) and its progress row.
	thread_of: ((chat: string) -> any)?,
	progress_of: ((chat: string) -> any)?,
	-- An orchestrator's id: its cards alone (a state, or a function).
	filter: any?,
	-- The orchestrators' filter over the board (the Control room).
	show_filter: boolean?,
	-- Denser, and narrow: a page's side panel.
	compact: boolean?,
	-- Whether cards say whose they are (default: unless `filter` is given
	-- without `show_filter`).
	context: boolean?,
	now: (() -> number)?,
	-- Kept by the host where it keeps its view (each a state; else the
	-- board's own): "board" | "list"; "all" | a column; the open card's id;
	-- which drawers are open (`{ snoozed = true }`).
	view: any?,
	tab: any?,
	selected: any?,
	drawers: any?,
	-- A list whose host shows a card's detail itself (a page's panel), and
	-- keeps where its sections are folded (`{ open = fn(key, default) ->
	-- boolean, set = fn(key, open) }`: ds/orch_board_list).
	on_open_card: ((id: string) -> ())?,
	fold: any?,
	on_open_thread: ((chat: string, message: string?) -> ())?,
	on_link: ((link: any) -> ())?,
	on_source: ((source: any) -> ())?,
	-- An ask answered (ds/orch_decide's choice), opened whole, or put off
	-- until a time (nil: back now).
	on_answer: ((ask: any, choice: any) -> any)?,
	on_expand_ask: ((ask: any) -> ())?,
	on_snooze: ((ask_id: string, until_at: number?) -> any)?,
	-- A card marked done (or dropped), opened again, kept for later (or
	-- let go), paused, resumed, started, sent back with what to change,
	-- taken off the board, put back, and opened (seen). Each may answer a
	-- promise (`:after(done, failed)`): refused, the board says so.
	on_close: ((id: string, dropped: boolean) -> any)?,
	on_reopen: ((id: string) -> any)?,
	on_hold: ((id: string, held: boolean) -> any)?,
	on_pause: ((id: string) -> any)?,
	on_resume: ((id: string, state: string?) -> any)?,
	on_start: ((id: string) -> any)?,
	on_request_changes: ((id: string, text: string) -> any)?,
	on_remove: ((id: string) -> any)?,
	on_restore: ((item: any) -> any)?,
	on_seen: ((id: string) -> any)?,
	-- A new task for an orchestrator (nil: whoever takes it), as a dump.
	on_new_task: ((text: string, orchestrator_id: string?) -> any)?,
	[string]: any,
}

export type Ctx = {
	props: Props,
	compact: boolean,
	-- Whether cards say whose they are.
	owners: boolean,
	at: () -> number,
	clock: (since: number) -> number,
	board: () -> Board,
	entry: (id: string?) -> Entry?,
	ids: (place: string) -> { string },
	counts: () -> Counts,
	filter: () -> string?,
	set_filter: (id: string?) -> (),
	orchestrators: () -> { any },
	owner: (id: string?) -> any?,
	thread: (chat: string?) -> any?,
	progress: (chat: string?) -> any?,
	selected: () -> string?,
	-- A card opened (its detail), or closed (nil).
	open: (id: string?) -> (),
}

local M = {}

-- A state's or a function's value (a plain value as it is).
local function read(v: any): any
	return control.value(v)
end
M.read = read

-- A state's `set`, where what was given is one (not a function of state).
local function setter(given: any): ((state: any, value: any) -> ())?
	local kind = type(given)
	if kind == "table" or kind == "userdata" then
		return given.set
	end
	return nil
end

-- A state of the host's, or the component's own.
local function kept(given: any, own: any): any
	if given ~= nil then
		return given
	end
	return own
end
M.kept = kept

-- A duration as a clock on a card says it: seconds while under a minute
-- (they show it moving), then to the minute.
function M.age(seconds: number, locale: any): string
	local t = math.max(0, math.floor(seconds))
	if t < 60 then
		return locale.s("time.short.seconds", { n = t })
	elseif t < 3600 then
		return locale.s("time.short.minutes", { n = t // 60 })
	end
	local m = t // 60
	if m % 60 == 0 or m >= 600 then
		return locale.s("time.short.hours", { n = m // 60 })
	end
	return locale.s("time.short.hours_minutes", { h = m // 60, m = m % 60 })
end

-- A promise's refusal said (a toast's words: `said(err)`), its success left
-- alone. What isn't a promise is nothing to wait on.
function M.after(sent: any, failed: ((err: any) -> ())?, done: (() -> ())?)
	if type(sent) == "table" and type((sent :: any).after) == "function" then
		(sent :: any):after(function()
			if done then
				done()
			end
		end, function(err: any)
			if failed then
				failed(err)
			end
		end)
	elseif done then
		done()
	end
end

-- Every place a card can stand in, a board's lists (ds/orch_board_model).
local PLACES = { "needs_you", "snoozed", "running", "planned", "done", "earlier" }

-- Each card's version, a small state its readers follow (`entry(id)`),
-- bumped when its Entry changes. States are made as cards are first read
-- and kept by a board's slot: a board gone hands its slot (and its states)
-- to the next one made, so boards opened and closed reuse them.
local versions: { [number]: { [string]: any } } = {}
local free: { number } = {}
local slots = 0
local function take_slot(): number
	local n = table.remove(free)
	if n then
		return n
	end
	slots += 1
	versions[slots] = {}
	return slots
end
local function give_slot(n: number)
	table.insert(free, n)
end
local function version(n: number, id: string): any
	local held = versions[n]
	local v = held[id]
	if not v then
		v = ui.state(`card:{n}:{id}`, 0)
		held[id] = v
	end
	return v
end
local function bump(n: number, id: string)
	local v = versions[n][id]
	-- (Never read: nobody to wake.)
	if v then
		v:set(v:peek() + 1)
	end
end

-- Made inside a component: its clock is the component's.
function M.make(props: Props): Ctx
	local compact = props.compact == true
	local own_filter = ui.use_state(nil :: string?)
	local own_selected = ui.use_state(nil :: string?)
	local selected_state = kept(props.selected, own_selected)

	local function filter(): string?
		if props.filter ~= nil then
			return read(props.filter) :: string?
		end
		return own_filter:get()
	end
	local function set_filter(id: string?)
		local set = setter(props.filter)
		if set then
			set(props.filter, id)
		else
			own_filter:set(id)
		end
	end

	-- The time: the host's (a fixture's), else a clock that ticks each
	-- second while a card's status is under two minutes old (its seconds
	-- show), and on the minute otherwise. Two states, so a clock past its
	-- seconds (and the board) wakes on the minute alone.
	local tick = ui.use_state(os.time())
	local minute = ui.use_state(os.time())
	local fresh = false
	if not props.now then
		ui.every(1000, function(): boolean?
			local t = os.time()
			if t // 60 ~= minute:peek() // 60 then
				ui.batch(function()
					minute:set(t)
					tick:set(t)
				end)
			elseif fresh then
				tick:set(t)
			end
			return nil
		end)
	end
	local function at(): number
		if props.now then
			return props.now()
		end
		return tick:get()
	end
	local function at_minute(): number
		if props.now then
			return props.now()
		end
		return minute:get()
	end
	local function clock(since: number): number
		if props.now then
			return props.now()
		end
		if tick:peek() - since < 120 then
			return tick:get()
		end
		return minute:get()
	end

	-- The boards: the filter's, and every card's (a detail open on one the
	-- filter hides). Plain tables, never state: what reads them follows the
	-- small states below, set only where something changed.
	local shown: Board, all: Board
	local function build(): (Board, Board)
		trace.hit("board.build")
		local t = at_minute()
		local rows = { items = read(props.items), cards = read(props.cards), asks = read(props.asks) }
		local f = filter()
		local b = model.board(rows, { filter = f, at = t })
		local a = if f == nil then b else model.board(rows, { at = t })
		-- (Whether a clock shows its seconds: read by the tick, untracked.)
		local young = false
		for _, place in { "needs_you", "running" } do
			for _, e in model.of(b, place) do
				if t - e.status_since < 120 then
					young = true
				end
			end
		end
		fresh = young
		return b, a
	end
	local function ids_of(b: Board, place: string): { string }
		local out: { string } = {}
		for _, e in model.of(b, place) do
			table.insert(out, e.id)
		end
		return out
	end
	local function same_ids(x: { string }, y: { string }): boolean
		if #x ~= #y then
			return false
		end
		for i, id in x do
			if y[i] ~= id then
				return false
			end
		end
		return true
	end

	shown, all = build()
	local n = take_slot()
	ui.on_cleanup(function()
		give_slot(n)
	end)
	-- A place's ids (as they were last set, plain), and the state its
	-- readers follow.
	local held_ids: { [string]: { string } } = {}
	local ids_state: { [string]: any } = {}
	for _, place in PLACES do
		held_ids[place] = ids_of(shown, place)
		ids_state[place] = ui.use_state(held_ids[place])
	end
	local counts = ui.use_state(shown.counts :: Counts)
	local whole = ui.use_state(0)

	-- A push: the boards again, and what changed woken.
	ui.effect(function()
		local was_shown, was_all = shown, all
		local b, a = build()
		if b == was_shown and a == was_all then
			return
		end
		shown, all = b, a
		ui.batch(function()
			if a ~= was_all then
				for id, e in a.by_id do
					if was_all.by_id[id] ~= e then
						bump(n, id)
					end
				end
				for id in was_all.by_id do
					if a.by_id[id] == nil then
						bump(n, id)
					end
				end
			end
			if b ~= was_shown then
				for _, place in PLACES do
					local now_ids = ids_of(b, place)
					if not same_ids(now_ids, held_ids[place]) then
						held_ids[place] = now_ids
						ids_state[place]:set(now_ids)
					end
				end
				-- (An equal value wakes nobody.)
				counts:set(b.counts)
				whole:set(whole:peek() + 1)
			end
		end)
	end)

	local function orchestrators(): { any }
		return (read(props.orchestrators) :: { any }?) or {}
	end
	local function owner(id: string?): any?
		if not id then
			return nil
		end
		if props.orchestrator_of then
			return props.orchestrator_of(id)
		end
		for _, o in orchestrators() do
			if o.id == id then
				return o
			end
		end
		return nil
	end

	local owners = props.context
	if owners == nil then
		owners = props.filter == nil or props.show_filter == true
	end

	return {
		props = props,
		compact = compact,
		owners = owners == true,
		at = at,
		clock = clock,
		board = function(): Board
			whole:get()
			return shown
		end,
		entry = function(id: string?): Entry?
			if not id then
				return nil
			end
			version(n, id):get()
			return all.by_id[id]
		end,
		ids = function(place: string): { string }
			local st = ids_state[place]
			if not st then
				return {}
			end
			return st:get()
		end,
		counts = function(): Counts
			return counts:get()
		end,
		filter = filter,
		set_filter = set_filter,
		orchestrators = orchestrators,
		owner = owner,
		thread = function(chat: string?): any?
			local of = props.thread_of
			if not chat or not of then
				return nil
			end
			return of(chat)
		end,
		progress = function(chat: string?): any?
			local of = props.progress_of
			if not chat or not of then
				return nil
			end
			return of(chat)
		end,
		selected = function(): string?
			return selected_state:get()
		end,
		open = function(id: string?)
			if id and props.on_open_card then
				props.on_open_card(id)
				return
			end
			selected_state:set(id)
			-- (Opened: seen, its dot gone.)
			if id and props.on_seen then
				local e = all.by_id[id]
				if e and e.card and e.card.unseen then
					props.on_seen(id)
				end
			end
		end,
	}
end

return M
