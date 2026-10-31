//! LaTeX math in Markdown: zatex lays formulas out, GPUI paints them.
//!
//! zatex's layout is linear in the font size, so every formula is laid out
//! once in ems and cached by source ([`formula`]); a frame scales it to the
//! element's font size and never lays out again. Glyphs paint natively
//! through GPUI's glyph atlas from the bundled math fonts — the rasterizer
//! and cache body text uses — and rules paint as quads.
//!
//! Inline math reuses the reserved-slot pattern of file-link glyphs: the
//! flattened text holds non-breaking spaces as wide as the formula, copy and
//! selection map the slot back to `$tex$`, and [`MathGlyphs`] paints the
//! formula over it on the line's baseline. Display math is the same kind of
//! one-line text element whose line box is the formula's height, so drags
//! cross it and copy yields `$$tex$$` like any paragraph.
//!
//! Glyphs name their font by index into zatex's fixed bundled table
//! ([`zatex::bundled::font_at`]) whichever face set them, so each font
//! registers and verifies once, when a glyph first needs it, and every face
//! paints through the same ids. Faces nobody uses are never parsed.

use std::borrow::Cow;
use std::collections::HashMap;
use std::ops::Range;
use std::sync::{Arc, LazyLock, Mutex, OnceLock, PoisonError};

use gpui::{
    AnyElement, App, Bounds, Element, ElementId, FontId, GlobalElementId, GlyphId, Hsla,
    InspectorElementId, LayoutId, Pixels, Point, SharedString, TextLayout, TextRun, Window, point,
    px, size,
};
use zatex::bundled::Face;
use zatex::{Formula, Item};

use super::link_presentation::{OffsetMap, OriginalText};
use super::render::{FlatText, range_rects};

/// Formulas kept laid out, across faces and styles: enough for every formula
/// on screen plus recent scrollback.
const CACHE_CAPACITY: usize = 4096;

/// Tallest inline formula, in ems of its text: a text element has one line
/// height, so a taller one would open every line of its paragraph.
pub(crate) const MAX_INLINE_EMS: f32 = 12.0;
/// The bytes those formulas may hold (sources plus laid-out items). Most are
/// a few hundred bytes, but a long valid source lays out to thousands of
/// items, so count alone doesn't bound memory.
const CACHE_MAX_BYTES: usize = 8 * 1024 * 1024;

/// The face a slot's spaces are shaped in. An NBSP's advance follows its
/// font, so pinning it to the bundled Geist makes a slot's width a fixed
/// number of ems, whatever the interface font (as composer chips do).
const SLOT_FAMILY: &str = "Geist";
const NBSP: char = '\u{00A0}';

/// Clear space above and below a display formula, inside its line box. With
/// the block gap around it, display math breathes like a short paragraph.
/// It's in the units of the text size, not the formula's: a larger math size
/// grows the formula, not the gap around it.
const DISPLAY_PAD_Y: f32 = 6.0;

/// How a surface sets math: the "Render math", "Math font" and "Math size"
/// settings ([`crate::settings::math_style`]). Disabled, inline math shows
/// its `$tex$` source as inline code and display math as a `math` code block
/// — the same fallbacks as source zatex can't render.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MathStyle {
    pub enabled: bool,
    pub face: Face,
    /// Percent of the surrounding text size.
    pub size: u16,
}

impl MathStyle {
    /// Formula size per unit of text size.
    pub fn scale(self) -> f32 {
        f32::from(self.size) / 100.0
    }
}

impl Default for MathStyle {
    fn default() -> Self {
        Self {
            enabled: true,
            face: Face::Fira,
            size: crate::settings::MATH_SIZE_DEFAULT,
        }
    }
}

/// A formula painted over a reserved run of NBSPs. `range` is in displayed
/// text coordinates; `display` centers the formula in its line box instead
/// of setting it on the text baseline. `scale` is the formula's size per
/// unit of the element's font size ([`MathStyle::scale`]).
#[derive(Clone)]
pub struct MathSlot {
    pub range: Range<usize>,
    pub formula: Arc<Formula>,
    pub display: bool,
    pub scale: f32,
}

/// Laid-out formulas by face, style (`display`) and source, evicted least
/// recently used first once over either bound.
struct FormulaCache {
    entries: HashMap<(Face, bool), HashMap<Arc<str>, CachedFormula>>,
    /// Each entry's key by last use, so eviction pops the oldest without
    /// scanning.
    order: std::collections::BTreeMap<u64, (Face, bool, Arc<str>)>,
    bytes: usize,
    tick: u64,
    capacity: usize,
    max_bytes: usize,
}

struct CachedFormula {
    formula: Option<Arc<Formula>>,
    used: u64,
    bytes: usize,
}

impl FormulaCache {
    fn new(capacity: usize, max_bytes: usize) -> Self {
        Self {
            entries: HashMap::new(),
            order: Default::default(),
            bytes: 0,
            tick: 0,
            capacity,
            max_bytes,
        }
    }

    /// zatex's options for `display`, sized in ems.
    fn options(display: bool) -> zatex::Options {
        if display {
            zatex::Options::display(1.0)
        } else {
            zatex::Options::inline(1.0)
        }
    }

    /// The cached result for `tex`, marked used: `Some(None)` for a cached
    /// error, `None` when it isn't cached yet.
    fn lookup(&mut self, tex: &str, display: bool, face: Face) -> Option<Option<Arc<Formula>>> {
        self.tick += 1;
        let tick = self.tick;
        let entry = self.entries.get_mut(&(face, display))?.get_mut(tex)?;
        if let Some(key) = self.order.remove(&entry.used) {
            self.order.insert(tick, key);
        }
        entry.used = tick;
        Some(entry.formula.clone())
    }

    /// Caches `formula` (laid out from `tex`, or `None` for an error) and
    /// returns what's now cached. Should another lookup have laid `tex` out
    /// meanwhile, the entry already there wins, so every caller shares one
    /// layout.
    fn insert(
        &mut self,
        tex: &str,
        display: bool,
        face: Face,
        formula: Option<Arc<Formula>>,
    ) -> Option<Arc<Formula>> {
        if let Some(cached) = self.lookup(tex, display, face) {
            return cached;
        }
        let tick = self.tick;
        let key: Arc<str> = tex.into();
        let bytes = key.len()
            + formula.as_ref().map_or(0, |formula| {
                formula.items.len() * std::mem::size_of::<Item>()
            });
        self.entries.entry((face, display)).or_default().insert(
            key.clone(),
            CachedFormula {
                formula: formula.clone(),
                used: tick,
                bytes,
            },
        );
        self.order.insert(tick, (face, display, key));
        self.bytes += bytes;
        while self.order.len() > self.capacity || self.bytes > self.max_bytes {
            let Some((_, (face, display, key))) = self.order.pop_first() else {
                break;
            };
            if let Some(entry) = self
                .entries
                .get_mut(&(face, display))
                .and_then(|entries| entries.remove(&key))
            {
                self.bytes -= entry.bytes;
            }
        }
        formula
    }

