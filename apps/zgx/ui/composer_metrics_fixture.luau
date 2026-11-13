local ds = require("@ds")
local Metrics = require("@ds/composer_metrics")
ds.setup()

local value = ui.state("fixture_metrics_text", "")
local width = ui.state("fixture_metrics_width", 320)
local viewport = ui.state("fixture_metrics_viewport", 800)
local editor_key = ui.state("fixture_metrics_editor", 1)
local field = ui.ref()
local metrics = Metrics.use {
	value = value,
	width = function()
		return width:get()
	end,
	editor = field,
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
	ui.presence(function(): string return tostring(editor_key:get()) end, function(): Node
		return ui.textarea {
			id = "metrics-editor",
			ref = field,
			value = value,
			w_full = true,
			h = 22,
			text_size = "$zeron.composer.text",
			line_height = "$zeron.composer.line",
			highlight = function(line)
				local a, b = string.find(line, "@[%w_]+")
				return if a then { { a, b, weight = 700 } } else {}
			end,
		}
	end),
}
