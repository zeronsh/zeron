//! What a row of the screen shows, as zgx draws it: the backgrounds,
//! the runs of text and the lines (strikes, the cursor), worked out from
//! the screen model only for rows that changed.
use crate::{Palette, Term, dim};
use alacritty_terminal::{
    grid::Dimensions,
    index::{Column, Line, Point},
    selection::SelectionRange,
    term::cell::{Cell, Flags},
    vte::ansi::{Color as VtColor, CursorShape, NamedColor, Rgb},
};
use std::sync::Arc;
use zgx::{
    canvas::{Canvas, CanvasText, LineCap, LineJoin, Path, Stroke, TextAnchor, TextBaseline},
    scene::{Color, Rect},
    text_layout::{FontFamily, FontFeatures, FontStyle, TextUnderline},
};

/// A cell's size and the font, in points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Metrics {
    pub font_size: f32,
    pub cell_width: f32,
    pub cell_height: f32,
}

/// How the terminal colours what programs leave to it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Look {
    pub palette: Palette,
    /// Behind selected cells.
    pub selection: Color,
    /// Selected text; `None` keeps each cell's own colour.
    pub selection_text: Option<Color>,
    /// Text under a block cursor; `None` is the background.
    pub cursor_text: Option<Color>,
}
impl Default for Look {
    fn default() -> Self {
        Self {
            palette: Palette::default(),
            selection: Color(0x4a, 0x78, 0xc0, 0x88),
            selection_text: None,
            cursor_text: None,
        }
    }
}
impl Look {
    /// The default look with the theme's `colors` over it, each `None`
    /// keeping the default.
    pub fn themed(colors: &zgx::terminal::TerminalColors) -> Self {
        let mut look = Self::default();
        let rgb = |color: Color| Rgb {
            r: color.0,
            g: color.1,
            b: color.2,
        };
        let palette = &mut look.palette;
        for (slot, color) in palette.ansi.iter_mut().zip(colors.ansi) {
            if let Some(color) = color {
                *slot = rgb(color);
            }
        }
        if let Some(color) = colors.foreground {
            palette.foreground = rgb(color);
        }
        if let Some(color) = colors.background {
            palette.background = rgb(color);
        }
        if let Some(color) = colors.cursor {
            palette.cursor = rgb(color);
        }
        if let Some(color) = colors.selection_background {
            look.selection = color;
        }
        look.selection_text = colors.selection_foreground;
        look.cursor_text = colors.cursor_text;
        look
    }
}

/// The cursor, where a row shows it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct CursorAt {
    pub column: usize,
    pub shape: CursorShape,
    /// The window has focus here: an unfocused cursor is a hollow block.
    pub focused: bool,
}

/// Cells from `column`, `columns` wide, filled with `color`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Fill {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub color: Color,
}

/// Text in one style from `column`.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Run {
    pub column: u16,
    pub text: Arc<str>,
    pub color: Color,
    pub bold: bool,
    pub italic: bool,
    /// Its cells' underline (`Flags::ALL_UNDERLINES`, none or one): the
    /// text's own where it is plain (`Flags::UNDERLINE`), and none where it
    /// is in a style or colour of its own (drawn as shapes, `UnderlineStyle`)
    /// once the row is built with them (`row_styled`).
    pub underline: Flags,
    /// Placed alone, centred on its cells: a wide character, or one whose
    /// glyph may come from another font than the grid's.
    pub alone: bool,
    /// How many cells it takes (alone).
    pub cells: u8,
}

/// How an underline is drawn where it isn't the text's own (`Run::underline`):
/// a style a program asked for (SGR 4:2 to 4:5), or any underline in a
/// colour of its own (SGR 58). It goes in a row's `lines` as a `Fill` whose
/// height is minus its code (`UnderlineStyle::of`), so a row without one is
/// the same as ever, its paint no bigger.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UnderlineStyle {
    Single = 1,
    Double = 2,
    Curly = 3,
    Dotted = 4,
    Dashed = 5,
}
impl UnderlineStyle {
    /// The style a line in `RowPaint::lines` marks, if it marks one.
    fn of(line: &Fill) -> Option<Self> {
        Some(match -line.height {
            1. => Self::Single,
            2. => Self::Double,
            3. => Self::Curly,
            4. => Self::Dotted,
            5. => Self::Dashed,
            _ => return None,
        })
    }
}

