-- The trays behind the composer: what waits on the agent, stacked behind
-- the pill as pages behind a page. Each tray is the menus' glass with
-- rounded top corners, tucked under the one below it (and the last under
-- the pill), and each sits a step further in than the one below, so the
-- stack recedes rather than towers. From the pill upward, by what the
-- person will act on soonest: the queue (their own words, sent next), what
-- is running (now), the plan (the whole).
--
-- One tray is open at a time. The others fold to their header, a line
-- that still says what matters: "Todo 3/7 · Porting the queue's reorder",
-- "Running 2 · cargo test · 2m 14s", "Queued 2 · Then run the tests…".
-- Opening one folds the rest; a tray whose work is done folds itself (the
-- plan once every step is done and the turn is idle). A queue or plan that
-- arrives while nothing is open, opens; what is running stays folded.
--
--   Dock { queue = queued, todos = plan, tasks = running, open = tray, … }
--
-- The queue's rows are Zeron's (crates/ui/src/queue.rs): a grip, its
-- thumbnails, its first line, and at its end Edit, Remove and one primary
-- action: "Send now" for a message with cargo (it interrupts), "Steer" where
-- the agent takes a word mid-turn, else "Send next". Rows drag into a new
-- order (or Alt+↑/↓). A row being edited says so. The plan's rows are the
-- todo panel's (todo_panel.rs): done, in progress (the working ring while
-- the turn is live), pending; a long plan folds to the step in progress and
-- its neighbours, "N earlier" and "N later" opening the rest. What is
-- running is ours: a row a task or terminal, how long it has run, Stop at
-- its end, Stop all in the header.

local Icon = require("@self/icon")
local IconButtonModule = require("@self/icon_button")
local IconButton = IconButtonModule.IconButton
local Button = require("@self/button")
local FadeText = require("@self/fade_text")
local status = require("@self/status")
local material = require("@self/material")
local control = require("@self/control")
local cargo = require("@self/composer_cargo")
local locale = require("@self/locale")

local M = {}

export type Queued = {
	id: string,
	text: string,
	cargo: { cargo.Item }?,
	-- Being edited in the composer (its text is there, not here).
	editing: boolean?,
	-- Held elsewhere: by whom ("Editing on devbox").
	locked: string?,
}
export type TodoState = "done" | "doing" | "pending"
export type Todo = { id: string, text: string, state: TodoState }
export type TaskKind = "terminal" | "agent" | "process"
export type Task = {
	id: string,
	kind: TaskKind,
	name: string,
	-- A word on it, after its name (":5173", "reading 14 files").
	detail: string?,
	-- When it started, in `os.time()` seconds.
	since: number?,
	-- Done: how it ended ("exited 0").
	ended: string?,
}
export type Tray = "todos" | "tasks" | "queue"
export type Props = {
	queue: State<{ Queued }>?,
	-- Whether the agent takes a word mid-turn (a row's action is then Steer).
	can_steer: (boolean | () -> boolean)?,
	on_queue_send: ((id: string) -> ())?,
	on_queue_edit: ((id: string) -> ())?,
	on_queue_remove: ((id: string) -> ())?,
	on_queue_reorder: ((from: number, to: number) -> ())?,
	-- A row being edited: saving it, and letting it go.
	on_queue_save: ((id: string) -> ())?,
	on_queue_cancel: ((id: string) -> ())?,
	todos: State<{ Todo }>?,
	-- The turn is live (the step in progress wears the working ring).
	live: (() -> boolean)?,
	on_dismiss_todos: (() -> ())?,
	tasks: State<{ Task }>?,
	on_stop_task: ((id: string) -> ())?,
	on_stop_all: (() -> ())?,
	-- Which tray is open; the dock keeps its own when not given.
	open: State<Tray?>?,
	-- Short of room: rows lose their thumbnails, actions become glyphs.
	narrow: (() -> boolean)?,
}

-- The trays' geometry, in points, read as the dock is built (the theme's).
local function n(token: string, fallback: number): number
	local v = ui.theme.get(token)
	return if type(v) == "number" then v else fallback
end

