//! Port of Ghostty's src/font/sprite/draw/powerline.zig.
//! Powerline + Powerline Extra Symbols | U+E0B0...U+E0D4
//!
//! The geometric glyphs only, not the stylized ones.

use tiny_skia::PathBuilder;

use super::Metrics;
use super::box_drawing;
use super::canvas::{Canvas, ON, pt, stroke};
use super::common::Thickness;
use super::geometric_shapes::inner_stroke_polyline;

/// Coefficient for approximating a quarter circle with a cubic Bézier.
const ARC_C: f64 = (std::f64::consts::SQRT_2 - 1.0) * 4.0 / 3.0;

/// U+E0B0
pub fn draw_e0b0(_cp: u32, canvas: &mut Canvas, width: u32, height: u32, _metrics: &Metrics) {
    let (w, h) = (width as f64, height as f64);
    canvas.triangle([pt(0.0, 0.0), pt(w, h / 2.0), pt(0.0, h)], ON);
}

/// U+E0B1
pub fn draw_e0b1(_cp: u32, canvas: &mut Canvas, width: u32, height: u32, metrics: &Metrics) {
    let (w, h) = (width as f32, height as f32);
    let mut pb = PathBuilder::new();
    pb.move_to(0.0, 0.0);
    pb.line_to(w, h / 2.0);
    pb.line_to(0.0, h);
    if let Some(path) = pb.finish() {
        let thick = Thickness::Light.height(metrics.box_thickness) as f64;
        canvas.stroke_path(&path, &stroke(thick), ON);
    }
}

/// U+E0B2
pub fn draw_e0b2(_cp: u32, canvas: &mut Canvas, width: u32, height: u32, _metrics: &Metrics) {
    let (w, h) = (width as f64, height as f64);
    canvas.triangle([pt(w, 0.0), pt(0.0, h / 2.0), pt(w, h)], ON);
}

/// U+E0B3
pub fn draw_e0b3(cp: u32, canvas: &mut Canvas, width: u32, height: u32, metrics: &Metrics) {
    draw_e0b1(cp, canvas, width, height, metrics);
    canvas.flip_horizontal();
}

/// U+E0B4
pub fn draw_e0b4(_cp: u32, canvas: &mut Canvas, width: u32, height: u32, _metrics: &Metrics) {
    let (w, h) = (width as f64, height as f64);
    let r = w.min(h / 2.0);
    let c = ARC_C;
    let f = |v: f64| v as f32;
    let mut pb = PathBuilder::new();
    pb.move_to(0.0, 0.0);
    pb.cubic_to(f(r * c), 0.0, f(r), f(r - r * c), f(r), f(r));
    pb.line_to(f(r), f(h - r));
    pb.cubic_to(f(r), f(h - r + r * c), f(r * c), f(h), 0.0, f(h));
    pb.close();
    if let Some(path) = pb.finish() {
        canvas.fill_path(&path, ON);
    }
}

/// U+E0B5
pub fn draw_e0b5(_cp: u32, canvas: &mut Canvas, width: u32, height: u32, metrics: &Metrics) {
    let (w, h) = (width as f64, height as f64);
    let r = w.min(h / 2.0);
    let c = ARC_C;
    // The first/final straight moves keep both ends horizontal, so the butt
    // caps are perpendicular (see Ghostty). The curves are flattened finely
    // since the inset is computed on a polyline.
    let mut points = vec![(0.0, 0.0), (1.0, 0.0)];
    cubic_points(
        &mut points,
        (1.0, 0.0),
        (r * c, 0.0),
        (r, r - r * c),
        (r, r),
    );
    points.push((r, h - r));
    cubic_points(
        &mut points,
        (r, h - r),
        (r, h - r + r * c),
        (r * c, h),
        (1.0, h),
    );
    points.push((0.0, h));
    inner_stroke_polyline(canvas, &points, false, metrics.box_thickness as f64, ON);
}

