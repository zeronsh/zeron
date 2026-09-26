//! `(style, text) → width` cache plus the measurement backends it fronts: rustybuzz shaping for
//! text the face covers, the host [`crate::FallbackMeasurer`] for everything else.

use std::collections::HashMap;
use std::hash::Hasher;

use hashbrown::HashTable;
use rustc_hash::{FxBuildHasher, FxHasher};
use rustybuzz::{Direction, Script, ShapePlan, UnicodeBuffer};
use unicode_segmentation::UnicodeSegmentation;

use crate::analysis::RawSeg;
use crate::chars::{forces_emoji, is_default_ignorable};
use crate::font::{FaceData, FontBook, StyleData, StyleId};
use crate::prepare::Piece;

const UNKNOWN: u32 = u32::MAX;

/// Hit/miss counters and size of a [`WidthCache`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CacheStats {
    /// Lookups answered from the cache.
    pub hits: u64,
    /// Lookups that had to measure.
    pub misses: u64,
    /// Calls made to the host fallback measurer.
    pub fallback_calls: u64,
    /// Distinct `(style, text)` entries held.
    pub entries: usize,
    /// Approximate heap bytes held by entries, key text and grapheme advances.
    pub bytes: usize,
}

#[derive(Clone, Copy)]
struct Entry {
    hash: u64,
    off: u32,
    len: u32,
    style: u16,
    fallback: bool,
    /// A fallback *run* (per-grapheme in-context advances from
    /// [`crate::FallbackMeasurer::measure_run`]) rather than a piece.
    run: bool,
    /// Final width in points, letter spacing included.
    width: f32,
    /// Per-grapheme advances live at `units[units_off..units_off + units_len]` when
    /// `units_len > 1`; `1` means "single grapheme, advance == width"; `UNKNOWN` means not yet
    /// computed.
    units_off: u32,
    units_len: u32,
}

struct Plan {
    face: u16,
    ligatures: bool,
    direction: Direction,
    script: Option<Script>,
    plan: ShapePlan,
}

/// Growable buffers `prepare` reuses across calls on the same thread.
#[derive(Default)]
pub(crate) struct Scratch {
    pub breaks: Vec<(u32, bool)>,
    pub scripts: Vec<u32>,
    pub segs: Vec<RawSeg>,
    pub pieces: Vec<Piece>,
    pub units: Vec<f32>,
}

/// Width cache keyed by `(StyleId, &str)`. Owned by one layout thread and passed to
/// [`crate::prepare`]; hits hash the key and compare bytes, with no allocation.
///
/// Entries are only valid for the [`FontBook`] they were measured with; handing the cache a
/// different book (or one whose fallback changed) clears it automatically. The cache grows
/// without bound — call [`WidthCache::clear`] to drop it.
pub struct WidthCache {
    book: u64,
    table: HashTable<u32>,
    entries: Vec<Entry>,
    arena: String,
    units: Vec<f32>,
    space: Vec<f32>,
    hyphen: Vec<f32>,
    plans: Vec<Plan>,
    /// Pair context `(Δa, Δb)` between printable-ASCII graphemes, per style: `[(a - 0x20) * 96 + (b - 0x20)]`,
    /// NaN until computed.
    kern_ascii: Vec<Option<Box<[PairContext]>>>,
    /// Pair context between other single-char graphemes.
    kern_chars: HashMap<(u16, char, char), PairContext, FxBuildHasher>,
    /// The fallback measurer declined [`crate::FallbackMeasurer::measure_run`]; don't ask again.
    pub(crate) runs_unsupported: bool,
    run_scratch: Vec<f32>,
    buffer: Option<UnicodeBuffer>,
    clusters: Vec<(u32, i32)>,
    starts: Vec<u32>,
    stats: CacheStats,
    pub(crate) scratch: Scratch,
}

impl Default for WidthCache {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for WidthCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WidthCache")
            .field("stats", &self.stats())
            .finish()
    }
}

#[inline]
fn key_hash(style: StyleId, text: &str) -> u64 {
    key_hash_kind(style, text, false)
}

#[inline]
fn key_hash_kind(style: StyleId, text: &str, run: bool) -> u64 {
    let mut h = FxHasher::default();
    h.write_u16(style.0 | ((run as u16) << 15));
    h.write(text.as_bytes());
    h.write_usize(text.len());
    h.finish()
}

