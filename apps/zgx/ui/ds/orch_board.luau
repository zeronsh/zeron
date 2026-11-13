--!strict
-- The board: where every piece of an orchestrator's work stands, in
-- columns Now, Next, Blocked, Review and Done (today), with Inbox before
-- them where loose items are captured (`inbox = true`). Shared by the
-- orchestrator's page (filtered to it, `compact`) and the Control room
-- (all of them, its filter on top).
--
-- Each column is a lane of its own, its name, glyph and count at its head;
-- an empty one stands slim, its head alone, so the columns with work in
-- them take the room (and a board with nothing on it says so). A card:
-- how its lead thread stands (Zeron's status glyph, held 300 ms at least), its title,
-- how far along it is (its threads' todo as segmented cells, else its own
-- progress), its threads as dots when it has several, its pull request,
-- its priority when that is high or urgent, and, where the board holds
-- more than one orchestrator's work, whose it is (its hue's dot and name,
-- quiet; a loose item says none). A card in Review or Done
-- carries its outcome, two lines of it until it's opened: the answer or
-- result, where it is read. Each card is a plugin slot
-- (`orchestration.item`, `provide = { item = id }`).
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
local material = require("@self/material")
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

-- The least a column takes side by side; under that for them all, they
-- stack. An empty one stands this wide.
local COLUMN_MIN = 168
local SLIM = 112
local COLUMN_MAX = 360
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
-- A card's corner.
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

	-- Whether the board shows more than one orchestrator's work: then each
	-- card says whose it is.
	local mixed = ui.computed(function(): boolean
		local seen: string? = nil
		for _, l in shown:get() do
			for _, it in l do
				local o = it.orchestrator_id
				if o then
					if seen and seen ~= o then
						return true
					end
					seen = o
				end
			end
		end
		return false
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
		-- Whose it is, said where the board holds more than one's.
		local function owner(): Orchestrator?
			local o = item().orchestrator_id
			if not o or not mixed:get() then
				return nil
			end
			for _, x in orchestrators() do
				if x.id == o then
					return x
				end
			end
			return nil
		end
		local function progress(): number?
			return progress_of(item(), props.todo_of)
		end
		local lead = hold.value(function(): string
			return lead_status(item(), props.thread_of)
		end)
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
		-- Its title: two lines at most, how its lead thread stands before it.
		local title_text = ui.text {
				function(): string
					return item().title
				end,
				wrap = true,
				w_full = not compact,
				grow = if compact then 1 else nil,
				min_w = 0,
				max_h = line * 2,
				overflow = "hidden",
				text_size = "$font.size.sm",
				line_height = "$font.line.sm",
				weight = "$font.weight.medium",
				color = function(): string
					return if item().state == "done" then "$text.secondary" else "$text.primary"
				end,
			}
		local title = ui.row {
			w_full = not compact,
			grow = if compact then 1 else nil,
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
			-- (Side by side, its priority at the title's end; compact, among
			-- its marks.)
			if compact then nil else ui.div { grow = 1 },
			if compact then nil else ui.div {
				h = line,
				items = "center",
				shrink = 0,
				priority_mark(function(): string
					return item().priority
				end),
			},
		}
		-- Its marks: priority, threads, pull request.
		local marks = ui.row {
				w_full = not compact,
				shrink = 0,
				h = line,
				gap = "$space.1",
				items = "center",
				if compact then priority_mark(function(): string
					return item().priority
				end) else nil,
				ui.show(function(): boolean
					return owner() ~= nil
				end, ui.row {
					class = "orch-card-owner",
					gap = "$zeron.space.tight",
					items = "center",
					min_w = 0,
					shrink = 1,
					hue.Dot { hue = function(): number?
						local o = owner()
						return o and o.hue
					end, size = 6 },
					ui.text {
						function(): string
							local o = owner()
							return if o then o.name else ""
						end,
						truncate = true,
						min_w = 0,
						text_size = "$font.size.xs",
						color = "$text.tertiary",
					},
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
						local p = progress()
						return if p then `{math.floor(p * 100 + 0.5)}%` else ""
					end,
					shrink = 0,
					tabular = true,
					text_size = "$font.size.xs",
					color = "$text.tertiary",
				},
			})
		-- (Compact, its marks stand at its title's end, the bar under both.)
		local body = ui.column {
			w_full = true,
			min_w = 0,
			gap = if compact then "$space.1" else "$zeron.space.snug",
			if compact then ui.row { w_full = true, gap = "$space.3", items = "start", title, marks } else title,
			progress_bar,
			-- (Its marks, where it has any: several threads, a pull request.)
			if compact then nil else ui.show(function(): boolean
				return #(item().threads or {}) > 1 or #prs() > 0 or owner() ~= nil
			end, marks),
			-- Its outcome: two lines, or all of it opened.
			ui.show(
				has_outcome,
				ui.button {
					class = "orch-outcome",
					label = locale.t("orch.board.outcome"),
					w_full = true,
					items = "start",
					p = 0,
					h = "auto",
					bg = "#00000000",
					hover = { bg = "#00000000" },
					active = { bg = "#00000000" },
					on_click = function()
						open:set(not open:peek())
					end,
					ui.row {
						w_full = true,
						gap = "$space.2",
						items = "start",
						p = "$space.2",
						rounded = "$radius.md",
						bg = "$zeron.settings.comment",
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
			p = 0,
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
				items = "stretch",
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
			-- (Side by side, an empty one stands slim: the room is for work.)
			grow = if stacked then nil else function(): number
				return if #list() > 0 then 1 else 0
			end,
			basis = if stacked then nil else 0,
			min_w = if stacked then nil else function(): number
				return if #list() > 0 then COLUMN_MIN else SLIM
			end,
			w = if stacked then nil else function(): number | string
				return if #list() > 0 then "auto" else SLIM
			end,
			-- (A few cards: a column keeps a card's reading width, not the
			-- board's.)
			max_w = if stacked then nil else COLUMN_MAX,
			-- (Empty, its head alone, unless a card is on its way: then it's
			-- somewhere to put one.)
			min_h = if stacked then nil else function(): number
				return if #list() > 0 or dragging:get() ~= nil then 96 else 0
			end,
			p = "$space.1",
			rounded = "$radius.lg",
			bg = function(): string
				return if #list() > 0 then material.pick("$zeron.settings.note", "$surface.sunken")() else "#00000000"
			end,
			border = 1,
			border_color = "$zeron.settings.edge",
			-- (Stacked in a page's side pane, an empty column stands aside
			-- until a card is dragged: then it's somewhere to put one.)
			hidden = if stacked and compact
				then function(): boolean
					return #list() == 0 and dragging:get() == nil
				end
				else nil,
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
				h = 28,
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
		local need = 0
		for _, c in columns do
			need += if #(shown:get()[c.key] or {}) > 0 then COLUMN_MIN else SLIM
		end
		return w > 0 and w < need + (#columns - 1) * 8
	end)
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
		if props.show_filter then chips() else nil,
		ui.show(function(): boolean
			for _, l in shown:get() do
				if #l > 0 then
					return false
				end
			end
			return true
		end, ui.row {
			class = "orch-board-empty",
			w_full = true,
			gap = "$space.2",
			items = "center",
			ui.div { color = "$text.tertiary", Icon { name = "checklist", size = 14 } },
			ui.text { locale.t("orch.board.empty"), text_size = "$font.size.sm", color = "$text.tertiary" },
		}),
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
