# Folded user bubbles: edge fade, and scrolling long chats

Captured on 2026-10-27 on Linux (X11, Mesa software Vulkan) from `message-markdown-fixture`. The `before-*` captures run the fixture against `main` `31d26eeb`; `before-*`/`after-*` share the fixture before its long report was added, and the `*-report*` captures come from the final build.

Build and run: `cargo run -p zeron-ui --features appshots-fixture --example message-markdown-fixture` (ZERON_PALETTE_LIGHT=1 for the light theme). `ZERON_FIXTURE_REPEAT=N` repeats the conversation N times; `ZERON_FIXTURE_PLAIN=1` strips the prompts' Markdown.

- A folded bubble's bottom fades out over its last line and a half instead of cutting a line in half above a `...` line; the `...` is gone (`before-*` vs `after-*`). Opening it shows the whole prompt with no fade (`after-dark-open.png`). The fade composes with the transcript's own fade, so a folded bubble still fades under the titlebar and composer.
- A folded prompt too long to fit lays out only the leading Markdown that fills the fold (`*-report.png`: a 240-item report shows its heading and first items); opening it lays out the rest (`after-dark-report-open.png`).

## Scroll cost

Main-thread CPU per rendered frame while wheel-scrolling the same ~28 s script over `ZERON_FIXTURE_REPEAT=100` (600 prompts, 100 of them 240-item reports), optimized builds with frame pointers, two runs each:

| | Markdown prompts | Plain prompts |
|---|---|---|
| `main` `31d26eeb` | 16.3 / 14.2 ms | 7.8 / 7.7 ms |
| this change | 3.2 / 3.7 ms | 3.0 / 3.8 ms |

`perf` on `main` put the transcript rail at 14% of main-thread samples: every frame it cloned the whole transcript, rebuilt every tick, matched each tick to its row by linear search, and built an element for every tick. Folded Markdown prompts laid out every block behind their five-line clip.

- `before-dark.png`: SHA256 `4813b4db7da39a429fc34151b40ebd0b0f5332da9a86f11eb43e8f45aae4c396`
- `after-dark.png`: SHA256 `364196210796f772cb9c28a4e6e4ef71daece970986d0c7cdf5d844f8dd6892f`
- `before-light.png`: SHA256 `a1902f11d44c7d94240e87bfcd95162937a3617d7085319cacebda0659947997`
- `after-light.png`: SHA256 `75ce886b848e74915fa0fd8ac9907ec0a3465fdb9824334a4a07aaade1b9fe19`
- `after-dark-open.png`: SHA256 `6f4046c4ed794a81d1249935ab9e9f6f6ded1e7f62cd14031c54dc11726c9305`
- `after-dark-report.png`: SHA256 `e0e5a0e953b0e4a6368b901f5c9f215bf762666109e50c4e1ff4f0e2fbb0630f`
- `after-light-report.png`: SHA256 `ab40beb12a4019cdedda066c23dd1e00503cbf4b3a3b09c3525cda3306acf138`
- `after-dark-report-open.png`: SHA256 `71123f83afe89223053d38ded6d6d5ff6886e3cc55d214c2225140f884d86e3e`
