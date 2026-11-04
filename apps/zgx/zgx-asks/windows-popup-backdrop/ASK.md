# Ask: a popup's glass on Windows blurs nothing

**Priority: high.** Users see it. On Windows a popover, menu, tooltip or
hover card with a translucent fill and `backdrop_blur` shows what's behind
it sharp: the model picker over a new chat's dithered wallpaper shows the
dither's dots through its panel, around its slider ("a black bar with a
dotted pattern").

**Today.** Each popup is a window of its own (`popup_window/win32.rs`). It
gets no system backdrop: DWM draws a never-activated window's backdrop flat
(the comment at `corner_of`), and a popup mustn't activate. And
`backdrop_blur` blurs only what's drawn in the popup's own window, which is
nothing of the app's. So the fill's alpha shows the parent window's pixels
(and the desktop) unblurred. macOS's popups get AppKit's material; Linux's
(`99a50ad5`) zgx's own blur.

**Repro:** a `ui.popover` with `bg = "#111113a6"`, `backdrop_blur = 24`,
over an image, on Windows.

**Ask** (either):

1. Blur what zgx itself drew under the popup: the parent window's last
   frame, cropped to the popup's rect, as the popup's backdrop texture (the
   parent and its popups share one GPU context). That's what an app's glass
   means in a window: the app's own content, blurred.
2. Or say that a popup can't blur (a `ui.prefers`-like
   `ui.window.popups_blur`, or `backdrop_blur` falling back to the fill made
   opaque), so an app draws its glass solid there.

**Zeron meanwhile:** the host says `system.popups_blur = false` on Windows
(`src/transparency.rs`), and `ds/material.menus_see_through()` makes the
menus' glass solid there (`$zeron.menu.solid`, as Windows draws its own
non-activated menus). In-window glass keeps its blur.

**Seen on:** zgx `99a50ad5`, unchanged at `e46ff6fc`; Windows 11 26200.
