--!strict
-- The board: where every piece of an orchestrator's work stands, in
-- columns Now, Next, Blocked, Review and Done (today), with Inbox before
-- them where loose items are captured (`inbox = true`). Shared by the
-- orchestrator's page (filtered to it, `compact`) and the Control room
-- (all of them, its filter on top).
--
-- A card: its orchestrator's hue as a stripe at its leading edge, its
-- title, how far along it is (its own progress, else its threads' todo
-- summed), its threads as dots in their state, its pull request, and its
-- priority when that is high or urgent. A card in Review or Done carries
-- its outcome, two lines of it until it's opened: the answer or result,
-- where it is read. Each card is a plugin slot (`orchestration.item`,
-- `provide = { item = id }`).
--
-- Cards are dragged between columns and within one (`on_move(id, state,
-- order)`, the order between its new neighbours'); a secondary click sets
-- the priority or opens a thread. Narrower than its columns need, the
-- columns stack, each a section of its own.
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
--     compact = true,            -- denser cards (a page's side pane)
--   }

local Icon = require("@self/icon")
local ContextMenu = require("@self/context_menu")
local Markdown = require("@self/markdown")
local badge = require("@self/badge")
local status = require("@self/status")
local control = require("@self/control")
local locale = require("@self/locale")
local motion = require("@self/motion")
local material = require("@self/material")
local hue = require("@self/orch_hue")

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
}
export type Orchestrator = { id: string, kind: string?, name: string, hue: number, archived: boolean? }
export type Todo = { done: number, total: number, current: string? }
export type Thread = { title: string, status: string?, agent: string? }
export type Props = {
	items: control.Live<{ Item }>,
	orchestrators: control.Live<{ Orchestrator }>?,
	todo_of: ((chat: string) -> Todo?)?,
	thread_of: ((chat: string) -> Thread?)?,
	filter: control.Live<string?>?,
	show_filter: boolean?,
	inbox: boolean?,
	now: (() -> number)?,
	on_move: ((id: string, state: string, order: number) -> ())?,
	on_priority: ((id: string, priority: string) -> ())?,
	on_open_thread: ((chat: string) -> ())?,
	on_link: ((link: Link) -> ())?,
	compact: boolean?,
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

-- The least a column takes side by side; under that for them all, they stack.
local COLUMN_MIN = 188
-- A card's corner, and the stripe's room inside it.
local CARD_RADIUS = "$radius.lg"
local DRAG_TAG = "orch-item"

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

-- An item's progress, 0–1: its own, else its threads' todo summed; nil
-- when nothing says.
local function progress_of(item: Item, todo_of: ((string) -> Todo?)?): number?
	if item.progress then
		return math.clamp(item.progress, 0, 1)
	end
	if not todo_of then
		return nil
	end
	local done, total = 0, 0
	for _, chat in item.threads or {} do
		local t = todo_of(chat)
		if t and t.total and t.total > 0 then
			done += t.done or 0
			total += t.total
		end
	end
	return if total > 0 then math.clamp(done / total, 0, 1) else nil
end

-- An outcome's words as one quiet line (its two lines, folded): the
-- markdown's marks left out.
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

-- A thin bar of how far along, in the hue, easing as it moves (still,
-- with motion reduced: ds/motion's tween).
local function bar(value: () -> number?, ink: () -> string): Node
	local track_w = ui.use_state(0)
	return ui.div {
		w_full = true,
		h = 3,
		relative = true,
		rounded = "$radius.full",
		bg = "$zeron.slider.track",
		overflow = "hidden",
		on_resize = function(size: { w: number, h: number })
			if math.abs(size.w - track_w:peek()) >= 0.5 then
				track_w:set(size.w)
			end
		end,
		ui.div {
			absolute = true,
			left = 0,
			top = 0,
			h = 3,
			rounded = "$radius.full",
			bg = ink,
			w = motion.tween(ui.computed(function(): number
				return math.clamp(value() or 0, 0, 1) * track_w:get()
			end), { duration = 320, ease = "out(3)" }),
		},
	}
end

local OrchBoard = ui.component(function(props: Props): Node
	local now = props.now or os.time
	local compact = props.compact == true
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
	local function hue_of(id: string?): number?
		if not id then
			return nil
		end
		for _, o in orchestrators() do
			if o.id == id then
				return o.hue
			end
		end
		return nil
	end
	local function ink_of(id: string?): string
		local h = hue_of(id)
		return if h then hue.paint(h) else "$text.tertiary"
	end

	local columns: { Column } = if props.inbox then { INBOX, table.unpack(COLUMNS) } else table.clone(COLUMNS)
	-- The items shown, by column, each sorted (filtered first).
	local shown = ui.computed(function(): { [string]: { Item } }
		local list = (control.value(props.items) :: { Item }?) or {}
		local only = filter()
		local today = day_start(now())
		local out: { [string]: { Item } } = {}
		for _, c in columns do
			out[c.key] = {}
		end
		for _, it in list do
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

	-- A drag under way: the card's id; where it would land.
	type Drop = { col: string, before: string?, after: string?, at_end: boolean? }
	local dragging = ui.use_state(nil :: string?)
	local drop = ui.use_state(nil :: Drop?)
	local function set_drop(d: Drop?)
		local was = drop:peek()
		if was and d and was.col == d.col and was.before == d.before and was.after == d.after and was.at_end == d.at_end then
			return
		end
		drop:set(d)
	end
	-- Where `id` lands at `d`: its state and the order between its new
	-- neighbours there.
	local function land(id: string, d: Drop)
		local col: Column? = nil
		for _, c in columns do
			if c.key == d.col then
				col = c
			end
		end
		if not col or not props.on_move then
			return
		end
		local list: { Item } = {}
		for _, it in shown:peek()[d.col] or {} do
			if it.id ~= id then
				table.insert(list, it)
			end
		end
		local order: number
		if #list == 0 then
			order = 0
		elseif d.at_end then
			order = (list[#list].order or 0) + 1
		else
			local at = 0
			for i, it in list do
				if it.id == d.before or it.id == d.after then
					at = i
				end
			end
			if at == 0 then
				order = (list[#list].order or 0) + 1
			elseif d.before then
				local nxt, prev = list[at], list[at - 1]
				order = if prev then ((prev.order or 0) + (nxt.order or 0)) / 2 else (nxt.order or 0) - 1
			else
				local prev, nxt = list[at], list[at + 1]
				order = if nxt then ((prev.order or 0) + (nxt.order or 0)) / 2 else (prev.order or 0) + 1
			end
		end
		(props.on_move :: any)(id, col.state, order)
	end
	local function accepts(payload: any): boolean
		return type(payload) == "table" and payload.tag == DRAG_TAG
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

	-- One card.
	local function card(first: Item, _i: any, row: any, col: Column): Node
		local function item(): Item
			return if row then row:get() else first
		end
		local id = first.id
		local hovered = ui.use_state(false)
		local open = ui.use_state(false)
		local pad = if compact then "$space.2" else ui.raw(10, "a card's even inset, between the steps")
		local function ink(): string
			return ink_of(item().orchestrator_id)
		end
		local function progress(): number?
			return progress_of(item(), props.todo_of)
		end
		local function has_outcome(): boolean
			local o = item().outcome
			return (item().state == "done" or item().state == "review") and o ~= nil and o ~= ""
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
		local line = (ui.theme.get("font.line.sm") :: number?) or 16
		local body = ui.column {
			w_full = true,
			min_w = 0,
			gap = if compact then "$space.1" else "$zeron.space.snug",
			-- Its title: two lines at most.
			ui.text {
				function(): string
					return item().title
				end,
				wrap = true,
				w_full = true,
				max_h = line * 2,
				overflow = "hidden",
				text_size = "$font.size.sm",
				line_height = "$font.line.sm",
				weight = "$font.weight.medium",
				color = function(): string
					return if item().state == "done" then "$text.secondary" else "$text.primary"
				end,
			},
			ui.show(function(): boolean
				return progress() ~= nil and item().state ~= "done"
			end, bar(progress, ink)),
			-- Its marks: priority, threads, pull request.
			ui.row {
				w_full = true,
				gap = "$space.1",
				items = "center",
				priority_mark(function(): string
					return item().priority
				end),
				ui.each(ui.computed(function(): { string }
					return item().threads or {}
				end), function(chat: string): Node
					return thread_dot(chat, props.thread_of, props.on_open_thread)
				end, { key = function(chat: string): string
					return chat
				end }),
				ui.div { grow = 1 },
				ui.each(ui.computed(prs), function(l: Link): Node
					local n = tonumber(string.match(l.label, "%d+"))
					return ui.button {
						label = l.label,
						shrink = 0,
						rounded = "$radius.full",
						hover = {},
						active = {},
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
				ui.text {
					function(): string
						local p = progress()
						return if p and item().state ~= "done" then `{math.floor(p * 100 + 0.5)}%` else ""
					end,
					shrink = 0,
					tabular = true,
					text_size = "$font.size.xs",
					color = "$text.tertiary",
				},
			},
			-- Its outcome: two lines, or all of it opened.
			ui.show(
				has_outcome,
				ui.button {
					class = "orch-outcome",
					label = locale.t("orch.board.outcome"),
					w_full = true,
					items = "start",
					hover = {},
					active = {},
					on_click = function()
						open:set(not open:peek())
					end,
					ui.row {
						w_full = true,
						gap = "$space.2",
						items = "start",
						ui.div { w = 2, shrink = 0, h_full = true, rounded = "$radius.full", bg = "$border.strong" },
						ui.presence(function(): string
							return if open:get() then "open" else "folded"
						end, function(k: string): Node
							if k == "open" then
								return ui.column {
									class = "orch-outcome-full",
									grow = 1,
									min_w = 0,
									text_size = "$font.size.sm",
									color = "$text.secondary",
									Markdown {
										text = function(): string
											return item().outcome or ""
										end,
									},
								}
							end
							return ui.text {
								function(): string
									return plain(item().outcome or "")
								end,
								class = "orch-outcome-folded",
								grow = 1,
								min_w = 0,
								wrap = true,
								max_h = line * 2,
								overflow = "hidden",
								text_size = "$font.size.sm",
								line_height = "$font.line.sm",
								color = "$text.secondary",
							}
						end, { grow = 1, min_w = 0 }),
					},
				}
			),
			-- Where plugins decorate it (the github plugin: its checks).
			ui.div { hook = "orchestration.item", class = "orch-item-slot", provide = { item = id }, w_full = true },
		}
		local function line_at(where: string): Node
			return ui.show(function(): boolean
				local d = drop:get()
				return d ~= nil and d.col == col.key and ((where == "before" and d.before == id) or (where == "after" and d.after == id))
			end, ui.div {
				absolute = true,
				left = 0,
				right = 0,
				top = if where == "before" then ui.raw(-3, "between two cards") else nil,
				bottom = if where == "after" then ui.raw(-3, "between two cards") else nil,
				h = 2,
				rounded = "$radius.full",
				bg = "$accent",
				pointer_events = "none",
			})
		end
		local box = ui.button {
			class = "orch-card",
			label = function(): string
				return item().title
			end,
			w_full = true,
			relative = true,
			rounded = CARD_RADIUS,
			bg = function(): string
				if hovered:get() then
					return "$hover"
				end
				return material.pick("$zeron.settings.note", "$zeron.settings.card")()
			end,
			hover = {},
			active = { bg = "$pressed" },
			opacity = function(): number
				return if dragging:get() == id then 0.4 else 1
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
					dragging:set(id)
					return { tag = DRAG_TAG, id = id }
				end
				else nil,
			drag_preview = function(): Node
				return ui.column {
					w = 220,
					p = "$space.2",
					rounded = CARD_RADIUS,
					bg = "$zeron.settings.card",
					border = 1,
					border_color = "$border",
					ui.text { item().title, truncate = true, text_size = "$font.size.sm", weight = "$font.weight.medium", color = "$text.primary" },
				}
			end,
			on_drag_end = function()
				dragging:set(nil)
				drop:set(nil)
			end,
			drop_when = accepts,
			on_drag_over = function(payload: any, e: any)
				if payload.id == id then
					set_drop(nil)
					return
				end
				local h = (e and e.h) or 1
				if e and e.local_y and e.local_y < h / 2 then
					set_drop({ col = col.key, before = id })
				else
					set_drop({ col = col.key, after = id })
				end
			end,
			on_drop = function(payload: any, e: any)
				local d = drop:peek()
				drop:set(nil)
				dragging:set(nil)
				if payload.id == id then
					return
				end
				if not d or d.col ~= col.key then
					local h = (e and e.h) or 1
					d = if e and e.local_y and e.local_y < h / 2 then { col = col.key, before = id } else { col = col.key, after = id }
				end
				land(payload.id, d :: Drop)
			end,
			ui.row {
				w_full = true,
				p = pad,
				gap = if compact then "$space.2" else ui.raw(10, "the stripe's gap, the card's inset"),
				items = "stretch",
				-- The stripe: its orchestrator's hue, a pill set in the card.
				ui.div { w = 3, shrink = 0, rounded = "$radius.full", bg = ink },
				body,
			},
			line_at("before"),
			line_at("after"),
		}
		return ContextMenu {
			label = locale.t("orch.board.card_actions"),
			ring = CARD_RADIUS,
			w_full = true,
			shrink = 0,
			items = function(): { any }?
				return menu(item)
			end,
			box,
		}
	end

	local function column(col: Column, stacked: boolean): Node
		local function list(): { Item }
			return shown:get()[col.key] or {}
		end
		return ui.column {
			id = if props.id then `{props.id}-{col.key}` else `orch-col-{col.key}`,
			class = `orch-column orch-column-{col.key}`,
			label = locale.t(col.name),
			role = "group",
			w_full = stacked,
			grow = if stacked then nil else 1,
			basis = if stacked then nil else 0,
			min_w = if stacked then nil else COLUMN_MIN,
			min_h = if stacked then nil else 64,
			gap = "$space.1",
			relative = true,
			-- Dropped past its cards (or into none): at its end.
			drop_when = accepts,
			on_drag_over = function(_payload: any)
				local d = drop:peek()
				if not d or d.col ~= col.key or (not d.before and not d.after) then
					set_drop({ col = col.key, at_end = true })
				end
			end,
			on_drag_leave = function()
				local d = drop:peek()
				if d and d.col == col.key and d.at_end then
					drop:set(nil)
				end
			end,
			on_drop = function(payload: any)
				local d = drop:peek()
				drop:set(nil)
				dragging:set(nil)
				land(payload.id, if d and d.col == col.key then d else { col = col.key, at_end = true })
			end,
			-- Its name and how many.
			ui.row {
				w_full = true,
				h = 24,
				px = "$space.1",
				gap = "$space.2",
				items = "center",
				ui.text { locale.t(col.name), text_size = "$font.size.xs", weight = "$font.weight.medium", color = "$text.secondary" },
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
				w_full = true,
				gap = "$space.1",
				ui.each(ui.computed(list), function(it: Item, i: any, row: any): Node
					return card(it, i, row, col)
				end, { key = function(it: Item): string
					return it.id
				end, w_full = true }),
			},
			-- The end, where a card dropped past the rest lands.
			ui.show(function(): boolean
				local d = drop:get()
				return d ~= nil and d.col == col.key and d.at_end == true
			end, ui.div { w_full = true, h = 2, rounded = "$radius.full", bg = "$accent", pointer_events = "none" }),
			ui.div { w_full = true, h = if stacked then 0 else 24 },
		}
	end

	-- The filter's chips: every orchestrator, and all of them.
	local function chips(): Node
		local function chip(id: string?, name: () -> string, h: number?): Node
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
			}
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
				return chip(o.id, function(): string
					return if row then row:get().name else o.name
				end, o.hue)
			end, { key = function(o: Orchestrator): string
				return o.id
			end }),
		}
	end

	-- Side by side where there's room, else stacked.
	local width = ui.use_state(0)
	local stacked = ui.computed(function(): boolean
		local w = width:get()
		return w > 0 and w < #columns * COLUMN_MIN + (#columns - 1) * 8
	end)
	return ui.column {
		id = props.id or "orch-board",
		class = "orch-board",
		label = locale.t("orch.board.label"),
		w_full = true,
		gap = "$space.2",
		on_resize = function(size: { w: number, h: number })
			if math.abs(size.w - width:peek()) >= 1 then
				width:set(size.w)
			end
		end,
		if props.show_filter then chips() else nil,
		ui.presence(function(): string
			return if stacked:get() then "stacked" else "side"
		end, function(k: string): Node
			local cols: { Node } = {}
			for _, c in columns do
				table.insert(cols, column(c, k == "stacked"))
			end
			if k == "stacked" then
				return ui.column { class = "orch-board-stacked", w_full = true, gap = "$space.3", table.unpack(cols) }
			end
			return ui.row { class = "orch-board-side", w_full = true, gap = "$space.2", items = "start", table.unpack(cols) }
		end, { w_full = true }),
	}
end)

return OrchBoard
