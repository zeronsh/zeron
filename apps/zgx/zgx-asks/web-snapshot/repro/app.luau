-- Repro: a web page's node snapshots as an empty box. Expected: a Web
-- snapshot API (see ../browser-panel-asks.md).
local web = ui.web({ profile = "ephemeral" })
local box = ui.ref()
local shot = ui.state("shot", nil :: string?)
return ui.column {
	w_full = true,
	h_full = true,
	p = 12,
	gap = 8,
	ui.row {
		gap = 8,
		ui.button {
			"Snapshot the page's node",
			on_click = function()
				local el = box:peek()
				shot:set(if el then el:snapshot(480) else nil)
			end,
		},
		ui.text {
			function()
				return `title: {web.title}  loading: {tostring(web.loading)}`
			end,
		},
	},
	ui.row {
		grow = 1,
		gap = 8,
		ui.webview { ref = box, web = web, src = "https://example.com", grow = 1, rounded = 8 },
		ui.column {
			w = 480,
			rounded = 8,
			border = 1,
			border_color = "#888",
			ui.show(function()
				return shot:get() ~= nil
			end, ui.image {
				src = function()
					return shot:get() or ""
				end,
				fit = "contain",
				w_full = true,
				grow = 1,
			}, ui.text { "(no snapshot yet)", p = 8 }),
		},
	},
}
