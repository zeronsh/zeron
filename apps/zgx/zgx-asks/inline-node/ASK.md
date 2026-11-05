# Ask: a node inside a paragraph's text

**Who needs it:** Zeron's replies, for inline LaTeX math ("where $e^{i\pi} + 1 = 0$ holds"). It would also serve file-link chips, mention pills and emoji images, which Zeron's shipping app draws over reserved space in a line.

**What exists:** a paragraph is `ui.rich { "text", ui.span { … } }`. The only non-text a span holds is `icon = { src, size, gap, tint }`: a square SVG, centred in the line (`compose_rich.rs`, `InlineIcon`, `(rect.height - icon.size) / 2`). A formula has its own width, and its own height above and below the baseline. Here it is 74 × 22, with 6 below the baseline.

## The repro

`zgx test apps/zgx/zgx-asks/inline-node` (zgx `b3371f8b`):

```text
node in ui.rich: runtime error: ui.span takes a string, a state or a function, not userdata
@4 #row 16,16 288x23
  @5 text "where " 16,16 58x23
  @8 text " holds, and the words wrap around it as a word would." 148,16 509x23
```

1. A node among a paragraph's spans is refused.
2. The workaround, a row of text, node and text, can't wrap. The last text runs 509 wide out of a 288 column. The node also sits on the row's alignment, not the text's baseline.

## The API we'd use

Either of these, in order of preference:

**A. A node as a span.** `ui.span { node = <Node>, w = 74, h = 22, baseline = 16 }`:
- The line keeps `w` of room, an unbreakable "word", as the icon placeholder does today.
- The node is laid out at `w` × `h`, its top `baseline` above the line's baseline.
- The line box grows to fit it, as a tall glyph would.
- Selection treats it as one character, and copy gives a `text` the span names (`text = "$e^{i\\pi}$"`), so copying a paragraph gives the TeX back.
- The node is a real child: it draws, hit-tests and can be a `ui.canvas`. `w`/`h`/`baseline` may be functions or states, so it re-flows in place when they change, like other props.

**B. Sized icons, if A is too much.** `icon = { code = <svg>, w, h, baseline }`, keeping today's `size` as `w = h = size` centred:
- We'd hand it an SVG of the formula's outlines (zatex's `Formula::to_svg`, `fill="currentColor"` so it takes the span's colour and follows the theme).
- We'd also need the same `text =` for copy.

With either, Zeron's inline math matches its display math (`ds/math.luau`, which draws on a `ui.canvas`). Until then, inline math shows as its TeX in an inline code chip.
