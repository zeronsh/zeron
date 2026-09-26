//! zeron-text — analytic text measurement and line layout for virtualized transcripts.
//!
//! A Rust port of the technique behind chenglou/pretext: split text layout into a one-time
//! [`prepare`] and a width-dependent layout that is pure arithmetic, so a virtualized list can
//! compute every row's height (and re-compute it on rotation/resize) without touching a text
//! engine.
//!
//! **prepare** normalizes whitespace per CSS `white-space`, finds UAX #14 break opportunities
//! (CJK per-ideograph breaks, kinsoku classes, hyphens, slashes, NBSP/WJ glue, ZWSP, soft
//! hyphens), cuts the text into segments with their trailing collapsible whitespace split off
//! (it hangs at a line end), splits segments into per-span pieces, and measures every piece.
//! Breaks never land inside an extended grapheme cluster, so emoji ZWJ sequences, flags,
//! keycaps and skin tones stay whole.
//!
//! **layout** ([`Prepared::line_count`], [`Prepared::stats`], [`Prepared::walk_lines`],
//! [`Prepared::lines`]) walks the cached widths greedily like a browser: fill the line, break at
//! the last opportunity that fits (a chosen soft hyphen adds a visible `-`), let trailing spaces
//! hang, and — under `overflow-wrap: anywhere` — split a run between graphemes only when it
//! can't fit on a line of its own. Re-layout at a new width never re-measures.
//!
//! Measurement ground truth: pieces the style's face covers are shaped with rustybuzz (kerning
//! and standard ligatures on, like CoreText; ligatures can be turned off per style) and
//! `width = Σ x_advance · size / units_per_em`, plus `letter_spacing` per grapheme. Per-grapheme
//! advances for splitting come from the same shaping pass via glyph clusters. A piece containing
//! any visible char missing from the face's cmap (or requesting emoji presentation) is measured
//! whole by the host's [`FallbackMeasurer`] — the platform engine with its font cascade — which
//! is exactly what will render it. With no fallback registered such pieces are shaped anyway and
//! measure with the face's `.notdef` advance.
//!
//! Every measurement goes through a [`WidthCache`] keyed by `(StyleId, &str)`: hits hash and
//! compare bytes without allocating, so warm `prepare` is dominated by hashing.
//!
//! Conventions: an empty paragraph (or one that normalizes to nothing) has zero lines; a
//! trailing forced break does not start an extra empty line; widths are points; byte ranges index
//! [`Prepared::text`] (the normalized text), fragments also carry UTF-16 ranges for NSString.

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
pub use prepare::{OverflowWrap, PrepareOptions, Prepared, Span, WhiteSpace, prepare};

#[cfg(test)]
mod tests;
