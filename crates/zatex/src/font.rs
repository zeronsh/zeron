//! A math font: glyph metrics plus the OpenType MATH table.
//!
//! Every number layout uses comes from here, so swapping the font swaps the
//! whole look — constants, italic corrections, accent attachments, cut-in
//! kerns, size variants and extensible assemblies. Nothing is tuned per font.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use ttf_parser::gsub::SubstitutionSubtable;
use ttf_parser::math::{GlyphConstruction, Table};
use ttf_parser::opentype_layout::Lookup;
use ttf_parser::{Face, GlyphId, OutlineBuilder, Tag};

/// Why a font can't be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FontError {
    /// Not a parseable OpenType/TrueType font.
    Parse,
    /// The font has no MATH table (or no MATH constants), so it can't lay out math.
    NoMathTable,
}

impl std::fmt::Display for FontError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            FontError::Parse => "not a valid font file",
            FontError::NoMathTable => "font has no OpenType MATH table",
        })
    }
}

impl std::error::Error for FontError {}

/// The MATH constants layout reads, in font units (percentages as fractions).
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Constants {
    pub script_scale: f32,
    pub script_script_scale: f32,
    pub display_operator_min_height: f32,
    pub axis_height: f32,
    pub accent_base_height: f32,
    pub subscript_shift_down: f32,
    pub subscript_top_max: f32,
    pub subscript_baseline_drop_min: f32,
    pub superscript_shift_up: f32,
    pub superscript_shift_up_cramped: f32,
    pub superscript_bottom_min: f32,
    pub superscript_baseline_drop_max: f32,
    pub sub_superscript_gap_min: f32,
    pub superscript_bottom_max_with_subscript: f32,
    pub space_after_script: f32,
    pub upper_limit_gap_min: f32,
    pub upper_limit_baseline_rise_min: f32,
    pub lower_limit_gap_min: f32,
    pub lower_limit_baseline_drop_min: f32,
    pub stack_top_shift_up: f32,
    pub stack_top_display_style_shift_up: f32,
    pub stack_bottom_shift_down: f32,
    pub stack_bottom_display_style_shift_down: f32,
    pub stack_gap_min: f32,
    pub stack_display_style_gap_min: f32,
    pub stretch_stack_gap_above_min: f32,
    pub stretch_stack_gap_below_min: f32,
    pub fraction_numerator_shift_up: f32,
    pub fraction_numerator_display_style_shift_up: f32,
    pub fraction_denominator_shift_down: f32,
    pub fraction_denominator_display_style_shift_down: f32,
    pub fraction_numerator_gap_min: f32,
    pub fraction_num_display_style_gap_min: f32,
    pub fraction_rule_thickness: f32,
    pub fraction_denominator_gap_min: f32,
    pub fraction_denom_display_style_gap_min: f32,
    pub overbar_vertical_gap: f32,
    pub overbar_rule_thickness: f32,
    pub overbar_extra_ascender: f32,
    pub underbar_vertical_gap: f32,
    pub underbar_rule_thickness: f32,
    pub underbar_extra_descender: f32,
    pub radical_vertical_gap: f32,
    pub radical_display_style_vertical_gap: f32,
    pub radical_rule_thickness: f32,
    pub radical_extra_ascender: f32,
    pub radical_kern_before_degree: f32,
    pub radical_kern_after_degree: f32,
    pub radical_degree_bottom_raise: f32,
    pub min_connector_overlap: f32,
}

/// A glyph's advance and ink box, in font units (y up).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct Metrics {
    pub advance: f32,
    pub x_min: f32,
    pub x_max: f32,
    /// Ink above the baseline (`y_max`).
    pub height: f32,
    /// Ink below the baseline (`-y_min`).
    pub depth: f32,
}

/// A cut-in kern corner (OpenType `MathKernInfo`).
#[derive(Debug, Clone, Copy)]
pub(crate) enum Corner {
    TopRight,
    BottomRight,
}

