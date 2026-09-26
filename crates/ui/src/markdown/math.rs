//! LaTeX math for Markdown surfaces.
//!
//! Agents write TeX between `$…$`, `$$…$$`, `\(…\)` and `\[…\]`. The parser
//! turns those spans into math runs ([`normalize_delimiters`] maps the two
//! bracket forms onto the dollar syntax pulldown-cmark understands), and this
//! module draws them: RaTeX, a pure-Rust port of KaTeX, parses and lays out
//! the TeX; the display list becomes a self-contained SVG whose glyph outlines
//! come from the bundled KaTeX fonts; GPUI paints that SVG as a monochrome
//! sprite tinted with the text color, so formulas follow the theme and the
//! sprite atlas caches each raster per size.
//!
//! Inline math stays in the text flow through a placeholder: the renderer
//! shapes a run of transparent letters as wide as the formula (letters, so the
//! line breaker keeps the run on one row) and paints the formula over it (see
//! [`Spacer`]). Wrapping, selection washes and link hit-testing keep working
//! on ordinary shaped text, and the original-text offset map turns a copied
//! placeholder back into its TeX source.

use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::ops::Range;
use std::sync::{Arc, LazyLock, Mutex, PoisonError};

use gpui::{
    App, Bounds, Font, Hsla, Pixels, Point, SharedString, TextRun, TransformationMatrix, Window,
    point, px, size,
};
use pulldown_cmark::{Event, Options, Parser, Tag};
use ratex_font::FontId;
use ratex_layout::{LayoutOptions, layout, to_display_list};
use ratex_types::display_item::{DisplayItem, DisplayList};
use ratex_types::math_style::MathStyle;
use ratex_types::path_command::PathCommand;
use ttf_parser::{Face, OutlineBuilder};

/// Math is set this much larger than the surrounding text, as KaTeX does
/// (`.katex { font-size: 1.21em }`): Computer Modern's x-height is small next
/// to UI faces, and at 1.21× it lines up with Geist's.
pub const MATH_SCALE: f32 = 1.21;

/// Formulas longer than this stay source text, which bounds layout work.
const MAX_TEX_BYTES: usize = 8 * 1024;
/// SVG bytes kept typeset (a formula is typically 2 to 60 KiB); the cache
/// starts over past this.
const CACHE_BYTES: usize = 16 * 1024 * 1024;
/// SVG user units per em.
const UNITS: f64 = 100.0;
/// Stroke width for unfilled display-list paths (em), as RaTeX's rasterizer.
const STROKE_EM: f64 = 0.0375;
/// Clear margin around the ink so antialiased edges are not clipped (em).
const INK_MARGIN_EM: f64 = 0.02;

// ---------------------------------------------------------------------------
// Delimiters
// ---------------------------------------------------------------------------

/// Rewrite paired `\(…\)` and `\[…\]` to `$$…$$` so pulldown-cmark's math
/// extension parses them. The rewrite keeps every byte offset: source ranges
/// the parser reports stay valid, and the original text at a math event's
/// range tells the forms apart again (`\(` stays inline). Code spans, code
/// blocks and raw HTML are left alone, and a delimiter only counts when its
/// partner is in reach: `\(` closes on the same line, `\[` before the next
/// blank line. An unpaired delimiter keeps its CommonMark meaning.
pub(crate) fn normalize_delimiters(source: &str) -> Cow<'_, str> {
    if !source.contains("\\(") && !source.contains("\\[") {
        return Cow::Borrowed(source);
    }
    let literal = literal_ranges(source);
    let bytes = source.as_bytes();
    let mut out: Option<Vec<u8>> = None;
    let mut i = 0;
    while i + 1 < bytes.len() {
        if let Some(end) = literal_end(&literal, i) {
            i = end;
            continue;
        }
        if bytes[i] != b'\\' {
            i += 1;
            continue;
        }
        let closer = match bytes[i + 1] {
            b'(' => b')',
            b'[' => b']',
            // Any other escape (`\\` included) is consumed whole, so `\\(` stays
            // a literal backslash before a parenthesis.
            _ => {
                i += 2;
                continue;
            }
        };
        match find_closer(bytes, &literal, i + 2, closer) {
            Some(close) => {
                let out = out.get_or_insert_with(|| bytes.to_vec());
                out[i..i + 2].copy_from_slice(b"$$");
                out[close..close + 2].copy_from_slice(b"$$");
                i = close + 2;
            }
            None => i += 2,
        }
    }
    match out {
        // Only ASCII bytes were swapped for ASCII bytes.
        Some(out) => Cow::Owned(String::from_utf8(out).expect("delimiter rewrite keeps UTF-8")),
        None => Cow::Borrowed(source),
    }
}

