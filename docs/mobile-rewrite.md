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

## Build

`scripts/ios/build-core.sh` (run by the Xcode "Rust core" phase) builds
`zeron-mobile` for the active platform with the `mobile` cargo profile (always
optimized), and regenerates the committed UniFFI bindings in
`apps/ios/Zeron/Core/Generated/`.
