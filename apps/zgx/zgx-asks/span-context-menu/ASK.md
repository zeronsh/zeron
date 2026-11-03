# Ask: a context menu for a span (a link in a paragraph)

**Priority: medium.** Every link in Zeron's replies and messages is a run of
a paragraph (`ui.span`). Zeron wants a context menu on each that suits the
link:
- **Web links:** Open, Open in default browser, Copy link.
- **File links:** Open, Reveal in Finder, Copy path, Copy relative path.
- **Chat links:** Open chat, Copy link.

Today none of these can be built.

**Today.**
- A span's only menu is the one zgx gives an `href` span (open or copy), and
  its items can't be changed.
- Most of Zeron's links can't use `href` anyway: it opens with the system
  handler after `on_click` (see [href-cancel](../href-cancel/ASK.md)). Web
  links open in Zeron's own browser panel. File links (`zeron-file:`) open
  in the session's panel. Chat links (`zeron-chat:`) open the chat.
- The paragraph (`ui.text` of runs) takes `context_menu`, but its
  `PointerEvent` says only where the pointer was, not which run, so the menu
  can't know which link it is for.

**Ask:** one of these, the first preferred:
1. `context_menu` on `SpanProps`, as on nodes: `(e: PointerEvent) -> { Node }?`,
   the items shown at the pointer. On an `href` span it replaces the
   built-in menu.
2. The run under the pointer in a paragraph's `PointerEvent` (for example
   `e.span`, the run's index among the paragraph's runs), so the paragraph's
   own `context_menu` can pick the link.

Either way, a secondary click on a span's text should still leave a text
selection's own menu (Copy) when the click lands in a selection rather than
on a link.

**Seen on:** zgx `b3371f8` (Zeron's pin), macOS.
