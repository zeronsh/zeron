//! Line layout over a [`Prepared`]: pure arithmetic on cached widths. Greedy browser-style
//! breaking — fill the line, break at the last opportunity that fits, let trailing whitespace
//! hang, split an overlong run between graphemes only when it can't fit on a line by itself.

use std::ops::Range;

use unicode_segmentation::UnicodeSegmentation;

use crate::analysis::{BRK_ALLOWED, BRK_HANG_ONLY, BRK_MANDATORY, BRK_SOFT_HYPHEN};
use crate::prepare::{
    F_BREAKABLE, F_DYN_H, F_DYN_W, F_LEAD, Hot, P_TAB, Piece, Prepared, utf16_len,
};

/// Slack allowed when testing whether content fits: CoreText lets a line exceed its width by
/// exactly this much (measured with CTFramesetter), which also absorbs float noise.
pub const LINE_FIT_EPSILON: f32 = 0.0002;

const NONE: u32 = u32::MAX;

/// A position between lines: segment index plus grapheme-cluster index within the segment's
/// content (0 at segment boundaries). Obtained from [`LineRange`]; resumable via
/// [`Prepared::layout_next_line`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Cursor {
    /// Segment index.
    pub segment: usize,
    /// Grapheme cluster (or atomic unit / tab) index within the segment's content.
    pub grapheme: usize,
}

impl Cursor {
    /// The start of the paragraph.
    pub const START: Cursor = Cursor {
        segment: 0,
        grapheme: 0,
    };
}

/// One laid-out line without materialized fragments.
#[derive(Clone, Debug, PartialEq)]
pub struct LineRange {
    /// Where the line starts.
    pub start: Cursor,
    /// Where the next line starts (past any hanging whitespace and forced break).
    pub end: Cursor,
    /// Byte range of the line's visible content in [`Prepared::text`], excluding hanging
    /// trailing whitespace and the forced-break character.
    pub range: Range<usize>,
    /// Advance of the visible content, including the hyphen when `hyphenated`.
    pub width: f32,
    /// The line ends at a soft hyphen that must be drawn as `-`.
    pub hyphenated: bool,
}

/// Line count and widest line at a given width.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LineStats {
    /// Number of lines.
    pub line_count: usize,
    /// Widest line's `width`.
    pub max_line_width: f32,
}

/// A laid-out line with drawable fragments.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    /// Byte range of the visible content (hanging whitespace excluded).
    pub range: Range<usize>,
    /// Advance of the visible content, including the hyphen when `hyphenated`.
    pub width: f32,
    /// Draw a `-` (U+002D, in the last fragment's style) at `width - hyphen` — i.e. right after
    /// the last fragment.
    pub hyphenated: bool,
    /// Maximal same-span runs in logical order; tabs are fragments of their own.
    pub fragments: Vec<Fragment>,
}

