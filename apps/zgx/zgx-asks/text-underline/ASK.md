# Ask: a plain text draws its underline

**Who needs it:** every inline link in Zeron's rows (Installed's Update,
Yours' Publish…): `ui.text { …, underline = true, underline_offset = 4 }`.
Seen on paint by Zeron's capture lane on zgx `7f66d42fb` (Zeron's pin),
Linux and macOS: no line is drawn under the words, at rest or under the
pointer, so a link reads as bold words.

**What fails today:** a plain `ui.text` takes `underline` and
`underline_offset`, and they reach its font (`compose_style.rs`'s
`TextUnderline`, inherited as CSS's `text-decoration` is), but nothing paints
them: reading the source, only a rich text turns an underline into a drawn
line (`compose.rs`, a paragraph's underline reaching its runs, and each
span's own). The same props on a `ui.rich` span draw the line. (zgx tests
see no paint, so this is read from the code and the pictures, not from a
test.)

**The ask:** a plain text with `underline = true` draws its underline as a
rich text's span does: `underline_offset` below the baseline (a tenth of the
size unset), in the text's ink, through `hover`'s changes.

**Repro:** [`repro/`](repro/app.luau): `zgx run zgx-asks/text-underline/repro`. The
first two lines (plain texts) show no line under them; the third (a rich
text's span) does.

**Today's workaround in Zeron:** the row's one inline link
(`ds.Settings.InlineLink`) draws its words as a rich span, which paints its
underline.