-- How a tray folds and opens (the dropdown's fold).
local FOLD_MS = 240
local FOLD: TweenOptions = { duration = FOLD_MS, ease = "out(3)" }
-- Over how many steps a long plan folds, and to how many.
local PLAN_FOLD_PAST, PLAN_FOLD_TO = 6, 3

-- A faint ring (a pending step), or the accent's ring with a dot (the step
-- in progress while the turn is idle), `size` across.
local function step_ring(size: number, doing: boolean): Node
	return ui.canvas {
		w = size,
		h = size,
		shrink = 0,
		draw = function(c, w, h)
			local r = w / 2 - 1
			if doing then
				c:stroke(ui.path():arc(w / 2, h / 2, r, 0, 360), "$accent", 1.5)
				c:fill(ui.path():arc(w / 2, h / 2, r * 0.4, 0, 360), "$accent")
			else
				c:stroke(ui.path():arc(w / 2, h / 2, r, 0, 360), "$border.strong", 1.5)
			end
		end,
	}
end

-- What folds: the body, clipped to nothing while its tray is folded, eased
-- open to its content's height (never past the trays' cap: past it the
-- body scrolls), fading as it goes.
-- Each tray's body height as last laid out, by tray: a tray made again (a
-- reload) opens at it from its first frame rather than growing open again.
local SEEN = ui.state("composer_tray_heights", {} :: { [string]: number })

local function folding(open: () -> boolean, cap: number, body: Node, key: string): Node
	local inner = ui.ref()
	local function want(): number
		local h = inner.height or 0
		if h <= 0 then
			-- (Not yet laid out: as it was.)
			h = SEEN:get()[key] or 0
		end
		return math.min(h, cap)
	end
	-- Folded, the body leaves the layout once the fold has played (a scroll
	-- of no height would be a warning); opening, it is laid out at once and
	-- the clip eases to it.
	local laid = ui.use_state(open())
	local leaving: (() -> ())? = nil
	ui.effect(function()
		if open() then
			if leaving then
				leaving()
				leaving = nil
			end
			laid:set(true)
		elseif laid:peek() then
			leaving = ui.after(FOLD_MS + 40, function()
				leaving = nil
				laid:set(false)
			end)
		end
	end)
	return ui.column {
		w_full = true,
		shrink = 0,
		overflow = "hidden",
		-- (Folded to a point, not nothing: a clip of no height holding a
		-- scroll is a layout warning while the fold plays.)
		h = ui.tween(ui.computed(function(): number
			return if open() then want() else 1
		end), FOLD),
		opacity = ui.tween(ui.computed(function(): number
			return if open() then 1 else 0
		end), { duration = 180 }),
		-- The scroll fills the clip: as tall as the body, or the cap, and
		-- past the cap the body scrolls in it.
		ui.scroll {
			w_full = true,
			grow = 1,
			min_h = 1,
			hidden = function()
				return not laid:get()
			end,
			overscroll = "contain",
			fade = function()
				return { 0, if (inner.height or 0) > cap then 16 else 0 }
			end,
			ui.column {
				ref = inner,
				w_full = true,
				shrink = 0,
				on_resize = function(size: { w: number, h: number })
					if math.abs((SEEN:peek()[key] or 0) - size.h) >= 1 then
						SEEN:set_key(key, size.h)
					end
				end,
				body,
			},
		},
	}
end

