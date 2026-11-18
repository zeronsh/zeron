# Ask: a rich paragraph breaks at a span's boundary as it breaks between words

**Headless only** (the zgx coordinator, Nov 18): it's zgx's fallback text
layout, the one `zgx test` runs with no renderer, wrapping by grapheme
rather than by word (the 62 below is 8 × its fixed 7.75 advance). With
the real shaper the app wraps a rich paragraph at a span's boundary as it
should. zgx's text lane is making the fallback wrap by word (UAX #14).

**Who needs it:** Settings → Plugins → Yours. Each of your plugins says where
it stands in the store in one sentence, and "Publish…" is that sentence's own
link: it should follow the sentence's last word, wrapping with it. That's
one `ui.rich`: the sentence as one span, a space ending it, then the link as
a `ui.span { on_click }` (`ds.Settings.Row`'s `detail_link`). Seen on zgx
`7f66d42fb` (Zeron's pin), headless.

**What fails today:** when the line has to break exactly at the boundary
between the two spans (the link doesn't fit after the words, whose span ends
in a space), the break isn't taken there: the link's word is broken inside
instead. Its first fragment stays on line one, cut at the line's end, and the
rest goes to line two. In the repro at 248 wide, "Share it in the plugin
store " + "Publish…": the tree's link box (its first fragment) is 16 wide at
line one's end, of the 62 it is on one line. The same happens with the space
as a span of its own, or leading the link's span. When the break falls
inside the first span (the control, at 300 wide: before "approved"), words
move whole and the link stays whole on line two.

(Read from the boxes: zgx tests see no paint. The first fragment's place and
width are what an accessible link and its hit area use, so a person pointing
or tabbing meets a link cut in two either way.)

**The ask:** a rich paragraph treats the boundary between two spans as the
break opportunity the text there gives (a space ending one span, or starting
the next), as a single span's text does: a word that doesn't fit at a line's
end moves whole to the next line, whichever span it's in, and is broken
inside only when it's wider than the line.

**Repro:** [`repro/`](repro/app.luau): `zgx test zgx-asks/rich-span-boundary-break/repro`
with `ZGX_CHECK=0`. The first test fails (16 against 62); the second, the
control, passes.

**Today's workaround in Zeron:** none in zgx's place. `detail_link` draws the
sentence and its link as two boxes in a wrapping row for now: the link a
word's space after the sentence when it fits on that line, else on the next
line at its start. One line, that's the same picture; when the sentence
itself wraps, the link goes under it rather than after its last word.
Correct in both the app and the tests (confirmed on paint), it's kept as
it is; once the pin carries the fallback's word wrap it may become the one
rich paragraph.
