-- Repro: a span's on_click runs, then the href opens in the system browser
-- too, whatever on_click returns. Expected: `return false` keeps it closed
-- (see ../browser-panel-asks.md).
local heard = ui.state("heard", 0)
return ui.column {
	p = 16,
	gap = 8,
	ui.rich {
		ui.span { "Click this link: " },
		ui.span {
			"example.com",
			href = "https://example.com",
			underline = true,
			on_click = function()
				heard:set(heard:peek() + 1)
				return false
			end,
		},
	},
	ui.text {
		function()
			return `on_click heard {heard:get()} time(s); the browser opened too`
		end,
	},
}