/// A row as drawn.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct RowPaint {
    pub fills: Vec<Fill>,
    pub runs: Vec<Run>,
    /// Lines over the text: strikes and the cursor's bars, and underlines
    /// in a style or colour of their own (`UnderlineStyle`).
    pub lines: Vec<Fill>,
    /// The row's text, for assistive technology.
    pub text: String,
}

pub(crate) fn color(rgb: Rgb) -> Color {
    Color(rgb.r, rgb.g, rgb.b, 0xff)
}

/// A cell colour as drawn: one the program set (OSC 4, 10, 11), else the
/// palette's.
fn resolve(term: &Term, palette: &Palette, color: VtColor) -> Rgb {
    match color {
        VtColor::Spec(rgb) => rgb,
        VtColor::Named(named) => {
            term.colors()[named as usize].unwrap_or_else(|| palette.color(named as usize))
        }
        VtColor::Indexed(index) => {
            let index = usize::from(index);
            term.colors()[index].unwrap_or_else(|| palette.color(index))
        }
    }
}

/// Whether a character's glyph may come from a font other than the grid's
/// (CJK, symbols, emoji, private-use icons): drawn alone, centred on its
/// cells. (A run is on the grid too, `LetterSpacing::cells`: a glyph there a
/// little wider or narrower than a cell moves nothing after it.) Most text
/// is out after a compare.
fn alone(c: char) -> bool {
    let c = c as u32;
    c >= 0x2190 && matches!(c, ..0x2500 | 0x25a0..0x2800 | 0x2e80..)
}

/// A block element (U+2580–U+259F) as the parts of its cell it fills:
/// `(left, top, right, bottom)` in eighths of the cell, and how much of the
/// ink it takes (a shade's quarter, half or three quarters).
fn block(c: char) -> Option<(&'static [(u8, u8, u8, u8)], f32)> {
    const UL: (u8, u8, u8, u8) = (0, 0, 4, 4);
    const UR: (u8, u8, u8, u8) = (4, 0, 8, 4);
    const LL: (u8, u8, u8, u8) = (0, 4, 4, 8);
    const LR: (u8, u8, u8, u8) = (4, 4, 8, 8);
    const FULL: &[(u8, u8, u8, u8)] = &[(0, 0, 8, 8)];
    Some(match c {
        '\u{2580}' => (&[(0, 0, 8, 4)], 1.),
        '\u{2581}' => (&[(0, 7, 8, 8)], 1.),
        '\u{2582}' => (&[(0, 6, 8, 8)], 1.),
        '\u{2583}' => (&[(0, 5, 8, 8)], 1.),
        '\u{2584}' => (&[(0, 4, 8, 8)], 1.),
        '\u{2585}' => (&[(0, 3, 8, 8)], 1.),
        '\u{2586}' => (&[(0, 2, 8, 8)], 1.),
        '\u{2587}' => (&[(0, 1, 8, 8)], 1.),
        '\u{2588}' => (FULL, 1.),
        '\u{2589}' => (&[(0, 0, 7, 8)], 1.),
        '\u{258a}' => (&[(0, 0, 6, 8)], 1.),
        '\u{258b}' => (&[(0, 0, 5, 8)], 1.),
        '\u{258c}' => (&[(0, 0, 4, 8)], 1.),
        '\u{258d}' => (&[(0, 0, 3, 8)], 1.),
        '\u{258e}' => (&[(0, 0, 2, 8)], 1.),
        '\u{258f}' => (&[(0, 0, 1, 8)], 1.),
        '\u{2590}' => (&[(4, 0, 8, 8)], 1.),
        '\u{2591}' => (FULL, 0.25),
        '\u{2592}' => (FULL, 0.5),
        '\u{2593}' => (FULL, 0.75),
        '\u{2594}' => (&[(0, 0, 8, 1)], 1.),
        '\u{2595}' => (&[(7, 0, 8, 8)], 1.),
        '\u{2596}' => (&[LL], 1.),
        '\u{2597}' => (&[LR], 1.),
        '\u{2598}' => (&[UL], 1.),
        '\u{2599}' => (&[UL, LL, LR], 1.),
        '\u{259a}' => (&[UL, LR], 1.),
        '\u{259b}' => (&[UL, UR, LL], 1.),
        '\u{259c}' => (&[UL, UR, LR], 1.),
        '\u{259d}' => (&[UR], 1.),
        '\u{259e}' => (&[UR, LL], 1.),
        '\u{259f}' => (&[UR, LL, LR], 1.),
        _ => return None,
    })
}