/// One glyph of a built-up (assembled) stretchy glyph.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Part {
    pub glyph: u16,
    /// Offset of the part's start (bottom for vertical, left for horizontal)
    /// along the stretch axis, in font units.
    pub offset: f32,
}

/// A glyph grown to a target size: a single (variant) glyph or an assembly.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Stretched {
    Glyph(u16),
    Assembly { parts: Vec<Part>, length: f32 },
}

fn percent(value: i16, default: f32) -> f32 {
    if (1..=100).contains(&value) {
        value as f32 / 100.0
    } else {
        default
    }
}

/// Hard ceiling on parts in one assembly (a hostile `\left(` 10 km tall).
const MAX_ASSEMBLY_PARTS: usize = 512;

/// Longest fallback chain followed (glyphs record their font as a `u8`).
const MAX_CHAIN: usize = 8;

/// A loaded math font, optionally with a fallback chain for characters it
/// lacks. Cheap to share (`Send + Sync`); metric lookups are cached per glyph
/// so CFF fonts aren't re-outlined for every bounding box.
pub struct MathFont {
    data: &'static [u8],
    face: Face<'static>,
    math: Table<'static>,
    pub(crate) c: Constants,
    upem: f32,
    space: f32,
    metrics: Mutex<HashMap<u16, Metrics>>,
    /// GSUB lookups of the `ssty` feature: glyphs redrawn for script sizes.
    ssty: Vec<Lookup<'static>>,
    fallback: Option<Arc<MathFont>>,
}

impl std::fmt::Debug for MathFont {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MathFont")
            .field("name", &self.name())
            .field("units_per_em", &self.upem)
            .finish()
    }
}

