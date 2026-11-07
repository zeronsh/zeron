//! Port of Ghostty's src/font/sprite/draw/branch.zig.
//! Branch Drawing Characters | U+F5D0...U+F60D
//!
//! Branch drawing character set, used for drawing git-like graphs in the
//! terminal. Originally implemented in Kitty.
//! Ref:
//! - https://github.com/kovidgoyal/kitty/pull/7681
//! - https://github.com/kovidgoyal/kitty/pull/7805

use tiny_skia::PathBuilder;

use super::Metrics;
use super::box_drawing::arc;
use super::canvas::{self, Canvas};
use super::common::{Corner, Edge, Shade, Thickness, hline_middle, vline_middle};

/// A branch drawing node: a circle, empty or filled, with lines optionally
/// connecting it to each of the 4 edges.
#[derive(Clone, Copy, Debug, Default)]
struct BranchNode {
    up: bool,
    right: bool,
    down: bool,
    left: bool,
    filled: bool,
}

pub fn draw_f5d0_f60d(cp: u32, canvas: &mut Canvas, _width: u32, _height: u32, metrics: &Metrics) {
    use Corner::{Bl, Br, Tl, Tr};
    let m = metrics;
    let c = canvas;
    let l = Thickness::Light;
    let n = |up, right, down, left, filled| BranchNode {
        up,
        right,
        down,
        left,
        filled,
    };
    match cp {
        0xf5d0 => hline_middle(m, c, l),
        0xf5d1 => vline_middle(m, c, l),
        0xf5d2 => fading_line(m, c, Edge::Right, l),
        0xf5d3 => fading_line(m, c, Edge::Left, l),
        0xf5d4 => fading_line(m, c, Edge::Bottom, l),
        0xf5d5 => fading_line(m, c, Edge::Top, l),
        0xf5d6 => arc(m, c, Br, l),
        0xf5d7 => arc(m, c, Bl, l),
        0xf5d8 => arc(m, c, Tr, l),
        0xf5d9 => arc(m, c, Tl, l),
        0xf5da => {
            vline_middle(m, c, l);
            arc(m, c, Tr, l);
        }
        0xf5db => {
            vline_middle(m, c, l);
            arc(m, c, Br, l);
        }
        0xf5dc => {
            arc(m, c, Tr, l);
            arc(m, c, Br, l);
        }
        0xf5dd => {
            vline_middle(m, c, l);
            arc(m, c, Tl, l);
        }
        0xf5de => {
            vline_middle(m, c, l);
            arc(m, c, Bl, l);
        }
        0xf5df => {
            arc(m, c, Tl, l);
            arc(m, c, Bl, l);
        }
        0xf5e0 => {
            arc(m, c, Bl, l);
            hline_middle(m, c, l);
        }
        0xf5e1 => {
            arc(m, c, Br, l);
            hline_middle(m, c, l);
        }
        0xf5e2 => {
            arc(m, c, Br, l);
            arc(m, c, Bl, l);
        }
        0xf5e3 => {
            arc(m, c, Tl, l);
            hline_middle(m, c, l);
        }
        0xf5e4 => {
            arc(m, c, Tr, l);
            hline_middle(m, c, l);
        }
        0xf5e5 => {
            arc(m, c, Tr, l);
            arc(m, c, Tl, l);
        }
        0xf5e6 => {
            vline_middle(m, c, l);
            arc(m, c, Tl, l);
            arc(m, c, Tr, l);
        }
        0xf5e7 => {
            vline_middle(m, c, l);
            arc(m, c, Bl, l);
            arc(m, c, Br, l);
        }
        0xf5e8 => {
            hline_middle(m, c, l);
            arc(m, c, Bl, l);
            arc(m, c, Tl, l);
        }
        0xf5e9 => {
            hline_middle(m, c, l);
            arc(m, c, Tr, l);
            arc(m, c, Br, l);
        }
        0xf5ea => {
            vline_middle(m, c, l);
            arc(m, c, Tl, l);
            arc(m, c, Br, l);
        }
        0xf5eb => {
            vline_middle(m, c, l);
            arc(m, c, Tr, l);
            arc(m, c, Bl, l);
        }
        0xf5ec => {
            hline_middle(m, c, l);
            arc(m, c, Tl, l);
            arc(m, c, Br, l);
        }
        0xf5ed => {
            hline_middle(m, c, l);
            arc(m, c, Tr, l);
            arc(m, c, Bl, l);
        }
        //                             up     right  down   left   filled
        0xf5ee => branch_node(m, c, n(false, false, false, false, true), l),
        0xf5ef => branch_node(m, c, n(false, false, false, false, false), l),
        0xf5f0 => branch_node(m, c, n(false, true, false, false, true), l),
        0xf5f1 => branch_node(m, c, n(false, true, false, false, false), l),
        0xf5f2 => branch_node(m, c, n(false, false, false, true, true), l),
        0xf5f3 => branch_node(m, c, n(false, false, false, true, false), l),
        0xf5f4 => branch_node(m, c, n(false, true, false, true, true), l),
        0xf5f5 => branch_node(m, c, n(false, true, false, true, false), l),
        0xf5f6 => branch_node(m, c, n(false, false, true, false, true), l),
        0xf5f7 => branch_node(m, c, n(false, false, true, false, false), l),
        0xf5f8 => branch_node(m, c, n(true, false, false, false, true), l),
        0xf5f9 => branch_node(m, c, n(true, false, false, false, false), l),
        0xf5fa => branch_node(m, c, n(true, false, true, false, true), l),
        0xf5fb => branch_node(m, c, n(true, false, true, false, false), l),
        0xf5fc => branch_node(m, c, n(false, true, true, false, true), l),
        0xf5fd => branch_node(m, c, n(false, true, true, false, false), l),
        0xf5fe => branch_node(m, c, n(false, false, true, true, true), l),
        0xf5ff => branch_node(m, c, n(false, false, true, true, false), l),
        0xf600 => branch_node(m, c, n(true, true, false, false, true), l),
        0xf601 => branch_node(m, c, n(true, true, false, false, false), l),
        0xf602 => branch_node(m, c, n(true, false, false, true, true), l),
        0xf603 => branch_node(m, c, n(true, false, false, true, false), l),
        0xf604 => branch_node(m, c, n(true, true, true, false, true), l),
        0xf605 => branch_node(m, c, n(true, true, true, false, false), l),
        0xf606 => branch_node(m, c, n(true, false, true, true, true), l),
        0xf607 => branch_node(m, c, n(true, false, true, true, false), l),
        0xf608 => branch_node(m, c, n(false, true, true, true, true), l),
        0xf609 => branch_node(m, c, n(false, true, true, true, false), l),
        0xf60a => branch_node(m, c, n(true, true, false, true, true), l),
        0xf60b => branch_node(m, c, n(true, true, false, true, false), l),
        0xf60c => branch_node(m, c, n(true, true, true, true, true), l),
        0xf60d => branch_node(m, c, n(true, true, true, true, false), l),
        _ => {}
    }
}

