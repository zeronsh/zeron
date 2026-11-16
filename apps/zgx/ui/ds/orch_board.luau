--!strict
-- The board: where every piece of an orchestrator's work stands, in
-- columns Queued, Now, Blocked, Review and Done (today). Shared by the
-- orchestrator's page (filtered to it, `compact`) and the Control room
-- (all of them, its filter on top).
--
-- Queued is what isn't started, in two groups: Up next (cards `planned`:
-- real work waiting for its turn, or for a slot while its crew is full) and,
-- under it, Later (cards `inbox`: someday, remind me, ideas, never started
-- on their own), folded until opened (`later_open`, its count on its
-- head). Its head counts Up next. A card dragged from one group to the
-- other changes its state; dragged into Now, it starts.
--
-- The columns share the board's width evenly, each a lane of its own, its
-- name, glyph and count on one line across them all; an empty one keeps
-- its width, quiet, but Blocked, which stands only while it holds a card
-- (and while one is carried, to be put there): the others take its room.
-- Each shows ten cards at most (Queued: Up next first, then Later where
-- it's open), "Show N more" under
-- the rest; shown, every column shows all of its (one choice for them all,
-- `expanded`), and "Show less" puts them all back. Done's head clears it:
-- every card in it taken off the board at once, as Delete takes one, one
-- Undo putting them all back. Narrower than the columns need, they stand in rows of
-- three, as wide as each other still; narrower still (a page's side pane),
-- one under another. A board with nothing on it (or nothing of the
-- orchestrator chosen) is one quiet line.
--
-- A card is calm: its title (two lines at most, a long one clamped) and one
-- quiet line under it. That line leads with whose it is (its
-- orchestrator's hue, named in its tooltip) where cards say so, then only
-- what adds something: its priority's word when urgent or high, how its
-- threads stand when that's worth a look (waiting on you, where one of
-- them asked the person and waits on the answer; working, stalled, needs
-- you, failed), its todo's cells and figure once a step of it is done, and
-- one clock (how long it has worked, how long stalled, or how long since it
-- last moved while it waits), then its pull request. Its menu's "…" shows
-- where the pointer rests. A card in Review or Done carries its outcome,
-- two lines of it, all of it in a hover card. Each card is a plugin slot
-- (`orchestration.item`, `provide = { item = id }`).
--
-- Cards are dragged between columns and within one (`on_move(id, state,
-- order)`, the order between its new neighbours', within its priority):
-- pressed and moved a few points, from anywhere on it and however fast, it
-- lifts and follows the pointer, the cards where it would land making room;
-- let go over the board it lands there, elsewhere (or Escape) it goes back.
-- A card in Review (finished work for the person to evaluate) has its end
-- under its status line and atop its menu: Approve (Done), or Request
-- changes… (a small box for what to change, sent to its thread; the card
-- back in Now).
--
-- A click opens its thread. A secondary click opens its menu: Start now
-- (a Queued card with no thread yet), its thread, Move to (Up next, Later,
-- Now, Blocked, Review, Done), its priority, the dump it came from, and
-- Delete, which takes it off the board at once (never its chat), an Undo
-- beside it for a while.
--
-- A Queued card says where it will be worked (its project, else its
-- machine) and, held back by the engine while its crew is full, that it
-- waits for a slot. Carried
-- into Now (or Start now), one without a thread starts: "Starting…" on it
-- until its thread is there; if that fails, it goes back, with a word.
--
--   OrchBoard {
--     items = function() return state.items end,
--     orchestrators = function() return state.orchestrators end,
--     todo_of = fn(chat) -> { done, total, current? }?,
--     thread_of = fn(chat) -> { title, status?, agent? }?,
--     ask_of = fn(chat) -> Ask?, -- the open ask a thread raised, if any
--     filter = state_or_fn,      -- an orchestrator's id: its items alone
--     show_filter = true,        -- its chips over the board (the Control room)
--     inbox = true,              -- (no longer used: Later is in Queued)
--     now = fn() -> number,      -- Unix seconds: what "today" is
--     on_move = fn(id, state, order), on_priority = fn(id, priority),
--     on_open_thread = fn(chat, message?), on_link = fn(link),
--     on_answer = fn(ask, { option?, text? }),  -- a card waiting on a
--                                -- decision: its options as chips (ds/orch_choices)
--     on_expand_ask = fn(ask),   -- the whole of it (ds/orch_decision)
--     on_source = fn({ dump_id, route_id }),  -- a card's dump, shown
--     on_approve = fn(id),           -- a card in Review approved: Done
--     on_request_changes = fn(id, text),  -- changes asked for on a card in
--                                      -- Review: to its thread, back to Now
--     on_remove = fn(id) -> promise?,  -- a card taken off the board (its
--                                      -- chat untouched); Undo puts it
--     on_restore = fn(item) -> promise?,  -- back as it was
--     on_start = fn(id, order?) -> promise?,  -- a Queued card
--                                -- without a thread started: its thread
--                                -- made, the card in Now (at `order`)
--     space_name = fn(space_id) -> string?,   -- a project's name
--     device_name = fn(device_id) -> string?, -- a machine's name
--     compact = true,            -- denser cards (a page's side pane)
--     project_of = fn(chat) -> string?,  -- the name of the project a chat works in
--     context = false,           -- cards say nothing of whose they are (an
--                                -- orchestrator's own page, where it's
--                                -- implied: the default when `filter` is
--                                -- given without `show_filter`)
--     progress_of = fn(chat) -> Row?,  -- its lead thread's progress row
--                                -- (orchestration.progress[chat]): its clocks
--     expanded = state,          -- whether every column shows all its cards
--                                -- (a boolean state, kept where the page
--                                -- keeps its view; else the board's own)
--     later_open = state,        -- whether Queued's Later group is open (a
--                                -- boolean state, kept as `expanded`)
--     card_build = fn(build, i) -> Node,  -- how each card is built (a page
--                                -- building the board in the background:
--                                -- a card a turn, ds/stage's `Idle`)
--   }

local Icon = require("@self/icon")
local ContextMenu = require("@self/context_menu")
local Markdown = require("@self/markdown")
local HoverCard = require("@self/hover_card")
local badge = require("@self/badge")
local control = require("@self/control")
local locale = require("@self/locale")
local material = require("@self/material")
local motion = require("@self/motion")
local hue = require("@self/orch_hue")
local Cells = require("@self/orch_cells")
local Progress = require("@self/orch_progress")
local Clocks = require("@self/orch_progress_clocks")
local Card = require("@self/orch_card")
local Choices = require("@self/orch_choices")
local Tooltip = require("@self/tooltip")
local data = require("@self/orch_data")
local toast = require("@self/toast")
local Button = require("@self/button")
local TextField = require("@self/text_field")
local Ref = require("@self/orch_ref")
local trace = require("@self/orch_perf_trace")

export type Link = { kind: string, label: string, target: string }
export type Item = {
	id: string,
	orchestrator_id: string?,
	title: string,
	detail: string?,
	state: string,
	priority: string,
	progress: number?,
	threads: { string }?,
	links: { Link }?,
	outcome: string?,
	order: number?,
	created: number?,
	updated: number?,
	done: number?,
	-- The dump it came from, where a dump's part made it.
	source: { dump_id: string?, route_id: string? }?,
	-- Taken off the board by the person (kept, never shown).
	removed: boolean?,
	-- Where it will be worked, before it's started: its project (a space's
	-- id) and machine; and why the engine is holding it back in Next
	-- (`waiting`: its crew was full).
	space_id: string?,
	device_id: string?,
	waiting: string?,
}
export type Orchestrator = { id: string, kind: string?, name: string, hue: number, archived: boolean? }
export type Todo = { done: number, total: number, current: string? }
export type Thread = { title: string, status: string?, agent: string? }
export type Props = {
	items: control.Live<{ Item }>,
	orchestrators: control.Live<{ Orchestrator }>?,
	todo_of: ((chat: string) -> Todo?)?,
	thread_of: ((chat: string) -> Thread?)?,
	ask_of: ((chat: string) -> { [string]: any }?)?,
	progress_of: ((chat: string) -> { [string]: any }?)?,
	filter: control.Live<string?>?,
	show_filter: boolean?,
	inbox: boolean?,
	now: (() -> number)?,
	on_move: ((id: string, state: string, order: number) -> ())?,
	on_priority: ((id: string, priority: string) -> ())?,
	on_open_thread: ((chat: string, message: string?) -> ())?,
	on_answer: ((ask: any, choice: any) -> any)?,
	on_expand_ask: ((ask: any) -> ())?,
	on_link: ((link: Link) -> ())?,
	on_source: ((source: { dump_id: string?, route_id: string? }) -> ())?,
	on_remove: ((id: string) -> any)?,
	on_approve: ((id: string) -> ())?,
	on_request_changes: ((id: string, text: string) -> ())?,
	on_restore: ((item: Item) -> any)?,
	on_start: ((id: string, order: number?) -> any)?,
	space_name: ((space_id: string) -> string?)?,
	device_name: ((device_id: string) -> string?)?,
	compact: boolean?,
	project_of: ((chat: string) -> string?)?,
	context: boolean?,
	expanded: any?,
	later_open: any?,
	id: string?,
	card_build: ((build: () -> Node, i: number) -> Node)?,
}

-- The columns, in order: each its key, the state its cards are in (Queued:
-- Up next's, Later's too), and its name's key.
type Column = { key: string, state: string, name: string, later: string? }
local COLUMNS: { Column } = {
	{ key = "queued", state = "planned", later = "inbox", name = "orch.board.column.queued" },
	{ key = "now", state = "active", name = "orch.board.column.now" },
	{ key = "blocked", state = "blocked", name = "orch.board.column.blocked" },
	{ key = "review", state = "review", name = "orch.board.column.review" },
	{ key = "done", state = "done", name = "orch.board.column.done" },
}
-- Where Move to puts a card: Queued's two groups, then the rest.
type Place = { key: string, state: string, name: string }
local PLACES: { Place } = {
	{ key = "queued", state = "planned", name = "orch.board.column.up_next" },
	{ key = "queued", state = "inbox", name = "orch.board.column.later" },
	{ key = "now", state = "active", name = "orch.board.column.now" },
	{ key = "blocked", state = "blocked", name = "orch.board.column.blocked" },
	{ key = "review", state = "review", name = "orch.board.column.review" },
	{ key = "done", state = "done", name = "orch.board.column.done" },
}
-- Queued's two groups' states.
local UP_NEXT, LATER = "planned", "inbox"

local RANK: { [string]: number } = { urgent = 0, high = 1, normal = 2, low = 3 }
local PRIORITIES = { "urgent", "high", "normal", "low" }

-- The least a column takes side by side: narrower than that for them all,
-- they stand in rows of `PER_ROW`; narrower than that for those, one under
-- another.
local COLUMN_MIN = 160
-- The most cards a column shows until its cards are all shown.
local CAP = 10
-- The columns that stand only while they hold a card.
local ONLY_HELD: { [string]: boolean } = { blocked = true }
local PER_ROW = 3
-- Each column's glyph at its head: how its cards stand.
local COLUMN_GLYPH: { [string]: Icon.Name } = {
	queued = "clock-circle",
	now = "status-working",
	blocked = "status-failed",
	review = "status-input",
	done = "status-done",
}
local COLUMN_INK: { [string]: string } = {
	now = "$zeron.status.working",
	blocked = "$zeron.status.failed",
	review = "$zeron.status.input",
	done = "$zeron.status.done",
}
local CARD_RADIUS = Card.RADIUS
local DRAG_TAG = "orch-item"
-- An outcome longer than about two lines' words: all of it in a hover card.
local OUTCOME_FOLD = 64

local function space(key: string, fallback: number): number
	return (ui.theme.get(key) :: number?) or fallback
end

-- Whether `item` stands in `col` today (a done one, done since local
-- midnight: `data.shown_today`, as the Control room's count has it).
local function in_column(item: Item, col: Column, now: number): boolean
	return (item.state == col.state or item.state == col.later) and data.shown_today(item, now)
end

-- A Queued list (Up next's, then Later's) split: Up next's cards, Later's.
local function groups(list: { Item }): ({ Item }, { Item })
	local up, later = {}, {}
	for _, it in list do
		table.insert(if it.state == LATER then later else up, it)
	end
	return up, later
end

local function sorted(list: { Item }): { Item }
	table.sort(list, function(a: Item, b: Item): boolean
		local ra, rb = RANK[a.priority] or 2, RANK[b.priority] or 2
		if ra ~= rb then
			return ra < rb
		end
		local oa, ob = a.order or 0, b.order or 0
		if oa ~= ob then
			return oa < ob
		end
		return a.id < b.id
	end)
	return list
end

-- Its threads' todo summed: done and total (nil when none says).
local function todo_of_item(item: Item, todo_of: ((string) -> Todo?)?): (number?, number?)
	if not todo_of then
		return nil, nil
	end
	local done, total = 0, 0
	for _, chat in item.threads or {} do
		local t = todo_of(chat)
		if t and t.total and t.total > 0 then
			done += t.done or 0
			total += t.total
		end
	end
	if total == 0 then
		return nil, nil
	end
	return done, total
end

-- Which of its threads' statuses leads: the one that needs you first, then
-- one that failed, then one working.
local LEAD: { [string]: number } = { input = 0, approval = 0, failed = 1, limited = 1, working = 2, connecting = 2, waiting = 2, done = 3 }

-- An outcome's words as one quiet run (its lines folded): the markdown's
-- marks left out.
local function plain(md: string): string
	local s = string.gsub(md, "%*%*", "")
	s = string.gsub(s, "`", "")
	s = string.gsub(s, "%[([^%]]*)%]%b()", "%1")
	s = string.gsub(s, "\n+", " ")
	return s
end

-- How its threads stand where that's worth a word on its card: the lead
-- thread's status (nil: none says, or it's idle).
local function thread_status(item: Item, thread_of: ((string) -> Thread?)?): string?
	local best, rank = nil :: string?, math.huge
	for _, chat in item.threads or {} do
		local t = if thread_of then thread_of(chat) else nil
		local st = t and t.status
		if st and LEAD[st] and LEAD[st] < rank then
			best, rank = st, LEAD[st]
		end
	end
	return best
end

-- A status's word on a card, and its ink (nil: it says nothing). Working is
-- the quiet default; what wants the person stands out.
local WORD: { [string]: { key: string, ink: string } } = {
	working = { key = "ds.status.working", ink = "$text.secondary" },
	connecting = { key = "ds.status.working", ink = "$text.secondary" },
	waiting = { key = "ds.status.working", ink = "$text.secondary" },
	input = { key = "orch.board.needs_you", ink = "$zeron.status.input" },
	approval = { key = "orch.board.needs_you", ink = "$zeron.status.input" },
	failed = { key = "ds.status.failed", ink = "$zeron.status.failed" },
	limited = { key = "ds.status.failed", ink = "$zeron.status.failed" },
}
local STALLED = { key = "ds.orch.progress.stalled", ink = "$zeron.tone.warning.ink" }
local STARTING = { key = "orch.board.starting", ink = "$text.secondary" }
local QUEUED = { key = "orch.board.queued", ink = "$text.tertiary" }
-- Captured or queued, not started: Inbox's and Next's.
local WAITING: { [string]: boolean } = { inbox = true, planned = true }
-- One of its threads asked the person and waits on the answer (gone stale
-- unanswered: in the warning's ink).
local ASKED = { key = "orch.board.waiting_on_you", ink = "$zeron.status.input" }
local ASKED_LONG = { key = "orch.board.waiting_on_you", ink = "$zeron.tone.warning.ink" }
-- Its ask put off until later (ds/orch_data `deferred`).
local DEFERRED = { key = "orch.board.deferred", ink = "$text.tertiary" }

-- Which card each open ask waits on, by card id: the one its `item_id`
-- names (the card of the asking turn's own work), else its thread's newest
-- card still at work. One card an ask: a thread's other cards, finished or
-- not, say nothing of it. A quiet ask (a "hasn't answered" chase) marks
-- none.
local function asks_by_card(list: { Item }, ask_of: ((string) -> { [string]: any }?)?): { [string]: { [string]: any } }
	local out: { [string]: { [string]: any } } = {}
	if not ask_of then
		return out
	end
	local ids: { [string]: boolean } = {}
	for _, it in list do
		ids[it.id] = true
	end
	local seen: { [string]: boolean } = {}
	for _, it in list do
		for _, chat in it.threads or {} do
			local a = ask_of(chat)
			if a and a.state == "open" and not a.quiet and not seen[a.id] then
				seen[a.id] = true
				local named: string? = a.item_id
				local target: string? = if named and ids[named] then named else nil
				if not target then
					local newest: Item? = nil
					for _, other in list do
						local at_work = other.state ~= "done" and other.state ~= "review" and other.state ~= "dropped"
						if at_work and table.find(other.threads or {}, chat) and (not newest or (other.created or 0) > (newest.created or 0)) then
							newest = other
						end
					end
					target = if newest then newest.id else nil
				end
				if target then
					local had = out[target]
					-- (An escalated one first.)
					if not had or (a.escalated ~= nil and had.escalated == nil) then
						out[target] = a
					end
				end
			end
		end
	end
	return out
end
-- Its priority's word, where it says something.
local PRIORITY_INK: { [string]: string } = { urgent = "$zeron.tone.danger.ink", high = "$zeron.tone.warning.ink" }

local function rect_of(r: any): { x: number, y: number, width: number, height: number }?
	local el = if r then r:peek() else nil
	if not el then
		return nil
	end
	return ui.window_rect(el)
end

local OrchBoard = ui.component(function(props: Props): Node
	local now = props.now or os.time
	local compact = props.compact == true
	local gap = space("space.1", 4)
	local own_filter = ui.use_state(nil :: string?)
	local function filter(): string?
		if props.filter ~= nil then
			return control.value(props.filter) :: string?
		end
		return own_filter:get()
	end
	local function set_filter(id: string?)
		local f = props.filter :: any
		local kind = type(f)
		if (kind == "table" or kind == "userdata") and (f :: any).set then
			(f :: any).set(f, id)
		else
			own_filter:set(id)
		end
	end
	local function orchestrators(): { Orchestrator }
		return (control.value(props.orchestrators) :: { Orchestrator }?) or {}
	end
	local function owner_of(id: string?): Orchestrator?
		if not id then
			return nil
		end
		for _, o in orchestrators() do
			if o.id == id then
				return o
			end
		end
		return nil
	end

	-- Whether every column shows all its cards: one choice for them all.
	local own_expanded = ui.use_state(false)
	local function expanded(): boolean
		local e = props.expanded
		if e ~= nil then
			return (e :: any):get() == true
		end
		return own_expanded:get()
	end
	local function set_expanded(on: boolean)
		local e = props.expanded
		if e ~= nil then
			(e :: any):set(on)
		else
			own_expanded:set(on)
		end
	end

	-- Whether Queued's Later group is open.
	local own_later = ui.use_state(false)
	local function later_open(): boolean
		local e = props.later_open
		if e ~= nil then
			return (e :: any):get() == true
		end
		return own_later:get()
	end
	local function set_later_open(on: boolean)
		local e = props.later_open
		if e ~= nil then
			(e :: any):set(on)
		else
			own_later:set(on)
		end
	end

	local columns: { Column } = table.clone(COLUMNS)

	-- A card just dropped, where it went, until the items say so (the
	-- engine answers a beat later): it stays where it was let go.
	type Pending = { id: string, state: string, order: number }
	local pending = ui.use_state(nil :: Pending?)
	-- Cards deleted here, gone at once (the engine's word follows).
	local gone = ui.use_state({} :: { [string]: boolean })
	-- Cards being started here, until their thread is there (or that fails).
	local starting = ui.use_state({} :: { [string]: boolean })
	local function set_starting(id: string, on: boolean)
		local now = table.clone(starting:peek())
		now[id] = if on then true else nil
		starting:set(now)
	end

	-- The card each open ask waits on (asks_by_card).
	local waits = ui.computed(function(): { [string]: { [string]: any } }
		return asks_by_card((control.value(props.items) :: { Item }?) or {}, props.ask_of)
	end)

	-- The items shown, by column, each sorted (filtered first).
	local shown = ui.computed(function(): { [string]: { Item } }
		local list = (control.value(props.items) :: { Item }?) or {}
		local only = filter()
		local moved = pending:get()
		local at = now()
		local today = data.day_start(at)
		local out: { [string]: { Item } } = {}
		for _, c in columns do
			out[c.key] = {}
		end
		local hidden = gone:get()
		for _, it in list do
			if it.removed or hidden[it.id] then
				continue
			end
			if moved and it.id == moved.id and (it.state ~= moved.state or it.order ~= moved.order) then
				local copy = table.clone(it)
				copy.state, copy.order = moved.state, moved.order
				if moved.state == "done" and (copy.done or 0) < today then
					copy.done = now()
				end
				it = copy
			end
			if not only or it.orchestrator_id == only then
				for _, c in columns do
					if in_column(it, c, at) then
						table.insert(out[c.key], it)
						break
					end
				end
			end
		end
		for k, l in out do
			if k == "queued" then
				local up, later = groups(l)
				out[k] = table.move(sorted(later), 1, #later, #up + 1, sorted(up))
			else
				out[k] = sorted(l)
			end
		end
		return out
	end)
	-- The cards a column shows, in order: ten at most until all are shown
	-- (Queued: Up next's, then Later's where it's open).
	local function visible(key: string, list: { Item }): { Item }
		local l = list
		if key == "queued" and not later_open() then
			l = (groups(l))
		end
		if expanded() or #l <= CAP then
			return l
		end
		return table.move(l, 1, CAP, 1, {})
	end
	-- How many a column has past the ten it shows (Queued: of those it
	-- would show, a folded Later's not among them).
	local function past_cap(key: string, list: { Item }): number
		local n = #list
		if key == "queued" and not later_open() then
			local up = groups(list)
			n = #up
		end
		return math.max(0, n - CAP)
	end
	local function empty(): boolean
		for _, l in shown:get() do
			if #l > 0 then
				return false
			end
		end
		return true
	end
	-- Whether cards say whose they are: everywhere but an orchestrator's
	-- own page (filtered to it, no filter of its own to show).
	local says_whose = if props.context ~= nil then props.context else not (props.filter ~= nil and not props.show_filter)
	local function project_of(chat: string?): string?
		local of = props.project_of
		if not chat or not of then
			return nil
		end
		local name = of(chat)
		return if name and name ~= "" then name else nil
	end
	-- Whose an item is: its orchestrator, and the project it's in (its
	-- threads', else its orchestrator's), the project left out where it's
	-- the orchestrator's own name.
	local function context_of(it: Item): (Orchestrator?, string?)
		if not says_whose then
			return nil, nil
		end
		local o = owner_of(it.orchestrator_id)
		local project: string? = nil
		for _, chat in it.threads or {} do
			project = project_of(chat)
			if project then
				break
			end
		end
		if not project and o then
			project = project_of(o.id)
		end
		if o and project and string.lower(project) == string.lower(o.name) then
			project = nil
		end
		return o, project
	end
	-- A card as a reference (ds/orch_ref): its state, first thread, its
	-- orchestrator, the project it's in.
	local function ref_of(it: Item): Ref.Ref
		local threads = it.threads or {}
		local project: string? = nil
		for _, chat in threads do
			project = project_of(chat)
			if project then
				break
			end
		end
		if not project and it.orchestrator_id then
			project = project_of(it.orchestrator_id)
		end
		return Ref.make("card", it.id, it.title, {
			state = it.state,
			thread = threads[1],
			orchestrator = it.orchestrator_id,
			project = project,
		})
	end

	-- A drag. `carry`: the card carried (its column, size and rank); `target`:
	-- where it would land (its column, its place among the others there from
	-- 1, and where that is in the column, from its top); `phase`: "idle",
	-- "carrying", or "landed" (let go on the board: the cards there take their
	-- new places at once, not sliding back).
	type Carry = { id: string, col: string, h: number, w: number, rank: number }
	type Target = { col: string, index: number, y: number, state: string? }
	local carry = ui.use_state(nil :: Carry?)
	local target = ui.use_state(nil :: Target?)
	local phase = ui.use_state("idle")
	local lane_refs: { [string]: any } = {}
	local body_refs: { [string]: any } = {}
	local slot_refs: { [string]: any } = {}
	-- Queued's Later head: above it Up next, below it Later.
	local later_ref = ui.ref()
	local function set_target(t: Target?)
		local was = target:peek()
		if was and t and was.col == t.col and was.index == t.index and was.state == t.state and math.abs(was.y - t.y) < 0.5 then
			return
		end
		if was == nil and t == nil then
			return
		end
		target:set(t)
	end

	-- The cards of `col` other than the one carried, in order.
	local function others(col: string, c: Carry): { Item }
		local out: { Item } = {}
		for _, it in shown:peek()[col] or {} do
			if it.id ~= c.id then
				table.insert(out, it)
			end
		end
		return out
	end
	-- How far a card of `col` stands from the column's top, its own slot
	-- closed: the carried card's gone from its column.
	local function top_in(col: string, id: string, c: Carry): number?
		local body, slot = rect_of(body_refs[col]), rect_of(slot_refs[id])
		if not body or not slot then
			return nil
		end
		local y = slot.y - body.y
		if col == c.col then
			for _, it in shown:peek()[col] or {} do
				if it.id == c.id then
					return y - (c.h + gap)
				elseif it.id == id then
					break
				end
			end
		end
		return y
	end

	-- Where the carried card would land with the pointer at (x, y) in the
	-- window: the column under it (else the nearest), before the first card
	-- whose middle is below it, kept among the cards of its own priority.
	local function target_at(x: number, y: number): Target?
		local c = carry:peek()
		if not c then
			return nil
		end
		local best: Column?, far = nil, math.huge
		for _, col in columns do
			local r = rect_of(lane_refs[col.key])
			if r and r.width > 0 and r.height > 0 then
				local dx = math.max(r.x - x, 0, x - (r.x + r.width))
				local dy = math.max(r.y - y, 0, y - (r.y + r.height))
				local d = dx * dx + dy * dy
				if d < far then
					best, far = col, d
				end
			end
		end
		if not best then
			return nil
		end
		local key = best.key
		local body = rect_of(body_refs[key])
		-- (Among the cards it shows: past ten, the rest are under "Show more".)
		local list = visible(key, others(key, c))
		-- (In Queued: Up next above Later's head, Later below it.)
		local state: string? = nil
		local offset = 0
		local floor: number? = nil
		if key == "queued" then
			local up, later = groups(list)
			local head = rect_of(later_ref)
			if head and head.height > 0 and y >= head.y then
				state, list, offset = LATER, later, #up
				if body then
					floor = head.y + head.height - body.y + gap
				end
			else
				state, list = UP_NEXT, up
			end
		end
		local index = #list + 1
		local tops: { number } = {}
		for i, it in list do
			local top = top_in(key, it.id, c)
			local slot = rect_of(slot_refs[it.id])
			tops[i] = top or 0
			if top and slot and body and index == #list + 1 and y < body.y + top + slot.height / 2 then
				index = i
			end
		end
		-- (Among the cards of its priority: the board keeps priority first.)
		local lo, hi = 1, 1
		for _, it in list do
			local r = RANK[it.priority] or 2
			if r < c.rank then
				lo += 1
			end
			if r <= c.rank then
				hi += 1
			end
		end
		index = math.clamp(index, lo, hi)
		local at: number
		if index <= #list then
			at = tops[index]
		elseif #list > 0 then
			local last = rect_of(slot_refs[list[#list].id])
			at = tops[#list] + (if last then last.height else 0) + gap
		else
			at = floor or 0
		end
		return { col = key, index = offset + index, y = at, state = state }
	end

	-- Its place where it was picked up (so nothing moves as it lifts).
	local function home(c: Carry): Target
		local index = 1
		local state: string? = nil
		for _, it in shown:peek()[c.col] or {} do
			if it.id == c.id then
				state = if c.col == "queued" then it.state else nil
				break
			end
			index += 1
		end
		return { col = c.col, index = index, y = top_in(c.col, c.id, c) or 0, state = state }
	end

	local function finish(how: string)
		phase:set(how)
		carry:set(nil)
		target:set(nil)
	end

	-- Whether `it` can be started here: captured or queued, no thread yet.
	local function startable(it: Item): boolean
		return props.on_start ~= nil and WAITING[it.state] == true and #(it.threads or {}) == 0
	end
	-- Started: in Now at once (at `order`, else its end), "Starting…" on it
	-- until its thread is there; back where it was, with a word, if not.
	local function start(it: Item, order: number?)
		local on_start = props.on_start
		if not on_start then
			return
		end
		local id = it.id
		local at = order
		if not at then
			local last = 0
			for _, x in shown:peek().now or {} do
				last = math.max(last, x.order or 0)
			end
			at = last + 1
		end
		pending:set({ id = id, state = "active", order = at :: number })
		set_starting(id, true)
		local function back()
			set_starting(id, false)
			local p = pending:peek()
			if p and p.id == id then
				pending:set(nil)
			end
		end
		local sent = on_start(id, order)
		if sent and (sent :: any).after then
			(sent :: any):after(function()
				-- (Then the items speak for themselves.)
				ui.after(4000, function()
					local p = pending:peek()
					if p and p.id == id then
						pending:set(nil)
					end
				end)
			end, function()
				back()
				toast.show { title = locale.s("orch.board.start_failed"), body = it.title, tone = "danger" }
			end)
		end
		-- (Never "Starting…" for good: the engine's word stands by then.)
		ui.after(30000, function()
			if starting:peek()[id] then
				back()
			end
		end)
	end

	-- Let go at `t`: its state, and the order between its new neighbours.
	local function land(c: Carry, t: Target)
		local col: Column? = nil
		for _, k in columns do
			if k.key == t.col then
				col = k
			end
		end
		local back = home(c)
		if not col or not props.on_move or (t.col == c.col and t.index == back.index and t.state == back.state) then
			return finish("idle")
		end
		local state = t.state or col.state
		local list = others(t.col, c)
		-- (Its neighbours of its own priority, and its own group in Queued:
		-- the board sorts by priority first, so another's order says nothing
		-- of where it stands.)
		local prev, nxt = list[t.index - 1], list[t.index]
		if prev and ((RANK[prev.priority] or 2) ~= c.rank or prev.state ~= state) then
			prev = nil
		end
		if nxt and ((RANK[nxt.priority] or 2) ~= c.rank or nxt.state ~= state) then
			nxt = nil
		end
		local order: number
		if prev and nxt then
			order = ((prev.order or 0) + (nxt.order or 0)) / 2
		elseif prev then
			order = (prev.order or 0) + 1
		elseif nxt then
			order = (nxt.order or 0) - 1
		else
			order = 0
		end
		local id = c.id
		-- (Queued, carried into Now: it starts.)
		if state == "active" then
			for _, it in shown:peek()[c.col] or {} do
				if it.id == id and startable(it) then
					finish("landed")
					return start(it, order)
				end
			end
		end
		pending:set({ id = id, state = state, order = order })
		finish("landed")
		;(props.on_move :: any)(id, state, order)
		-- (Once the engine has had time to say otherwise, the items speak
		-- for themselves.)
		ui.after(4000, function()
			local p = pending:peek()
			if p and p.id == id and p.order == order then
				pending:set(nil)
			end
		end)
	end
	local function accepts(payload: any): boolean
		local c = carry:peek()
		return c ~= nil and type(payload) == "table" and payload.tag == DRAG_TAG and payload.id == c.id
	end

	-- A card taken off the board: gone at once, an Undo beside it for a
	-- while (it comes back as it was); back, with a word, if that failed.
	local function set_gone(id: string, on: boolean)
		local now = table.clone(gone:peek())
		now[id] = if on then true else nil
		gone:set(now)
	end
	local function remove(it: Item)
		local on_remove = props.on_remove
		if not on_remove then
			return
		end
		local id = it.id
		set_gone(id, true)
		local function failed()
			set_gone(id, false)
			toast.show { title = locale.s("orch.board.delete_failed"), tone = "danger" }
		end
		local sent = on_remove(id)
		if sent and (sent :: any).after then
			(sent :: any):after(function() end, failed)
		end
		toast.show {
			key = `orch-board-removed-{id}`,
			title = locale.s("orch.board.deleted"),
			body = it.title,
			duration = 6000,
			action = if props.on_restore
				then {
					label = locale.s("orch.board.undo"),
					run = function()
						local back: any = (props.on_restore :: any)(it)
						set_gone(id, false)
						local function undo_failed()
							toast.show { title = locale.s("orch.board.undo_failed"), tone = "danger" }
						end
						if back ~= nil and (back :: any).after ~= nil then
							(back :: any):after(function() end, undo_failed)
						end
					end,
				}
				else nil,
		}
	end

	-- Every Done card taken off the board at once, as Delete takes one (each
	-- kept, its chat untouched); one Undo puts them all back.
	local function clear_done()
		local on_remove = props.on_remove
		if not on_remove then
			return
		end
		local list = table.clone(shown:peek().done or {})
		if #list == 0 then
			return
		end
		local hidden = table.clone(gone:peek())
		for _, it in list do
			hidden[it.id] = true
		end
		gone:set(hidden)
		local failed = false
		for _, it in list do
			local id = it.id
			local sent = on_remove(id)
			if sent and (sent :: any).after then
				(sent :: any):after(function() end, function()
					set_gone(id, false)
					if not failed then
						failed = true
						toast.show { title = locale.s("orch.board.delete_failed"), tone = "danger" }
					end
				end)
			end
		end
		toast.show {
			key = "orch-board-cleared-done",
			title = locale.s("orch.board.cleared", { n = #list }),
			duration = 6000,
			action = if props.on_restore
				then {
					label = locale.s("orch.board.undo"),
					run = function()
						local back = table.clone(gone:peek())
						local undo_failed = false
						for _, it in list do
							back[it.id] = nil
							local sent: any = (props.on_restore :: any)(it)
							if sent ~= nil and (sent :: any).after ~= nil then
								(sent :: any):after(function() end, function()
									if not undo_failed then
										undo_failed = true
										toast.show { title = locale.s("orch.board.undo_failed"), tone = "danger" }
									end
								end)
							end
						end
						gone:set(back)
					end,
				}
				else nil,
		}
	end

	-- Moved to the end of `place` (a column, or one of Queued's groups).
	local function move_to(it: Item, place: Place)
		if place.state == "active" and startable(it) then
			return start(it)
		end
		local last = 0
		for _, x in shown:peek()[place.key] or {} do
			if x.id ~= it.id and x.state == place.state then
				last = math.max(last, x.order or 0)
			end
		end
		local order = last + 1
		pending:set({ id = it.id, state = place.state, order = order })
		;(props.on_move :: any)(it.id, place.state, order)
	end

	-- The Review cards whose "Request changes…" box is open, by id.
	local changing = ui.use_state({} :: { [string]: boolean })
	local function set_changing(id: string, on: boolean)
		local now = table.clone(changing:peek())
		now[id] = if on then true else nil
		changing:set(now)
	end
	-- A card in Review: finished, waiting on the person's evaluation.
	local function reviewable(it: Item): boolean
		return it.state == "review" and (props.on_approve ~= nil or props.on_request_changes ~= nil)
	end

	-- A card's menu: Approve and Request changes (in Review), its thread,
	-- Move to, its priority, its dump; Delete.
	local function menu(item: () -> Item): { any }?
		local it = item()
		local entries: { any } = {}
		if it.state == "review" and props.on_approve then
			table.insert(entries, {
				label = locale.s("orch.board.approve"),
				icon = "check",
				on_select = function()
					(props.on_approve :: any)(item().id)
				end,
			})
		end
		if it.state == "review" and props.on_request_changes then
			table.insert(entries, {
				label = locale.s("orch.board.request_changes"),
				icon = "pen",
				on_select = function()
					set_changing(item().id, true)
				end,
			})
		end
		if startable(it) and not starting:peek()[it.id] then
			table.insert(entries, {
				label = locale.s("orch.board.start_now"),
				icon = "action-play",
				on_select = function()
					start(item())
				end,
			})
		end
		local threads = it.threads or {}
		if props.on_open_thread and #threads > 0 then
			for _, chat in threads do
				local t = if props.thread_of then props.thread_of(chat) else nil
				table.insert(entries, {
					label = if #threads == 1 or not t
						then locale.s("orch.board.open_thread")
						else locale.s("orch.board.open_thread_named", { title = t.title }),
					icon = "chat-round-line",
					on_select = function()
						(props.on_open_thread :: any)(chat)
					end,
				})
			end
		end
		if props.on_move then
			local sub: { any } = {}
			for _, place in PLACES do
				table.insert(sub, {
					label = locale.s(place.name),
					checked = function(): boolean
						return item().state == place.state
					end,
					on_select = function()
						if item().state ~= place.state then
							move_to(item(), place)
						end
					end,
				})
			end
			table.insert(entries, { label = locale.s("orch.board.move_to"), icon = "arrow-right", submenu = sub })
		end
		if props.on_priority then
			local sub: { any } = {}
			for _, p in PRIORITIES do
				table.insert(sub, {
					label = locale.s(`orch.board.priority.{p}`),
					checked = function(): boolean
						return item().priority == p
					end,
					on_select = function()
						(props.on_priority :: any)(it.id, p)
					end,
				})
			end
			table.insert(entries, { label = locale.s("orch.board.priority.title"), icon = "flag", submenu = sub })
		end
		local src = it.source
		if props.on_source and src and src.dump_id and src.dump_id ~= "" then
			table.insert(entries, {
				label = locale.s("orch.board.show_dump"),
				icon = "pen-new-square",
				on_select = function()
					(props.on_source :: any)(src)
				end,
			})
		end
		if props.on_remove then
			if #entries > 0 then
				table.insert(entries, { separator = true })
			end
			table.insert(entries, {
				label = locale.s("common.delete"),
				icon = "trash-bin-minimalistic",
				danger = true,
				on_select = function()
					remove(item())
				end,
			})
		end
		return if #entries > 0 then entries else nil
	end

	-- The cards' clocks, to the minute: they move once a minute, on the
	-- minute (as the progress strip's), nothing by the second. A test's
	-- `now` is its clock.
	local minute = ui.use_state(os.time())
	if not props.now then
		ui.effect(function()
			minute:get()
			ui.after(Clocks.until_minute(os.time()) * 1000, function()
				minute:set(math.max(os.time(), minute:peek() + 1))
			end)
		end)
	end
	local function clock_now(): number
		return if props.now then props.now() else minute:get()
	end

	-- What a card shows lifted and carried: its title, at its size, risen
	-- off the board.
	local function lifted(it: Item, w: number, h: number): Node
		return ui.row {
			class = "orch-card-lifted",
			w = w,
			min_h = ui.raw(h, "the card's own height"),
			p = Card.pad(compact),
			items = "start",
			rounded = CARD_RADIUS,
			bg = "$zeron.menu.solid",
			border = 1,
			border_color = "$border",
			shadow = "$shadow.md",
			scale = motion.TAB_LIFT,
			ui.text {
				it.title,
				wrap = true,
				lines = 2,
				truncate = true,
				grow = 1,
				min_w = 0,
				text_size = "$font.size.sm",
				line_height = "$font.line.sm",
				weight = "$font.weight.medium",
				color = "$text.primary",
			},
		}
	end

	-- One card.
	local function card(first: Item, _i: any, row: any, col: Column): Node
		local function item(): Item
			return if row then row:get() else first
		end
		local id = first.id
		local hovered = ui.use_state(false)
		local change_text = ui.use_state("")
		local slot = ui.ref()
		slot_refs[id] = slot
		ui.on_cleanup(function()
			if slot_refs[id] == slot then
				slot_refs[id] = nil
			end
		end)
		local pad = Card.pad(compact)
		local function outcome(): string?
			local it = item()
			local o = it.outcome
			return if (it.state == "done" or it.state == "review") and o ~= nil and o ~= "" then o else nil
		end
		local function prs(): { Link }
			local out: { Link } = {}
			for _, l in item().links or {} do
				if l.kind == "pr" then
					table.insert(out, l)
				end
			end
			return out
		end

		-- Where it stands while another is carried: moved aside to make
		-- room where that one would land, or up into the room it left.
		local function shift(): number
			local c = carry:get()
			if not c or c.id == id then
				return 0
			end
			local t = target:get()
			local list = shown:get()[col.key] or {}
			local q, past = 0, false
			for _, it in list do
				if it.id == c.id then
					past = true
				else
					q += 1
					if it.id == id then
						break
					end
				end
			end
			local d = c.h + gap
			local off = 0
			if col.key == c.col and past then
				off -= d
			end
			if t and t.col == col.key and q >= t.index then
				off += d
			end
			return off
		end
		-- (Moved by its transform alone: nothing lays out again. Let go on the
		-- board, the cards are where they'll stay, and take that at once.)
		local off = ui.use_state(0)
		local run: any = nil
		ui.effect(function()
			local to = shift()
			if run then
				run:cancel()
				run = nil
			end
			if phase:peek() == "landed" or motion.reduced_now() or math.abs(off:peek() - to) < 0.5 then
				off:set(to)
				return
			end
			run = anime.animate(off, { value = to, ease = motion.tab_ease() })
		end)
		ui.on_cleanup(function()
			if run then
				run:cancel()
			end
		end)

		-- Its title: two lines at most (a long one, from before cards had
		-- short titles, clamped).
		local title_text = Card.Title {
			text = function(): string
				return item().title
			end,
			quiet = function(): boolean
				return item().state == "done"
			end,
		}

		-- Its one quiet line, each part only where it adds something.
		-- (What it reads of the board's live sources, each through a
		-- selector of the card's own: a write anywhere (another thread's
		-- status, the orchestration's state pushed whole) runs these few
		-- again, and nothing it shows unless its own value changed.)
		local own = ui.computed(function(): any
			local o = context_of(item())
			return if o then { id = o.id, name = o.name, hue = o.hue } else nil
		end)
		local function owner(): Orchestrator?
			return own:get()
		end
		local place = ui.computed(function(): any
			local it = item()
			if not WAITING[it.state] or #(it.threads or {}) > 0 then
				return nil
			end
			local project = if it.space_id and props.space_name then props.space_name(it.space_id) else nil
			if project and project ~= "" then
				return { kind = "project", name = project }
			end
			local device = if it.device_id and props.device_name then props.device_name(it.device_id) else nil
			if device and device ~= "" then
				return { kind = "device", name = device }
			end
			return nil
		end)
		local function where(): { kind: string, name: string }?
			return place:get()
		end
		local function urgency(): string?
			local p = item().priority
			return if PRIORITY_INK[p] then p else nil
		end
		-- Its lead thread's progress row (memoized: the row alone, not the
		-- table of every row it's looked up in).
		local lead_row = ui.computed(function(): any
			local chat = (item().threads or {})[1]
			return if chat and props.progress_of then props.progress_of(chat) else nil
		end)
		local function progress_row(): { [string]: any }?
			return lead_row:get()
		end
		local lead_status = ui.computed(function(): string?
			return thread_status(item(), props.thread_of)
		end)
		local todo = ui.computed(function(): any
			local d, t = todo_of_item(item(), props.todo_of)
			return if d and t then { d = d, t = t } else nil
		end)
		local ask = ui.computed(function(): any
			return waits:get()[id]
		end)
		local is_stalled = ui.computed(function(): boolean
			return item().state ~= "done" and Progress.stalled(progress_row())
		end)
		local function stalled(): boolean
			return is_stalled:get()
		end
		local the_word = ui.computed(function(): any
			trace.hit("board.word")
			local it = item()
			if starting:get()[id] and #(it.threads or {}) == 0 then
				return STARTING
			elseif it.state == "done" then
				return nil
			elseif WAITING[it.state] and (it.waiting or "") ~= "" then
				return QUEUED
			end
			local a = ask:get()
			if a then
				if data.deferred(a, clock_now()) then
					return DEFERRED
				end
				return if a.escalated ~= nil then ASKED_LONG else ASKED
			elseif stalled() then
				return STALLED
			end
			local st = lead_status:get()
			return if st then WORD[st] else nil
		end)
		local function word(): { key: string, ink: string }?
			return the_word:get()
		end
		-- How far along: its threads' todo once a step of it is done, else
		-- its own share once there is some (nil: nothing to show yet).
		local the_steps = ui.computed(function(): any
			if item().state == "done" then
				return nil
			end
			local td = todo:get()
			if td then
				return { d = if td.d > 0 then td.d else nil, t = td.t }
			end
			local p = item().progress
			if p and p > 0 then
				return { d = math.floor(math.clamp(p, 0, 1) * 10 + 0.5), t = 10 }
			end
			return nil
		end)
		local function steps(): (number?, number?)
			local s = the_steps:get()
			if not s then
				return nil, nil
			end
			return s.d, s.t
		end
		local function steps_text(): string
			local td = todo:get()
			if td then
				return `{td.d}/{td.t}`
			end
			local p = item().progress
			return if p then `{math.floor(math.clamp(p, 0, 1) * 100 + 0.5)}%` else ""
		end
		-- One clock: how long it's been stalled, else how long it has worked,
		-- else (waiting on someone) how long since it last moved.
		local the_clock = ui.computed(function(): string
			local it = item()
			if it.state == "done" or it.state == "planned" or it.state == "inbox" then
				return ""
			end
			local at = clock_now()
			local c = Progress.clocks(progress_row(), at)
			if stalled() and c.since_step then
				return Progress.short(c.since_step)
			elseif it.state == "active" then
				return if c.elapsed then Progress.short(c.elapsed) else ""
			elseif it.updated then
				return Progress.short(at - it.updated)
			end
			return ""
		end)
		local function clock(): string
			return the_clock:get()
		end
		local function has_word(): boolean
			return urgency() ~= nil or word() ~= nil
		end
		local function has_steps(): boolean
			return (steps()) ~= nil
		end
		local function has_clock(): boolean
			return clock() ~= ""
		end
		local sep = Card.Sep
		local menu_open = ui.use_state(false)
		local status_line = Card.Line {
			-- Whose it is: its orchestrator's hue, named where the pointer rests.
			ui.show(function(): boolean
				return owner() ~= nil
			end, ui.div {
				class = "orch-card-owner",
				shrink = 0,
				pr = "$zeron.space.hair",
				tooltip = Tooltip { function(): string
					local o = owner()
					return if o then o.name else ""
				end },
				tooltip_delay = 300,
				label = function(): string
					local o = owner()
					return if o then o.name else ""
				end,
				role = "img",
				hue.Dot {
					hue = function(): number?
						local o = owner()
						return o and o.hue
					end,
					size = 6,
				},
			}),
			-- Its name too, where the line has nothing else to say.
			ui.show(function(): boolean
				return owner() ~= nil and not (has_word() or has_steps() or has_clock())
			end, Card.Word {
				text = function(): string
					local o = owner()
					return if o then o.name else ""
				end,
				class = "orch-card-owner-name",
			}),
			-- Where it will be worked, while it waits: its project, else its
			-- machine.
			ui.show(function(): boolean
				return where() ~= nil
			end, ui.row {
				class = "orch-card-place",
				min_w = 0,
				shrink = 1,
				gap = "$zeron.space.tight",
				items = "center",
				ui.div {
					color = "$text.tertiary",
					shrink = 0,
					Icon {
						name = function(): any
							local w = where()
							return if w and w.kind == "device" then "monitor" else "folder"
						end,
						size = 11,
					},
				},
				Card.Word {
					text = function(): string
						local w = where()
						return if w then w.name else ""
					end,
					class = "orch-card-place-name",
				},
			}),
			ui.show(function(): boolean
				return urgency() ~= nil
			end, Card.Word {
				text = function(): string
					local u = urgency()
					return if u then locale.s(`orch.board.priority.{u}`) else ""
				end,
				class = "orch-card-priority",
				shrink = 0,
				strong = true,
				ink = function(): string
					return PRIORITY_INK[urgency() or ""] or "$text.tertiary"
				end,
			}),
			sep(function(): boolean
				return urgency() ~= nil and word() ~= nil
			end),
			ui.show(function(): boolean
				return word() ~= nil
			end, Card.Word {
				text = function(): string
					local w = word()
					return if w then locale.s(w.key) else ""
				end,
				class = "orch-card-word",
				ink = function(): string
					local w = word()
					return if w then w.ink else "$text.tertiary"
				end,
			}),
			sep(function(): boolean
				return has_word() and has_steps()
			end),
			ui.show(has_steps, ui.row {
				class = "orch-card-progress",
				shrink = 0,
				gap = "$zeron.space.tight",
				items = "center",
				Cells {
					done = function(): number
						local d = steps()
						return d or 0
					end,
					total = function(): number
						local _, t = steps()
						return t or 0
					end,
					current = function(): boolean
						return not stalled() and lead_status:get() == "working"
					end,
					cell = 3,
					h = 6,
					max = 10,
				},
				ui.text { steps_text, shrink = 0, tabular = true, text_size = "$font.size.xs", color = "$text.tertiary" },
			}),
			sep(function(): boolean
				return (has_word() or has_steps()) and has_clock()
			end),
			ui.show(has_clock, Card.Word {
				text = clock,
				class = "orch-card-clock",
				shrink = 0,
				ink = function(): string
					return if stalled() then "$zeron.tone.warning.ink" else "$text.tertiary"
				end,
			}),
			ui.div { grow = 1 },
			ui.each(ui.computed(prs), function(l: Link): Node
				local n = tonumber(string.match(l.label, "%d+"))
				return ui.button {
					label = l.label,
					shrink = 0,
					grow = 0,
					p = 0,
					h = "auto",
					rounded = "$radius.full",
					bg = "#00000000",
					hover = { bg = "#00000000" },
					active = { bg = "#00000000" },
					on_click = if props.on_link
						then function()
							(props.on_link :: any)(l)
						end
						else nil,
					badge.PullRequest { state = if item().state == "done" then "merged" else "open", number = n, size = "sm" },
				}
			end, { key = function(l: Link): string
				return l.target
			end }),
			-- Its menu, where the pointer rests (or while it's open).
			Card.Menu {
				items = function(): { any }
					return menu(item) or {}
				end,
				open = menu_open,
				shown = function(): boolean
					return hovered:get() and not carry:get()
				end,
				label = locale.t("orch.board.card_actions"),
			},
		}
		local function has_status(): boolean
			return owner() ~= nil or where() ~= nil or has_word() or has_steps() or has_clock() or #prs() > 0
		end

		-- Its outcome: two lines of it, quietly; all of it where the pointer
		-- rests, when there's more.
		local outcome_text = ui.show(function(): boolean
			return outcome() ~= nil
		end, ui.presence(function(): string
			local o = outcome()
			return if o and (#plain(o) > OUTCOME_FOLD or string.find(o, "\n") ~= nil) then "long" else "short"
		end, function(k: string): Node
			return ui.text {
				function(): string
					return plain(outcome() or "")
				end,
				class = "orch-outcome",
				w_full = true,
				min_w = 0,
				wrap = true,
				lines = 2,
				truncate = true,
				text_size = "$font.size.sm",
				line_height = "$font.line.sm",
				color = "$text.secondary",
				tooltip = if k == "long"
					then HoverCard.Card {
						class = "orch-outcome-full",
						w = 320,
						p = "$space.3",
						text_size = "$font.size.sm",
						color = "$text.secondary",
						Markdown {
							text = function(): string
								return item().outcome or ""
							end,
						},
					}
					else nil,
				tooltip_delay = 300,
				tooltip_placement = if k == "long" then { side = "bottom", align = "start", offset = 6 } else nil,
			}
		end, { w_full = true, min_w = 0 }))
		-- The decision it waits on, compact: the question in a line, its
		-- options as chips (not while it's put off).
		local function asked(): { [string]: any }?
			local a = ask:get()
			if not a or not props.on_answer or data.deferred(a, clock_now()) then
				return nil
			end
			return a
		end
		local decision = ui.show(function(): boolean
			return asked() ~= nil
		end, ui.column {
			class = "orch-card-decision",
			w_full = true,
			min_w = 0,
			gap = "$space.1",
			ui.text {
				function(): string
					local a = asked()
					return if a then plain(a.question or "") else ""
				end,
				class = "orch-card-question",
				w_full = true,
				min_w = 0,
				truncate = true,
				text_size = "$font.size.sm",
				line_height = "$font.line.sm",
				color = "$text.secondary",
			},
			Choices {
				ask = function(): any
					return asked()
				end,
				on_answer = function(a: any, choice: any): any
					if props.on_answer then
						return (props.on_answer :: any)(a, choice)
					end
					return nil
				end,
				on_expand = props.on_expand_ask,
				size = "xs",
				class = "orch-card-choices",
			},
		})
		local body = ui.column {
			w_full = true,
			min_w = 0,
			p = pad,
			ui.column {
				w_full = true,
				min_w = 0,
				gap = if compact then "$space.1" else "$zeron.space.snug",
				title_text,
				outcome_text,
				decision,
				ui.show(has_status, status_line),
				-- In Review: its end, Approve or Request changes (a box for
				-- what to change, sent to its thread).
				ui.show(function(): boolean
					return reviewable(item()) and not changing:get()[id]
				end, ui.row {
					class = "orch-card-review",
					gap = "$space.1",
					items = "center",
					pt = "$zeron.space.hair",
					ui.show(function(): boolean
						return props.on_approve ~= nil
					end, Button {
						tone = "secondary",
						size = "xs",
						on_click = function()
							(props.on_approve :: any)(id)
						end,
						locale.t("orch.board.approve"),
					}),
					ui.show(function(): boolean
						return props.on_request_changes ~= nil
					end, Button {
						tone = "ghost",
						size = "xs",
						on_click = function()
							set_changing(id, true)
						end,
						locale.t("orch.board.request_changes"),
					}),
				}),
				ui.show(function(): boolean
					return changing:get()[id] == true
				end, TextField {
					class = "orch-card-changes",
					value = change_text,
					size = "sm",
					w_full = true,
					autofocus = true,
					name = locale.t("orch.board.request_changes"),
					placeholder = locale.t("orch.board.changes_placeholder"),
					on_commit = function(text: string)
						local t = string.gsub(text, "^%s+", "")
						t = string.gsub(t, "%s+$", "")
						if t ~= "" and props.on_request_changes then
							(props.on_request_changes :: any)(id, t)
						end
						change_text:set("")
						set_changing(id, false)
					end,
					-- (Left empty: put away.)
					on_focus_change = function(focused: boolean)
						if not focused and change_text:peek() == "" then
							set_changing(id, false)
						end
					end,
					on_key = function(e: any): boolean?
						if e.key == "Escape" then
							change_text:set("")
							set_changing(id, false)
							return true
						end
						return nil
					end,
				}),
			},
			-- Where plugins decorate it (the github plugin: its checks).
			-- (Outside the steps' gaps: empty, it takes no room.)
			ui.div { hook = "orchestration.item", class = "orch-item-slot", provide = { item = id }, w_full = true },
		}
		local box = ui.button {
			class = "orch-card",
			label = function(): string
				return item().title
			end,
			w_full = true,
			p = 0,
			items = "start",
			rounded = CARD_RADIUS,
			bg = Card.bg(function(): boolean
				return hovered:get() and not carry:get()
			end),
			hover = {},
			active = { bg = "$pressed" },
			y = function(): number
				return off:get()
			end,
			-- (Carried, its place is the gap it leaves.)
			opacity = function(): number
				local c = carry:get()
				return if c and c.id == id then 0 else 1
			end,
			on_hover = function(inside: boolean)
				hovered:set(inside)
			end,
			-- Into its thread: waiting on a decision, at the message that
			-- asked (its thread's), else its lead thread.
			on_click = function()
				local threads = item().threads or {}
				if not props.on_open_thread or #threads == 0 then
					return
				end
				local a = waits:peek()[id]
				if a and a.from_chat and table.find(threads, a.from_chat :: string) then
					(props.on_open_thread :: any)(a.from_chat, a.turn_id)
				else
					(props.on_open_thread :: any)(threads[1])
				end
			end,
			-- Dragged: to another column (the board's), and a reference to it
			-- (Capture's composer takes `ref`): one payload, its drop the
			-- target's to choose.
			on_drag = function(): any
				local ref = ref_of(item())
				if not props.on_move then
					return { ref = ref }
				end
				local r = rect_of(slot)
				local c: Carry = {
					id = id,
					col = col.key,
					h = if r then r.height else 64,
					w = if r then r.width else COLUMN_MIN,
					rank = RANK[item().priority] or 2,
				}
				phase:set("carrying")
				carry:set(c)
				target:set(home(c))
				return { tag = DRAG_TAG, id = id, ref = ref }
			end,
			drag_preview = function(): Node
				local c = carry:peek()
				if not (c and c.id == id) then
					return Ref.Pill(ref_of(item()))
				end
				return lifted(item(), c.w, c.h)
			end,
			on_drag_end = function()
				-- (Let go off the board, or Escape: it goes back.)
				local c = carry:peek()
				if c and c.id == id then
					finish("idle")
				end
			end,
			body,
		}
		return ui.div {
			ref = slot,
			class = "orch-card-slot",
			w_full = true,
			ContextMenu {
				label = locale.t("orch.board.card_actions"),
				ring = CARD_RADIUS,
				w_full = true,
				shrink = 0,
				items = function(): { any }?
					return menu(item)
				end,
				box,
			},
		}
	end


	-- Queued's Later: its head ("Later · 4", folded or open, where a card
	-- carried below it lands in Later), and its cards while it's open. It
	-- moves with the cards above it while one is carried, as they do.
		-- A card, built as the page asks (`card_build`), else at once.
	local function built_card(it: Item, i: any, row: any, col: Column): Node
		local wrap = props.card_build
		if not wrap then
			return card(it, i, row, col)
		end
		return wrap(function(): Node
			return card(it, i, row, col)
		end, if type(i) == "number" then i else 0)
	end

local function later_group(col: Column, count: () -> number, cards: () -> { Item }): Node
		local function shift(): number
			local c = carry:get()
			if not c then
				return 0
			end
			local d = c.h + gap
			local off = 0
			local t = target:get()
			local carried: Item? = nil
			for _, it in shown:get()[col.key] or {} do
				if it.id == c.id then
					carried = it
				end
			end
			if carried and carried.state == UP_NEXT then
				off -= d
			end
			if t and t.col == col.key and t.state == UP_NEXT then
				off += d
			end
			return off
		end
		local off = ui.use_state(0)
		local run: any = nil
		ui.effect(function()
			local to = shift()
			if run then
				run:cancel()
				run = nil
			end
			if phase:peek() == "landed" or motion.reduced_now() or math.abs(off:peek() - to) < 0.5 then
				off:set(to)
				return
			end
			run = anime.animate(off, { value = to, ease = motion.tab_ease() })
		end)
		ui.on_cleanup(function()
			if run then
				run:cancel()
			end
		end)
		return ui.column {
			w_full = true,
			gap = "$space.1",
			-- (While a card is carried, Later is somewhere to put it.)
			hidden = function(): boolean
				return count() == 0 and carry:get() == nil
			end,
			ui.button {
				ref = later_ref,
				class = "orch-later-head",
				label = function(): string
					return locale.s("orch.board.later_count", { n = count() })
				end,
				expanded = later_open,
				w_full = true,
				h = 28,
				px = "$space.2",
				gap = "$zeron.space.tight",
				items = "center",
				rounded = CARD_RADIUS,
				color = "$text.tertiary",
				bg = "#00000000",
				hover = { bg = "$hover", color = "$text.secondary" },
				active = { bg = "$pressed" },
				y = function(): number
					return off:get()
				end,
				on_click = function()
					set_later_open(not later_open())
				end,
				ui.div {
					shrink = 0,
					Icon {
						name = function(): any
							return if later_open() then "alt-arrow-down" else "alt-arrow-right"
						end,
						size = 12,
					},
				},
				ui.text {
					function(): string
						return locale.s("orch.board.later_count", { n = count() })
					end,
					class = "orch-later-label",
					text_size = "$font.size.xs",
					weight = "$font.weight.medium",
				},
			},
			ui.each(ui.computed(cards), function(it: Item, i: any, row: any): Node
				return built_card(it, i, row, col)
			end, { key = function(it: Item): string
				return it.id
			end, w_full = true }),
		}
	end

	local function column(col: Column, mode: string): Node
		local stacked = mode == "stacked"
		local queued = col.later ~= nil
		local function list(): { Item }
			return shown:get()[col.key] or {}
		end
		-- (Queued's head counts Up next; its Later head, Later.)
		local function counted(): number
			if not queued then
				return #list()
			end
			local up = groups(list())
			return #up
		end
		local function later_count(): number
			local _, later = groups(list())
			return #later
		end
		-- What it shows: ten at most until all are (Queued: by group).
		local shows = ui.computed(function(): { Item }
			return visible(col.key, list())
		end)
		local lane, body = ui.ref(), ui.ref()
		lane_refs[col.key] = lane
		body_refs[col.key] = body
		-- Where the carried card would land in it: in an empty column, the
		-- column's own slot; among cards, the room they make.
		local function hole(): boolean
			local c = carry:get()
			if not c then
				return false
			end
			local t = target:get()
			return (t ~= nil and t.col == col.key) or #list() == 0
		end
		local function aimed(): boolean
			local t = target:get()
			return carry:get() ~= nil and t ~= nil and t.col == col.key
		end
		return ui.column {
			id = if props.id then `{props.id}-{col.key}` else `orch-col-{col.key}`,
			ref = lane,
			class = `orch-column orch-column-{col.key}`,
			label = locale.t(col.name),
			role = "group",
			w_full = stacked,
			grow = if stacked then nil else 1,
			basis = if stacked then nil else 0,
			min_w = if stacked then nil else COLUMN_MIN,
			p = "$space.1",
			rounded = "$radius.lg",
			bg = material.pick("$zeron.settings.note", "$surface.sunken"),
			-- (Stacked in a page's side pane, an empty column stands aside
			-- until a card is carried: then it's somewhere to put one. Inbox
			-- and Blocked stand only while they hold one; Blocked too while
			-- one is carried, to be put there.)
			hidden = if stacked and compact
				then function(): boolean
					return #list() == 0 and carry:get() == nil
				end
				elseif ONLY_HELD[col.key] then function(): boolean
					return #list() == 0 and not (col.key == "blocked" and carry:get() ~= nil)
				end
				else nil,
			gap = "$space.1",
			-- Its name and how many: one line across the columns.
			ui.row {
				class = "orch-column-head",
				w_full = true,
				h = 28,
				shrink = 0,
				px = "$space.2",
				gap = "$zeron.space.tight",
				items = "center",
				ui.div {
					shrink = 0,
					color = function(): string
						return if counted() > 0 then COLUMN_INK[col.key] or "$text.tertiary" else "$text.tertiary"
					end,
					Icon { name = COLUMN_GLYPH[col.key] or "checklist", size = 12 },
				},
				ui.text {
					locale.t(col.name),
					truncate = true,
					min_w = 0,
					text_size = "$font.size.xs",
					weight = "$font.weight.medium",
					color = function(): string
						return if counted() > 0 then "$text.primary" else "$text.tertiary"
					end,
				},
				ui.text {
					function(): string
						local n = counted()
						return if n > 0 then tostring(n) else ""
					end,
					tooltip = if queued then locale.s("orch.board.column.up_next") else nil,
					tooltip_delay = if queued then 300 else nil,
					class = "orch-column-count",
					tabular = true,
					text_size = "$font.size.xs",
					color = "$text.tertiary",
				},
				-- Done's: clear it.
				if col.key == "done" and props.on_remove
					then ui.row {
						grow = 1,
						justify = "end",
						ui.show(function(): boolean
							return #list() > 0
						end, ui.button {
							class = "orch-clear-done",
							label = locale.t("orch.board.clear_done"),
							tooltip = locale.s("orch.board.clear_done"),
							tooltip_delay = 300,
							h = 22,
							px = "$space.2",
							items = "center",
							rounded = "$radius.md",
							shrink = 0,
							color = "$text.tertiary",
							bg = "#00000000",
							hover = { bg = "$hover", color = "$text.secondary" },
							active = { bg = "$pressed" },
							on_click = clear_done,
							ui.text { locale.t("orch.board.clear"), text_size = "$font.size.xs", weight = "$font.weight.medium" },
						}),
					}
					else nil,
			},
			ui.column {
				ref = body,
				class = "orch-column-body",
				w_full = true,
				grow = 1,
				relative = true,
				gap = "$space.1",
				-- (Room for the card on its way in, once, as it's aimed here.)
				pb = function(): any
					local c = carry:get()
					return if c and aimed() and c.col ~= col.key then ui.raw(c.h + gap, "the carried card's height") else 0
				end,
				-- Where the carried card would land: a faint dashed slot.
				ui.show(hole, ui.div {
					class = "orch-column-drop",
					absolute = true,
					left = 0,
					right = 0,
					top = 0,
					h = function(): number
						local c = carry:get()
						return if c then c.h else 0
					end,
					y = function(): number
						local t = target:get()
						return if t and t.col == col.key then t.y else 0
					end,
					rounded = CARD_RADIUS,
					border = 1,
					border_dash = { 4, 3 },
					border_color = function(): string
						return if aimed() then "$text.tertiary" else "$border"
					end,
					pointer_events = "none",
				}),
				ui.each(ui.computed(function(): { Item }
					local l = shows:get()
					if not queued then
						return l
					end
					return (groups(l))
				end), function(it: Item, i: any, row: any): Node
					return built_card(it, i, row, col)
				end, { key = function(it: Item): string
					return it.id
				end, w_full = true }),
				if queued then later_group(col, later_count, function(): { Item }
					local _, later = groups(shows:get())
					return later
				end) else nil,
			},
			-- Past ten: the rest, for every column at once (and back).
			ui.show(function(): boolean
				return past_cap(col.key, list()) > 0 and carry:get() == nil
			end, ui.button {
				class = "orch-column-more",
				label = function(): string
					return if expanded()
						then locale.s("orch.board.show_less")
						else locale.s("orch.board.show_more", { n = past_cap(col.key, list()) })
				end,
				w_full = true,
				h = 28,
				shrink = 0,
				items = "center",
				justify = "center",
				rounded = CARD_RADIUS,
				color = "$text.tertiary",
				bg = "#00000000",
				hover = { bg = "$hover", color = "$text.secondary" },
				active = { bg = "$pressed" },
				on_click = function()
					set_expanded(not expanded())
				end,
				ui.text {
					function(): string
						return if expanded()
							then locale.s("orch.board.show_less")
							else locale.s("orch.board.show_more", { n = past_cap(col.key, list()) })
					end,
					text_size = "$font.size.xs",
					weight = "$font.weight.medium",
				},
			}),
		}
	end

	-- The filter's chips: every orchestrator, and all of them.
	local function chips(): Node
		local function chip(id: string?, name: () -> string, h: number?, suffix: (() -> string?)?): Node
			local hovered = ui.use_state(false)
			return ui.button {
				label = name,
				class = "orch-filter-chip",
				h = 24,
				px = "$space.2",
				gap = "$space.1",
				items = "center",
				rounded = "$radius.full",
				shrink = 0,
				bg = function(): string
					if filter() == id then
						return "$zeron.menu.selected"
					end
					return if hovered:get() then "$hover" else "#00000000"
				end,
				hover = {},
				active = { bg = "$pressed" },
				on_hover = function(inside: boolean)
					hovered:set(inside)
				end,
				on_click = function()
					set_filter(id)
				end,
				if h then ui.div { w = 7, h = 7, rounded = "$radius.full", bg = hue.paint(h), shrink = 0 } else nil,
				ui.text {
					name,
					text_size = "$font.size.xs",
					weight = "$font.weight.medium",
					color = function(): string
						return if filter() == id then "$text.primary" else "$text.secondary"
					end,
				},
				-- (Its project, quietly, where another has the same name.)
				if suffix
					then ui.show(function(): boolean
						return suffix() ~= nil
					end, ui.text {
						function(): string
							return suffix() or ""
						end,
						class = "orch-filter-project",
						text_size = "$font.size.xs",
						color = "$text.tertiary",
					})
					else nil,
			}
		end
		-- Whether another orchestrator shown has this one's name.
		local function shared(o: Orchestrator): boolean
			local name = string.lower(o.name)
			for _, x in orchestrators() do
				if x.id ~= o.id and (x.kind == nil or x.kind == "standard") and not x.archived and string.lower(x.name) == name then
					return true
				end
			end
			return false
		end
		return ui.row {
			class = "orch-board-filter",
			w_full = true,
			gap = "$space.1",
			flex_wrap = true,
			chip(nil, function(): string
				return locale.s("orch.board.all")
			end),
			ui.each(ui.computed(function(): { Orchestrator }
				local out: { Orchestrator } = {}
				for _, o in orchestrators() do
					if (o.kind == nil or o.kind == "standard") and not o.archived then
						table.insert(out, o)
					end
				end
				return out
			end), function(o: Orchestrator, _i: any, row: any): Node
				local function now_o(): Orchestrator
					return if row then row:get() else o
				end
				return chip(o.id, function(): string
					return now_o().name
				end, o.hue, function(): string?
					local x = now_o()
					return if shared(x) then project_of(x.id) else nil
				end)
			end, { key = function(o: Orchestrator): string
				return o.id
			end }),
		}
	end

	-- Side by side where there's room; in rows of three where there's
	-- less; one under another where there's little.
	local width = ui.use_state(0)
	local function need(n: number): number
		return n * COLUMN_MIN + (n - 1) * space("space.2", 8)
	end
	-- The columns that stand now (Inbox and Blocked only while they hold a
	-- card): how many the width must hold.
	local function standing(): number
		local n = 0
		local lists = shown:get()
		for _, c in columns do
			-- (Not Blocked standing for a carried card: the layout never
			-- changes under a drag.)
			if not ONLY_HELD[c.key] or #(lists[c.key] or {}) > 0 then
				n += 1
			end
		end
		return n
	end
	local mode = ui.computed(function(): string
		local w = width:get()
		if w <= 0 or w >= need(standing()) then
			return "side"
		end
		return if w >= need(PER_ROW) then "rows" else "stacked"
	end)
	local function lanes(k: string): Node
		if k == "stacked" then
			local cols: { Node } = {}
			for _, c in columns do
				table.insert(cols, column(c, k))
			end
			return ui.column { class = "orch-board-stacked", w_full = true, gap = "$space.3", table.unpack(cols) }
		end
		-- (In rows, the last one's columns as wide as the rest: room kept
		-- where it has fewer.)
		local per = if k == "rows" then PER_ROW else #columns
		local rows: { Node } = {}
		for from = 1, #columns, per do
			local cells: { [any]: any } = {
				class = "orch-board-side",
				w_full = true,
				gap = "$space.2",
				items = "stretch",
			}
			for i = from, from + per - 1 do
				local c = columns[i]
				table.insert(cells, if c then column(c, k) else ui.div { grow = 1, basis = 0, min_w = COLUMN_MIN })
			end
			table.insert(rows, ui.row(cells))
		end
		if #rows == 1 then
			return rows[1]
		end
		return ui.column { class = "orch-board-rows", w_full = true, gap = "$space.2", table.unpack(rows) }
	end
	return ui.column {
		id = props.id or "orch-board",
		class = "orch-board",
		label = locale.t("orch.board.label"),
		w_full = true,
		gap = "$space.3",
		on_resize = function(size: { w: number, h: number })
			if math.abs(size.w - width:peek()) >= 1 then
				width:set(size.w)
			end
		end,
		-- The whole board takes the carried card: where it lands is the
		-- pointer's, column and place, whatever is under it.
		drop_when = accepts,
		on_drag_over = function(_payload: any, e: any)
			set_target(target_at(e.x, e.y))
		end,
		on_drag_leave = function()
			set_target(nil)
		end,
		on_drop = function(_payload: any, e: any)
			local c = carry:peek()
			if not c then
				return
			end
			local t = target_at(e.x, e.y) or target:peek()
			if not t then
				return finish("idle")
			end
			land(c, t)
		end,
		if props.show_filter then chips() else nil,
		ui.presence(function(): string
			return if empty() then "empty" else mode:get()
		end, function(k: string): Node
			if k == "empty" then
				return ui.row {
					class = "orch-board-empty",
					w_full = true,
					h = 28,
					px = "$space.2",
					gap = "$space.2",
					items = "center",
					ui.div { color = "$text.tertiary", Icon { name = "checklist", size = 14 } },
					ui.text { locale.t("orch.board.empty"), text_size = "$font.size.sm", color = "$text.tertiary" },
				}
			end
			return lanes(k)
		end, { w_full = true }),
	}
end)

return OrchBoard
