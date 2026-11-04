-- A window's title bar over its sidebar: the window's buttons, the
-- sidebar's toggle, back and forward, at Zeron's exact places (crates/ui/
-- src/shell.rs: `titlebar_cluster_start`, `TITLEBAR_*`; lib.rs: the
-- window's `traffic_light_position`), so a sidebar under it in a real window
-- lines up with the system's buttons.
--
-- Open, they sit flat on the sidebar's top. Collapsed, the sidebar no
-- longer runs under them: they stand over the page (its wallpaper, its
-- art), so an island of glass gathers behind them, as Zeron's does
-- (`titlebar_island_vertical_geometry`: growing from 28 to 32 points about
-- the controls' middle). They never move; only the island comes and goes.
--
-- `TitleBar { collapsed = state, on_back = …, can_forward = … }` (placed
-- over the window's top, the window's width). `lights = true` draws the
-- window's buttons too (a preview has no real ones). `actions`: the page's
-- own controls at the bar's other end (a files pane's toggle), on the same
-- line, at the same size (`TitleBar.control`).
--
-- Where the system has no menu bar (Linux, Windows), the app's menus are
-- the host's (`app.menus`, src/plugins/host_menu.rs): a ☰ before the
-- sidebar's toggle opens them; Alt pressed and released alone (the host's
-- `app.menu_alt`) turns the controls into the menus' titles in place, the
-- first focused: Left and Right go between them, Enter or Down opens one,
-- Escape, a click elsewhere or Alt again turns them back.

local Icon = require("@self/icon")
local SidebarIcon = require("@self/sidebar_icon")
local island = require("@self/island")
local IconButton = require("@self/icon_button").IconButton
local motion = require("@self/motion")
local locale = require("@self/locale")
local ContextMenu = require("@self/context_menu")

-- (Read-only: a call through the module's `__call` checks its table
-- exactly otherwise.)
export type Props = {
	read collapsed: State<boolean>,
	-- Its sidebar toggle pressed: the caller's (a sidebar hidden for room
	-- comes back rather than closing). Else it flips `collapsed`.
	read on_toggle: (() -> ())?,
	read can_back: (() -> boolean)?,
	read can_forward: (() -> boolean)?,
	read on_back: (() -> ())?,
	read on_forward: (() -> ())?,
	read lights: boolean?,
	-- When the island shows: Zeron's shows only while the controls stand
	-- over artwork (the sidebar collapsed, on a new session's canvas, a
	-- wallpaper set). By default, whenever the sidebar is collapsed.
	read island: (() -> boolean)?,
	-- At the bar's right end, centred on its line: the page's controls.
	read actions: Node?,
	-- Whether they stand over artwork (a new chat's wallpaper): then, while
	-- they hold anything, an island of glass gathers behind them, as behind
	-- the controls at its left, so they read on any picture.
	read actions_island: (() -> boolean)?,
	-- Where the actions' width is kept (a named state of the caller's, so it
	-- outlives a reload of the scripts); its own otherwise.
	read actions_width: State<number>?,
}

-- The app's menus, as the host serves them where the system has none
-- (nil on macOS, whose menu bar has them, and with no host: a preview).
export type MenuEntry = {
	label: string?,
	id: string?,
	keys: string?,
	checked: boolean?,
	disabled: boolean?,
	separator: boolean?,
	submenu: { MenuEntry }?,
}
export type AppMenu = { label: string, entries: { MenuEntry } }
local app_menus = ui.shared("app.menus", nil :: { AppMenu }?)
-- True while Alt has the controls show the menus' titles: the host sets it
-- as Alt is tapped, the bar clears it as it puts them away.
local menu_alt = ui.shared("app.menu_alt", false)

local function menus(): { AppMenu }?
	local list = app_menus:get()
	return if type(list) == "table" and #list > 0 then list else nil
end

-- The ☰'s room before the toggle: a control and a group's gap, where the
-- host serves the menus.
local function menu_room(): number
	if menus() == nil then
		return 0
	end
	local m = island.metrics()
	return m.control + m.group
