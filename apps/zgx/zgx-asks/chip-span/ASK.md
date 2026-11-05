# Ask: an inline chip span as one pill

**Who needs it:** Zeron's sent messages. A file or chat mention (`[chat.rs](zeron-file:src/chat.rs)`) shows as a chip in the line, as the shipping app's bubble draws it (crates/ui transcript.rs `user_bubble_text`, composer/chip.rs `paint_chip`): an icon in a small well, then the name, on one rounded wash (corner 5), clickable. The app draws it as a `ui.span` in ds/markdown.luau (`mention_chip`).

**What's wrong** (zgx `b3371f8b`):

1. **A clickable span is always underlined.** `on_click` underlines the span even with `underline = false`, and the underline runs under its padding. The types say: "a link (`on_click`) under the pointer: underlined … With it, the link isn't underlined at rest unless `underline` says so."
2. **A span's `icon` and `padding` are washed apart from its text.** With `bg` and `radius`, the icon gets a rounded box of its own, the text another, and the trailing padding a sliver of its own (gone with `padding = nil`). The types say of `padding`: "with `bg`, the background hugs the text plus this (an inline code chip)".

## The repro

`zgx-run app.luau` (one window, three lines; `app.luau` beside this file):

- Line 1, clickable: underlined under the text and its padding.
- Line 2, with an icon: three pieces of wash (icon box | text box | padding sliver).
- Line 3: both.

## Wanted

Each line one rounded wash around icon, gap, text and padding, not underlined at rest (the hover's `underline`, if set, still applies). Nice to have: an icon well (its own fill, inset 1, corner 4), as chip.rs's `paint_chip` draws.

## Meanwhile (what the app does)

`underline_color = "#00000000"` hides the underline. There's no span padding; its room is thin spaces (U+2009) inside the text, so a selection across a chip copies them. The icon keeps its own box.
