-- A reply with every kind of block markdown draws, for plugins that place
-- by them (`.md-code.lang-<language>`, `.md-math`, `.md-table`, `.md-image`,
-- `.md-quote`, `.md-heading`): src/plugins/markdown_tests.rs mounts it with
-- plugins-dev/md-hello loaded or not. `stream` writes more of the last
-- block, as a reply streaming in does.

ui.theme.strict("warn")
local ds = require("@ds")
ds.setup()
local Markdown = require("@ds/markdown")
local Math = require("@ds/math")

-- (No host here to lay math out: a box of its size stands in.)
local stub = Math :: any
stub.lay_out = function(tex: string, display: boolean?): any
	local w = #tex * 0.4
	return { w = w, ascent = 1, descent = 0.3, left = 0, path = { 0, 0, 0, 1, w, 0, 1, w, 1, 1, 0, 1, 4 } }
end

local START = table.concat({
	"## A heading",
	"",
	"> A quote.",
	"",
	"| a | b |",
	"|---|---|",
	"| 1 | 2 |",
	"",
	"![a picture](pic.png)",
	"",
	"$$x^2$$",
	"",
	"```lua",
	"print(1)",
	"```",
	"",
	"```hello",
	"first block",
	"```",
	"",
	"```hello",
	"second",
}, "\n")

local text = ui.state("md_blocks_text", START)

ui.command {
	id = "stream",
	title = "Write more of the last block",
	run = function()
		text:set(text:peek() .. " block, streaming")
	end,
}

return ui.column {
	id = "reply",
	selectable = true,
	w_full = true,
	p = "$space.6",
	Markdown {
		key = "md-blocks",
		text = function(): string
			return text:get()
		end,
	},
}
