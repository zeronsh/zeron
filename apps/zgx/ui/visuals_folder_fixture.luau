-- Visual scripts a host hands in (`visual_sources`: { { id, source } }),
-- each drawn as a reply draws it: src/visuals_tests.rs's check of a folder
-- of real visuals.

local ds = require("@ds")
ds.setup()
local Visual = require("@ds/visual")

local SOURCES = ui.shared("visual_sources", {} :: { { id: string, source: string } })

local blocks: { Node } = {}
for i, v in SOURCES:peek() do
	table.insert(blocks, ui.column {
		id = `visual-{i}`,
		w_full = true,
		Visual { id = v.id, title = v.id, source = v.source },
	})
end

return ui.column { id = "reply", w_full = true, gap = "$space.7", blocks }
