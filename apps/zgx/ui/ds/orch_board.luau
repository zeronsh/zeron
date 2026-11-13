--!strict
-- The board: where every piece of an orchestrator's work stands, in
-- columns Now, Next, Blocked, Review and Done (today), with Inbox before
-- them where loose items are captured (`inbox = true`). Shared by the
-- orchestrator's page (filtered to it, `compact`) and the Control room
-- (all of them, its filter on top).
--
-- The columns share the board's width evenly, each a lane of its own, its
-- name, glyph and count on one line across them all; an empty one keeps
-- its width, quiet. Narrower than the columns need, they stand in rows of
-- three, as wide as each other still; narrower still (a page's side pane),
-- one under another. A board with nothing on it (or nothing of the
-- orchestrator chosen) is one quiet line.
--
-- A card: how its lead thread stands (Zeron's status glyph, held 300 ms at
-- least), its title, how far along it is (its threads' todo as segmented
-- cells, else its own progress), and a quiet line of what else it has:
-- whose it is (its orchestrator's hue dot and name, and the project it's in
-- where that's named otherwise; a loose item, its project), its threads as
-- dots when it has several, its pull request, and, made from a dump's part,
-- a mark that shows that dump (`on_source`). (On an orchestrator's own
-- page, whose is implied: none says.) Its priority stands at its title's end when it's high or
-- urgent. A card in Review or Done carries its outcome, two lines of it,
-- all of it in a hover card. Each card is a plugin slot
-- (`orchestration.item`, `provide = { item = id }`).
--
-- Cards are dragged between columns and within one (`on_move(id, state,
-- order)`, the order between its new neighbours', within its priority):
-- pressed and moved a few points, from anywhere on it and however fast, it
-- lifts and follows the pointer, the cards where it would land making room;
-- let go over the board it lands there, elsewhere (or Escape) it goes back.
-- A click opens its thread; a secondary click sets the priority or opens a
-- thread.
--
--   OrchBoard {
--     items = function() return state.items end,
--     orchestrators = function() return state.orchestrators end,
--     todo_of = fn(chat) -> { done, total, current? }?,
--     thread_of = fn(chat) -> { title, status?, agent? }?,
--     filter = state_or_fn,      -- an orchestrator's id: its items alone
--     show_filter = true,        -- its chips over the board (the Control room)
--     inbox = true,              -- an Inbox column first
--     now = fn() -> number,      -- Unix seconds: what "today" is
--     on_move = fn(id, state, order), on_priority = fn(id, priority),
--     on_open_thread = fn(chat), on_link = fn(link),
--     on_source = fn({ dump_id, route_id }),  -- a card's dump, shown
--     compact = true,            -- denser cards (a page's side pane)
--     project_of = fn(chat) -> string?,  -- the name of the project a chat works in
--     context = false,           -- cards say nothing of whose they are (an
--                                -- orchestrator's own page, where it's
--                                -- implied: the default when `filter` is
--                                -- given without `show_filter`)
--     progress = fn(chat) -> Node?,  -- a line of its lead thread's progress
--                                -- (the app's ds/orch_progress strip)
--   }

local Icon = require("@self/icon")
local ContextMenu = require("@self/context_menu")
local Markdown = require("@self/markdown")
local HoverCard = require("@self/hover_card")
local badge = require("@self/badge")
local status = require("@self/status")
local control = require("@self/control")
local locale = require("@self/locale")
local material = require("@self/material")
local motion = require("@self/motion")
local hue = require("@self/orch_hue")
local Cells = require("@self/orch_cells")
local hold = require("@self/orch_hold")

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
}
export type Orchestrator = { id: string, kind: string?, name: string, hue: number, archived: boolean? }
export type Todo = { done: number, total: number, current: string? }
export type Thread = { title: string, status: string?, agent: string? }
export type Props = {
	items: control.Live<{ Item }>,
	orchestrators: control.Live<{ Orchestrator }>?,
	todo_of: ((chat: string) -> Todo?)?,
	thread_of: ((chat: string) -> Thread?)?,
	progress: ((chat: string) -> Node?)?,
	filter: control.Live<string?>?,
	show_filter: boolean?,
	inbox: boolean?,
	now: (() -> number)?,
	on_move: ((id: string, state: string, order: number) -> ())?,
	on_priority: ((id: string, priority: string) -> ())?,
	on_open_thread: ((chat: string) -> ())?,
	on_link: ((link: Link) -> ())?,
	on_source: ((source: { dump_id: string?, route_id: string? }) -> ())?,
	compact: boolean?,
	project_of: ((chat: string) -> string?)?,
	context: boolean?,
	id: string?,
}

-- The columns, in order: each its key, the state its cards are in, and its
-- name's key.
type Column = { key: string, state: string, name: string }
local INBOX: Column = { key = "inbox", state = "inbox", name = "orch.board.column.inbox" }
local COLUMNS: { Column } = {
	{ key = "now", state = "active", name = "orch.board.column.now" },
	{ key = "next", state = "planned", name = "orch.board.column.next" },
	{ key = "blocked", state = "blocked", name = "orch.board.column.blocked" },
	{ key = "review", state = "review", name = "orch.board.column.review" },
	{ key = "done", state = "done", name = "orch.board.column.done" },
}
local RANK: { [string]: number } = { urgent = 0, high = 1, normal = 2, low = 3 }
local PRIORITIES = { "urgent", "high", "normal", "low" }

-- The least a column takes side by side: narrower than that for them all,
-- they stand in rows of `PER_ROW`; narrower than that for those, one under
-- another.
local COLUMN_MIN = 160
local PER_ROW = 3
-- Each column's glyph at its head: how its cards stand.
local COLUMN_GLYPH: { [string]: Icon.Name } = {
	inbox = "inbox",
	now = "status-working",
	next = "clock-circle",
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
local CARD_RADIUS = "$radius.lg"
local DRAG_TAG = "orch-item"
-- An outcome longer than about two lines' words: all of it in a hover card.
local OUTCOME_FOLD = 64

local function space(key: string, fallback: number): number
	return (ui.theme.get(key) :: number?) or fallback
end

-- Local midnight of `t`.
local function day_start(t: number): number
	local d = os.date("*t", t) :: any
	return t - (d.hour * 3600 + d.min * 60 + d.sec)
end

-- Whether `item` stands in `col` today.
local function in_column(item: Item, col: Column, today: number): boolean
	if item.state ~= col.state then
		return false
	end
	if col.state == "done" then
		local at = item.done or item.updated or 0
		return at >= today
	end
	return true
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

-- How its lead thread stands (the one that needs you first, then one that
-- failed, then one working), else how the card's column says it does.
local LEAD: { [string]: number } = { input = 0, approval = 0, failed = 1, limited = 1, working = 2, connecting = 2, waiting = 2, done = 3 }
local function lead_status(item: Item, thread_of: ((string) -> Thread?)?): string
	local best, rank = nil :: string?, math.huge
	for _, chat in item.threads or {} do
		local t = if thread_of then thread_of(chat) else nil
		local st = t and t.status
		if st and LEAD[st] and LEAD[st] < rank then
			best, rank = st, LEAD[st]
		end
	end
	if item.state == "done" then
		return "done"
	elseif best then
		return best
	elseif item.state == "review" then
		return "input"
	elseif item.state == "blocked" then
		return "approval"
	end
	return "none"
end

-- An outcome's words as one quiet run (its lines folded): the markdown's
-- marks left out.
local function plain(md: string): string
	local s = string.gsub(md, "%*%*", "")
	s = string.gsub(s, "`", "")
	s = string.gsub(s, "%[([^%]]*)%]%b()", "%1")
	s = string.gsub(s, "\n+", " ")
	return s
end

-- A thread's state as a dot's kind (idle: none).
local DOT_KINDS: { [string]: boolean } = { working = true, connecting = true, waiting = true, input = true, approval = true, done = true, failed = true, limited = true }

local function thread_dot(chat: string, thread_of: ((string) -> Thread?)?, on_open: ((string) -> ())?): Node
	local function info(): Thread?
		return if thread_of then thread_of(chat) else nil
	end
	local t = info()
	return ui.button {
		label = if t then t.title else chat,
		tooltip = if t then t.title else nil,
		tooltip_delay = 300,
		w = 14,
		h = 14,
		items = "center",
		justify = "center",
		rounded = "$radius.full",
		shrink = 0,
		hover = {},
		active = {},
		on_click = if on_open
			then function()
				on_open(chat)
			end
			else nil,
		ui.switch(function(): string
			local now = info()
			local k = now and now.status
			return if k and DOT_KINDS[k] then k else "idle"
		end, (function(): { [string]: Node }
			local cases: { [string]: Node } = {
				idle = ui.div { w = 7, h = 7, rounded = "$radius.full", border = 1, border_color = "$text.tertiary" },
			}
			for k in DOT_KINDS do
				cases[k] = status.StatusDot { kind = k :: any, size = 7 }
			end
			return cases
		end)() :: any),
	}
end

-- Its priority, where it says something: urgent and high (low, quietly).
local function priority_mark(p: () -> string): Node
	local function mark(key: string, icon: Icon.Name, ink: string): Node
		return ui.div {
			shrink = 0,
			color = ink,
			tooltip = locale.s(`orch.board.priority.{key}`),
			label = locale.t(`orch.board.priority.{key}`),
			role = "img",
			Icon { name = icon, size = 12 },
		}
	end
	return ui.switch(function(): string
		local k = p()
		return if k == "urgent" or k == "high" or k == "low" then k else "normal"
	end, {
		urgent = mark("urgent", "flag", "$zeron.tone.danger.ink"),
		high = mark("high", "arrow-up", "$zeron.tone.warning.ink"),
		low = mark("low", "arrow-down", "$text.tertiary"),
		normal = ui.div {},
	})
end

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

	local columns: { Column } = if props.inbox then { INBOX, table.unpack(COLUMNS) } else table.clone(COLUMNS)

	-- A card just dropped, where it went, until the items say so (the
	-- engine answers a beat later): it stays where it was let go.
	type Pending = { id: string, state: string, order: number }
	local pending = ui.use_state(nil :: Pending?)

	-- The items shown, by column, each sorted (filtered first).
	local shown = ui.computed(function(): { [string]: { Item } }
		local list = (control.value(props.items) :: { Item }?) or {}
		local only = filter()
		local moved = pending:get()
		local today = day_start(now())
		local out: { [string]: { Item } } = {}
		for _, c in columns do
			out[c.key] = {}
		end
		for _, it in list do
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
					if in_column(it, c, today) then
						table.insert(out[c.key], it)
						break
					end
				end
			end
		end
		for k, l in out do
			out[k] = sorted(l)
		end
		return out
	end)
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

	-- A drag. `carry`: the card carried (its column, size and rank); `target`:
	-- where it would land (its column, its place among the others there from
	-- 1, and where that is in the column, from its top); `phase`: "idle",
	-- "carrying", or "landed" (let go on the board: the cards there take their
	-- new places at once, not sliding back).
	type Carry = { id: string, col: string, h: number, w: number, rank: number }
	type Target = { col: string, index: number, y: number }
	local carry = ui.use_state(nil :: Carry?)
	local target = ui.use_state(nil :: Target?)
	local phase = ui.use_state("idle")
	local lane_refs: { [string]: any } = {}
	local body_refs: { [string]: any } = {}
	local slot_refs: { [string]: any } = {}
	local function set_target(t: Target?)
		local was = target:peek()
		if was and t and was.col == t.col and was.index == t.index and math.abs(was.y - t.y) < 0.5 then
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
		local list = others(key, c)
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
			at = 0
		end
		return { col = key, index = index, y = at }
	end

	-- Its place where it was picked up (so nothing moves as it lifts).
	local function home(c: Carry): Target
		local index = 1
		for _, it in shown:peek()[c.col] or {} do
			if it.id == c.id then
				break
			end
			index += 1
		end
		return { col = c.col, index = index, y = top_in(c.col, c.id, c) or 0 }
	end

	local function finish(how: string)
		phase:set(how)
		carry:set(nil)
		target:set(nil)
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
		if not col or not props.on_move or (t.col == c.col and t.index == back.index) then
			return finish("idle")
		end
		local list = others(t.col, c)
		-- (Its neighbours of its own priority: the board sorts by priority
		-- first, so another's order says nothing of where it stands.)
		local prev, nxt = list[t.index - 1], list[t.index]
		if prev and (RANK[prev.priority] or 2) ~= c.rank then
			prev = nil
		end
		if nxt and (RANK[nxt.priority] or 2) ~= c.rank then
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
		local id, state = c.id, col.state
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

	-- A card's menu: its priority, its threads.
	local function menu(item: () -> Item): { any }?
		local it = item()
		local entries: { any } = {}
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
		local threads = it.threads or {}
		if props.on_open_thread and #threads > 0 then
			if #entries > 0 then
				table.insert(entries, { separator = true })
			end
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
		return if #entries > 0 then entries else nil
	end

	local line = (ui.theme.get("font.line.sm") :: number?) or 16

	-- Whose it is, quietly: its orchestrator's hue and name, and its
	-- project (a folder, as the Map names one).
	local function owner(item: () -> Item): Node
		local function orch(): Orchestrator?
			local o = context_of(item())
			return o
		end
		local function project(): string?
			local _, p = context_of(item())
			return p
		end
		return ui.show(function(): boolean
			return orch() ~= nil or project() ~= nil
		end, ui.row {
			class = "orch-card-owner",
			min_w = 0,
			shrink = 1,
			gap = "$space.2",
			items = "center",
			ui.show(function(): boolean
				return orch() ~= nil
			end, ui.row {
				min_w = 0,
				shrink = 1,
				gap = "$zeron.space.tight",
				items = "center",
				hue.Dot {
					hue = function(): number?
						local o = orch()
						return o and o.hue
					end,
					size = 6,
				},
				ui.text {
					function(): string
						local o = orch()
						return if o then o.name else ""
					end,
					class = "orch-card-orchestrator",
					truncate = true,
					min_w = 0,
					text_size = "$font.size.xs",
					color = "$text.tertiary",
				},
			}),
			ui.show(function(): boolean
				return project() ~= nil
			end, ui.row {
				min_w = 0,
				shrink = 1,
				gap = "$zeron.space.tight",
				items = "center",
				ui.div { color = "$text.tertiary", shrink = 0, Icon { name = "folder", size = 11 } },
				ui.text {
					function(): string
						return project() or ""
					end,
					class = "orch-card-project",
					truncate = true,
					min_w = 0,
					text_size = "$font.size.xs",
					color = "$text.tertiary",
				},
			}),
		})
	end

	-- What a card shows lifted and carried: its status and title, at its
	-- size, risen off the board.
	local function lifted(it: Item, w: number, h: number): Node
		local lead = lead_status(it, props.thread_of)
		return ui.row {
			class = "orch-card-lifted",
			w = w,
			min_h = ui.raw(h, "the card's own height"),
			p = if compact then "$space.2" else ui.raw(10, "a card's even inset"),
			gap = "$zeron.space.tight",
			items = "start",
			rounded = CARD_RADIUS,
			bg = "$zeron.menu.solid",
			border = 1,
			border_color = "$border",
			shadow = "$shadow.md",
			scale = motion.TAB_LIFT,
			if lead ~= "none"
				then ui.div { h = line, items = "center", shrink = 0, status.StatusIcon { kind = lead :: any, size = 14 } }
				else nil,
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
		local slot = ui.ref()
		slot_refs[id] = slot
		ui.on_cleanup(function()
			if slot_refs[id] == slot then
				slot_refs[id] = nil
			end
		end)
		local pad = if compact then "$space.2" else ui.raw(10, "a card's even inset")
		local lead = hold.value(function(): string
			return lead_status(item(), props.thread_of)
		end)
		local function outcome(): string?
			local it = item()
			local o = it.outcome
			return if (it.state == "done" or it.state == "review") and o ~= nil and o ~= "" then o else nil
		end
		local function from_dump(): boolean
			local src = item().source
			return src ~= nil and src.dump_id ~= nil and src.dump_id ~= ""
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

		-- Its title: two lines at most, how its lead thread stands before it.
		local title_text = ui.text {
			function(): string
				return item().title
			end,
			wrap = true,
			lines = 2,
			truncate = true,
			grow = 1,
			min_w = 0,
			text_size = "$font.size.sm",
			line_height = "$font.line.sm",
			weight = "$font.weight.medium",
			color = function(): string
				return if item().state == "done" then "$text.secondary" else "$text.primary"
			end,
		}
		local title = ui.row {
			w_full = true,
			min_w = 0,
			gap = "$zeron.space.tight",
			items = "start",
			ui.show(function(): boolean
				return lead() ~= "none"
			end, ui.div {
				class = "orch-card-status",
				h = line,
				items = "center",
				shrink = 0,
				status.StatusIcon {
					kind = function(): any
						local k = lead()
						return if k == "none" then "done" else k
					end,
					size = 14,
				},
			}),
			title_text,
			ui.div {
				h = line,
				items = "center",
				shrink = 0,
				priority_mark(function(): string
					return item().priority
				end),
			},
		}
		-- Its quiet line: whose it is, its threads, its pull request.
		local function has_meta(): boolean
			local o, p = context_of(item())
			return o ~= nil or p ~= nil or #(item().threads or {}) > 1 or #prs() > 0 or from_dump()
		end
		local meta = ui.row {
			class = "orch-card-meta",
			w_full = true,
			min_w = 0,
			h = line,
			gap = "$space.2",
			items = "center",
			owner(item),
			ui.row {
				shrink = 0,
				gap = "$space.1",
				items = "center",
				-- Made from a dump's part: the dump, shown where it was said.
				ui.show(from_dump, ui.button {
					class = "orch-card-source",
					label = locale.t("orch.board.from_dump"),
					tooltip = locale.s("orch.board.from_dump"),
					tooltip_delay = 300,
					w = 16,
					h = 16,
					items = "center",
					justify = "center",
					rounded = "$radius.sm",
					shrink = 0,
					color = "$text.tertiary",
					bg = "#00000000",
					hover = { bg = "$hover", color = "$text.secondary" },
					active = { bg = "$pressed" },
					on_click = if props.on_source
						then function()
							local src = item().source
							if src then
								(props.on_source :: any)(src)
							end
						end
						else nil,
					Icon { name = "pen-new-square", size = 12 },
				}),
				ui.each(ui.computed(function(): { string }
					-- (One thread: its status leads the title already.)
					local threads = item().threads or {}
					return if #threads > 1 then threads else {}
				end), function(chat: string): Node
					return thread_dot(chat, props.thread_of, props.on_open_thread)
				end, { key = function(chat: string): string
					return chat
				end }),
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
			},
		}
		-- How far along: its threads' steps as cells, else its own share in
		-- ten; and the figure.
		local function steps(): (number?, number?)
			local d, t = todo_of_item(item(), props.todo_of)
			if d then
				return d, t
			end
			local p = item().progress
			if p then
				return math.floor(math.clamp(p, 0, 1) * 10 + 0.5), 10
			end
			return nil, nil
		end
		local progress_bar = ui.show(function(): boolean
			return steps() ~= nil and item().state ~= "done"
		end, ui.row {
			class = "orch-card-progress",
			w_full = true,
			gap = "$space.2",
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
					return lead() == "working"
				end,
				cell = 4,
				h = 8,
				max = 24,
			},
			ui.text {
				function(): string
					local d, t = todo_of_item(item(), props.todo_of)
					if d and t then
						return `{d}/{t}`
					end
					local p = item().progress
					return if p then `{math.floor(math.clamp(p, 0, 1) * 100 + 0.5)}%` else ""
				end,
				shrink = 0,
				tabular = true,
				text_size = "$font.size.xs",
				color = "$text.tertiary",
			},
		})
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
		local body = ui.column {
			w_full = true,
			min_w = 0,
			p = pad,
			ui.column {
				w_full = true,
				min_w = 0,
				gap = if compact then "$space.1" else "$zeron.space.snug",
				title,
				progress_bar,
				-- How its lead thread's work is going, while it's under way.
				if props.progress
					then ui.presence(function(): string
						local it = item()
						return if it.state ~= "done" then (it.threads or {})[1] or "" else ""
					end, function(chat: string): Node
						return if chat == "" then ui.div {} else (props.progress :: any)(chat) or ui.div {}
					end, { w_full = true, min_w = 0 })
					else nil,
				outcome_text,
				ui.show(has_meta, meta),
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
			bg = function(): string
				if hovered:get() and not carry:get() then
					return "$hover"
				end
				return material.pick("$zeron.settings.note", "$zeron.settings.card")()
			end,
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
			on_click = function()
				local threads = item().threads or {}
				if props.on_open_thread and #threads > 0 then
					(props.on_open_thread :: any)(threads[1])
				end
			end,
			on_drag = if props.on_move
				then function(): any
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
					return { tag = DRAG_TAG, id = id }
				end
				else nil,
			drag_preview = function(): Node
				local c = carry:peek()
				return lifted(item(), if c then c.w else COLUMN_MIN, if c then c.h else 0)
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

	local function column(col: Column, mode: string): Node
		local stacked = mode == "stacked"
		local function list(): { Item }
			return shown:get()[col.key] or {}
		end
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
			-- until a card is carried: then it's somewhere to put one.)
			hidden = if stacked and compact
				then function(): boolean
					return #list() == 0 and carry:get() == nil
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
						return if #list() > 0 then COLUMN_INK[col.key] or "$text.tertiary" else "$text.tertiary"
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
						return if #list() > 0 then "$text.primary" else "$text.tertiary"
					end,
				},
				ui.text {
					function(): string
						local n = #list()
						return if n > 0 then tostring(n) else ""
					end,
					class = "orch-column-count",
					tabular = true,
					text_size = "$font.size.xs",
					color = "$text.tertiary",
				},
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
				ui.each(ui.computed(list), function(it: Item, i: any, row: any): Node
					return card(it, i, row, col)
				end, { key = function(it: Item): string
					return it.id
				end, w_full = true }),
			},
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
	local mode = ui.computed(function(): string
		local w = width:get()
		if w <= 0 or w >= need(#columns) then
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
