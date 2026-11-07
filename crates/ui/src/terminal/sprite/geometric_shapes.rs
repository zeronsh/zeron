//! Port of Ghostty's src/font/sprite/draw/geometric_shapes.zig.
//! Geometric Shapes | U+25A0...U+25FF
//!
//! Only a subset of this block is drawn here: ◢ ◣ ◤ ◥ ◸ ◹ ◺ ◿

use tiny_skia::PathBuilder;

use super::Metrics;
use super::canvas::{Canvas, ON, stroke};
use super::common::{Corner, Shade, Thickness};

/// ◢ ◣ ◤ ◥
pub fn draw_25e2_25e5(cp: u32, canvas: &mut Canvas, _width: u32, _height: u32, metrics: &Metrics) {
    match cp {
        0x25e2 => corner_triangle_shade(metrics, canvas, Corner::Br, Shade::On),
        0x25e3 => corner_triangle_shade(metrics, canvas, Corner::Bl, Shade::On),
        0x25e4 => corner_triangle_shade(metrics, canvas, Corner::Tl, Shade::On),
        0x25e5 => corner_triangle_shade(metrics, canvas, Corner::Tr, Shade::On),
        _ => {}
    }
}

/// ◸ ◹ ◺
pub fn draw_25f8_25fa(cp: u32, canvas: &mut Canvas, _width: u32, _height: u32, metrics: &Metrics) {
    match cp {
        0x25f8 => corner_triangle_outline(metrics, canvas, Corner::Tl),
        0x25f9 => corner_triangle_outline(metrics, canvas, Corner::Tr),
        0x25fa => corner_triangle_outline(metrics, canvas, Corner::Bl),
        _ => {}
    }
}

/// ◿
pub fn draw_25ff(_cp: u32, canvas: &mut Canvas, _width: u32, _height: u32, metrics: &Metrics) {
    corner_triangle_outline(metrics, canvas, Corner::Br);
}

/// The triangle filling half the cell, with its right angle at `corner`.
fn corner_triangle(metrics: &Metrics, corner: Corner) -> [(f64, f64); 3] {
    let w = metrics.cell_width as f64;
    let h = metrics.cell_height as f64;
    match corner {
        Corner::Tl => [(0.0, 0.0), (0.0, h), (w, 0.0)],
        Corner::Tr => [(0.0, 0.0), (w, h), (w, 0.0)],
        Corner::Bl => [(0.0, 0.0), (0.0, h), (w, h)],
        Corner::Br => [(0.0, h), (w, h), (w, 0.0)],
    }
}

pub fn corner_triangle_shade(metrics: &Metrics, canvas: &mut Canvas, corner: Corner, shade: Shade) {
    let p = corner_triangle(metrics, corner);
    let mut pb = PathBuilder::new();
    pb.move_to(p[0].0 as f32, p[0].1 as f32);
    pb.line_to(p[1].0 as f32, p[1].1 as f32);
    pb.line_to(p[2].0 as f32, p[2].1 as f32);
    pb.close();
    if let Some(path) = pb.finish() {
        canvas.fill_path(&path, shade.into());
    }
}

pub fn corner_triangle_outline(metrics: &Metrics, canvas: &mut Canvas, corner: Corner) {
    let thick = Thickness::Light.height(metrics.box_thickness) as f64;
    inner_stroke_polyline(canvas, &corner_triangle(metrics, corner), true, thick, ON);
}

/// Ghostty's `Canvas.innerStrokePath` for a path of straight segments:
/// strokes (butt caps, miter joins) the polyline offset inward by
/// `width / 2`, so the stroke's outer edge lies on the original path.
/// "Inward" is the side the polyline turns towards (the interior of a
/// closed shape). Curves must be flattened by the caller.
pub(super) fn inner_stroke_polyline(
    canvas: &mut Canvas,
    points: &[(f64, f64)],
    closed: bool,
    width: f64,
    color: u8,
) {
    let mut pts: Vec<(f64, f64)> = Vec::with_capacity(points.len());
    for &p in points {
        if pts
            .last()
            .is_none_or(|&q: &(f64, f64)| (q.0 - p.0).hypot(q.1 - p.1) > 1e-9)
        {
            pts.push(p);
        }
    }
    if closed && pts.len() > 1 && pts[0] == pts[pts.len() - 1] {
        pts.pop();
    }
    let n = pts.len();
    if n < 2 {
        return;
    }
    let seg_count = if closed { n } else { n - 1 };

    // Which side is inside: the sign of the signed area (y down: positive
    // area means clockwise on screen, interior to the right of travel).
    let mut area = 0.0;
    for i in 0..n {
        let (a, b) = (pts[i], pts[(i + 1) % n]);
        area += a.0 * b.1 - b.0 * a.1;
    }
    let d = if area >= 0.0 {
        width / 2.0
    } else {
        -width / 2.0
    };

    // Each segment offset along its right-hand normal (−dy, dx) by `d`.
    let seg = |i: usize| -> ((f64, f64), (f64, f64)) {
        let (a, b) = (pts[i], pts[(i + 1) % n]);
        let len = (b.0 - a.0).hypot(b.1 - a.1);
        let (nx, ny) = (-(b.1 - a.1) / len * d, (b.0 - a.0) / len * d);
        ((a.0 + nx, a.1 + ny), (b.0 + nx, b.1 + ny))
    };
    // Intersection of two offset segments' lines, falling back to the end
    // of the first when they are parallel.
    let join = |s0: ((f64, f64), (f64, f64)), s1: ((f64, f64), (f64, f64))| -> (f64, f64) {
        let (p, r) = (s0.0, (s0.1.0 - s0.0.0, s0.1.1 - s0.0.1));
        let (q, s) = (s1.0, (s1.1.0 - s1.0.0, s1.1.1 - s1.0.1));
        let denom = r.0 * s.1 - r.1 * s.0;
        if denom.abs() < 1e-12 {
            return s0.1;
        }
        let t = ((q.0 - p.0) * s.1 - (q.1 - p.1) * s.0) / denom;
        (p.0 + t * r.0, p.1 + t * r.1)
    };

    let mut out = Vec::with_capacity(n);
    if closed {
        for i in 0..n {
            out.push(join(seg((i + n - 1) % n), seg(i)));
        }
    } else {
        out.push(seg(0).0);
        for i in 1..seg_count {
            out.push(join(seg(i - 1), seg(i)));
        }
        out.push(seg(seg_count - 1).1);
    }

    let mut pb = PathBuilder::new();
    pb.move_to(out[0].0 as f32, out[0].1 as f32);
    for p in &out[1..] {
        pb.line_to(p.0 as f32, p.1 as f32);
    }
    if closed {
        pb.close();
    }
    if let Some(path) = pb.finish() {
        canvas.stroke_path(&path, &stroke(width), color);
    }
}