fn branch_node(metrics: &Metrics, canvas: &mut Canvas, node: BranchNode, thickness: Thickness) {
    let thick_px = thickness.height(metrics.box_thickness);
    let float_width = metrics.cell_width as f64;
    let float_height = metrics.cell_height as f64;
    let float_thick = thick_px as f64;

    // Top / bottom of horizontal strokes.
    let h_top = metrics.cell_height.saturating_sub(thick_px) / 2;
    let h_bottom = h_top.saturating_add(thick_px);
    // Left / right of vertical strokes.
    let v_left = metrics.cell_width.saturating_sub(thick_px) / 2;
    let v_right = v_left.saturating_add(thick_px);

    // The center is computed this way to align with the box drawing lines,
    // which are sometimes off center to stay on whole pixels.
    let cx = v_left as f64 + float_thick / 2.0;
    let cy = h_top as f64 + float_thick / 2.0;
    // The radius is the smallest distance from the center to an edge.
    let r = cx.min(cy).min((float_width - cx).min(float_height - cy));

    if node.up {
        canvas.box_(
            v_left as i32,
            0,
            v_right as i32,
            (cy - r + float_thick / 2.0).ceil() as i32,
            canvas::ON,
        );
    }
    if node.right {
        canvas.box_(
            (cx + r - float_thick / 2.0).floor() as i32,
            h_top as i32,
            metrics.cell_width as i32,
            h_bottom as i32,
            canvas::ON,
        );
    }
    if node.down {
        canvas.box_(
            v_left as i32,
            (cy + r - float_thick / 2.0).floor() as i32,
            v_right as i32,
            metrics.cell_height as i32,
            canvas::ON,
        );
    }
    if node.left {
        canvas.box_(
            0,
            h_top as i32,
            (cx - r + float_thick / 2.0).ceil() as i32,
            h_bottom as i32,
            canvas::ON,
        );
    }

    let on: canvas::Color = Shade::On.into();
    let mut pb = PathBuilder::new();
    if node.filled {
        pb.push_circle(cx as f32, cy as f32, r as f32);
        if let Some(path) = pb.finish() {
            canvas.fill_path(&path, on);
        }
    } else {
        pb.push_circle(cx as f32, cy as f32, (r - float_thick / 2.0) as f32);
        // Fill the stroke's outline rather than `stroke_path`: tiny-skia
        // draws strokes 1px wide or thinner as alpha-scaled hairlines, while
        // z2d always fills the stroke outline with area coverage.
        if let Some(outline) = pb
            .finish()
            .and_then(|p| p.stroke(&canvas::stroke(float_thick), 1.0))
        {
            canvas.fill_path(&outline, on);
        }
    }
}

fn fading_line(metrics: &Metrics, canvas: &mut Canvas, to: Edge, thickness: Thickness) {
    let thick_px = thickness.height(metrics.box_thickness);
    let float_width = metrics.cell_width as f64;
    let float_height = metrics.cell_height as f64;

    let h_top = metrics.cell_height.saturating_sub(thick_px) / 2;
    let h_bottom = h_top.saturating_add(thick_px);
    let v_left = metrics.cell_width.saturating_sub(thick_px) / 2;
    let v_right = v_left.saturating_add(thick_px);

    // Fading to the top or left starts at 0 and counts up; otherwise it
    // starts at 255 and counts down.
    let mut color: f64 = match to {
        Edge::Top | Edge::Left => 0.0,
        Edge::Bottom | Edge::Right => 255.0,
    };
    let inc: f64 = 255.0
        / match to {
            Edge::Top => float_height,
            Edge::Bottom => -float_height,
            Edge::Left => float_width,
            Edge::Right => -float_width,
        };

    match to {
        Edge::Top | Edge::Bottom => {
            for y in 0..metrics.cell_height {
                for x in v_left..v_right {
                    canvas.pixel(x as i32, y as i32, color.round() as u8);
                }
                color += inc;
            }
        }
        Edge::Left | Edge::Right => {
            for x in 0..metrics.cell_width {
                for y in h_top..h_bottom {
                    canvas.pixel(x as i32, y as i32, color.round() as u8);
                }
                color += inc;
            }
        }
    }
}