/// Byte ranges pulldown-cmark reads literally (code spans and blocks, raw
/// HTML), sorted and disjoint.
fn literal_ranges(source: &str) -> Vec<Range<usize>> {
    let options =
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let mut ranges: Vec<Range<usize>> = Vec::new();
    for (event, range) in Parser::new_ext(source, options).into_offset_iter() {
        let literal = matches!(
            event,
            Event::Start(Tag::CodeBlock(_) | Tag::HtmlBlock)
                | Event::Code(_)
                | Event::Html(_)
                | Event::InlineHtml(_)
        );
        // Events arrive in document order; content nested in a recorded block
        // (a code block's text, an HTML block's lines) is already covered.
        if literal && ranges.last().is_none_or(|last| range.start >= last.end) {
            ranges.push(range);
        }
    }
    ranges
}

/// End of the literal range covering `at`, if any.
fn literal_end(literal: &[Range<usize>], at: usize) -> Option<usize> {
    let ix = literal.partition_point(|range| range.end <= at);
    literal
        .get(ix)
        .filter(|range| range.start <= at)
        .map(|range| range.end)
}

/// Byte index of the `\` of the closing `\)`/`\]` for an opener whose body
/// starts at `from`.
fn find_closer(bytes: &[u8], literal: &[Range<usize>], from: usize, closer: u8) -> Option<usize> {
    let mut j = from;
    while j < bytes.len() {
        if literal_end(literal, j).is_some() {
            return None;
        }
        match bytes[j] {
            b'\n' if closer == b')' || next_line_blank(bytes, j + 1) => return None,
            b'\\' if j + 1 < bytes.len() => {
                if bytes[j + 1] == closer {
                    return Some(j);
                }
                // `\\` (a TeX line break) and other escapes pass as a pair.
                j += 2;
            }
            _ => j += 1,
        }
    }
    None
}

fn next_line_blank(bytes: &[u8], from: usize) -> bool {
    bytes[from.min(bytes.len())..]
        .iter()
        .take_while(|&&b| b != b'\n')
        .all(u8::is_ascii_whitespace)
}

// ---------------------------------------------------------------------------
// Typesetting
// ---------------------------------------------------------------------------

/// One typeset formula. Metrics are in em of the math size; the SVG spans the
/// ink box, which contains the layout box and anything drawn outside it
/// (italic overhangs, `\llap`, …).
pub struct MathRender {
    /// Sprite-atlas key, unique per formula and style.
    pub key: SharedString,
    pub svg: Arc<[u8]>,
    /// Advance width.
    pub width: f32,
    /// Extent above the baseline.
    pub height: f32,
    /// Extent below the baseline.
    pub depth: f32,
    /// Ink box relative to the layout box's top-left corner:
    /// `[left, top, right, bottom]` with `left, top <= 0`.
    pub ink: [f32; 4],
}

impl MathRender {
    pub fn total_height(&self) -> f32 {
        self.height + self.depth
    }

    /// Height of the ink box (em).
    pub fn ink_height(&self) -> f32 {
        self.ink[3] - self.ink[1]
    }
}

