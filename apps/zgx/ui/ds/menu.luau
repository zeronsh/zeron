-- A menu's pieces: one row, heading, rule and group for every menu in
-- the app (the sidebar's view and section menus, the dropdown, the
-- composer's panels, the context menu), so they are one design.
--
-- A panel (`Menu.Panel`, the menus' glass) holds groups (`Menu.Group`:
-- its rows inset from the panel's edge, the list's gap between them, a
-- `heading` over them if it has one) with rules between them, edge to edge
-- (`Menu.Rule`). A row (`Menu.Row`) is compact and full width: a glyph in
-- its box (kept empty without one, so every name lines up), its name
-- (fading at its end if it runs long), and at its end its keys, a check
-- while it is on, or a chevron to a submenu.
--
--   Menu.Panel { w = 232,
--     Menu.Group { heading = "Sort",
--       Menu.Row { icon = "clock-circle", label = "Last updated", checked = by_updated, on_click = … },
--     },
--     Menu.Rule {},
--     Menu.Group { Menu.Row { icon = "trash-bin-minimalistic", label = "Delete", danger = true, on_click = … } },
--   }
--
-- A row washes under the pointer. A menu that moves a highlight with the
-- keys passes `lit` (a function) instead: the wash then follows it alone,
-- so the pointer's and the keys' never disagree. Other props go to the
-- row (its role, its handlers, a `bg` of its own).

local Icon = require("@self/icon")
local FadeText = require("@self/fade_text")
local Kbd = require("@self/kbd")
local glass = require("@self/glass")

local M = {}

export type RowProps = {
	icon: (Icon.Name | () -> Icon.Name)?,
	-- In the glyph's place: anything a line high (a tile, a mark).
	lead: Node?,
	-- No glyph box at all (a menu with no glyphs anywhere).
	bare: boolean?,
	-- (A message, `ui.t`, too: its name and what it's called.)
	label: (string | Message | () -> string)?,
	-- A line under the name; the row grows to hold it.
	description: (string | Message)?,
	shortcut: string?,
	checked: (State<boolean> | () -> boolean)?,
	submenu: boolean?,
	danger: boolean?,
	disabled: boolean?,
	lit: (() -> boolean)?,
	-- Anything else at the row's end, before its keys or check.
	trail: { Node }?,
	[any]: any,
}

local OWN = {
	icon = true,
	lead = true,
	bare = true,
	label = true,
	description = true,
	shortcut = true,
	checked = true,
	submenu = true,
	danger = true,
	disabled = true,
	lit = true,
	trail = true,
}

-- The box a row's glyph sits in (a tile's size): every name starts at the
-- same place, with a glyph or without.
function M.glyph(name: Icon.Name | () -> Icon.Name, ink: string?): Node
	return ui.div {
		w = "$zeron.menu.glyph.box",
		h = "$zeron.menu.glyph.box",
		shrink = 0,
		items = "center",
		justify = "center",
		color = ink or "$text.secondary",
		Icon { name = name, size = (ui.theme.get("zeron.menu.glyph.size") :: number?) or 16 },
	}
end

local function empty_glyph(): Node
	return ui.div { w = "$zeron.menu.glyph.box", h = "$zeron.menu.glyph.box", shrink = 0 }
end

-- A glyph's box, empty: a row without a glyph among rows with them.
M.glyph_space = empty_glyph

-- A check at a row's end while `on`.
function M.check(on: State<boolean> | () -> boolean): Node
	return ui.div {
		w = 14,
		shrink = 0,
		items = "center",
		color = "$text.secondary",
		ui.show(on, Icon { name = "check", size = 14 }),
	}
end

-- A row's chevron to its submenu.
function M.chevron(): Node
	return ui.div { shrink = 0, color = "$text.tertiary", Icon { name = "alt-arrow-right", size = 14 } }
end

-- A row's corner: concentric with the panel, through its edge and its
-- group's inset (the token ds/tokens makes of them).
function M.row_radius(): string
	return "$zeron.menu.row_radius"
end

-- A row's look without its content: for a row built by hand (a
-- dropdown's trigger, a context menu's submenu item) that should still be
-- a menu row. `tall`: it grows past a row (a description under it).
function M.look(props: { [any]: any }, tall: boolean?): { [any]: any }
	props.w_full = true
	props.px = "$space.2"
	props.gap = "$space.2"
	props.rounded = M.row_radius()
	if tall then
		props.py = "$space.2"
		props.items = props.items or "start"
	else
		props.h = props.h or "$zeron.menu.row"
		props.items = props.items or "center"
	end
	if props.cursor == nil then
		props.cursor = "pointer"
	end
	if props.hover == nil then
		props.hover = { bg = "$hover" }
	end
	if props.active == nil then
		props.active = { bg = "$pressed" }
	end
	return props
