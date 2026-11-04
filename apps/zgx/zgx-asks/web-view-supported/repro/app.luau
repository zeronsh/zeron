-- Repro: where web views can't be pages (Windows with WGPU_BACKEND=vulkan,
-- or without the WebView2 runtime; Linux without `webkitgtk`), `ui.webview`
-- draws zgx's placeholder, and nothing a script can read says so: the page
-- reports its load as if it showed. Expected: `ui.webview_supported()`.
local web = ui.web("repro", { profile = "ephemeral" })
return ui.column {
	w_full = true,
	h_full = true,
	p = 12,
	gap = 8,
	ui.text {
		function(): string
			local asked = (ui :: any).webview_supported
			local supported = if asked then tostring(asked()) else "(no way to ask)"
			return `supported: {supported}  url: {web.url}  loading: {tostring(web.loading)}`
		end,
	},
	ui.webview { web = web, src = "https://example.com", grow = 1, rounded = 8 },
}
