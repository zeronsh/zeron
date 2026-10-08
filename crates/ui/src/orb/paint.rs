// Extracted from Bezel 6141af9c16f7353cdf36003f7404e0a94566a163; MIT. See THIRD_PARTY_NOTICES.md.
//! Paint a backend-agnostic [`Frame`] into a GPUI window.

use gpui::{
    BorderStyle, Bounds, PathBuilder, Pixels, Point, Window, opaque_grey, point, px, quad, size,
    transparent_black,
};
use zeron_orb::engine::{Frame, Line};

/// Map ink `white` + substrate into a grayscale with alpha.
fn ink_color(white: f32, alpha: f32, dark: bool) -> gpui::Hsla {
    let w = white.clamp(0.0, 1.0);
    let lightness = if dark { 1.0 - w } else { w };
    opaque_grey(lightness, alpha.clamp(0.0, 1.0))
}

/// Paint every stroke as its own `Path`, at its exact colour.
///
/// Merging segments into shared `PathBuilder`s was tried and reverted. It is
/// tempting — a single builder accepts any number of disjoint subpaths, and
/// each `Path` primitive costs GPUI a full two-pass render — but it visibly
/// brightens the connecting state's web.
///
/// The cause is that GPUI composites paths in two stages with different blend
/// modes: subpaths of one path are rasterised together into a cleared
/// intermediate texture with premultiplied-alpha blending, and that texture is
/// then blitted with `color: OVER, alpha: ADDITIVE`. So where strokes overlap,
/// merging changes which stage accumulates them, and the result does not match
/// painting each segment separately. Merging also forces a shared colour per
/// path, which costs another approximation on top.
///
/// Measured: batching saves nothing at one orb (2.0 % either way) and about one
/// point of a core at twelve. Not worth a visible change to the artwork.
fn paint_lines(window: &mut Window, origin: Point<Pixels>, lines: &[Line], dark: bool, scale: f32) {
    for l in lines {
        if l.a < 0.02 {
            continue;
        }
        let mut b = PathBuilder::stroke(px(l.w * scale));
        b.move_to(point(
            origin.x + px(l.x1 * scale),
            origin.y + px(l.y1 * scale),
        ));
        b.line_to(point(
            origin.x + px(l.x2 * scale),
            origin.y + px(l.y2 * scale),
        ));
        if let Ok(path) = b.build() {
            window.paint_path(path, ink_color(l.white, l.a, dark));
        }
    }
}

/// Paint dots as rounded quads (true circles at GPU level via corner radii).
fn paint_dots(
    window: &mut Window,
    origin: Point<Pixels>,
    frame: &Frame,
    dark: bool,
    r_min: f32,
    scale: f32,
) {
    for d in &frame.dots {
        if d.a < 0.02 {
            continue;
        }
        let r = d.r.max(r_min) * scale;
        let diameter = r * 2.0;
        let bounds = Bounds {
            origin: point(
                origin.x + px(d.x * scale - r),
                origin.y + px(d.y * scale - r),
            ),
            size: size(px(diameter), px(diameter)),
        };
        // Fully rounded corners → disk.
        window.paint_quad(quad(
            bounds,
            px(r),
            ink_color(d.white, d.a, dark),
            px(0.),
            transparent_black(),
            BorderStyle::default(),
        ));
    }
}

/// Verification scaffold: `ORB_NO_LAYER=1` paints without the `paint_layer`
/// wrap, i.e. the pre-optimization path. `scripts/golden-diff.sh` uses it to
/// render both paths from one binary and diff the results pixel-for-pixel.
/// Read once; costs a single relaxed atomic load per frame.
fn no_layer() -> bool {
    static N: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *N.get_or_init(|| std::env::var("ORB_NO_LAYER").as_deref() == Ok("1"))
}

/// Paint a complete frame into `bounds` (top-left of the orb).
///
/// Background is fully transparent — the host supplies the substrate.
/// `scale` magnifies the preset's geometry uniformly (dots, strokes and
/// spacing), so a preset tuned at one edge length stays crisp at another.
pub fn paint_frame(
    window: &mut Window,
    bounds: Bounds<Pixels>,
    frame: &Frame,
    dark: bool,
    r_min: f32,
    scale: f32,
) {
    if no_layer() {
        paint_lines(window, bounds.origin, &frame.lines, dark, scale);
        paint_dots(window, bounds.origin, frame, dark, r_min, scale);
        return;
    }

    // Everything is painted inside one layer, which is the single biggest win
    // in the paint path. Outside a layer, `Scene::insert_primitive` calls
    // `BoundsTree::insert` for *every* primitive to compute its draw order —
    // a bounding-volume-hierarchy insertion whose `find_max_ordering` walk
    // degenerates when primitives overlap heavily, which is exactly the case
    // here (hundreds of discs stacked inside a 64px box). Inside a layer the
    // tree is touched once and every primitive inherits the layer's order.
    //
    // Ordering is preserved: `Scene::finish` sorts with `sort_by_key`, which is
    // stable, so quads sharing the layer's order keep their insertion order —
    // and the engine already emits dots back-to-front. The painter's algorithm
    // still holds. Confirmed by pixel-diffing the golden grid against a build
    // with this call removed: every dot-only orb came out byte-identical.
    window.paint_layer(bounds, |window| {
        paint_lines(window, bounds.origin, &frame.lines, dark, scale);
        paint_dots(window, bounds.origin, frame, dark, r_min, scale);
    });
}
