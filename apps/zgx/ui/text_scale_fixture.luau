-- Interface text, the conversation's, a visual's and code's in a reply,
-- a paragraph's line and a Settings row's height, under the main window's look (ds/look
-- `window_theme`, as screens/shell wears it), the system's text size set
-- by the test (src/text_scale_tests.rs).
local ds = require("@ds")
ds.setup()
return ui.column {
	theme = function()
		return ds.look.window_theme()
	end,
	w = 600,
	ui.text { "Interface", id = "interface", text_size = "$font.size.md", line_height = "$font.line.md" },
	ui.text { "Conversation", id = "conversation", text_size = "$zeron.prose.size", line_height = "$zeron.prose.line" },
	ui.text { "Visual", id = "visual", text_size = "$zeron.visual.text.body.size", line_height = "$zeron.visual.text.body.line" },
	ui.text { "code", id = "code", text_size = "$zeron.prose.code.inline", line_height = "$zeron.prose.code.line" },
	ui.text { "Paragraph", id = "paragraph", text_size = "$font.size.md", line_height = "$zeron.paragraph.line.md" },
	-- How tall Settings lays a two-line row out (Archived's), as a number.
	ui.text { function()
		return tostring(ds.Settings.truncated_row_h())
	end, id = "row" },
}
