//! Port of Ghostty's src/font/sprite/draw/symbols_for_legacy_computing_supplement.zig.
//! Symbols for Legacy Computing Supplement | U+1CC00...U+1CEBF
//! https://en.wikipedia.org/wiki/Symbols_for_Legacy_Computing_Supplement

use tiny_skia::PathBuilder;

use super::Metrics;
use super::box_drawing::{self, Lines, Style};
use super::canvas::{self, Canvas, ON};
use super::common::{Alignment, Corner, Fraction, fill};
use super::legacy_computing;

const OCTANT_MIN: u32 = 0x1CD00;

/// Octants U+1CD00…U+1CDE5, in codepoint order: bit `n-1` set means octant
/// `n` is filled. Generated from Ghostty's `octants.txt`
/// ("BLOCK OCTANT-1235" → 0b0001_0111).
const OCTANTS: [u8; 230] = [
    0x04, 0x06, 0x07, 0x08, 0x09, 0x0B, 0x0C, 0x0D, 0x0E, 0x10, 0x11, 0x12, 0x13, 0x15, 0x16, 0x17,
    0x18, 0x19, 0x1A, 0x1B, 0x1C, 0x1D, 0x1E, 0x1F, 0x20, 0x21, 0x22, 0x23, 0x24, 0x25, 0x26, 0x27,
    0x29, 0x2A, 0x2B, 0x2C, 0x2D, 0x2E, 0x2F, 0x30, 0x31, 0x32, 0x33, 0x34, 0x35, 0x36, 0x37, 0x38,
    0x39, 0x3A, 0x3B, 0x3C, 0x3D, 0x3E, 0x41, 0x42, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49, 0x4A,
    0x4B, 0x4C, 0x4D, 0x4E, 0x4F, 0x51, 0x52, 0x53, 0x54, 0x56, 0x57, 0x58, 0x59, 0x5B, 0x5C, 0x5D,
    0x5E, 0x60, 0x61, 0x62, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69, 0x6A, 0x6B, 0x6C, 0x6D, 0x6E,
    0x6F, 0x70, 0x71, 0x72, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7A, 0x7B, 0x7C, 0x7D, 0x7E,
    0x7F, 0x81, 0x82, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89, 0x8A, 0x8B, 0x8C, 0x8D, 0x8E, 0x8F,
    0x90, 0x91, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9A, 0x9B, 0x9C, 0x9D, 0x9E, 0x9F,
    0xA1, 0xA2, 0xA3, 0xA4, 0xA6, 0xA7, 0xA8, 0xA9, 0xAB, 0xAC, 0xAD, 0xAE, 0xB0, 0xB1, 0xB2, 0xB3,
    0xB4, 0xB5, 0xB6, 0xB7, 0xB8, 0xB9, 0xBA, 0xBB, 0xBC, 0xBD, 0xBE, 0xBF, 0xC1, 0xC2, 0xC3, 0xC4,
    0xC5, 0xC6, 0xC7, 0xC8, 0xC9, 0xCA, 0xCB, 0xCC, 0xCD, 0xCE, 0xCF, 0xD0, 0xD1, 0xD2, 0xD3, 0xD4,
    0xD5, 0xD6, 0xD7, 0xD8, 0xD9, 0xDA, 0xDB, 0xDC, 0xDD, 0xDE, 0xDF, 0xE0, 0xE1, 0xE2, 0xE3, 0xE4,
    0xE5, 0xE6, 0xE7, 0xE8, 0xE9, 0xEA, 0xEB, 0xEC, 0xED, 0xEE, 0xEF, 0xF1, 0xF2, 0xF3, 0xF4, 0xF6,
    0xF7, 0xF8, 0xF9, 0xFB, 0xFD, 0xFE,
];