impl std::fmt::Debug for MathRender {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MathRender")
            .field("key", &self.key)
            .field("width", &self.width)
            .field("height", &self.height)
            .field("depth", &self.depth)
            .finish_non_exhaustive()
    }
}

/// Typeset `tex` (display or text style), memoized. `None` when the TeX does
/// not parse; callers show the source instead.
pub fn render(tex: &str, display: bool) -> Option<Arc<MathRender>> {
    #[derive(Default)]
    struct Cache {
        formulas: HashMap<(String, bool), Option<Arc<MathRender>>>,
        bytes: usize,
    }
    static CACHE: LazyLock<Mutex<Cache>> = LazyLock::new(Default::default);

    let tex = tex.trim();
    if tex.is_empty() || tex.len() > MAX_TEX_BYTES {
        return None;
    }
    let key = (tex.to_owned(), display);
    if let Some(hit) = CACHE
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .formulas
        .get(&key)
    {
        return hit.clone();
    }
    let typeset = typeset(tex, display).map(Arc::new);
    let size = key.0.len() + typeset.as_ref().map_or(0, |render| render.svg.len());
    let mut cache = CACHE.lock().unwrap_or_else(PoisonError::into_inner);
    if cache.bytes + size > CACHE_BYTES {
        *cache = Cache::default();
    }
    cache.bytes += size;
    cache.formulas.insert(key, typeset.clone());
    typeset
}

fn typeset(tex: &str, display: bool) -> Option<MathRender> {
    // RaTeX is young; a layout panic on odd input degrades to source text
    // instead of taking the window down (the app's panic hook still logs it).
    let list = std::panic::catch_unwind(|| {
        let nodes = ratex_parser::parser::parse(tex).ok()?;
        let style = if display {
            MathStyle::Display
        } else {
            MathStyle::Text
        };
        let options = LayoutOptions::default().with_style(style);
        Some(to_display_list(&layout(&nodes, &options)))
    })
    .ok()
    .flatten()?;
    if list.items.is_empty() && list.width <= 0.0 {
        return None;
    }
    let (svg, ink) = svg_document(&list);
    let mut hasher = DefaultHasher::new();
    (tex, display).hash(&mut hasher);
    Some(MathRender {
        key: format!("zeron-math/{:016x}", hasher.finish()).into(),
        svg: svg.into_bytes().into(),
        width: list.width as f32,
        height: list.height as f32,
        depth: list.depth as f32,
        ink: ink.map(|v| (v / UNITS) as f32),
    })
}

/// SVG for a display list (y down, baseline at `height`) and its ink box in
/// user units.
fn svg_document(list: &DisplayList) -> (String, [f64; 4]) {
    let mut body = String::new();
    let mut ink = Ink::new(list.width * UNITS, (list.height + list.depth) * UNITS);
    for item in &list.items {
        match item {
            DisplayItem::GlyphPath {
                x,
                y,
                scale,
                font,
                char_code,
                ..
            } => glyph(
                &mut body,
                &mut ink,
                x * UNITS,
                y * UNITS,
                scale * UNITS,
                font,
                *char_code,
            ),
            DisplayItem::Line {
                x,
                y,
                width,
                thickness,
                dashed,
                ..
            } => {
                let (x, y, width, thickness) =
                    (x * UNITS, y * UNITS, width * UNITS, thickness * UNITS);
                let top = y - thickness / 2.0;
                if *dashed {
                    let dash = (4.0 * thickness).max(2.0);
                    let mut at = x;
                    while at < x + width {
                        rect(
                            &mut body,
                            &mut ink,
                            at,
                            top,
                            dash.min(x + width - at),
                            thickness,
                        );
                        at += 2.0 * dash;
                    }
                } else {
                    rect(&mut body, &mut ink, x, top, width, thickness);
                }
            }
            DisplayItem::Rect {
                x,
                y,
                width,
                height,
                ..
            } => rect(
                &mut body,
                &mut ink,
                x * UNITS,
                y * UNITS,
                width * UNITS,
                height * UNITS,
            ),
            DisplayItem::Path {
                x,
                y,
                commands,
                fill,
                ..
            } => path(&mut body, &mut ink, x * UNITS, y * UNITS, commands, *fill),
        }
    }
    let margin = INK_MARGIN_EM * UNITS;
    let [left, top, right, bottom] = [
        ink.left - margin,
        ink.top - margin,
        ink.right + margin,
        ink.bottom + margin,
    ];
    let (w, h) = (right - left, bottom - top);
    let svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="{left:.1} {top:.1} {w:.1} {h:.1}" width="{w:.1}" height="{h:.1}">{body}</svg>"#
    );
    (svg, [left, top, right, bottom])
}

