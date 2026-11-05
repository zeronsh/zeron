-- The composer's chip strip alone, for tests/annotation_flow.test.luau:
-- what it carries is `test_cargo` (ds/composer_cargo items), as the chat
-- page hands it the draft's.

ui.theme.strict("warn")
local ds = require("@ds")
local Cargo = require("@ds/composer_cargo")
ds.setup()

local items = ui.state("test_cargo", {} :: { any })
local removed = ui.state("test_cargo_removed", "")

return ui.column {
	w_full = true,
	h_full = true,
	-- (On a ground of its own, so a capture reads as the app does.)
	theme = "zeron-dark",
	bg = "$background",
	p = "$space.6",
	justify = "end",
	Cargo.Strip {
		items = items,
		on_remove = function(id: string)
			removed:set(id)
		end,
	},
}
