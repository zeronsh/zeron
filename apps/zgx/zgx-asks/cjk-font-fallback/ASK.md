# Ask: system font fallback for scripts the registered family lacks

**Priority: high.** Any Chinese, Japanese or Korean text (a reply, a file name, a session title, a pasted line) draws as blank space. Users can't read their own content.

**Who needs it:** every Zeron surface that shows user or model text; first found in the browser panel's annotation chips.

**Today.** With the host's fonts registered (Geist, Geist Mono), `ui.text { "该域名仅用于文档示例 default" }` draws only the Latin part; the Chinese glyphs are blank, in the default face and the mono face alike. With `font = "PingFang SC"` the whole run is blank. Cyrillic shapes fine. A web view beside it shows the same text correctly.

**Repro:** `zgx run repro` (`ZGX_CHECK=0`): two lines of mixed CJK and Latin text, one in the default face and one in mono.

**Ask:** fall back to system fonts per run for characters the requested family lacks (CoreText's cascade list on macOS, fontconfig on Linux, DirectWrite's fallback on Windows), as browsers do.

**Seen on:** zgx `b3371f8` (Zeron's pin), macOS. From the browser chat's notes.

## Update: still blank on `c9d85c6` (Nov 1), with the cause

Rendered offscreen through Zeron's audit rig (`src/audit_shots.rs`, zgx's GPU renderer; `AUDIT_TEXT_LANGUAGE` sets the text language):

- Japanese (a line with kana) and Korean now draw: Hiragino and Apple SD Gothic Neo are found.
- **Chinese still draws blank:** a line of Han without kana, in the default face, in mono, and with the text language `zh-Hans` (what `ui.i18n` sets for a Chinese locale). With `zh-Hans` set, even the Japanese line's kanji go blank.
- The cause: `han_locale` sends Han to the Chinese list, whose first face is PingFang SC. `load_pingfang` loads `/System/Library/PrivateFrameworks/FontServices.framework/Resources/Reserved/PingFangUI.ttc`, which on macOS 27.0.1 (26A434) has **no `glyf` or `CFF` table**: its outlines are only in Apple's `hvgl` table (tables: BASE GSUB HVAR OS/2 STAT VORG avar cidg cmap fvar head hhea hmtx **hvgl** maxp meta name post trak vhea vmtx). The face covers every codepoint, so fallback stops there, and swash draws nothing for it. `font = "PingFang SC"` draws a blank line, Latin included.
- `font = "Heiti SC"` (`/System/Library/Fonts/STHeiti Light.ttc`, `glyf` outlines) draws Chinese correctly, so the cascade's next face would work.

**Ask, narrowed:** skip a face with no outlines the rasterizer can draw (no `glyf`/`CFF`/`CFF2`, only `hvgl`) when building the fallback lists, so Heiti SC (or Hiragino Sans GB) takes Chinese Han. Better still, rasterize `hvgl` through Core Text for those glyphs. Zeron's `zh-Hans` locale (ui/locales) is ready and waits on this.

Repro offscreen (no window, no TCC): a fixture with `ui.text { "设置 · 通用 · 语言" }`, then
`AUDIT_OUT=<dir> AUDIT_SHOTS="<abs path>.luau@600x220" AUDIT_LIGHT=1 AUDIT_TEXT_LANGUAGE=zh-Hans cargo test --bin zeron-zgx audit_shots -- --ignored --nocapture`.

## Fixed in zgx PR #1340 (head 7b6e0d517), verified Nov 2 on macOS 27.0.1

Fallback passes faces it can't rasterize (`hvgl`-only PingFang) for the next covering face. zgx's `chinese_text_on_macos_draws` and `undrawable_fonts` pass on this Mac. Zeron built against it (scratch copy, pin unchanged) draws Chinese, Japanese, Korean and mixed lines offscreen, and its Settings and command palette fully in 简体中文. Close this ask once Zeron's pin moves past 7b6e0d517. Cosmetic: Han looks a weight heavier than the Latin beside it (STHeiti Medium for regular?).

**Layer 2, zgx PR #1342 (944fa878d), verified Nov 2 on macOS 27.0.1:** `hvgl` faces draw through Core Text, so Chinese takes PingFang itself at Regular, matching Geist's weight (the STHeiti fallback from #1340 drew heavier). zgx's core_text_glyphs, text_scripts and undrawable_fonts pass 15/15. Zeron built against it shows Settings in PingFang. Move the pin to 944fa878d or later and close this ask.
