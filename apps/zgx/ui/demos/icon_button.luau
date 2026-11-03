-- The gallery's Icon button page: every size and tone, lit, disabled, a
-- tooltip moved below, and a bar of them as the find bar has.

local ds = require("@ds")

local function block(title: string, ...: Child): Node
	return ui.column { gap = "$space.3", ds.SectionTitle { title }, ... }
end
local function row(...: Child): Node
	return ui.row { gap = "$space.3", items = "center", flex_wrap = true, ... }
end

return {
	id = "icon-button",
	title = "Icon button",
	about = "A glyph alone on a square, clear until touched: title bar controls, a row's corner, a \"…\" that opens a menu, a bar's arrows and close. Its label is its tooltip.",
	demo = function(): Node
		local open = ui.state("icon_button_open", false)
		return ui.column {
			gap = "$space.6",
			block(
				"SIZES",
				row(
					ds.IconButton { icon = "more-horizontal", label = "Small (22)", size = "sm" },
					ds.IconButton { icon = "more-horizontal", label = "Medium (24)", size = "md" },
					ds.IconButton { icon = "more-horizontal", label = "Large (28)", size = "lg" }
				)
			),
			block(
				"TONES",
				row(
					ds.IconButton { icon = "pen", label = "Normal" },
					ds.IconButton { icon = "pen", label = "Quiet", tone = "quiet" },
					ds.IconButton { icon = "pen", label = "Quiet, small", tone = "quiet", size = "sm" }
				)
			),
			block(
				"STATES",
				row(
					ds.IconButton {
						icon = "more-horizontal",
						label = "Lit while its menu is open (click)",
						lit = open,
						on_click = function()
							open:set(not open:peek())
						end,
					},
					ds.IconButton { icon = "arrow-left", label = "Disabled", disabled = true },
					ds.IconButton { icon = "arrow-down", label = "Tooltip below", tooltip_side = "bottom" }
				)
			),
			block(
				"IN A BAR",
				ds.Glass {
					kind = "island",
					p = "$space.1",
					w = 220,
					ui.row {
						gap = "$zeron.space.hair",
						items = "center",
						ui.text { "3 of 12", grow = 1, px = "$space.2", text_size = "$font.size.xs", color = "$text.tertiary" },
						ds.IconButton { icon = "arrow-up", label = "Previous Match", size = "sm", tooltip_side = "bottom" },
						ds.IconButton { icon = "arrow-down", label = "Next Match", size = "sm", tooltip_side = "bottom" },
						ds.IconButton { icon = "close", label = "Close", size = "sm", tooltip_side = "bottom" },
					},
				}
			),
		}
	end,
}
