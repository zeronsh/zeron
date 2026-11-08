# Ask: Windows caption buttons an app can live beside

**Priority: high.** Users see it. With `Titlebar::Inset` on Windows, zgx draws
the minimise, maximise and close buttons itself (`window_chrome.rs`,
`CaptionHost::buttons`) and answers `WM_NCHITTEST` for them, which is what
gives Windows 11's snap layouts. An app can't replace them (they're mounted
over its content, and `Titlebar::Hidden` drops the hit-testing with them),
so each of these is the app's to live with until zgx changes it. Seen on
Zeron's pin `99a50ad5` (the caption code is the same at `e46ff6fc`), Windows 11 26200, at 100%.

## 1. The glyphs follow the system's light or dark, not the window's

`CaptionHost::sync` sets `dark` from `window.theme()`. On Windows winit's
`set_theme` (vendored, `platform_impl/windows/window.rs:970`) only calls
`try_theme`, which sets DWM's dark mode; it never updates
`window_state.current_theme`, which is what `theme()` returns. So
`WindowHandle::set_theme(Some(Light))` makes Mica light, but the caption
glyphs stay white: **invisible on a light app on a dark system** (and black
on a dark app on a light one).

Repro: any `Titlebar::Inset` window; `window.set_theme(Some(Appearance::Light))`
on a system in dark mode. Zeron: `src/main.rs` `set_appearance` does exactly
this as the scheme changes; screenshots `light-*.png` in the PR.

**Fix:** in `set_theme`, set `window_state.preferred_theme` and
`current_theme` to what `try_theme` returns (as window creation does at
`:1151`), or have `CaptionHost` read the requested theme
(`WindowState::theme`, falling back to `window.theme()`).

## 2. Drawn with an icon font: tofu where it's missing

The glyphs are text in `Segoe Fluent Icons` (Windows 11), else `Segoe MDL2
Assets` (`window_chrome.rs:1048`), U+E921/E922/E923/E8BB. A tester on
Windows 10 sees empty boxes. (zgx's fallback should find MDL2 there, so
which step fails on that machine is unconfirmed; the shipping gpui app,
`crates/ui/src/shell.rs`, names Fluent alone and fails the same way.) An
icon font is also the one part of the window that zgx doesn't draw itself:
its weight, hinting and size follow the font, not the app.

**Fix:** draw them as zgx draws Linux's (`linux` `glyph`: bars and boxes of
`div`s), or as SVG paths, pixel-snapped: minimise a 10 by 1 bar; maximise a
10 by 10 square, 1 stroke; restore two 8 by 8 squares offset 2; close two
10-point diagonals, 1 stroke (WinUI's sizes at 100%, scaled by DPI). Better
still, let the app hand its own glyphs (`WindowOptions::caption_glyphs`:
an SVG per button), so Zeron's match its own icon set.

## 3. Zoomed, the buttons drawn, hit and reported disagree

The buttons are mounted as ordinary content, so they zoom with it; but
`hit` sizes them by DPI alone, and `WindowInfo::refresh` divides `controls`
by the zoom ("the platform's own buttons do not [zoom]", true on macOS only).
At 150% zoom on a 1240-point window, the buttons are drawn 69 px apart
(46 × 1.5, 207 px in all), Windows hit-tests 138 px, and `controls` says
92 points (138 px). So a click on the drawn Minimise's glyph lands in the
client area, and an app clearing `controls` lays its own controls under
the drawn buttons. (Zeron's controls centred on `controls`' middle stood
past the window's top until Zeron clamped them.)

**Fix:** don't zoom them: mount them outside the zoomed root, or scale their
view by 1/zoom, so drawn, hit-tested and reported all agree in window
pixels.

## 4. Their height is fixed at 32

`BUTTON_HEIGHT` is 32 whatever the app's title bar is. Zeron's is 38
(the shipping app's caption buttons span it, as WinUI's do in a tall title
bar), so the buttons stop 6 points short of the bar and Zeron centres its
controls on their line (16 down) rather than its bar's.

**Fix:** `WindowOptions::caption_height` (or the `Titlebar::Inset` strip's
height), used for drawing, hit-testing and `controls` alike.

## 5. Non-resizable windows still show and hit Maximise

Setup's window is `resizable: false`; its Maximise button still shows and
still hit-tests as `HTMAXBUTTON` (and the resize bands still answer, per
`hit`, which checks `IsZoomed` alone). Expected: Maximise hidden or disabled,
no resize bands, as Linux's `hit` already does (`is_resizable`).

**Zeron meanwhile** (`ui/ds/title_bar.luau`): its controls centre on the
reported buttons' middle, never nearer the top than an island's padding;
its actions end a divider's room before them; over a wallpaper an island
of glass hangs behind them; `set_appearance` sets the window's theme (so
Mica follows; the glyphs wait on 1). Nothing it can do for 2, 3's hit
test, or 5.
