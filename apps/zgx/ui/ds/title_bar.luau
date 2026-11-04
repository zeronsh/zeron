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
-- The toggle: 88 in beside the traffic lights (Zeron's
-- `titlebar_cluster_start`), 10 in with nothing at the left (the shipping
-- app's elsewhere). The controls, their gaps and the island round them are
-- the theme's island (`zeron.island`), read as the bar is built; its row
-- runs 10 past its last control (Zeron's `TITLEBAR_CLUSTER_PAD`).
local TOGGLE_IN, ROW_PAD = 88, 10

-- The window's own buttons as the window reports them, and the side they
-- stand at: the left (macOS's traffic lights; on Linux a `close:…` layout)
-- or the right (Windows' caption buttons; GNOME's `appmenu:close`). Nil
-- where none are reported: in full screen, in a plugin (which reads no
-- `ui.window.controls`) and headless.
type Side = "left" | "right"
local function window_buttons(): (Side?, any)
	local window = ui.window :: any
	local area_state, controls_state = window.titlebar_area, window.controls
	if area_state == nil or controls_state == nil then
		return nil, nil
	end
	local area, buttons = area_state:get(), controls_state:get()
	if not area or not buttons then
		return nil, nil
	end
	return if buttons.x >= area.x + area.width then "right" else "left", buttons
end

-- How far in from the window's right edge the window's own buttons reach;
-- 0 where they stand at the left, or none are reported.
local function window_buttons_right(): number
	local side, buttons = window_buttons()
	return if side == "right" then buttons.width else 0
end

-- How far in from the window's right edge the page's controls (the bar's
-- `actions`) end: their row's padding; beside the window's own buttons at
-- the right, past them and a divider standing a group's gap either side of
-- it (Zeron's controls one group, the window's another).
local DIVIDER = 1
local function actions_right(): number
	local side, buttons = window_buttons()
	if side == "right" then
		return buttons.width + island.metrics().group * 2 + DIVIDER
	end
	return ROW_PAD
end

-- Where the toggle stands from the window's left: past the window's buttons
-- there (never nearer than beside the traffic lights), 10 in with them at
-- the right. None reported (full screen, a plugin), beside the lights.
local function toggle_in(): number
	local side, buttons = window_buttons()
	if side == "right" then
		return ROW_PAD
	elseif side == "left" then
		return math.max(TOGGLE_IN, buttons.x + buttons.width)
	end
	return TOGGLE_IN
end

-- The title line's middle, from the window's top: where everything on the
-- bar centres. 21, as beside the traffic lights; where the window's buttons
-- stand at the right in a strip shorter than the bar (Windows' 32-point
-- caption buttons), on their middle, so Zeron's controls and theirs share
-- one line; never nearer the window's top than an island's middle, its
-- controls held their padding in from the edge (zgx reports the buttons
-- shorter in a zoomed window than it draws them: zgx-asks/
-- windows-caption-buttons).
local function middle(): number
	local side, buttons = window_buttons()
	if side == "right" and buttons.height < H then
		return math.max(buttons.y + buttons.height / 2, island.thickness() / 2)
	end
	return MIDDLE
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

-- The bar's own controls' row, its width: the toggle, then back and
-- forward.
local function row_width(m: island.Metrics): number
	return m.control + m.group + m.control + m.gap + m.control
end

-- Where the bar's controls end, from the window's left (its island's
-- right edge): what lies on the title line beside them starts past it.
-- Beside the window's buttons, the row runs ROW_PAD past its last control;
-- alone at the left, its island holds it evenly, as far past its end as
-- before its start.
local function controls_end(): number
	local m = island.metrics()
	local start = toggle_in()
	local tail = if start == ROW_PAD then ROW_PAD - m.inset else ROW_PAD
	return start + row_width(m) + tail
end

local TitleBar = ui.component(function(props: Props): Node
	local m = island.metrics()
	local CONTROL, GROUP_GAP, CONTROL_GAP = m.control, m.group, m.gap
	-- The island: from the window's inset to the control row's end; as tall
	-- as an island (a control and its padding), a padding's worth less as it
	-- comes (Zeron's 28 to 32).
	local ISLAND_LEFT = m.inset
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
		w = function(): number
			return controls_end() - ISLAND_LEFT
		end,
		top = motion.tween(ui.computed(function(): number
			return middle() - (if shown() then ISLAND_TO else ISLAND_FROM) / 2
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
			left = toggle_in,
			top = function(): number
				return middle() - CONTROL / 2
			end,
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
		-- The window's own buttons where they stand at the right (Windows'
		-- caption buttons): over artwork they need the glass too, thin glyphs
		-- on a picture, so the island takes them in, hanging from the
		-- window's corner as their strip does (flush with its top and its
		-- edge, the window's own corner rounding it there), its inner foot
		-- rounded; and shows whether the row holds anything or not.
		local function caption(): any
			local at, buttons = window_buttons()
			return if at == "right" then buttons else nil
		end
		local on_art = ui.computed(function(): number
			return if props.actions_island and props.actions_island() and (holds() or caption() ~= nil) then 1 else 0
		end)
		local side = ROW_PAD - m.inset
		local function hung(): any
			return if caption() ~= nil then 0 else "$zeron.island.radius"
		end
		local actions_island: ContainerProps = {
			id = "titlebar-actions-island",
			absolute = true,
			right = function(): number
				return if caption() ~= nil then 0 else m.inset
			end,
			w = function(): number
				if caption() ~= nil then
					return if holds() then actions_right() + row_w() + side else caption().width
				end
				return row_w() + side * 2
			end,
			top = motion.tween(ui.computed(function(): number
				local buttons = caption()
				if buttons then
					return buttons.y
				end
				return middle() - (if on_art:get() == 1 then ISLAND_TO else ISLAND_FROM) / 2
			end), { duration = 260, ease = "out(3)" }),
			h = motion.tween(ui.computed(function(): number
				local buttons = caption()
				if buttons then
					return buttons.height
				end
				return if on_art:get() == 1 then ISLAND_TO else ISLAND_FROM
			end), { duration = 260, ease = "out(3)" }),
			opacity = ui.tween(on_art, { duration = 220, ease = "out(3)" }),
		}
		island.glass(actions_island)
		actions_island.rounded_tl = hung
		actions_island.rounded_tr = hung
		actions_island.rounded_br = hung
		actions_island.rounded_bl = "$zeron.island.radius"
		table.insert(bar, ui.div(actions_island))
		-- Between the page's controls and the window's buttons at the right, a
		-- hairline on the line: two groups, as the left cluster's are, read
		-- apart. There while the row holds anything.
		table.insert(
			bar,
			ui.div {
				id = "titlebar-divider",
				absolute = true,
				right = function(): number
					local buttons = caption()
					return if buttons then buttons.width + m.group else 0
				end,
				top = function(): number
					return middle() - CONTROL / 3
				end,
				w = ui.raw(DIVIDER, "a hairline"),
				h = ui.raw(CONTROL * 2 / 3, "two thirds of a control, on its line"),
				bg = "$border.subtle",
				pointer_events = "none",
				opacity = function(): number
					return if caption() ~= nil and holds() then 1 else 0
				end,
			}
		)
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
				right = actions_right,
				top = function(): number
					return middle() - CONTROL / 2
				end,
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

-- `control`: a control for the bar's `actions` (its size, a quiet glyph,
-- its tooltip below; `enabled`, `on_click` as the bar's own).
-- `controls_end()`, `window_buttons_right()`, `actions_right()`, `middle()`: where what lies on
-- the title line beside the bar stands (read in a binding: they follow the
-- window).
local M = setmetatable({
	control = control,
	controls_end = controls_end,
	window_buttons_right = window_buttons_right,
	actions_right = actions_right,
	middle = middle,
}, {
	__call = function(_, props: Props): Node
		return TitleBar(props)
	end,
})
return M
