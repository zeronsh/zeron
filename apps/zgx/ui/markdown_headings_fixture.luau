-- A text with a heading of each level, drawn as a chat reply draws it and
-- as a document under a page's title does (ds/markdown's `heading_step`),
-- side by side (tests/markdown_headings).

local ds = require("@ds")
ds.setup()
local Markdown = require("@ds/markdown")

local TEXT = table.concat({
	"Words before.",
	"",
	"# One",
	"",
	"Words under one.",
	"",
	"## Two",
	"",
	"Words under two.",
	"",
	"### Three",
	"",
	"Words under three.",
}, "\n")

return ui.row {
	w_full = true,
	gap = "$space.6",
	p = "$space.4",
	items = "start",
	ui.column { id = "md-chat", grow = 1, basis = 0, Markdown { text = TEXT } },
	ui.column { id = "md-doc", grow = 1, basis = 0, Markdown { text = TEXT, heading_step = 1 } },
}
