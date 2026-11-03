//! The colors visuals paint with: the app palette for one appearance
//! (`apps/ios/Zeron/Design/Palette.swift`, the desktop's Zeron theme), and
//! the categorical chart series derived from its accent as the desktop does
//! (`crates/ui/src/visuals.rs`, `chart_series`).
//!
//! Visuals resolve colors in Rust, unlike transcript rows: scenes and
//! shaders bake them into GPU frames, so the painter only ever sees RGBA.

use zeron_visual::{Color, Token};

use super::Rgba;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Palette {
    pub dark: bool,
    pub text: Rgba,
    pub muted: Rgba,
    pub faint: Rgba,
    pub accent: Rgba,
    pub on_accent: Rgba,
    pub success: Rgba,
    pub warning: Rgba,
    pub danger: Rgba,
    pub border: Rgba,
    /// A stronger rule (a line chart's hover cursor).
    pub border_strong: Rgba,
    /// Translucent fill for pressed rows, secondary buttons and tracks.
    pub wash: Rgba,
    pub chart: [Rgba; 6],
}

pub(crate) fn hex(v: u32) -> Rgba {
    Rgba {
        r: ((v >> 16) & 0xff) as f32 / 255.0,
        g: ((v >> 8) & 0xff) as f32 / 255.0,
        b: (v & 0xff) as f32 / 255.0,
        a: 1.0,
    }
}

impl Rgba {
    pub(crate) fn alpha(self, a: f32) -> Rgba {
        Rgba {
            a: self.a * a,
            ..self
        }
    }
}

impl Palette {
    pub fn new(dark: bool) -> Self {
        let pick = |light: u32, night: u32| hex(if dark { night } else { light });
        let text = pick(0x27272C, 0xE8E8EA);
        let accent = pick(0x5B43E8, 0x8B7CF6);
        Self {
            dark,
            text,
            muted: pick(0x62626A, 0xA9A9AE),
            faint: pick(0x97979F, 0x6B6B72),
            accent,
            on_accent: hex(0xFFFFFF),
            success: pick(0x15803D, 0x34D399),
            warning: pick(0xA16207, 0xFACC15),
            danger: pick(0xDC2626, 0xF87171),
            border: pick(0xE2E2E6, 0x232327),
            border_strong: text.alpha(0.28),
            wash: text.alpha(0.075),
            chart: chart_series(accent, dark),
        }
    }

    pub fn token(&self, token: Token) -> Rgba {
        match token {
            Token::Text => self.text,
            Token::Muted => self.muted,
            Token::Faint => self.faint,
            Token::Accent => self.accent,
            Token::OnAccent => self.on_accent,
            Token::Success => self.success,
            Token::Warning => self.warning,
            Token::Danger => self.danger,
            Token::Border => self.border,
            Token::Chart(i) => self.chart[usize::from(i) % 6],
            Token::Rgb(rgb) => hex(rgb),
        }
    }

    pub fn color(&self, color: Color) -> Rgba {
        let mut out = self.token(color.token);
        if let Some((other, t)) = color.mix {
            // In RGB, so a ramp between two hues doesn't swing through a third.
            let b = self.token(other);
            let lerp = |x: f32, y: f32| x + (y - x) * t;
            out = Rgba {
                r: lerp(out.r, b.r),
                g: lerp(out.g, b.g),
                b: lerp(out.b, b.b),
                a: lerp(out.a, b.a),
            };
        }
        out.a *= color.alpha;
        out
    }
}

/// Categorical chart colors: the accent, then the five fixed hues farthest
/// from it, so no two series read alike. Deeper on light, brighter on dark.
fn chart_series(accent: Rgba, dark: bool) -> [Rgba; 6] {
    // (hue in turns, light tone, dark tone): teal, amber, rose, violet, lime, sky.
    const FIXED: [(f32, u32, u32); 6] = [
        (0.486, 0x0d9488, 0x2dd4bf),
        (0.106, 0xd97706, 0xfbbf24),
        (0.964, 0xe11d48, 0xfb7185),
        (0.728, 0x7c3aed, 0xa78bfa),
        (0.236, 0x65a30d, 0xa3e635),
        (0.553, 0x0284c7, 0x38bdf8),
    ];
    let (hue, saturation) = hue_saturation(accent);
    let distance = |h: f32| {
        let d = (h - hue).rem_euclid(1.0);
        d.min(1.0 - d)
    };
    // A grey accent has no hue to collide with; drop the last fixed color.
    let closest = if saturation < 0.15 {
        FIXED.len() - 1
    } else {
        (0..FIXED.len())
            .min_by(|a, b| distance(FIXED[*a].0).total_cmp(&distance(FIXED[*b].0)))
            .unwrap_or(0)
    };
    let mut out = [accent; 6];
    for (slot, (_, (_, light, night))) in FIXED
        .iter()
        .enumerate()
        .filter(|(ix, _)| *ix != closest)
        .enumerate()
    {
        out[slot + 1] = hex(if dark { *night } else { *light });
    }
    out
}

/// HSL hue (in turns) and saturation.
fn hue_saturation(c: Rgba) -> (f32, f32) {
    let max = c.r.max(c.g).max(c.b);
    let min = c.r.min(c.g).min(c.b);
    let delta = max - min;
    if delta <= f32::EPSILON {
        return (0.0, 0.0);
    }
    let lightness = (max + min) / 2.0;
    let saturation = delta / (1.0 - (2.0 * lightness - 1.0).abs()).max(f32::EPSILON);
    let hue = if max == c.r {
        ((c.g - c.b) / delta).rem_euclid(6.0)
    } else if max == c.g {
        (c.b - c.r) / delta + 2.0
    } else {
        (c.r - c.g) / delta + 4.0
    } / 6.0;
    (hue, saturation)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chart_series_are_six_distinct_colors_led_by_the_accent() {
        for dark in [false, true] {
            let palette = Palette::new(dark);
            assert_eq!(palette.chart[0], palette.accent);
            for (i, a) in palette.chart.iter().enumerate() {
                for b in &palette.chart[i + 1..] {
                    assert_ne!(a, b);
                }
            }
            // The violet accent drops the fixed violet.
            assert!(
                !palette
                    .chart
                    .contains(&hex(if dark { 0xa78bfa } else { 0x7c3aed }))
            );
        }
    }

    #[test]
    fn colors_mix_in_rgb_and_carry_alpha() {
        let palette = Palette::new(false);
        let half = palette.color(Color::token(Token::Accent).with_alpha(0.5));
        assert!((half.a - 0.5).abs() < 1e-6);
        let mixed = palette.color(Color {
            token: Token::Rgb(0x000000),
            mix: Some((Token::Rgb(0xffffff), 0.25)),
            alpha: 1.0,
        });
        assert!((mixed.r - 0.25).abs() < 1e-6 && (mixed.g - 0.25).abs() < 1e-6);
    }
}
