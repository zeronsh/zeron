# Ask: bring an element into view, not only a focused one

**Who needs it:** a store plugin's page (Settings → Plugins), its Versions.
A version's notes open whole in place (up to 64 KB, 27,000 pt tall); Show
less at their foot folds them. Folded from far down, everything above the
button shrinks, so the version's own head, and the button with it, ends up
far above the view. The version (its head first) should come back into
view. Seen on zgx `7f66d42fb` (Zeron's pin), headless.

**What fails today:** nothing in Luau brings a given element into view. Only
focus scrolls (`el:focus()`, "the least that shows the focused node"), and
it shows the node that takes focus: focusing a section hands focus to its
first focusable descendant (the button at its foot) and shows that, never
the section round it. So the button can be kept in view (blur, then focus
it: a real move), but the head above it can't, short of making a
non-control focusable for a moment.

**The ask:** `el:scroll_into_view(options?)` on any element: each scroll view
and list holding it scrolls the least that shows it (`align = "nearest"`,
the default; `"start"`, `"center"`, `"end"` as `scroll_to_row` takes them),
its `scroll_padding` kept, smoothly or not as the view's own scrolling is.
For a box taller than the view, `nearest` shows its start. As the web's
`Element.scrollIntoView({ block: "nearest" })`.

**Repro:** [`repro/`](repro/app.luau): `zgx test zgx-asks/scroll-into-view/repro`
with `ZGX_CHECK=0`. The first test fails (the section's head stays above
the view); the second, the control, passes (the focused button shows).

**Today's workaround in Zeron:** the button only. Folding, Show less blurs
and focuses its version's row, so the button that was pressed (focus lands
on it again) comes back into view at the view's edge; the version's head
stays above it until this lands, then `row:scroll_into_view()`.
