-- `zgx run repro/main.luau` on Windows, then flip Settings →
-- Personalization → Colors → Transparency effects. The line should follow
-- the switch, and the card over the stripe turn opaque with it.
local prefers = ui.prefers :: any

return ui.column {
	w_full = true,
	h_full = true,
	items = "center",
	justify = "center",
	gap = 16,
	bg = "#2193b0",
	ui.text {
		function(): string
			return `reduced_transparency: {tostring(prefers.reduced_transparency:get())}`
		end,
		color = "#ffffff",
	},
	ui.div {
		relative = true,
		w = 240,
		h = 120,
		-- What the card should hide once transparency is off.
		ui.div { absolute = true, left = -40, top = 40, w = 320, h = 40, bg = "#ff5f6d" },
		ui.div {
			absolute = true,
			left = 0,
			top = 0,
			w = 240,
			h = 120,
			rounded = 12,
			bg = "$glass.tint",
			backdrop_blur = "$glass.blur",
			border = 1,
			border_color = "$border",
		},
	},
}
