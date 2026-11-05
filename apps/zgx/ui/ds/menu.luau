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
		props.min_h = "$zeron.menu.row"
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
-- the gap under it is the rows'.
function M.Heading(props: { [any]: any }): Node
	return ui.row {
		w_full = true,
		h = "$zeron.menu.heading",
		px = "$space.2",
		items = "center",
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
		table.insert(p, 1, M.Heading { heading })
	end
	return ui.column(p)
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

return M