/// A box-drawing character (U+2500–U+257F) as its four arms from the
/// cell's middle, `[up, right, down, left]`: 0 none, 1 light, 2 heavy,
/// 3 double. Dashed lines are drawn solid; arcs as corners; a mixed
/// crossing as a light one.
fn arms(c: char) -> Option<[u8; 4]> {
    let c = c as u32;
    if !(0x2500..0x2580).contains(&c) {
        return None;
    }
    const TABLE: [[u8; 4]; 128] = [
        // 2500
        [0, 1, 0, 1], [0, 2, 0, 2], [1, 0, 1, 0], [2, 0, 2, 0],
        [0, 1, 0, 1], [0, 2, 0, 2], [1, 0, 1, 0], [2, 0, 2, 0],
        [0, 1, 0, 1], [0, 2, 0, 2], [1, 0, 1, 0], [2, 0, 2, 0],
        [0, 1, 1, 0], [0, 2, 1, 0], [0, 1, 2, 0], [0, 2, 2, 0],
        // 2510
        [0, 0, 1, 1], [0, 0, 1, 2], [0, 0, 2, 1], [0, 0, 2, 2],
        [1, 1, 0, 0], [1, 2, 0, 0], [2, 1, 0, 0], [2, 2, 0, 0],
        [1, 0, 0, 1], [1, 0, 0, 2], [2, 0, 0, 1], [2, 0, 0, 2],
        [1, 1, 1, 0], [1, 2, 1, 0], [2, 1, 1, 0], [1, 1, 2, 0],
        // 2520
        [2, 1, 2, 0], [2, 2, 1, 0], [1, 2, 2, 0], [2, 2, 2, 0],
        [1, 0, 1, 1], [1, 0, 1, 2], [2, 0, 1, 1], [1, 0, 2, 1],
        [2, 0, 2, 1], [2, 0, 1, 2], [1, 0, 2, 2], [2, 0, 2, 2],
        [0, 1, 1, 1], [0, 1, 1, 2], [0, 2, 1, 1], [0, 2, 1, 2],
        // 2530
        [0, 1, 2, 1], [0, 1, 2, 2], [0, 2, 2, 1], [0, 2, 2, 2],
        [1, 1, 0, 1], [1, 1, 0, 2], [1, 2, 0, 1], [1, 2, 0, 2],
        [2, 1, 0, 1], [2, 1, 0, 2], [2, 2, 0, 1], [2, 2, 0, 2],
        [1, 1, 1, 1], [1, 1, 1, 2], [1, 2, 1, 1], [1, 2, 1, 2],
        // 2540
        [2, 1, 1, 1], [1, 1, 2, 1], [2, 1, 2, 1], [2, 1, 1, 2],
        [2, 2, 1, 1], [1, 1, 2, 2], [1, 2, 2, 1], [2, 2, 1, 2],
        [1, 2, 2, 2], [2, 2, 2, 1], [2, 1, 2, 2], [2, 2, 2, 2],
        [0, 1, 0, 1], [0, 2, 0, 2], [1, 0, 1, 0], [2, 0, 2, 0],
        // 2550
        [0, 3, 0, 3], [3, 0, 3, 0], [0, 3, 1, 0], [0, 1, 3, 0],
        [0, 3, 3, 0], [0, 0, 1, 3], [0, 0, 3, 1], [0, 0, 3, 3],
        [1, 3, 0, 0], [3, 1, 0, 0], [3, 3, 0, 0], [1, 0, 0, 3],
        [3, 0, 0, 1], [3, 0, 0, 3], [1, 3, 1, 0], [3, 1, 3, 0],
        // 2560
        [3, 3, 3, 0], [1, 0, 1, 3], [3, 0, 3, 1], [3, 0, 3, 3],
        [0, 3, 1, 3], [0, 1, 3, 1], [0, 3, 3, 3], [1, 3, 0, 3],
        [3, 1, 0, 1], [3, 3, 0, 3], [1, 3, 1, 3], [3, 1, 3, 1],
        [3, 3, 3, 3], [0, 1, 1, 0], [0, 0, 1, 1], [1, 0, 0, 1],
        // 2570
        [1, 1, 0, 0], [0, 0, 0, 0], [0, 0, 0, 0], [0, 0, 0, 0],
        [0, 0, 0, 1], [1, 0, 0, 0], [0, 1, 0, 0], [0, 0, 1, 0],
        [0, 0, 0, 2], [2, 0, 0, 0], [0, 2, 0, 0], [0, 0, 2, 0],
        [0, 2, 0, 1], [1, 0, 2, 0], [0, 1, 0, 2], [2, 0, 1, 0],
    ];
    let arms = TABLE[(c - 0x2500) as usize];
    // (The diagonals, 2571–2573, are left to the font.)
    (arms != [0; 4]).then_some(arms)
}