/// Bounding box of everything drawn, seeded with the layout box.
struct Ink {
    left: f64,
    top: f64,
    right: f64,
    bottom: f64,
}

impl Ink {
    fn new(width: f64, height: f64) -> Self {
        Self {
            left: 0.0,
            top: 0.0,
            right: width.max(0.0),
            bottom: height.max(0.0),
        }
    }

    fn add(&mut self, x0: f64, y0: f64, x1: f64, y1: f64) {
        if [x0, y0, x1, y1].iter().all(|v| v.is_finite()) {
            self.left = self.left.min(x0.min(x1));
            self.top = self.top.min(y0.min(y1));
            self.right = self.right.max(x0.max(x1));
            self.bottom = self.bottom.max(y0.max(y1));
        }
    }
}

fn rect(out: &mut String, ink: &mut Ink, x: f64, y: f64, width: f64, height: f64) {
    if width <= 0.0 || height <= 0.0 {
        return;
    }
    ink.add(x, y, x + width, y + height);
    let _ = write!(
        out,
        r#"<rect x="{x:.1}" y="{y:.1}" width="{width:.1}" height="{height:.1}"/>"#
    );
}

fn path(out: &mut String, ink: &mut Ink, x: f64, y: f64, commands: &[PathCommand], fill: bool) {
    let mut d = String::new();
    let mut point = |d: &mut String, cmd: char, coords: &[(f64, f64)]| {
        d.push(cmd);
        for (px, py) in coords {
            let (px, py) = (x + px * UNITS, y + py * UNITS);
            ink.add(px, py, px, py);
            let _ = write!(d, "{px:.1} {py:.1} ");
        }
    };
    let mut subpaths: Vec<String> = Vec::new();
    for command in commands {
        match *command {
            PathCommand::MoveTo { x: px, y: py } => {
                // Filled KaTeX stretchy pieces can wind in opposite directions;
                // separate elements keep them from cancelling out.
                if fill && !d.is_empty() {
                    subpaths.push(std::mem::take(&mut d));
                }
                point(&mut d, 'M', &[(px, py)]);
            }
            PathCommand::LineTo { x: px, y: py } => point(&mut d, 'L', &[(px, py)]),
            PathCommand::CubicTo {
                x1,
                y1,
                x2,
                y2,
                x: px,
                y: py,
            } => point(&mut d, 'C', &[(x1, y1), (x2, y2), (px, py)]),
            PathCommand::QuadTo {
                x1,
                y1,
                x: px,
                y: py,
            } => point(&mut d, 'Q', &[(x1, y1), (px, py)]),
            PathCommand::Close => d.push('Z'),
        }
    }
    if !d.is_empty() {
        subpaths.push(d);
    }
    for d in subpaths {
        if fill {
            let _ = write!(out, r#"<path d="{}"/>"#, d.trim_end());
        } else {
            let stroke = STROKE_EM * UNITS;
            let _ = write!(
                out,
                r#"<path d="{}" fill="none" stroke="black" stroke-width="{stroke:.2}"/>"#,
                d.trim_end()
            );
        }
    }
    if !fill {
        let half = STROKE_EM * UNITS / 2.0;
        ink.left -= half;
        ink.top -= half;
        ink.right += half;
        ink.bottom += half;
    }
}

const KATEX_FONTS: [(FontId, &[u8]); 19] = [
    (
        FontId::AmsRegular,
        include_bytes!("../../assets/fonts/katex/KaTeX_AMS-Regular.ttf"),
    ),
    (
        FontId::CaligraphicRegular,
        include_bytes!("../../assets/fonts/katex/KaTeX_Caligraphic-Regular.ttf"),
    ),
    (
        FontId::FrakturRegular,
        include_bytes!("../../assets/fonts/katex/KaTeX_Fraktur-Regular.ttf"),
    ),
    (
        FontId::FrakturBold,
        include_bytes!("../../assets/fonts/katex/KaTeX_Fraktur-Bold.ttf"),
    ),
    (
        FontId::MainBold,
        include_bytes!("../../assets/fonts/katex/KaTeX_Main-Bold.ttf"),
    ),
    (
        FontId::MainBoldItalic,
        include_bytes!("../../assets/fonts/katex/KaTeX_Main-BoldItalic.ttf"),
    ),
    (
        FontId::MainItalic,
        include_bytes!("../../assets/fonts/katex/KaTeX_Main-Italic.ttf"),
    ),
    (
        FontId::MainRegular,
        include_bytes!("../../assets/fonts/katex/KaTeX_Main-Regular.ttf"),
    ),
    (
        FontId::MathBoldItalic,
        include_bytes!("../../assets/fonts/katex/KaTeX_Math-BoldItalic.ttf"),
    ),
    (
        FontId::MathItalic,
        include_bytes!("../../assets/fonts/katex/KaTeX_Math-Italic.ttf"),
    ),
    (
        FontId::SansSerifBold,
        include_bytes!("../../assets/fonts/katex/KaTeX_SansSerif-Bold.ttf"),
    ),
    (
        FontId::SansSerifItalic,
        include_bytes!("../../assets/fonts/katex/KaTeX_SansSerif-Italic.ttf"),
    ),
    (
        FontId::SansSerifRegular,
        include_bytes!("../../assets/fonts/katex/KaTeX_SansSerif-Regular.ttf"),
    ),
    (
        FontId::ScriptRegular,
        include_bytes!("../../assets/fonts/katex/KaTeX_Script-Regular.ttf"),
    ),
    (
        FontId::Size1Regular,
        include_bytes!("../../assets/fonts/katex/KaTeX_Size1-Regular.ttf"),
    ),
    (
        FontId::Size2Regular,
        include_bytes!("../../assets/fonts/katex/KaTeX_Size2-Regular.ttf"),
    ),
    (
        FontId::Size3Regular,
        include_bytes!("../../assets/fonts/katex/KaTeX_Size3-Regular.ttf"),
    ),
    (
        FontId::Size4Regular,
        include_bytes!("../../assets/fonts/katex/KaTeX_Size4-Regular.ttf"),
    ),
    (
        FontId::TypewriterRegular,
        include_bytes!("../../assets/fonts/katex/KaTeX_Typewriter-Regular.ttf"),
    ),
];

fn katex_face(id: FontId) -> Option<&'static Face<'static>> {
    static FACES: LazyLock<Vec<(FontId, Face<'static>)>> = LazyLock::new(|| {
        KATEX_FONTS
            .iter()
            .filter_map(|(id, bytes)| Some((*id, Face::parse(bytes, 0).ok()?)))
            .collect()
    });
    FACES
        .iter()
        .find_map(|(face_id, face)| (*face_id == id).then_some(face))
}