/// Octants
pub fn draw_1cd00_1cde5(
    cp: u32,
    canvas: &mut Canvas,
    _width: u32,
    _height: u32,
    metrics: &Metrics,
) {
    use Fraction::{Half, One, OneQuarter, ThreeQuarters, Zero};
    let oct = OCTANTS[(cp - OCTANT_MIN) as usize];
    let has = |n: u32| oct & (1 << (n - 1)) != 0;
    if has(1) {
        fill(metrics, canvas, Zero, Half, Zero, OneQuarter);
    }
    if has(2) {
        fill(metrics, canvas, Half, One, Zero, OneQuarter);
    }
    if has(3) {
        fill(metrics, canvas, Zero, Half, OneQuarter, Half);
    }
    if has(4) {
        fill(metrics, canvas, Half, One, OneQuarter, Half);
    }
    if has(5) {
        fill(metrics, canvas, Zero, Half, Half, ThreeQuarters);
    }
    if has(6) {
        fill(metrics, canvas, Half, One, Half, ThreeQuarters);
    }
    if has(7) {
        fill(metrics, canvas, Zero, Half, ThreeQuarters, One);
    }
    if has(8) {
        fill(metrics, canvas, Half, One, ThreeQuarters, One);
    }
}

/// Separated Block Quadrants
/// 𜰡 𜰢 𜰣 𜰤 𜰥 𜰦 𜰧 𜰨 𜰩 𜰪 𜰫 𜰬 𜰭 𜰮 𜰯
pub fn draw_1cc21_1cc2f(cp: u32, canvas: &mut Canvas, width: u32, height: u32, _metrics: &Metrics) {
    // Bits in codepoint order: tl, tr, bl, br.
    let bits = (cp - 0x1CC20) & 0xF;
    let (tl, tr, bl, br) = (bits & 1 != 0, bits & 2 != 0, bits & 4 != 0, bits & 8 != 0);

    let gap = (width / 12).max(1) as i32;

    let mid_gap_x = gap * 2 + (width % 2) as i32;
    let mid_gap_y = gap * 2 + (height % 2) as i32;

    let w = (width as i32 - gap * 2 - mid_gap_x) / 2;
    let h = (height as i32 - gap * 2 - mid_gap_y) / 2;

    if tl {
        canvas.box_(gap, gap, gap + w, gap + h, ON);
    }
    if tr {
        canvas.box_(
            gap + w + mid_gap_x,
            gap,
            gap + w + mid_gap_x + w,
            gap + h,
            ON,
        );
    }
    if bl {
        canvas.box_(
            gap,
            gap + h + mid_gap_y,
            gap + w,
            gap + h + mid_gap_y + h,
            ON,
        );
    }
    if br {
        canvas.box_(
            gap + w + mid_gap_x,
            gap + h + mid_gap_y,
            gap + w + mid_gap_x + w,
            gap + h + mid_gap_y + h,
            ON,
        );
    }
}

/// Twelfth and Quarter circle pieces.
/// 𜰰 𜰱 𜰲 𜰳 𜰴 𜰵 𜰶 𜰷 𜰸 𜰹 𜰺 𜰻 𜰼 𜰽 𜰾 𜰿
///
/// 𜰰𜰱𜰲𜰳
/// 𜰴𜰵𜰶𜰷
/// 𜰸𜰹𜰺𜰻
/// 𜰼𜰽𜰾𜰿
///
/// These are actually ellipses, sized to touch the edge of their enclosing
/// set of cells.
pub fn draw_1cc30_1cc3f(cp: u32, canvas: &mut Canvas, width: u32, height: u32, metrics: &Metrics) {
    use Corner::{Bl, Br, Tl, Tr};
    let (x, y, w, h, corner) = match cp {
        // 𜰰 UPPER LEFT TWELFTH CIRCLE
        0x1CC30 => (0.0, 0.0, 2.0, 2.0, Tl),
        // 𜰱 UPPER CENTRE LEFT TWELFTH CIRCLE
        0x1CC31 => (1.0, 0.0, 2.0, 2.0, Tl),
        // 𜰲 UPPER CENTRE RIGHT TWELFTH CIRCLE
        0x1CC32 => (2.0, 0.0, 2.0, 2.0, Tr),
        // 𜰳 UPPER RIGHT TWELFTH CIRCLE
        0x1CC33 => (3.0, 0.0, 2.0, 2.0, Tr),
        // 𜰴 UPPER MIDDLE LEFT TWELFTH CIRCLE
        0x1CC34 => (0.0, 1.0, 2.0, 2.0, Tl),
        // 𜰵 UPPER LEFT QUARTER CIRCLE
        0x1CC35 => (0.0, 0.0, 1.0, 1.0, Tl),
        // 𜰶 UPPER RIGHT QUARTER CIRCLE
        0x1CC36 => (1.0, 0.0, 1.0, 1.0, Tr),
        // 𜰷 UPPER MIDDLE RIGHT TWELFTH CIRCLE
        0x1CC37 => (3.0, 1.0, 2.0, 2.0, Tr),
        // 𜰸 LOWER MIDDLE LEFT TWELFTH CIRCLE
        0x1CC38 => (0.0, 2.0, 2.0, 2.0, Bl),
        // 𜰹 LOWER LEFT QUARTER CIRCLE
        0x1CC39 => (0.0, 1.0, 1.0, 1.0, Bl),
        // 𜰺 LOWER RIGHT QUARTER CIRCLE
        0x1CC3A => (1.0, 1.0, 1.0, 1.0, Br),
        // 𜰻 LOWER MIDDLE RIGHT TWELFTH CIRCLE
        0x1CC3B => (3.0, 2.0, 2.0, 2.0, Br),
        // 𜰼 LOWER LEFT TWELFTH CIRCLE
        0x1CC3C => (0.0, 3.0, 2.0, 2.0, Bl),
        // 𜰽 LOWER CENTRE LEFT TWELFTH CIRCLE
        0x1CC3D => (1.0, 3.0, 2.0, 2.0, Bl),
        // 𜰾 LOWER CENTRE RIGHT TWELFTH CIRCLE
        0x1CC3E => (2.0, 3.0, 2.0, 2.0, Br),
        // 𜰿 LOWER RIGHT TWELFTH CIRCLE
        0x1CC3F => (3.0, 3.0, 2.0, 2.0, Br),
        _ => return,
    };
    circle_piece(canvas, width, height, metrics, x, y, w, h, corner);
}

