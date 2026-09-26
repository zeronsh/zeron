//! zeron-text — analytic text measurement and line layout for virtualized transcripts.
//!
//! A Rust port of the technique behind chenglou/pretext: split text layout into a one-time
//! [`prepare`] and a width-dependent layout that is pure arithmetic, so a virtualized list can
//! compute every row's height (and re-compute it on rotation/resize) without touching a text
//! engine. The target is *the platform's own line breaker*: on Apple platforms the line starts
//! match `CTFramesetter` on the same font bytes (tests/coretext.rs measures this on macOS: 100%
//! of 48k cases across prose, code, URLs, CJK, Thai, RTL and emoji at 3 faces × 3 sizes × 30
//! widths, ≥ 99.99% on held-out random corpora).
//!
//! # prepare
//!
//! 1. **Normalize** whitespace per CSS `white-space`, remapping span ranges.
//! 2. **Break opportunities**: Unicode 17 UAX #14 via ICU4X (the CLDR/ICU rules CoreText uses),
//!    with Apple's tailoring (curly `‘ “` break like opening brackets, `”` like a closing one)
//!    and ICU's dictionaries for Thai/Lao/Khmer/Myanmar. Never inside a grapheme cluster, so
//!    emoji ZWJ sequences, flags, keycaps and skin tones stay whole. Atomic spans are one
//!    U+FFFC. Whitespace inside a UAX #14 segment (`( x`, `8 !=`) becomes a
//!    [`SegmentBreak::WhitespaceOverflow`] boundary (see *layout*).
//! 3. **Segments**: text between opportunities, trailing whitespace split off as a hang.
//! 4. **Measure** each segment's per-span pieces through the [`WidthCache`] (keyed by
//!    `(StyleId, &str)`, so words repeat for free):
//!    - text the style's face covers is shaped with rustybuzz (kerning and standard ligatures on,
//!      like CoreText; ligatures can be turned off per style), `Σ x_advance · size / upem`, plus
//!      `letter_spacing` per grapheme;
//!    - text it doesn't cover goes to the host's [`FallbackMeasurer`] (the platform engine with
//!      its font cascade). With [`FallbackMeasurer::measure_run`], whole uncovered runs are
//!      measured once in context — CoreText picks fallback fonts and kerns per run — and each
//!      piece takes its share; otherwise each piece is measured on its own.
//! 5. **Seams**: a paragraph is shaped as a whole by the platform, so segments measured apart
//!    get the difference back at each boundary: the pair context of the two graphemes around it
//!    (kerning lands on the left glyph; a ligature or contextual form spanning it, like Geist's
//!    `->` arrow, puts its whole advance on the left and makes the right glyph a "lead" that
//!    only counts mid-line). No context crosses a script-run change (ICU `UScriptRun`
//!    semantics, resetting at fallback text), a bidi level change, a style change, padding, or
//!    an atomic span — CoreText shapes those separately too. An unwrapped line therefore equals
//!    the paragraph shaped whole.
//! 6. **Units** for overflow splitting: per-grapheme advances from glyph clusters; a ligature
//!    spanning graphemes is one unit (its tail graphemes have zero advance and never start a
//!    line).
//!
//! # layout
//!
//! [`Prepared::line_count`], [`Prepared::stats`], [`Prepared::walk_lines`] and
//! [`Prepared::lines`] walk the cached widths greedily, the way CoreText's typesetter does:
//!
//! - advances accumulate in `f64`; a line fits when its advance ≤ `max_width` +
//!   [`LINE_FIT_EPSILON`] (CoreText's own slack, 0.0002pt);
//! - a line's advance includes its last glyph's kerning with the next line's first glyph
//!   (CoreText fits against the paragraph's glyph advances) — except on a line that starts inside
//!   a substituted glyph pair, which CoreText shapes anew;
//! - whitespace hangs; if the line overflows *inside* whitespace it ends right after it, even at
//!   a `WhitespaceOverflow` boundary (so, like CoreText's, line counts are not always monotone
//!   in width);
//! - otherwise the line ends at the last opportunity that fits (a chosen soft hyphen adds a
//!   visible `-`); an overlong run moves to a fresh line first and, under
//!   `overflow-wrap: anywhere`, is split at the overflowing grapheme; with no opportunity on the
//!   line at all it splits there too (CoreText's character wrapping).
//!
//! Re-layout at a new width never re-measures.
//!
//! Conventions: an empty paragraph (or one that normalizes to nothing) has zero lines; a
//! trailing forced break does not start an extra empty line; widths are points; byte ranges
//! index [`Prepared::text`] (the normalized text), fragments also carry UTF-16 ranges for
//! NSString / Java strings. Fragments are in logical order; paragraphs with right-to-left text
//! ([`Prepared::has_rtl`]) should be drawn a line at a time by a bidi-aware renderer.

mod analysis;
mod cache;
mod chars;
mod font;
mod layout;
mod prepare;

pub use cache::{CacheStats, WidthCache};
pub use font::{
    FaceId, FallbackMeasurer, FontBook, FontError, FontMetrics, Style, StyleId, StyleOptions,
};
pub use layout::{Cursor, Fragment, LINE_FIT_EPSILON, Line, LineRange, LineStats};
pub use prepare::{
    OverflowWrap, PrepareOptions, Prepared, SegmentBreak, Span, WhiteSpace, prepare,
};

#[cfg(test)]
mod tests;
