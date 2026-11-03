//! zatex — fast, low-memory LaTeX math layout for Zeron.
//!
//! [`layout`] turns a TeX math string into a [`Formula`]: a width, ascent and
//! descent plus a flat list of [`Item`]s (glyph ids and rules) in points.
//! Hosts paint it however they like — [`Formula::paint`] streams every glyph
//! outline and rule as paths, so a painter needs no font registration at all.
//!
//! # Any math font
//!
//! Layout reads every metric from the font's OpenType MATH table: script
//! scales, fraction/radical/limit gaps, the axis, italic corrections, accent
//! attachments, cut-in kerns, size variants and extensible assemblies. Load
//! any MATH font with [`MathFont::new`] (STIX Two Math, Latin Modern Math,
//! Libertinus Math, Fira Math, New Computer Modern Math, Cambria Math, …) and
//! the same source renders in that font's own design; nothing is tuned per font.
//!
//! # Scope
//!
//! The common LaTeX math set: letters, digits, Greek, ~300 symbols, `^ _ '`,
//! `\frac`/`\dfrac`/`\tfrac`/`\binom`/`\over`, `\sqrt[n]{}`, large operators
//! with limits, named operators and `\operatorname`, `\left \middle \right`,
//! `\big`…`\Bigg`, accents (incl. wide and arrow accents), `\overline`,
//! `\underline`, `\overbrace`, `\underbrace`, `\overset`, `\underset`,
//! `\stackrel`, `\xrightarrow`, math alphabets (`\mathbf \mathbb \mathcal
//! \mathfrak \mathsf \mathtt \boldsymbol`), `\text`, spacing commands, styles,
//! `\boxed`, `\not`, `\phantom`, and the `matrix`/`pmatrix`/…/`cases`/
//! `aligned`/`gathered`/`array` environments. No macros.
//!
//! Parsing is strict — an unknown command is an [`Error`] with a byte range —
//! so hosts can show the source for anything zatex can't render faithfully
//! (including half-streamed formulas).
//!
//! # Cost
//!
//! One pass to parse, one to lay out; no allocation per glyph beyond its
//! 20-byte [`Item`]. Glyph metrics are cached inside [`MathFont`]. Inputs are
//! bounded by [`Options`] (bytes, nesting depth, emitted items), so hostile
//! source can't run away with time or memory.

mod ast;
#[cfg(feature = "bundled")]
pub mod bundled;
mod font;
mod layout;
mod parse;
mod symbols;
mod unicode;

#[cfg(test)]
mod tests;

use std::ops::Range;

pub use font::{FontError, MathFont, PathSink};

/// Layout options.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Options {
    /// Font size in points (the em of text-style math).
    pub size: f32,
    /// Display style (`$$…$$`): big operators, limits above and below.
    /// Otherwise inline text style.
    pub display: bool,
    /// Longest accepted source, in bytes.
    pub max_bytes: usize,
    /// Deepest accepted nesting (groups, arguments, environments). Parsing
    /// and layout recurse once per level, and script chains (`x^{x^{…}}`)
    /// reach the full depth: at the default a release build needs about
    /// 192 KB of stack, a debug build about 1.5 MB (20 nested `\overbrace`s
    /// already fill 512 KB). Run layout on a thread with a few MB of stack —
    /// not a 512 KB secondary or dispatch-queue thread.
    pub max_depth: usize,
    /// Most items a formula may emit (bounds memory and paint time), and at
    /// most 64 plus 8 per source byte; layout work is bounded at a few times
    /// this.
    pub max_items: usize,
    /// Widest accepted formula, in ems. Anything wider (or any item farther
    /// out) is [`ErrorKind::TooLarge`], so hosts never size a slot, row or
    /// backing store from a runaway formula.
    pub max_width: f32,
    /// Tallest accepted formula (ascent plus descent), in ems.
    pub max_height: f32,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            size: 16.0,
            display: false,
            max_bytes: 16 * 1024,
            max_depth: 64,
            max_items: 20_000,
            max_width: 2000.0,
            max_height: 200.0,
        }
    }
}

impl Options {
    pub fn inline(size: f32) -> Self {
        Self {
            size,
            ..Self::default()
        }
    }

    pub fn display(size: f32) -> Self {
        Self {
            size,
            display: true,
            ..Self::default()
        }
    }
}

