# Ask: hide the inset toolbar in full screen

**Priority: high.** Users see it. In full screen (the green button,
⌃⌘F), an opaque grey bar 40 points tall covers the top of the window, under
the menu bar: the sidebar's first rows, the page's top and the panel's
header.

**Today.** `Titlebar::Inset` (`crates/zgx-desktop/src/window_chrome.rs`,
`mac::inset_buttons`) gives the window an empty compact unified `NSToolbar`
to centre the traffic lights. In full screen AppKit moves that toolbar into
an `NSToolbarFullScreenWindow` over the content. `titlebarAppearsTransparent`
doesn't reach that window: its `NSTitlebarBackgroundView` has an opaque layer
background of (0.118, 0.118, 0.118, 1), which is #1E1E1E. The window's
`contentLayoutRect` loses those 40 points (1044 of 1084), and a full-size
content view draws under them. Nothing in zgx changes the toolbar on
entering or leaving full screen.

With the toolbar hidden while in full screen, AppKit still makes the strip
window, but at 32 points and alpha 0 (shown only with the menu bar's
reveal), and `contentLayoutRect` is the whole window. Nothing covers the
content.

**Repro** (no zgx needed; it's the same AppKit calls as `content_under_titlebar`
and `inset_buttons`): `swiftc -O -o fsrepro repro/main.swift`, then
`./fsrepro /tmp/asis` and `./fsrepro fix /tmp/fix`. Each opens a window, goes
full screen for about two seconds, and prints its own strip window: as-is,
`NSToolbarFullScreenWindow` 40 pt, alpha 1, with an opaque #1E1E1E
`NSTitlebarBackgroundView`. With `fix` (the toolbar's `isVisible` false from
`windowWillEnterFullScreen`, true again on `windowDidExitFullScreen`): 32 pt,
alpha 0.

**Ask:** with `Titlebar::Inset`, hide the window's toolbar while it's in full
screen, and show it again on leaving. In full screen the traffic lights are
hidden anyway. `application.rs` already notices the change ("Into or out of
fullscreen: its popups close"); `window_chrome::refresh` could set
`toolbar.isVisible = !fullscreen` there, or observe
`windowWillEnterFullScreen` and `windowDidExitFullScreen`.

**Seen on:** zgx `dda319ccd` (Zeron's pin), macOS 27.0.1, with the menu bar
showing in full screen.

**Done upstream (Nov 2):** zgx `465858d8` (#1413) hides the inset toolbar on
`NSWindowWillEnterFullScreen` and shows it again on `DidExitFullScreen`. That
covers the green button and `set_fullscreen`. Waiting on Zeron's pin bump
(the adoption chat owns it).