end

-- A menu's entries as the menus' own rows (ds/context_menu); `chosen` runs
-- after an item's.
local function entries_of(list: { MenuEntry }, chosen: (() -> ())?): { ContextMenu.Entry }
	local out: { ContextMenu.Entry } = {}
	for _, e in list do
		if e.separator then
			table.insert(out, { separator = true })
		elseif e.submenu then
			table.insert(out, { label = e.label, submenu = entries_of(e.submenu, chosen) })
		elseif e.id then
			local id, checked = e.id :: string, e.checked
			table.insert(out, {
				label = e.label,
				shortcut = e.keys,
				disabled = e.disabled,
				checked = if checked ~= nil then function(): boolean
					return checked :: boolean
				end else nil,
				on_select = function()
					if chosen then
						chosen()
					end
					-- The host runs it, as its menu bar would (a script's
					-- command, or the way back to the default interface).
					pcall(ui.invoke, "app.menu_choose", id)
				end,
			})
		end
	end
	return out
end

-- Zeron's metrics. The bar: 38 tall, its row padded 4 at the top, so its
-- controls centre 21 down.
local H, TOP_PAD = 38, 4
local MIDDLE = (H + TOP_PAD) / 2
-- macOS's buttons, as macOS 26 draws them (measured off Zeron's window): 14
-- across, 23 apart, the first centred 21 in, so 21, 44 and 67.
local LIGHT, LIGHT_FIRST, LIGHT_PITCH = 14, 21, 23
-- The toggle: 88 in (Zeron's `titlebar_cluster_start`). The controls, their
-- gaps and the island round them are the theme's island (`zeron.island`),
-- read as the bar is built; its row runs 10 past its last control (Zeron's
-- `TITLEBAR_CLUSTER_PAD`).
local TOGGLE_IN, ROW_PAD = 88, 10

-- How far in from the window's right edge the window's own buttons reach:
-- Windows' caption buttons, and on Linux the buttons zgx draws where the
-- desktop's `button-layout` asks (GNOME's `appmenu:close`, close at the
-- right). 0 where they stand at the left (macOS's traffic lights, which
-- TOGGLE_IN already clears), are not reported, or there are none. A plugin
-- reads no `ui.window.controls`: 0.
local function window_buttons_right(): number
	local window = ui.window :: any
	local area_state, controls_state = window.titlebar_area, window.controls
	if area_state == nil or controls_state == nil then
		return 0
	end
	local area, buttons = area_state:get(), controls_state:get()
	if area and buttons and buttons.x >= area.x + area.width then
		return buttons.width
	end
	return 0
end

-- A bar control: the design system's icon button at the island's size,
-- a quiet glyph, a wash under the pointer; when it can't (nothing to go
-- back or forward to), muted and still. Its tooltip hangs below it: above
-- is the window's edge.
local function control(label: Label, glyph: Node, enabled: (() -> boolean)?, on_click: (() -> ())?): Node
	local function able(): boolean
		return enabled == nil or enabled()
	end
	local size = island.metrics().control
	return IconButton {
		label = label,
		w = size,
		h = size,
		tooltip_side = "bottom",
		disabled = function(): boolean
			return not able()
		end,
		on_click = function()
			if able() and on_click then
				on_click()
			end
		end,
		glyph,
	}
end

-- The ☰: every menu, each a submenu, in the menus' own panel.
local function menu_button(): Node
	local open = ui.use_state(false)
	return ContextMenu {
		open = open,
		label = locale.t("ds.title_bar.menu"),
		items = function(): { ContextMenu.Entry }?
			local out: { ContextMenu.Entry } = {}
			for _, m in menus() or {} do
				table.insert(out, { label = m.label, submenu = entries_of(m.entries) })
			end
			return out
		end,
		control(locale.t("ds.title_bar.menu"), Icon { name = "hamburger-menu", size = 16 }, nil, function()
			open:set(not open:peek())
		end),
	}