/// One positioned piece of a formula. Coordinates are points, y down, with
/// the origin on the formula's baseline at its left edge.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Item {
    /// Glyph `glyph` of font `font` in the [`MathFont::chain`] (0 is the
    /// font passed to [`layout`]), its origin at (`x`, `y`), drawn at `size`
    /// points.
    Glyph {
        font: u8,
        glyph: u16,
        x: f32,
        y: f32,
        size: f32,
    },
    /// A filled rectangle (fraction bars, radical overbars, …); (`x`, `y`)
    /// is its top-left corner.
    Rule {
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    },
}

impl Item {
    fn shifted(self, dx: f32, dy: f32) -> Self {
        match self {
            Item::Glyph {
                font,
                glyph,
                x,
                y,
                size,
            } => Item::Glyph {
                font,
                glyph,
                x: x + dx,
                y: y + dy,
                size,
            },
            Item::Rule {
                x,
                y,
                width,
                height,
            } => Item::Rule {
                x: x + dx,
                y: y + dy,
                width,
                height,
            },
        }
    }
}

/// A laid-out formula.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Formula {
    /// Advance width, points.
    pub width: f32,
    /// Extent above the baseline, points.
    pub ascent: f32,
    /// Extent below the baseline, points.
    pub descent: f32,
    /// Leftmost and rightmost extent of everything the formula paints,
    /// together with its advance box (so `left <= min(0, width)` and
    /// `right >= max(0, width)`), in points from the origin. Hosts reserve
    /// `right - left` and put the origin `-left` in from the slot's left
    /// edge, so neither negative space nor overhanging ink paints over the
    /// text beside a formula.
    pub left: f32,
    pub right: f32,
    pub items: Vec<Item>,
}

impl Formula {
    pub fn height(&self) -> f32 {
        self.ascent + self.descent
    }

    /// The width a host reserves: everything painted plus the advance box
    /// (see [`Formula::left`]).
    pub fn span(&self) -> f32 {
        self.right - self.left
    }

    /// Streams every glyph outline and rule as paths, the formula's baseline
    /// origin placed at (`x`, `y`). Fill with the non-zero rule.
    pub fn paint(&self, font: &MathFont, x: f32, y: f32, sink: &mut dyn PathSink) {
        for item in &self.items {
            match *item {
                Item::Glyph {
                    font: index,
                    glyph,
                    x: gx,
                    y: gy,
                    size,
                } => font.at(index).outline(glyph, size, x + gx, y + gy, sink),
                Item::Rule {
                    x: rx,
                    y: ry,
                    width,
                    height,
                } => {
                    let (l, t) = (x + rx, y + ry);
                    sink.move_to(l, t);
                    sink.line_to(l + width, t);
                    sink.line_to(l + width, t + height);
                    sink.line_to(l, t + height);
                    sink.close();
                }
            }
        }
    }

    /// A standalone SVG of the formula (for debugging and snapshots).
    pub fn to_svg(&self, font: &MathFont) -> String {
        struct Svg(String);
        impl PathSink for Svg {
            fn move_to(&mut self, x: f32, y: f32) {
                self.0.push_str(&format!("M{x:.2} {y:.2}"));
            }
            fn line_to(&mut self, x: f32, y: f32) {
                self.0.push_str(&format!("L{x:.2} {y:.2}"));
            }
            fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
                self.0.push_str(&format!("Q{x1:.2} {y1:.2} {x:.2} {y:.2}"));
            }
            fn cubic_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
                self.0
                    .push_str(&format!("C{x1:.2} {y1:.2} {x2:.2} {y2:.2} {x:.2} {y:.2}"));
            }
            fn close(&mut self) {
                self.0.push('Z');
            }
        }
        let mut path = Svg(String::new());
        self.paint(font, 0.0, self.ascent, &mut path);
        format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w:.2}\" height=\"{h:.2}\" viewBox=\"0 0 {w:.2} {h:.2}\"><path d=\"{d}\"/></svg>",
            w = self.width,
            h = self.height(),
            d = path.0
        )
    }
}