/// TODO (Ghostty): 1CC1F 𜰟 and 1CC20 𜰠 (double diagonals) are not drawn:
/// it is not clear how they should align with adjacent cells.
pub fn draw_1cc1b_1cc1e(cp: u32, canvas: &mut Canvas, width: u32, height: u32, metrics: &Metrics) {
    let w = width as i32;
    let h = height as i32;
    let t = metrics.box_thickness as i32;
    let horizontal = Lines {
        left: Style::Light,
        right: Style::Light,
        ..Lines::default()
    };
    match cp {
        // 𜰛 BOX DRAWINGS LIGHT HORIZONTAL AND UPPER RIGHT
        0x1CC1B => {
            box_drawing::lines_char(metrics, canvas, horizontal);
            canvas.box_(w - t, 0, w, h.div_euclid(2), ON);
        }
        // 𜰜 BOX DRAWINGS LIGHT HORIZONTAL AND LOWER RIGHT
        0x1CC1C => {
            box_drawing::lines_char(metrics, canvas, horizontal);
            canvas.box_(w - t, h.div_euclid(2), w, h, ON);
        }
        // 𜰝 BOX DRAWINGS LIGHT TOP AND UPPER LEFT
        0x1CC1D => {
            canvas.box_(0, 0, w, t, ON);
            canvas.box_(0, 0, t, h.div_euclid(2), ON);
        }
        // 𜰞 BOX DRAWINGS LIGHT BOTTOM AND LOWER LEFT
        0x1CC1E => {
            canvas.box_(0, h - t, w, h, ON);
            canvas.box_(0, h.div_euclid(2), t, h, ON);
        }
        _ => {}
    }
}

/// 𜸀 RIGHT HALF AND LEFT HALF WHITE CIRCLE
pub fn draw_1ce00(_cp: u32, canvas: &mut Canvas, _width: u32, _height: u32, metrics: &Metrics) {
    legacy_computing::circle(metrics, canvas, Alignment::LEFT, false);
    legacy_computing::circle(metrics, canvas, Alignment::RIGHT, false);
}

/// 𜸁 LOWER HALF AND UPPER HALF WHITE CIRCLE
pub fn draw_1ce01(_cp: u32, canvas: &mut Canvas, _width: u32, _height: u32, metrics: &Metrics) {
    legacy_computing::circle(metrics, canvas, Alignment::UPPER, false);
    legacy_computing::circle(metrics, canvas, Alignment::LOWER, false);
}