end

-- The menus' titles in the controls' place, while Alt has them: a quiet
-- pill each, as the controls are, opening its menu below it.
local AltMenus = ui.component(function(props: { width: State<number> }): Node
	local size = island.metrics().control
	-- (The menus as Alt came: a change while they show waits for the next.)
	local served = app_menus:peek()
	local list: { AppMenu } = if type(served) == "table" then served else {}
	local refs: { any } = {}
	local opens: { State<boolean> } = {}
	local focused = 0
	local appear = ui.use_state(0)
	local function leave()
		if menu_alt:peek() then
			menu_alt:set(false)
		end
	end
	local function any_open(): boolean
		for _, o in opens do
			if o:peek() then
				return true
			end
		end
		return false
	end
	local function focus(i: number)
		local el = refs[i] and refs[i]:peek()
		if el then
			el:focus()
		end
	end
	-- Put away once the keyboard and the menus have both left them (a click
	-- elsewhere, Tab away): a frame later, as focus may be moving between
	-- them.
	local function check()
		ui.after(0, function()
			if focused == 0 and not any_open() then
				leave()
			end
		end)
	end
	local pills: { Node } = {}
	for i, m in list do
		local open = ui.use_state(false)
		opens[i] = open
		local ref = ui.ref()
		refs[i] = ref
		ui.effect(function()
			if not open:get() then
				check()
			end
		end)
		table.insert(pills, ContextMenu {
			open = open,
			label = m.label,
			items = function(): { ContextMenu.Entry }?
				return entries_of(m.entries, leave)
			end,
			ui.button {
				ref = ref,
				label = m.label,
				h = size,
				px = "$space.2",
				shrink = 0,
				items = "center",
				rounded = "$zeron.icon_button.radius",
				cursor = "pointer",
				color = "$text.secondary",
				bg = function(): string
					return if open:get() then "$pressed" else "#00000000"
				end,
				hover = { bg = "$hover", color = "$text.primary" },
				active = { bg = "$pressed", color = "$text.primary" },
				text_size = "$font.size.md",
				weight = "$font.weight.medium",
				on_click = function()
					open:set(not open:peek())
				end,
				on_focus = function()
					focused += 1
				end,
				on_blur = function()
					focused = math.max(0, focused - 1)
					check()
				end,
				on_key = function(e: KeyEvent): boolean
					local n = #list
					if e.key == "ArrowRight" then
						focus(i % n + 1)
					elseif e.key == "ArrowLeft" then
						focus((i - 2) % n + 1)
					elseif e.key == "ArrowDown" then
						open:set(true)
					elseif e.key == "Escape" then
						leave()
					else
						return false
					end
					return true
				end,
				ui.text { m.label },
			},
		})
	end
	-- Away as the window goes to the back.
	ui.effect(function()
		if ui.window.active:get() == false then
			leave()
		end
	end)
	ui.after(0, function()
		appear:set(1)
		focus(1)
	end)
	return ui.row {
		id = "titlebar-menus",
		role = "toolbar",
		label = locale.t("ds.title_bar.menus"),
		h = size,
		items = "center",
		gap = "$zeron.island.gap",
		opacity = ui.tween(appear, { duration = motion.ms(motion.FADE), ease = "out(3)" }),
		on_resize = function(s)
			if props.width:peek() ~= s.w then
				props.width:set(s.w)
			end
		end,
		pills,
	}
end)

