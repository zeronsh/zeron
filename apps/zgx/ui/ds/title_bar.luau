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
-- On macOS the toggle stands this far past the traffic lights' right edge
-- (`ui.window.controls`: 12 in and 60 across in zgx's inset title bar, so
-- 88 in, as ever). In full screen macOS hides them, and the controls take
-- the window's edge inset (Zeron's `titlebar_cluster_start(true)`). Off
-- macOS the bar keeps its 88.
local LIGHTS_GAP, FULL_SCREEN_IN = 16, 12
-- How they slide over: as Zeron's do (`titlebar_tween`, 200 ms, ease out),
-- once the system's animation is over. The window draws no frames while
-- macOS animates it in or out (it shows a picture of the window meanwhile:
-- zgx's frames say "the window stopped presenting" for its 0.35 s going in,
-- and as long coming out), so a slide timed to it is never seen; nothing
-- moves until it's done, then they slide from where they stood.
local FULL_SCREEN_SLIDE = { duration = 200, ease = "cubicBezier(0, 0, 0.58, 1)" }

-- A state of the window's (`ui.window.<name>`), or nil where there's none
-- (a plugin, lent the design system, has no window of its own).
local function window_state(name: string): State<any>?
	local window = (ui :: any).window
	return if window then window[name] else nil
end
local function read(state: State<any>?, tracked: boolean): any
	if state == nil then
		return nil
	end
	return if tracked then state:get() else state:peek()
end

-- Whether the window is on macOS, as the host says (src/main.rs
-- `platform`): its buttons the traffic lights at the bar's left. Asked once.
local macos: boolean? = nil
local function on_macos(): boolean
	if macos == nil then
		local ok, platform = pcall(function(): any
			return ui.shared("platform", ""):peek()
		end)
		macos = ok and platform == "macos"
	end
	return macos :: boolean
end

-- Whether the window is in full screen (tracked).
local function full_screen(): boolean
	return read(window_state("full_screen"), true) == true
end

-- Where the toggle stands from the window's left, in full screen (`full`)
-- or out of it: past the traffic lights, or at the edge once they're hidden.
-- (Leaving full screen, the lights aren't reported again until the
-- system's animation ends: 88 meanwhile, where they put it.)
local function toggle_in(full: boolean, tracked: boolean?): number
	if not on_macos() then
		return TOGGLE_IN
	end
	if full then
		return FULL_SCREEN_IN
	end
	local lights = read(window_state("controls"), tracked ~= false)
	return if lights then lights.x + lights.width + LIGHTS_GAP else TOGGLE_IN
end

-- The row's controls' width, past the toggle's start, to the row's end.
local function controls_w(): number
	local m = island.metrics()
	return m.control + m.group + m.control + m.gap + m.control + ROW_PAD
end

-- Whether the window is in full screen, untracked (a value to start from).
local function full_screen_now(): boolean
	return read(window_state("full_screen"), false) == true
end

-- `target`'s props kept where each says (`at[prop](full)`, read tracked: the
-- window in full screen or not, the lights, whatever else it reads), by the
-- engine: still while the window goes in or out of full screen, then
-- sliding over as Zeron's controls do (`FULL_SCREEN_SLIDE`); otherwise eased
-- by `settle` (a tween's options) or set at once. Nothing runs per frame
-- and nothing at rest: the node's own style gives them a plain number to
-- start from (`at` as it stands then), never a function (an animated style
-- runs its node's functions again each frame). With motion reduced they
-- land at once.
local function follow(target: any, at: { [string]: (full: boolean) -> number }, settle: { duration: number, ease: string }?)
	local moving = window_state("full_screen_transition")
	-- Where they stand, and whether full screen's move is held for its slide.
	local last: { [string]: number }? = nil
	local held = false
	local function go(to: { [string]: number }, timing: { duration: number, ease: string }?)
		local from = last :: { [string]: number }
		last = to
		if timing == nil or motion.reduced_now() then
			anime.utils.set(target, to :: any)
			return
		end
		local params: { [string]: any } = { duration = timing.duration, ease = timing.ease }
		for prop, v in to do
			params[prop] = { from[prop], v }
		end
		anime.animate(target, params :: any)
	end
	ui.effect(function()
		local full = full_screen()
		local now: { [string]: number } = {}
		for prop, f in at do
			now[prop] = f(full)
		end
		-- (The first: the node's own style already says it.)
		if last == nil then
			last = now
			return
		end
		-- Held while macOS animates (what it reports meanwhile is never
		-- seen: coming out, the lights stand at 9,9 for a moment).
		if read(moving, true) ~= nil then
			held = true
			return
		end
		local changed = false
		for prop, v in now do
			if (last :: any)[prop] ~= v then
				changed = true
			end
		end
		if not changed then
			return
		end
		if held then
			-- Once what macOS says as it ends has settled (the lights come
			-- back a moment after the transition ends; a change meanwhile
			-- runs this again and waits anew).
			ui.after(0.05, function()
				held = false
				go(now, FULL_SCREEN_SLIDE)
			end)
			return
		end
		go(now, settle)
	end)
end

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

local TitleBar = ui.component(function(props: Props): Node
	local m = island.metrics()
	local CONTROL, GROUP_GAP, CONTROL_GAP = m.control, m.group, m.gap
	-- The island: from the window's inset to the control row's end; as tall
	-- as an island (a control and its padding), a padding's worth less as it
	-- comes (Zeron's 28 to 32). Its end follows the row (full screen).
	local ISLAND_LEFT = m.inset
	local function island_w(full: boolean, tracked: boolean?): number
		return toggle_in(full, tracked) + controls_w() - ISLAND_LEFT
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
	local island_ref = ui.ref()
	local left_island: ContainerProps = {
		ref = island_ref,
		absolute = true,
		left = ISLAND_LEFT,
		-- (Kept by `follow` below, from here.)
		w = island_w(full_screen_now(), false),
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
	local row_ref = ui.ref()
	follow(row_ref, {
		left = function(full: boolean): number
			return toggle_in(full)
		end,
	})
	follow(island_ref, {
		w = function(full: boolean): number
			return island_w(full)
		end,
	})
	table.insert(
		bar,
		ui.row {
			ref = row_ref,
			absolute = true,
			-- (Kept by `follow` above, from here.)
			left = toggle_in(full_screen_now(), false),
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
-- Now (in full screen they stand further left), or in full screen or out
-- of it (`full`); read tracked unless `tracked` is false.
local function controls_end(full: boolean?, tracked: boolean?): number
	local at = if full == nil then (if tracked == false then full_screen_now() else full_screen()) else full
	return toggle_in(at, tracked ~= false) + controls_w()
end

local M = setmetatable({
	control = control,
	controls_end = controls_end,
	window_buttons_right = window_buttons_right,
	-- What lies beside the controls follows them: `follow(ref, { prop =
	-- function(full) … end }, settle?)`, with `controls_end(full)`, its
	-- prop's own style a plain number to start from (`full_screen_now()`).
	follow = follow,
	full_screen_now = full_screen_now,
}, {
	__call = function(_, props: Props): Node
		return TitleBar(props)
	end,
})
return M
