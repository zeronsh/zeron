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

local Icon = require("@self/icon")
local SidebarIcon = require("@self/sidebar_icon")
local island = require("@self/island")
local IconButton = require("@self/icon_button").IconButton
local motion = require("@self/motion")
local locale = require("@self/locale")

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

local TitleBar = ui.component(function(props: Props): Node
	local m = island.metrics()
	local CONTROL, GROUP_GAP, CONTROL_GAP = m.control, m.group, m.gap
	-- The island: from the window's inset to the control row's end; as tall
	-- as an island (a control and its padding), a padding's worth less as it
	-- comes (Zeron's 28 to 32).
	local ISLAND_LEFT = m.inset
	local ISLAND_RIGHT = TOGGLE_IN + CONTROL + GROUP_GAP + CONTROL + CONTROL_GAP + CONTROL + ROW_PAD
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
		h = H,
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
		w = ISLAND_RIGHT - ISLAND_LEFT,
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
			pointer_events = "auto",
			-- (Between its controls, the bar is still the window's to move:
			-- they keep their presses, the gaps between them move it.)
			window_drag = true,
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
			right = m.inset,
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
		-- its last control at the left.
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
				right = ROW_PAD,
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
	return TOGGLE_IN + m.control + m.group + m.control + m.gap + m.control + ROW_PAD
end

local M = setmetatable({ control = control, controls_end = controls_end }, {
	__call = function(_, props: Props): Node
		return TitleBar(props)
	end,
})
return M