/// 𜸋 LEFT HALF WHITE ELLIPSE
pub fn draw_1ce0b(_cp: u32, canvas: &mut Canvas, width: u32, height: u32, metrics: &Metrics) {
    circle_piece(
        canvas,
        width,
        height,
        metrics,
        0.0,
        0.0,
        1.0,
        0.5,
        Corner::Tl,
    );
    circle_piece(
        canvas,
        width,
        height,
        metrics,
        0.0,
        0.0,
        1.0,
        0.5,
        Corner::Bl,
    );
}

/// 𜸌 RIGHT HALF WHITE ELLIPSE
pub fn draw_1ce0c(_cp: u32, canvas: &mut Canvas, width: u32, height: u32, metrics: &Metrics) {
    circle_piece(
        canvas,
        width,
        height,
        metrics,
        1.0,
        0.0,
        1.0,
        0.5,
        Corner::Tr,
    );
    circle_piece(
        canvas,
        width,
        height,
        metrics,
        1.0,
        0.0,
        1.0,
        0.5,
        Corner::Br,
    );
}

pub fn draw_1ce16_1ce19(cp: u32, canvas: &mut Canvas, width: u32, height: u32, metrics: &Metrics) {
    let w = width as i32;
    let h = height as i32;
    let t = metrics.box_thickness as i32;
    let vertical = Lines {
        up: Style::Light,
        down: Style::Light,
        ..Lines::default()
    };
    match cp {
        // 𜸖 BOX DRAWINGS LIGHT VERTICAL AND TOP RIGHT
        0x1CE16 => {
            box_drawing::lines_char(metrics, canvas, vertical);
            canvas.box_(w.div_euclid(2), 0, w, t, ON);
        }
        // 𜸗 BOX DRAWINGS LIGHT VERTICAL AND BOTTOM RIGHT
        0x1CE17 => {
            box_drawing::lines_char(metrics, canvas, vertical);
            canvas.box_(w.div_euclid(2), h - t, w, h, ON);
        }
        // 𜸘 BOX DRAWINGS LIGHT VERTICAL AND TOP LEFT
        0x1CE18 => {
            box_drawing::lines_char(metrics, canvas, vertical);
            canvas.box_(0, 0, w.div_euclid(2), t, ON);
        }
        // 𜸙 BOX DRAWINGS LIGHT VERTICAL AND BOTTOM LEFT
        0x1CE19 => {
            box_drawing::lines_char(metrics, canvas, vertical);
            canvas.box_(0, h - t, w.div_euclid(2), h, ON);
        }
        _ => {}
    }
}

/// Separated Block Sextants
pub fn draw_1ce51_1ce8f(cp: u32, canvas: &mut Canvas, width: u32, height: u32, _metrics: &Metrics) {
    // Bits in codepoint order: tl, tr, ml, mr, bl, br.
    let bits = (cp - 0x1CE50) & 0x3F;
    let has = |n: u32| bits & (1 << n) != 0;
    let (tl, tr, ml, mr, bl, br) = (has(0), has(1), has(2), has(3), has(4), has(5));

    let gap = (width / 12).max(1) as i32;

    let mid_gap_x = gap * 2 + (width % 2) as i32;
    let y_extra = (height % 3) as i32;
    let mid_gap_y = gap * 2 + y_extra.div_euclid(2);

    let w = (width as i32 - gap * 2 - mid_gap_x) / 2;
    let h = (height as i32 - gap * 2 - mid_gap_y * 2).div_euclid(3);
    // Distribute any leftover height in to the middle row of blocks.
    let h_m = height as i32 - gap * 2 - mid_gap_y * 2 - h * 2;

    let (x0, x1) = (gap, gap + w + mid_gap_x);
    let (y0, y1, y2) = (
        gap,
        gap + h + mid_gap_y,
        gap + h + mid_gap_y + h_m + mid_gap_y,
    );
    if tl {
        canvas.box_(x0, y0, x0 + w, y0 + h, ON);
    }
    if tr {
        canvas.box_(x1, y0, x1 + w, y0 + h, ON);
    }
    if ml {
        canvas.box_(x0, y1, x0 + w, y1 + h_m, ON);
    }
    if mr {
        canvas.box_(x1, y1, x1 + w, y1 + h_m, ON);
    }
    if bl {
        canvas.box_(x0, y2, x0 + w, y2 + h, ON);
    }
    if br {
        canvas.box_(x1, y2, x1 + w, y2 + h, ON);
    }
}

