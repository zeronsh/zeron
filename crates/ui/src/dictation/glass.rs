//! The glass surface of the dictation track: a vertical gradient (lit from above),
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
