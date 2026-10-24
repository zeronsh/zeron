//! Port of Ghostty's src/font/sprite/draw/braille.zig.
//! Braille Patterns | U+2800...U+28FF
//!
//! The low byte of the codepoint is the dot pattern: bit 0 top-left, 1
//! upper-left, 2 lower-left, 3 top-right, 4 upper-right, 5 lower-right,
//! 6 bottom-left, 7 bottom-right.

use super::Metrics;
use super::canvas::{Canvas, ON};

pub fn draw_2800_28ff(cp: u32, canvas: &mut Canvas, width: u32, height: u32, _metrics: &Metrics) {
    let mut w = (width / 4).min(height / 8) as i32;
    let mut x_spacing = (width / 4) as i32;
    let mut y_spacing = (height / 8) as i32;
    let mut x_margin = x_spacing.div_euclid(2);
    let mut y_margin = y_spacing.div_euclid(2);

    let mut x_px_left = width as i32 - 2 * x_margin - x_spacing - 2 * w;
    let mut y_px_left = height as i32 - 2 * y_margin - 3 * y_spacing - 4 * w;

    // First, try hard to ensure the dot width is non-zero.
    if x_px_left >= 2 && y_px_left >= 4 && w == 0 {
        w += 1;
        x_px_left -= 2;
        y_px_left -= 4;
    }

    // Second, prefer a non-zero margin.
    if x_px_left >= 2 && x_margin == 0 {
        x_margin = 1;
        x_px_left -= 2;
    }
    if y_px_left >= 2 && y_margin == 0 {
        y_margin = 1;
        y_px_left -= 2;
    }

    // Third, increase spacing.
    if x_px_left >= 1 {
        x_spacing += 1;
        x_px_left -= 1;
    }
    if y_px_left >= 3 {
        y_spacing += 1;
        y_px_left -= 3;
    }

    // Fourth, margins ("spacing", but on the sides).
    if x_px_left >= 2 {
        x_margin += 1;
        x_px_left -= 2;
    }
    if y_px_left >= 2 {
        y_margin += 1;
        y_px_left -= 2;
    }

    // Last, increase dot width.
    if x_px_left >= 2 && y_px_left >= 4 {
        w += 1;
    }

    let x = [x_margin, x_margin + w + x_spacing];
    let mut y = [y_margin; 4];
    for i in 1..4 {
        y[i] = y[i - 1] + w + y_spacing;
    }

    // (bit, column, row)
    const DOTS: [(u32, usize, usize); 8] = [
        (0, 0, 0),
        (1, 0, 1),
        (2, 0, 2),
        (3, 1, 0),
        (4, 1, 1),
        (5, 1, 2),
        (6, 0, 3),
        (7, 1, 3),
    ];
    for (bit, col, row) in DOTS {
        if cp & (1 << bit) != 0 {
            canvas.box_(x[col], y[row], x[col] + w, y[row] + w, ON);
        }
    }
}
