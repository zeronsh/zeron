-- A toggle button: an icon button that stays on. The same square, sizes,
-- inks and tooltip as `IconButton`, so a row of them beside icon buttons
-- reads as one bar (the find bar's Aa, whole word and pattern beside its
-- arrows and close). On, it holds a selection's flat wash (the sidebar's
-- chosen row's) and the full ink; under the pointer it keeps that wash, so
-- on never looks like off being hovered.
--
-- `ToggleButton { on = state, label = "Match Case", text = "Aa" }`, or
-- `icon = "wrap-text"` for a glyph. `text` is a short mark set in the mono
-- face at the glyph's weight, sized to sit like a 14-point glyph. A click
-- flips `on`; `on_change(v)` hears it. `size`, `tone`, `tooltip` and
-- `tooltip_side` are `IconButton`'s.

local Icon = require("@self/icon")
local icon_button = require("@self/icon_button")

local recipe = ui.variants {
	base = {
		shrink = 0,
		items = "center",
		justify = "center",
		p = 0,
		rounded = "$zeron.icon_button.radius",
		bg = "#00000000",
		cursor = "pointer",
		hover = { bg = "$hover", color = "$text.primary" },
		active = { bg = "$pressed", color = "$text.primary" },
		disabled = { bg = "#00000000", color = "$text.disabled", cursor = "default" },
	},
	variants = {
		size = {
			sm = { w = "$zeron.icon_button.sm", h = "$zeron.icon_button.sm", text_size = "$font.size.sm" },
			md = { w = "$zeron.icon_button.md", h = "$zeron.icon_button.md", text_size = "$font.size.md" },
			lg = { w = "$zeron.icon_button.lg", h = "$zeron.icon_button.lg", text_size = "$font.size.md" },
		},
		tone = {
			normal = { color = "$text.secondary" },
			quiet = { color = "$text.tertiary" },
		},
		on = { [true] = {}, [false] = {} },
	},
	-- On wins over either tone, and over hover and press.
	compound = {
		{
			on = true,
			bg = "$zeron.selected.flat.bg",
			color = "$text.primary",
			hover = { bg = "$zeron.selected.flat.bg" },
			active = { bg = "$zeron.selected.flat.bg" },
		},
	},
	defaults = { size = "md", tone = "normal", on = false },
}

export type Props = Merge<ContainerProps, {
	on: State<boolean>,
	-- (Its tooltip a plain name's or a message's only.)
	label: Label,
	icon: (Icon.Name | () -> Icon.Name)?,
	text: string?,
	size: icon_button.Size?,
	tone: icon_button.Tone?,
	tooltip: (string | false)?,
	tooltip_side: icon_button.Side?,
	on_change: ((on: boolean) -> ())?,
}>

return function(props: Props): Node
	local p = table.clone(props) :: any
	local on = props.on
	local size: icon_button.Size = props.size or "md"
	p.variants = recipe { size = size, tone = props.tone, on = on :: any }
	p.on, p.size, p.tone, p.icon, p.text, p.tooltip_side, p.on_change = nil, nil, nil, nil, nil, nil, nil
	if props.tooltip == false then
		p.tooltip = nil
	elseif props.tooltip == nil then
		-- (A tooltip is a string, read as it is built: a message's text in
		-- the locale then.)
		local label: any = props.label
		p.tooltip = if type(label) == "string" then label elseif typeof(label) == "MessageUd" then label:get() else nil
	end
	if p.tooltip then
		p.tooltip_placement = icon_button.placement(props.tooltip_side)
	end
	p.role = "checkbox"
	p.checked = function(): boolean
		return on:get()
	end
	p.on_click = function()
		local v = not on:peek()
		on:set(v)
		if props.on_change then
			props.on_change(v)
		end
	end
	local icon = props.icon
	if icon ~= nil then
		local name: Icon.Name | () -> Icon.Name = icon
		p[1] = Icon { name = name, size = icon_button.glyph_size(size) }
	elseif props.text then
		p[1] = ui.text {
			props.text,
			literal = true,
			font = "$font.mono",
			weight = "$font.weight.medium",
		}
	end
	return ui.button(p)
end
