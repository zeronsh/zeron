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

/// Neutral glass. `t` fades the whole treatment in.
pub(crate) fn light_plate(theme: &Theme, t: f32) -> Plate {
    let dark = theme.appearance.is_dark();
    let (top, bottom, rim, highlight, drop) = if dark {
        (
            white(0.09),
            white(0.04),
            white(0.10),
            white(0.10),
            black(0.35),
        )
    } else {
        (
            hsla(0.0, 0.0, 0.985, 1.0),
            hsla(0.0, 0.0, 0.925, 1.0),
            black(0.10),
            white(0.95),
            black(0.08),
        )
    };
    Plate {
        background: vertical(top.opacity(t), bottom.opacity(t)),
        rim: rim.opacity(t),
        shadows: vec![
            shadow(highlight.opacity(t), 1.0, 0.0, 0.0, true),
            shadow(drop.opacity(t), 1.0, 3.0, 0.0, false),
        ],
    }
}

/// Accent glass. `t` fades it in over the resting control; `glow` (0–1)
/// spreads its coloured shadow.
pub(crate) fn accent_plate(theme: &Theme, t: f32, glow: f32) -> Plate {
    let dark = theme.appearance.is_dark();
    let base = theme.accent;
    let top = lift(base, if dark { 0.18 } else { 0.32 });
    let rim = lift(base, 0.45).opacity(if dark { 0.45 } else { 0.7 });
    let highlight = white(if dark { 0.22 } else { 0.38 });
    let halo = base.opacity((0.22 + 0.4 * glow) * t);
    Plate {
        background: vertical(top.opacity(t), base.opacity(t)),
        rim: rim.opacity(t),
        shadows: vec![
            shadow(highlight.opacity(t), 1.0, 0.0, 0.0, true),
            shadow(white(0.12 * t), 0.0, 0.0, 1.0, true),
            shadow(halo, 2.0 + 2.0 * glow, 6.0 + 14.0 * glow, 0.0, false),
        ],
    }
}

/// The opaque handle: the light-appearance neutral plate in every
/// appearance, so it stays solid over translucent dark glass, with the
/// appearance's own drop beneath it.
pub(crate) fn thumb_plate(theme: &Theme) -> Plate {
    let drop = black(if theme.appearance.is_dark() {
        0.35
    } else {
        0.12
    });
    Plate {
        background: vertical(hsla(0.0, 0.0, 0.985, 1.0), hsla(0.0, 0.0, 0.925, 1.0)),
        rim: black(0.10),
        shadows: vec![
            shadow(white(0.95), 1.0, 0.0, 0.0, true),
            shadow(drop, 1.0, 3.0, 0.0, false),
        ],
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