impl MathFont {
    /// Loads a math font from bytes that live for the whole process (an
    /// `include_bytes!` or a mapped file). Fails if the font has no MATH table.
    pub fn new(data: &'static [u8]) -> Result<Self, FontError> {
        let face = Face::parse(data, 0).map_err(|_| FontError::Parse)?;
        let math = face.tables().math.ok_or(FontError::NoMathTable)?;
        let k = math.constants.ok_or(FontError::NoMathTable)?;
        let v = |m: ttf_parser::math::MathValue| m.value as f32;
        let c = Constants {
            // A zero or nonsensical scale would divide by zero; TeX's own defaults stand in.
            script_scale: percent(k.script_percent_scale_down(), 0.7),
            script_script_scale: percent(k.script_script_percent_scale_down(), 0.5),
            display_operator_min_height: k.display_operator_min_height() as f32,
            axis_height: v(k.axis_height()),
            accent_base_height: v(k.accent_base_height()),
            subscript_shift_down: v(k.subscript_shift_down()),
            subscript_top_max: v(k.subscript_top_max()),
            subscript_baseline_drop_min: v(k.subscript_baseline_drop_min()),
            superscript_shift_up: v(k.superscript_shift_up()),
            superscript_shift_up_cramped: v(k.superscript_shift_up_cramped()),
            superscript_bottom_min: v(k.superscript_bottom_min()),
            superscript_baseline_drop_max: v(k.superscript_baseline_drop_max()),
            sub_superscript_gap_min: v(k.sub_superscript_gap_min()),
            superscript_bottom_max_with_subscript: v(k.superscript_bottom_max_with_subscript()),
            space_after_script: v(k.space_after_script()),
            upper_limit_gap_min: v(k.upper_limit_gap_min()),
            upper_limit_baseline_rise_min: v(k.upper_limit_baseline_rise_min()),
            lower_limit_gap_min: v(k.lower_limit_gap_min()),
            lower_limit_baseline_drop_min: v(k.lower_limit_baseline_drop_min()),
            stack_top_shift_up: v(k.stack_top_shift_up()),
            stack_top_display_style_shift_up: v(k.stack_top_display_style_shift_up()),
            stack_bottom_shift_down: v(k.stack_bottom_shift_down()),
            stack_bottom_display_style_shift_down: v(k.stack_bottom_display_style_shift_down()),
            stack_gap_min: v(k.stack_gap_min()),
            stack_display_style_gap_min: v(k.stack_display_style_gap_min()),
            stretch_stack_gap_above_min: v(k.stretch_stack_gap_above_min()),
            stretch_stack_gap_below_min: v(k.stretch_stack_gap_below_min()),
            fraction_numerator_shift_up: v(k.fraction_numerator_shift_up()),
            fraction_numerator_display_style_shift_up: v(
                k.fraction_numerator_display_style_shift_up()
            ),
            fraction_denominator_shift_down: v(k.fraction_denominator_shift_down()),
            fraction_denominator_display_style_shift_down: v(
                k.fraction_denominator_display_style_shift_down()
            ),
            fraction_numerator_gap_min: v(k.fraction_numerator_gap_min()),
            fraction_num_display_style_gap_min: v(k.fraction_num_display_style_gap_min()),
            fraction_rule_thickness: v(k.fraction_rule_thickness()),
            fraction_denominator_gap_min: v(k.fraction_denominator_gap_min()),
            fraction_denom_display_style_gap_min: v(k.fraction_denom_display_style_gap_min()),
            overbar_vertical_gap: v(k.overbar_vertical_gap()),
            overbar_rule_thickness: v(k.overbar_rule_thickness()),
            overbar_extra_ascender: v(k.overbar_extra_ascender()),
            underbar_vertical_gap: v(k.underbar_vertical_gap()),
            underbar_rule_thickness: v(k.underbar_rule_thickness()),
            underbar_extra_descender: v(k.underbar_extra_descender()),
            radical_vertical_gap: v(k.radical_vertical_gap()),
            radical_display_style_vertical_gap: v(k.radical_display_style_vertical_gap()),
            radical_rule_thickness: v(k.radical_rule_thickness()),
            radical_extra_ascender: v(k.radical_extra_ascender()),
            radical_kern_before_degree: v(k.radical_kern_before_degree()),
            radical_kern_after_degree: v(k.radical_kern_after_degree()),
            radical_degree_bottom_raise: k.radical_degree_bottom_raise_percent() as f32 / 100.0,
            min_connector_overlap: math
                .variants
                .map(|v| v.min_connector_overlap as f32)
                .unwrap_or(0.0),
        };
        let upem = face.units_per_em() as f32;
        let space = face
            .glyph_index(' ')
            .and_then(|g| face.glyph_hor_advance(g))
            .map(|a| a as f32)
            .unwrap_or(upem / 4.0);
        let ssty = face
            .tables()
            .gsub
            .map(|gsub| {
                let mut indices: Vec<u16> = gsub
                    .features
                    .into_iter()
                    .filter(|f| f.tag == Tag::from_bytes(b"ssty"))
                    .flat_map(|f| f.lookup_indices)
                    .collect();
                indices.sort_unstable();
                indices.dedup();
                indices
                    .into_iter()
                    .filter_map(|i| gsub.lookups.get(i))
                    .collect()
            })
            .unwrap_or_default();
        Ok(Self {
            data,
            face,
            math,
            c,
            upem,
            space,
            metrics: Mutex::new(HashMap::new()),
            ssty,
            fallback: None,
        })
    }

    /// Uses `fallback` (and its own fallbacks) for characters this font lacks
    /// — e.g. a sans font that has no script or Fraktur alphabet. Layout
    /// constants still come from this font, so it keeps deciding the look.
    pub fn with_fallback(mut self, fallback: Arc<MathFont>) -> Self {
        self.fallback = Some(fallback);
        self
    }

    /// This font then its fallbacks, in lookup order.
    pub fn chain(&self) -> impl Iterator<Item = &MathFont> {
        std::iter::successors(Some(self), |f| f.fallback.as_deref()).take(MAX_CHAIN)
    }

    /// The font at `index` in [`MathFont::chain`] (an [`crate::Item::Glyph`]'s `font`).
    pub fn at(&self, index: u8) -> &MathFont {
        self.chain().nth(index as usize).unwrap_or(self)
    }

