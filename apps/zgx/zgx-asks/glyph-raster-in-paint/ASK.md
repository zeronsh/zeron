# Ask: rasterize new glyphs ahead of the frame that shows them

**Priority: medium.** While a reply streams, about two frames per stream take 13-17 ms of paint, past a 120 Hz budget, where new text first appears (new words, bold, code). Visible as small hitches while reading a streaming reply.

**Today.** Streaming a reply at 1100x720 (release, Zeron's main window): p50 1.8 ms, p95 11 ms, max 19 ms per frame; the slow frames are paint, on frames where glyphs not seen before are rasterized into the atlas.

**Repro:** stream text in a new style into a `ui.rich` (or Zeron's transcript, chat_fixture `go_s6` then `long_reply`) and read `zgx inspect frames`: the slow frames' time is in `paint`.

**Ask:** rasterize newly shaped glyphs off the main thread as text is shaped, or spread a burst of new glyphs over a few frames (drawing those that are ready), so one frame of new text stays inside the budget.

**Seen on:** zgx `b3371f8` (Zeron's pin), macOS.
