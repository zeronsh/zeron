//! `prepare()`: normalize, segment, and measure once; the result lays out at any width.

use std::ops::Range;

use crate::analysis::{self, BRK_SOFT_HYPHEN, RawSeg};
use crate::cache::{UnitsRef, WidthCache};
use crate::chars::is_strong_rtl;
use crate::font::{FontBook, StyleId};

/// CSS `white-space` modes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum WhiteSpace {
    /// Collapse runs of spaces/tabs/newlines to one space, trim both ends, wrap.
    #[default]
    Normal,
    /// Collapse spaces/tabs, keep `\n` as a forced break (spaces around it removed), wrap.
    PreLine,
    /// Preserve spaces and tabs, `\n` forces a break, wrap; trailing spaces/tabs hang.
    PreWrap,
    /// Preserve everything, break only at `\n`, never wrap.
    Pre,
}

/// CSS `overflow-wrap`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum OverflowWrap {
    /// An unbreakable run wider than the line overflows it.
    Normal,
    /// An unbreakable run wider than the line breaks between grapheme clusters.
    #[default]
    Anywhere,
}

/// Options for [`prepare`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PrepareOptions {
    /// Whitespace processing and wrapping mode.
    pub white_space: WhiteSpace,
    /// What to do with runs wider than the line.
    pub overflow_wrap: OverflowWrap,
    /// Tab stop interval in space widths (`PreWrap`/`Pre`; 0 renders tabs zero-width).
    pub tab_size: u8,
}

impl Default for PrepareOptions {
    fn default() -> Self {
        Self {
            white_space: WhiteSpace::Normal,
            overflow_wrap: OverflowWrap::Anywhere,
            tab_size: 4,
        }
    }
}

/// A styled run of the input. Spans must cover the text contiguously and in order.
#[derive(Clone, Debug, PartialEq)]
pub struct Span {
    /// Byte range into the text (input text for [`prepare`], normalized text in
    /// [`Prepared::spans`]).
    pub range: Range<usize>,
    /// Style the run is measured in.
    pub style: StyleId,
    /// Extra advance before the span's first grapheme (e.g. inline-code chip padding).
    pub pad_start: f32,
    /// Extra advance after the span's last grapheme.
    pub pad_end: f32,
    /// Lay the whole span out as one unbreakable unit (a mention chip): line breaking treats it
    /// like a CSS atomic inline (U+FFFC).
    pub atomic: bool,
}

impl Span {
    /// A plain span.
    pub fn new(range: Range<usize>, style: StyleId) -> Self {
        Self {
            range,
            style,
            pad_start: 0.0,
            pad_end: 0.0,
            atomic: false,
        }
    }

    /// Sets `pad_start`/`pad_end`.
    pub fn with_padding(mut self, start: f32, end: f32) -> Self {
        self.pad_start = start;
        self.pad_end = end;
        self
    }

    /// Sets `atomic`.
    pub fn with_atomic(mut self, atomic: bool) -> Self {
        self.atomic = atomic;
        self
    }
}

// Segment flags.
pub(crate) const F_DYN_W: u8 = 1; // content contains a tab: width depends on line position
pub(crate) const F_DYN_H: u8 = 2; // hang contains a tab
pub(crate) const F_BREAKABLE: u8 = 4; // overflow-wrap may split the content (>1 unit)

// Piece kinds.
pub(crate) const P_TEXT: u8 = 0;
pub(crate) const P_TAB: u8 = 1;
pub(crate) const P_ATOMIC: u8 = 2;

/// Per-segment data the line walker touches on every step.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Hot {
    /// Advance of the visible content (static unless `F_DYN_W`).
    pub width: f32,
    /// Advance of the trailing hanging whitespace when the line continues past it.
    pub hang: f32,
    pub brk: u8,
    pub flags: u8,
}

