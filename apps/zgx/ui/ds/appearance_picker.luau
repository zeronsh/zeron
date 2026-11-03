-- Make it yours: how Zeron looks, chosen in three rows, each seen as it's
-- made. The colour scheme (System, Light, Dark), each a small window drawn
-- in that scheme; the window's material (Frosted, the desktop showing
-- through it softly, or Opaque); and the accent, Zeron's presets after the
-- theme's own (ds/look). Setup's first step after sign-in is this, and the
-- gallery's Make it yours draws it too.
--
--   AppearancePicker { scheme = state, frosted = state, accent = state }
--
-- It only sets the states: the window round it takes each choice (the app
-- applies its scheme, material and accent; the gallery previews them in its
-- stage), so the step is its own preview, the accent showing in the ring of
-- the chosen card and in each small window's send button.

local Settings = require("@self/settings")
local Segmented = require("@self/segmented")
local control = require("@self/control")
local look = require("@self/look")
local locale = require("@self/locale")
local palettes = require("@self/palettes")

export type Props = {
	-- "system", "light" or "dark".
	scheme: State<string>,
	frosted: State<boolean>,
	-- An accent's value (ds/look's ACCENTS).
	accent: State<string>,
}

-- The scheme cards' geometry: three across the list's measure (480) with a
-- gap of 12; the small window inside each at the card's even inset.
local CARD_W = 152
local INSET = 4
local PREVIEW_W = CARD_W - 2 * (1 + INSET)
-- Near a window's own proportions (the user found 56 squished).
local PREVIEW_H = 72
-- The small window's own parts: its sidebar, a row of it, the composer.
local SIDE_W = 44
local SIDE_ROW = 10
local COMPOSER = 14

-- A small window in one scheme: its sidebar with the chosen chat in the
-- sidebar's wash, two lines of a chat, and the composer with the accent's
-- send button at its end. Drawn in the scheme's own tokens, whatever the
-- window around it shows.
local function miniature(on_dark: boolean, chosen_ink: () -> string): Node
	local function line(w: string): Node
		return ui.div { h = 4, w = w, shrink = 0, rounded = "$radius.full", bg = "$border.strong" }
	end
	local function side_row(selected: boolean, w: string): Node
		return ui.row {
			w_full = true,
			h = SIDE_ROW,
			shrink = 0,
			px = "$zeron.space.hair",
			items = "center",
			rounded = "$radius.sm",
			bg = if selected then "$zeron.selected.flat.bg" else nil,
			line(w),
		}
	end
	return ui.row {
		-- (The scheme as worn now: a template's colours, or Zeron's.)
		theme = if on_dark then palettes.scheme.dark else palettes.scheme.light,
		w = PREVIEW_W,
		h = PREVIEW_H,
		shrink = 0,
		bg = "$zeron.window",
		ui.column {
			w = SIDE_W,
			h_full = true,
			shrink = 0,
			p = "$space.1",
			gap = "$zeron.space.hair",
			bg = "$hover",
			side_row(true, "70%"),
			side_row(false, "55%"),
			side_row(false, "80%"),
		},
		ui.column {
			grow = 1,
			min_w = 0,
			h_full = true,
			p = "$space.2",
			gap = "$space.1",
			line("75%"),
			line("50%"),
			ui.div { grow = 1 },
			ui.row {
				w_full = true,
				h = COMPOSER,
				shrink = 0,
				p = "$zeron.space.hair",
				justify = "end",
				rounded = "$radius.full",
				border = 1,
				border_color = "$border",
				-- (A pill's contents are pills.)
				ui.div {
					w = COMPOSER - 2 * 3,
					h_full = true,
					rounded = "$radius.full",
					bg = chosen_ink,
				},
			},
		},
	}
end

-- System: the two schemes side by side, dark at the left, light at the
-- right, one window cut down the middle.
local function halves(chosen_ink: () -> string): Node
	local half = PREVIEW_W / 2
	return ui.row {
		w = PREVIEW_W,
		h = PREVIEW_H,
		shrink = 0,
		ui.div { w = half, h_full = true, shrink = 0, relative = true, overflow = "hidden", ui.div { absolute = true, left = 0, top = 0, miniature(true, chosen_ink) } },
		ui.div { w = half, h_full = true, shrink = 0, relative = true, overflow = "hidden", ui.div { absolute = true, left = -half, top = 0, miniature(false, chosen_ink) } },
	}
end

-- A scheme as an option card: the small window, its name under it. Chosen,
-- the accent's ring over its edge (onboarding's option cards: a ring,
-- never a fill or a shadow), easing in.
local function scheme_card(scheme: State<string>, chosen_ink: () -> string, value: string, label: string | Message): Node
	local function on(): boolean
		return scheme:get() == value
	end
	local function pick()
		scheme:set(value)
	end
	local preview = if value == "system" then halves(chosen_ink) else miniature(value == "dark", chosen_ink)
	return ui.column {
		role = "radio",
		checked = on,
		label = label,
		focusable = true,
		cursor = "pointer",
		relative = true,
		w = CARD_W,
		shrink = 0,
		p = ui.raw(INSET, "the card's even inset round its window"),
		gap = "$space.1",
		rounded = "$radius.xl",
		border = 1,
		bg = "$zeron.option.bg",
		border_color = "$zeron.option.edge",
		hover = { bg = "$hover" },
		focus = control.ring,
		on_click = pick,
		on_key = function(e)
			if e.key == " " or e.key == "Enter" then
				pick()
				return true
			end
			return false
		end,
		-- The small window follows the card's corner, its inset in.
		ui.div {
			w_full = true,
			shrink = 0,
			rounded = ui.raw(control.nest("$radius.xl", 1, INSET), "the card's corner, its edge and inset in"),
			overflow = "hidden",
			preview,
		},
		ui.text {
			label,
			w_full = true,
			-- Room under the name to the card's edge, as much as over it.
			py = "$space.1",
			align = "center",
			text_size = "$font.size.sm",
			line_height = "$font.line.sm",
			weight = "$font.weight.medium",
			color = function()
				return if on() then "$text.primary" else "$text.secondary"
			end,
		},
		ui.div {
			absolute = true,
			left = -1,
			top = -1,
			right = -1,
			bottom = -1,
			rounded = "$radius.xl",
			border = 2,
			border_color = chosen_ink,
			pointer_events = "none",
			opacity = ui.tween(ui.computed(function(): number
				return if on() then 1 else 0
			end), { duration = 140 }),
		},
	}
