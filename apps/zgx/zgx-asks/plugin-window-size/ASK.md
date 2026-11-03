# Ask: a plugin can read the window's size

**Who needs it:** any plugin that draws something over the window: the design system's lightbox (`ui/ds/lightbox.luau`, lent to plugins as `@ds`) opens a picture in a `ui.modal` fitted to the window, and reads `ui.window.size` for that. GitHub's plugin opens pictures from pull request comments in it.

Seen at zgx `c9d85c65` (Zeron's pin).

## What fails today

`windows::install` (`zgx-script/src/windows.rs`) leaves `ui.window`'s `__index` out for a plugin VM (`Grants::Only`), so `ui.window.size` is `nil` there. The lightbox then falls back to 1 by 1, and the modal opens at that size: a click on a picture appears to do nothing. The title bar's area and the window's controls are rightly the app's alone, but the size of the window a plugin draws in is no secret from it.

## Zeron's workaround (to remove once this lands)

Zeron shares the size as `zeron.window` (`{ width, height }`, `src/plugins` `share_window_size`, exposed to every plugin without a grant), and the lightbox reads `ui.shared("zeron.window")` where `ui.window.size` is missing.

## The ask

`ui.window.size` (read-only, as `active` and `shown` are) in a plugin VM too, without `Capability::Windows`.
