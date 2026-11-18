# Ask: `gap_x`, `gap_y` and `min_h` take a theme token, as `gap` and `h` do

**Who needs it:** `ds.Settings.Row`'s `detail_link` (Settings → Plugins →
Yours): a sentence and its link in a wrapping row, a word's space
(`$space.1`) between them and none between their lines. Seen on zgx
`7f66d42fb` (Zeron's pin), headless.

**What fails today:** `gap_x = "$space.1"` spaces nothing (0), where
`gap_x = 4` and `gap = "$space.1"` both space 4. And with `gap = "$space.1",
gap_y = 0` on a wrapping row, the lines come 4 apart: the token `gap` wins
over the number `gap_y`. The design lint, meanwhile, flags `gap_x = 4` and
asks for `"$space.1"`, the form that is dropped.

**`min_h` too (found Nov 18):** `min_h = "$zeron.settings.row"` (52) lays a
row out at its content's height (19 for a line of text), where `min_h = 52`
and `h = "$zeron.settings.row"` both give 52. So `ds.Settings.Row`, which
asks for that least, is as tall as what it holds everywhere in Zeron: 42
with a name alone, 48 with a switch, 60 with a line under the name. (Fixing
it raises every bare row in Zeron to 52: Zeron decides that look change on
its side, and wants the prop to do what it says either way.)

**The ask:** `gap_x`, `gap_y` and `min_h` (and any size or spacing prop that
doesn't yet) resolve a theme token as `gap`, `h` and the padding props do,
and `gap_x`/`gap_y` refine `gap` whichever form each is given in.

**Repro:** [`repro/`](repro/app.luau): `zgx test zgx-asks/gap-axis-tokens/repro`
with `ZGX_CHECK=0`. The first test fails (0 against 4); the controls pass.

**Today's workaround in Zeron:** `gap_x = ui.raw(ui.theme.get("space.1"), …)`,
the token's number with the reason, in `ds/settings.luau`'s `detail_link`.