/// A block element or box-drawing line at `x` as rectangles filling its
/// cell to the edges, so neighbours meet (the font's glyphs stop short of
/// the line height, and leave a gap between rows).
fn shapes(c: char, x: f32, width: f32, height: f32, ink: Color) -> Option<Vec<Fill>> {
    let rect = |x: f32, y: f32, w: f32, h: f32, color: Color| Fill { x, y, width: w, height: h, color };
    if let Some((parts, share)) = block(c) {
        let color = Color(ink.0, ink.1, ink.2, (f32::from(ink.3) * share).round() as u8);
        let (ex, ey) = (width / 8., height / 8.);
        return Some(
            parts
                .iter()
                .map(|&(l, t, r, b)| rect(x + f32::from(l) * ex, f32::from(t) * ey, f32::from(r - l) * ex, f32::from(b - t) * ey, color))
                .collect(),
        );
    }
    let [up, right, down, left] = arms(c)?;
    let light = (width / 8.).round().max(1.);
    let thick = |weight: u8| if weight == 2 { light * 2. } else { light };
    let (cx, cy) = ((x + width / 2. - light / 2.).round() + light / 2., (height / 2. - light / 2.).round() + light / 2.);
    // Each arm runs from the cell's edge past its middle, as far as the
    // widest arm across it reaches, so a corner or a tee closes.
    let half = |weight: u8| match weight {
        0 => 0.,
        3 => light * 1.5,
        weight => thick(weight) / 2.,
    };
    let across_v = half(left).max(half(right));
    let across_h = half(up).max(half(down));
    // A double arm: two light lines a line apart.
    let lanes = |weight: u8| -> &'static [f32] { if weight == 3 { &[-1., 1.] } else { &[0.] } };
    let mut out = Vec::new();
    for (weight, y0, y1) in [(up, 0., cy + across_v), (down, cy - across_v, height)] {
        if weight > 0 {
            let t = thick(weight);
            for lane in lanes(weight) {
                out.push(rect(cx + lane * light - t / 2., y0, t, y1 - y0, ink));
            }
        }
    }
    for (weight, x0, x1) in [(left, x, cx + across_h), (right, cx - across_h, x + width)] {
        if weight > 0 {
            let t = thick(weight);
            for lane in lanes(weight) {
                out.push(rect(x0, cy + lane * light - t / 2., x1 - x0, t, ink));
            }
        }
    }
    Some(out)
}

/// The row at viewport `row` (0 at the top of what shows).
pub(crate) fn row(
    term: &Term,
    row: usize,
    look: &Look,
    metrics: Metrics,
    cursor: Option<CursorAt>,
    selection: Option<&SelectionRange>,
) -> RowPaint {
    let (paint, styled) = row_built::<false>(term, row, look, metrics, cursor, selection);
    if styled {
        return row_styled(term, row, look, metrics, cursor, selection);
    }
    paint
}

