-- A split button: an action and its variants in one control (Commit, and
-- beside it ▾ for Commit and push, Amend). The face does the usual thing;
-- the chevron opens the others as a menu under it. Both halves are the
-- same Button tone and size, joined: the inner corners square, a hairline
-- of the label's ink between them.
--
-- `SplitButton { "Commit", on_click = commit, items = {
--   { label = "Commit and push", icon = "arrow-up", shortcut = "cmd+shift+enter", on_click = … },
--   { label = "Amend last commit", on_click = … } } }`

local Button = require("@self/button")
local Icon = require("@self/icon")
local Menu = require("@self/menu")
local Kbd = require("@self/kbd")
local FadeText = require("@self/fade_text")
local locale = require("@self/locale")

-- (Its words may be messages, `ui.t`, drawn as they are.)
export type Item = {
	label: string | Message,
	icon: Icon.Name?,
	description: (string | Message)?,
	shortcut: string?,
	danger: boolean?,
	disabled: boolean?,
	on_click: (() -> ())?,
}
export type Props = {
	[number]: Child,
	on_click: (() -> ())?,
	items: { Item },
	tone: Button.Tone?,
	size: Button.Size?,
	disabled: (boolean | () -> boolean)?,
	-- What assistive technology calls the chevron.
	menu_label: (string | Message)?,
}

local RADIUS: { [Button.Size]: string } = { xs = "$radius.lg", sm = "$radius.md", md = "$radius.lg", lg = "$radius.lg" }
local SQUARE: { [Button.Size]: number } = { xs = 24, sm = 28, md = 32, lg = 40 }
local FILL: { [Button.Tone]: string } = {
	primary = "$zeron.primary.bg",
	secondary = "$zeron.secondary.bg",
	ghost = "#00000000",
	accent = "$accent",
	danger = "$zeron.destructive.bg",
}

return ui.component(function(props: Props): Node
	local open = ui.use_state(false)
	local size: Button.Size = props.size or "md"
	local tone: Button.Tone = props.tone or "primary"
	local function off(): boolean
		local d = props.disabled
		return if type(d) == "function" then d() else d == true
	end
	-- Inside the halves: the row round them clips anything outside.
	local ring: Styles = { outline = "$zeron.focus.width", outline_color = "$zeron.focus.ring", outline_offset = -2 }
	local face: Button.Props = {
		tone = tone,
		size = size,
		disabled = props.disabled,
		on_click = props.on_click,
		focus = ring,
		-- Square: the row round both halves rounds and clips them.
		rounded = 0,
	}
	for _, c in ipairs(props :: any) do
		table.insert(face, c)
	end
	local chevron = Button {
		tone = tone,
		size = size,
		icon = true,
		label = props.menu_label or locale.t("ds.split_button.more"),
		disabled = props.disabled,
		w = SQUARE[size] - 4,
		rounded = 0,
		focus = ring,
		expanded = function()
			return open:get()
		end,
		on_click = function()
			open:toggle()
		end,
		ui.div {
			rotate = ui.spring(open, { from = 0, to = 180, stiffness = 380, damping = 30 }),
			Icon { name = "alt-arrow-down", size = 14 },
		},
	}
	local lit = ui.use_state(0)
	local els: { any } = {}
	local rows: { Node } = {}
	for i, item in props.items do
		local ink = if item.disabled then "$text.disabled" elseif item.danger then "$danger" else "$text.primary"
		table.insert(rows, ui.menu_item(Menu.lights(Menu.look({
			label = item.label,
			disabled = item.disabled,
			on_click = item.on_click,
			if item.icon then Menu.glyph(item.icon, if item.disabled then "$text.disabled" elseif item.danger then "$danger" else nil) else nil,
			if item.description
				then ui.column {
					grow = 1,
					min_w = 0,
					gap = "$zeron.space.hair",
					FadeText { item.label, text_size = "$font.size.md", color = ink },
					ui.text { item.description, text_size = "$font.size.sm", line_height = "$font.line.sm", color = "$text.tertiary", wrap = true },
				}
				else FadeText { item.label, grow = 1, text_size = "$font.size.md", color = ink },
			if item.shortcut then Kbd { keys = item.shortcut, look = "plain", size = "md", tone = "quiet" } else nil,
		}, item.description ~= nil), i, lit, els)))
	end
	return ui.row {
		shrink = 0,
		items = "center",
		-- One control: its corner (the size's), clipping both halves.
		rounded = RADIUS[size],
		overflow = "hidden",
		Button(face),
		-- The seam: the label's ink, faint, inset from the ends.
		ui.div {
			w = 1,
			h = SQUARE[size],
			shrink = 0,
			relative = true,
			-- Behind it, the tone's own fill, so the two halves read as one
			-- (dimmed with them).
			bg = FILL[tone],
			opacity = function()
				return if off() then 0.4 else 1
			end,
			ui.div {
				absolute = true,
				left = 0,
				top = 8,
				bottom = 8,
				w = 1,
				bg = if tone == "primary" then "$zeron.primary.text" elseif tone == "accent" then "$accent.text" elseif tone == "danger" then "$zeron.destructive.text" else "$text.primary",
				opacity = 0.25,
			},
		},
		ui.menu(Menu.panel {
			open = open,
			anchor = chevron,
			placement = { side = "bottom", align = "end", offset = 4 },
			w = 248,
			ui.column { w_full = true, p = "$space.1", gap = "$zeron.list.gap", table.unpack(rows) },
		}),
	}
end)
