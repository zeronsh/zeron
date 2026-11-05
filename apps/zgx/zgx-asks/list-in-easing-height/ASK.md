# Ask: a virtual list in a box whose height eases, laid out without a panic

**Priority: medium.** A panic takes the whole window down. Zeron's sidebar
groups and the panel home's Side chats and Subagents lists fold with an
eased height; with about 200 rows in a fold, folding it panicked the app.
Zeron works around it (below), and any zgx app that eases a box round a
`ui.list` meets it.

**Today.** A `ui.list` (`data`, fixed `height`, its own `h`) inside a column
inside a box that clips (`overflow = "hidden"`) while its `h` eases from the
list's height to 0. Each row is a fixed-height `ui.row` holding a
`h_full` row. During the ease, zgx panics:

```
thread 'main' panicked at crates/zgx/src/widgets.rs:3238:37:
layout allocation did not settle after 64 passes; check size-dependent view mutations
```

No script mutates anything as it lays out: no `on_resize`, no refs read in
bindings. The column lets the list shrink below its own `h` inside the
shrinking box, and the list's rows (built for the height it's given) never
settle. Any one of these avoids it:
- the list straight in the box, with no column between them
- the row without its inner `h_full` row
- the column given the list's height and `shrink = 0` (Zeron's workaround,
  ds/session_row `Fold`: its content laid out at full height, the clip
  alone easing)

**Repro:** `ZGX_CHECK=0 zgx test zgx-asks/list-in-easing-height` (`app.luau`,
`tests/repro.test.luau`). It panics on zgx `f6506b6e6` while the box eases
shut.

**Ask:**
1. A list laid out in less room than its `h` (squeezed by its parent)
   settles: it builds rows for the room it has, once per layout, instead of
   reallocating until the pass limit.
2. Otherwise, at the limit, an error the app can see (`app:error()`, the
   inspector), not a panic, naming the node that didn't settle.

**Seen on:** zgx `f6506b6e6` (Zeron's pin), macOS, Nov 2.
