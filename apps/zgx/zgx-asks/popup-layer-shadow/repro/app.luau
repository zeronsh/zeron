-- Rest the pointer on the button: a card painting its own shadow shows in
-- a dark rim the shadow's reach bigger than it (the window server outlines
-- the soft shadow too). Without its shadow, one frame.
local card = ui.column {
	w = 240,
	p = 12,
	gap = 4,
	rounded = 8,
	border = 1,
	border_color = "#ffffff1f",
	bg = "#1c1c1fa6",
	backdrop_blur = 30,
	shadow = "0 20 56 0 #00000052",
	ui.text { "Context window", text_size = 11, color = "#ffffff80" },
	ui.text { "408,389 / 1,000,000 tokens", text_size = 13, color = "#ffffffee" },
}
return ui.column {
	w_full = true,
	h_full = true,
	items = "center",
	justify = "center",
	bg = "#111113",
	ui.button {
		"Rest here",
		px = 12,
		py = 6,
		tooltip = card,
		tooltip_placement = { side = "bottom", align = "center", offset = 8 },
	},
}