/// Per-segment data needed for ranges, fragments, splits and soft hyphens.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Cold {
    pub start: u32,
    pub content_end: u32,
    pub ws_end: u32,
    /// Stored pieces: content at `pieces..pieces + n_content`, hanging whitespace right after
    /// (`n_hang`). A region that is one non-tab run of one span (the common case) stores no
    /// piece: it is implied by the segment's byte range, `span`/`hang_span`, and `Hot`.
    pub pieces: u32,
    pub n_content: u32,
    pub n_hang: u32,
    pub span: u32,
    pub hang_span: u32,
    /// Breakable units (grapheme clusters; an atomic span or a tab is one unit) live at
    /// `units..units + n_units` in [`Prepared::units`] when `F_BREAKABLE`.
    pub units: u32,
    pub n_units: u32,
    /// Hyphen advance shown when the line breaks at this segment's soft hyphen.
    pub hyphen: f32,
    /// Kind of the implicit content piece.
    pub kind: u8,
}

/// A same-span, same-kind slice of a segment.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Piece {
    pub start: u32,
    pub end: u32,
    pub span: u32,
    /// Advance including span padding; for tabs, the tab-stop interval.
    pub width: f32,
    /// Index of the piece's first unit within its segment, and how many units it has.
    pub unit0: u32,
    pub n_units: u32,
    pub kind: u8,
}

/// Text measured once and ready to lay out at any width with pure arithmetic.
///
/// Holds the normalized text, spans remapped onto it, per-segment widths, and per-grapheme
/// advances for segments that `overflow-wrap: anywhere` may have to split.
#[derive(Clone, Debug)]
pub struct Prepared {
    pub(crate) text: String,
    pub(crate) spans: Vec<Span>,
    pub(crate) hot: Vec<Hot>,
    pub(crate) cold: Vec<Cold>,
    pub(crate) pieces: Vec<Piece>,
    pub(crate) units: Vec<f32>,
    pub(crate) wrap: bool,
    /// Wrapping with `overflow-wrap: anywhere`.
    pub(crate) anywhere: bool,
    pub(crate) tab_size: u8,
    pub(crate) ascii: bool,
    pub(crate) rtl: bool,
    pub(crate) options: PrepareOptions,
}

impl Prepared {
    fn empty(text: String, spans: Vec<Span>, opts: &PrepareOptions) -> Self {
        Self {
            ascii: text.is_ascii(),
            text,
            spans,
            hot: Vec::new(),
            cold: Vec::new(),
            pieces: Vec::new(),
            units: Vec::new(),
            wrap: opts.white_space != WhiteSpace::Pre,
            anywhere: opts.white_space != WhiteSpace::Pre
                && opts.overflow_wrap == OverflowWrap::Anywhere,
            tab_size: opts.tab_size,
            rtl: false,
            options: *opts,
        }
    }

    /// The normalized text every byte range refers to.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The input spans remapped onto [`Prepared::text`] (same count and order; a span whose
    /// text collapsed away has an empty range). `Fragment::span` indexes this slice.
    pub fn spans(&self) -> &[Span] {
        &self.spans
    }

    /// Options this was prepared with.
    pub fn options(&self) -> &PrepareOptions {
        &self.options
    }

    /// Whether the text contains any strong right-to-left character (bidi class R or AL).
    /// Fragments are always in logical order; RTL-bearing paragraphs should be drawn a whole
    /// line at a time by a bidi-aware renderer.
    pub fn has_rtl(&self) -> bool {
        self.rtl
    }

    /// True when there is nothing to lay out (zero lines at any width).
    pub fn is_empty(&self) -> bool {
        self.hot.is_empty()
    }

    /// Number of break-opportunity segments (diagnostics).
    pub fn segment_count(&self) -> usize {
        self.hot.len()
    }

    /// UTF-16 code-unit offset of byte offset `byte` in [`Prepared::text`].
    pub fn utf16_offset(&self, byte: usize) -> usize {
        if self.ascii {
            return byte;
        }
        utf16_len(&self.text.as_bytes()[..byte])
    }

