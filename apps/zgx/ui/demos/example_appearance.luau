-- Setup, this device's first step: how Zeron looks. Three choices, each
-- seen as it's made: the colour scheme (System, Light, Dark), each a small
-- window drawn in that scheme; the window's material (Frosted, the desktop
-- showing through it softly, or Opaque); and the accent, Zeron's presets
-- (crates/theme AccentPreset) after the theme's own. The window the step
-- stands in takes every choice at once, as the real setup does
-- (onboarding.rs render_appearance applies each on the click), so the step
-- is its own preview: its scheme, its glass, and the accent in the ring of
-- the chosen card and in each small window's send button. The choices
-- are ds.AppearancePicker, the app's own step's.
--
-- All of it is in Settings → Appearance too, and the line under the title
-- says so: nothing here has to be right the first time. The new chat
-- wallpaper, which the real step also offers, stays in Settings: three
-- rows of choices are what the window holds without scrolling.

local ds = require("@ds")
local kit = require("@demos/example_kit")
local setup = require("@demos/setup_parts")

local shown = ui.state("ex_ap_state", "default")
local scheme = ui.state("ex_ap_scheme", "system")
local frosted = ui.state("ex_ap_frosted", true)
local accent = ui.state("ex_ap_accent", "default")

-- What shows through Frosted: a desktop under the window, blurred as the
-- glass blurs it (the menus' glass, far past legibility), its edges past
-- the window's so the blur doesn't fade at them.
local DESKTOP = "assets/cards/violet-dusk/sky.webp"
local DESKTOP_BLUR = 48
local BLEED = 96

-- The window the step stands in, in the scheme and material chosen. System
-- is no scheme of its own: the window takes whatever is around it (here,
-- the gallery's Light or Dark, as the app would take the Mac's).
-- (The scheme is its theme, following the choice in place: the step, and
-- the picker being clicked, stay as they are.)
local function window(content: () -> Node): Node
	return ui.column {
		w_full = true,
		h_full = true,
		relative = true,
		-- (System: no theme of its own, the one around it. The types take a
		-- theme function's name, not its nil.)
		theme = (function(): string?
			local s = scheme:get()
			return if s == "system" then nil else `zeron-{s}`
		end) :: any,
		ui.show(function()
			return frosted:get()
		end, ui.div {
			absolute = true,
			left = -BLEED,
			top = -BLEED,
			right = -BLEED,
			bottom = -BLEED,
			blur = DESKTOP_BLUR,
			pointer_events = "none",
			ui.image { src = DESKTOP, w_full = true, h_full = true, fit = "cover" },
		}),
		ui.div {
			absolute = true,
			left = 0,
			top = 0,
			right = 0,
			bottom = 0,
			pointer_events = "none",
			bg = function()
				return if frosted:get() then "$background" else "$zeron.window"
			end,
		},
		content(),
	}
end

return {
	id = "ex_appearance",
	title = "Make it yours",
	about = "This device's first setup step: the colour scheme as three small windows in their schemes, the window's material and the accent, each taken by the window as it's chosen, so the step is its own preview. Everything here is in Settings → Appearance too.",
	demo = function()
		ui.effect(function()
			local s = shown:get()
			if s == "default" then
				scheme:set("system")
				frosted:set(true)
				accent:set("default")
			elseif s == "dark" then
				scheme:set("dark")
				frosted:set(false)
				accent:set("orange")
			elseif s == "light" then
				scheme:set("light")
				frosted:set(true)
				accent:set("blue")
			end
		end)
		local function step(count: number): Node
			return kit.step {
				step = 1,
				count = count,
				back = kit.back {},
				continue = { label = "Continue" },
				kit.form {
					wide = true,
					title = "Make it yours",
					body = "Pick a look. You can change it any time in Settings.",
					ds.AppearancePicker { scheme = scheme, frosted = frosted, accent = accent },
				},
			}
		end
		return kit.stage {
			states = {
				{ value = "default", label = "Defaults" },
				{ value = "dark", label = "Dark, opaque" },
				{ value = "light", label = "Light, frosted" },
			},
			state = shown,
			also = setup.account_switch(),
			w = 680,
			h = 500,
			window(function()
				return setup.counted(step)
			end),
		}
	end,
}
