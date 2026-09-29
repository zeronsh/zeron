//! Glass surfaces shared by controls: each is a vertical gradient (lit from
//! above), a hairline rim, an inner top highlight, and a soft drop. Dark
//! appearances keep the structure with translucent white instead of opaque
//! fills. `light` is the neutral plate (a track), `accent` the active plate
//! (Stop, an enabled switch, a slider's fill) and `thumb` the opaque handle
//! that rides on either.
use gpui::{
    Background, Bounds, BoxShadow, Corners, Hsla, Pixels, Styled, Window, hsla, linear_color_stop,
    linear_gradient, point, px,
};

use crate::theme::Theme;

fn shadow(color: Hsla, y: f32, blur: f32, spread: f32, inset: bool) -> BoxShadow {
    BoxShadow {
        color,
        offset: point(px(0.0), px(y)),
        blur_radius: px(blur),
        spread_radius: px(spread),
        inset,
    }
}

fn white(alpha: f32) -> Hsla {
    hsla(0.0, 0.0, 1.0, alpha)
}

fn black(alpha: f32) -> Hsla {
    hsla(0.0, 0.0, 0.0, alpha)
}

fn vertical(top: Hsla, bottom: Hsla) -> Background {
    linear_gradient(
        180.0,
        linear_color_stop(top, 0.0),
        linear_color_stop(bottom, 1.0),
    )
}

/// A lighter tint of `color` for the lit top of a gradient.
fn lift(color: Hsla, amount: f32) -> Hsla {
    crate::motion::mix(color, white(color.a), amount)
}

/// One glass surface, applicable to an element or paintable in a canvas.
pub(crate) struct Plate {
    background: Background,
    rim: Hsla,
    shadows: Vec<BoxShadow>,
}

impl Plate {
    pub(crate) fn apply<E: Styled>(self, el: E) -> E {
        el.bg(self.background)
            .border_1()
            .border_color(self.rim)
            .shadow(self.shadows)
    }

    /// Paints the plate the way a styled element would: drops beneath the
    /// fill, the rim on it, and inset light above it.
    pub(crate) fn paint(&self, window: &mut Window, bounds: Bounds<Pixels>, radius: f32) {
        let corners = Corners::all(px(radius));
        let (inset, drop): (Vec<_>, Vec<_>) = self.shadows.iter().cloned().partition(|s| s.inset);
        window.paint_drop_shadows(bounds, corners, &drop);
        window.paint_quad(gpui::quad(
            bounds,
            corners,
            self.background,
            px(1.0),
            self.rim,
            gpui::BorderStyle::default(),
        ));
        window.paint_inset_shadows(bounds, corners, &inset);
    }
}

/// Neutral glass. `t` fades the whole treatment in. Dark appearances lift
/// translucent white off the surface; light ones sink a translucent tint
/// into it (shaded under the top edge, lit inside the bottom edge), since a
/// near-white plate on a near-white surface washes out.
pub(crate) fn light_plate(theme: &Theme, t: f32) -> Plate {
    if theme.appearance.is_dark() {
        return Plate {
            background: vertical(white(0.08 * t), white(0.05 * t)),
            rim: white(0.09 * t),
            shadows: vec![
                shadow(white(0.07 * t), 1.0, 0.0, 0.0, true),
                shadow(black(0.16 * t), 1.0, 2.0, 0.0, false),
            ],
        };
    }
    Plate {
        background: vertical(black(0.075 * t), black(0.04 * t)),
        rim: black(0.08 * t),
        // Inset only: GPUI paints drop shadows under the whole box, so an
        // outer lip would show through the translucent fill and whiten it.
        shadows: vec![
            shadow(black(0.08 * t), 1.0, 2.0, 0.0, true),
            shadow(white(0.55 * t), -1.0, 0.0, 0.0, true),
        ],
    }
}

