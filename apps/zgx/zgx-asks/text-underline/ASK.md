# Ask: a text's underline is drawn past its line box, as CSS's is

**Who needs it:** every inline link in Zeron's rows (Installed's Update,
Yours' Publish…), and any small text underlined clear of its descenders: a
13 pt text on a 16 pt line with `underline = true, underline_offset = 4`.
Seen on paint by Zeron's capture lane on zgx `7f66d42fb` (Zeron's pin).

**What fails today:** the underline is clipped by the text's own line box.
At `underline_offset = 4` on a 16 pt line the line lands at about +16, the
box's bottom edge, and nothing is drawn, at rest or under the pointer; at
offset 2 (+14) or 0 it shows; offset 4 on a 24 pt line shows (+20.5), as
Markdown's links do. Plain texts and rich spans alike. So a link's
underline can only sit where its line box has room, which for a small
line is nearer the glyphs than a link's underline should be.

(Measured on paint: zgx tests see none. An earlier reading of this ask,
that a plain text never paints its underline, was wrong: case 5 below
paints.)

**The ask:** a text's underline (and a span's) is drawn where its offset
puts it, past the line box if need be, as CSS's `text-underline-offset`
draws past the line, not clipped to the text's own box (a clip round the
text from an ancestor still applies).

**Repro:** [`repro/`](repro/app.luau): `zgx run zgx-asks/text-underline/repro`.
Its first line (offset 4 on a 16 line) shows no line; the second (offset 2)
and the third (offset 4 on a 24 line) do.

**Today's workaround in Zeron:** a row's inline link
(`ds.Settings.InlineLink`) underlines 2 under the baseline, which fits its
16 pt line.