end

-- What a `ui.menu_item` row needs to light as zgx's own items do, for a menu
-- with rows of its own look: one highlight, the keys' and the pointer's
-- alike (the row under the pointer takes the focus, so the keys go on from
-- it), a press on it. `lit` holds the lit row's place in the menu, `els` the
-- rows' elements; added to `props`, which it returns.
function M.lights(props: { [any]: any }, i: number, lit: State<number>, els: { any }): { [any]: any }
	local enabled = not props.disabled
	props.bg = function()
		return if lit:get() == i then "$hover" else "#00000000"
	end
	props.hover = {}
	props.active = if enabled then { bg = "$pressed" } else {}
	props.focus = { outline = 0, border = 0 }
	props.cursor = if enabled then "pointer" else "default"
	props.on_mount = function(el: Element)
		els[i] = el
	end
	props.on_focus = function()
		lit:set(i)
	end
	props.on_blur = function()
		if lit:peek() == i then
			lit:set(0)
		end
	end
	props.on_hover = function(inside: boolean)
		if inside and enabled and els[i] then
			els[i]:focus()
			-- (Set here too: zgx skips this row's `on_focus`, its listener being
			-- the one running.)
			lit:set(i)
		end
	end
	return props
end

function M.Row(props: RowProps): Node
	local p: { [any]: any } = {}
	for k, v in props do
		if not OWN[k] and type(k) ~= "number" then
			p[k] = v
		end
	end
	local enabled = not props.disabled
	local ink = if not enabled then "$text.disabled" elseif props.danger then "$danger" else "$text.primary"
	local glyph_ink = if not enabled then "$text.disabled" elseif props.danger then "$danger" else nil
	local tall = props.description ~= nil
	M.look(p, tall)
	if not enabled then
		p.cursor = "default"
		p.hover = {}
		p.active = {}
	end
	local hover: Styles? = p.hover
	if props.danger and enabled and hover and hover.bg == "$hover" then
		p.hover = { bg = "$zeron.menu.danger" }
		p.active = { bg = "$zeron.menu.danger" }
	end
	local lit = props.lit
	if lit then
		local wash = if props.danger then "$zeron.menu.danger" else "$hover"
		local rest: (Color | () -> Color)? = p.bg
		p.bg = function(): Color
			if lit() then
				return wash
			end
			if type(rest) == "function" then
				return rest()
			end
			return rest or "#00000000"
		end
		p.hover = {}
	end
	p.label = p.label or (if type(props.label) == "string" or typeof(props.label) == "MessageUd" then props.label else nil)
	if props.checked then
		p.role = p.role or "checkbox"
		p.checked = props.checked
	end
	p.disabled = props.disabled

	if props.lead then
		table.insert(p, props.lead)
	elseif props.icon then
		table.insert(p, M.glyph(props.icon, glyph_ink))
	elseif not props.bare then
		table.insert(p, empty_glyph())
	end
	-- Its name: the children given, or its label.
	local own = 0
	for i, c in ipairs(props :: any) do
		own = i
		table.insert(p, c)
	end
	if own == 0 and props.label then
		local name = FadeText { props.label, text_size = "$font.size.md", color = ink, grow = 1 }
		if props.description then
			table.insert(
				p,
				ui.column {
					grow = 1,
					min_w = 0,
					gap = "$zeron.space.hair",
					name,
					ui.text {
						props.description,
						text_size = "$font.size.sm",
						line_height = "$font.line.sm",
						color = "$text.tertiary",
						wrap = true,
					},
				}
			)
		else
			table.insert(p, name)
		end
	end
	for _, t in props.trail or {} do
		table.insert(p, t)
	end
	if props.shortcut then
		table.insert(p, Kbd { keys = props.shortcut, look = "plain", size = "md", tone = if enabled then "quiet" else "disabled" })
	end
	if props.checked then
		table.insert(p, M.check(props.checked))
	end
	if props.submenu then
		table.insert(p, M.chevron())
	end
	return ui.row(p)
