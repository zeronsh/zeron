-- A theme's preview, drawn in its own colours (no picture needed): a
-- window's shape a row's tile high, one half for each scheme it has, side by
-- side (light, then dark), or one alone across the whole tile. Each half
-- wears that scheme's theme (a registered theme's name) and draws its
-- window, a raised bar, its text's two inks, and its accent and code's
-- colours as dots. The Templates page's rows (a template's colours) and the
-- Themes page's (a theme's) both lead with one.
--
--   ThemeSwatch { themes = function() return { "light-name", "dark-name" } end, class = "theme-swatch" }
--
-- (Each half is `<class>-half`, for a test to count.)

local M = {}

M.W = 48
M.H = 32

export type Props = { themes: () -> { string }, class: string? }

local function half(props: Props, i: number): Node
	local function bar(color: string, width: string): Node
		return ui.div { w = width, h = 3, shrink = 0, rounded = "$radius.sm", bg = color }
	end
	local dots: { Node } = {}
	for _, color in { "$accent", "$zeron.syntax.keyword", "$zeron.syntax.string", "$zeron.syntax.function" } do
		table.insert(dots, ui.div { w = 4, h = 4, shrink = 0, rounded = "$radius.sm", bg = color })
	end
	return ui.show(
		function(): boolean
			return #props.themes() >= i
		end,
		ui.column {
			theme = function(): string
				return props.themes()[i] or "zeron-dark"
			end,
			-- (Inside the box's 1pt edge, the halves sharing it.)
			w = function(): number
				return (M.W - 2) / math.max(#props.themes(), 1)
			end,
			shrink = 0,
			h_full = true,
			bg = "$zeron.window",
			class = if props.class then `{props.class}-half` else nil,
			ui.div { w_full = true, h = 6, shrink = 0, bg = "$popover.bg" },
			ui.column {
				p = "$zeron.space.hair",
				gap = "$zeron.space.hair",
				pt = "$space.1",
				bar("$text.primary", "70%"),
				bar("$text.secondary", "45%"),
				ui.row { gap = "$zeron.space.hair", pt = "$zeron.space.hair", table.unpack(dots) },
			},
		}
	)
end

function M.Swatch(props: Props): Node
	return ui.row {
		w = M.W,
		h = M.H,
		shrink = 0,
		rounded = "$radius.md",
		overflow = "hidden",
		border = 1,
		border_color = "$zeron.settings.edge",
		aria_hidden = true,
		class = props.class,
		half(props, 1),
		half(props, 2),
	}
end

return M