/// A run of one span within a line.
#[derive(Clone, Debug, PartialEq)]
pub struct Fragment {
    /// Byte range in [`Prepared::text`].
    pub range: Range<usize>,
    /// UTF-16 code-unit range in [`Prepared::text`] (for NSString / Java strings).
    pub utf16: Range<usize>,
    /// Index into [`Prepared::spans`].
    pub span: usize,
    /// Start offset from the line's origin: the exact cumulative advance of everything before it.
    pub x: f32,
    /// Advance, including the span's `pad_start`/`pad_end` when this fragment holds the span's
    /// first/last grapheme.
    pub width: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Pos {
    seg: u32,
    unit: u32,
}

impl Pos {
    const START: Pos = Pos { seg: 0, unit: 0 };
}

impl From<Pos> for Cursor {
    fn from(p: Pos) -> Self {
        Cursor {
            segment: p.seg as usize,
            grapheme: p.unit as usize,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Step {
    /// Where the next line starts.
    next: Pos,
    width: f32,
    hyphenated: bool,
    /// Last segment wholly on the line, or `NONE` when the line ends inside `next.seg`.
    end_seg: u32,
}

#[inline]
fn fit_limit(max_width: f32) -> f64 {
    if max_width.is_nan() {
        f64::INFINITY
    } else {
        max_width.max(0.0) as f64 + LINE_FIT_EPSILON as f64
    }
}

/// Advance of a tab at line position `x`: to the next multiple of `stop`, skipping a stop closer
/// than half a space (Blink's rule; `stop` is `tab_size` spaces).
#[inline]
pub(crate) fn tab_advance(stop: f32, x: f32, tab_size: u8) -> f32 {
    if stop <= 0.0 {
        return 0.0;
    }
    let mut d = stop - x.rem_euclid(stop);
    if d < stop / (2.0 * tab_size.max(1) as f32) {
        d += stop;
    }
    d
}

impl Prepared {
    #[inline]
    fn content_adv(&self, i: usize, h: Hot, x0: f32) -> f32 {
        if h.flags & F_DYN_W == 0 {
            h.width
        } else {
            self.pieces_adv(self.content_run(i).as_slice(), x0)
        }
    }

    #[inline]
    fn hang_adv(&self, i: usize, x0: f32) -> f32 {
        let h = self.hot[i];
        if h.flags & F_DYN_H == 0 {
            h.hang
        } else {
            self.pieces_adv(self.hang_run(i).as_slice(), x0)
        }
    }

    fn pieces_adv(&self, pieces: &[Piece], x0: f32) -> f32 {
        let mut x = x0;
        for p in pieces {
            x += if p.kind == P_TAB {
                tab_advance(p.width, x, self.tab_size)
            } else {
                p.width
            };
        }
        x - x0
    }

    /// Places the units of breakable segment `i` from unit `from` onto a line whose content so
    /// far ends at `x0`, while they fit within `fit`. With `force_first` the first unit is placed
    /// even if it doesn't fit (a line must make progress). Returns the new line end and, when
    /// the segment doesn't fit entirely, the unit to resume from.
    fn fit_units(&self, i: usize, from: u32, x0: f64, fit: f64, force_first: bool) -> (f64, Option<u32>) {
        let c = &self.cold[i];
        let mut x = x0;
        let mut placed = !force_first;
        if self.hot[i].flags & F_DYN_W == 0 {
            let us = &self.units[c.units as usize..(c.units + c.n_units) as usize];
            for (k, &u) in us.iter().enumerate().skip(from as usize) {
                let nx = x + u as f64;
                // Zero-advance units (ligature tails, invisible graphemes) stay with the unit
                // before them.
                if placed && nx > fit && u != 0.0 {
                    return (x, Some(k as u32));
                }
                x = nx;
                placed = true;
            }
        } else {
            let run = self.content_run(i);
            for p in run.as_slice() {
                if p.unit0 + p.n_units <= from {
                    continue;
                }
                for k in from.max(p.unit0)..p.unit0 + p.n_units {
                    let a = if p.kind == P_TAB {
                        tab_advance(p.width, x as f32, self.tab_size) as f64
                    } else {
                        self.units[(c.units + k) as usize] as f64
                    };
                    if placed && x + a > fit && a != 0.0 {
                        return (x, Some(k));
                    }
                    x += a;
                    placed = true;
                }
            }
        }
        (x, None)
    }

    /// Lays out one line starting at `start`. `None` once the text is exhausted.
    ///
    /// Mirrors CoreText's greedy fit: advances accumulate in `f64`; a line fits when its visible
    /// advance is within `fit` (`max_width + LINE_FIT_EPSILON`); whitespace hangs, and when the
    /// line overflows *inside* whitespace it ends right after that whitespace even where
    /// UAX #14 has no opportunity (`BRK_HANG_ONLY`); otherwise it ends at the last opportunity
    /// that fits, or — with none on the line — at the overflowing grapheme.
    fn step(&self, start: Pos, fit: f64) -> Option<Step> {
        let hot = self.hot.as_slice();
        let n = hot.len();
        let mut i = start.seg as usize;
        if i >= n {
            return None;
        }
        let wrap = self.wrap;
        let mut unit = start.unit;
        let mut x = 0.0f64;
        let mut last = usize::MAX;
        // Latest break opportunity on this line that fits: (segment, line width if broken there).
        let mut brk = usize::MAX;
        let mut brk_w = 0.0f64;
        let mut brk_hy = false;
        // A line starting where glyphs were substituted across the boundary (`a-|>b`) is shaped
        // anew by CoreText: its last glyph gets no kerning with the next line.
        let reshaped = unit == 0 && hot[i].flags & F_LEAD != 0;
        let tail = |k: usize| {
            if reshaped {
                self.cold[k].tail as f64
            } else {
                0.0
            }
        };
        let end_after = |seg: usize, width: f64, hyphenated: bool| Step {
            next: Pos {
                seg: seg as u32 + 1,
                unit: 0,
            },
            width: width as f32,
            hyphenated,
            end_seg: seg as u32,
        };
        let end_inside = |seg: usize, unit: u32, width: f64| Step {
            next: Pos {
                seg: seg as u32,
                unit,
            },
            width: width as f32,
            hyphenated: false,
            end_seg: NONE,
        };
        loop {
            let h = hot[i];
            if last == usize::MAX {
                let split = unit > 0
                    || (wrap
                        && h.flags & F_BREAKABLE != 0
                        && self.content_adv(i, h, 0.0) as f64 - tail(i) > fit);
                if split {
                    let (nx, rest) = self.fit_units(i, unit, 0.0, fit, true);
                    if let Some(u) = rest {
                        return Some(end_inside(i, u, nx));
                    }
                    x = nx;
                    unit = 0;
                } else {
                    x = self.content_adv(i, h, 0.0) as f64;
                }
            } else {
                let mut base = x + self.hang_adv(last, x as f32) as f64;
                if h.flags & F_LEAD != 0 {
                    base += self.cold[i].lead as f64;
                }
                let cand = base + self.content_adv(i, h, base as f32) as f64;
                if wrap && cand - tail(i) > fit {
                    if brk != usize::MAX {
                        return Some(end_after(brk, brk_w, brk_hy));
                    }
                    // No opportunity on the line fits.
                    if hot[last].brk == BRK_SOFT_HYPHEN {
                        // Break at the soft hyphen anyway; under overflow-wrap: anywhere, when
                        // only the hyphen overflows, break there as a plain grapheme boundary.
                        let x = x - tail(last);
                        let hy = !(self.anywhere && x <= fit);
                        let w = if hy { x + self.cold[last].hyphen as f64 } else { x };
                        return Some(end_after(last, w, hy));
                    }
                    if self.anywhere && h.flags & F_BREAKABLE != 0 && x <= fit {
                        // Everything so far fits but has no opportunity: break at the
                        // overflowing grapheme, like CoreText's character wrapping.
                        if let (nx, Some(u)) = self.fit_units(i, 0, base, fit, false)
                            && u > 0
                        {
                            return Some(end_inside(i, u, nx));
                        }
                    }
                    return Some(end_after(last, x - tail(last), false));
                }
                x = cand;
            }
            last = i;
            let xe = x - tail(i);
            match h.brk {
                BRK_MANDATORY => return Some(end_after(i, xe, false)),
                BRK_ALLOWED => {
                    if xe <= fit {
                        brk = i;
                        brk_w = xe;
                        brk_hy = false;
                    }
                }
                BRK_HANG_ONLY => {
                    if wrap && xe <= fit && x + self.hang_adv(i, x as f32) as f64 > fit {
                        return Some(end_after(i, xe, false));
                    }
                }
                _ => {
                    let w = xe + self.cold[i].hyphen as f64;
                    if w <= fit {
                        brk = i;
                        brk_w = w;
                        brk_hy = true;
                    }
                }
            }
            i += 1;
            if i >= n {
                // Unreachable in practice: the last segment always ends in a mandatory break.
                return Some(Step {
                    next: Pos {
                        seg: n as u32,
                        unit: 0,
                    },
                    width: x as f32,
                    hyphenated: false,
                    end_seg: last as u32,
                });
            }
        }
    }

    /// Number of lines at `max_width`. Allocation-free; the virtualization hot path.
    pub fn line_count(&self, max_width: f32) -> usize {
        let fit = fit_limit(max_width);
        let mut pos = Pos::START;
        let mut n = 0;
        while let Some(s) = self.step(pos, fit) {
            n += 1;
            pos = s.next;
        }
        n
    }

    /// Line count and widest line at `max_width`. Allocation-free.
    pub fn stats(&self, max_width: f32) -> LineStats {
        let fit = fit_limit(max_width);
        let mut pos = Pos::START;
        let mut stats = LineStats::default();
        while let Some(s) = self.step(pos, fit) {
            stats.line_count += 1;
            stats.max_line_width = stats.max_line_width.max(s.width);
            pos = s.next;
        }
        stats
    }

    /// Visits every line at `max_width` without materializing fragments.
    pub fn walk_lines(&self, max_width: f32, mut f: impl FnMut(&LineRange)) {
        let fit = fit_limit(max_width);
        let mut loc = Locator::new();
        let mut pos = Pos::START;
        while let Some(s) = self.step(pos, fit) {
            f(&LineRange {
                start: pos.into(),
                end: s.next.into(),
                range: self.line_bytes(pos, &s, &mut loc),
                width: s.width,
                hyphenated: s.hyphenated,
            });
            pos = s.next;
        }
    }

    /// Lays out the single line starting at `start` (a previous line's `end`, or
    /// [`Cursor::START`]) at `max_width` — for flowing text through lines of varying width.
    /// `None` at the end of the text.
    pub fn layout_next_line(&self, start: Cursor, max_width: f32) -> Option<LineRange> {
        let n = self.hot.len();
        let mut pos = Pos {
            seg: start.segment.min(n) as u32,
            unit: start.grapheme.min(u32::MAX as usize) as u32,
        };
        if (pos.seg as usize) < n && pos.unit > 0 {
            let c = &self.cold[pos.seg as usize];
            if self.hot[pos.seg as usize].flags & F_BREAKABLE == 0 || pos.unit >= c.n_units {
                pos = Pos {
                    seg: pos.seg + 1,
                    unit: 0,
                };
            }
        }
        let s = self.step(pos, fit_limit(max_width))?;
        Some(LineRange {
            start: pos.into(),
            end: s.next.into(),
            range: self.line_bytes(pos, &s, &mut Locator::new()),
            width: s.width,
            hyphenated: s.hyphenated,
        })
    }

    /// Every line at `max_width` with its fragments.
    pub fn lines(&self, max_width: f32) -> Vec<Line> {
        let fit = fit_limit(max_width);
        let mut out = Vec::new();
        let mut loc = Locator::new();
        let mut u16c = Utf16Cursor::default();
        let mut pos = Pos::START;
        while let Some(s) = self.step(pos, fit) {
            let range = self.line_bytes(pos, &s, &mut loc);
            let fragments = self.fragments(pos, &s, &range, &mut u16c);
            out.push(Line {
                range,
                width: s.width,
                hyphenated: s.hyphenated,
                fragments,
            });
            pos = s.next;
        }
        out
    }

    /// Narrowest width that avoids overflow: the widest unbreakable unit (a grapheme or atomic
    /// span for breakable runs under `overflow-wrap: anywhere`, else a whole segment, plus the
    /// hyphen for soft-hyphen breaks). Tabs count as a full tab stop. Equals
    /// [`Prepared::max_content_width`] for `Pre`.
    pub fn min_content_width(&self) -> f32 {
        if !self.wrap {
            return self.max_content_width();
        }
        let mut m = 0.0f32;
        for (i, &h) in self.hot.iter().enumerate() {
            let c = &self.cold[i];
            let hyphen = if h.brk == BRK_SOFT_HYPHEN {
                c.hyphen
            } else {
                0.0
            };
            let w = if h.flags & F_BREAKABLE != 0 {
                let us = &self.units[c.units as usize..(c.units + c.n_units) as usize];
                // Tab units hold the full stop interval.
                let widest = us.iter().copied().fold(0.0, f32::max);
                widest.max(us.last().copied().unwrap_or(0.0) + hyphen)
            } else {
                self.content_adv(i, h, 0.0) + hyphen
            };
            m = m.max(w);
        }
        m
    }

    /// Width of the widest line when nothing wraps (forced breaks only).
    pub fn max_content_width(&self) -> f32 {
        self.stats(f32::INFINITY).max_line_width
    }

    fn line_bytes(&self, start: Pos, s: &Step, loc: &mut Locator) -> Range<usize> {
        let a = self.pos_byte(start, loc);
        let b = if s.end_seg != NONE {
            self.cold[s.end_seg as usize].content_end as usize
        } else {
            self.pos_byte(s.next, loc)
        };
        a..b
    }

    fn pos_byte(&self, p: Pos, loc: &mut Locator) -> usize {
        if (p.seg as usize) >= self.cold.len() {
            self.text.len()
        } else if p.unit == 0 {
            self.cold[p.seg as usize].start as usize
        } else {
            loc.locate(self, p.seg as usize, p.unit)
        }
    }

    /// Builds the fragments of the line `start..s`, whose visible bytes are `bytes`.
    fn fragments(
        &self,
        start: Pos,
        s: &Step,
        bytes: &Range<usize>,
        u16c: &mut Utf16Cursor,
    ) -> Vec<Fragment> {
        let mut fb = FragBuilder {
            p: self,
            u16c,
            out: Vec::new(),
            cur: None,
            x: 0.0,
        };
        let (last, split) = if s.end_seg != NONE {
            (s.end_seg as usize, None)
        } else {
            (s.next.seg as usize, Some(s.next.unit))
        };
        let reshaped = start.unit == 0 && self.hot[start.seg as usize].flags & F_LEAD != 0;
        for seg in start.seg as usize..=last {
            let c = &self.cold[seg];
            let from = if seg == start.seg as usize {
                start.unit
            } else {
                0
            };
            let to = if seg == last { split } else { None };
            // The reshaped line's last glyph has no kerning with the next line.
            let mut tail = if reshaped && seg == last && to.is_none() {
                c.tail
            } else {
                0.0
            };
            let run = self.content_run(seg);
            // A segment continuing the line carries the right half of the pair context.
            let mut lead = if seg != start.seg as usize && self.hot[seg].flags & F_LEAD != 0 {
                c.lead
            } else {
                0.0
            };
            if from == 0 && to.is_none() {
                let pieces = run.as_slice();
                for (k, piece) in pieces.iter().enumerate() {
                    let mut w = piece.width + std::mem::take(&mut lead);
                    if k + 1 == pieces.len() {
                        w -= std::mem::take(&mut tail);
                    }
                    fb.piece(piece.span, piece.start as usize, piece.end as usize, w, piece.kind);
                }
            } else {
                // Partial segment (split by overflow-wrap): clip pieces to the line's bytes and
                // sum the units that fall inside.
                let lo = if from == 0 { c.start as usize } else { bytes.start };
                let hi = if to.is_some() {
                    bytes.end
                } else {
                    c.content_end as usize
                };
                let to_u = to.unwrap_or(c.n_units);
                let pieces = run.as_slice();
                let first = pieces.partition_point(|p| (p.end as usize) <= lo);
                for piece in &pieces[first..] {
                    if piece.start as usize >= hi {
                        break;
                    }
                    let u0 = piece.unit0.max(from);
                    let u1 = (piece.unit0 + piece.n_units).min(to_u);
                    let a = (piece.start as usize).max(lo);
                    let b = (piece.end as usize).min(hi);
                    let width = if u0 == piece.unit0 && u1 == piece.unit0 + piece.n_units {
                        piece.width
                    } else {
                        self.units[(c.units + u0) as usize..(c.units + u1) as usize]
                            .iter()
                            .sum()
                    };
                    fb.piece(piece.span, a, b, width + std::mem::take(&mut lead), piece.kind);
                }
            }
            if seg != last {
                for piece in self.hang_run(seg).as_slice() {
                    fb.piece(piece.span, piece.start as usize, piece.end as usize, piece.width, piece.kind);
                }
            }
        }
        fb.finish()
    }
}

/// Maps (segment, unit) to byte offsets, memoizing its position so a monotone walk over a split
/// segment stays linear.
struct Locator {
    seg: usize,
    piece: usize,
    unit: u32,
    byte: usize,
}

impl Locator {
    fn new() -> Self {
        Self {
            seg: usize::MAX,
            piece: 0,
            unit: 0,
            byte: 0,
        }
    }

    fn locate(&mut self, p: &Prepared, seg: usize, unit: u32) -> usize {
        let run = p.content_run(seg);
        let pieces = run.as_slice();
        if self.seg != seg || self.unit > unit {
            self.seg = seg;
            self.piece = 0;
            self.unit = 0;
            self.byte = p.cold[seg].start as usize;
        }
        while self.piece + 1 < pieces.len() {
            let pc = &pieces[self.piece];
            if unit < pc.unit0 + pc.n_units {
                break;
            }
            self.piece += 1;
            self.unit = pieces[self.piece].unit0;
            self.byte = pieces[self.piece].start as usize;
        }
        let pc = &pieces[self.piece];
        if self.unit < unit {
            let k = (unit - self.unit) as usize;
            let s = &p.text[self.byte..pc.end as usize];
            let adv = if p.ascii {
                k.min(s.len())
            } else {
                s.grapheme_indices(true).nth(k).map_or(s.len(), |(o, _)| o)
            };
            self.byte += adv;
            self.unit = unit;
        }
        self.byte
    }
}

/// Byte → UTF-16 offset conversion for a monotone sequence of offsets.
#[derive(Default)]
struct Utf16Cursor {
    byte: usize,
    unit: usize,
}

impl Utf16Cursor {
    fn at(&mut self, p: &Prepared, byte: usize) -> usize {
        if p.ascii {
            return byte;
        }
        if byte < self.byte {
            self.byte = 0;
            self.unit = 0;
        }
        self.unit += utf16_len(&p.text.as_bytes()[self.byte..byte]);
        self.byte = byte;
        self.unit
    }
}

struct FragBuilder<'a> {
    p: &'a Prepared,
    u16c: &'a mut Utf16Cursor,
    out: Vec<Fragment>,
    /// Open fragment and whether it is a tab.
    cur: Option<(Fragment, bool)>,
    x: f32,
}

impl FragBuilder<'_> {
    fn piece(&mut self, span: u32, a: usize, b: usize, width: f32, kind: u8) {
        let tab = kind == P_TAB;
        let width = if tab {
            tab_advance(width, self.x, self.p.tab_size)
        } else {
            width
        };
        if let Some((cur, cur_tab)) = self.cur.as_mut()
            && !tab
            && !*cur_tab
            && cur.span == span as usize
            && cur.range.end == a
        {
            cur.range.end = b;
            cur.width += width;
        } else {
            self.flush();
            self.cur = Some((
                Fragment {
                    range: a..b,
                    utf16: 0..0,
                    span: span as usize,
                    x: self.x,
                    width,
                },
                tab,
            ));
        }
        self.x += width;
    }

    fn flush(&mut self) {
        if let Some((mut f, _)) = self.cur.take() {
            let a = self.u16c.at(self.p, f.range.start);
            let b = self.u16c.at(self.p, f.range.end);
            f.utf16 = a..b;
            self.out.push(f);
        }
    }

    fn finish(mut self) -> Vec<Fragment> {
        self.flush();
        self.out
    }
}
