# Ask: a trackpad stretch never left behind

**Priority: medium.** A candidate cause, not proven, for Settings' nav sometimes staying shifted up by about 10 rows: the top rows are hidden under the search field, with a large gap above the account row, in a window tall enough for every row.

**Today.** `elastic.rs` springs back only `State::active`. `after_scroll` replaces `active` whenever the active view doesn't match the event's axis, even while that view is still stretched or springing. So a vertical view released at its end, followed within its spring (about 0.4 s) by a sideways gesture that stretches a horizontal view, keeps its `shown` translation for good. Once that view fits (`limit() == 0`, for example after a taller window), no gesture picks it again (`after_scroll` filters on `limit() > 0`). It stays shifted until it's remounted.

**Repro:** `repro.rs`: drop it into `crates/zgx/tests/` and run `cargo test --test repro`. It fails with the list stuck at -52.4.

**Not the cause in Settings, as far as Zeron could find:** Settings has no horizontal scroller, so this exact path can't happen there. A headless fuzz ran 40 seeds × 40 gestures on Zeron's shell fixture, with every axis vertical. It fed well-formed AppKit phase sequences (MayBegin/Cancelled, momentum starting within and after 50 ms) through a copy of the desktop host's `Trackpad` machine. It mixed in resizes, page changes, reloads and leaving and reopening Settings mid-gesture. The nav never stayed out of place. A lost `Ended` leaves a stretch held, but the next gesture's `Ended` releases it.

**Ask:**
1. On `Ended`/`MomentumEnded`, spring back every stretched view, not only `active`. Or, when `after_scroll` moves `active` to another view, release the one it leaves.
2. Never keep a stretch on a view that can't scroll: when `limit()` becomes 0 and no gesture holds it, spring back.
3. If (1) and (2) don't explain Zeron's report: a way to see a view's stretch (`zgx inspect`: `shown`/`pushed`/`active` per scroll view), so the next stuck nav can be told apart from an offset past its range.

**Seen on:** zgx `aaa304eb2` (Zeron's pin), macOS, Nov 2.