-- A tray's header: its glyph, its name, its count, and (folded) the line
-- that stands for it; at its end whatever the tray adds, then the chevron.
-- A click anywhere on it opens the tray, or folds it.
type Head = {
	id: Tray,
	glyph: Node,
	-- (Its title's message's key.)
	title: string,
	count: (() -> string)?,
	summary: (() -> string)?,
	trail: { Node }?,
}
local function head(opts: Head, open: State<Tray?>, height: number): Node
	local function is_open(): boolean
		return open:get() == opts.id
	end
	local parts: { Child } = {
		ui.div { w = 18, h = 18, shrink = 0, items = "center", justify = "center", color = "$text.secondary", opts.glyph },
		ui.text {
			locale.t(opts.title),
			shrink = 0,
			text_size = "$font.size.md",
			weight = "$font.weight.medium",
			color = "$text.primary",
		},
	}
	if opts.count then
		table.insert(parts, ui.text {
			opts.count,
			shrink = 0,
			tabular = true,
			text_size = "$font.size.md",
			color = "$text.tertiary",
		})
	end
	if opts.summary then
		-- Folded: what the tray would say, after its name; open, the room
		-- stays (the header's end holds still).
		table.insert(parts, ui.row {
			grow = 1,
			min_w = 0,
			items = "center",
			opacity = ui.tween(ui.computed(function(): number
				return if is_open() then 0 else 1
			end), { duration = 180 }),
			FadeText { opts.summary, grow = 1, text_size = "$font.size.md", color = "$text.secondary" },
		})
	else
		table.insert(parts, ui.div { grow = 1 })
	end
	for _, t in opts.trail or {} do
		table.insert(parts, t)
	end
	table.insert(parts, ui.div {
		w = 22,
		h = 22,
		shrink = 0,
		items = "center",
		justify = "center",
		color = "$text.tertiary",
		Icon {
			name = function(): Icon.Name
				return if is_open() then "alt-arrow-down" else "alt-arrow-up"
			end,
			size = 14,
		},
	})
	local p: { [any]: any } = {
		h = height,
		pl = "$space.2",
		pr = "$space.1",
		gap = "$space.2",
		items = "center",
		shrink = 0,
		-- Concentric with the tray's top corners, through its edge and inset.
		-- Concentric with the tray: its corner less its edge and padding.
		rounded = "$zeron.composer.tray.row_radius",
		cursor = "pointer",
		role = "button",
		label = function()
			return locale.s(if is_open() then "ds.composer_dock.fold" else "ds.composer_dock.open", { title = locale.s(opts.title) })
		end,
		expanded = is_open,
		hover = { bg = "$hover" },
		active = { bg = "$pressed" },
		focus = control.ring,
		on_click = function()
			open:set(if is_open() then nil else opts.id)
		end,
	}
	for _, c in parts do
		table.insert(p, c)
	end
	return ui.row(p)
end

-- A tray: the glass, its top corners, its inset by level (1 just behind
-- the pill, 2 behind that…), tucked under what is below it.
local function tray(level: () -> number, head_node: Node, body: Node, open: () -> boolean, geometry: { [string]: number }, key: string): Node
	local step, tuck, radius = geometry.step, geometry.tuck, geometry.radius
	return ui.column {
		shrink = 0,
		mx = function(): number
			return step * level()
		end,
		mb = ui.raw(-tuck, "tucked under the next tray, or the pill"),
		rounded_tl = ui.raw(radius, "the tray's corner, worked out from the pill's (geometry)"),
		rounded_tr = ui.raw(radius, "the tray's corner, worked out from the pill's (geometry)"),
		border = 1,
		border_sides = { 1, 1, 0, 1 },
		border_color = "$zeron.composer.tray.edge",
		bg = function()
			return if material.frosted:get() then "$zeron.composer.tray.tint" else "$zeron.composer.tray.solid"
		end,
		backdrop_blur = function()
			return if material.frosted:get() then "$zeron.composer.tray.blur" else 0
		end,
		px = "$space.1",
		pt = "$space.1",
		pb = ui.raw(tuck + 4, "the tuck, and the tray's own inset"),
		head_node,
		folding(open, geometry.body, body, key),
	}
end

-- A queued message's thumbnails: its first picture or two (one, narrow),
-- and how many more.
local function thumbs(items: { cargo.Item }, narrow: () -> boolean): Node?
	if #items == 0 then
		return nil
	end
	local shown: { Node } = {}
	local most = if narrow() then 1 else 2
	for i, it in items do
		if i > most then
			break
		end
		table.insert(
			shown,
			ui.overlay {
				w = 40,
				h = 28,
				shrink = 0,
				-- Concentric with its row, a step in from the row's edge.
				rounded = "$zeron.composer.tray.thumb_radius",
				overflow = "hidden",
				border = 1,
				border_color = "$zeron.composer.chip.edge",
				border_over = true,
				bg = "$hover",
				items = "center",
				justify = "center",
				label = it.name,
				tooltip = it.name,
				if it.kind == "image" and it.src
					then ui.image {
						src = it.src,
						fit = "cover",
						w_full = true,
						h_full = true,
						-- (Its file gone since: the glyph a file has.)
						fallback = ui.div { color = "$text.secondary", Icon { name = "paperclip", size = 14 } },
					}
					else ui.div { color = "$text.secondary", Icon { name = if it.kind == "annotation" then "cursor" else "paperclip", size = 14 } },
			}
		)
	end
	local rest = #items - most
	if rest > 0 then
		local names: { string } = {}
		for i = most + 1, #items do
			table.insert(names, items[i].name)
		end
		table.insert(shown, ui.text {
			`+{rest}`,
			shrink = 0,
			tabular = true,
			text_size = "$font.size.xs",
			weight = "$font.weight.medium",
			color = "$text.tertiary",
			tooltip = locale.s("ds.composer_dock.rest", { n = rest, names = table.concat(names, " · ") }),
		})
	end
	return ui.row { gap = "$space.1", items = "center", shrink = 0, table.unpack(shown) }
end

-- A queued row.
local function queue_row(q: Queued, i: () -> number, last: () -> boolean, row: State<Queued>, props: Props, narrow: () -> boolean, height: number): Node
	local hovered = ui.use_state(false)
	local function now(): Queued
		return row:get()
	end
	local function steer(): boolean
		local s = props.can_steer
		return if type(s) == "function" then s() else s == true
	end
	-- Its one action: Send now for a message with cargo (it interrupts),
	-- Steer where the agent takes a word mid-turn, else Send next. (Its
	-- kind; `words`, as it reads.)
	local function action(): string
		if now().cargo and #(now().cargo :: { cargo.Item }) > 0 then
			return "send_now"
		end
		return if steer() then "steer" else "send_next"
	end
	local function words(): string
		return locale.s(`ds.composer_dock.{action()}`)
	end
	local function hint(): string
		local what = locale.s(`ds.composer_dock.{action()}_hint`)
		return if last() then locale.s("ds.composer_dock.hint_last", { what = what, key = ui.shortcut_text("cmd+enter") }) else what
	end
	local function locked(): boolean
		return now().locked ~= nil
	end
	local function editing(): boolean
		return now().editing == true
	end
	local function send()
		if props.on_queue_send then
			props.on_queue_send(q.id)
		end
	end
	-- Edit and Remove stand in their room at all times, seen under the
	-- pointer (or with the keys), so the row's words never shift.
	local function quiet_when(): number
		return if hovered:get() then 1 else 0
	end
	-- Being edited: Save to queue, and Cancel, in the actions' place.
	local editing_side = ui.row {
		gap = "$zeron.space.hair",
		shrink = 0,
		items = "center",
		IconButton {
			icon = "check",
			label = locale.t("ds.composer_dock.save_to_queue"),
			tooltip = locale.s("ds.composer_dock.save_to_queue"),
			size = "sm",
			on_click = function()
				if props.on_queue_save then
					props.on_queue_save(q.id)
				end
			end,
		},
		IconButton {
			icon = "close",
			label = locale.t("ds.composer_dock.cancel"),
			tooltip = locale.s("ds.composer_dock.cancel"),
			size = "sm",
			tone = "quiet",
			on_click = function()
				if props.on_queue_cancel then
					props.on_queue_cancel(q.id)
				end
			end,
		},
	}
	local function side(): Node
		-- Typed apart: checked inline, the icon's function against
		-- `Icon.Name | () -> Icon.Name` is too complex for the solver.
		local compact: IconButtonModule.Props = {
			icon = function(): Icon.Name
				return if action() == "steer" then "arrow-right" else "arrow-up"
			end,
			label = words,
			tooltip = hint(),
			size = "sm",
			disabled = locked,
			on_click = send,
		}
		local primary: Node = ui.show(narrow, IconButton(compact), Button {
			size = "sm",
			tone = "secondary",
			h = 24,
			label = words,
			tooltip = hint(),
			disabled = locked,
			on_click = send,
			ui.text { words, text_size = "$font.size.xs" },
		})
		return ui.row {
			gap = "$zeron.space.hair",
			shrink = 0,
			items = "center",
			ui.row {
				gap = "$zeron.space.hair",
				items = "center",
				opacity = ui.tween(ui.computed(quiet_when), { duration = 120 }),
				IconButton {
					icon = "pen",
					label = locale.t("ds.composer_dock.edit"),
					tooltip = locale.s("ds.composer_dock.edit"),
					size = "sm",
					tone = "quiet",
					disabled = locked,
					on_click = function()
						if props.on_queue_edit then
							props.on_queue_edit(q.id)
						end
					end,
				},
				IconButton {
					icon = "trash-bin-minimalistic",
					label = locale.t("ds.composer_dock.remove"),
					tooltip = locale.s("ds.composer_dock.remove"),
					size = "sm",
					tone = "quiet",
					disabled = locked,
					on_click = function()
						if props.on_queue_remove then
							props.on_queue_remove(q.id)
						end
					end,
				},
			},
			primary,
		}
	end
	return ui.row {
		h = height,
		pl = "$space.1",
		pr = "$space.1",
		gap = "$space.2",
		items = "center",
		-- Concentric with the tray, as its head is.
		rounded = "$zeron.composer.tray.row_radius",
		role = "listitem",
		label = function()
			return `{i()}. {now().text}`
		end,
		opacity = function(): number
			return if locked() then 0.55 else 1
		end,
		bg = function()
			return if editing() then "$zeron.selected.flat.bg" else "#00000000"
		end,
		hover = { bg = "$hover" },
		on_hover = function(inside: boolean)
			hovered:set(inside)
		end,
		-- Its grip: the row drags by it into a new place.
		ui.div {
			w = 14,
			h = 22,
			shrink = 0,
			items = "center",
			justify = "center",
			color = "$text.tertiary",
			cursor = "grab",
			Icon { name = "drag-handle", size = 14 },
		},
		ui.text {
			function()
				return tostring(i())
			end,
			w = 12,
			shrink = 0,
			tabular = true,
			align = "right",
			text_size = "$font.size.xs",
			color = "$text.tertiary",
		},
		ui.show(function()
			return not narrow() or (q.cargo ~= nil and #(q.cargo :: { cargo.Item }) > 0)
		end, thumbs(q.cargo or {}, narrow) or ui.div {}),
		FadeText {
			function()
				if editing() then
					return locale.s("ds.composer_dock.editing")
				end
				local l = now().locked
				return if l then l else (string.gsub(now().text, "%s+", " "))
			end,
			grow = 1,
			text_size = "$font.size.md",
			color = function()
				return if editing() or locked() then "$text.tertiary" else "$text.primary"
			end,
		},
		ui.show(editing, editing_side, side()),
	}
end

-- A step of the plan.
local function todo_row(t: Todo, row: State<Todo>, live: () -> boolean): Node
	local function state(): TodoState
		return row:get().state
	end
	local glyph = ui.div {
		w = 14,
		h = 14,
		shrink = 0,
		items = "center",
		justify = "center",
		ui.show(function()
			return state() == "done"
		end, ui.div { color = "$success", Icon { name = "check-circle", size = 14 } }),
		ui.show(function()
			return state() == "doing" and live()
		end, status.StatusIcon { kind = "working", size = 14 }),
		ui.show(function()
			return state() == "doing" and not live()
		end, step_ring(12, true)),
		ui.show(function()
			return state() == "pending"
		end, step_ring(10, false)),
	}
	return ui.row {
		min_h = 26,
		px = "$space.2",
		py = "$space.1",
		gap = "$space.2",
		items = "start",
		role = "listitem",
		ui.div { h = 18, shrink = 0, items = "center", glyph },
		ui.text {
			t.text,
			wrap = true,
			grow = 1,
			min_w = 0,
			text_size = "$font.size.md",
			line_height = "$font.line.md",
			weight = function()
				return if state() == "doing" then "$font.weight.medium" else "$font.weight.regular"
			end,
			color = function()
				local s = state()
				return if s == "done" then "$text.tertiary" elseif s == "doing" then "$text.primary" else "$text.secondary"
			end,
		},
	}
end

-- "N earlier" / "N later": a row that opens the folded steps.
local function fold_row(count: () -> number, later: boolean, on_click: () -> ()): Node
	return ui.row {
		h = 24,
		px = "$space.2",
		gap = "$space.2",
		items = "center",
		cursor = "pointer",
		rounded = "$zeron.composer.tray.row_radius",
		hover = { bg = "$hover" },
		role = "button",
		label = function()
			return locale.s("ds.composer_dock.show_steps", { n = count(), later = tostring(later) })
		end,
		on_click = on_click,
		ui.div { w = 14, shrink = 0, items = "center", justify = "center", color = "$text.tertiary", Icon { name = if later then "alt-arrow-down" else "alt-arrow-up", size = 12 } },
		ui.text {
			function()
				return locale.s("ds.composer_dock.steps", { n = count(), later = tostring(later) })
			end,
			text_size = "$font.size.xs",
			color = "$text.tertiary",
		},
	}
end

-- A running task or terminal: its glyph, its name, a word on it, how long,
-- and Stop.
local function task_row(t: Task, row: State<Task>, now: State<number>, props: Props, height: number): Node
	local function ended(): boolean
		return row:get().ended ~= nil
	end
	local glyph: Icon.Name = if t.kind == "terminal" then "terminal" elseif t.kind == "agent" then "bot" else "action-play"
	return ui.row {
		h = height,
		pl = "$space.2",
		pr = "$space.1",
		gap = "$space.2",
		items = "center",
		-- Concentric with the tray, as its head is.
		rounded = "$zeron.composer.tray.row_radius",
		role = "listitem",
		hover = { bg = "$hover" },
		ui.div {
			w = 18,
			h = 18,
			shrink = 0,
			items = "center",
			justify = "center",
			color = function()
				return if ended() then "$text.tertiary" else "$text.secondary"
			end,
			ui.show(function()
				return not ended() and t.kind == "agent"
			end, status.StatusIcon { kind = "working", size = 14 }, Icon { name = glyph, size = 14 }),
		},
		FadeText {
			t.name,
			literal = true,
			shrink = 1,
			min_w = 48,
			font = if t.kind == "agent" then nil else "$font.mono",
			text_size = if t.kind == "agent" then "$font.size.md" else "$font.size.sm",
			color = function()
				return if ended() then "$text.tertiary" else "$text.primary"
			end,
		},
		-- The word on it takes what the name leaves (from nothing, not from
		-- its own width): a long command beside it would otherwise take the
		-- larger share of the shrinking and cut the name to a few letters.
		if t.detail
			then FadeText { t.detail, literal = true, grow = 1, basis = 0, text_size = "$font.size.sm", color = "$text.tertiary" }
			else ui.div { grow = 1 },
		ui.text {
			function(): string
				local e = row:get().ended
				if e then
					return e
				end
				local since = row:get().since
				return if since then status.elapsed(now:get() - since) else ""
			end,
			shrink = 0,
			tabular = true,
			text_size = "$font.size.xs",
			color = "$text.tertiary",
		},
		ui.show(
			function()
				return not ended()
			end,
			IconButton {
				icon = "stop",
				label = locale.t("ds.composer_dock.stop_named", { name = t.name }),
				tooltip = locale.s("ds.composer_dock.stop"),
				size = "sm",
				tone = "quiet",
				on_click = function()
					if props.on_stop_task then
						props.on_stop_task(t.id)
					end
				end,
			}
		),
	}
end

M.Dock = ui.component(function(props: Props): Node
	local geometry = {
		radius = n("zeron.composer.tray.radius", 16),
		step = n("zeron.composer.tray.step", 16),
		tuck = n("zeron.composer.tray.tuck", 10),
		header = n("zeron.composer.tray.header", 32),
		row = n("zeron.composer.tray.row", 32),
		body = n("zeron.composer.tray.body", 216),
	}
	local open: State<Tray?> = props.open or ui.use_state(nil :: Tray?)
	local narrow = props.narrow or function()
		return false
	end
	local live = props.live or function()
		return false
	end
	local function opened(id: Tray): () -> boolean
		return function(): boolean
			return open:get() == id
		end
	end
	-- Which trays are there: the queue with rows, the plan with steps,
	-- tasks running.
	local function has_queue(): boolean
		return props.queue ~= nil and #props.queue:get() > 0
	end
	local function has_todos(): boolean
		return props.todos ~= nil and #props.todos:get() > 0
	end
	local function has_tasks(): boolean
		return props.tasks ~= nil and #props.tasks:get() > 0
	end
	-- Each tray's level: how many trays sit between it and the pill, and one.
	local function queue_level(): number
		return 1
	end
	local function tasks_level(): number
		return 1 + (if has_queue() then 1 else 0)
	end
	local function todos_level(): number
		return 1 + (if has_queue() then 1 else 0) + (if has_tasks() then 1 else 0)
	end
	-- A tray arriving while none is open, opens; one gone while open,
	-- closes; the plan folds itself once every step is done and the turn
	-- rests.
	local function all_done(): boolean
		if not props.todos then
			return false
		end
		for _, t in props.todos:get() do
			if t.state ~= "done" then
				return false
			end
		end
		return true
	end
	ui.effect(function()
		local now = open:get()
		if now == "queue" and not has_queue() then
			open:set(nil)
		elseif now == "todos" and (not has_todos() or (all_done() and not live())) then
			open:set(nil)
		elseif now == "tasks" and not has_tasks() then
			open:set(nil)
		end
	end)
	-- What is running doesn't: it stays folded to its header ("Running 3 ·
	-- cargo test · 2m") until opened, as a session can hold a score of
	-- processes for hours. Opened, it stays open while any run.
	local seen = { queue = false, todos = false }
	ui.effect(function()
		local q, t = has_queue(), has_todos()
		local arrived: Tray? = if q and not seen.queue
			then "queue"
			elseif t and not seen.todos and not (all_done() and not live()) then "todos"
			else nil
		seen.queue, seen.todos = q, t
		if arrived and open:peek() == nil then
			open:set(arrived)
		end
	end)

	local trays: { Node } = {}

	-- The plan.
	if props.todos then
		local todos = props.todos
		local show_all = ui.use_state(false)
		local function counts(): (number, number, number?)
			local done, total, current = 0, 0, nil
			for i, t in todos:get() do
				total += 1
				if t.state == "done" then
					done += 1
				elseif t.state == "doing" and current == nil then
					current = i
				end
			end
			return done, total, current
		end
		-- The rows shown: all of them, or (a long plan) the step in progress
		-- with a neighbour either side; a finished plan, its last three.
		type Shown = { kind: "step", todo: Todo } | { kind: "fold", count: number, later: boolean }
		local shown = ui.computed(function(): { Shown }
			local list = todos:get()
			local out: { Shown } = {}
			if show_all:get() or #list <= PLAN_FOLD_PAST then
				for _, t in list do
					table.insert(out, { kind = "step", todo = t } :: Shown)
				end
				return out
			end
			local _, total, current = counts()
			local from: number
			if current then
				from = math.clamp(current - 1, 1, total - PLAN_FOLD_TO + 1)
			else
				-- No step in progress: the first pending, else the last three.
				local pending: number? = nil
				for i, t in list do
					if t.state == "pending" then
						pending = i
						break
					end
				end
				from = if pending then math.clamp(pending - 1, 1, total - PLAN_FOLD_TO + 1) else total - PLAN_FOLD_TO + 1
			end
			local to = from + PLAN_FOLD_TO - 1
			if from > 1 then
				table.insert(out, { kind = "fold", count = from - 1, later = false } :: Shown)
			end
			for i = from, to do
				table.insert(out, { kind = "step", todo = list[i] } :: Shown)
			end
			if to < total then
				table.insert(out, { kind = "fold", count = total - to, later = true } :: Shown)
			end
			return out
		end)
		local function summary(): string
			local list = todos:get()
			for _, t in list do
				if t.state == "doing" then
					return t.text
				end
			end
			for _, t in list do
				if t.state == "pending" then
					return t.text
				end
			end
			return locale.s("ds.composer_dock.all_done")
		end
		local trail: { Node } = {}
		if props.on_dismiss_todos then
			table.insert(trail, ui.show(function()
				return not live()
			end, IconButton {
				icon = "close",
				label = locale.t("ds.composer_dock.dismiss_todos"),
				tooltip = locale.s("ds.composer_dock.dismiss"),
				size = "sm",
				tone = "quiet",
				on_click = function()
					(props.on_dismiss_todos :: () -> ())()
				end,
			}))
		end
		local glyph = ui.div {
			ui.show(all_done, ui.div { color = "$success", Icon { name = "check-circle", size = 16 } }, Icon { name = "checklist", size = 16 }),
		}
		local body = ui.column {
			w_full = true,
			pb = "$space.1",
			ui.each(shown, function(s: Shown): Node
				if s.kind == "fold" then
					local count = s.count
					return fold_row(function()
						return count
					end, s.later, function()
						show_all:set(true)
					end)
				end
				local t = s.todo
				-- The row follows its step (its state changes under it).
				local row = ui.computed(function(): Todo
					for _, x in todos:get() do
						if x.id == t.id then
							return x
						end
					end
					return t
				end)
				return todo_row(t, row, live)
			end, {
				key = function(s: Shown): string
					return if s.kind == "fold" then (if s.later then "\0later" else "\0earlier") else s.todo.id
				end,
				animate = { duration = 220, ease = "outCubic" },
				enter = { opacity = { 0, 1 }, y = { -4, 0 }, duration = 200, ease = "outCubic" },
				exit = { opacity = 0, duration = 120 },
			}),
		}
		table.insert(
			trays,
			ui.show(
				has_todos,
				tray(todos_level, head({
					id = "todos",
					glyph = glyph,
					title = "ds.composer_dock.todo",
					count = function()
						local done, total = counts()
						return `{done}/{total}`
					end,
					summary = summary,
					trail = trail,
				}, open, geometry.header), body, opened("todos"), geometry, "todos")
			)
		)
	end

	-- What is running.
	if props.tasks then
		local tasks = props.tasks
		local now = ui.use_state(os.time())
		local function running(): { Task }
			local out: { Task } = {}
			for _, t in tasks:get() do
				if not t.ended then
					table.insert(out, t)
				end
			end
			return out
		end
		-- The clock ticks only while something runs (a timer made in an
		-- effect belongs to its run, and stops as it runs again).
		local any_running = ui.computed(function(): boolean
			return #running() > 0
		end)
		ui.effect(function()
			if not any_running:get() then
				return
			end
			now:set(os.time())
			ui.every(1000, function()
				now:set(os.time())
			end)
		end)
		local function summary(): string
			local list = running()
			local first = list[1]
			if not first then
				return locale.s("ds.composer_dock.nothing_running")
			end
			local s = first.name
			if first.since then
				s ..= ` · {status.elapsed(now:get() - first.since)}`
			end
			if #list > 1 then
				s ..= ` · {locale.s("ds.composer_dock.more_running", { n = #list - 1 })}`
			end
			return s
		end
		local trail: { Node } = {}
		if props.on_stop_all then
			table.insert(trail, ui.show(function()
				return #running() > 0
			end, Button {
				locale.t("ds.composer_dock.stop_all"),
				size = "sm",
				tone = "ghost",
				h = 24,
				px = "$space.2",
				text_size = "$font.size.xs",
				on_click = function()
					(props.on_stop_all :: () -> ())()
				end,
			}))
		end
		local body = ui.column {
			w_full = true,
			pb = "$space.1",
			ui.each(tasks, function(t: Task): Node
				-- The row follows its task (zgx's animated rows get no state
				-- of their own): found in the list by its id.
				local row = ui.computed(function(): Task
					for _, x in tasks:get() do
						if x.id == t.id then
							return x
						end
					end
					return t
				end)
				return task_row(t, row, now, props, geometry.row)
			end, {
				key = function(t: Task): string
					return t.id
				end,
				animate = { duration = 220, ease = "outCubic" },
				enter = { opacity = { 0, 1 }, y = { -4, 0 }, duration = 200, ease = "outCubic" },
				exit = { opacity = 0, duration = 120 },
			}),
		}
		table.insert(
			trays,
			ui.show(
				has_tasks,
				tray(tasks_level, head({
					id = "tasks",
					glyph = Icon { name = "action-play", size = 16 },
					title = "ds.composer_dock.running",
					count = function()
						return tostring(#running())
					end,
					summary = summary,
					trail = trail,
				}, open, geometry.header), body, opened("tasks"), geometry, "tasks")
			)
		)
	end

	-- The queue.
	if props.queue then
		local queue = props.queue
		local function summary(): string
			local first = queue:get()[1]
			return if first then (string.gsub(first.text, "%s+", " ")) else ""
		end
		local body = ui.column {
			w_full = true,
			pb = "$space.1",
			role = "list",
			label = locale.t("ds.composer_dock.queued_messages"),
			ui.each(queue, function(q: Queued): Node
				local row = ui.computed(function(): Queued
					for _, x in queue:get() do
						if x.id == q.id then
							return x
						end
					end
					return q
				end)
				local function index(): number
					for i, x in queue:get() do
						if x.id == q.id then
							return i
						end
					end
					return 0
				end
				local function last(): boolean
					local list = queue:get()
					return list[#list] ~= nil and list[#list].id == q.id
				end
				return queue_row(q, index, last, row, props, narrow, geometry.row)
			end, {
				key = function(q: Queued): string
					return q.id
				end,
				gap = "$zeron.space.hair",
				animate = { duration = 220, ease = "outCubic" },
				enter = { opacity = { 0, 1 }, y = { 6, 0 }, duration = 220, ease = "outCubic" },
				exit = { opacity = 0, duration = 140 },
				reorder = props.on_queue_reorder,
				item_label = function(q: Queued): string
					return q.text
				end,
			}),
		}
		table.insert(
			trays,
			ui.show(
				has_queue,
				tray(queue_level, head({
					id = "queue",
					glyph = Icon { name = "list", size = 16 },
					title = "ds.composer_dock.queued",
					count = function()
						return tostring(#queue:get())
					end,
					summary = summary,
				}, open, geometry.header), body, opened("queue"), geometry, "queue")
			)
		)
	end

	return ui.column {
		w_full = true,
		shrink = 0,
		-- Each tray is painted after the one above it, so the lower tray
		-- covers the tuck of the one behind.
		table.unpack(trays),
	}
end)

return M
