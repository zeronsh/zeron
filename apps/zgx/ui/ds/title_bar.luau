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
	-- Whether the page's art reaches under the window's own buttons (v2's, to the
	-- window's edges), so the island takes them in too. In a card under the bar, it
	-- doesn't.
	read caption_over_art: boolean?,
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

-- The window's own buttons as the window reports them, and the side they
-- stand at: the left (macOS's traffic lights; on Linux a `close:…` layout)
-- or the right (Windows' caption buttons; GNOME's `appmenu:close`). Nil
-- where none are reported: in full screen, in a plugin (which reads no
-- `ui.window.controls`) and headless. Read tracked unless `tracked` is
-- false.
type Side = "left" | "right"
local function window_buttons(tracked: boolean?): (Side?, any)
	local area = read(window_state("titlebar_area"), tracked ~= false)
	local buttons = read(window_state("controls"), tracked ~= false)
	if not area or not buttons then
		return nil, nil
	end
	return if buttons.x >= area.x + area.width then "right" else "left", buttons
end

-- Where the toggle stands from the window's left, in full screen (`full`)
-- or out of it. On macOS: past the traffic lights, or at the edge once
-- they're hidden. (Leaving full screen, the lights aren't reported again
-- until the system's animation ends: 88 meanwhile, where they put it.)
-- Elsewhere, by where the window's buttons stand: 10 in with them at the
-- right (the shipping app's), past them at the left (never nearer than
-- the lights' 88), 88 with none reported.
local function toggle_in(full: boolean, tracked: boolean?): number
	if not on_macos() then
		local side, buttons = window_buttons(tracked)
		if side == "right" then
			return ROW_PAD
		elseif side == "left" then
			return math.max(TOGGLE_IN, buttons.x + buttons.width)
		end
		return TOGGLE_IN
	end
	if full then
		return FULL_SCREEN_IN
	end
	local lights = read(window_state("controls"), tracked ~= false)
	return if lights then lights.x + lights.width + LIGHTS_GAP else TOGGLE_IN
end

-- The row's controls' width, past the toggle's start (`start`), to the
-- row's end: ROW_PAD past its last control; alone at the window's left
-- (off macOS, the window's buttons at the right), its island holds it
-- evenly, as far past its end as before its start.
local function controls_tail(start: number): number
	return if not on_macos() and start == ROW_PAD then ROW_PAD - island.metrics().inset else ROW_PAD
end
local function controls_w(start: number): number
	local m = island.metrics()
	return m.control + m.group + m.control + m.gap + m.control + controls_tail(start)
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

-- How far in from the window's right edge the window's own buttons reach;
-- 0 where they stand at the left, or none are reported.
local function window_buttons_right(): number
	local side, buttons = window_buttons()
	return if side == "right" then buttons.width else 0
end

-- Windows' caption glyph (zgx's: a 10-point square in each button) and how
-- much of a control's box its glyph inks (the island's 16-point glyphs, the
-- panel toggle's frame among them, are 12 across): what the page's controls
-- are spaced from the window's buttons by.
local CAPTION_GLYPH, CONTROL_INK = 10, 12

-- How far in from the window's right edge the page's controls (the bar's
-- `actions`) end: their row's padding; beside the window's own buttons at
-- the right, past them by one rhythm, measured by ink: from the last
-- control's glyph to Minimize's as far as from one caption glyph to the
-- next (a button's width less its glyph), so the two read as one group.
local function actions_right(): number
	local side, buttons = window_buttons()
	-- (Linux's buttons, circles in a strip the bar's height or more, keep
	-- the row's padding before them.)
	if side == "right" and buttons.height >= H then
		return buttons.width + ROW_PAD
	end
	if side == "right" then
		-- (Three buttons: the main window's, which has the page's controls.)
		local pitch = buttons.width / 3
		local ink_gap = pitch - CAPTION_GLYPH
		-- Minimize's glyph starts half its button's spare room in; the
		-- last control's ink ends half its own spare room in from its box.
		local into_caption = (pitch - CAPTION_GLYPH) / 2
		local into_control = (island.metrics().control - CONTROL_INK) / 2
		return buttons.width + ink_gap - into_caption - into_control
	end
	return ROW_PAD
end

-- The title line's middle, from the window's top: where everything on the
-- bar centres. 21, as beside the traffic lights; where the window's buttons
-- stand at the right in a strip shorter than the bar (Windows' 32-point
-- caption buttons), on their middle, so Zeron's controls and theirs share
-- one line; never nearer the window's top than an island's middle, its
-- controls held their padding in from the edge (zgx reports the buttons
-- shorter in a zoomed window than it draws them: zgx-asks/
-- windows-caption-buttons). Read tracked unless `tracked` is false.
local function middle(tracked: boolean?): number
	local side, buttons = window_buttons(tracked)
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

local TitleBar = ui.component(function(props: Props): Node
	local m = island.metrics()
	local CONTROL, GROUP_GAP, CONTROL_GAP = m.control, m.group, m.gap
	-- The island: from the window's inset to the control row's end; as tall
	-- as an island (a control and its padding), a padding's worth less as it
	-- comes (Zeron's 28 to 32). Its end follows the row (full screen).
	local ISLAND_LEFT = m.inset
	local function island_w(full: boolean, tracked: boolean?): number
		local start = toggle_in(full, tracked)
		return start + controls_w(start) - ISLAND_LEFT
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
	local row_ref = ui.ref()
	follow(row_ref, {
		left = function(full: boolean): number
			return toggle_in(full)
		end,
		-- (On the bar's line: the window's buttons' where they set it.)
		top = function(): number
			return middle() - CONTROL / 2
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
			top = middle(false) - CONTROL / 2,
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
			return if props.actions_island and props.actions_island() and (holds() or (props.caption_over_art == true and caption() ~= nil)) then 1 else 0
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

-- A control for the bar's `actions`: its size, a quiet glyph, its tooltip
-- below (`enabled`, `on_click` as the bar's own).
-- Where the bar's controls end, from the window's left (its island's
-- right edge): what lies on the title line beside them starts past it.
-- Now (in full screen they stand further left), or in full screen or out
-- of it (`full`); read tracked unless `tracked` is false.
local function controls_end(full: boolean?, tracked: boolean?): number
	local at = if full == nil then (if tracked == false then full_screen_now() else full_screen()) else full
	local start = toggle_in(at, tracked ~= false)
	return start + controls_w(start)
end
-- Where the last of them ends: `controls_end` less the row's own room past it.
local function last_control_end(): number
	local start = toggle_in(full_screen(), true)
	return start + controls_w(start) - controls_tail(start)
end

-- `window_buttons_right()`, `actions_right()`, `middle()`: where the
-- window's own buttons and the page's controls stand on the bar, and its
-- line (read in a binding: they follow the window).
local M = setmetatable({
	control = control,
	controls_end = controls_end,
	last_control_end = last_control_end,
	window_buttons_right = window_buttons_right,
	actions_right = actions_right,
	middle = middle,
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