    /// Segment byte ranges `(content, hanging whitespace, forced-break char)` (diagnostics and
    /// tests; the three ranges of a segment are adjacent and segments tile the text).
    pub fn segment_ranges(&self) -> Vec<(Range<usize>, Range<usize>, Range<usize>)> {
        self.cold
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let end = self
                    .cold
                    .get(i + 1)
                    .map_or(self.text.len(), |n| n.start as usize);
                let (s, ce, we) = (
                    c.start as usize,
                    c.content_end as usize,
                    c.ws_end as usize,
                );
                (s..ce, ce..we, we..end)
            })
            .collect()
    }

    /// Content pieces of segment `i`.
    #[inline]
    pub(crate) fn content_run(&self, i: usize) -> Run<'_> {
        let c = &self.cold[i];
        if c.n_content > 0 {
            Run::Stored(&self.pieces[c.pieces as usize..(c.pieces + c.n_content) as usize])
        } else if c.content_end > c.start {
            Run::One(Piece {
                start: c.start,
                end: c.content_end,
                span: c.span,
                width: self.hot[i].width,
                unit0: 0,
                n_units: c.n_units,
                kind: c.kind,
            })
        } else {
            Run::Stored(&[])
        }
    }

    /// Hanging-whitespace pieces of segment `i`.
    #[inline]
    pub(crate) fn hang_run(&self, i: usize) -> Run<'_> {
        let c = &self.cold[i];
        if c.n_hang > 0 {
            let a = (c.pieces + c.n_content) as usize;
            Run::Stored(&self.pieces[a..a + c.n_hang as usize])
        } else if c.ws_end > c.content_end {
            Run::One(Piece {
                start: c.content_end,
                end: c.ws_end,
                span: c.hang_span,
                width: self.hot[i].hang,
                unit0: 0,
                n_units: 0,
                kind: P_TEXT,
            })
        } else {
            Run::Stored(&[])
        }
    }

    /// Heap bytes held (diagnostics).
    pub fn heap_bytes(&self) -> usize {
        self.text.capacity()
            + self.spans.capacity() * std::mem::size_of::<Span>()
            + self.hot.capacity() * std::mem::size_of::<Hot>()
            + self.cold.capacity() * std::mem::size_of::<Cold>()
            + self.pieces.capacity() * std::mem::size_of::<Piece>()
            + self.units.capacity() * 4
    }
}

/// A segment region's pieces: stored, or the single implicit one.
pub(crate) enum Run<'a> {
    Stored(&'a [Piece]),
    One(Piece),
}

impl Run<'_> {
    #[inline]
    pub(crate) fn as_slice(&self) -> &[Piece] {
        match self {
            Run::Stored(s) => s,
            Run::One(p) => std::slice::from_ref(p),
        }
    }
}

/// UTF-16 length of UTF-8 bytes: one unit per scalar, two for 4-byte scalars.
#[inline]
pub(crate) fn utf16_len(bytes: &[u8]) -> usize {
    bytes
        .iter()
        .map(|&b| ((b & 0xC0) != 0x80) as usize + (b >= 0xF0) as usize)
        .sum()
}

