-- Mixed CJK and Latin text: the CJK glyphs draw as blank space.
return ui.column {
	gap = 12,
	p = 24,
	ui.text { "该域名仅用于文档示例 default face" },
	ui.text { "日本語のテキスト and 한국어 text", font = "mono" },
	ui.text { "Кириллица shapes fine" },
}