    /// Loads a math font from owned bytes. The bytes are leaked so the font can
    /// borrow them for the rest of the process — load each font once and share it.
    pub fn from_vec(data: Vec<u8>) -> Result<Self, FontError> {
        // Validate first so a bad file isn't leaked.
        let face = Face::parse(&data, 0).map_err(|_| FontError::Parse)?;
        if face.tables().math.and_then(|m| m.constants).is_none() {
            return Err(FontError::NoMathTable);
        }
        Self::new(Box::leak(data.into_boxed_slice()))
    }

    /// The font file's bytes, for painters that rasterize glyph ids natively
    /// (CoreText, a GPU glyph atlas) rather than filling [`MathFont::outline`]s.
    pub fn data(&self) -> &'static [u8] {
        self.data
    }

    /// The font's full name, e.g. "STIX Two Math".
    pub fn name(&self) -> Option<String> {
        let names = self.face.names();
        [ttf_parser::name_id::FULL_NAME, ttf_parser::name_id::FAMILY]
            .into_iter()
            .find_map(|id| {
                names
                    .into_iter()
                    .filter(|n| n.name_id == id && n.is_unicode())
                    .find_map(|n| n.to_string())
            })
    }

    pub fn units_per_em(&self) -> u16 {
        self.upem as u16
    }

    pub(crate) fn upem(&self) -> f32 {
        self.upem
    }

    /// Advance of the font's space, in font units.
    pub(crate) fn space(&self) -> f32 {
        self.space
    }

    pub fn has_char(&self, ch: char) -> bool {
        self.glyph(ch).is_some()
    }

    pub(crate) fn glyph(&self, ch: char) -> Option<u16> {
        self.face.glyph_index(ch).map(|g| g.0).filter(|&g| g != 0)
    }

    pub(crate) fn metrics(&self, glyph: u16) -> Metrics {
        let mut cache = self.metrics.lock().unwrap_or_else(|e| e.into_inner());
        *cache.entry(glyph).or_insert_with(|| {
            let id = GlyphId(glyph);
            let advance = self.face.glyph_hor_advance(id).unwrap_or(0) as f32;
            match self.face.glyph_bounding_box(id) {
                Some(r) => Metrics {
                    advance,
                    x_min: r.x_min as f32,
                    x_max: r.x_max as f32,
                    height: r.y_max as f32,
                    depth: -(r.y_min as f32),
                },
                None => Metrics {
                    advance,
                    ..Metrics::default()
                },
            }
        })
    }

    /// The font's script-size form of `glyph` (`ssty`): `level` 1 for script,
    /// 2 for scriptscript. Optical variants with sturdier strokes, and the
    /// prime drawn for superscript use.
    pub(crate) fn script_glyph(&self, glyph: u16, level: u8) -> u16 {
        let id = GlyphId(glyph);
        for lookup in &self.ssty {
            for i in 0..lookup.subtables.len() {
                let found = match lookup.subtables.get::<SubstitutionSubtable>(i) {
                    Some(SubstitutionSubtable::Single(single)) => {
                        use ttf_parser::gsub::SingleSubstitution::*;
                        match single {
                            Format1 { coverage, delta } => coverage
                                .get(id)
                                .map(|_| (glyph as i32 + delta as i32) as u16),
                            Format2 {
                                coverage,
                                substitutes,
                            } => coverage
                                .get(id)
                                .and_then(|c| substitutes.get(c))
                                .map(|g| g.0),
                        }
                    }
                    Some(SubstitutionSubtable::Alternate(alt)) => alt
                        .coverage
                        .get(id)
                        .and_then(|c| alt.alternate_sets.get(c))
                        .and_then(|set| {
                            let n = set.alternates.len();
                            (n > 0).then(|| set.alternates.get((level as u16 - 1).min(n - 1)))?
                        })
                        .map(|g| g.0),
                    _ => None,
                };
                if let Some(g) = found {
                    return g;
                }
            }
        }
        glyph
    }

    pub(crate) fn italic_correction(&self, glyph: u16) -> f32 {
        self.math
            .glyph_info
            .and_then(|i| i.italic_corrections)
            .and_then(|v| v.get(GlyphId(glyph)))
            .map(|v| v.value as f32)
            .unwrap_or(0.0)
    }

    /// Horizontal position accents attach to, if the font says.
    pub(crate) fn top_accent(&self, glyph: u16) -> Option<f32> {
        self.math
            .glyph_info
            .and_then(|i| i.top_accent_attachments)
            .and_then(|v| v.get(GlyphId(glyph)))
            .map(|v| v.value as f32)
    }

    /// The cut-in kern at `corner` for a correction height (font units).
    pub(crate) fn kern(&self, glyph: u16, corner: Corner, height: f32) -> f32 {
        let Some(info) = self
            .math
            .glyph_info
            .and_then(|i| i.kern_infos)
            .and_then(|k| k.get(GlyphId(glyph)))
        else {
            return 0.0;
        };
        let Some(kern) = (match corner {
            Corner::TopRight => info.top_right,
            Corner::BottomRight => info.bottom_right,
        }) else {
            return 0.0;
        };
        let n = kern.count();
        let mut i = 0;
        while i < n {
            match kern.height(i) {
                Some(h) if height < h.value as f32 => break,
                Some(_) => i += 1,
                None => break,
            }
        }
        kern.kern(i).map(|v| v.value as f32).unwrap_or(0.0)
    }

    fn construction(&self, glyph: u16, vertical: bool) -> Option<GlyphConstruction<'static>> {
        let v = self.math.variants?;
        let set = if vertical {
            v.vertical_constructions
        } else {
            v.horizontal_constructions
        };
        set.get(GlyphId(glyph))
    }

    /// Ink extent along the stretch axis, in font units.
    fn extent(&self, glyph: u16, vertical: bool) -> f32 {
        let m = self.metrics(glyph);
        if vertical {
            m.height + m.depth
        } else {
            m.advance
        }
    }

    /// The first size variant whose extent reaches `min` (or the base glyph if
    /// it already does). `None` when nothing reaches it.
    pub(crate) fn variant_at_least(&self, glyph: u16, min: f32, vertical: bool) -> Option<u16> {
        if self.extent(glyph, vertical) >= min {
            return Some(glyph);
        }
        let c = self.construction(glyph, vertical)?;
        c.variants
            .into_iter()
            .find(|v| v.advance_measurement as f32 >= min)
            .map(|v| v.variant_glyph.0)
    }

    /// The largest size variant (the base glyph if there are none).
    pub(crate) fn largest_variant(&self, glyph: u16, vertical: bool) -> u16 {
        self.construction(glyph, vertical)
            .and_then(|c| c.variants.into_iter().last())
            .map(|v| v.variant_glyph.0)
            .unwrap_or(glyph)
    }

    /// Grows `glyph` along an axis to cover `target` font units: the smallest
    /// variant that does, else an extensible assembly, else the largest variant.
    pub(crate) fn stretch(&self, glyph: u16, target: f32, vertical: bool) -> Stretched {
        if let Some(g) = self.variant_at_least(glyph, target, vertical) {
            return Stretched::Glyph(g);
        }
        let Some(assembly) = self.construction(glyph, vertical).and_then(|c| c.assembly) else {
            return Stretched::Glyph(self.largest_variant(glyph, vertical));
        };
        let parts: Vec<_> = assembly.parts.into_iter().collect();
        if parts.is_empty() {
            return Stretched::Glyph(self.largest_variant(glyph, vertical));
        }
        let o_min = self.c.min_connector_overlap;
        let (mut fixed, mut ext, mut n_fixed, mut n_ext) = (0.0f32, 0.0f32, 0usize, 0usize);
        for p in &parts {
            if p.part_flags.extender() {
                ext += p.full_advance as f32;
                n_ext += 1;
            } else {
                fixed += p.full_advance as f32;
                n_fixed += 1;
            }
        }
        // Fewest extender repetitions whose longest assembly (minimal overlap)
        // reaches the target.
        let longest = |r: usize| {
            let n = n_fixed + r * n_ext;
            fixed + r as f32 * ext - n.saturating_sub(1) as f32 * o_min
        };
        let mut reps = 0usize;
        if n_ext > 0 {
            let step = ext - n_ext as f32 * o_min;
            if step > 0.0 && longest(0) < target {
                reps = ((target - longest(0)) / step).ceil().max(0.0) as usize;
            }
            reps = reps.max(usize::from(n_fixed == 0));
            let cap = MAX_ASSEMBLY_PARTS.saturating_sub(n_fixed) / n_ext.max(1);
            reps = reps.min(cap.max(1));
        }
        let mut seq = Vec::with_capacity(n_fixed + reps * n_ext);
        for p in &parts {
            let count = if p.part_flags.extender() { reps } else { 1 };
            for _ in 0..count {
                seq.push(*p);
            }
        }
        if seq.is_empty() {
            return Stretched::Glyph(self.largest_variant(glyph, vertical));
        }
        // Shared overlap: as large as the connectors allow while still
        // reaching the target, but never under the font's minimum.
        let o_max = seq
            .windows(2)
            .map(|w| w[0].end_connector_length.min(w[1].start_connector_length) as f32)
            .fold(f32::INFINITY, f32::min);
        let gaps = seq.len().saturating_sub(1) as f32;
        let total: f32 = seq.iter().map(|p| p.full_advance as f32).sum();
        let overlap = if gaps > 0.0 {
            let o_max = if o_max.is_finite() {
                o_max.max(o_min)
            } else {
                o_min
            };
            ((total - target) / gaps).clamp(o_min, o_max)
        } else {
            0.0
        };
        let mut offset = 0.0;
        let mut out = Vec::with_capacity(seq.len());
        for p in &seq {
            out.push(Part {
                glyph: p.glyph_id.0,
                offset,
            });
            offset += p.full_advance as f32 - overlap;
        }
        let length = total - gaps * overlap;
        Stretched::Assembly { parts: out, length }
    }

    /// Streams `glyph`'s outline, scaled to `size` points with its origin at
    /// (`x`, `y`) in a y-down space (the display list's).
    pub fn outline(&self, glyph: u16, size: f32, x: f32, y: f32, sink: &mut dyn PathSink) {
        let s = size / self.upem;
        let mut t = Transform { sink, s, x, y };
        self.face.outline_glyph(GlyphId(glyph), &mut t);
    }
}

