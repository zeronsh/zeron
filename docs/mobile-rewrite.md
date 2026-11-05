# iOS rewrite — Rust core, UIKit shell

The iOS app is rebuilt from scratch around one rule: **Rust decides what to
paint and where; the platform only paints, scrolls and handles gestures.** The
same Rust library will back the Android app.

## Why

The SwiftUI app hosted SwiftUI inside every transcript cell, self-sized rows by
measuring hosted views, re-decoded the whole Loro doc and rebuilt every row per
streamed token, and re-rendered the whole session screen on each token. Home
re-rendered every row on any registry frame. The sync layer was a hand port of
crates that already exist in Rust.

## Layers

```
crates/text      zeron-text      pretext-style text engine: UAX#14 segmentation,
                                 rustybuzz measurement on the bundled Geist faces
                                 (platform fallback for uncovered glyphs), cached
                                 segment widths, pure-arithmetic line layout
crates/markdown  zeron-markdown  block model + append-incremental reparse
                                 (extracted from the desktop; desktop re-exports it)
crates/client    zeron-client    engine-free viewer device: registry + chat2 rooms
                                 (zeron-sync), docs (zeron-doc), relay RPCs, auth,
                                 view models (front page ordering = desktop sidebar:
                                 pins, sections, recency), demo dataset
crates/mobile    zeron-mobile    UniFFI facade (Swift today, Kotlin later) + the
                                 transcript layout engine: rows → measured display
                                 lists, prefix-sum offsets, visible-range queries
apps/ios                          UIKit shell: tab bar + glass, virtualized
                                 transcript painter (CoreText at Rust positions),
                                 composer, sheets
```

## Transcript pipeline

1. `zeron-client` applies chat2 rows to the session doc and republishes an
   immutable snapshot; unchanged entries stay pointer-equal.
2. The layout engine (background thread) turns changed entries into rows
   (one per top-level markdown block / tool group / user message), parses
   markdown incrementally, and lays each row out with `zeron-text` at the
   viewport width. A row's output is a **display list**: text runs (UTF-16
   ranges + style id + exact x/baseline), boxes (code/quote/table/bubble),
   link hit rects, horizontal scrollers.
3. Heights are exact before a row is ever shown; offsets live in a prefix-sum
   tree, so `visible(y0, y1)` is a binary search. No estimates, no self-sizing.
4. Swift draws each run with CoreText at the Rust-computed position, so
   measurement and rendering can never disagree about line breaks.

## Data path per streamed token

The session doc's `subscribe_root` observer marks only the touched entry maps;
`zeron-client` re-decodes those entries and republishes a snapshot in which
every other entry is the same `Arc`. The layout thread reuses rows for
pointer-equal entries, re-parses only the streaming tail block
(`IncrementalParser`), re-measures only that row, and rebuilds the prefix sums.
Swift receives one coalesced "frame ready" per display frame; if the visible
tail row changed, its new display model is built off the main thread and swapped
in while the old one keeps painting. Newly appended text fades in (veil) on a
separate layer, split at the exact glyph offset.

## Verification

| What | Where |
| --- | --- |
| Paint/measure agreement at 7 widths, streaming ≡ full parse, prefix reuse, toggles, mentions | `cargo test -p zeron-mobile --lib layout` |
| Layout timing (3,300 rows) | `cargo test --release -p zeron-mobile --lib bench_layout -- --ignored --nocapture` |
| Line breaks vs CoreText on the same font bytes | `ZeronTests/LineBreakAccuracyTests`, `crates/text/tests/coretext.rs` |
| Demo end-to-end (send → echo → reply, questions, queue, offline host) | `crates/client/tests/demo.rs`, `ZeronUITests/SessionFlowTests` |
| Live sync against an in-process edge | `crates/client/tests/live.rs` |
| Real stack (wrangler dev edge + headless engine, mock harness) | `ZeronUITests/LiveStackTests` |
| Frame pacing (hitch ratio, idle + streaming) | `-lab -bench`, `ZeronUITests/ScrollPerformanceTests` |

Reference numbers (M-series Mac, iPhone 17 Pro simulator): cold layout of
3,300 rows 30 ms; width change 0.42 ms; display list 0.8 µs/row; streamed
token 0.19 ms; display-link flings through 3,300 rows — 0 hitches idle
(671 frames) and while streaming (536 frames).

## Build

`scripts/ios/build-core.sh` (run by the Xcode "Rust core" phase) builds
`zeron-mobile` for the active platform with the `mobile` cargo profile (always
optimized), and regenerates the committed UniFFI bindings in
`apps/ios/Zeron/Core/Generated/`.