/// `row`, for one with an underline in a style or colour of its own: built
/// again, those underlines drawn as shapes. Out of `row`'s line, as the
/// building with them is (`row_built::<true>`): a plain row's has none.
#[cold]
#[inline(never)]
fn row_styled(
    term: &Term,
    row: usize,
    look: &Look,
    metrics: Metrics,
    cursor: Option<CursorAt>,
    selection: Option<&SelectionRange>,
) -> RowPaint {
    row_built::<true>(term, row, look, metrics, cursor, selection).0
}

/// The row at viewport `row`, and whether it has an underline in a style or
/// colour of its own. Without `STYLED` those are left to the text (the row
/// is built again with it, `row_styled`), so a plain row carries nothing of
/// them.
fn row_built<const STYLED: bool>(
    term: &Term,
    row: usize,
    look: &Look,
    metrics: Metrics,
    cursor: Option<CursorAt>,
    selection: Option<&SelectionRange>,
) -> (RowPaint, bool) {
    let grid = term.grid();
    let line = Line(row as i32 - grid.display_offset() as i32);
    let cells = &grid[line];
    let columns = grid.columns();
    let (width, height) = (metrics.cell_width, metrics.cell_height);
    let palette = &look.palette;
    let background = palette.background;
    let mut paint = RowPaint::default();
    // The underlines its runs start with, and whether one is in a colour of
    // its own (`CellExtra`, rare: found where a cell's zero-width characters
    // are read), for a plain row to say whether it has a styled one.
    let mut underlines = Flags::empty();
    let mut coloured = false;
    // The open run, its text so far, and how many blanks end it.
    let mut run: Option<Run> = None;
    let mut open = String::new();
    let mut blanks = 0usize;
    let finish =
        |run: &mut Option<Run>, open: &mut String, blanks: &mut usize, runs: &mut Vec<Run>| {
            if let Some(mut done) = run.take() {
                let keep = open.len() - *blanks;
                if keep > 0 {
                    done.text = open[..keep].into();
                    runs.push(done);
                }
            }
            open.clear();
            *blanks = 0;
        };
    let fill_at = |paint: &mut RowPaint, column: usize, color: Color| {
        let x = column as f32 * width;
        if let Some(last) = paint.fills.last_mut()
            && last.color == color
            && (last.x + last.width - x).abs() < 0.01
        {
            last.width += width;
            return;
        }
        paint.fills.push(Fill {
            x,
            y: 0.,
            width,
            height,
            color,
        });
    };
    for column in 0..columns {
        let cell: &Cell = &cells[Column(column)];
        let flags = cell.flags;
        if flags.contains(Flags::WIDE_CHAR_SPACER) {
            paint.text.push(' ');
        }
        let mut fg = resolve(term, palette, cell.fg);
        let mut bg = match cell.bg {
            VtColor::Named(NamedColor::Background) => None,
            other => Some(resolve(term, palette, other)),
        };
        if flags.contains(Flags::DIM) {
            fg = dim(fg);
        }
        if flags.contains(Flags::INVERSE) {
            let was = fg;
            fg = bg.unwrap_or(background);
            bg = Some(was);
        }
        let mut fill = bg.map(color);
        let mut ink = color(fg);
        if selection.is_some_and(|range| range.contains(Point::new(line, Column(column)))) {
            fill = Some(look.selection);
            ink = look.selection_text.unwrap_or(ink);
        }
        // The cursor's cell, or a wide one's second half.
        let under_cursor = cursor.filter(|at| {
            at.column == column
                || (at.column + 1 == column
                    && cells[Column(at.column)].flags.contains(Flags::WIDE_CHAR))
        });
        if let Some(at) = under_cursor {
            let cursor_color =
                color(term.colors()[NamedColor::Cursor as usize].unwrap_or(palette.cursor));
            let x = column as f32 * width;
            let thin = (height / 14.).clamp(1., 2.);
            match (at.shape, at.focused) {
                (CursorShape::Hidden, _) => {}
                (CursorShape::Block, true) => {
                    fill = Some(cursor_color);
                    ink = look.cursor_text.unwrap_or(color(background));
                }
                (CursorShape::Beam, true) if at.column == column => paint.lines.push(Fill {
                    x,
                    y: 0.,
                    width: thin * 1.5,
                    height,
                    color: cursor_color,
                }),
                (CursorShape::Underline, true) => paint.lines.push(Fill {
                    x,
                    y: height - thin * 1.5,
                    width,
                    height: thin * 1.5,
                    color: cursor_color,
                }),
                (CursorShape::Beam, true) => {}
                // Unfocused (or asked for): an outline.
                _ => {
                    let last = at.column != column
                        || !cells[Column(column)].flags.contains(Flags::WIDE_CHAR);
                    let first = at.column == column;
                    for (x, y, w, h) in [(x, 0., width, thin), (x, height - thin, width, thin)] {
                        paint.lines.push(Fill {
                            x,
                            y,
                            width: w,
                            height: h,
                            color: cursor_color,
                        });
                    }
                    if first {
                        paint.lines.push(Fill {
                            x,
                            y: 0.,
                            width: thin,
                            height,
                            color: cursor_color,
                        });
                    }
                    if last {
                        paint.lines.push(Fill {
                            x: x + width - thin,
                            y: 0.,
                            width: thin,
                            height,
                            color: cursor_color,
                        });
                    }
                }
            }
        }
        if let Some(fill) = fill {
            fill_at(&mut paint, column, fill);
        }
        if flags.intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER) {
            continue;
        }
        let c = cell.c;
        paint.text.push(if c == '\t' { ' ' } else { c });
        if flags.contains(Flags::STRIKEOUT) {
            let span = if flags.contains(Flags::WIDE_CHAR) {
                2.
            } else {
                1.
            };
            paint.lines.push(Fill {
                x: column as f32 * width,
                y: (height / 2.).round() - 0.5,
                width: width * span,
                height: (height / 14.).clamp(1., 2.),
                color: ink,
            });
        }
        let blank = c == ' ' || c == '\t' || c == '\0' || flags.contains(Flags::HIDDEN);
        let bold = flags.contains(Flags::BOLD);
        let italic = flags.contains(Flags::ITALIC);
        let mut underline = flags & Flags::ALL_UNDERLINES;
        // In a style or a colour of its own: drawn as shapes under the cells,
        // not by the text (whose underline is a plain line in its colour).
        // A plain row's runs say whether it has one (`row`).
        if STYLED
            && !underline.is_empty()
            && (underline.intersects(STYLED_UNDERLINES) || cell.underline_color().is_some())
        {
            styled_underline(&mut paint, term, palette, cell, column, width, ink);
            underline = Flags::empty();
        }
        if blank && underline.is_empty() {
            // A blank joins the open run, whatever its colour (which doesn't
            // show), unless that run is underlined.
            match &run {
                Some(open_run) if open_run.underline.is_empty() => {
                    open.push(' ');
                    blanks += 1;
                }
                _ => finish(&mut run, &mut open, &mut blanks, &mut paint.runs),
            }
            continue;
        }
        // Blocks and box lines: drawn to the cell, not from the font.
        if cfg!(windows) && !blank
            && let Some(parts) = shapes(c, column as f32 * width, width, height, ink)
        {
            finish(&mut run, &mut open, &mut blanks, &mut paint.runs);
            paint.lines.extend(parts);
            continue;
        }
        let wide = flags.contains(Flags::WIDE_CHAR);
        if wide || alone(c) {
            finish(&mut run, &mut open, &mut blanks, &mut paint.runs);
            underlines |= underline;
            let mut text = String::new();
            text.push(c);
            if let Some(extra) = cell.zerowidth() {
                coloured |= !underline.is_empty() && cell.underline_color().is_some();
                text.extend(extra);
            }
            paint.runs.push(Run {
                column: column as u16,
                text: text.into(),
                color: ink,
                bold,
                italic,
                underline,
                alone: true,
                cells: if wide { 2 } else { 1 },
            });
            continue;
        }
        let joins = run.as_ref().is_some_and(|open| {
            open.color == ink
                && open.bold == bold
                && open.italic == italic
                && open.underline == underline
        });
        if joins {
            blanks = 0;
        } else {
            finish(&mut run, &mut open, &mut blanks, &mut paint.runs);
            underlines |= underline;
            run = Some(Run {
                column: column as u16,
                text: Arc::from(""),
                color: ink,
                bold,
                italic,
                underline,
                alone: false,
                cells: 1,
            });
        }
        open.push(if blank { ' ' } else { c });
        if let Some(extra) = cell.zerowidth() {
            coloured |= !underline.is_empty() && cell.underline_color().is_some();
            open.extend(extra);
        }
    }
    finish(&mut run, &mut open, &mut blanks, &mut paint.runs);
    // Its white end gone: blank cells' spaces, most of a row, as bytes, then
    // any other white character before them.
    let mut end = paint.text.trim_ascii_end().len();
    while let Some(c) = paint.text[..end]
        .chars()
        .next_back()
        .filter(|c| c.is_whitespace())
    {
        end -= c.len_utf8();
    }
    paint.text.truncate(end);
    let styled = !STYLED && (coloured || underlines.intersects(STYLED_UNDERLINES));
    (paint, styled)
}