/// Sixteenth Blocks
pub fn draw_1ce90_1ceaf(
    cp: u32,
    canvas: &mut Canvas,
    _width: u32,
    _height: u32,
    metrics: &Metrics,
) {
    let q = Fraction::QUARTERS;
    let (x0, x1, y0, y1) = match cp {
        // 𜺐 UPPER LEFT ONE SIXTEENTH BLOCK
        0x1CE90 => (0, 1, 0, 1),
        // 𜺑 UPPER CENTRE LEFT ONE SIXTEENTH BLOCK
        0x1CE91 => (1, 2, 0, 1),
        // 𜺒 UPPER CENTRE RIGHT ONE SIXTEENTH BLOCK
        0x1CE92 => (2, 3, 0, 1),
        // 𜺓 UPPER RIGHT ONE SIXTEENTH BLOCK
        0x1CE93 => (3, 4, 0, 1),
        // 𜺔 UPPER MIDDLE LEFT ONE SIXTEENTH BLOCK
        0x1CE94 => (0, 1, 1, 2),
        // 𜺕 UPPER MIDDLE CENTRE LEFT ONE SIXTEENTH BLOCK
        0x1CE95 => (1, 2, 1, 2),
        // 𜺖 UPPER MIDDLE CENTRE RIGHT ONE SIXTEENTH BLOCK
        0x1CE96 => (2, 3, 1, 2),
        // 𜺗 UPPER MIDDLE RIGHT ONE SIXTEENTH BLOCK
        0x1CE97 => (3, 4, 1, 2),
        // 𜺘 LOWER MIDDLE LEFT ONE SIXTEENTH BLOCK
        0x1CE98 => (0, 1, 2, 3),
        // 𜺙 LOWER MIDDLE CENTRE LEFT ONE SIXTEENTH BLOCK
        0x1CE99 => (1, 2, 2, 3),
        // 𜺚 LOWER MIDDLE CENTRE RIGHT ONE SIXTEENTH BLOCK
        0x1CE9A => (2, 3, 2, 3),
        // 𜺛 LOWER MIDDLE RIGHT ONE SIXTEENTH BLOCK
        0x1CE9B => (3, 4, 2, 3),
        // 𜺜 LOWER LEFT ONE SIXTEENTH BLOCK
        0x1CE9C => (0, 1, 3, 4),
        // 𜺝 LOWER CENTRE LEFT ONE SIXTEENTH BLOCK
        0x1CE9D => (1, 2, 3, 4),
        // 𜺞 LOWER CENTRE RIGHT ONE SIXTEENTH BLOCK
        0x1CE9E => (2, 3, 3, 4),
        // 𜺟 LOWER RIGHT ONE SIXTEENTH BLOCK
        0x1CE9F => (3, 4, 3, 4),

        // 𜺠 RIGHT HALF LOWER ONE QUARTER BLOCK
        0x1CEA0 => (2, 4, 3, 4),
        // 𜺡 RIGHT THREE QUARTERS LOWER ONE QUARTER BLOCK
        0x1CEA1 => (1, 4, 3, 4),
        // 𜺢 LEFT THREE QUARTERS LOWER ONE QUARTER BLOCK
        0x1CEA2 => (0, 3, 3, 4),
        // 𜺣 LEFT HALF LOWER ONE QUARTER BLOCK
        0x1CEA3 => (0, 2, 3, 4),
        // 𜺤 LOWER HALF LEFT ONE QUARTER BLOCK
        0x1CEA4 => (0, 1, 2, 4),
        // 𜺥 LOWER THREE QUARTERS LEFT ONE QUARTER BLOCK
        0x1CEA5 => (0, 1, 1, 4),
        // 𜺦 UPPER THREE QUARTERS LEFT ONE QUARTER BLOCK
        0x1CEA6 => (0, 1, 0, 3),
        // 𜺧 UPPER HALF LEFT ONE QUARTER BLOCK
        0x1CEA7 => (0, 1, 0, 2),
        // 𜺨 LEFT HALF UPPER ONE QUARTER BLOCK
        0x1CEA8 => (0, 2, 0, 1),
        // 𜺩 LEFT THREE QUARTERS UPPER ONE QUARTER BLOCK
        0x1CEA9 => (0, 3, 0, 1),
        // 𜺪 RIGHT THREE QUARTERS UPPER ONE QUARTER BLOCK
        0x1CEAA => (1, 4, 0, 1),
        // 𜺫 RIGHT HALF UPPER ONE QUARTER BLOCK
        0x1CEAB => (2, 4, 0, 1),
        // 𜺬 UPPER HALF RIGHT ONE QUARTER BLOCK
        0x1CEAC => (3, 4, 0, 2),
        // 𜺭 UPPER THREE QUARTERS RIGHT ONE QUARTER BLOCK
        0x1CEAD => (3, 4, 0, 3),
        // 𜺮 LOWER THREE QUARTERS RIGHT ONE QUARTER BLOCK
        0x1CEAE => (3, 4, 1, 4),
        // 𜺯 LOWER HALF RIGHT ONE QUARTER BLOCK
        0x1CEAF => (3, 4, 2, 4),

        _ => return,
    };
    fill(metrics, canvas, q[x0], q[x1], q[y0], q[y1]);
}