/// Prepares `text` for layout: normalizes whitespace per `opts.white_space`, finds UAX #14 break
/// opportunities, splits styled pieces, and measures every piece through `cache`.
///
/// `spans` must cover `text` contiguously in order; invalid spans are debug-asserted and
/// repaired (each span clamped to follow the previous one, the last extended to the end).
/// Empty text — or text that normalizes to nothing — yields a `Prepared` with zero lines.
///
/// # Panics
/// If a span references a `StyleId` that is not from `book`.
pub fn prepare(
    book: &FontBook,
    cache: &mut WidthCache,
    text: &str,
    spans: &[Span],
    opts: &PrepareOptions,
) -> Prepared {
    cache.bind(book);
    if text.is_empty() || spans.is_empty() {
        debug_assert!(text.is_empty(), "prepare: spans must cover the text");
        let spans = spans
            .iter()
            .map(|s| Span {
                range: 0..0,
                ..s.clone()
            })
            .collect();
        return Prepared::empty(text.to_owned(), spans, opts);
    }
    let repaired = analysis::repair_spans(text, spans);
    debug_assert!(
        repaired.is_none(),
        "prepare: spans must cover the text contiguously in order on char boundaries"
    );
    let spans = repaired.as_deref().unwrap_or(spans);

    let (ntext, nspans) = analysis::normalize(text, spans, opts.white_space);
    if ntext.is_empty() {
        return Prepared::empty(ntext, nspans, opts);
    }

    let mut scratch = std::mem::take(&mut cache.scratch);
    analysis::break_opportunities(&ntext, &nspans, opts.white_space, &mut scratch.breaks);
    analysis::segments(&ntext, &scratch.breaks, opts.white_space, &mut scratch.segs);

    let wrap = opts.white_space != WhiteSpace::Pre;
    let tabs =
        matches!(opts.white_space, WhiteSpace::PreWrap | WhiteSpace::Pre) && ntext.contains('\t');
    scratch.pieces.clear();
    scratch.units.clear();
    let mut b = Builder {
        book,
        cache,
        text: &ntext,
        spans: &nspans,
        tabs,
        tab_size: opts.tab_size,
        want_units: wrap && opts.overflow_wrap == OverflowWrap::Anywhere,
        pieces: std::mem::take(&mut scratch.pieces),
        units: std::mem::take(&mut scratch.units),
        span: 0,
    };
    let n = scratch.segs.len();
    let mut hot = Vec::with_capacity(n);
    let mut cold = Vec::with_capacity(n);
    for seg in &scratch.segs {
        let (h, c) = b.segment(seg);
        hot.push(h);
        cold.push(c);
    }
    // Exact-size copies; the growable buffers go back to the cache for the next prepare.
    let pieces = b.pieces.as_slice().to_vec();
    let units = b.units.as_slice().to_vec();
    scratch.pieces = std::mem::take(&mut b.pieces);
    scratch.units = std::mem::take(&mut b.units);
    b.cache.scratch = scratch;
    let ascii = ntext.is_ascii();
    let rtl = !ascii && ntext.chars().any(is_strong_rtl);
    Prepared {
        text: ntext,
        spans: nspans,
        hot,
        cold,
        pieces,
        units,
        wrap,
        anywhere: wrap && opts.overflow_wrap == OverflowWrap::Anywhere,
        tab_size: opts.tab_size,
        ascii,
        rtl,
        options: *opts,
    }
}

struct Builder<'a> {
    book: &'a FontBook,
    cache: &'a mut WidthCache,
    text: &'a str,
    spans: &'a [Span],
    tabs: bool,
    tab_size: u8,
    want_units: bool,
    pieces: Vec<Piece>,
    units: Vec<f32>,
    /// Span containing the current position (monotone).
    span: usize,
}

struct Region {
    width: f32,
    has_tab: bool,
    n_units: u32,
}

