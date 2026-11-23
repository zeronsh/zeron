--!strict
-- What the board's parts share: the rows a host gives it, read once, and
-- what the person can do to a card. The board (ds/orch_board), its list
-- view (ds/orch_board_list) and a card's detail (ds/orch_board_detail)
-- each take the same props and make one of these from them, so a host
-- wires a board once and its list or a detail the same way.
--
--   local ctx = Ctx.make(props)     -- inside a component
--   ctx.board()                     the cards by place (ds/orch_board_model)
--   ctx.entry(id)                   one card, wherever it stands
--   ctx.at()                        Unix seconds, ticking while a clock shows
--   ctx.act.<what>(…)               what the person does (each a host's prop)

local model = require("@self/orch_board_model")
local control = require("@self/control")

export type Entry = model.Entry
export type Board = model.Board

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
	board: () -> Board,
	entry: (id: string?) -> Entry?,
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
	-- show), and on the minute otherwise.
	local tick = ui.use_state(os.time())
	local fresh = false
	if not props.now then
		ui.every(1000, function(): boolean?
			local t = os.time()
			if fresh or t // 60 ~= tick:peek() // 60 then
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

	local rows = ui.computed(function(): any
		return { items = read(props.items), cards = read(props.cards), asks = read(props.asks) }
	end)
	local board = ui.computed(function(): Board
		local t = at()
		local b = model.board(rows:get(), { filter = filter(), at = t })
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
		return b
	end)
	-- Every card, whoever's it is (a detail open on one the filter hides).
	local all = ui.computed(function(): Board
		if filter() == nil then
			return board:get()
		end
		return model.board(rows:get(), { at = at() })
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
		board = function(): Board
			return board:get()
		end,
		entry = function(id: string?): Entry?
			if not id then
				return nil
			end
			return all:get().by_id[id]
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
				local e = all:peek().by_id[id]
				if e and e.card and e.card.unseen then
					props.on_seen(id)
				end
			end
		end,
	}
end

return M
