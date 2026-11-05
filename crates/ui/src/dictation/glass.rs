//! Glass surfaces for the voice controls: a light plate for the waveform track
//! and an accent plate for Stop. Each is a vertical gradient (lit from above),
//! a hairline rim, an inner top highlight, and a soft drop. Dark appearances
//! keep the structure with translucent white instead of opaque fills.
use gpui::{
    Background, BoxShadow, Hsla, Styled, hsla, linear_color_stop, linear_gradient, point, px,
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

/// Neutral glass plate. `t` fades the whole treatment in with the morph.
pub(crate) fn light<E: Styled>(el: E, theme: &Theme, t: f32) -> E {
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
    el.bg(vertical(top.opacity(t), bottom.opacity(t)))
        .border_1()
        .border_color(rim.opacity(t))
        .shadow(vec![
            shadow(highlight.opacity(t), 1.0, 0.0, 0.0, true),
            shadow(drop.opacity(t), 1.0, 3.0, 0.0, false),
        ])
}

/// Accent glass plate. `t` fades it in over the resting control; `glow`
/// (0–1, the live voice level) spreads its coloured shadow.
pub(crate) fn accent<E: Styled>(el: E, theme: &Theme, t: f32, glow: f32) -> E {
    let dark = theme.appearance.is_dark();
    let base = theme.accent;
    let top = lift(base, if dark { 0.18 } else { 0.32 });
    let bottom = base;
    let rim = lift(base, 0.45).opacity(if dark { 0.45 } else { 0.7 });
    let highlight = white(if dark { 0.22 } else { 0.38 });
    let halo = base.opacity((0.22 + 0.4 * glow) * t);
    el.bg(vertical(top.opacity(t), bottom.opacity(t)))
        .border_1()
        .border_color(rim.opacity(t))
        .shadow(vec![
            shadow(highlight.opacity(t), 1.0, 0.0, 0.0, true),
            shadow(white(0.12 * t), 0.0, 0.0, 1.0, true),
            shadow(halo, 2.0 + 2.0 * glow, 6.0 + 14.0 * glow, 0.0, false),
        ])
}