impl Builder<'_> {
    fn segment(&mut self, seg: &RawSeg) -> (Hot, Cold) {
        let pieces = self.pieces.len() as u32;
        let units = self.units.len() as u32;
        let content = self.region(seg.start, seg.content_end, true);
        let (n_content, span, kind) = self.implicit(pieces);
        let hang_at = self.pieces.len() as u32;
        let hang = self.region(seg.content_end, seg.ws_end, false);
        let (n_hang, hang_span, _) = self.implicit(hang_at);
        let hyphen = if seg.brk == BRK_SOFT_HYPHEN && seg.content_end > seg.start {
            let last = if n_content > 0 {
                self.pieces[(pieces + n_content) as usize - 1].span
            } else {
                span
            };
            self.cache
                .hyphen_width(self.book, self.spans[last as usize].style)
        } else {
            0.0
        };
        let breakable = self.want_units && content.n_units > 1;
        if !breakable {
            self.units.truncate(units as usize);
        }
        let mut flags = 0;
        if content.has_tab {
            flags |= F_DYN_W;
        }
        if hang.has_tab {
            flags |= F_DYN_H;
        }
        if breakable {
            flags |= F_BREAKABLE;
        }
        (
            Hot {
                width: content.width,
                hang: hang.width,
                brk: seg.brk,
                flags,
            },
            Cold {
                start: seg.start,
                content_end: seg.content_end,
                ws_end: seg.ws_end,
                pieces,
                n_content,
                n_hang,
                span,
                hang_span,
                units: if breakable { units } else { 0 },
                n_units: if breakable { content.n_units } else { 0 },
                hyphen,
                kind,
            },
        )
    }

    /// Drops the pieces pushed since `from` when they are a single non-tab piece (implied by
    /// the segment instead). Returns (stored count, implicit span, implicit kind).
    fn implicit(&mut self, from: u32) -> (u32, u32, u8) {
        let n = self.pieces.len() as u32 - from;
        match self.pieces.last() {
            Some(p) if n == 1 && p.kind != P_TAB => {
                let p = self.pieces.pop().unwrap();
                (0, p.span, p.kind)
            }
            _ => (n, 0, P_TEXT),
        }
    }

    /// Splits `[a, b)` at span boundaries (and tabs), measuring each piece.
    fn region(&mut self, a: u32, b: u32, content: bool) -> Region {
        let text = self.text;
        let bytes = text.as_bytes();
        let (a, b) = (a as usize, b as usize);
        let want_units = content && self.want_units;
        let mut r = Region {
            width: 0.0,
            has_tab: false,
            n_units: 0,
        };
        let mut p = a;
        while p < b {
            while self.spans[self.span].range.end <= p {
                self.span += 1;
            }
            let span = &self.spans[self.span];
            let run_end = span.range.end.min(b);
            if self.tabs && bytes[p] == b'\t' {
                let stop = self.tab_size as f32 * self.cache.space_width(self.book, span.style);
                if want_units {
                    self.units.push(stop);
                }
                self.pieces.push(Piece {
                    start: p as u32,
                    end: p as u32 + 1,
                    span: self.span as u32,
                    width: stop,
                    unit0: r.n_units,
                    n_units: want_units as u32,
                    kind: P_TAB,
                });
                r.n_units += want_units as u32;
                r.has_tab = true;
                p += 1;
                continue;
            }
            let q = if self.tabs {
                bytes[p..run_end]
                    .iter()
                    .position(|&c| c == b'\t')
                    .map_or(run_end, |i| p + i)
            } else {
                run_end
            };
            let piece = &text[p..q];
            let atomic = span.atomic;
            let pad_start = if p == span.range.start {
                span.pad_start
            } else {
                0.0
            };
            let pad_end = if q == span.range.end { span.pad_end } else { 0.0 };
            let (base, n_units) = if !content && q - p == 1 && bytes[p] == b' ' {
                (self.cache.space_width(self.book, span.style), 0)
            } else {
                let units = want_units && !atomic;
                let idx = self.cache.lookup(self.book, span.style, piece, units);
                let n = if units {
                    self.push_units(idx, pad_start, pad_end)
                } else {
                    0
                };
                (self.cache.width(idx), n)
            };
            let width = base + pad_start + pad_end;
            let n_units = if want_units && atomic {
                self.units.push(width);
                1
            } else {
                n_units
            };
            self.pieces.push(Piece {
                start: p as u32,
                end: q as u32,
                span: self.span as u32,
                width,
                unit0: r.n_units,
                n_units,
                kind: if atomic { P_ATOMIC } else { P_TEXT },
            });
            r.width += width;
            r.n_units += n_units;
            p = q;
        }
        r
    }

    /// Appends the cached per-grapheme advances of entry `idx`, with the span padding folded
    /// into the first and last unit. Returns the unit count.
    fn push_units(&mut self, idx: u32, pad_start: f32, pad_end: f32) -> u32 {
        let base = self.units.len();
        match self.cache.units(idx) {
            UnitsRef::Single(w) => self.units.push(w),
            UnitsRef::Many(us) => self.units.extend_from_slice(us),
        }
        self.units[base] += pad_start;
        if let Some(last) = self.units.last_mut() {
            *last += pad_end;
        }
        (self.units.len() - base) as u32
    }
}
