-- An icon button: a glyph alone on a square, clear until it is touched.
-- Every bar and row control that is only a glyph is one of these: the title
-- bar's, a sidebar row's corner, a "…" that opens a menu, a find bar's.
--
-- `IconButton { icon = "close", label = "Close", on_click = … }`. `label`
-- names it to assistive technology and is its tooltip (`tooltip` says
-- something else, or `false` none). `size`: `sm` (22, a 14 glyph: a row's
-- corner, a dense bar), `md` (24, a 16 glyph: the title bar's) or `lg`
-- (28). `tone`: `normal` (the secondary ink) or `quiet` (the tertiary,
-- for controls that sit among content and shouldn't draw the eye) or
-- `danger` (quiet, turning to the danger's wash and ink under the pointer:
-- a delete among other actions).
-- `lit` (a boolean, state or function) holds it lit while what it opens
-- is open. `tooltip_side` moves the tooltip off whatever it would cover
-- (`"bottom"` in a bar at the top of something). Children, if given, take
-- the glyph's place (a glyph that isn't an `Icon`, as the sidebar's toggle).
--
-- Each state says its own fill and ink, so zgx's button (its raised fill,
-- its pressed wash over it) never shows through.

local Icon = require("@self/icon")

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
			sm = { w = "$zeron.icon_button.sm", h = "$zeron.icon_button.sm" },
			md = { w = "$zeron.icon_button.md", h = "$zeron.icon_button.md" },
			lg = { w = "$zeron.icon_button.lg", h = "$zeron.icon_button.lg" },
		},
		tone = {
			normal = { color = "$text.secondary" },
			quiet = { color = "$text.tertiary" },
			-- Quiet at rest; under the pointer, what it does is destructive:
			-- the danger's wash and ink (a menu's destructive row's).
			danger = {
				color = "$text.tertiary",
				hover = { bg = "$zeron.menu.danger", color = "$danger" },
				active = { bg = "$zeron.menu.danger", color = "$danger" },
			},
		},
		-- Lit: what it opens is open (the hover wash, held, the ink full;
		-- below, over either tone).
		lit = { [true] = {}, [false] = {} },
	},
	compound = {
		{ lit = true, bg = "$hover", color = "$text.primary" },
	},
	defaults = { size = "md", tone = "normal", lit = false },
}

local GLYPH = { sm = "$zeron.icon_button.glyph.sm", md = "$zeron.icon_button.glyph.md", lg = "$zeron.icon_button.glyph.lg" }

export type Size = "sm" | "md" | "lg"
export type Tone = "normal" | "quiet" | "danger"
export type Side = "top" | "bottom" | "left" | "right"
export type Props = Merge<ContainerProps, {
	icon: (Icon.Name | () -> Icon.Name)?,
	-- What it's called (a message, `ui.t`; a function, or a state, for one
	-- that changes: the button follows it; its tooltip is a plain name's or
	-- a message's only).
	label: Label,
	size: Size?,
	tone: Tone?,
	lit: (boolean | State<boolean> | () -> boolean)?,
	tooltip: (string | false)?,
	tooltip_side: Side?,
	[number]: Child,
}>

-- The glyph's size for a button size, in points (read as it is built: the
-- theme's).
local function glyph_size(size: Size): number
	return (ui.theme.get(string.sub(GLYPH[size], 2)) :: number?) or if size == "sm" then 14 else 16
end

local M = {}

-- Where a tooltip sits, `side` of its control, 6 points off (zgx's).
function M.placement(side: Side?): { [string]: any }?
	if side == nil or side == "top" then
		return nil
	end
	return { side = side, align = "center", offset = 6 }
end

function M.glyph_size(size: Size?): number
	return glyph_size(size or "md")
end

function M.IconButton(props: Props): Node
	local p = table.clone(props) :: any
	local size: Size = props.size or "md"
	p.variants = recipe { size = size, tone = props.tone, lit = (props.lit or false) :: any }
	p.size, p.tone, p.lit, p.icon, p.tooltip_side = nil, nil, nil, nil, nil
	if props.tooltip == false then
		p.tooltip = nil
	elseif props.tooltip == nil then
		-- (A tooltip is a string, read as it is built: a message's text in
		-- the locale then.)
		local label: any = props.label
		p.tooltip = if type(label) == "string" then label elseif typeof(label) == "MessageUd" then label:get() else nil
	end
	if p.tooltip then
		p.tooltip_placement = M.placement(props.tooltip_side)
	end
	local icon = props.icon
	if icon ~= nil and p[1] == nil then
		local name: Icon.Name | () -> Icon.Name = icon
		p[1] = Icon { name = name, size = glyph_size(size) }
	end
	return ui.button(p)
end

return M
