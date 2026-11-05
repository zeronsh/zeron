# Ask: popup windows placed at the window's zoom

**Priority: high (users see it).** In a window zoomed to anything but 100%, every menu and popover opens away from its anchor. A text menu opened by a right-click in Zeron's composer showed well below and right of the click, past the window's bottom. The user's window is at 90% (`~/Library/Application Support/zgx/zeron-zgx/zooms`: `zeron-zgx-main	0.9`).

**Seen on:** zgx `a2c9cf1` (Zeron's pin), macOS.

**Why:** the scene places popups in content points, which are AppKit points ÷ zoom. Pointer input already converts this way (`application.rs` ~4831: `1 / (scale_factor * zoom)`), and so does `popup_screens` (`popup_window.rs` ~1245, `/ zoom`). But the panel placement uses the scene rect as AppKit points:

- `popup_window.rs` ~845 `Panel::place`: `NSRect::new(rect.x, rect.y, rect.width, rect.height)` → `content.convertRect_toView` → `convertRectToScreen`, with no `* zoom`;
- the same at ~1588 (a drag preview's move) and ~1071.

At zoom z, a popup whose scene top-left is (x, y) shows at (x, y) AppKit points instead of (z·x, z·y). At 90%, that's 11% of its distance from the content's top-left further down and right. Its panel is also 1/z the size its content draws at (the renderer's scale is `backing × zoom`, ~1942). The panel's pointer origin (`ivars().origin`) probably wants the same treatment.

**Ask:** multiply the rect by the window's zoom (`zoom_of(window)`) wherever a panel's frame is set from a scene rect, and divide by it where panel-local input comes back. A test: a window at 0.9 with a menu at scene (500, 400) has its panel's screen origin at the content origin + (450, 360).

**Zeron:** nothing to work around on our side. In the scene the menu is at the click at 100% and at 90% (`cargo test --bin zeron-zgx placement_tests` in apps/zgx). To confirm live: ⌘0 (100%), and the same menu opens at the click.