/// Accent glass. `t` fades it in over the resting control; `glow` (0–1)
/// spreads its coloured shadow.
pub(crate) fn accent_plate(theme: &Theme, t: f32, glow: f32) -> Plate {
    let dark = theme.appearance.is_dark();
    let base = theme.accent;
    let top = lift(base, if dark { 0.10 } else { 0.22 });
    // Light: a defined edge a shade deeper than the fill, with a paler ring
    // just inside it. Dark: a soft lifted rim.
    let rim = if dark {
        lift(base, 0.35).opacity(0.35)
    } else {
        crate::motion::mix(base, black(1.0), 0.14)
    };
    let highlight = white(if dark { 0.14 } else { 0.32 });
    let ring = white(if dark { 0.08 } else { 0.22 });
    // Dark plates sit flat at rest; light ones keep a faint coloured glow.
    let halo = base.opacity((if dark { 0.0 } else { 0.14 } + 0.3 * glow) * t);
    Plate {
        background: vertical(top.opacity(t), base.opacity(t)),
        rim: rim.opacity(t),
        shadows: vec![
            shadow(highlight.opacity(t), 1.0, 0.0, 0.0, true),
            shadow(ring.opacity(t), 0.0, 0.0, 1.0, true),
            shadow(halo, 2.0 + 2.0 * glow, 6.0 + 14.0 * glow, 0.0, false),
        ],
    }
}

/// The opaque handle: the light-appearance neutral plate in every
/// appearance, so it stays solid over translucent dark glass, with the
/// appearance's own drop beneath it.
pub(crate) fn thumb_plate(theme: &Theme) -> Plate {
    let dark = theme.appearance.is_dark();
    let mut shadows = vec![
        shadow(white(0.8), 1.0, 0.0, 0.0, true),
        shadow(black(if dark { 0.22 } else { 0.14 }), 1.0, 2.0, 0.0, false),
    ];
    if !dark {
        // A wide, faint ambient drop separates it from pale tracks.
        shadows.push(shadow(black(0.06), 2.0, 6.0, 0.0, false));
    }
    Plate {
        background: vertical(hsla(0.0, 0.0, 1.0, 1.0), hsla(0.0, 0.0, 0.965, 1.0)),
        rim: black(if dark { 0.12 } else { 0.11 }),
        shadows,
    }
}

pub(crate) fn light<E: Styled>(el: E, theme: &Theme, t: f32) -> E {
    light_plate(theme, t).apply(el)
}

pub(crate) fn accent<E: Styled>(el: E, theme: &Theme, t: f32, glow: f32) -> E {
    accent_plate(theme, t, glow).apply(el)
}

pub(crate) fn thumb<E: Styled>(el: E, theme: &Theme) -> E {
    thumb_plate(theme).apply(el)
}

/// A grippy dimpled texture for pill thumbs: a grid of small dimples, each a
/// soft shade with a light lip below it, kept `margin` inside the pill's
/// outline so no dimple sits on the rounded ends. Fills its parent.
/// The thumb is near-white in every appearance, so the dimples are too.
pub(crate) fn grip(pitch: f32, dimple: f32, margin: f32) -> gpui::AnyElement {
    use gpui::{IntoElement, Styled as _};
    let shade = black(0.11);
    let lip = white(0.85);
    gpui::canvas(
        |_, _, _| (),
        move |bounds, _, window, _| {
            let w = f32::from(bounds.size.width);
            let h = f32::from(bounds.size.height);
            let radius = h / 2.0;
            // Inside the pill: within `radius - margin` of its centre line.
            let inside = |x: f32, y: f32| {
                let cx = x.clamp(radius, (w - radius).max(radius));
                ((x - cx).powi(2) + (y - radius).powi(2)).sqrt() <= radius - margin
            };
            let cols = ((w - 2.0 * margin) / pitch).floor().max(0.0) as i32;
            let rows = ((h - 2.0 * margin) / pitch).floor().max(0.0) as i32;
            let x0 = (w - cols as f32 * pitch) / 2.0 + pitch / 2.0;
            let y0 = (h - rows as f32 * pitch) / 2.0 + pitch / 2.0;
            let dot = |window: &mut Window, x: f32, y: f32, color: Hsla| {
                window.paint_quad(gpui::quad(
                    Bounds::new(
                        bounds.origin + point(px(x - dimple / 2.0), px(y - dimple / 2.0)),
                        gpui::size(px(dimple), px(dimple)),
                    ),
                    Corners::all(px(dimple / 2.0)),
                    color,
                    px(0.0),
                    gpui::transparent_black(),
                    gpui::BorderStyle::default(),
                ));
            };
            for row in 0..rows {
                for col in 0..cols {
                    let (x, y) = (x0 + col as f32 * pitch, y0 + row as f32 * pitch);
                    if inside(x, y) {
                        // The lip first, peeking out below the shade.
                        dot(window, x, y + 0.5, lip);
                        dot(window, x, y, shade);
                    }
                }
            }
        },
    )
    .absolute()
    .inset_0()
    .into_any_element()
}
