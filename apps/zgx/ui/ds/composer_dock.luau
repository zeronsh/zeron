-- The deck: what waits on the agent, as the composer's own top rather than
-- trays behind it. The composer's frame (its glass, edge, corner and
-- shadow) holds the deck and the pill as one surface, a rule between them,
-- so it grows upward as there is more to say. It holds the queue (their own
-- words, sent next), what is running (now) and the plan (the whole).
--
-- With one of them there, the header reads as its title and, folded, says
-- what it holds: "Running 2 · cargo test · 2m 14s", "Todo 3/7 · Porting the
-- queue's reorder". With more, the header is tabs, as ⌘K's scopes: each its
-- glyph, name and count, the open one on a thumb that springs from tab to
-- tab. A tab opens its rows under the header, a click on the open one (or
-- the chevron) folds them; folded, the header alone is as tall as the
-- pill's own line. A tab whose work is done folds itself (the plan once
-- every step is done and the turn is idle). A queue or plan that arrives
-- while nothing is open, opens; what is running stays folded.
--
--   Dock { queue = queued, todos = plan, tasks = running, open = tab, … }
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
-- its end, Stop all in the header. The chat's subagents run in the same
-- tab (`kind = "agent"`): a row with its title that opens it
-- (`on_open_task`), no Stop; those done fold behind "N finished", and the
-- header counts what runs and leads with an agent's name over a command's.
-- A list longer than the deck's cap scrolls, its edges fading as there is
-- more past them.

local Icon = require("@self/icon")
local IconButtonModule = require("@self/icon_button")
local IconButton = IconButtonModule.IconButton
local Button = require("@self/button")
local FadeText = require("@self/fade_text")
local RollText = require("@self/roll_text")
local glass = require("@self/glass")
local motion = require("@self/motion")
local status = require("@self/status")
local material = require("@self/material")
local control = require("@self/control")
local cargo = require("@self/composer_cargo")
local locale = require("@self/locale")
local Grid = require("@self/grid")

local M = {}