/// Why a formula couldn't be laid out.
#[derive(Debug, Clone, PartialEq)]
pub struct Error {
    pub kind: ErrorKind,
    /// Byte range in the source the error points at.
    pub range: Range<usize>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ErrorKind {
    UnknownCommand(String),
    UnknownEnvironment(String),
    Unexpected(String),
    Unsupported(String),
    MissingArgument,
    MissingBrace,
    MissingBracket,
    MissingRight,
    MissingEnd(String),
    MismatchedEnvironment {
        expected: String,
        found: String,
    },
    MissingDelimiter,
    BadDelimiter,
    BadDimension,
    DoubleScript,
    /// The font has no glyph for this character.
    MissingGlyph(char),
    TooLong,
    TooDeep,
    TooLarge,
    /// [`Options::size`] isn't a positive, finite number.
    BadSize,
    /// Layout failed unexpectedly (a bug): the host shows the source.
    Internal,
}

impl Error {
    pub(crate) fn new(kind: ErrorKind, range: Range<usize>) -> Self {
        Self { kind, range }
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        use ErrorKind::*;
        match &self.kind {
            UnknownCommand(name) => write!(f, "unknown command \\{name}"),
            UnknownEnvironment(name) => write!(f, "unknown environment {name}"),
            Unexpected(what) => write!(f, "unexpected {what}"),
            Unsupported(what) => write!(f, "unsupported: {what}"),
            MissingArgument => f.write_str("missing argument"),
            MissingBrace => f.write_str("missing }"),
            MissingBracket => f.write_str("missing ]"),
            MissingRight => f.write_str("\\left without \\right"),
            MissingEnd(name) => write!(f, "missing \\end{{{name}}}"),
            MismatchedEnvironment { expected, found } => {
                write!(f, "\\begin{{{expected}}} ended by \\end{{{found}}}")
            }
            MissingDelimiter => f.write_str("missing delimiter"),
            BadDelimiter => f.write_str("not a delimiter"),
            BadDimension => f.write_str("bad dimension"),
            DoubleScript => f.write_str("double superscript or subscript"),
            MissingGlyph(ch) => write!(f, "font has no glyph for {ch:?}"),
            TooLong => f.write_str("formula too long"),
            TooDeep => f.write_str("formula nested too deeply"),
            TooLarge => f.write_str("formula too large"),
            BadSize => f.write_str("font size must be positive and finite"),
            Internal => f.write_str("formula could not be laid out"),
        }?;
        write!(f, " at {}..{}", self.range.start, self.range.end)
    }
}

impl std::error::Error for Error {}

/// Where a missing glyph came from: the token at `start` when it spells the
/// character, else the character's first literal use (text inside
/// `\operatorname`), else the whole formula (a symbol a command produced).
fn glyph_range(source: &str, start: usize, ch: char) -> Range<usize> {
    let token = token_at(source, start);
    let spelled = source
        .get(token.clone())
        .is_some_and(|t| t.starts_with('\\') || t.contains(ch));
    if start > 0 && spelled {
        return token;
    }
    match source.find(ch) {
        Some(i) => i..i + ch.len_utf8(),
        None if start > 0 => token,
        None => 0..source.len(),
    }
}

/// The source token starting at `start`: a command or one character.
fn token_at(source: &str, start: usize) -> Range<usize> {
    let Some(rest) = source.get(start..) else {
        return start..start;
    };
    let mut chars = rest.chars();
    let len = match chars.next() {
        Some('\\') => {
            let name = &rest[1..];
            1 + match name.find(|c: char| !c.is_ascii_alphabetic()) {
                Some(0) => name.chars().next().map_or(0, char::len_utf8),
                Some(n) => n,
                None => name.len(),
            }
        }
        Some(c) => c.len_utf8(),
        None => 0,
    };
    start..start + len
}

/// Lays out TeX math `source` (without `$` delimiters) in `font`. Hosts run
/// this on agent output in their render paths, so a panic (a layout bug) is
/// an [`ErrorKind::Internal`] error rather than a crash.
pub fn layout(source: &str, font: &MathFont, options: &Options) -> Result<Formula, Error> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        lay_out(source, font, options)
    }))
    .unwrap_or_else(|_| Err(Error::new(ErrorKind::Internal, 0..source.len())))
}