/// Text outside KaTeX's coverage (`\text{…}` in other scripts) borrows the
/// bundled interface face.
fn fallback_face() -> Option<&'static Face<'static>> {
    static FACE: LazyLock<Option<Face<'static>>> = LazyLock::new(|| {
        crate::typography::bundled_font_faces()
            .next()
            .and_then(|bytes| Face::parse(bytes, 0).ok())
    });
    FACE.as_ref()
}

/// Emit one glyph outline, `size` user units per em, origin on the baseline.
fn glyph(out: &mut String, ink: &mut Ink, x: f64, y: f64, size: f64, font: &str, code: u32) {
    let id = FontId::parse(font).unwrap_or(FontId::MainRegular);
    let ch = ratex_font::katex_ttf_glyph_char(id, code);
    let faces = [
        katex_face(id),
        katex_face(FontId::MainRegular),
        fallback_face(),
    ];
    let Some((face, glyph)) = faces
        .into_iter()
        .flatten()
        .find_map(|face| Some((face, face.glyph_index(ch)?)))
    else {
        return;
    };
    let scale = size / f64::from(face.units_per_em());
    let mut sink = PathSink {
        d: String::new(),
        x,
        y,
        scale,
    };
    let Some(bbox) = face.outline_glyph(glyph, &mut sink) else {
        return;
    };
    ink.add(
        x + f64::from(bbox.x_min) * scale,
        y - f64::from(bbox.y_max) * scale,
        x + f64::from(bbox.x_max) * scale,
        y - f64::from(bbox.y_min) * scale,
    );
    let _ = write!(out, r#"<path d="{}"/>"#, sink.d.trim_end());
}

/// Glyph outline → SVG path data, flipping font units (y up) into the
/// display list's y-down space.
struct PathSink {
    d: String,
    x: f64,
    y: f64,
    scale: f64,
}

impl PathSink {
    fn point(&mut self, x: f32, y: f32) {
        let x = self.x + f64::from(x) * self.scale;
        let y = self.y - f64::from(y) * self.scale;
        let _ = write!(self.d, "{x:.1} {y:.1} ");
    }
}

impl OutlineBuilder for PathSink {
    fn move_to(&mut self, x: f32, y: f32) {
        self.d.push('M');
        self.point(x, y);
    }

    fn line_to(&mut self, x: f32, y: f32) {
        self.d.push('L');
        self.point(x, y);
    }

    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        self.d.push('Q');
        self.point(x1, y1);
        self.point(x, y);
    }

    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        self.d.push('C');
        self.point(x1, y1);
        self.point(x2, y2);
        self.point(x, y);
    }

    fn close(&mut self) {
        self.d.push('Z');
    }
}

