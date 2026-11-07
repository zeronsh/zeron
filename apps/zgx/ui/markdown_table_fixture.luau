-- The file preview's Markdown and the transcript's windowed, incremental path.
local ds = require("@ds")
ds.setup()
local Markdown = require("@ds/markdown")
local text = ui.state("table_test_text", "| ID | Description |\n| --- | --- |\n| 1 | A long description with **bold words**, `inline code`, and a [link](https://example.com) that must wrap into several lines when its column narrows. |\n| 2 | Short. |")
local windowed = ui.state("table_test_windowed", false)
local view = ui.ref()
local offset = ui.state("table_test_offset", 0)
return ui.column {
	ref = view,
	w_full = true,
	h_full = true,
	min_w = 0,
	ui.presence(function(): string
		return if windowed:get() then "reply" else "preview"
	end, function(mode: string): Node
		return Markdown {
			text = function(): string return text:get() end,
			key = if mode == "reply" then "table-test-reply" else nil,
			window = if mode == "reply" then { key = "table-test-reply", view = view, offset = offset } else nil,
		}
	end),
}