/// One quarter of an ellipse spanning `w`×`h` cells, drawn into the cell at
/// (`x`, `y`) within that span and clipped to the cell.
#[allow(clippy::too_many_arguments)]
fn circle_piece(
    canvas: &mut Canvas,
    width: u32,
    height: u32,
    metrics: &Metrics,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    corner: Corner,
) {
    // Radius in pixels of the arc we need to draw.
    let wdth = width as f64 * w;
    let hght = height as f64 * h;

    // Position in pixels (rather than cells) for x/y.
    let xp = width as f64 * x;
    let yp = height as f64 * y;

    // Set the clip so we don't include anything outside of the cell.
    canvas.clip_left = canvas.padding_x;
    canvas.clip_right = canvas.padding_x;
    canvas.clip_top = canvas.padding_y;
    canvas.clip_bottom = canvas.padding_y;

    // Coefficient for approximating a circular arc.
    let c = (std::f64::consts::SQRT_2 - 1.0) * 4.0 / 3.0;
    let cw = c * wdth;
    let ch = c * hght;

    let thick = metrics.box_thickness as f64;
    let ht = thick * 0.5;

    let mut pb = PathBuilder::new();
    let mv = |pb: &mut PathBuilder, x: f64, y: f64| pb.move_to(x as f32, y as f32);
    let cubic = |pb: &mut PathBuilder, x1: f64, y1: f64, x2: f64, y2: f64, x: f64, y: f64| {
        pb.cubic_to(
            x1 as f32, y1 as f32, x2 as f32, y2 as f32, x as f32, y as f32,
        )
    };

    match corner {
        Corner::Tl => {
            mv(&mut pb, wdth - xp, ht - yp);
            cubic(
                &mut pb,
                wdth - cw - xp,
                ht - yp,
                ht - xp,
                hght - ch - yp,
                ht - xp,
                hght - yp,
            );
        }
        Corner::Tr => {
            mv(&mut pb, wdth - xp, ht - yp);
            cubic(
                &mut pb,
                wdth + cw - xp,
                ht - yp,
                wdth * 2.0 - ht - xp,
                hght - ch - yp,
                wdth * 2.0 - ht - xp,
                hght - yp,
            );
        }
        Corner::Bl => {
            mv(&mut pb, ht - xp, hght - yp);
            cubic(
                &mut pb,
                ht - xp,
                hght + ch - yp,
                wdth - cw - xp,
                hght * 2.0 - ht - yp,
                wdth - xp,
                hght * 2.0 - ht - yp,
            );
        }
        Corner::Br => {
            mv(&mut pb, wdth * 2.0 - ht - xp, hght - yp);
            cubic(
                &mut pb,
                wdth * 2.0 - ht - xp,
                hght + ch - yp,
                wdth + cw - xp,
                hght * 2.0 - ht - yp,
                wdth - xp,
                hght * 2.0 - ht - yp,
            );
        }
    }

    if let Some(path) = pb.finish() {
        canvas.stroke_path(&path, &canvas::stroke(thick), ON);
    }
}
