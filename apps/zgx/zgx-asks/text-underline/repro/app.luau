-- The same words underlined three ways. The first, offset 4 on a 16 pt
-- line, draws no line (it falls on the box's bottom edge and is clipped);
-- the second, offset 2 on the same line, and the third, offset 4 on a 24 pt
-- line, draw theirs.

return ui.column {
	p = 24,
	gap = 24,
	ui.text { "Publish… (offset 4, a 16 line: no line)", underline = true, underline_offset = 4, text_size = 13, line_height = 16 },
	ui.text { "Publish… (offset 2, a 16 line: a line)", underline = true, underline_offset = 2, text_size = 13, line_height = 16 },
	ui.text { "Publish… (offset 4, a 24 line: a line)", underline = true, underline_offset = 4, text_size = 13, line_height = 24 },
}