local TitleBar = ui.component(function(props: Props): Node
	local m = island.metrics()
	local CONTROL, GROUP_GAP, CONTROL_GAP = m.control, m.group, m.gap
	-- The island: from the window's inset to the control row's end; as tall
	-- as an island (a control and its padding), a padding's worth less as it
	-- comes (Zeron's 28 to 32).
	local ISLAND_LEFT = m.inset
	-- The menus' titles' width, while Alt has them in the controls' place.
	local alt_w = ui.use_state(0)
	local function alt_on(): boolean
		return menu_alt:get() == true and menus() ~= nil and not props.lights
	end
	local function island_right(): number
		local controls = TOGGLE_IN + menu_room() + CONTROL + GROUP_GAP + CONTROL + CONTROL_GAP + CONTROL + ROW_PAD
		return if alt_on() then math.max(controls, TOGGLE_IN + alt_w:get() + ROW_PAD) else controls
	end
	local ISLAND_TO = island.thickness()
	local ISLAND_FROM = ISLAND_TO - m.pad
	local collapsed = props.collapsed
	local function shown(): boolean
		return if props.island then props.island() else collapsed:get()
	end
	local here = ui.computed(function(): number
		return if shown() then 1 else 0
	end)
	-- (Moves land at once with motion reduced: ds/motion.)
	local island_h = motion.tween(ui.computed(function(): number
		return if shown() then ISLAND_TO else ISLAND_FROM
	end), motion.sidebar)
	local lights: { Node } = {}
	if props.lights then
		for i, hue in { "red", "amber", "green" } do
			table.insert(
				lights,
				ui.div {
					absolute = true,
					left = LIGHT_FIRST + (i - 1) * LIGHT_PITCH - LIGHT / 2,
					top = MIDDLE - LIGHT / 2,
					w = LIGHT,
					h = LIGHT,
					rounded = "$radius.full",
					bg = `$palette.{hue}.400`,
				}
			)
		end
	end
	local bar: { [any]: any } = {
		w_full = true,
		-- (A strip of no height its controls hang from, each placed on the
		-- bar's line: a box the bar's size, even one passing the pointer
		-- through, stands in front of what's under it for the pointer's
		-- cursor, and the panel's tabs, a split pane's ×, the top of a
		-- resize edge showed none of theirs. The bar's room is `H`.)
		h = 0,
		overflow = "visible",
		relative = true,
		-- The bar lets the pointer through to what is under it, but for its
		-- controls.
		pointer_events = "none",
		-- The island: glass behind the buttons and controls as the sidebar
		-- goes, centred on them as it grows; gone as it comes back.
	}
	local left_island: ContainerProps = {
		absolute = true,
		left = ISLAND_LEFT,
		w = function(): number
			return island_right() - ISLAND_LEFT
		end,
		top = motion.tween(ui.computed(function(): number
			return MIDDLE - (if shown() then ISLAND_TO else ISLAND_FROM) / 2
		end), motion.sidebar),
		h = island_h,
		opacity = ui.tween(here, { duration = 220, ease = "out(3)" }),
	}
	island.glass(left_island)
	table.insert(bar, ui.div(left_island))
	-- (Appended, not unpacked mid-table: Luau would keep only the first.)
	for _, light in lights do
		table.insert(bar, light)
	end
	-- (Its own controls first, then the page's: Tab reads the bar left to
	-- right.)
	table.insert(
		bar,
		ui.row {
			absolute = true,
			left = TOGGLE_IN,
			top = MIDDLE - CONTROL / 2,
			items = "center",
			-- (Under Alt the menus' titles stand in their place.)
			pointer_events = function(): string
				return if alt_on() then "none" else "auto"
			end,
			opacity = ui.tween(ui.computed(function(): number
				return if alt_on() then 0 else 1
			end), { duration = motion.ms(motion.FADE), ease = "out(3)" }),
			-- (Between its controls, the bar is still the window's to move:
			-- they keep their presses, the gaps between them move it.)
			window_drag = true,
			ui.show(function(): boolean
				return menus() ~= nil and not props.lights
			end, ui.row { items = "center", menu_button(), ui.div { w = GROUP_GAP, shrink = 0 } }),
			control(
				locale.t("ds.title_bar.toggle_sidebar"),
				SidebarIcon {
					open = ui.computed(function(): boolean
						return not collapsed:get()
					end) :: any,
					size = 16,
				},
				nil,
				function()
					if props.on_toggle then
						props.on_toggle()
					else
						collapsed:set(not collapsed:peek())
					end
				end
			),
			ui.div { w = GROUP_GAP, shrink = 0 },
			control(locale.t("common.back"), Icon { name = "arrow-left", size = 16 }, props.can_back, props.on_back),
			ui.div { w = CONTROL_GAP, shrink = 0 },
			control(locale.t("ds.title_bar.forward"), Icon { name = "arrow-right", size = 16 }, props.can_forward, props.on_forward),
		}
	)
	-- Alt's: the menus' titles where the controls are.
	table.insert(
		bar,
		ui.show(
			alt_on,
			ui.row {
				absolute = true,
				left = TOGGLE_IN,
				top = MIDDLE - CONTROL / 2,
				h = CONTROL,
				items = "center",
				pointer_events = "auto",
				window_drag = true,
				AltMenus { width = alt_w },
			}
		)
	)
	if props.actions then
		-- Over artwork, its island: the left island's glass and height,
		-- round the row as far as the row stands in from the window's edge
		-- (the island's inset from it, as at the left), there while the row
		-- holds anything.
		local row_ref = ui.ref()
		-- (Its last width, kept across a reload when the caller keeps it: the
		-- row is 0 wide until laid out again, and the island would fade in
		-- anew.)
		local kept_w = props.actions_width or ui.use_state(0)
		local function row_w(): number
			return row_ref.width or kept_w:get()
		end
		local function holds(): boolean
			return row_w() > 1
		end
		local on_art = ui.computed(function(): number
			return if props.actions_island and props.actions_island() and holds() then 1 else 0
		end)
		local side = ROW_PAD - m.inset
		local actions_island: ContainerProps = {
			id = "titlebar-actions-island",
			absolute = true,
			right = function(): number
				return m.inset + window_buttons_right()
			end,
			w = function(): number
				return row_w() + side * 2
			end,
			top = motion.tween(ui.computed(function(): number
				return MIDDLE - (if on_art:get() == 1 then ISLAND_TO else ISLAND_FROM) / 2
			end), { duration = 260, ease = "out(3)" }),
			h = motion.tween(ui.computed(function(): number
				return if on_art:get() == 1 then ISLAND_TO else ISLAND_FROM
			end), { duration = 260, ease = "out(3)" }),
			opacity = ui.tween(on_art, { duration = 220, ease = "out(3)" }),
		}
		island.glass(actions_island)
		table.insert(bar, ui.div(actions_island))
		-- The page's controls, as far in from the right as the row runs past
		-- its last control at the left, and clear of the window's own
		-- buttons where they stand at the right.
		table.insert(
			bar,
			ui.row {
				ref = row_ref,
				on_resize = function(size)
					if kept_w:peek() ~= size.w then
						kept_w:set(size.w)
					end
				end,
				absolute = true,
				right = function(): number
					return ROW_PAD + window_buttons_right()
				end,
				top = MIDDLE - CONTROL / 2,
				h = CONTROL,
				items = "center",
				gap = ui.raw(CONTROL_GAP, "the island's gap between its controls, counted in its width"),
				pointer_events = "auto",
				-- (Between its controls, the bar is still the window's to move.)
				window_drag = true,
				props.actions,
			}
		)
	end
	return ui.div(bar)
end)

-- A control for the bar's `actions`: its size, a quiet glyph, its tooltip
-- below (`enabled`, `on_click` as the bar's own).
-- Where the bar's controls end, from the window's left (its island's
-- right edge): what lies on the title line beside them starts past it.
local function controls_end(): number
	local m = island.metrics()
	return TOGGLE_IN + menu_room() + m.control + m.group + m.control + m.gap + m.control + ROW_PAD
end

local M = setmetatable({ control = control, controls_end = controls_end, window_buttons_right = window_buttons_right }, {
	__call = function(_, props: Props): Node
		return TitleBar(props)
	end,
})
return M
