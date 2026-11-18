# Ask: a list scrolled by the view around it builds its rows when it fades in

**Who needs it:** Settings → Plugins → Browse (the plugin store). Its rows
are a `ui.list` with `virtualize = "ancestor"` (`ds.Settings.ListRows`, as
Settings → Archived's `EachList`), in a card whose content is a
`ui.presence` that fades each kind in (loading, the rows, nothing found), as
every Settings catalog does. The page itself fades in too when it comes
back from a plugin's page. Seen on zgx `b80a8af1` (Zeron's pin).

**What fails today:** a list built as its presence swaps to it with an
opacity `enter` builds no rows. The card is as tall as all its rows (its
`heights` are known) and empty, until the view around it scrolls or the
window resizes; then the rows in view appear at once. With the same
presence without `enter` (with `mode = "wait"` and `exit` alone, too), the
rows are built at once.

**Where it seems to come from** (reading `compose.rs`'s `AncestorWindow`,
not proven): `window()` asks `scene.visible_bounds(viewport)`, which looks
to count an ancestor at opacity 0 as showing nothing, so the first run
finds no rows; it runs again only as the observed bounds move (the list,
the scroll views, the boxes between), and an opacity rising to 1 moves
none of them. The fix #1446 made for a box easing open (it follows clips)
would want the same for an ancestor's opacity, or for virtualization to
ignore opacity (rows under a fade are still on screen as it rises).

**The ask:** a `virtualize = "ancestor"` list builds the rows in its view
while an ancestor fades in (a presence's `enter`, an `opacity` tween), as
one not fading does.

**Repro:** [`repro/`](repro/app.luau): `zgx test zgx-asks/ancestor-list-fading-in/repro`
with `ZGX_CHECK=0`. The first test fails (no rows built after 60 frames);
the second, the same list without the enter, passes.

**Today's workaround in Zeron:** none taken. Dropping the fade from the
store's card would leave the page that fades in around it (coming back
from a plugin's page) with the same empty card, and trades the motion
every catalog has. Browse waits on this.