/// The underlines a text can't draw: every one but the plain single line.
const STYLED_UNDERLINES: Flags = Flags::DOUBLE_UNDERLINE
    .union(Flags::UNDERCURL)
    .union(Flags::DOTTED_UNDERLINE)
    .union(Flags::DASHED_UNDERLINE);

/// `cell`'s underline in a style or colour of its own (column `column`,
/// cells `width` wide, its text in `ink`), joined to the one before it where
/// they meet in the same style and colour.
#[cold]
#[inline(never)]
fn styled_underline(
    paint: &mut RowPaint,
    term: &Term,
    palette: &Palette,
    cell: &Cell,
    column: usize,
    width: f32,
    ink: Color,
) {
    let flags = cell.flags;
    let style = if flags.contains(Flags::UNDERCURL) {
        UnderlineStyle::Curly
    } else if flags.contains(Flags::DOUBLE_UNDERLINE) {
        UnderlineStyle::Double
    } else if flags.contains(Flags::DOTTED_UNDERLINE) {
        UnderlineStyle::Dotted
    } else if flags.contains(Flags::DASHED_UNDERLINE) {
        UnderlineStyle::Dashed
    } else {
        UnderlineStyle::Single
    };
    let color = cell
        .underline_color()
        .map_or(ink, |given| self::color(resolve(term, palette, given)));
    let span = if flags.contains(Flags::WIDE_CHAR) {
        2.
    } else {
        1.
    };
    let x = column as f32 * width;
    let height = -(style as u8 as f32);
    if let Some(last) = paint.lines.last_mut()
        && last.height == height
        && last.color == color
        && (last.x + last.width - x).abs() < 0.01
    {
        last.width += width * span;
        return;
    }
    paint.lines.push(Fill {
        x,
        y: 0.,
        width: width * span,
        height,
        color,
    });
}

