-- What the Examples pages share: a screen set in the window it ships in,
-- at that window's size, with the states it can be in (empty, loading,
-- failed) a click away on a switch over it. Setup's window is 680 by 500
-- and opaque (src/main.rs `ONBOARDING_SIZE`); a settings window opens at
-- 900 by 640. The stage draws the window's own chrome round the screen, so
-- a page reads as the window it will be, not as a card in the gallery.

local ds = require("@ds")
local SettingsTypes = require("@ds/settings")

local M = {}

-- The window's own geometry, not spacing: the inset title bar's strip
-- (setup's frame's) and macOS's buttons in it (12 points each, 8 apart, the
-- first 13 in from the edge, on the strip's middle).
local TITLEBAR = ds.Onboarding.TITLEBAR
local LIGHT, LIGHT_GAP, LIGHT_X = 12, 8, 13

-- The window's buttons, as macOS draws them, the same in either theme.
local LIGHTS = { "#ff5f57", "#febc2e", "#28c840" }

local function lights(): Node
	local dots: { Node } = {}
	for _, color in LIGHTS do
		table.insert(dots, ui.div { w = LIGHT, h = LIGHT, rounded = "$radius.full", bg = ui.raw(color, "macOS's own window buttons") })
	end
	return ui.row {
		absolute = true,
		left = LIGHT_X,
		top = (TITLEBAR - LIGHT) / 2,
		gap = ui.raw(LIGHT_GAP, "macOS's step from one window button to the next"),
		aria_hidden = true,
		pointer_events = "none",
		table.unpack(dots),
	}
end

export type Stage = {
	-- What the switch offers, and the state it sets.
	states: { { value: string, label: string } }?,
	state: State<string>?,
	-- Another switch beside it (a page's width: full, or a side pane's).
	also: Node?,
	-- The window's size; `w` may be a function (a width that switches).
	w: number | () -> number,
	h: number,
	-- What floats over the window (dialogs, sheets): a modal never mounts
	-- inside the window's clipped frame, so these stand beside it.
	overlays: { Node }?,
	[number]: Child,
}

function M.stage(props: Stage): Node
	local screen: { any } = {}
	for _, c in ipairs(props :: any) do
		table.insert(screen, c)
	end
	return ui.column {
		w_full = true,
		gap = "$space.3",
		items = "start",
		if (props.states and props.state) or props.also
			then ui.row {
				gap = "$space.3",
				items = "center",
				flex_wrap = true,
				if props.states and props.state
					then ds.Segmented { value = props.state, options = props.states, label = "State" }
					else nil,
				props.also,
			}
			else nil,
		table.unpack(props.overlays or {}),
		ui.column {
			id = "stage",
			w_full = true,
			max_w = props.w,
			h = props.h,
			shrink = 0,
			relative = true,
			rounded = "$radius.xl",
			border = 1,
			border_color = "$border.subtle",
			bg = "$zeron.window",
			overflow = "hidden",
			table.unpack(screen),
			lights(),
			-- Toasts rise in the window's own corner, as they would in the app.
			ds.toast.Host {},
		},
	}
end

-- A step of setup, its footer's pills and a step's form: the app's own
-- (ds/onboarding_frame), so an example and the screen it shows can't drift
-- apart. See there for how a step stands.
export type Step = ds.OnboardingStep
export type Forward = ds.OnboardingForward
export type Quiet = ds.OnboardingQuiet
export type Form = ds.OnboardingForm

M.step = ds.Onboarding.Step
M.scroll_step = ds.Onboarding.ScrollStep
M.forward = ds.Onboarding.Forward
M.quiet = ds.Onboarding.Quiet
M.back = ds.Onboarding.Back
M.form = ds.Onboarding.Form

-- A plain window: the title bar's strip over a page that scrolls (a pull
-- request's, a commit's), at a reading width. `narrow` (a side pane's
-- width) tightens the gutters to 16, as the app's pull request page does
-- below 480 (pull_request_detail.rs NARROW_PAD_X). `fixed` keeps the page
-- to the window's height instead, with no scroll of its own: a page whose
-- parts scroll on their own (a pull request's Code, the app's `h_full`
-- tab: its file list stays put beside a diff that scrolls).
function M.window(props: { narrow: (() -> boolean)?, fixed: boolean?, [number]: Child }): Node
	local kids: { any } = {}
	for _, c in ipairs(props :: any) do
		table.insert(kids, c)
	end
	local narrow = props.narrow
	local px = if narrow
		then function()
			return if narrow() then "$space.4" else "$space.6"
		end
		else "$space.6"
	local page: Node
	if props.fixed then
		page = ui.column {
			w_full = true,
			grow = 1,
			min_h = 0,
			items = "center",
			px = px,
			pt = "$space.4",
			pb = "$space.6",
			table.unpack(kids),
		}
	else
		page = ui.scroll {
			w_full = true,
			grow = 1,
			min_h = 0,
			fade = { 16, 16 },
			fade_overflow = true,
			ui.column {
				w_full = true,
				items = "center",
				px = px,
				pt = "$space.4",
				pb = "$space.6",
				table.unpack(kids),
			},
		}
	end
	return ui.column {
		w_full = true,
		h_full = true,
		ui.div { h = TITLEBAR, w_full = true, shrink = 0 },
		page,
	}
end

-- Each page's glyph says what the page is: a person, tuning
-- dials, a keyboard; Zeron's own widget for Providers; people, a
-- plug, a wallet (Solar Linear, as Zeron's settings use).
local PAGES: { SettingsTypes.NavItem } = {
	{ heading = "Personal" },
	{ value = "profile", label = "Profile", icon = "user-rounded" },
	{ value = "appearance", label = "Appearance", icon = "tuning" },
	{ value = "shortcuts", label = "Shortcuts", icon = "keyboard" },
	-- Agents are per device (their installs and logins live
	-- there), whichever device the page is showing.
	{ heading = "Devices" },
	{ value = "providers", label = "Providers", icon = "widget" },
	{ heading = "Organization" },
	{ value = "members", label = "Members", icon = "users-group-rounded" },
	{ value = "plugins", label = "Plugins", icon = "plug-circle" },
	{ value = "billing", label = "Billing", icon = "wallet" },
}

-- A settings window: its list of pages down the window's full height, the
-- title bar's strip and the window's buttons inside it as a macOS sidebar
-- has them (its rows start under the strip), and the page scrolling beside
-- it under a strip of its own (the inset on the column inside: a scroll
-- clips inside its own padding).
function M.settings(props: { page: string, [number]: Child }): Node
	-- (Each page is an example of its own: the list shows where it would be.)
	local page = ui.state(`settings_at_{props.page}`, props.page)
	local room = ui.ref()
	-- Short of room, the page keeps it all and the list goes.
	local function wide(): boolean
		return (room.width or 900) >= 640
	end
	return ui.row {
		ref = room,
		w_full = true,
		h_full = true,
		ui.show(wide, ds.Settings.Nav {
				value = page,
				top = TITLEBAR,
				on_choose = function() end,
				items = PAGES,
			}),
		ui.column {
			grow = 1,
			min_w = 0,
			h_full = true,
			ui.div { h = TITLEBAR, w_full = true, shrink = 0 },
			ui.scroll {
				w_full = true,
				grow = 1,
				min_h = 0,
				fade = { 16, 16 },
				fade_overflow = true,
				ui.column { w_full = true, items = "center", px = "$space.6", pt = "$space.4", pb = "$space.6", props[1] },
			},
		},
	}
end

return M