/// Append the flattened cubic from `p0` (already in `out`) to `p3`.
fn cubic_points(
    out: &mut Vec<(f64, f64)>,
    p0: (f64, f64),
    p1: (f64, f64),
    p2: (f64, f64),
    p3: (f64, f64),
) {
    const STEPS: usize = 64;
    for i in 1..=STEPS {
        let t = i as f64 / STEPS as f64;
        let u = 1.0 - t;
        let (a, b, cc, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
        out.push((
            a * p0.0 + b * p1.0 + cc * p2.0 + d * p3.0,
            a * p0.1 + b * p1.1 + cc * p2.1 + d * p3.1,
        ));
    }
}

/// U+E0B6
pub fn draw_e0b6(cp: u32, canvas: &mut Canvas, width: u32, height: u32, metrics: &Metrics) {
    draw_e0b4(cp, canvas, width, height, metrics);
    canvas.flip_horizontal();
}

/// U+E0B7
pub fn draw_e0b7(cp: u32, canvas: &mut Canvas, width: u32, height: u32, metrics: &Metrics) {
    draw_e0b5(cp, canvas, width, height, metrics);
    canvas.flip_horizontal();
}

/// U+E0B8
pub fn draw_e0b8(_cp: u32, canvas: &mut Canvas, width: u32, height: u32, _metrics: &Metrics) {
    let (w, h) = (width as f64, height as f64);
    canvas.triangle([pt(0.0, 0.0), pt(w, h), pt(0.0, h)], ON);
}

/// U+E0B9
pub fn draw_e0b9(_cp: u32, canvas: &mut Canvas, _width: u32, _height: u32, metrics: &Metrics) {
    box_drawing::light_diagonal_upper_left_to_lower_right(metrics, canvas);
}

/// U+E0BA
pub fn draw_e0ba(_cp: u32, canvas: &mut Canvas, width: u32, height: u32, _metrics: &Metrics) {
    let (w, h) = (width as f64, height as f64);
    canvas.triangle([pt(w, 0.0), pt(w, h), pt(0.0, h)], ON);
}

/// U+E0BB
pub fn draw_e0bb(_cp: u32, canvas: &mut Canvas, _width: u32, _height: u32, metrics: &Metrics) {
    box_drawing::light_diagonal_upper_right_to_lower_left(metrics, canvas);
}

/// U+E0BC
pub fn draw_e0bc(_cp: u32, canvas: &mut Canvas, width: u32, height: u32, _metrics: &Metrics) {
    let (w, h) = (width as f64, height as f64);
    canvas.triangle([pt(0.0, 0.0), pt(w, 0.0), pt(0.0, h)], ON);
}

/// U+E0BD
pub fn draw_e0bd(_cp: u32, canvas: &mut Canvas, _width: u32, _height: u32, metrics: &Metrics) {
    box_drawing::light_diagonal_upper_right_to_lower_left(metrics, canvas);
}

/// U+E0BE
pub fn draw_e0be(_cp: u32, canvas: &mut Canvas, width: u32, height: u32, _metrics: &Metrics) {
    let (w, h) = (width as f64, height as f64);
    canvas.triangle([pt(0.0, 0.0), pt(w, 0.0), pt(w, h)], ON);
}

/// U+E0BF
pub fn draw_e0bf(_cp: u32, canvas: &mut Canvas, _width: u32, _height: u32, metrics: &Metrics) {
    box_drawing::light_diagonal_upper_left_to_lower_right(metrics, canvas);
}

/// U+E0D2
pub fn draw_e0d2(_cp: u32, canvas: &mut Canvas, width: u32, height: u32, metrics: &Metrics) {
    let (w, h) = (width as f64, height as f64);
    let t = metrics.box_thickness as f64;
    // Top piece, then bottom piece.
    canvas.quad(
        [
            pt(0.0, 0.0),
            pt(w, 0.0),
            pt(w / 2.0, h / 2.0 - t / 2.0),
            pt(0.0, h / 2.0 - t / 2.0),
        ],
        ON,
    );
    canvas.quad(
        [
            pt(0.0, h),
            pt(w, h),
            pt(w / 2.0, h / 2.0 + t / 2.0),
            pt(0.0, h / 2.0 + t / 2.0),
        ],
        ON,
    );
}

/// U+E0D4
pub fn draw_e0d4(cp: u32, canvas: &mut Canvas, width: u32, height: u32, metrics: &Metrics) {
    draw_e0d2(cp, canvas, width, height, metrics);
    canvas.flip_horizontal();
}
