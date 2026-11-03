-- The title bar's page controls over a new chat's wallpaper, holding what a
-- plugin adds (a "Focus" button), for src/shell_tests.rs and a look: their
-- island of glass shows while they stand on art and hold anything. The
-- command `art` takes the picture away (a session's page).

ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()

local on_art = ui.state("on_art", true)
local holds = ui.state("holds", true)
ui.command {
	id = "art",
	title = "Over art, or not",
	run = function()
		on_art:set(not on_art:peek())
	end,
}
ui.command {
	id = "empty",
	title = "Nothing in the row, or something",
	run = function()
		holds:set(not holds:peek())
	end,
}

return ui.column {
	w_full = true,
	h_full = true,
	relative = true,
	bg = "$background",
	-- Bright art (a JPEG: a plain zgx-run decodes no WebP).
	ui.show(
		function(): boolean
			return on_art:get()
		end,
		ui.image { src = "assets/art/plate-3-sunrise.jpg", absolute = true, left = 0, right = 0, top = 0, bottom = 0, fit = "cover", aria_hidden = true }
	),
	ui.div {
		absolute = true,
		left = 0,
		right = 0,
		top = 0,
		pointer_events = "none",
		ds.TitleBar {
			collapsed = ui.state("collapsed", false),
			lights = true,
			island = function(): boolean
				return false
			end,
			actions = ui.row {
				id = "titlebar-actions",
				items = "center",
				gap = "$zeron.island.gap",
				ui.show(holds, ds.Button { tone = "ghost", size = "sm", "Focus" }),
			},
			actions_island = function(): boolean
				return on_art:get()
			end,
		},
	},
}
