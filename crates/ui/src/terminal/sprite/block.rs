//! Port of Ghostty's src/font/sprite/draw/block.zig.
//! Block Elements | U+2580...U+259F
//!
//! ▀▁▂▃▄▅▆▇█▉▊▋▌▍▎▏
//! ▐░▒▓▔▕▖▗▘▙▚▛▜▝▞▟

use super::Metrics;
use super::canvas::{Canvas, Rect};
use super::common::{
    Alignment, FIVE_EIGHTHS, Fraction, HALF, Horizontal, ONE_EIGHTH, ONE_QUARTER, Quads,
    SEVEN_EIGHTHS, Shade, THREE_EIGHTHS, THREE_QUARTERS, Vertical, fill,
};

pub fn draw_2580_259f(cp: u32, canvas: &mut Canvas, _width: u32, _height: u32, metrics: &Metrics) {
    let m = metrics;
    let q = |tl, tr, bl, br| Quads { tl, tr, bl, br };
    match cp {
        0x2580 => block(m, canvas, Alignment::UPPER, 1.0, HALF),
        0x2581 => block(m, canvas, Alignment::LOWER, 1.0, ONE_EIGHTH),
        0x2582 => block(m, canvas, Alignment::LOWER, 1.0, ONE_QUARTER),
        0x2583 => block(m, canvas, Alignment::LOWER, 1.0, THREE_EIGHTHS),
        0x2584 => block(m, canvas, Alignment::LOWER, 1.0, HALF),
        0x2585 => block(m, canvas, Alignment::LOWER, 1.0, FIVE_EIGHTHS),
        0x2586 => block(m, canvas, Alignment::LOWER, 1.0, THREE_QUARTERS),
        0x2587 => block(m, canvas, Alignment::LOWER, 1.0, SEVEN_EIGHTHS),
        0x2588 => full_block_shade(m, canvas, Shade::On),
        0x2589 => block(m, canvas, Alignment::LEFT, SEVEN_EIGHTHS, 1.0),
        0x258a => block(m, canvas, Alignment::LEFT, THREE_QUARTERS, 1.0),
        0x258b => block(m, canvas, Alignment::LEFT, FIVE_EIGHTHS, 1.0),
        0x258c => block(m, canvas, Alignment::LEFT, HALF, 1.0),
        0x258d => block(m, canvas, Alignment::LEFT, THREE_EIGHTHS, 1.0),
        0x258e => block(m, canvas, Alignment::LEFT, ONE_QUARTER, 1.0),
        0x258f => block(m, canvas, Alignment::LEFT, ONE_EIGHTH, 1.0),
        0x2590 => block(m, canvas, Alignment::RIGHT, HALF, 1.0),
        0x2591 => full_block_shade(m, canvas, Shade::Light),
        0x2592 => full_block_shade(m, canvas, Shade::Medium),
        0x2593 => full_block_shade(m, canvas, Shade::Dark),
        0x2594 => block(m, canvas, Alignment::UPPER, 1.0, ONE_EIGHTH),
        0x2595 => block(m, canvas, Alignment::RIGHT, ONE_EIGHTH, 1.0),
        0x2596 => quadrant(m, canvas, q(false, false, true, false)),
        0x2597 => quadrant(m, canvas, q(false, false, false, true)),
        0x2598 => quadrant(m, canvas, q(true, false, false, false)),
        0x2599 => quadrant(m, canvas, q(true, false, true, true)),
        0x259a => quadrant(m, canvas, q(true, false, false, true)),
        0x259b => quadrant(m, canvas, q(true, true, true, false)),
        0x259c => quadrant(m, canvas, q(true, true, false, true)),
        0x259d => quadrant(m, canvas, q(false, true, false, false)),
        0x259e => quadrant(m, canvas, q(false, true, true, false)),
        0x259f => quadrant(m, canvas, q(false, true, true, true)),
        _ => {}
    }
}

pub fn block(
    metrics: &Metrics,
    canvas: &mut Canvas,
    alignment: Alignment,
    width: f64,
    height: f64,
) {
    block_shade(metrics, canvas, alignment, width, height, Shade::On);
}

pub fn block_shade(
    metrics: &Metrics,
    canvas: &mut Canvas,
    alignment: Alignment,
    width: f64,
    height: f64,
    shade: Shade,
) {
    let w = (metrics.cell_width as f64 * width).round() as u32;
    let h = (metrics.cell_height as f64 * height).round() as u32;
    let x = match alignment.horizontal {
        Horizontal::Left => 0,
        Horizontal::Right => metrics.cell_width - w,
        Horizontal::Center => (metrics.cell_width - w) / 2,
    };
    let y = match alignment.vertical {
        Vertical::Top => 0,
        Vertical::Bottom => metrics.cell_height - h,
        Vertical::Middle => (metrics.cell_height - h) / 2,
    };
    canvas.rect(
        Rect {
            x: x as i32,
            y: y as i32,
            width: w as i32,
            height: h as i32,
        },
        shade.into(),
    );
}

pub fn full_block_shade(metrics: &Metrics, canvas: &mut Canvas, shade: Shade) {
    canvas.box_(
        0,
        0,
        metrics.cell_width as i32,
        metrics.cell_height as i32,
        shade.into(),
    );
}

fn quadrant(metrics: &Metrics, canvas: &mut Canvas, quads: Quads) {
    use Fraction::{Half, One, Zero};
    if quads.tl {
        fill(metrics, canvas, Zero, Half, Zero, Half);
    }
    if quads.tr {
        fill(metrics, canvas, Half, One, Zero, Half);
    }
    if quads.bl {
        fill(metrics, canvas, Zero, Half, Half, One);
    }
    if quads.br {
        fill(metrics, canvas, Half, One, Half, One);
    }
}