// ---------------------------------------------------------------------------
// Painting
// ---------------------------------------------------------------------------

/// Paint `render` with its layout box's top-left corner at `origin`, `em`
/// pixels per math em, tinted like text.
pub fn paint(
    render: &MathRender,
    origin: Point<Pixels>,
    em: Pixels,
    color: Hsla,
    window: &mut Window,
    cx: &App,
) {
    let [left, top, right, bottom] = render.ink;
    let bounds = Bounds::new(
        point(origin.x + em * left, origin.y + em * top),
        size(em * (right - left), em * (bottom - top)),
    );
    if bounds.size.width <= px(0.0) || bounds.size.height <= px(0.0) {
        return;
    }
    let _ = window.paint_svg(
        bounds,
        render.key.clone(),
        Some(&render.svg),
        TransformationMatrix::unit(),
        color,
        cx,
    );
}

// ---------------------------------------------------------------------------
// Inline placeholders
// ---------------------------------------------------------------------------

/// Letter advances of the body face (em), from which inline-math placeholders
/// are assembled. The letters have straight outer stems, so they are rarely
/// kerned against each other, and GPUI's line breaker treats them as one word:
/// a placeholder never splits across rows.
pub struct Spacer {
    /// Distinct advances, widest first.
    letters: Vec<(char, f32)>,
}

const SPACER_LETTERS: [char; 16] = [
    'M', 'H', 'N', 'U', 'D', 'm', 'n', 'u', 'h', 'd', 'b', '0', 'I', 'l', 'i', '1',
];

