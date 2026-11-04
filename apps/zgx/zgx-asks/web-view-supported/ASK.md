# Ask: let a script know whether its web views are pages

**Priority: medium.** Zeron's browser tab must say why it can't show a page (on
Windows without the WebView2 runtime, or without Direct3D 12) instead of zgx's
grey "Web view · <url>" placeholder. Today it can't ask zgx.

**Today.** zgx-desktop decides once per window: `NativeHost::new` sets
`NativeViews::set_supported(webview2::available() && WGPU_BACKEND allows dx12)`
(native_host/mod.rs:125), and `renderer_for` turns it off again when the
DirectComposition tree or the visual's renderer can't be made ("zgx: native
views unavailable: …", mod.rs:193, 199). A script sees none of it: `ui.webview`
silently draws `placeholder()` (zgx/src/web_view.rs:1151), and `ui.web(id)`'s
page loads, titles and all, into nothing. Zeron now repeats zgx's first test
itself (apps/zgx/src/browser.rs: the static loader's
`GetAvailableCoreWebView2BrowserVersionString`, and `WGPU_BACKEND`) and shows its
own words; it can't see the second (a failed composition), where users still
get the placeholder.

**Repro:** `WGPU_BACKEND=vulkan zgx run repro` on Windows (or any Linux build
without `webkitgtk`): the page's node is the placeholder, and the line above
it, which is all a script can learn, says the page loaded.

**Ask:** `ui.webview_supported(): boolean`: the window's
`NativeViews::supported()`, tracked, so it turns false if `renderer_for` gives
up after the first frame, and a reason where there is one
(`ui.webview_unavailable(): ("runtime" | "renderer" | "platform")?` would do as
well). Zeron would then drop its own check (src/browser.rs) and read zgx's.

**Seen on:** zgx `99a50ad5` and `e46ff6fc` (Zeron's pin), Windows 11 26200, with zgx-desktop's
`webview2` feature (now on in Zeron for Windows).