end

-- A group's title, over its rows: small, quiet, a short row of its own so
-- the gap under it is the rows'. `first`: it opens its list, so it has no
-- room over it of its own (the list's inset is that room): the space over
-- a list's first line is its inset whatever that line is, as under its last.
function M.Heading(props: { [any]: any }): Node
	local first = props.first == true
	return ui.row {
		w_full = true,
		h = if first then nil else "$zeron.menu.heading",
		pb = if first then "$space.1" else nil,
		px = "$space.2",
		items = if first then "end" else "center",
		shrink = 0,
		aria_hidden = true,
		ui.text {
			props[1],
			text_size = "$font.size.xs",
			weight = "$font.weight.medium",
			color = "$text.tertiary",
		},
	}
end

-- A rule between groups, edge to edge (the groups inset themselves).
function M.Rule(_props: { [any]: any }?): Node
	return ui.div { h = 1, w_full = true, shrink = 0, bg = "$border.subtle", aria_hidden = true }
end

-- A group of rows: inset from the panel's edge, the list's gap between
-- them, a `heading` over them if it has one.
function M.Group(props: { heading: (string | Message)?, [any]: any }): Node
	local p = table.clone(props) :: any
	local heading = p.heading
	p.heading = nil
	p.w_full = true
	p.p = p.p or "$space.1"
	p.gap = p.gap or "$zeron.list.gap"
	p.shrink = 0
	if heading then
		p.role = p.role or "group"
		p.label = p.label or heading
		table.insert(p, 1, M.Heading { heading, first = true })
	end
	return ui.column(p)
end

-- A menu's list that scrolls: its lines (rows, headings) in a scroll inset
-- from the panel's edge as a group is, the list's gap between them. `h` is
-- the scroll's height, `M.list_height` of what it holds: its lines' own,
-- up to whole rows (a scroll isn't as tall as its content of itself), so
-- the space over the first line and under the last is the inset alone,
-- whatever it holds (a line, a full list scrolled, nothing at all).
-- `before`: lines over the scroll in the same inset (a heading, what an
-- empty or loading list says). `id` names the list; other props go to the
-- scroll (its role, its label, its `offset`: a list whose highlight the
-- keys move keeps it in view with `M.reveal`).
function M.List(props: { lines: Node, h: any, before: { Node }?, [any]: any }): Node
	local scroll: { [any]: any } = { w_full = true, overscroll = "contain" }
	for k, v in props do
		if k ~= "lines" and k ~= "before" and k ~= "id" and type(k) ~= "number" then
			scroll[k] = v
		end
	end
	table.insert(scroll, props.lines)
	local col: { [any]: any } = { id = props.id, w_full = true, shrink = 0, p = "$space.1", gap = 0 }
	for _, n in props.before or {} do
		table.insert(col, n)
	end
	table.insert(col, ui.scroll(scroll))
	return ui.column(col)
end

-- A line's height there: a row's `row`, a heading's the theme's (a first
-- one's without the room over it).
local function line_height(line: string, i: number, row: number): number
	if line == "heading" then
		local heading = (ui.theme.get("zeron.menu.heading") :: number?) or 24
		local inset = (ui.theme.get("space.1") :: number?) or 4
		return if i == 1 then heading - inset else heading
	end
	return row
end
local function list_gap(): number
	return (ui.theme.get("zeron.list.gap") :: number?) or 2
end

-- How tall a list's lines stand, up to its first `rows` rows (with the
-- headings among them): a list's scroll's height, whole rows and nothing
-- under them. `lines` in order, each `"heading"` or `"row"`; `row` a row's
-- height. (A first heading's as `M.Heading` lays it: no room over it.)
function M.list_height(lines: { string }, rows: number, row: number): number
	local h, n, any = 0, 0, false
	local gap = list_gap()
	for i, line in lines do
		if line == "row" then
			n += 1
			if n > rows then
				break
			end
		end
		if any then
			h += gap
		end
		any = true
		h += line_height(line, i, row)
	end
	return h
end

-- Scroll a list (its scroll's `offset`) as little as brings its line `at`
-- into view whole, with the heading over it if one is (going up, a
-- section's first row shows under its name). `lines`, `rows` and `row` as
-- `M.list_height` takes them: the view is the height it gives. A list the
-- keys move a highlight through calls it as they move it (not as the
-- pointer does: a row it's over is in view, and the wheel is its own).
function M.reveal(offset: State<number>, lines: { string }, at: number, rows: number, row: number)
	if not lines[at] then
		return
	end
	local from = if at > 1 and lines[at - 1] == "heading" then at - 1 else at
	local gap = list_gap()
	local y, top = 0, 0
	for i = 1, at do
		if i > 1 then
			y += gap
		end
		if i == from then
			top = y
		end
		y += line_height(lines[i], i, row)
	end
	local view = M.list_height(lines, rows, row)
	-- (A heading and its row taller than the view: the row.)
	top = math.max(top, y - view)
	local o = offset:peek()
	if top < o then
		offset:set(top)
	elseif y > o + view then
		offset:set(y - view)
	end
end

-- A panel of the menus' glass, its groups and rules edge to edge.
function M.Panel(props: ContainerProps): Node
	local p = table.clone(props)
	p.p = p.p or 0
	p.gap = p.gap or 0
	glass.style(p, "menu")
	return ui.column(p)
end

-- The panel's props for zgx's own popovers (`ui.popover`, `ui.submenu`):
-- the glass and the edge-to-edge layout, added to `props`.
function M.panel(props: { [any]: any }): { [any]: any }
	props.p = props.p or 0
	props.gap = props.gap or 0
	return glass.style(props, "menu")
end

-- Submenu hover-intent corridor (safe polygon):
-- How long the pointer may cross sibling triggers or parent rows toward an
-- open submenu before the hovered row takes over (or the submenu closes),
-- the short grace for crossing the gap when not tracking a corridor, how far
-- past the submenu's edges the corridor reaches, the apex half-band that
-- absorbs slight wrist curvature without delaying straight vertical moves,
-- and the horizontal step that counts as advancing or reversing.
local CORRIDOR_GRACE = 300
local CORRIDOR_CLOSE = 150
local CORRIDOR_SLACK = 12
local CORRIDOR_APEX = 10
local CORRIDOR_NUDGE = 2

export type Point = { x: number, y: number }

export type CorridorItem = {
	open: State<boolean>,
	show: () -> (),
	close: () -> (),
	trigger: Element?,
	panel: Element?,
}

export type Corridor = {
	register: (self: Corridor, open: State<boolean>, show: () -> (), close: () -> ()) -> CorridorItem,
	unregister: (self: Corridor, item: CorridorItem) -> (),
	mount_trigger: (self: Corridor, item: CorridorItem, el: Element) -> (),
	mount_panel: (self: Corridor, item: CorridorItem, el: Element?) -> (),
	enter_trigger: (self: Corridor, item: CorridorItem) -> (),
	move_trigger: (self: Corridor, item: CorridorItem, p: Point) -> (),
	leave_trigger: (self: Corridor, item: CorridorItem) -> (),
	click_trigger: (self: Corridor, item: CorridorItem) -> (),
	enter_panel: (self: Corridor, item: CorridorItem) -> (),
	leave_panel: (self: Corridor, item: CorridorItem) -> (),
	move_parent: (self: Corridor, p: Point) -> (),
	leave_parent: (self: Corridor) -> (),
	opened: (self: Corridor, item: CorridorItem) -> (),
	closed: (self: Corridor, item: CorridorItem) -> (),
	close_all: (self: Corridor) -> (),
	cleanup: (self: Corridor) -> (),
}

-- Whether the pointer at `p`, having last moved on the open submenu's trigger
-- at `o`, is still heading for the submenu `b` (`left`: opened to its left):
-- inside the trapezoid from `[o.y - CORRIDOR_APEX, o.y + CORRIDOR_APEX]` at
-- `o.x` to `[b.y - CORRIDOR_SLACK, b.y + b.height + CORRIDOR_SLACK]` at `b`'s
-- near edge. Straight vertical moves (`advance <= 0`) immediately return false.
function M.toward(o: Point, p: Point, b: Element, left: boolean): boolean
	local bx, by, bw, bh = b.x, b.y, b.width, b.height
	if not (bx and by and bw and bh) then
		return false
	end
	local edge = if left then bx + bw else bx
	local dir = if left then -1 else 1
	local distance = (edge - o.x) * dir
	local advance = (p.x - o.x) * dir
	if distance <= 0 or advance <= 0 or advance > distance + CORRIDOR_SLACK then
		return false
	end
	local f = math.min(advance / distance, 1)
	local y0 = o.y - CORRIDOR_APEX
	local y1 = o.y + CORRIDOR_APEX
	local top = y0 + (by - CORRIDOR_SLACK - y0) * f
	local bottom = y1 + (by + bh + CORRIDOR_SLACK - y1) * f
	return p.y >= top and p.y <= bottom
end

-- Coordinates a parent panel's hover-opened submenus so moving diagonally
-- from an active trigger toward its open submenu across sibling triggers,
-- parent rows, or the gap between panels keeps the submenu open, while
-- straight-down moves, reversals, or pauses hand over promptly.
function M.corridor(): Corridor
	local items: { CorridorItem } = {}
	local active: CorridorItem? = nil
	local origin: Point? = nil
	local over_trigger: CorridorItem? = nil
	local over_panel: CorridorItem? = nil
	local pending_switch: CorridorItem? = nil
	local pending_at: Point? = nil
	local cancel_grace: (() -> ())? = nil
	local cancel_close: (() -> ())? = nil

	local function stop_grace()
		if cancel_grace then
			cancel_grace()
			cancel_grace = nil
		end
		pending_switch = nil
		pending_at = nil
	end

	local function stop_close()
		if cancel_close then
			cancel_close()
			cancel_close = nil
		end
	end

	local function live_active(): CorridorItem?
		if active and not active.open:peek() then
			active = nil
			origin = nil
			stop_grace()
			stop_close()
		end
		return active
	end

	local function is_left(item: CorridorItem): boolean
		local trig = item.trigger
		local panel = item.panel
		return trig ~= nil
			and panel ~= nil
			and panel.x ~= nil
			and trig.x ~= nil
			and (panel.x :: number) < (trig.x :: number)
	end

	local function activate(item: CorridorItem)
		stop_grace()
		stop_close()
		for _, other in items do
			if other ~= item and other.open:peek() then
				other.close()
			end
		end
		if active ~= item then
			origin = nil
		end
		active = item
		if not item.open:peek() then
			item.show()
		end
	end

	local function schedule_close()
		if cancel_close or cancel_grace then
			return
		end
		local cur = live_active()
		if not cur or over_trigger == cur or over_panel == cur then
			return
		end
		cancel_close = ui.after(CORRIDOR_CLOSE, function()
			cancel_close = nil
			local now = live_active()
			if now and over_trigger ~= now and over_panel ~= now and not cancel_grace then
				now.close()
				if active == now then
					active = nil
					origin = nil
				end
			end
		end)
	end

	local function arm_grace(target: CorridorItem?, p: Point)
		stop_close()
		if cancel_grace then
			cancel_grace()
			cancel_grace = nil
		end
		pending_switch = target
		pending_at = p
		cancel_grace = ui.after(CORRIDOR_GRACE, function()
			cancel_grace = nil
			local waiting = pending_switch
			pending_switch = nil
			pending_at = nil
			if waiting and over_trigger == waiting then
				activate(waiting)
				return
			end
			local now = live_active()
			if now and over_trigger ~= now and over_panel ~= now then
				if over_trigger then
					activate(over_trigger)
				else
					now.close()
					if active == now then
						active = nil
						origin = nil
					end
				end
			end
		end)
	end

	local self = {} :: Corridor

	function self:register(open: State<boolean>, show: () -> (), close: () -> ()): CorridorItem
		local item: CorridorItem = {
			open = open,
			show = show,
			close = close,
			trigger = nil,
			panel = nil,
		}
		table.insert(items, item)
		return item
	end

	function self:unregister(item: CorridorItem)
		for i, existing in items do
			if existing == item then
				table.remove(items, i)
				break
			end
		end
		if over_trigger == item then
			over_trigger = nil
		end
		if over_panel == item then
			over_panel = nil
		end
		if pending_switch == item then
			stop_grace()
		end
		if active == item then
			stop_grace()
			stop_close()
			active = nil
			origin = nil
		end
	end

	function self:mount_trigger(item: CorridorItem, el: Element)
		item.trigger = el
	end

	function self:mount_panel(item: CorridorItem, el: Element?)
		item.panel = el
	end

	function self:enter_trigger(item: CorridorItem)
		over_trigger = item
		local cur = live_active()
		if not cur or cur == item then
			stop_grace()
			stop_close()
			activate(item)
			return
		end
		-- Another submenu is open: wait for this trigger's pointer move if we
		-- have an origin and submenu bounds to test the corridor against.
		if origin and cur.panel then
			return
		end
		activate(item)
	end

	function self:move_trigger(item: CorridorItem, p: Point)
		over_trigger = item
		local cur = live_active()
		if not cur then
			activate(item)
			origin = p
			return
		end
		if cur == item then
			origin = p
			stop_grace()
			stop_close()
			return
		end
		local panel = cur.panel
		if origin and panel then
			local left = is_left(cur)
			if M.toward(origin, p, panel, left) then
				local dir = if left then -1 else 1
				local was = pending_at
				local forward = if was then (p.x - was.x) * dir else CORRIDOR_NUDGE
				if pending_switch == item and forward <= -CORRIDOR_NUDGE then
					activate(item)
					origin = p
					return
				end
				if pending_switch ~= item or forward >= CORRIDOR_NUDGE or not cancel_grace then
					arm_grace(item, p)
				end
				return
			end
		end
		activate(item)
		origin = p
	end

	function self:leave_trigger(item: CorridorItem)
		if over_trigger == item then
			over_trigger = nil
		end
		schedule_close()
	end

	function self:click_trigger(item: CorridorItem)
		activate(item)
	end

	function self:enter_panel(item: CorridorItem)
		over_panel = item
		if live_active() == item then
			stop_grace()
			stop_close()
			-- Once inside the submenu, leaving it later should not reuse the
			-- trigger's old cone origin.
			origin = nil
		end
	end

	function self:leave_panel(item: CorridorItem)
		if over_panel == item then
			over_panel = nil
		end
		schedule_close()
	end

	function self:move_parent(p: Point)
		if over_trigger then
			return
		end
		local cur = live_active()
		if not cur or over_panel == cur then
			return
		end
		local panel = cur.panel
		if origin and panel then
			local left = is_left(cur)
			if M.toward(origin, p, panel, left) then
				local dir = if left then -1 else 1
				local was = pending_at
				local forward = if was then (p.x - was.x) * dir else CORRIDOR_NUDGE
				if was and forward <= -CORRIDOR_NUDGE then
					stop_grace()
					schedule_close()
				elseif not cancel_grace or pending_switch ~= nil or forward >= CORRIDOR_NUDGE then
					arm_grace(nil, p)
				end
				return
			end
		end
		stop_grace()
		schedule_close()
	end

	function self:leave_parent()
		schedule_close()
	end

	function self:opened(item: CorridorItem)
		stop_grace()
		stop_close()
		for _, other in items do
			if other ~= item and other.open:peek() then
				other.close()
			end
		end
		if active ~= item then
			origin = nil
		end
		active = item
	end

	function self:closed(item: CorridorItem)
		item.panel = nil
		if over_panel == item then
			over_panel = nil
		end
		if active == item then
			stop_grace()
			stop_close()
			active = nil
			origin = nil
		end
	end

	function self:close_all()
		stop_grace()
		stop_close()
		for _, item in items do
			if item.open:peek() then
				item.close()
			end
		end
		active = nil
		origin = nil
		over_trigger = nil
		over_panel = nil
	end

	function self:cleanup()
		stop_grace()
		stop_close()
		active = nil
		origin = nil
		over_trigger = nil
		over_panel = nil
	end

	return self
end

return M
