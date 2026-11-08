local ds = require("@ds")
local Metrics = require("@ds/composer_metrics")
ds.setup()

local value = ui.state("fixture_metrics_text", "")
local width = ui.state("fixture_metrics_width", 320)
local viewport = ui.state("fixture_metrics_viewport", 800)
local metrics = Metrics.use {
	value = value,
	width = function() return width:get() end,
	left = 16,
	keep = "fixture",
}
local result = ui.state("fixture_metrics_result", {} :: any)
local history = ui.state("fixture_metrics_history", {} :: { string })
ui.effect(function()
	local r = metrics.result:get()
	result:set(r)
	local h = table.clone(history:peek())
	table.insert(h, r.status)
	history:set(h)
end)

ui.command {
	id = "fixture.metrics.type",
	title = "Larger composer type",
	run = function()
		ui.theme.tokens { zeron = { composer = { text = 18, line = 28 } } }
	end,
}

return ui.div {
	w = viewport,
	h_full = true,
	relative = true,
	metrics.nodes[1],
	metrics.nodes[2],
}