/// Receives glyph outlines and rules as paths (y down).
pub trait PathSink {
    fn move_to(&mut self, x: f32, y: f32);
    fn line_to(&mut self, x: f32, y: f32);
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32);
    fn cubic_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32);
    fn close(&mut self);
}

struct Transform<'a> {
    sink: &'a mut dyn PathSink,
    s: f32,
    x: f32,
    y: f32,
}

impl Transform<'_> {
    fn p(&self, x: f32, y: f32) -> (f32, f32) {
        (self.x + x * self.s, self.y - y * self.s)
    }
}

impl OutlineBuilder for Transform<'_> {
    fn move_to(&mut self, x: f32, y: f32) {
        let (x, y) = self.p(x, y);
        self.sink.move_to(x, y);
    }
    fn line_to(&mut self, x: f32, y: f32) {
        let (x, y) = self.p(x, y);
        self.sink.line_to(x, y);
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let (x1, y1) = self.p(x1, y1);
        let (x, y) = self.p(x, y);
        self.sink.quad_to(x1, y1, x, y);
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let (x1, y1) = self.p(x1, y1);
        let (x2, y2) = self.p(x2, y2);
        let (x, y) = self.p(x, y);
        self.sink.cubic_to(x1, y1, x2, y2, x, y);
    }
    fn close(&mut self) {
        self.sink.close();
    }
}