    /// [`Self::lookup`], laying `tex` out and caching it on a miss. Tests use
    /// this; [`formula`] does the same without holding the cache through the
    /// layout.
    #[cfg(test)]
    fn get(&mut self, tex: &str, display: bool, face: Face) -> Option<Arc<Formula>> {
        let options = Self::options(display);
        if tex.len() > options.max_bytes {
            return None;
        }
        if let Some(cached) = self.lookup(tex, display, face) {
            return cached;
        }
        let laid = lay_out(tex, face, &options);
        self.insert(tex, display, face, laid)
    }
}

/// `tex` laid out in `face`, or `None` on any error.
fn lay_out(tex: &str, face: Face, options: &zatex::Options) -> Option<Arc<Formula>> {
    zatex::bundled::layout(tex, face, options)
        .ok()
        .map(Arc::new)
}

/// The process-wide formula cache.
fn cache() -> std::sync::MutexGuard<'static, FormulaCache> {
    static CACHE: LazyLock<Mutex<FormulaCache>> =
        LazyLock::new(|| Mutex::new(FormulaCache::new(CACHE_CAPACITY, CACHE_MAX_BYTES)));
    CACHE.lock().unwrap_or_else(PoisonError::into_inner)
}

/// `tex` laid out in `face` in ems (1.0 = the surrounding font size), or
/// `None` when zatex can't render it faithfully — the caller shows the
/// source instead. Errors are cached like formulas, so a malformed formula
/// isn't re-parsed every frame either.
///
/// The cache is only held to look up and to insert, never through a layout,
/// so one slow formula doesn't stall every other lookup (and the window)
/// behind it. Two threads missing on the same source may both lay it out;
/// the first to insert wins and both return its layout.
pub(crate) fn formula(tex: &str, display: bool, face: Face) -> Option<Arc<Formula>> {
    formula_with(tex, display, face, lay_out)
}

/// [`formula`] laying out with `lay_out`.
fn formula_with(
    tex: &str,
    display: bool,
    face: Face,
    lay_out: impl FnOnce(&str, Face, &zatex::Options) -> Option<Arc<Formula>>,
) -> Option<Arc<Formula>> {
    let options = FormulaCache::options(display);
    // zatex refuses these without parsing; keeping the key would hold the
    // whole oversized source for nothing.
    if tex.len() > options.max_bytes {
        return None;
    }
    if let Some(cached) = cache().lookup(tex, display, face) {
        return cached;
    }
    let laid = lay_out(tex, face, &options);
    cache().insert(tex, display, face, laid)
}

/// The advance of Geist's NBSP in ems: the unit a slot is measured in.
fn nbsp_em() -> f32 {
    static NBSP_EM: OnceLock<f32> = OnceLock::new();
    *NBSP_EM.get_or_init(|| {
        crate::typography::bundled_font_faces()
            .next()
            .and_then(|data| ttf_parser::Face::parse(data, 0).ok())
            .and_then(|face| {
                let advance = face.glyph_hor_advance(face.glyph_index(NBSP)?)?;
                Some(f32::from(advance) / f32::from(face.units_per_em()))
            })
            // Geist's own value, should the bundled face ever fail to parse.
            .unwrap_or(0.25)
    })
}

/// How many fonts glyphs can name ([`zatex::bundled::FONTS`]).
const FONTS: usize = zatex::bundled::FONTS as usize;