#[inline]
fn sanitize(w: f32) -> f32 {
    if w.is_finite() && w > 0.0 { w } else { 0.0 }
}

/// Whether `text` needs the host measurer: some visible char has no glyph in the face, or the
/// text requests emoji presentation.
pub(crate) fn needs_fallback(face: &FaceData, text: &str) -> bool {
    text.chars().any(|c| {
        if forces_emoji(c) {
            return true;
        }
        if (c as u32) < 0x20 || c == '\u{7F}' || is_default_ignorable(c) {
            return false;
        }
        !face.covers(c)
    })
}

pub(crate) fn grapheme_count(text: &str) -> usize {
    if text.is_ascii() && !text.contains('\r') {
        text.len()
    } else {
        text.graphemes(true).count()
    }
}

impl WidthCache {
    /// An empty cache.
    pub fn new() -> Self {
        Self {
            book: 0,
            table: HashTable::new(),
            entries: Vec::new(),
            arena: String::new(),
            units: Vec::new(),
            space: Vec::new(),
            hyphen: Vec::new(),
            plans: Vec::new(),
            kern_ascii: Vec::new(),
            kern_chars: HashMap::default(),
            runs_unsupported: false,
            run_scratch: Vec::new(),
            buffer: None,
            clusters: Vec::new(),
            starts: Vec::new(),
            stats: CacheStats::default(),
            scratch: Scratch::default(),
        }
    }

    /// Drops every entry (and shaping plans). Counters are reset too.
    pub fn clear(&mut self) {
        self.table.clear();
        self.entries.clear();
        self.arena.clear();
        self.units.clear();
        self.space.clear();
        self.hyphen.clear();
        self.plans.clear();
        self.kern_ascii.clear();
        self.kern_chars.clear();
        self.runs_unsupported = false;
        self.stats = CacheStats::default();
    }

    /// Hit/miss counters and current size.
    pub fn stats(&self) -> CacheStats {
        CacheStats {
            entries: self.entries.len(),
            bytes: self.entries.capacity() * std::mem::size_of::<Entry>()
                + self.arena.capacity()
                + self.units.capacity() * 4
                + self.table.capacity() * 4,
            ..self.stats
        }
    }

    /// Resets hit/miss/fallback counters, keeping entries.
    pub fn reset_stats(&mut self) {
        self.stats = CacheStats::default();
    }

    /// Binds the cache to `book`, clearing it when it was filled against another book.
    pub(crate) fn bind(&mut self, book: &FontBook) {
        if self.book != book.id() {
            self.clear();
            self.book = book.id();
        }
    }

    /// Looks up (or measures) `text` in `style`, returning the entry index. With `want_units`
    /// the entry's per-grapheme advances are guaranteed to be computed.
    pub(crate) fn lookup(
        &mut self,
        book: &FontBook,
        style: StyleId,
        text: &str,
        want_units: bool,
    ) -> u32 {
        let hash = key_hash(style, text);
        let entries = &self.entries;
        let arena = &self.arena;
        let found = self.table.find(hash, |&i| {
            let e = &entries[i as usize];
            e.hash == hash
                && e.style == style.0
                && !e.run
                && &arena[e.off as usize..(e.off + e.len) as usize] == text
        });
        if let Some(&idx) = found {
            self.stats.hits += 1;
            if want_units && self.entries[idx as usize].units_len == UNKNOWN {
                self.compute_units(book, idx, text);
            }
            return idx;
        }
        self.stats.misses += 1;
        self.insert(book, style, text, hash, want_units)
    }

    #[inline]
    pub(crate) fn width(&self, idx: u32) -> f32 {
        self.entries[idx as usize].width
    }