/// Placeholder letters measured for `font`, memoized per family, weight and
/// style (shaped advances scale linearly with size).
pub fn spacer(font: &Font, window: &Window) -> Arc<Spacer> {
    /// Letters shaped per measurement, averaging out rounding.
    const SAMPLE: usize = 20;
    type Key = (SharedString, u32, bool);
    static SPACERS: LazyLock<Mutex<HashMap<Key, Arc<Spacer>>>> = LazyLock::new(Default::default);

    let key = (
        font.family.clone(),
        font.weight.0.to_bits(),
        font.style == gpui::FontStyle::Italic,
    );
    if let Some(hit) = SPACERS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&key)
    {
        return hit.clone();
    }
    // Shape runs of each letter rather than read the font's advances: the
    // text system tracks the interface face slightly tighter than its
    // advances, and a placeholder must be as wide as shaping makes it.
    let text_system = window.text_system();
    let measured = SPACER_LETTERS.map(|ch| {
        let text: String = std::iter::repeat_n(ch, SAMPLE).collect();
        let run = TextRun {
            len: text.len(),
            font: font.clone(),
            color: gpui::black(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let width = text_system
            .shape_line(text.into(), px(100.0), &[run], None)
            .width;
        (ch, f32::from(width) / (100.0 * SAMPLE as f32))
    });
    let spacer = Arc::new(Spacer::new(measured));
    SPACERS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(key, spacer.clone());
    spacer
}

impl Spacer {
    pub fn new(measured: impl IntoIterator<Item = (char, f32)>) -> Self {
        let mut letters: Vec<(char, f32)> = Vec::new();
        for (ch, advance) in measured {
            if advance > 0.0 && !letters.iter().any(|(_, a)| (a - advance).abs() < 1e-3) {
                letters.push((ch, advance));
            }
        }
        letters.sort_by(|a, b| b.1.total_cmp(&a.1));
        Self { letters }
    }

    /// Letters whose advances add up closest to `width` (em of the text
    /// size): runs of the widest letter plus up to three others. Never empty.
    pub fn fill(&self, width: f32) -> String {
        let Some(&(wide, wide_advance)) = self.letters.first() else {
            return "M".into();
        };
        let n = self.letters.len();
        let base = (width.max(0.0) / wide_advance).floor() as usize;
        let mut best = (f32::INFINITY, 0usize, [n; 3]);
        for count in [base.saturating_sub(1), base] {
            let rest = width - count as f32 * wide_advance;
            for a in 0..=n {
                for b in a..=n {
                    for c in b..=n {
                        let picks = [a, b, c];
                        let extra = picks.iter().filter(|&&k| k < n).count();
                        if count + extra == 0 {
                            continue;
                        }
                        let sum: f32 = picks
                            .iter()
                            .filter(|&&k| k < n)
                            .map(|&k| self.letters[k].1)
                            .sum();
                        let err = (rest - sum).abs();
                        if err + 1e-4 < best.0 {
                            best = (err, count, picks);
                        }
                    }
                }
            }
        }
        let mut out = String::with_capacity(best.1 + 3);
        out.extend(std::iter::repeat_n(wide, best.1));
        out.extend(
            best.2
                .iter()
                .filter(|&&k| k < n)
                .map(|&k| self.letters[k].0),
        );
        out
    }

    /// Total advance of `text` in em, for letters this spacer knows.
    #[cfg(test)]
    fn advance(&self, text: &str) -> f32 {
        text.chars()
            .filter_map(|ch| self.letters.iter().find(|(c, _)| *c == ch).map(|l| l.1))
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bracket_delimiters_become_dollars_without_moving_offsets() {
        let source = "Let \\(x^2\\) and\n\\[\n\\int_0^1 f\n\\]\nend";
        let normalized = normalize_delimiters(source);
        assert_eq!(normalized, "Let $$x^2$$ and\n$$\n\\int_0^1 f\n$$\nend");
        assert_eq!(normalized.len(), source.len());
    }

    #[test]
    fn code_escapes_and_unpaired_delimiters_stay_literal() {
        for source in [
            "`\\(x\\)` stays code",
            "```\n\\[x\\]\n```",
            "    \\(indented\\) code",
            "escaped \\\\(not math\\\\)",
            "open \\( never closes",
            "inline \\(does not\ncross lines\\)",
            "display \\[does not\n\ncross paragraphs\\]",
            "<div>\n\\(html block\\)\n</div>",
            "no delimiters at all",
        ] {
            assert_eq!(normalize_delimiters(source), source, "{source:?}");
        }
    }

    #[test]
    fn tex_line_breaks_do_not_close_display_math() {
        let source = "\\[\\begin{aligned} a &= 1 \\\\ b &= 2 \\end{aligned}\\]";
        let normalized = normalize_delimiters(source);
        assert!(normalized.starts_with("$$\\begin{aligned}"));
        assert!(normalized.ends_with("\\end{aligned}$$"));
        assert!(normalized.contains("\\\\ b"));
    }

    #[test]
    fn formulas_typeset_to_parseable_svg_with_positive_metrics() {
        for (tex, display) in [
            ("x^2", false),
            ("\\frac{a}{b}", false),
            ("\\sum_{i=1}^{n} x_i", true),
            ("\\begin{pmatrix} a & b \\\\ c & d \\end{pmatrix}", true),
            (
                "f(x) = \\begin{cases} 1 & x > 0 \\\\ 0 & \\text{otherwise} \\end{cases}",
                true,
            ),
            ("\\sqrt{\\pi} \\to \\infty", false),
            ("\\text{Grüße}", false),
        ] {
            let render = render(tex, display).unwrap_or_else(|| panic!("{tex} typesets"));
            assert!(render.width > 0.0, "{tex}");
            assert!(render.total_height() > 0.0, "{tex}");
            let [left, top, right, bottom] = render.ink;
            assert!(left <= 0.0 && top <= 0.0, "{tex}");
            assert!(
                right >= render.width && bottom >= render.total_height(),
                "{tex}"
            );
            let tree = usvg::Tree::from_data(&render.svg, &usvg::Options::default())
                .unwrap_or_else(|err| panic!("{tex}: {err}"));
            assert!(tree.root().has_children(), "{tex} draws something");
        }
    }

    #[test]
    fn display_style_is_taller_than_text_style() {
        let text = render("\\sum_{i=1}^n i", false).unwrap();
        let display = render("\\sum_{i=1}^n i", true).unwrap();
        assert!(display.total_height() > text.total_height());
        assert_ne!(text.key, display.key);
    }

    #[test]
    fn invalid_or_oversized_tex_is_rejected() {
        assert!(render("\\frac{a}{b", false).is_none());
        assert!(render("\\undefinedcommand", false).is_none());
        assert!(render("   ", false).is_none());
        assert!(render(&"x+".repeat(MAX_TEX_BYTES), false).is_none());
    }

    #[test]
    fn placeholders_match_the_requested_width() {
        let spacer = Spacer::new([
            ('M', 0.83),
            ('H', 0.72),
            ('m', 0.84),
            ('n', 0.55),
            ('0', 0.6),
            ('I', 0.27),
            ('l', 0.23),
            ('i', 0.22),
        ]);
        for width in [0.1, 0.57, 1.0, 2.34, 7.77, 25.0] {
            let placeholder = spacer.fill(width);
            assert!(!placeholder.is_empty());
            assert!(
                placeholder.chars().all(|c| c.is_ascii_alphanumeric()),
                "letters only, so the line breaker keeps the run whole"
            );
            let error = (spacer.advance(&placeholder) - width).abs();
            // Never further off than the narrowest letter.
            assert!(
                error <= 0.125,
                "{width}em -> {placeholder:?} off by {error}"
            );
        }
        assert_eq!(Spacer::new([]).fill(3.0), "M");
    }
}