/// The bundled math face a glyph font index names, parsed on first use.
fn math_face(font: u8) -> Option<&'static ttf_parser::Face<'static>> {
    static FACES: [OnceLock<Option<ttf_parser::Face<'static>>>; FONTS] =
        [const { OnceLock::new() }; FONTS];
    FACES
        .get(usize::from(font))?
        .get_or_init(|| ttf_parser::Face::parse(zatex::bundled::font_at(font).data(), 0).ok())
        .as_ref()
}

/// The horizontal extent `formula` paints over, in ems from its origin: its
/// advance box widened to everything it paints (zatex's `left`/`right`
/// cover every glyph's ink and rule), so negative space or an overhang never
/// paints past its slot. O(1), as it's asked every frame per formula.
pub(crate) fn span(formula: &Formula) -> Range<f32> {
    formula.left.min(formula.width).min(0.0)..formula.right.max(formula.width).max(0.0)
}

/// The widest (and tallest) a formula may set, in ems of the surrounding
/// text, math size included. zatex already refuses formulas wider than
/// [`zatex::Options::max_width`] (2000 em, so 4000 at the largest math size);
/// this is the second line of defence, so a slot never grows into
/// a string GPUI shapes every frame on surfaces that don't cache layout. At
/// Geist's quarter-em NBSP it's at most ~32K spaces (64 KB).
const MAX_SLOT_EMS: f32 = 8192.0;

/// The NBSPs that reserve room for `formula` set at `scale`: whole spaces
/// covering all it paints, rounded up so it never overlaps its neighbors; it
/// centers in the slack. NBSPs are word characters to GPUI's wrapper, so a
/// slot only splits across rows when it (with any text glued to it) is wider
/// than the column.
///
/// `None` when the formula is too big to set (past [`MAX_SLOT_EMS`], or with
/// a non-finite extent): the caller shows its source instead, as for one
/// zatex can't render.
pub(crate) fn slot_text(formula: &Formula, scale: f32) -> Option<String> {
    let span = span(formula);
    // `span` skips a NaN advance (`min`/`max` ignore NaN), so check it here.
    let width = if formula.width.is_finite() {
        (span.end - span.start) * scale
    } else {
        f32::NAN
    };
    let height = formula.height() * scale;
    // NaN fails every comparison, so it's rejected too.
    let fits = |ems: f32| (0.0..=MAX_SLOT_EMS).contains(&ems);
    if !(fits(width) && fits(height.abs())) {
        return None;
    }
    let count = (width / nbsp_em()).ceil().max(1.0) as usize;
    Some(std::iter::repeat_n(NBSP, count).collect())
}

/// The font a slot shapes in (see [`SLOT_FAMILY`]).
pub(crate) fn slot_font() -> gpui::Font {
    gpui::font(SLOT_FAMILY)
}

/// The line height, in the same units as `size`, that keeps a text
/// element's inline formulas inside their line boxes. GPUI sets a line's
/// baseline `(ascent - descent) / 2` below the box's middle, taking ascent
/// and descent as the maxima over the fonts of that line's runs, so a
/// formula fits when the box's half-height covers its ascent above that
/// point and its descent below it. The line height is the element's, so a
/// tall formula (fraction, stacked scripts) opens every line of its
/// paragraph; paragraphs without one keep their rhythm.
pub(crate) fn line_height(flat: &FlatText, line_height: f32, size: f32, window: &Window) -> f32 {
    let text_system = window.text_system();
    line_height_with(flat, line_height, size, |font| {
        let id = text_system.resolve_font(font);
        (
            f32::from(text_system.ascent(id, px(1.0))),
            -f32::from(text_system.descent(id, px(1.0))),
        )
    })
}

/// [`line_height`] with font metrics (ascent, descent in ems) from `metrics`.
fn line_height_with(
    flat: &FlatText,
    line_height: f32,
    size: f32,
    mut metrics: impl FnMut(&gpui::Font) -> (f32, f32),
) -> f32 {
    if flat.math.iter().all(|slot| slot.display) {
        return line_height;
    }
    let mut half = 0.0f32;
    // One forward pass over lines, inline slots (by position) and runs:
    // hard-broken lines of formulas must not cost lines × slots per render.
    let mut slots: Vec<&MathSlot> = flat.math.iter().filter(|slot| !slot.display).collect();
    slots.sort_by_key(|slot| slot.range.start);
    let mut next_slot = 0;
    let (mut first_run, mut first_run_at) = (0, 0);
    let mut start = 0;
    for line in flat.text.split('\n') {
        let range = start..start + line.len();
        start = range.end + 1;
        while next_slot < slots.len() && slots[next_slot].range.start < range.start {
            next_slot += 1;
        }
        let begin = next_slot;
        while next_slot < slots.len() && range.contains(&slots[next_slot].range.start) {
            next_slot += 1;
        }
        let line_slots = &slots[begin..next_slot];
        if line_slots.is_empty() {
            continue;
        }
        while first_run < flat.runs.len() && first_run_at + flat.runs[first_run].len <= range.start
        {
            first_run_at += flat.runs[first_run].len;
            first_run += 1;
        }
        let (mut ascent, mut descent) = (0.0f32, 0.0f32);
        let mut at = first_run_at;
        for run in &flat.runs[first_run..] {
            if at >= range.end {
                break;
            }
            let run_range = at..at + run.len;
            at = run_range.end;
            if run_range.end > range.start {
                let (run_ascent, run_descent) = metrics(&run.font);
                ascent = ascent.max(run_ascent);
                descent = descent.max(run_descent);
            }
        }
        let lift = (ascent - descent) / 2.0;
        for slot in line_slots {
            let (ascent, descent) = (
                slot.formula.ascent * slot.scale,
                slot.formula.descent * slot.scale,
            );
            half = half.max((ascent - lift).max(descent + lift));
        }
    }
    line_height.max((2.0 * half * size).ceil())
}

/// The line box a display formula set at `scale` sits centered in, in the
/// units of `size`.
pub(crate) fn display_line_height(
    formula: &Formula,
    scale: f32,
    size: f32,
    line_height: f32,
) -> f32 {
    line_height.max((formula.height() * scale * size + 2.0 * DISPLAY_PAD_Y).ceil())
}

/// A display formula as one line of text: its slot shows the formula, and
/// copy and selection read the source as `$$tex$$`. `None` when the formula
/// is too big to set ([`slot_text`]).
pub(crate) fn display_flat(
    tex: &str,
    formula: Arc<Formula>,
    scale: f32,
    color: Hsla,
) -> Option<FlatText> {
    let text = slot_text(&formula, scale)?;
    let source = format!("$${tex}$$");
    let range = 0..text.len();
    Some(FlatText {
        original: Some(OriginalText {
            offsets: OffsetMap {
                omissions: vec![(0..source.len(), range.clone())],
                prior: None,
            },
            text: source.into(),
        }),
        runs: vec![TextRun {
            len: text.len(),
            font: slot_font(),
            color,
            background_color: None,
            underline: None,
            strikethrough: None,
        }],
        text: text.into(),
        links: Vec::new(),
        code_ranges: Vec::new(),
        file_glyphs: Vec::new(),
        math: vec![MathSlot {
            range,
            formula,
            display: true,
            scale,
        }],
    })
}

/// The paint color of the text run covering byte `at`.
pub(crate) fn color_at(runs: &[TextRun], at: usize) -> Option<Hsla> {
    let mut end = 0;
    runs.iter().find_map(|run| {
        end += run.len;
        (at < end).then_some(run.color)
    })
}

/// The bundled math fonts' GPUI ids, by glyph font index: `None` until a
/// glyph first needs the font, then what [`register`] made of it. A font
/// that fails verification stays `Some(None)` and its glyphs paint as
/// outlines from the bundled data, never through some other copy of the
/// family whose glyph ids might differ.
#[derive(Default)]
struct MathFonts([Option<Option<FontId>>; FONTS]);

impl gpui::Global for MathFonts {}

impl MathFonts {
    /// `font`'s id, registered through `register` the first time it's asked
    /// for. `None` paints outlines.
    fn id(
        &mut self,
        font: u8,
        register: impl FnOnce(&'static zatex::MathFont) -> Option<FontId>,
    ) -> Option<FontId> {
        *self
            .0
            .get_mut(usize::from(font))?
            .get_or_insert_with(|| register(zatex::bundled::font_at(font)))
    }
}

/// Registers `font` with the text system and resolves it by family; `None`
/// unless what resolves is the bundled bytes (see [`is_bundled_face`]).
fn register(
    text_system: &gpui::WindowTextSystem,
    font: &'static zatex::MathFont,
) -> Option<FontId> {
    if let Err(error) = text_system.add_fonts(vec![Cow::Borrowed(font.data())]) {
        tracing::warn!(%error, "failed to register a bundled math font");
    }
    let family = family_name(font.data())?;
    let id = text_system.resolve_font(&gpui::font(family.clone()));
    let bundled = is_bundled_face(text_system, id, &family, font.data());
    if !bundled {
        tracing::warn!(%family, "math font resolved to another copy; painting outlines");
    }
    bundled.then_some(id)
}

fn math_font_id(font: u8, window: &Window, cx: &mut App) -> Option<FontId> {
    cx.default_global::<MathFonts>()
        .id(font, |data| register(window.text_system(), data))
}

/// Whether `id` shapes like `data`: a spread of the face's characters, laid
/// out in `family`, must come back in `id` with the glyph ids `data` maps
/// them to. A system copy of another version (macOS ships STIX Two Math)
/// renumbers glyphs, which this catches.
fn is_bundled_face(
    text_system: &gpui::WindowTextSystem,
    id: FontId,
    family: &str,
    data: &[u8],
) -> bool {
    let Ok(face) = ttf_parser::Face::parse(data, 0) else {
        return false;
    };
    let mut chars = Vec::new();
    if let Some(cmap) = face.tables().cmap {
        for subtable in cmap.subtables {
            if subtable.is_unicode() {
                subtable.codepoints(|code| chars.extend(char::from_u32(code)));
            }
        }
    }
    // Latin, Greek, arrows and operators: what math sets, and what shapes
    // one glyph per character. (Some faces carry Indic marks, which reorder
    // and combine.)
    chars.retain(|c| {
        c.is_ascii_alphanumeric()
            || ('\u{0391}'..='\u{03C9}').contains(c)
            || ('\u{2190}'..='\u{22FF}').contains(c)
    });
    chars.sort_unstable();
    chars.dedup();
    if chars.is_empty() {
        return false;
    }
    let step = chars.len().div_ceil(32);
    // Spaced apart, so no ligature or contextual form can stand in.
    let text: String = chars.iter().step_by(step).flat_map(|&c| [c, ' ']).collect();
    let run = TextRun {
        len: text.len(),
        font: gpui::font(family.to_owned()),
        color: gpui::black(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let line = text_system.layout_line(&text, px(16.0), &[run], None);
    let mut checked = 0;
    for run in &line.runs {
        for glyph in &run.glyphs {
            let Some(c) = text[glyph.index..].chars().next() else {
                return false;
            };
            if c == ' ' {
                continue;
            }
            let expected = face.glyph_index(c).map(|glyph| u32::from(glyph.0));
            if run.font_id != id || expected != Some(glyph.id.0) {
                return false;
            }
            checked += 1;
        }
    }
    checked > 0
}

/// The family name a text system selects `data` by.
fn family_name(data: &[u8]) -> Option<String> {
    let face = ttf_parser::Face::parse(data, 0).ok()?;
    let names = face.names();
    [
        ttf_parser::name_id::TYPOGRAPHIC_FAMILY,
        ttf_parser::name_id::FAMILY,
    ]
    .into_iter()
    .find_map(|id| {
        names
            .into_iter()
            .filter(|name| name.name_id == id && name.is_unicode())
            .find_map(|name| name.to_string())
    })
}

/// Paint `formula` with its baseline's left end at `origin`, scaled from
/// ems to `font_size`.
pub(crate) fn paint_formula(
    formula: &Formula,
    origin: Point<Pixels>,
    font_size: Pixels,
    color: Hsla,
    window: &mut Window,
    cx: &mut App,
) {
    let scale = f32::from(font_size);
    // Glyphs of a face the text system can't vouch for, as filled outlines.
    let mut outlines: Option<OutlineSink> = None;
    // Thin rules (fraction bars) keep at least one device pixel; a quad
    // snapped to zero height would vanish.
    let min_rule = 1.0 / window.scale_factor();
    for item in &formula.items {
        match *item {
            Item::Glyph {
                font,
                glyph,
                x,
                y,
                size,
            } => {
                let at = point(origin.x + px(x * scale), origin.y + px(y * scale));
                if let Some(font_id) = math_font_id(font, window, cx) {
                    window
                        .paint_glyph(
                            at,
                            font_id,
                            GlyphId(u32::from(glyph)),
                            px(size * scale),
                            color,
                        )
                        .ok();
                } else {
                    let sink = outlines.get_or_insert_with(OutlineSink::default);
                    zatex::bundled::font_at(font).outline(
                        glyph,
                        size * scale,
                        at.x.into(),
                        at.y.into(),
                        sink,
                    );
                }
            }
            Item::Rule {
                x,
                y,
                width,
                height,
            } => window.paint_quad(gpui::fill(
                Bounds::new(
                    point(origin.x + px(x * scale), origin.y + px(y * scale)),
                    size(px(width * scale), px((height * scale).max(min_rule))),
                ),
                color,
            )),
        }
    }
    if let Some(sink) = outlines
        && let Ok(path) = sink.0.build()
    {
        window.paint_path(path, color);
    }
}

/// Glyph outlines into one path, filled non-zero as fonts are drawn.
struct OutlineSink(gpui::PathBuilder);

impl Default for OutlineSink {
    fn default() -> Self {
        Self(gpui::PathBuilder::fill().with_style(gpui::PathStyle::Fill(
            gpui::FillOptions::default().with_fill_rule(gpui::FillRule::NonZero),
        )))
    }
}

impl zatex::PathSink for OutlineSink {
    fn move_to(&mut self, x: f32, y: f32) {
        self.0.move_to(point(px(x), px(y)));
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.0.line_to(point(px(x), px(y)));
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        self.0.curve_to(point(px(x), px(y)), point(px(x1), px(y1)));
    }
    fn cubic_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        self.0.cubic_bezier_to(
            point(px(x), px(y)),
            point(px(x1), px(y1)),
            point(px(x2), px(y2)),
        );
    }
    fn close(&mut self) {
        self.0.close();
    }
}

/// Paints each slot's formula over the shaped text: centered in the slot's
/// reserved width, on its line's baseline (inline) or centered in the line
/// box (display). The NBSPs underneath keep layout, selection and copy.
pub(crate) struct MathGlyphs {
    pub id: SharedString,
    pub child: AnyElement,
    pub layout: TextLayout,
    /// Each slot with the paint color of the run it replaces (veil included).
    pub slots: Vec<(MathSlot, Hsla)>,
}

impl gpui::IntoElement for MathGlyphs {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

/// One formula placed for paint: its baseline origin, font size and color.
type Placed = (Arc<Formula>, Point<Pixels>, Pixels, Hsla);

impl Element for MathGlyphs {
    type RequestLayoutState = ();
    type PrepaintState = Vec<Placed>;

    fn id(&self) -> Option<ElementId> {
        Some(self.id.clone().into())
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.child.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Vec<Placed> {
        self.child.prepaint(window, cx);
        let font_size = window.text_style().font_size.to_pixels(window.rem_size());
        let line_height = self.layout.line_height();
        let mut placed = Vec::with_capacity(self.slots.len());
        for (slot, color) in &self.slots {
            // The row holding the slot's start. A slot only splits when it's
            // wider than its column (see `slot_text`); the formula then sets
            // from the slot's start and runs on past the row's end, never
            // back over the text before it.
            let rects = range_rects(&self.layout, &slot.range, 0.0, 0.0);
            let Some(&rect) = rects.first() else {
                continue;
            };
            let Some(line) = self.layout.line_layout_for_index(slot.range.start) else {
                continue;
            };
            let formula = &slot.formula;
            // What it paints, not its advance (negative space can shrink
            // that below zero).
            let span = span(formula);
            let mut math_size = font_size * slot.scale;
            // Wider than the room from its slot to the element's edge (a
            // slot that wrapped, a column narrower than the formula): shrink
            // it to fit, as nothing beside a formula may be painted over.
            let room = f32::from(bounds.origin.x + bounds.size.width - rect.origin.x);
            // No room left on its row (a slot past the element's edge):
            // nothing it painted could stay inside, so it isn't painted.
            if room <= 0.0 {
                continue;
            }
            let wide = f32::from(math_size) * (span.end - span.start);
            if wide > room {
                math_size *= room / wide;
            }
            let baseline = if slot.display {
                rect.origin.y
                    + (line_height - math_size * formula.height()) / 2.0
                    + math_size * formula.ascent
            } else {
                // GPUI's own baseline for this row (see `line_height`).
                let line = &line.unwrapped_layout;
                rect.origin.y + (line_height - line.ascent - line.descent) / 2.0 + line.ascent
            };
            // Centered in the slot's slack by what it paints.
            let scale = f32::from(math_size);
            // Never past the room left: centering in a slot wider than the
            // row can't push the formula over the element's edge.
            let painted = scale * (span.end - span.start);
            let slack = if rects.len() == 1 {
                ((f32::from(rect.size.width) - painted).max(0.0) / 2.0)
                    .min((room - painted).max(0.0))
            } else {
                0.0
            };
            let x = rect.origin.x + px(slack - scale * span.start);
            placed.push((formula.clone(), point(x, baseline), math_size, *color));
        }
        placed
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        placed: &mut Vec<Placed>,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.paint(window, cx);
        for (formula, origin, font_size, color) in placed.iter() {
            paint_formula(formula, *origin, *font_size, *color, window, cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formulas_are_cached_by_source_and_style() {
        let first = formula(r"\frac{a}{b}", false, Face::Fira).expect("lays out");
        let again = formula(r"\frac{a}{b}", false, Face::Fira).expect("lays out");
        assert!(
            Arc::ptr_eq(&first, &again),
            "a repeat lookup reuses the layout"
        );
        let display = formula(r"\frac{a}{b}", true, Face::Fira).expect("lays out");
        assert!(!Arc::ptr_eq(&first, &display));
        assert!(display.height() > first.height(), "display style is taller");
        // Errors are remembered too.
        assert!(formula(r"\notacommand", false, Face::Fira).is_none());
        assert!(formula(r"\notacommand", false, Face::Fira).is_none());
    }

    #[test]
    fn the_cache_stays_under_its_byte_bound() {
        let mut cache = FormulaCache::new(CACHE_CAPACITY, 1024 * 1024);
        let mut kept = 0;
        for i in 0..24 {
            // ~10 KB of source: valid, within zatex's width and height
            // limits, and thousands of items laid out.
            let row = format!("{}x", "x&".repeat(49));
            let rows = vec![row; 100].join(r"\\");
            let tex = format!(r"{i}\begin{{matrix}}{rows}\end{{matrix}}");
            let laid = cache.get(&tex, true, Face::Fira).expect("lays out");
            assert!(laid.items.len() > 5_000);
            let held: usize = cache
                .entries
                .values()
                .flat_map(HashMap::values)
                .map(|entry| entry.bytes)
                .sum();
            assert_eq!(held, cache.bytes);
            assert!(cache.bytes <= cache.max_bytes, "{}", cache.bytes);
            kept = cache.order.len();
        }
        assert!(kept > 1 && kept < 24, "{kept}");
        // A source past zatex's limit is refused, never kept.
        let long = "x".repeat(zatex::Options::default().max_bytes + 1);
        let (count, bytes) = (cache.order.len(), cache.bytes);
        assert!(cache.get(&long, false, Face::Fira).is_none());
        assert_eq!((cache.order.len(), cache.bytes), (count, bytes));
        // Count still bounds small formulas, least recently used out first.
        let mut cache = FormulaCache::new(2, CACHE_MAX_BYTES);
        let a = cache.get("a", false, Face::Fira).unwrap();
        cache.get("b", false, Face::Fira);
        assert!(Arc::ptr_eq(&a, &cache.get("a", false, Face::Fira).unwrap()));
        cache.get("c", false, Face::Fira);
        let entries = &cache.entries[&(Face::Fira, false)];
        assert!(entries.contains_key("a") && !entries.contains_key("b"));
    }

    #[test]
    fn formulas_are_cached_per_face() {
        let tex = r"\mathcal{L} = \int_0^1 f(x)\,dx";
        let fira = formula(tex, false, Face::Fira).expect("lays out");
        let stix = formula(tex, false, Face::StixTwo).expect("lays out");
        assert!(!Arc::ptr_eq(&fira, &stix), "each face keeps its own layout");
        assert!(Arc::ptr_eq(
            &stix,
            &formula(tex, false, Face::StixTwo).unwrap()
        ));
        assert!(Arc::ptr_eq(
            &fira,
            &formula(tex, false, Face::Fira).unwrap()
        ));
        let options = zatex::Options::inline(1.0);
        for (face, laid) in [(Face::Fira, &fira), (Face::StixTwo, &stix)] {
            let fresh = zatex::bundled::layout(tex, face, &options).unwrap();
            assert_eq!(laid.items, fresh.items, "{face:?}");
        }
        assert_ne!(fira.width, stix.width, "the faces set differently");
    }

    /// Counts the outline commands a glyph paints with.
    #[derive(Default)]
    struct CountingSink(usize);

    impl zatex::PathSink for CountingSink {
        fn move_to(&mut self, _: f32, _: f32) {
            self.0 += 1;
        }
        fn line_to(&mut self, _: f32, _: f32) {
            self.0 += 1;
        }
        fn quad_to(&mut self, _: f32, _: f32, _: f32, _: f32) {
            self.0 += 1;
        }
        fn cubic_to(&mut self, _: f32, _: f32, _: f32, _: f32, _: f32, _: f32) {
            self.0 += 1;
        }
        fn close(&mut self) {}
    }

    #[test]
    fn every_face_sets_and_paints_through_the_bundled_table() {
        let stix = Face::StixTwo.index();
        let tex = r"\mathcal{L} = \int_0^1 \frac{x^2}{2}\,dx + \alpha";
        for face in Face::ALL {
            let laid = formula(tex, true, face).unwrap();
            let mut fonts = Vec::new();
            let mut painted = CountingSink::default();
            for item in &laid.items {
                if let Item::Glyph { font, glyph, .. } = *item {
                    assert!(usize::from(font) < FONTS, "{face:?}");
                    let parsed = math_face(font).unwrap();
                    assert!(
                        parsed
                            .glyph_hor_advance(ttf_parser::GlyphId(glyph))
                            .is_some()
                    );
                    zatex::bundled::font_at(font).outline(glyph, 16.0, 0.0, 0.0, &mut painted);
                    fonts.push(font);
                }
            }
            assert!(painted.0 > 0, "{face:?} paints");
            // Each face sets its own glyphs; STIX Two only fills its gaps.
            assert!(fonts.contains(&face.index()), "{face:?}");
            assert!(
                fonts
                    .iter()
                    .all(|&font| font == face.index() || font == stix),
                "{face:?}: {fonts:?}"
            );
        }
        // Fira falls back to STIX Two for script letters.
        let fira = formula(r"\mathcal{L}", false, Face::Fira).unwrap();
        assert!(fira.items.iter().any(|item| matches!(
            *item,
            Item::Glyph { font, .. } if font == stix
        )));
    }

    #[test]
    fn math_fonts_register_only_the_faces_glyphs_use() {
        let mut fonts = MathFonts::default();
        let mut registered = Vec::new();
        let laid = formula(r"\mathcal{L} + x", false, Face::Fira).unwrap();
        for _ in 0..2 {
            for item in &laid.items {
                if let Item::Glyph { font, .. } = *item {
                    fonts.id(font, |data| {
                        registered.push(font);
                        assert!(std::ptr::eq(data, zatex::bundled::font_at(font)));
                        Some(FontId(usize::from(font)))
                    });
                }
            }
        }
        registered.sort_unstable();
        assert_eq!(
            registered,
            [Face::Fira.index(), Face::StixTwo.index()],
            "each used face once, no other"
        );
        assert_eq!(
            fonts.0.iter().filter(|id| id.is_some()).count(),
            2,
            "untouched faces stay unregistered"
        );
        // A face that fails verification paints outlines and isn't retried.
        let euler = Face::Euler.index();
        assert_eq!(fonts.id(euler, |_| None), None);
        assert_eq!(fonts.id(euler, |_| unreachable!()), None);
        // Past the table there's nothing to register.
        assert_eq!(fonts.id(zatex::bundled::FONTS, |_| unreachable!()), None);
    }

    #[test]
    fn slots_cover_the_formula_in_whole_spaces() {
        let wide = formula(r"a + b + c + d", false, Face::Fira).unwrap();
        let text = slot_text(&wide, 1.0).unwrap();
        assert!(text.chars().all(|c| c == NBSP));
        let count = text.chars().count() as f32;
        assert!(count * nbsp_em() >= wide.width);
        assert!((count - 1.0) * nbsp_em() < wide.width);
        // Geist's NBSP is a real advance, not the fallback guess.
        assert!(nbsp_em() > 0.1 && nbsp_em() < 0.5);
    }

    #[test]
    fn negative_space_stays_inside_the_slot() {
        let formula = formula(r"a\hspace{-5em}b", false, Face::Fira).unwrap();
        assert!(formula.width < 0.0, "the advance runs backwards");
        let span = span(&formula);
        assert!(span.start < -4.0 && span.end > 0.0, "{span:?}");
        // The slot reserves everything painted, not the negative advance.
        let count = slot_text(&formula, 1.0).unwrap().chars().count() as f32;
        assert!(count * nbsp_em() >= span.end - span.start);
        // An ordinary formula spans its advance box.
        let plain = self::formula("x+y", false, Face::Fira).unwrap();
        let span = self::span(&plain);
        assert!(span.start >= 0.0 && (span.end - plain.width).abs() < 0.2);
    }

    /// A formula `width` ems wide and one em tall, as no source sets it.
    fn ruled(width: f32) -> Formula {
        Formula {
            width,
            ascent: 0.75,
            descent: 0.25,
            left: 0.0,
            right: width,
            items: vec![Item::Rule {
                x: 0.0,
                y: -0.75,
                width,
                height: 1.0,
            }],
        }
    }

    #[test]
    fn slots_refuse_formulas_too_big_to_set() {
        // At the cap a slot is still a bounded string.
        let edge = slot_text(&ruled(MAX_SLOT_EMS), 1.0).expect("fits");
        assert!(edge.len() <= 2 * (MAX_SLOT_EMS / nbsp_em()).ceil() as usize);
        assert!(edge.len() < 128 * 1024, "{}", edge.len());
        // Past it, in ems or once scaled, nothing is built.
        for (formula, scale) in [
            (ruled(1e7), 1.0),
            (ruled(MAX_SLOT_EMS * 2.0), 1.0),
            (ruled(MAX_SLOT_EMS * 0.75), 2.0),
            (ruled(f32::INFINITY), 1.0),
            (ruled(f32::NAN), 1.0),
            (ruled(10.0), f32::NAN),
            (ruled(10.0), -1.0),
        ] {
            assert!(
                slot_text(&formula, scale).is_none(),
                "{} @ {scale}",
                formula.width
            );
        }
        // As are absurdly tall ones and non-finite extents.
        let tall = Formula {
            ascent: 1e7,
            ..ruled(1.0)
        };
        assert!(slot_text(&tall, 1.0).is_none());
        let nan = Formula {
            descent: f32::NAN,
            ..ruled(1.0)
        };
        assert!(slot_text(&nan, 1.0).is_none());
        assert!(display_flat("x", Arc::new(ruled(1e7)), 1.0, gpui::black()).is_none());
    }

    #[test]
    fn formulas_too_big_to_set_show_their_source() {
        // Seed the cache with a layout no source would set, so this doesn't
        // depend on zatex's own width limit.
        let tex = r"\text{too wide to set (slot test)}";
        let laid = formula_with(tex, false, Face::Fira, |_, _, _| Some(Arc::new(ruled(1e7))))
            .expect("cached");
        assert_eq!(laid.width, 1e7);
        let flat = inline(&[("so ", false), (tex, true)]);
        assert!(flat.math.is_empty(), "no slot");
        assert_eq!(flat.text.as_ref(), format!("so ${tex}$"));
        assert_eq!(flat.code_ranges.len(), 1, "shown as inline code");
    }

    #[test]
    fn layout_runs_outside_the_cache_lock() {
        use std::sync::mpsc;
        use std::time::Duration;
        let (started, on_start) = mpsc::channel();
        let (release, on_release) = mpsc::channel::<()>();
        let slow = r"\text{slow layout (lock test)}";
        let blocked = std::thread::spawn(move || {
            formula_with(slow, false, Face::Fira, |tex, face, options| {
                started.send(()).unwrap();
                on_release.recv().unwrap();
                lay_out(tex, face, options)
            })
        });
        on_start.recv().unwrap();
        // While that layout is in flight, other lookups and layouts go
        // through: on another thread, so a held lock fails instead of hangs.
        let (done, on_done) = mpsc::channel();
        std::thread::spawn(move || {
            let hit = formula("x", false, Face::Fira).is_some();
            let miss = formula(r"\text{fresh (lock test)}", true, Face::Fira).is_some();
            done.send(hit && miss).unwrap();
        });
        let finished = on_done.recv_timeout(Duration::from_secs(10));
        // The same source laid out meanwhile is cached first; the slow
        // thread's layout then defers to it, so everyone shares one.
        let first = formula_with(slow, false, Face::Fira, lay_out).expect("lays out");
        release.send(()).unwrap();
        let late = blocked.join().unwrap().expect("lays out");
        assert_eq!(finished, Ok(true), "a slow layout blocked other lookups");
        assert!(Arc::ptr_eq(&first, &late));
        assert!(Arc::ptr_eq(
            &first,
            &formula(slow, false, Face::Fira).unwrap()
        ));
    }

    #[test]
    fn slots_scale_with_the_math_size() {
        let wide = formula(r"a + b + c + d", false, Face::Fira).unwrap();
        let count = |scale: f32| slot_text(&wide, scale).unwrap().chars().count() as f32;
        // 100% is the size-independent slot, unscaled.
        let span = span(&wide);
        let normal = ((span.end - span.start) / nbsp_em()).ceil();
        assert_eq!(count(1.0), normal);
        // Linear up to the rounding to whole spaces.
        for scale in [0.75, 1.35, 2.0] {
            let scaled = count(scale);
            assert!(scaled * nbsp_em() >= scale * (span.end - span.start));
            assert!((scaled - normal * scale).abs() <= 1.0, "{scale}: {scaled}");
        }
        assert_eq!(MathStyle::default().scale(), 1.0);
        assert_eq!(
            MathStyle {
                size: 135,
                ..Default::default()
            }
            .scale(),
            1.35
        );
    }

    #[test]
    fn display_blocks_scale_the_formula_but_not_its_padding() {
        let laid = formula(r"\int_0^1 \frac{x^2}{2}\,dx", true, Face::Fira).unwrap();
        let normal = display_line_height(&laid, 1.0, 14.0, 22.0);
        assert_eq!(
            normal,
            22.0f32.max((laid.height() * 14.0 + 2.0 * DISPLAY_PAD_Y).ceil())
        );
        let larger = display_line_height(&laid, 1.5, 14.0, 22.0);
        let grown = larger - 2.0 * DISPLAY_PAD_Y;
        let formula = normal - 2.0 * DISPLAY_PAD_Y;
        assert!((grown - 1.5 * formula).abs() <= 2.0, "{normal} -> {larger}");
        // The slot widens with it.
        let width = |scale| {
            display_flat("x", laid.clone(), scale, gpui::black())
                .unwrap()
                .text
                .len()
        };
        assert!(width(1.5) > width(1.0));
        assert_eq!(
            display_flat("x", laid.clone(), 1.5, gpui::black())
                .unwrap()
                .math[0]
                .scale,
            1.5
        );
    }

    fn inline(parts: &[(&str, bool)]) -> FlatText {
        let runs: Vec<_> = parts
            .iter()
            .map(|&(text, math)| crate::markdown::parser::InlineRun {
                text: text.into(),
                style: crate::markdown::parser::InlineStyle {
                    math,
                    ..Default::default()
                },
            })
            .collect();
        crate::markdown::render::flatten_runs(&runs, &crate::theme::Theme::dark(), false)
    }

    /// [`inline`] with every formula set at `scale`.
    fn inline_at(parts: &[(&str, bool)], scale: f32) -> FlatText {
        let mut flat = inline(parts);
        for slot in &mut flat.math {
            slot.scale = scale;
        }
        flat
    }

    #[test]
    fn line_height_follows_the_metrics_gpui_sets_the_baseline_with() {
        const GEIST: (f32, f32) = (1.005, 0.295);
        let geist = |_: &gpui::Font| GEIST;
        let short = inline(&[("so ", false), ("x^2", true)]);
        assert_eq!(line_height_with(&short, 22.0, 14.0, geist), 22.0);
        let mut tall = inline(&[("so ", false), (r"\frac{a^2}{b_1}", true)]);
        let grown = line_height_with(&tall, 22.0, 14.0, geist);
        assert!(grown > 22.0 && grown < 40.0, "{grown}");
        // A taller interface font lifts the baseline; the box grows to keep
        // the fraction's denominator inside.
        tall.runs[0].font.family = "Tall".into();
        let lifted = line_height_with(&tall, 22.0, 14.0, |font| {
            if font.family.as_ref() == "Tall" {
                (1.6, 0.3)
            } else {
                GEIST
            }
        });
        assert!(lifted > grown, "{lifted} vs {grown}");
        // Only lines holding a formula count.
        let mut apart = inline(&[
            ("Tall\n", false),
            ("so ", false),
            (r"\frac{a^2}{b_1}", true),
        ]);
        apart.runs[0].font.family = "Tall".into();
        let lone = line_height_with(&apart, 22.0, 14.0, |font| {
            if font.family.as_ref() == "Tall" {
                (1.6, 0.3)
            } else {
                GEIST
            }
        });
        assert_eq!(lone, grown);
        // Display slots size their own block instead.
        let display = display_flat(
            r"\frac{a^2}{b_1}",
            formula(r"\frac{a^2}{b_1}", true, Face::Fira).unwrap(),
            1.0,
            gpui::black(),
        )
        .unwrap();
        assert_eq!(line_height_with(&display, 22.0, 14.0, geist), 22.0);
    }

    #[test]
    fn line_height_growth_scales_with_the_math_size() {
        const GEIST: (f32, f32) = (1.005, 0.295);
        let geist = |_: &gpui::Font| GEIST;
        let parts = [("so ", false), (r"\frac{a^2}{b_1}", true)];
        let at = |scale: f32| line_height_with(&inline_at(&parts, scale), 0.0, 14.0, geist);
        // 100% grows exactly as before the size setting existed.
        let laid = formula(r"\frac{a^2}{b_1}", false, Face::Fira).unwrap();
        let lift = (GEIST.0 - GEIST.1) / 2.0;
        let half = (laid.ascent - lift).max(laid.descent + lift);
        assert_eq!(at(1.0), (2.0 * half * 14.0).ceil());
        let (normal, larger) = (at(1.0), at(1.5));
        let scaled = 2.0 * (1.5 * laid.ascent - lift).max(1.5 * laid.descent + lift) * 14.0;
        assert_eq!(larger, scaled.ceil());
        assert!(larger > normal * 1.3, "{normal} -> {larger}");
        assert!(at(0.75) < normal);
        // A small formula still keeps the paragraph's own line height.
        let short = inline_at(&[("so ", false), ("x", true)], 1.5);
        assert_eq!(line_height_with(&short, 22.0, 14.0, geist), 22.0);
    }

    #[test]
    fn display_math_copies_its_delimited_source() {
        let formula = formula(r"\sum_{i=1}^n i", true, Face::Fira).unwrap();
        let flat = display_flat(r"\sum_{i=1}^n i", formula, 1.0, gpui::black()).unwrap();
        let original = flat.original.as_ref().unwrap();
        assert_eq!(original.text.as_ref(), r"$$\sum_{i=1}^n i$$");
        assert_eq!(original.offsets.original(0), 0);
        assert_eq!(
            original.offsets.original(flat.text.len()),
            original.text.len()
        );
        assert_eq!(flat.math[0].range, 0..flat.text.len());
        assert_eq!(
            flat.runs.iter().map(|run| run.len).sum::<usize>(),
            flat.text.len()
        );
    }

    #[test]
    fn soft_wraps_never_split_a_slot_that_fits_its_row() {
        let (_native, text_system) = crate::typography::native_text_system();
        let flat = inline(&[
            ("The cost is ", false),
            (r"O(n^2) + \frac{a}{b}", true),
            (" per step, so ", false),
            (r"\sum_{i=1}^n i", true),
            (" is the total (", false),
            (r"x_1 + y_2", true),
            ("), with ", false),
            (r"\alpha", true),
            (", ", false),
            (r"\beta", true),
            (" and ", false),
            (r"\gamma \cdot \delta", true),
            (" glued to words like f", false),
            ("x", true),
            ("y around it.", false),
        ]);
        let font_size = px(14.0);
        let full = text_system
            .shape_text(flat.text.clone(), font_size, &flat.runs, None, None)
            .unwrap();
        let unwrapped = &full[0].unwrapped_layout;
        // The widest run of text a slot is glued into (no breakable space).
        let widest = flat
            .math
            .iter()
            .map(|slot| {
                let start = flat.text[..slot.range.start]
                    .rfind(' ')
                    .map_or(0, |at| at + 1);
                let end = flat.text[slot.range.end..]
                    .find(' ')
                    .map_or(flat.text.len(), |at| slot.range.end + at);
                unwrapped.x_for_index(end) - unwrapped.x_for_index(start)
            })
            .fold(px(0.0), Pixels::max);
        let mut width = f32::from(widest).ceil() + 1.0;
        let mut wraps = 0;
        while width < f32::from(unwrapped.width) {
            let lines = text_system
                .shape_text(
                    flat.text.clone(),
                    font_size,
                    &flat.runs,
                    Some(px(width)),
                    None,
                )
                .unwrap();
            let line = &lines[0];
            for boundary in &line.wrap_boundaries {
                wraps += 1;
                let at =
                    line.unwrapped_layout.runs[boundary.run_ix].glyphs[boundary.glyph_ix].index;
                for slot in &flat.math {
                    assert!(
                        at <= slot.range.start || at >= slot.range.end,
                        "a {width}px row splits the slot {:?} at {at}",
                        slot.range
                    );
                }
            }
            width += 0.5;
        }
        assert!(wraps > 100, "the sweep wraps around every slot ({wraps})");
    }

    #[test]
    fn bundled_math_faces_register_one_by_one_and_verify() {
        let (_native, text_system) = crate::typography::native_text_system();
        // Each face is its own `add_fonts` call, as first paints make them.
        for index in 0..zatex::bundled::FONTS {
            let font = zatex::bundled::font_at(index);
            let family = family_name(font.data()).unwrap();
            let id = register(&text_system, font);
            assert!(id.is_some(), "{family}");
            assert_eq!(id, Some(text_system.resolve_font(&gpui::font(family))));
        }
        // Another face's glyph numbering fails the check.
        let geist = text_system.resolve_font(&slot_font());
        assert!(!is_bundled_face(
            &text_system,
            geist,
            "Geist",
            zatex::bundled::font_at(0).data()
        ));
    }

    #[test]
    fn bundled_math_families_resolve_by_name() {
        let names: Vec<String> = (0..zatex::bundled::FONTS)
            .filter_map(|index| family_name(zatex::bundled::font_at(index).data()))
            .collect();
        assert_eq!(
            names,
            [
                "Fira Math",
                "STIX Two Math",
                "NewComputerModern Math",
                "Libertinus Math",
                "TeX Gyre Pagella Math",
                "Euler Math",
            ]
        );
    }
}