end

-- An accent as a swatch, its ring (the text's ink, round the dot with a
-- gap) easing in when it's chosen. Each preset's swatch shows the ink it
-- would take in the scheme shown.
local SWATCH = 16
local RING = 24
local function swatch(accent: State<string>, a: look.Accent): Node
	local function on(): boolean
		return accent:get() == a.value
	end
	local function pick()
		accent:set(a.value)
	end
	return ui.row {
		role = "radio",
		checked = on,
		-- (Read in a function: its name follows the locale.)
		label = function(): string
			return a.label
		end,
		focusable = true,
		cursor = "pointer",
		relative = true,
		w = RING,
		h = RING,
		shrink = 0,
		items = "center",
		justify = "center",
		rounded = "$radius.full",
		focus = control.ring,
		on_click = pick,
		on_key = function(e)
			if e.key == " " or e.key == "Enter" then
				pick()
				return true
			end
			return false
		end,
		ui.div {
			w = SWATCH,
			h = SWATCH,
			rounded = "$radius.full",
			bg = look.ink(a),
		},
		ui.div {
			absolute = true,
			left = 0,
			top = 0,
			right = 0,
			bottom = 0,
			rounded = "$radius.full",
			border = 2,
			border_color = "$text.secondary",
			pointer_events = "none",
			opacity = ui.tween(ui.computed(function(): number
				return if on() then 1 else 0
			end), { duration = 140 }),
		},
	}
end

-- The accents as a row of swatches, one chosen (Settings → Appearance's
-- Accent row; this picker's own).
local function accents(accent: State<string>): Node
	local swatches: { Node } = {}
	for _, a in look.ACCENTS do
		table.insert(swatches, swatch(accent, a))
	end
	return ui.row {
		role = "radiogroup",
		label = locale.t("ds.appearance_picker.accent"),
		items = "center",
		table.unpack(swatches),
	}
end

local function picker(props: Props): Node
	local scheme, frosted, accent = props.scheme, props.frosted, props.accent
	local function chosen_ink(): string
		return look.ink(look.find(accent:get()))
	end
	-- The cards and the rows under them a step closer than a form's own
	-- gap: one set of choices, and the window's room.
	return ui.column {
		w_full = true,
		gap = "$space.3",
		ui.row {
			w_full = true,
			gap = "$space.3",
			role = "radiogroup",
			label = locale.t("ds.appearance_picker.color_scheme"),
			scheme_card(scheme, chosen_ink, "system", locale.t("ds.appearance_picker.scheme.system")),
			scheme_card(scheme, chosen_ink, "light", locale.t("ds.appearance_picker.scheme.light")),
			scheme_card(scheme, chosen_ink, "dark", locale.t("ds.appearance_picker.scheme.dark")),
		},
		Settings.Group {
			Settings.Row {
				label = locale.t("ds.appearance_picker.window"),
				description = function()
					return if frosted:get() then locale.s("ds.appearance_picker.frosted_hint") else locale.s("ds.appearance_picker.opaque_hint")
				end,
				control = Segmented {
					value = frosted,
					label = locale.t("ds.appearance_picker.window"),
					segment = 64,
					options = {
						{ value = true, label = locale.t("ds.appearance_picker.frosted") },
						{ value = false, label = locale.t("ds.appearance_picker.opaque") },
					},
				},
			},
			Settings.Row {
				label = locale.t("ds.appearance_picker.accent"),
				description = function()
					return look.find(accent:get()).label
				end,
				control = accents(accent),
			},
		},
	}
end

-- `AppearancePicker { scheme, frosted, accent }`, and its accents alone
-- (`AppearancePicker.Accents { value = state }`).
return setmetatable({
	Accents = function(p: { value: State<string> }): Node
		return accents(p.value)
	end,
}, {
	__call = function(_: any, props: Props): Node
		return picker(props)
	end,
})