fn lay_out(source: &str, font: &MathFont, options: &Options) -> Result<Formula, Error> {
    if source.len() > options.max_bytes {
        return Err(Error::new(ErrorKind::TooLong, 0..source.len()));
    }
    if !(options.size.is_finite() && options.size > 0.0) {
        return Err(Error::new(ErrorKind::BadSize, 0..0));
    }
    let nodes = parse::parse(source, options.max_depth)?;
    let style = if options.display {
        ast::Style::Display
    } else {
        ast::Style::Text
    };
    // Items are bounded by the source too: real TeX emits about one per
    // character plus a few per stretchy delimiter, while nested `\left(`
    // around a tall body multiplied 434 bytes into 18,000 items — paint and
    // memory every host pays per formula.
    let max_items = options.max_items.min(64 + 8 * source.len());
    let mut ctx = layout::Ctx::new(font, options.size, max_items);
    let b = ctx.list(&nodes, layout::St::new(style)).map_err(|mut e| {
        match e.kind {
            ErrorKind::TooLarge => e.range = 0..source.len(),
            ErrorKind::MissingGlyph(ch) => e.range = glyph_range(source, e.range.start, ch),
            _ => {}
        }
        e
    })?;
    let (w, h) = (
        options.max_width * options.size,
        options.max_height * options.size,
    );
    // A huge `size` (or bound) overflows the bounds to infinity, which every
    // extent would pass.
    if !(w.is_finite() && h.is_finite()) {
        return Err(Error::new(ErrorKind::TooLarge, 0..source.len()));
    }
    // Hosts size a formula's slot, line or row from its box, so the box must
    // hold all of its ink: anything painted outside would land on the
    // surrounding text (a negative row gap moves a whole row out of the box:
    // `\text{Approve}\\[-40em]\\`; negative space moves ink left of the
    // origin). Vertically the box grows to cover the ink; horizontally the
    // advance stays TeX's and `left`/`right` report the painted extent.
    let ink = ink_bounds(&b.items, font);
    let ascent = b.h.max(ink.top);
    let descent = b.d.max(ink.bottom).max(-ascent);
    let left = ink.left.min(b.w).min(0.0);
    let right = ink.right.max(b.w).max(0.0);
    // Room is bounded by the source too: real TeX sets well under an em or
    // two per byte (`\quad` is one em in five), while spacing alone
    // (`\hspace{100em}`, `\\[100em]`) would make a few bytes reserve a
    // page of blank slot, line or row in every host.
    let budget = (4.0 + 2.0 * source.len() as f32) * options.size;
    let fits = b.w.abs() <= w && right - left <= w.min(budget) && ascent + descent <= h.min(budget);
    if !fits {
        return Err(Error::new(ErrorKind::TooLarge, 0..source.len()));
    }
    let mut items = b.items;
    // Hosts keep formulas around (caches, prepared rows): drop growth slack.
    items.shrink_to_fit();
    Ok(Formula {
        width: b.w,
        ascent,
        descent,
        left,
        right,
        items,
    })
}

/// The extent of everything `items` paint, in points: `left`/`right` from
/// the origin, `top` above and `bottom` below the baseline (negative infinity
/// when nothing is painted, so an empty formula keeps its own box).
struct Ink {
    left: f32,
    right: f32,
    top: f32,
    bottom: f32,
}

fn ink_bounds(items: &[Item], font: &MathFont) -> Ink {
    let mut ink = Ink {
        left: 0.0,
        right: 0.0,
        top: f32::NEG_INFINITY,
        bottom: f32::NEG_INFINITY,
    };
    let mut add = |left: f32, right: f32, top: f32, bottom: f32| {
        ink.left = ink.left.min(left);
        ink.right = ink.right.max(right);
        ink.top = ink.top.max(top);
        ink.bottom = ink.bottom.max(bottom);
    };
    for item in items {
        match *item {
            Item::Glyph {
                font: index,
                glyph,
                x,
                y,
                size,
            } => {
                let face = font.at(index);
                let m = face.metrics(glyph);
                let scale = size / face.upem();
                add(
                    x + m.x_min * scale,
                    x + m.x_max * scale,
                    m.height * scale - y,
                    m.depth * scale + y,
                );
            }
            Item::Rule {
                x,
                y,
                width,
                height,
            } => add(x.min(x + width), x.max(x + width), -y, y + height),
        }
    }
    ink
}
