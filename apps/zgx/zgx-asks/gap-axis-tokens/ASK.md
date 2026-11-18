# Ask: `gap_x` and `gap_y` take a theme token, as `gap` does

**Who needs it:** `ds.Settings.Row`'s `detail_link` (Settings → Plugins →
Yours): a sentence and its link in a wrapping row, a word's space
(`$space.1`) between them and none between their lines. Seen on zgx
`7f66d42fb` (Zeron's pin), headless.

**What fails today:** `gap_x = "$space.1"` spaces nothing (0), where
`gap_x = 4` and `gap = "$space.1"` both space 4. And with `gap = "$space.1",
gap_y = 0` on a wrapping row, the lines come 4 apart: the token `gap` wins
over the number `gap_y`. The design lint, meanwhile, flags `gap_x = 4` and
asks for `"$space.1"`, the form that is dropped.

**The ask:** `gap_x` and `gap_y` resolve a theme token as `gap` and every
other spacing prop do, and refine `gap` whichever form each is given in.

**Repro:** [`repro/`](repro/app.luau): `zgx test zgx-asks/gap-axis-tokens/repro`
with `ZGX_CHECK=0`. The first test fails (0 against 4); the controls pass.

**Today's workaround in Zeron:** `gap_x = ui.raw(ui.theme.get("space.1"), …)`,
the token's number with the reason, in `ds/settings.luau`'s `detail_link`.
