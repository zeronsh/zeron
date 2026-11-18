-- The same words underlined two ways. The first, a plain text with
-- `underline = true` (and an offset), draws no line under it; the second, a
-- rich text's span with the same props, does.

return ui.column {
	p = 24,
	gap = 24,
	ui.text { "Publish… (a plain text, underline = true)", underline = true, underline_offset = 4, text_size = 13, line_height = 16 },
	ui.text { "Publish… (a plain text, underline = true, no offset)", underline = true, text_size = 13, line_height = 16 },
	ui.rich {
		text_size = 13,
		line_height = 16,
		ui.span { "Publish… (a rich text's span, underline = true)", underline = true, underline_offset = 4 },
	},
}
