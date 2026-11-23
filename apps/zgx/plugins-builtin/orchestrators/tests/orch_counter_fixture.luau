-- A rolling figure among neighbours, for the counter's mid-roll pictures
-- (ds/orch_counter): a label before it, a suffix after, rows of text above
-- and below and a block beside it, its value the state `cpu`.

ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()
local Counter = require("@orchestrators/ds/orch_counter")

local cpu = ui.state("cpu", 4)

local function words(s: string): Node
	return ui.text { s, text_size = "$font.size.sm", color = "$text.secondary" }
end

return ui.column {
	w_full = true,
	h_full = true,
	p = "$space.5",
	gap = "$space.2",
	bg = "$background",
	words("A row above the figure"),
	ui.row {
		gap = "$space.2",
		items = "center",
		ui.text { "CPU", id = "cpu-label", text_size = "$font.size.xs", color = "$text.tertiary" },
		Counter { id = "counter", value = function(): number
			return cpu:get()
		end, suffix = "%", text_size = "$font.size.md", weight = "$font.weight.medium" },
		ui.div { w = 80, h = 16, rounded = "$radius.sm", bg = "$hover" },
	},
	words("A row below the figure"),
}