    /// Per-grapheme advances of a looked-up entry (computed via `want_units`). A single-grapheme
    /// entry yields `[width]`.
    pub(crate) fn units(&self, idx: u32) -> UnitsRef<'_> {
        let e = &self.entries[idx as usize];
        debug_assert!(e.units_len != UNKNOWN);
        if e.units_len <= 1 {
            UnitsRef::Single(e.width)
        } else {
            UnitsRef::Many(&self.units[e.units_off as usize..(e.units_off + e.units_len) as usize])
        }
    }

    /// Width of one U+0020 in `style` (memoized; hanging whitespace hits this constantly).
    pub(crate) fn space_width(&mut self, book: &FontBook, style: StyleId) -> f32 {
        let i = style.0 as usize;
        if let Some(&w) = self.space.get(i)
            && !w.is_nan()
        {
            return w;
        }
        let idx = self.lookup(book, style, " ", false);
        let w = self.width(idx);
        if self.space.len() <= i {
            self.space.resize(i + 1, f32::NAN);
        }
        self.space[i] = w;
        w
    }

    /// Width of the hyphen drawn when a soft hyphen is chosen as the break (U+002D).
    pub(crate) fn hyphen_width(&mut self, book: &FontBook, style: StyleId) -> f32 {
        let i = style.0 as usize;
        if let Some(&w) = self.hyphen.get(i)
            && !w.is_nan()
        {
            return w;
        }
        let idx = self.lookup(book, style, "-", false);
        let w = self.width(idx);
        if self.hyphen.len() <= i {
            self.hyphen.resize(i + 1, f32::NAN);
        }
        self.hyphen[i] = w;
        w
    }

    /// In-context advances of each grapheme of `run` (a maximal run of text the face can't
    /// draw), without letter spacing, from the host's
    /// [`crate::FallbackMeasurer::measure_run`]. `None` when there is no fallback measurer or it
    /// doesn't measure runs (remembered), or its answer was malformed.
    pub(crate) fn run_advances(&mut self, book: &FontBook, style: StyleId, run: &str) -> Option<&[f32]> {
        if self.runs_unsupported {
            return None;
        }
        let m = book.fallback()?;
        let hash = key_hash_kind(style, run, true);
        let entries = &self.entries;
        let arena = &self.arena;
        let found = self.table.find(hash, |&i| {
            let e = &entries[i as usize];
            e.hash == hash
                && e.style == style.0
                && e.run
                && &arena[e.off as usize..(e.off + e.len) as usize] == run
        });
        let idx = if let Some(&idx) = found {
            self.stats.hits += 1;
            idx
        } else {
            self.stats.misses += 1;
            self.stats.fallback_calls += 1;
            let mut per_char = std::mem::take(&mut self.run_scratch);
            per_char.clear();
            let ok = m.measure_run(style, run, &mut per_char);
            if !ok {
                self.runs_unsupported = true;
                self.run_scratch = per_char;
                return None;
            }
            let units_off = self.units.len() as u32;
            let valid = per_char.len() == run.chars().count();
            if valid {
                let mut k = 0;
                for g in run.graphemes(true) {
                    let mut w = 0.0;
                    for _ in g.chars() {
                        w += sanitize(per_char[k]);
                        k += 1;
                    }
                    self.units.push(w);
                }
            }
            self.run_scratch = per_char;
            let units_len = if valid {
                self.units.len() as u32 - units_off
            } else {
                UNKNOWN
            };
            let width = self.units[units_off as usize..].iter().sum();
            let off = self.arena.len() as u32;
            self.arena.push_str(run);
            let idx = self.entries.len() as u32;
            self.entries.push(Entry {
                hash,
                off,
                len: run.len() as u32,
                style: style.0,
                fallback: true,
                run: true,
                width,
                units_off,
                units_len,
            });
            let entries = &self.entries;
            self.table
                .insert_unique(hash, idx, |&i| entries[i as usize].hash);
            idx
        };
        let e = &self.entries[idx as usize];
        (e.units_len != UNKNOWN)
            .then(|| &self.units[e.units_off as usize..(e.units_off + e.units_len) as usize])
    }

    /// Whether `grapheme` in `style` is shaped with the style's face (as opposed to going to the
    /// host fallback measurer, i.e. a different font).
    pub(crate) fn is_shaped(&self, book: &FontBook, style: StyleId, grapheme: &str) -> bool {
        if book.fallback().is_none() {
            return true;
        }
        let face = book.face_data(book.style_data(style).style.face);
        !needs_fallback(face, grapheme)
    }

    /// Advance changes from shaping two adjacent graphemes together rather than apart, split by
    /// side: `(Δa, Δb)` in points, where `Δa = adv_in_pair(a) − shape(a)` and likewise for `b`
    /// (glyphs are attributed to a side by their cluster). This is what a paragraph-level shaper
    /// applies across a segment boundary: pair kerning lands on `a` (GPOS adjusts the first
    /// glyph's advance); a ligature or contextual form spanning both (Geist's `->` arrow) puts
    /// its whole advance on `a` and leaves `b` with `−shape(b)`. Both graphemes must be shaped
    /// by the style's face (see [`WidthCache::is_shaped`]).
    pub(crate) fn pair_context(
        &mut self,
        book: &FontBook,
        style: StyleId,
        a: &str,
        b: &str,
    ) -> PairContext {
        let ab = a.as_bytes();
        let bb = b.as_bytes();
        if ab.len() == 1
            && bb.len() == 1
            && (0x20..0x80).contains(&ab[0])
            && (0x20..0x80).contains(&bb[0])
        {
            let i = style.0 as usize;
            if self.kern_ascii.len() <= i {
                self.kern_ascii.resize_with(i + 1, || None);
            }
            let k = (ab[0] as usize - 0x20) * 96 + (bb[0] as usize - 0x20);
            let cached = self.kern_ascii[i]
                .as_ref()
                .map_or(PairContext::UNKNOWN, |t| t[k]);
            if !cached.left.is_nan() {
                return cached;
            }
            let v = self.compute_pair_context(book, style, a, b);
            self.kern_ascii[i]
                .get_or_insert_with(|| vec![PairContext::UNKNOWN; 96 * 96].into_boxed_slice())[k] =
                v;
            return v;
        }
        let mut ca = a.chars();
        let mut cb = b.chars();
        if let (Some(x), None, Some(y), None) = (ca.next(), ca.next(), cb.next(), cb.next()) {
            if let Some(&v) = self.kern_chars.get(&(style.0, x, y)) {
                return v;
            }
            let v = self.compute_pair_context(book, style, a, b);
            self.kern_chars.insert((style.0, x, y), v);
            return v;
        }
        self.compute_pair_context(book, style, a, b)
    }

    fn compute_pair_context(&mut self, book: &FontBook, style: StyleId, a: &str, b: &str) -> PairContext {
        let sd = book.style_data(style);
        let face = book.face_data(sd.style.face);
        let mut pair = String::with_capacity(a.len() + b.len());
        pair.push_str(a);
        pair.push_str(b);
        let (mut in_a, mut in_b) = (0i64, 0i64);
        let mut ids: Vec<u32> = Vec::new();
        let glyphs = self.shape_glyphs(sd, face, &pair);
        for (info, pos) in glyphs.glyph_infos().iter().zip(glyphs.glyph_positions()) {
            ids.push(info.glyph_id);
            if (info.cluster as usize) < a.len() {
                in_a += pos.x_advance as i64;
            } else {
                in_b += pos.x_advance as i64;
            }
        }
        self.buffer = Some(glyphs.clear());
        let mut apart: Vec<u32> = Vec::new();
        let mut units = [0i64; 2];
        for (k, t) in [a, b].into_iter().enumerate() {
            let glyphs = self.shape_glyphs(sd, face, t);
            apart.extend(glyphs.glyph_infos().iter().map(|g| g.glyph_id));
            units[k] = glyphs.glyph_positions().iter().map(|p| p.x_advance as i64).sum();
            self.buffer = Some(glyphs.clear());
        }
        PairContext {
            left: (in_a - units[0]) as f32 * sd.scale,
            right: (in_b - units[1]) as f32 * sd.scale,
            substituted: ids != apart,
        }
    }

    fn shape_glyphs(&mut self, sd: &StyleData, face: &FaceData, text: &str) -> rustybuzz::GlyphBuffer {
        let mut buf = self.buffer.take().unwrap_or_default();
        buf.push_str(text);
        buf.guess_segment_properties();
        let direction = buf.direction();
        let script = buf.script();
        let script = (script != rustybuzz::script::UNKNOWN).then_some(script);
        let plan = self.plan(sd, face, direction, script);
        rustybuzz::shape_with_plan(&face.hb, &self.plans[plan].plan, buf)
    }

    fn insert(
        &mut self,
        book: &FontBook,
        style: StyleId,
        text: &str,
        hash: u64,
        want_units: bool,
    ) -> u32 {
        let sd = book.style_data(style);
        let face = book.face_data(sd.style.face);
        let ls = sd.style.options.letter_spacing;
        let fallback = match book.fallback() {
            Some(m) if needs_fallback(face, text) => Some(m),
            _ => None,
        };
        let units_off = self.units.len() as u32;
        let mut units_len = UNKNOWN;
        let raw = match fallback {
            Some(m) => {
                self.stats.fallback_calls += 1;
                sanitize(m.measure(style, text))
            }
            None => {
                let (w, n) = self.shape(sd, face, text, want_units);
                if want_units {
                    units_len = n;
                }
                w
            }
        };
        let width = if ls != 0.0 {
            let n = if units_len != UNKNOWN {
                units_len as usize
            } else {
                grapheme_count(text)
            };
            raw + ls * n as f32
        } else {
            raw
        };
        if units_len != UNKNOWN && units_len <= 1 {
            self.units.truncate(units_off as usize);
        }
        let off = self.arena.len() as u32;
        self.arena.push_str(text);
        let idx = self.entries.len() as u32;
        self.entries.push(Entry {
            hash,
            off,
            len: text.len() as u32,
            style: style.0,
            fallback: fallback.is_some(),
            run: false,
            width,
            units_off,
            units_len,
        });
        let entries = &self.entries;
        self.table
            .insert_unique(hash, idx, |&i| entries[i as usize].hash);
        if want_units && units_len == UNKNOWN {
            self.compute_units(book, idx, text);
        }
        idx
    }

    fn compute_units(&mut self, book: &FontBook, idx: u32, text: &str) {
        let e = self.entries[idx as usize];
        let style = StyleId(e.style);
        let sd = book.style_data(style);
        let ls = sd.style.options.letter_spacing;
        let off = self.units.len() as u32;
        let n = if !e.fallback {
            let face = book.face_data(sd.style.face);
            self.shape(sd, face, text, true).1
        } else {
            // The host measured the piece as a whole; measure each grapheme through the same
            // cache (covered ones get shaped, the rest go to the host) and scale so the advances
            // sum exactly to the piece width.
            let bounds: Vec<(usize, usize)> = text
                .grapheme_indices(true)
                .map(|(i, g)| (i, i + g.len()))
                .collect();
            if bounds.len() > 1 {
                let mut raw = Vec::with_capacity(bounds.len());
                for &(a, b) in &bounds {
                    let gi = self.lookup(book, style, &text[a..b], false);
                    raw.push((self.width(gi) - ls).max(0.0));
                }
                let target = (e.width - ls * bounds.len() as f32).max(0.0);
                let sum: f32 = raw.iter().sum();
                let off = self.units.len();
                if sum > 0.0 {
                    let k = target / sum;
                    self.units.extend(raw.iter().map(|w| w * k + ls));
                } else {
                    let each = target / bounds.len() as f32;
                    self.units.extend(raw.iter().map(|_| each + ls));
                }
                debug_assert_eq!(self.units.len() - off, bounds.len());
            }
            bounds.len() as u32
        };
        let e = &mut self.entries[idx as usize];
        e.units_len = n;
        if n > 1 {
            e.units_off = off;
        } else {
            self.units.truncate(off as usize);
        }
    }

    /// Shapes `text` with rustybuzz. Returns the raw advance in points (no letter spacing) and,
    /// when `want_units`, pushes per-grapheme advances (letter spacing included) onto
    /// `self.units` and returns their count.
    fn shape(&mut self, sd: &StyleData, face: &FaceData, text: &str, want_units: bool) -> (f32, u32) {
        let mut buf = self.buffer.take().unwrap_or_default();
        buf.push_str(text);
        buf.guess_segment_properties();
        let direction = buf.direction();
        let script = buf.script();
        let script = (script != rustybuzz::script::UNKNOWN).then_some(script);
        let plan = self.plan(sd, face, direction, script);
        let glyphs = rustybuzz::shape_with_plan(&face.hb, &self.plans[plan].plan, buf);
        let positions = glyphs.glyph_positions();
        let total: i64 = positions.iter().map(|p| p.x_advance as i64).sum();
        let width = total as f32 * sd.scale;
        let mut n = 0;
        if want_units {
            n = self.cluster_units(
                text,
                glyphs.glyph_infos(),
                positions,
                sd.scale,
                sd.style.options.letter_spacing,
            );
        }
        self.buffer = Some(glyphs.clear());
        (width, n)
    }

    fn plan(
        &mut self,
        sd: &StyleData,
        face: &FaceData,
        direction: Direction,
        script: Option<Script>,
    ) -> usize {
        let face_id = sd.style.face.0;
        let ligatures = sd.style.options.ligatures;
        if let Some(i) = self.plans.iter().position(|p| {
            p.face == face_id
                && p.ligatures == ligatures
                && p.direction == direction
                && p.script == script
        }) {
            return i;
        }
        let plan = ShapePlan::new(&face.hb, direction, script, None, &sd.features);
        self.plans.push(Plan {
            face: face_id,
            ligatures,
            direction,
            script,
            plan,
        });
        self.plans.len() - 1
    }

    /// Distributes glyph advances onto extended grapheme clusters: a shaping cluster's advance
    /// goes to the first grapheme starting inside it (the others in a ligature get zero), and a
    /// cluster starting mid-grapheme folds into that grapheme. Sums exactly to the run.
    fn cluster_units(
        &mut self,
        text: &str,
        infos: &[rustybuzz::GlyphInfo],
        positions: &[rustybuzz::GlyphPosition],
        scale: f32,
        ls: f32,
    ) -> u32 {
        let clusters = &mut self.clusters;
        clusters.clear();
        for (info, pos) in infos.iter().zip(positions) {
            match clusters.last_mut() {
                Some(last) if last.0 == info.cluster => last.1 += pos.x_advance,
                _ => clusters.push((info.cluster, pos.x_advance)),
            }
        }
        if clusters.windows(2).any(|w| w[0].0 > w[1].0) {
            clusters.sort_by_key(|c| c.0);
            clusters.dedup_by(|b, a| {
                if a.0 == b.0 {
                    a.1 += b.1;
                    true
                } else {
                    false
                }
            });
        }

        let starts = &mut self.starts;
        starts.clear();
        if text.is_ascii() && !text.contains('\r') {
            starts.extend(0..text.len() as u32);
        } else {
            starts.extend(text.grapheme_indices(true).map(|(i, _)| i as u32));
        }
        let n = starts.len();
        let base = self.units.len();
        self.units.resize(base + n, 0.0);
        let out = &mut self.units[base..];
        let len = text.len() as u32;
        let mut g = 0usize;
        for (k, &(c, adv)) in clusters.iter().enumerate() {
            let c_end = clusters.get(k + 1).map_or(len, |x| x.0);
            let adv = adv as f32 * scale;
            while g < n && starts[g] < c {
                g += 1;
            }
            let first = g;
            while g < n && starts[g] < c_end {
                g += 1;
            }
            if g == first {
                // Cluster begins inside a grapheme: fold it into that grapheme.
                let into = first.saturating_sub(1).min(n.saturating_sub(1));
                if n > 0 {
                    out[into] += adv;
                }
            } else {
                // A cluster spanning several graphemes (a ligature like `fi`) is one glyph:
                // CoreText never breaks inside it, so its whole advance sits on its first
                // grapheme and the rest are zero-advance units the line walker never splits
                // before.
                out[first] += adv;
            }
        }
        if ls != 0.0 {
            for u in out.iter_mut() {
                *u += ls;
            }
        }
        n as u32
    }
}

/// How shaping two adjacent graphemes together differs from shaping them apart (see
/// [`WidthCache::pair_context`]).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct PairContext {
    /// Advance change on the left grapheme (kerning; a ligature's whole advance), points.
    pub left: f32,
    /// Advance change on the right grapheme (e.g. minus its advance when absorbed by a
    /// ligature), points.
    pub right: f32,
    /// Glyphs were substituted across the boundary (a ligature or contextual form): a line
    /// starting here must be shaped anew, as CoreText does.
    pub substituted: bool,
}

impl PairContext {
    const UNKNOWN: Self = Self {
        left: f32::NAN,
        right: 0.0,
        substituted: false,
    };
    pub(crate) const NONE: Self = Self {
        left: 0.0,
        right: 0.0,
        substituted: false,
    };
}

/// Borrowed per-grapheme advances of a cache entry.
pub(crate) enum UnitsRef<'a> {
    Single(f32),
    Many(&'a [f32]),
}
