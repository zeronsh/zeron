# Ask: repaint what a node mounted mid-scroll leaves when it goes

**Priority: high.** Users see it. A reply's hover lane (its time and Copy)
hangs in the empty space under the reply, far from its text, after a
scroll with the pointer over the transcript. It stays until something
paints there again: more scrolling or a resize. It looked like layout
(blank space between the text and the lane), but the layout is right; the
pixels are stale. It has been reported again and again.

**Today.** A measured `ui.list` (Zeron's transcript) scrolls under a still
pointer. A row's content mounts while the list moves (Zeron's hover lane,
a `ui.presence` turned on by `on_hover` or a lookup under the pointer),
and unmounts before the list stops (the pointer has left the row). The
window keeps the lane's last frame where it was last drawn. A full repaint
clears it.

- `partial_repaint` off: no ghost (2 of 2 runs). `scroll_copy` off: ghost
  still (2 of 2). So a damage rect is missed, not a scroll copy.
- The lane mounted at rest, then unmounted mid-scroll: no ghost (2 of 2).
  Mounted mid-scroll, unmounted mid-scroll: a ghost (8 of 8).
- The lane's fade-in (`enter` opacity) removed: a ghost still (2 of 2).
- `zgx inspect snapshot` shows it, and so does `screencapture` of the window.

Likely place: whatever records where a node was painted, for the damage of
its removal, has a stale rect for a node mounted in a frame where an
ancestor moved (`scene.rs`, `resolve_moves` and the damage of a removed
subtree).

**Repro** (from `apps/zgx`, release app built, `ZGX_CHECK=0`):
`zgx-asks/stale-paint-mounted-mid-scroll/repro/run.sh`. It runs
`chat_fixture.luau` with the lane as it was before Zeron's workaround
(`ds/transcript.luau` at `dd32e339`), flings the list past a still pointer
with a wheel tape (`up400.tape`, `down400.tape`), and compares what was
drawn with the same frame painted whole. It prints
`DIFF … px box 547,619-…`: the lane's ghost at the pointer's height.
`same` would mean fixed.

**Ask:** a node removed (or changed) in a frame is damaged where it was
last painted, whatever moved since it mounted, so no partial frame
leaves it on screen.

**Seen on:** zgx `aaa304e` (Zeron's pin, the release app), macOS 27.

**Done (Nov 2):** fixed in zgx `c43b4e5ee` (#1368): a move that repaints
also damages where the content was. In Zeron's pin `f6506b6e6`. The app's
workaround (a lane shown only after 90 ms of rest) is gone; the lane
follows the pointer mid-scroll again. Tests: `ui/tests/hover_lane.test.luau`.
