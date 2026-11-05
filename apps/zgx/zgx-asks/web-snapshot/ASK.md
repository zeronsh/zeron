# Ask: a picture of a web view (and an image's bytes)

**Priority: medium.** Zeron's browser panel attaches an annotated picture of the page to a message; today it can't take one.

**Today.** `Element:snapshot` and `Term:snapshot` give an image `src`, but a `ui.webview` is a native view and shows as what's under it: the repro's snapshot of the node holding the page is an empty box. There's also no way to get an image `src`'s bytes to disk for the agent.

**Repro:** `zgx run repro` (`ZGX_CHECK=0`): loads a page, snapshots its node and shows the result beside it.

**Ask:** `Web:snapshot(opts: { rect, max_width }?, reply: (src?, err?) -> ())` (macOS `WKWebView.takeSnapshot(with:)`, WebView2 `CapturePreview`, WebKitGTK `webkit_web_view_get_snapshot`); and either `ui.image_bytes(src) -> string?` (PNG) or `ui.save_image(src, path, reply)`.

**Seen on:** zgx `b3371f8` (Zeron's pin), macOS. From the browser chat's notes.
