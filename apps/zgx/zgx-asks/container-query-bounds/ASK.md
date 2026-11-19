# Ask: a container query's bounds can follow a state

**Who needs it:** `ds.Settings.Row`'s `give_way` (Settings → Plugins →
Installed and Yours). A row's controls move under its name in steps as the
row gets narrower: its badges first, then its detail's action under its
sentence, then its switch and Remove. Each step starts at a row width that
depends on what the row's parts say: its version, Unknown author, Update and
its controls, measured in the real font. Seen on zgx `7f66d42fb` (Zeron's
pin).

**What fails today:** `queries` takes literal bounds when the node is built
(`node.rs`, `"queries"`), so a step whose bound is a measured width can't be
a query. Zeron reads the row's width after layout (`on_resize`) and sets the
step from it, so in a frame where the step changes, layout runs twice: once
at the old step, then again after the script. With 30 rows crossing a step at
once, that frame lays out about 3.3x a steady resize frame's nodes (the capture
rig: 5.7 to 10 ms of layout, a steady frame 2.2 ms).

**The ask:** a query's `min_width`/`max_width` (and the height bounds) take
a state or a function, as other props do, read when it changes (rarely:
when what a row says changes), and applied in layout as a literal bound is.
A resize then crosses a step in one layout pass, with no script.

**Today's workaround in Zeron:** the step is read from widths after layout
and set by script; a step-crossing frame lays out twice.