export type Queued = {
	id: string,
	text: string,
	cargo: { cargo.Item }?,
	-- Being edited in the composer (its text is there, not here).
	editing: boolean?,
	-- Held elsewhere: by whom ("Editing on devbox").
	locked: string?,
	-- Kept back until something happens ("Waits for a size"): it can't be
	-- sent or edited yet, only removed. Its words stand in its action's place.
	waiting: string?,
	-- How long it has waited, said beside it once that's worth knowing
	-- ("3 days ago").
	age: string?,
	-- false: it can't be taken back from here (no Remove).
	removable: boolean?,
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
	-- Done: how it ended ("exited 0", a subagent's "done").
	ended: string?,
	-- A subagent that ended badly: its mark says so.
	failed: boolean?,
}
export type Tray = "alerts" | "todos" | "agents" | "tasks" | "queue"
-- Something to know about the chat (a context nearly full, a rate limit, a
-- server that failed to start): how serious, what it is, a word more, and
-- a thing to do about it (its button's label).
export type AlertTone = "info" | "warning" | "danger"
-- `spinner`: still being worked on (reconnecting), the working ring in its
-- glyph's place. `dismissable = false`: it stays until what it says is
-- over (not connected), no × of its own. `body`: what its row's one line
-- can't hold, wrapped under it. `more`: choices listed under it, each a row
-- of its own whose action comes to `on_alert_action` by its id. They're a grid
-- (`grid`): every row's name, `cells` and trailing slot on the same lines,
-- whatever their words; a column goes once the list is narrower than its `min`,
-- from every row at once, so nothing's ever out of line. The trailing slot holds
-- its action, or `trail`, why there's none. `disabled`: there, but not to be
-- taken (dimmed, unless it's `current`: the one in use isn't out of reach).
-- `note`: said under those. `quiet`: there to read once the alerts are open,
-- but it raises nothing of its own (no pill, no count): a state worth knowing,
-- not one to act on.
export type GridColumn = Grid.Column
export type Alert = {
	id: string,
	tone: AlertTone,
	title: string,
	detail: string?,
	action: string?,
	spinner: boolean?,
	dismissable: boolean?,
	body: string?,
	more: { Alert }?,
	grid: { name: number, columns: { GridColumn } }?,
	note: string?,
	disabled: boolean?,
	current: boolean?,
	cells: { string }?,
	trail: string?,
	quiet: boolean?,
}
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
	-- A subagent's row opens it (a tab of the panel). Its row is a plain
	-- listing without this.
	on_open_task: ((id: string) -> ())?,
	-- Which tray is open; the dock keeps its own when not given.
	open: State<Tray?>?,
	-- Short of room: rows lose their thumbnails, actions become glyphs.
	narrow: (() -> boolean)?,
	-- What to know about the chat, each in its tone's colours; its action,
	-- and letting one go.
	alerts: State<{ Alert }>?,
	on_alert_action: ((id: string) -> ())?,
	on_dismiss_alert: ((id: string) -> ())?,
	-- Another pill floating over the composer's middle ("Scroll to end"):
	-- its width while it shows. The folded pill moves aside for it.
	aside: (() -> number?)?,
	-- The folded pill's width while it shows, nil while it doesn't (so the
	-- other pill can move aside for it in turn).
	on_pill_width: ((w: number?) -> ())?,
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

-- How deep a tray list's edges fade: each as far as there is more past it
-- that way, up to this, easing in with the scroll rather than popping on.
local FADE_DEPTH = 16

-- A scroll's edge fades (the sidebar's `edge_fades`, ds/sidebar): the top as
-- deep as it is scrolled, the bottom as deep as there is left to scroll,
-- each up to `depth`; none at either end. (`over`: how far the content runs
-- past its view.)
local function edge_fades(offset: State<number>, over: () -> number, depth: number): () -> { number }
	return function(): { number }
		local o = offset:get()
		return { math.min(o, depth), math.min(math.max(0, over() - o), depth) }
	end
end

local function folding(open: () -> boolean, cap: number, body: Node, key: string): Node
	local inner = ui.ref()
	local offset = ui.use_state(0)
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
			offset = offset,
			fade = edge_fades(offset, function(): number
				return math.max(0, (inner.height or 0) - want())
			end, FADE_DEPTH),
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

-- A tab of the deck: its glyph, its name (a message's key, or a function
-- giving one: a tab whose name follows what it holds), its count, the line
-- that stands for it while the deck is folded, whatever it adds at the
-- header's end, its rows, and whether it has anything to show.
type Tab = {
	id: Tray,
	glyph: Node,
	title: string | () -> string,
	count: () -> string,
	summary: () -> string,
	trail: { Node }?,
	body: Node,
	present: () -> boolean,
	-- Its glyph again, for the folded pill (a node lives in one place).
	pill_glyph: Node,
	-- A tab that says something's wrong: its tone (the worst it holds),
	-- which colours it, in the pill on its tone's wash.
	tone: (() -> AlertTone)?,
}

-- The deck's tabs: a capsule each, quiet until touched; the one open sits
-- on a thumb that springs from tab to tab (the segmented control's), there
-- only while there are tabs to choose between and one is open.
-- A tab's look: on the thumb (full ink, no wash of its own over it), alone
-- (a title: full ink, washed as it is touched), or one of several not open
-- (quieter, coming up as it is touched).
local tab_ink = ui.variants {
	base = { bg = "#00000000" },
	variants = {
		look = {
			thumb = { color = "$text.primary", hover = { bg = "#00000000" }, active = { bg = "#00000000" } },
			title = { color = "$text.primary", hover = { bg = "$hover" }, active = { bg = "$pressed" } },
			quiet = { color = "$text.secondary", hover = { bg = "$hover", color = "$text.primary" }, active = { bg = "$pressed", color = "$text.primary" } },
		},
	},
	defaults = { look = "quiet" },
}
local THUMB_SPRING: SpringOptions = { stiffness = 380, damping = 30 }
-- The folded pill: its height and the capsules' inset in it (the "Scroll
-- to end" pill's height, ds/transcript), and how far over the composer it
-- floats (that pill's six).
local PILL_H, PILL_PAD = 30, 3
-- Between it and "Scroll to end", side by side.
local PILL_PAIR_GAP = 8
M.PILL_PAIR_GAP = PILL_PAIR_GAP
M.PILL_H, M.PILL_GAP = PILL_H, 6

-- The deck, in two parts. Folded, a pill floating over the conversation
-- just above the composer (the "Scroll to end" pill's glass and height):
-- each thing waiting its glyph and count, a tap opening it. Open, the
-- composer's own top: a header of tabs (one alone reads as a title), the
-- open tab's rows, a rule, then the pill. Returns the top, then the pill.
local function deck(tabs: { Tab }, open: State<Tray?>, geometry: { [string]: number }, narrow: () -> boolean, aside: (() -> number?)?, on_pill_width: ((w: number?) -> ())?): (Node, Node)
	local inset, tab_h, gap = geometry.inset, geometry.tab, geometry.tab_gap
	local function present(): { Tab }
		local out: { Tab } = {}
		for _, t in tabs do
			if t.present() then
				table.insert(out, t)
			end
		end
		return out
	end
	local function any(): boolean
		return #present() > 0
	end
	local function many(): boolean
		return #present() > 1
	end
	-- The tab the header stands for: the one open, else the one open last,
	-- else the first there.
	local last = ui.use_state(nil :: Tray?)
	ui.effect(function()
		local o = open:get()
		if o then
			last:set(o)
		end
	end)
	local function current(): Tray?
		local list = present()
		local want = open:get() or last:get()
		for _, t in list do
			if t.id == want then
				return t.id
			end
		end
		return if list[1] then list[1].id else nil
	end
	local function is_open(): boolean
		return open:get() ~= nil and any()
	end
	-- The top is laid out while open, and until its fold has played.
	local laid = ui.use_state(is_open())
	local leaving: (() -> ())? = nil
	ui.effect(function()
		if is_open() then
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

	local refs: { [Tray]: any } = {}
	local tab_nodes: { Node } = {}
	for _, t in tabs do
		local id = t.id
		local ref = ui.ref()
		refs[id] = ref
		local function chosen(): boolean
			return open:get() == id and many()
		end
		local function title_key(): string
			local k = t.title
			return if type(k) == "function" then k() else k
		end
		table.insert(
			tab_nodes,
			ui.show(
				t.present,
				ui.button {
					ref = ref,
					h = tab_h,
					shrink = 0,
					pl = "$space.2",
					pr = ui.raw(10, "a step past the count, so it sits off the capsule's curve"),
					gap = "$zeron.space.tight",
					items = "center",
					rounded = ui.raw(tab_h / 2, "a capsule: the frame's corner less its edge and the header's inset"),
					cursor = "pointer",
					role = "tab",
					selected = chosen,
					expanded = function(): boolean
						return open:get() == id
					end,
					label = function()
						return locale.s(if open:get() == id then "ds.composer_dock.fold" else "ds.composer_dock.open", { title = locale.s(title_key()) })
					end,
					focus = control.ring,
					variants = tab_ink {
						look = function(): string
							return if chosen() then "thumb" elseif not many() then "title" else "quiet"
						end,
					},
					on_click = function()
						open:set(if open:peek() == id then nil else id)
					end,
					ui.div {
						w = 18,
						h = 18,
						shrink = 0,
						items = "center",
						justify = "center",
						color = function(): string
							return if open:get() == id or not many() then "$text.secondary" else "$text.tertiary"
						end,
						t.glyph,
					},
					ui.show(function(): boolean
						return not (narrow() and many())
					end, ui.text {
						function(): string
							return locale.s(title_key())
						end,
						shrink = 0,
						text_size = "$font.size.md",
						weight = "$font.weight.medium",
					}),
					RollText {
						t.count,
						shrink = 0,
						tabular = true,
						text_size = "$font.size.md",
						color = if t.tone
							then function(): string
								return `$zeron.tone.{(t.tone :: () -> AlertTone)()}.ink`
							end
							else "$text.tertiary",
					},
				}
			)
		)
	end
	-- The thumb under the open tab: where it starts and how wide, from the
	-- widths of the tabs there before it.
	local function thumb_box(): (number, number)
		local cur = open:get()
		local x = 0
		for _, t in present() do
			local w = refs[t.id].width or 0
			if t.id == cur then
				return x, w
			end
			x += w + gap
		end
		return 0, 0
	end
	local thumb = ui.div {
		absolute = true,
		left = 0,
		top = 0,
		h = tab_h,
		rounded = ui.raw(tab_h / 2, "the tab's capsule"),
		bg = "$zeron.composer.deck.thumb",
		pointer_events = "none",
		opacity = ui.tween(ui.computed(function(): number
			return if is_open() and many() then 1 else 0
		end), { duration = 160 }),
		x = motion.spring(ui.computed(function(): number
			local x = thumb_box()
			return x
		end), THUMB_SPRING),
		w = motion.spring(ui.computed(function(): number
			local _, w = thumb_box()
			return w
		end), THUMB_SPRING),
	}
	local strip: { [any]: any } = {
		relative = true,
		h = tab_h,
		shrink = 1,
		min_w = 0,
		gap = "$zeron.space.hair",
		items = "center",
		role = "tablist",
		thumb,
	}
	for _, node in tab_nodes do
		table.insert(strip, node)
	end

	-- Past the tabs, a click folds it.
	local spacer = ui.div {
		grow = 1,
		h_full = true,
		cursor = "pointer",
		on_click = function()
			open:set(nil)
		end,
	}
	-- At the header's end: what the tab it stands for adds (Stop all, the
	-- plan's dismiss), then the chevron that opens it or folds it.
	local trail: { Child } = {}
	for _, t in tabs do
		if t.trail and #t.trail > 0 then
			local id = t.id
			table.insert(trail, ui.show(function(): boolean
				return current() == id
			end, ui.row { shrink = 0, items = "center", gap = "$space.1", table.unpack(t.trail) }))
		end
	end
	local chevron = ui.div {
		w = 28,
		h = 28,
		shrink = 0,
		items = "center",
		justify = "center",
		rounded = ui.raw(14, "round, as the pill's controls"),
		color = "$text.tertiary",
		cursor = "pointer",
		role = "button",
		focusable = true,
		label = locale.t("ds.composer_dock.fold_all"),
		hover = { bg = "$hover", color = "$text.secondary" },
		active = { bg = "$pressed", color = "$text.secondary" },
		focus = control.ring,
		on_click = function()
			open:set(nil)
		end,
		Icon { name = "alt-arrow-down", size = 14 },
	}
	local head_row: { [any]: any } = {
		h = tab_h,
		shrink = 0,
		gap = "$space.2",
		items = "center",
		ui.row(strip),
		spacer,
	}
	for _, c in trail do
		table.insert(head_row, c)
	end
	table.insert(head_row, chevron)

	-- The rows of the tab it stands for.
	local bodies: { [string]: Node } = {}
	for _, t in tabs do
		bodies[t.id] = ui.column {
			w_full = true,
			px = ui.raw(inset, "the header's inset: rows' glyphs under the tabs'"),
			pb = ui.raw(inset - 4, "the header's inset, less the rows' own"),
			t.body,
		}
	end
	bodies.none = ui.div {}
	local body = folding(is_open, geometry.body, ui.switch(function(): string
		return current() or "none"
	end, bodies), "deck")

	local top = ui.column {
		w_full = true,
		shrink = 0,
		hidden = function(): boolean
			return not laid:get()
		end,
		overflow = "hidden",
		-- The header grows in from nothing as something arrives (the frame
		-- above the pill rising with it), and goes the same way.
		ui.column {
			w_full = true,
			shrink = 0,
			overflow = "hidden",
			h = ui.tween(ui.computed(function(): number
				return if is_open() then tab_h + inset * 2 else 0
			end), FOLD),
			opacity = ui.tween(ui.computed(function(): number
				return if is_open() then 1 else 0
			end), { duration = 180 }),
			ui.column {
				w_full = true,
				p = ui.raw(inset, "the header's inset: its capsules concentric with the frame"),
				ui.row(head_row),
			},
		},
		body,
		-- The rule between it and the pill, edge to edge (a menu's).
		ui.div {
			h = 1,
			w_full = true,
			shrink = 0,
			bg = "$border.subtle",
			aria_hidden = true,
			opacity = ui.tween(ui.computed(function(): number
				return if is_open() then 1 else 0
			end), { duration = 180 }),
		},
	}

	-- The pill: each thing waiting, its glyph and count (one alone, its name
	-- too), a tap opening that one; the chevron opens the one open last.
	-- (Its width, as laid out: how far it moves aside.)
	local pill_w = ui.use_state(0)
	local function pill(): Node
		local p = glass.style({
			h = PILL_H,
			-- The capsules' inset, the same on all four sides: the glass's 1
			-- point border is laid out inside the pill, so the padding is the
			-- inset less it (a side padding alone left the ends a point wider
			-- than the top and bottom).
			p = ui.raw(PILL_PAD - 1, "the capsules' inset in the pill, less its border"),
			gap = "$zeron.space.hair",
			items = "center",
			shrink = 0,
			rounded = ui.raw(PILL_H / 2, "a capsule, as the Scroll to end pill"),
			role = "toolbar",
			label = locale.t("ds.composer_dock.waiting"),
		}, "menu")
		p.shadow = "$shadow.md"
		local inner = PILL_H - PILL_PAD * 2
		for _, t in tabs do
			local id = t.id
			local function title_key(): string
				local k = t.title
				return if type(k) == "function" then k() else k
			end
			local tone = t.tone
			table.insert(p, ui.show(t.present, ui.button {
				relative = true,
				h = inner,
				pl = "$space.2",
				pr = "$space.2",
				gap = "$zeron.space.tight",
				items = "center",
				shrink = 0,
				rounded = ui.raw(inner / 2, "concentric with the pill: its corner less the inset"),
				bg = "#00000000",
				color = "$text.secondary",
				cursor = "pointer",
				hover = { bg = "$hover", color = "$text.primary" },
				active = { bg = "$pressed", color = "$text.primary" },
				focus = control.ring,
				label = function()
					return locale.s("ds.composer_dock.open", { title = locale.s(title_key()) })
				end,
				on_click = function()
					open:set(id)
				end,
				-- (Something wrong: on its tone's wash, under the touch's.)
				if tone
					then ui.div {
						absolute = true,
						left = 0,
						right = 0,
						top = 0,
						bottom = 0,
						rounded = ui.raw(inner / 2, "the capsule's"),
						pointer_events = "none",
						bg = function(): string
							return `$zeron.tone.{tone()}.wash`
						end,
					}
					else nil,
				ui.div { w = 16, h = 16, shrink = 0, items = "center", justify = "center", t.pill_glyph },
				ui.show(function(): boolean
					return not many()
				end, ui.text {
					function(): string
						return locale.s(title_key())
					end,
					shrink = 0,
					text_size = "$font.size.sm",
					weight = "$font.weight.medium",
					color = "$text.primary",
				}),
				RollText {
					t.count,
					shrink = 0,
					tabular = true,
					text_size = "$font.size.sm",
					weight = "$font.weight.medium",
					color = if tone
						then function(): string
							return `$zeron.tone.{tone()}.ink`
						end
						else nil,
				},
			}))
		end
		table.insert(p, ui.button {
			w = inner,
			h = inner,
			shrink = 0,
			items = "center",
			justify = "center",
			rounded = ui.raw(inner / 2, "round, as the capsules beside it"),
			bg = "#00000000",
			color = "$text.tertiary",
			cursor = "pointer",
			hover = { bg = "$hover", color = "$text.primary" },
			active = { bg = "$pressed", color = "$text.primary" },
			focus = control.ring,
			label = locale.t("ds.composer_dock.open_all"),
			on_click = function()
				open:set(current())
			end,
			Icon { name = "alt-arrow-up", size = 14 },
		})
		p.on_resize = function(size: { w: number, h: number })
			if math.abs(pill_w:peek() - size.w) >= 1 then
				pill_w:set(size.w)
			end
		end
		return ui.row(p)
	end
	-- Centred over the composer; while another pill floats there ("Scroll
	-- to end", `aside` its width), the two glide apart into a centred pair,
	-- this one on the right (the other moves left by as much: `on_pill_width`
	-- says how wide this one is while it shows).
	local floating = ui.div {
		shrink = 0,
		x = motion.spring(ui.computed(function(): number
			local other = if aside then aside() else nil
			if not other or other <= 0 then
				return 0
			end
			return (other + PILL_PAIR_GAP) / 2
		end), THUMB_SPRING),
		ui.presence(function(): string?
			return if any() and not is_open() then "pill" else nil
		end, pill, {
			enter = { opacity = { 0, 1 }, y = { 8, 0 }, duration = 180, ease = "outQuad" },
			exit = { opacity = 0, y = 8, duration = 120 },
		}),
	}
	if on_pill_width then
		local report = on_pill_width
		ui.effect(function()
			local w = pill_w:get()
			report(if any() and not is_open() and w > 0 then w else nil)
		end)
		-- (Gone with it: a pill beside it no longer moves aside for it.)
		ui.on_cleanup(function()
			report(nil)
		end)
	end
	return top, floating
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
	local function waits(): boolean
		return now().waiting ~= nil
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
			label = function(): string
				return now().waiting or words()
			end,
			tooltip = hint(),
			size = "sm",
			disabled = function(): boolean
				return locked() or waits()
			end,
			on_click = send,
		}
		local primary: Node = ui.show(narrow, IconButton(compact), Button {
			size = "sm",
			tone = "secondary",
			h = 24,
			label = function(): string
				return now().waiting or words()
			end,
			tooltip = hint(),
			disabled = function(): boolean
				return locked() or waits()
			end,
			on_click = send,
			ui.text {
				function(): string
					return now().waiting or words()
				end,
				text_size = "$font.size.xs",
			},
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
					disabled = function(): boolean
						return locked() or waits()
					end,
					on_click = function()
						if props.on_queue_edit then
							props.on_queue_edit(q.id)
						end
					end,
				},
				ui.show(function(): boolean
					return now().removable ~= false
				end, IconButton {
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
				}),
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
		ui.show(function(): boolean
			return now().age ~= nil
		end, ui.text {
			function(): string
				return now().age or ""
			end,
			shrink = 0,
			text_size = "$font.size.xs",
			color = "$text.tertiary",
		}),
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
-- and Stop. A subagent's is its mark (working, or how it ended), its title
-- and, ended, how; with `on_open_task` the whole row opens it, and it has no
-- Stop (the host stops a process, not a subagent).
-- A command as one line: its line breaks as spaces (a script of several
-- lines would otherwise stand as several rows in a one-line slot).
local function one_line(text: string): string
	return (string.gsub(string.gsub(text, "%s*[\r\n]+%s*", " "), "^%s+", ""))
end

local function task_row(t: Task, row: State<Task>, now: State<number>, props: Props, height: number): Node
	local function ended(): boolean
		return row:get().ended ~= nil
	end
	local agent = t.kind == "agent"
	local glyph: Icon.Name = if t.kind == "terminal" then "terminal" elseif agent then "side-chat" else "action-play"
	local opens = agent and props.on_open_task ~= nil
	local function open()
		if props.on_open_task then
			props.on_open_task(t.id)
		end
	end
	local p: { [any]: any } = {
		h = height,
		pl = "$space.2",
		pr = "$space.1",
		gap = "$space.2",
		items = "center",
		-- Concentric with the tray, as its head is.
		rounded = "$zeron.composer.tray.row_radius",
		role = if opens then "button" else "listitem",
		hover = { bg = "$hover" },
	}
	if opens then
		p.label = locale.t("ds.composer_dock.open_agent", { name = t.name })
		p.cursor = "pointer"
		p.focus = control.ring
		p.active = { bg = "$pressed" }
		p.on_click = open
	end
	local parts: { Child } = {
		ui.div {
			w = 18,
			h = 18,
			shrink = 0,
			items = "center",
			justify = "center",
			color = function()
				return if ended() then "$text.tertiary" else "$text.secondary"
			end,
			if agent
				then ui.switch(function(): string
					local r = row:get()
					return if not r.ended then "working" elseif r.failed then "failed" else "done"
				end, {
					working = status.StatusIcon { kind = "working", size = 14 },
					done = status.StatusIcon { kind = "done", size = 14 },
					failed = status.StatusIcon { kind = "failed", size = 14 },
				})
				else Icon { name = glyph, size = 14 },
		},
		FadeText {
			t.name,
			literal = true,
			shrink = 1,
			min_w = 48,
			font = if agent then nil else "$font.mono",
			text_size = if agent then "$font.size.md" else "$font.size.sm",
			color = function()
				return if ended() then "$text.tertiary" else "$text.primary"
			end,
		},
		-- The word on it takes what the name leaves (from nothing, not from
		-- its own width): a long command beside it would otherwise take the
		-- larger share of the shrinking and cut the name to a few letters.
		if t.detail
			then FadeText { one_line(t.detail), literal = true, grow = 1, basis = 0, text_size = "$font.size.sm", color = "$text.tertiary" }
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
	}
	if not agent then
		table.insert(
			parts,
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
			)
		)
	else
		-- (Room where a process's Stop is, so the rows' ends line up.)
		table.insert(parts, ui.div { w = 6, shrink = 0 })
	end
	for _, c in parts do
		table.insert(p, c)
	end
	return ui.row(p)
end

-- An alert: its tone's glyph in its ink, what it is, a word more, its
-- action, and letting it go.
-- Whether any of these alerts raises the tab (one that isn't quiet).
local function loud_in(list: { Alert }): boolean
	for _, x in list do
		if not x.quiet then
			return true
		end
	end
	return false
end

local function alert_row(a: Alert, row: State<Alert>, props: Props, height: number, width: State<number>, grid: { name: number, columns: { GridColumn } }?): Node
	local choice = grid ~= nil
	local function ink(): string
		return `$zeron.tone.{row:get().tone}.ink`
	end
	local function off(): boolean
		return row:get().disabled == true
	end
	-- Alone, the tab's glyph over it says its tone: a second would only repeat
	-- it. (Still working, its ring says what the tab's glyph doesn't. With
	-- others, each says its own.)
	local function has_glyph(): boolean
		if props.alerts ~= nil and #props.alerts:get() == 1 then
			return not choice and row:get().spinner == true
		end
		return true
	end
	local parts: { [any]: any } = {
		-- (A choice is told from the next by its name.)
		label = if choice then function(): string
			return row:get().title
		end else nil,
		disabled = if choice then off else nil,
		opacity = if choice then function(): any
			return if off() and row:get().current ~= true then "$zeron.disabled" else 1
		end else nil,
		min_h = height,
		pl = "$space.2",
		pr = "$space.1",
		gap = "$space.2",
		items = "center",
		rounded = "$zeron.composer.tray.row_radius",
		role = "listitem",
		hover = { bg = "$hover" },
		ui.div {
			w = 18,
			h = 18,
			shrink = 0,
			items = "center",
			justify = "center",
			color = ink,
			hidden = function(): boolean
				return not has_glyph()
			end,
			-- (A choice has no glyph, only its place: its alert's above it.)
			if choice then nil else ui.show(function(): boolean
				return row:get().spinner == true
			end, status.StatusIcon { kind = "working", size = 14 }, Icon {
				name = function(): Icon.Name
					return if row:get().tone == "info" then "info-circle" else "danger-triangle"
				end,
				size = 14,
			}),
		},
	}
	local function title(): string
		return row:get().title
	end
	if grid then
		-- A row of the grid (ds/grid): its name, its cells, and the trailing
		-- slot (its action, or why there's none) at its end.
		table.insert(parts, Grid.name(grid, title))
		for _, cell in Grid.cells(grid, width, function(): { string }?
			return row:get().cells
		end) do
			table.insert(parts, cell)
		end
		table.insert(parts, Grid.gap())
		table.insert(parts, Grid.trail(function(): string?
			return row:get().trail
		end, function(): boolean
			return row:get().current == true
		end))
	else
		table.insert(parts, FadeText {
			title,
			literal = true,
			shrink = 1,
			min_w = 48,
			text_size = "$font.size.md",
			color = "$text.primary",
		})
		table.insert(parts, FadeText {
			function(): string
				return row:get().detail or ""
			end,
			literal = true,
			grow = 1,
			basis = 0,
			text_size = "$font.size.sm",
			color = "$text.tertiary",
		})
	end
	if props.on_alert_action then
		table.insert(parts, ui.show(function(): boolean
			return row:get().action ~= nil
		end, Button {
			ui.text {
				function(): string
					return row:get().action or ""
				end,
			},
			size = "sm",
			tone = "ghost",
			h = 24,
			px = "$space.2",
			text_size = "$font.size.xs",
			color = ink,
			on_click = function()
				(props.on_alert_action :: (string) -> ())(a.id)
			end,
		}))
	end
	if props.on_dismiss_alert and not choice then
		table.insert(parts, ui.show(function(): boolean
			return row:get().dismissable ~= false
		end, IconButton {
			icon = "close",
			label = function(): string
				return locale.s("ds.composer_dock.dismiss_alert", { title = row:get().title })
			end,
			tooltip = locale.s("ds.composer_dock.dismiss"),
			size = "sm",
			tone = "quiet",
			on_click = function()
				(props.on_dismiss_alert :: (string) -> ())(a.id)
			end,
		}, ui.div { w = 6, shrink = 0 }))
	else
		table.insert(parts, ui.div { w = 6, shrink = 0 })
	end
	if choice then
		return ui.row(parts)
	end
	-- What it says past its one line, under its words (past the glyph).
	local function words(text: () -> string?): Node
		return ui.show(function(): boolean
			return text() ~= nil
		end, ui.text {
			function(): string
				return text() or ""
			end,
			literal = true,
			wrap = true,
			pl = function(): any
				return if has_glyph() then ui.raw(34, "under the row's words: its inset, the glyph and the gap") else "$space.2"
			end,
			pr = "$space.2",
			pb = "$space.1",
			text_size = "$font.size.sm",
			line_height = "$font.line.sm",
			color = "$text.secondary",
		})
	end
	local more = ui.computed(function(): { Alert }
		return row:get().more or {}
	end)
	return ui.column {
		w_full = true,
		ui.row(parts),
		words(function(): string?
			return row:get().body
		end),
		ui.each(more, function(x: Alert, _i: number, now: State<Alert>): Node
			return alert_row(x, now, props, height, width, row:peek().grid or { name = 0, columns = {} })
		end, {
			key = function(x: Alert): string
				return x.id
			end,
			w_full = true,
		}),
		words(function(): string?
			return row:get().note
		end),
	}
end

-- "3 finished": a row that opens the subagents that are done, or folds them.
local function finished_row(count: () -> number, open: () -> boolean, on_click: () -> ()): Node
	return ui.row {
		h = 28,
		px = "$space.2",
		gap = "$space.2",
		items = "center",
		cursor = "pointer",
		rounded = "$zeron.composer.tray.row_radius",
		hover = { bg = "$hover" },
		active = { bg = "$pressed" },
		focus = control.ring,
		role = "button",
		expanded = open,
		label = function()
			return locale.s(if open() then "ds.composer_dock.hide_finished" else "ds.composer_dock.show_finished", { n = count() })
		end,
		on_click = on_click,
		ui.div { w = 18, shrink = 0, items = "center", justify = "center", color = "$text.tertiary", Icon { name = "check-circle", size = 14 } },
		ui.text {
			function()
				return locale.s("ds.composer_dock.n_finished", { n = count() })
			end,
			text_size = "$font.size.sm",
			color = "$text.tertiary",
		},
		ui.div { grow = 1 },
		ui.div {
			shrink = 0,
			color = "$text.tertiary",
			Icon {
				name = function(): Icon.Name
					return if open() then "alt-arrow-up" else "alt-arrow-down"
				end,
				size = 12,
			},
		},
	}
end

local function build(props: Props): ({ Tab }, State<Tray?>, { [string]: number }, () -> boolean)
	local geometry = {
		inset = n("zeron.composer.deck.inset", 10),
		tab = n("zeron.composer.deck.tab", 30),
		tab_gap = n("zeron.space.hair", 2),
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
	-- Which trays are there: the queue with rows, the plan with steps,
	-- tasks running.
	local function has_queue(): boolean
		return props.queue ~= nil and #props.queue:get() > 0
	end
	local function has_todos(): boolean
		return props.todos ~= nil and #props.todos:get() > 0
	end
	-- What runs: each tab's own `present` (made with the tab, below).
	local present_fns: { { id: Tray, present: () -> boolean } } = {}
	local function has(id: Tray): boolean
		for _, f in present_fns do
			if f.id == id then
				return f.present()
			end
		end
		-- (Its tab not made yet: as it was.)
		return props.tasks ~= nil
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
		elseif (now == "tasks" or now == "agents") and not has(now) then
			open:set(nil)
		elseif now == "alerts" and (props.alerts == nil or not loud_in(props.alerts:get())) then
			open:set(nil)
		end
	end)
	-- Nothing opens itself: what arrives shows in the folded pill, and the
	-- person opens it if they want it.

	local tabs: { Tab } = {}

	-- What to know: first, as the most pressing.
	if props.alerts then
		local alerts = props.alerts
		-- (How wide its rows are, for what their grid can hold.)
		local alerts_w = ui.use_state(0)
		-- What raises the tab: the alerts that aren't quiet. A quiet one is read
		-- with them, never on its own.
		local function loud(): number
			local n = 0
			for _, x in alerts:get() do
				if not x.quiet then
					n += 1
				end
			end
			return n
		end
		local function worst(): AlertTone
			local w: AlertTone = "info"
			for _, x in alerts:get() do
				if x.tone == "danger" then
					return "danger"
				elseif x.tone == "warning" then
					w = "warning"
				end
			end
			return w
		end
		local function glyph(size: number): Node
			return ui.div {
				color = function(): string
					return `$zeron.tone.{worst()}.ink`
				end,
				Icon { name = "danger-triangle", size = size },
			}
		end
		local body = ui.column {
			w_full = true,
			pb = "$space.1",
			role = "list",
			label = locale.t("ds.composer_dock.alerts"),
			on_resize = Grid.measure(alerts_w),
			ui.each(alerts, function(x: Alert): Node
				local row = ui.computed(function(): Alert
					for _, y in alerts:get() do
						if y.id == x.id then
							return y
						end
					end
					return x
				end)
				return alert_row(x, row, props, geometry.row, alerts_w)
			end, {
				key = function(x: Alert): string
					return x.id
				end,
				animate = { duration = 220, ease = "outCubic" },
				enter = { opacity = { 0, 1 }, y = { -4, 0 }, duration = 200, ease = "outCubic" },
				exit = { opacity = 0, duration = 120 },
			}),
		}
		table.insert(tabs, {
			id = "alerts",
			glyph = glyph(16),
			pill_glyph = glyph(14),
			title = "ds.composer_dock.alerts",
			count = function()
				return tostring(loud())
			end,
			summary = function(): string
				for _, x in alerts:get() do
					if not x.quiet then
						return x.title
					end
				end
				return ""
			end,
			body = body,
			present = function(): boolean
				return loud() > 0
			end,
			tone = worst,
		})
	end

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
		table.insert(tabs, {
			id = "todos",
			glyph = glyph,
			pill_glyph = ui.div {
				ui.show(all_done, ui.div { color = "$success", Icon { name = "check-circle", size = 14 } }, Icon { name = "checklist", size = 14 }),
			},
			title = "ds.composer_dock.todo",
			count = function()
				local done, total = counts()
				return `{done}/{total}`
			end,
			summary = summary,
			trail = trail,
			body = body,
			present = has_todos,
		})
	end

	-- What is running.
	if props.tasks then
		local tasks = props.tasks
		-- One clock for both tabs, ticking only while something runs (a timer
		-- made in an effect belongs to its run, and stops as it runs again).
		local now = ui.use_state(os.time())
		local any_running = ui.computed(function(): boolean
			for _, t in tasks:get() do
				if not t.ended then
					return true
				end
			end
			return false
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
		-- A tab of what runs: the subagents (`agents`), or the processes and
		-- terminals (`tasks`).
		local function running_tab(id: Tray, agents: boolean)
			local function mine(t: Task): boolean
				return (t.kind == "agent") == agents
			end
			local function list(): { Task }
				local out: { Task } = {}
				for _, t in tasks:get() do
					if mine(t) then
						table.insert(out, t)
					end
				end
				return out
			end
			local function running(): { Task }
				local out: { Task } = {}
				for _, t in list() do
					if not t.ended then
						table.insert(out, t)
					end
				end
				return out
			end
			-- Whether the subagents that are done are listed (they fold behind
			-- one row while others run; with none running they are the list).
			local show_finished = ui.use_state(false)
			local function finished_count(): number
				if not agents then
					return 0
				end
				local n = 0
				for _, t in list() do
					if t.ended then
						n += 1
					end
				end
				return n
			end
			-- There while any runs; once none does, only while it is open
			-- (what just finished, a subagent to go back to, stays in reach).
			local function present(): boolean
				local l = list()
				for _, t in l do
					if not t.ended then
						return true
					end
				end
				return #l > 0 and open:get() == id
			end
			table.insert(present_fns, { id = id, present = present })
			-- What stands for it: the first running, how long, how many more.
			local function summary(): string
				local r = running()
				local first = r[1]
				if not first then
					return locale.s("ds.composer_dock.nothing_running")
				end
				local s = first.name
				if first.since and not agents then
					s ..= ` · {status.elapsed(now:get() - first.since)}`
				end
				if #r > 1 then
					s ..= ` · {locale.s("ds.composer_dock.more_running", { n = #r - 1 })}`
				end
				return s
			end
			local trail: { Node } = {}
			if not agents and props.on_stop_all then
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
			-- The rows: what runs, in the order given; then (subagents) those
			-- done behind "N finished", listed outright when none runs.
			type Listed = { kind: "task", task: Task } | { kind: "finished" }
			local shown = ui.computed(function(): { Listed }
				local out: { Listed } = {}
				local done: { Task } = {}
				for _, t in list() do
					if agents and t.ended then
						table.insert(done, t)
					else
						table.insert(out, { kind = "task", task = t } :: Listed)
					end
				end
				if #done > 0 then
					local alone = #out == 0
					if not alone then
						table.insert(out, { kind = "finished" } :: Listed)
					end
					if alone or show_finished:get() then
						for _, t in done do
							table.insert(out, { kind = "task", task = t } :: Listed)
						end
					end
				end
				return out
			end)
			local body = ui.column {
				w_full = true,
				pb = "$space.1",
				ui.each(shown, function(item: Listed): Node
					if item.kind == "finished" then
						return finished_row(finished_count, function(): boolean
							return show_finished:get()
						end, function()
							show_finished:set(not show_finished:peek())
						end)
					end
					local t = item.task
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
					key = function(item: Listed): string
						return if item.kind == "finished" then "\0finished" else item.task.id
					end,
					animate = { duration = 220, ease = "outCubic" },
					enter = { opacity = { 0, 1 }, y = { -4, 0 }, duration = 200, ease = "outCubic" },
					exit = { opacity = 0, duration = 120 },
				}),
			}
			-- Subagents: the right panel's side chat's glyph; what runs, the
			-- command line's.
			local glyph_name: Icon.Name = if agents then "side-chat" else "terminal"
			local name_key = if agents then "ds.composer_dock.subagents" else "ds.composer_dock.running"
			table.insert(tabs, {
				id = id,
				glyph = Icon { name = glyph_name, size = 16 },
				pill_glyph = Icon { name = glyph_name, size = 14 },
				-- (Nothing running, the tab held open: what finished.)
				title = function(): string
					return if #running() > 0 or agents then name_key else "ds.composer_dock.finished"
				end,
				count = function()
					local n = #running()
					return tostring(if n > 0 then n else (if agents then finished_count() else #list()))
				end,
				summary = summary,
				trail = trail,
				body = body,
				present = present,
			})
		end
		running_tab("agents", true)
		running_tab("tasks", false)
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
		table.insert(tabs, {
			id = "queue",
			glyph = Icon { name = "list", size = 16 },
			pill_glyph = Icon { name = "list", size = 14 },
			title = "ds.composer_dock.queued",
			count = function()
				return tostring(#queue:get())
			end,
			summary = summary,
			body = body,
			present = has_queue,
		})
	end

	return tabs, open, geometry, narrow
end

-- The deck's two parts, for the composer: its top (inside the frame, over
-- the pill), and the folded pill (floating over the conversation).
function M.parts(props: Props): (Node, Node)
	local tabs, open, geometry, narrow = build(props)
	return deck(tabs, open, geometry, narrow, props.aside, props.on_pill_width)
end

-- The deck alone (a fixture's): the pill over the top.
M.Dock = ui.component(function(props: Props): Node
	local top, pill = M.parts(props)
	return ui.column {
		w_full = true,
		shrink = 0,
		gap = M.PILL_GAP,
		items = "start",
		pill,
		top,
	}
end)

return M