/// The grid's font: the platform's monospaced one, without ligatures.
pub(crate) fn font() -> FontStyle {
    FontStyle {
        family: FontFamily::Monospace,
        features: FontFeatures::new([(*b"liga", 0), (*b"calt", 0), (*b"clig", 0), (*b"dlig", 0)]),
        // What the monospaced font lacks (CJK) from the interface font's
        // fallbacks.
        fallbacks: Arc::from([FontFamily::SansSerif]),
        ..FontStyle::default()
    }
}

/// `paint` drawn on a row's canvas, in `font`: the grid's, on its cells
/// (`LetterSpacing::cells`, set once with the cell, `view::row_view`).
pub(crate) fn draw(paint: &RowPaint, metrics: Metrics, font: &FontStyle) -> Canvas {
    let mut canvas = Canvas::new();
    for fill in paint.fills.iter().chain(&paint.lines) {
        if fill.height < 0. {
            draw_underline(&mut canvas, fill, metrics);
            continue;
        }
        canvas.fill(
            Path::rectangle(Rect {
                x: fill.x,
                y: fill.y,
                width: fill.width,
                height: fill.height,
            }),
            fill.color,
        );
    }
    for run in &paint.runs {
        let mut font = font.clone();
        if run.bold {
            font.weight = 700;
        }
        font.italic = run.italic;
        if !run.underline.is_empty() {
            font.underline = Some(TextUnderline::new(None));
        }
        let x = run.column as f32 * metrics.cell_width;
        let (x, anchor) = if run.alone {
            (
                x + metrics.cell_width * f32::from(run.cells) / 2.,
                TextAnchor::Center,
            )
        } else {
            (x, TextAnchor::Start)
        };
        canvas.text(CanvasText {
            text: run.text.clone(),
            x,
            y: 0.,
            size: metrics.font_size,
            color: run.color,
            font,
            anchor,
            baseline: TextBaseline::Top,
        });
    }
    canvas
}

