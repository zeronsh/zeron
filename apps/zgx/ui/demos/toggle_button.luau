-- The gallery's Toggle button page: text marks and glyphs, off and on,
-- beside icon buttons as a find bar has them.

local ds = require("@ds")

local function block(title: string, ...: Child): Node
	return ui.column { gap = "$space.3", ds.SectionTitle { title }, ... }
end
local function row(...: Child): Node
	return ui.row { gap = "$space.3", items = "center", flex_wrap = true, ... }
end

return {
	id = "toggle-button",
	title = "Toggle button",
	about = "An icon button that stays on: the same square, inks and tooltip, holding a selection's flat wash while on.",
	demo = function(): Node
		local case = ui.state("toggle_case", false)
		local word = ui.state("toggle_word", true)
		local regex = ui.state("toggle_regex", false)
		local wrap = ui.state("toggle_wrap", true)
		local on = ui.state("toggle_on", true)
		local off = ui.state("toggle_off", false)
		return ui.column {
			gap = "$space.6",
			block(
				"OFF AND ON",
				row(
					ds.ToggleButton { on = off, label = "Off", text = "Aa" },
					ds.ToggleButton { on = on, label = "On", text = "Aa" },
					ds.ToggleButton { on = off, label = "Off", icon = "wrap-text" },
					ds.ToggleButton { on = on, label = "On", icon = "wrap-text" },
					ds.ToggleButton { on = off, label = "Disabled", icon = "wrap-text", disabled = true }
				)
			),
			block(
				"SIZES",
				row(
					ds.ToggleButton { on = wrap, label = "Small", icon = "wrap-text", size = "sm" },
					ds.ToggleButton { on = wrap, label = "Medium", icon = "wrap-text", size = "md" },
					ds.ToggleButton { on = wrap, label = "Large", icon = "wrap-text", size = "lg" },
					ds.ToggleButton { on = case, label = "Small", text = "Aa", size = "sm" },
					ds.ToggleButton { on = case, label = "Medium", text = "Aa", size = "md" },
					ds.ToggleButton { on = case, label = "Large", text = "Aa", size = "lg" }
				)
			),
			block(
				"BESIDE ICON BUTTONS",
				ds.Glass {
					kind = "island",
					p = "$space.1",
					w = 300,
					ui.row {
						gap = "$zeron.space.hair",
						items = "center",
						ui.text { "Find", grow = 1, px = "$space.2", text_size = "$font.size.sm", color = "$text.tertiary" },
						ds.ToggleButton { on = case, label = "Match Case", text = "Aa", size = "sm", tooltip_side = "bottom" },
						ds.ToggleButton { on = word, label = "Whole Word", text = "ab", size = "sm", tooltip_side = "bottom" },
						ds.ToggleButton { on = regex, label = "Use Pattern", text = ".*", size = "sm", tooltip_side = "bottom" },
						ds.IconButton { icon = "arrow-up", label = "Previous Match", size = "sm", tooltip_side = "bottom" },
						ds.IconButton { icon = "arrow-down", label = "Next Match", size = "sm", tooltip_side = "bottom" },
						ds.IconButton { icon = "close", label = "Close", size = "sm", tooltip_side = "bottom" },
					},
				}
			),
		}
	end,
}