/// An underline in a style of its own (`UnderlineStyle`) on a row's canvas:
/// where the text's own goes (a tenth of the font's size below its
/// baseline, the baseline 0.8 of the size below the top of the font's box,
/// centred in the cell), one point thick.
#[cold]
#[inline(never)]
fn draw_underline(canvas: &mut Canvas, underline: &Fill, metrics: Metrics) {
    let Some(style) = UnderlineStyle::of(underline) else {
        return;
    };
    let (size, height) = (metrics.font_size, metrics.cell_height);
    let thick = 1.;
    let baseline = (height - size).max(0.) / 2. + size * 0.8;
    // Inside the cell with the room its style needs below it (a double's
    // second line, a wave's trough), on a whole point: a single line where
    // the text's own would be.
    let room = match style {
        UnderlineStyle::Double => thick * 3.,
        UnderlineStyle::Curly => thick * 3.5,
        _ => thick,
    };
    let y = (baseline + size * 0.1).min(height - room).max(0.).round();
    let (x, width) = (underline.x, underline.width);
    let rect = |canvas: &mut Canvas, x: f32, y: f32, width: f32| {
        canvas.fill(
            Path::rectangle(Rect {
                x,
                y,
                width,
                height: thick,
            }),
            underline.color,
        );
    };
    match style {
        UnderlineStyle::Single => rect(canvas, x, y, width),
        UnderlineStyle::Double => {
            rect(canvas, x, y, width);
            rect(canvas, x, y + thick * 2., width);
        }
        // Squares a point apart, and dashes, on the grid's own beat, so an
        // underline across runs or rows goes on without a break in step.
        UnderlineStyle::Dotted => {
            let step = thick * 2.;
            let mut at = (x / step).ceil() * step;
            while at + thick <= x + width + 0.01 {
                rect(canvas, at, y, thick);
                at += step;
            }
        }
        UnderlineStyle::Dashed => {
            let dash = (size * 0.25).round().max(2.);
            let step = dash + (dash * 0.66).round().max(1.);
            let mut at = (x / step).floor() * step;
            while at < x + width {
                let (from, to) = (at.max(x), (at + dash).min(x + width));
                if to > from {
                    rect(canvas, from, y, to - from);
                }
                at += step;
            }
        }
        // A wave a cell long, from the cell's start, so neighbouring cells'
        // waves join.
        UnderlineStyle::Curly => {
            let period = metrics.cell_width.max(2.);
            let amplitude = thick * 1.25;
            let middle = y + amplitude;
            let steps = ((width / period) * 8.).ceil().max(1.) as usize;
            let mut path = Path::builder();
            for step in 0..=steps {
                let px = x + width * step as f32 / steps as f32;
                let phase = (px / period) * std::f32::consts::TAU;
                let py = middle - amplitude * phase.sin();
                if step == 0 {
                    path.move_to(px, py);
                } else {
                    path.line_to(px, py);
                }
            }
            let Ok(path) = path.build() else {
                return;
            };
            canvas.stroke(
                path,
                underline.color,
                Stroke {
                    width: thick,
                    cap: LineCap::Butt,
                    join: LineJoin::Round,
                    ..Stroke::default()
                },
            );
        }
    }
}
