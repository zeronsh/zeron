//! Cell colors for the phone: the Zeron palettes, xterm's cube and ramp, and
//! the desktop's contrast floor (`crates/ui/src/terminal/view.rs`
//! `resolve_color` / `ensure_contrast`) so a program's own colors stay
//! readable on the terminal's background.

use zeron_term::emulator::CellColor;

/// Text a program colored itself is lifted to at least this contrast
/// (WCAG ratio) against what's behind it.
pub const MIN_CONTRAST: f32 = 3.0;

/// xterm 6×6×6 cube levels.
const CUBE: [u8; 6] = [0, 95, 135, 175, 215, 255];

/// Zeron Dark / Light ANSI 16 (crates/theme builtins).
const ANSI_DARK: [u32; 16] = [
    0x242424, 0xf87171, 0x4ade80, 0xfacc15, 0x60a5fa, 0xc084fc, 0x22d3ee, 0xd4d4d8, 0x52525b,
    0xfca5a5, 0x86efac, 0xfde047, 0x93c5fd, 0xd8b4fe, 0x67e8f9, 0xfafafa,
];
const ANSI_LIGHT: [u32; 16] = [
    0x1f1f1f, 0xdc2626, 0x16a34a, 0xb45309, 0x2563eb, 0x9333ea, 0x0e7490, 0x3f3f46, 0x71717a,
    0xb91c1c, 0x15803d, 0x92400e, 0x1d4ed8, 0x7e22ce, 0x155e75, 0x18181b,
];

/// The app's text and terminal background (Palette.text / the terminal well).
pub const FG_DARK: u32 = 0xe8e8ea;
pub const FG_LIGHT: u32 = 0x27272c;
pub const BG_DARK: u32 = 0x090909;
pub const BG_LIGHT: u32 = 0xfafafb;

/// A cell color as 24-bit RGB; `None` = the default (foreground or
/// background, the platform's own).
pub fn rgb(color: CellColor, dark: bool) -> Option<u32> {
    Some(match color {
        CellColor::Foreground | CellColor::Background => return None,
        CellColor::Indexed(ix @ 0..=15) => {
            if dark {
                ANSI_DARK[ix as usize]
            } else {
                ANSI_LIGHT[ix as usize]
            }
        }
        // The cube is the program's arithmetic: left alone.
        CellColor::Indexed(ix @ 16..=231) => {
            let n = ix as usize - 16;
            (CUBE[n / 36] as u32) << 16 | (CUBE[(n / 6) % 6] as u32) << 8 | CUBE[n % 6] as u32
        }
        // The grayscale ramp means "dimmer": mirrored in light mode so its
        // faint end stays the faint one.
        CellColor::Indexed(ix) => {
            let step = ix - 232;
            let step = if dark { step } else { 23 - step };
            let v = 8 + 10 * step as u32;
            v << 16 | v << 8 | v
        }
        CellColor::Rgb(r, g, b) => (r as u32) << 16 | (g as u32) << 8 | b as u32,
    })
}

pub fn rgba(rgb: u32, alpha: u8) -> u32 {
    (rgb << 8) | alpha as u32
}

fn channels(rgb: u32) -> [f32; 3] {
    [(rgb >> 16) & 0xff, (rgb >> 8) & 0xff, rgb & 0xff].map(|c| c as f32 / 255.0)
}

fn luminance(rgb: u32) -> f32 {
    let lin = |c: f32| {
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    let [r, g, b] = channels(rgb);
    0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
}

pub fn contrast(a: u32, b: u32) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

fn to_hsl(rgb: u32) -> (f32, f32, f32) {
    let [r, g, b] = channels(rgb);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    if max == min {
        return (0.0, 0.0, l);
    }
    let d = max - min;
    let s = if l > 0.5 {
        d / (2.0 - max - min)
    } else {
        d / (max + min)
    };
    let h = if max == r {
        (g - b) / d + if g < b { 6.0 } else { 0.0 }
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    } / 6.0;
    (h, s, l)
}

fn from_hsl(h: f32, s: f32, l: f32) -> u32 {
    let channel = |t: f32| {
        let q = if l < 0.5 {
            l * (1.0 + s)
        } else {
            l + s - l * s
        };
        let p = 2.0 * l - q;
        let t = t.rem_euclid(1.0);
        let v = if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 0.5 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        };
        (v.clamp(0.0, 1.0) * 255.0).round() as u32
    };
    if s == 0.0 {
        let v = (l.clamp(0.0, 1.0) * 255.0).round() as u32;
        return v << 16 | v << 8 | v;
    }
    channel(h + 1.0 / 3.0) << 16 | channel(h) << 8 | channel(h - 1.0 / 3.0)
}

/// `fg` moved in lightness just far enough from `bg` to reach `min`,
/// keeping its hue (the desktop's `ensure_contrast`). Identical colors are
/// deliberate (hidden text) and stay.
pub fn ensure_contrast(fg: u32, bg: u32, min: f32) -> u32 {
    if fg == bg || contrast(fg, bg) >= min {
        return fg;
    }
    let (h, s, l) = to_hsl(fg);
    let lighter = luminance(bg) < 0.18;
    let (mut lo, mut hi) = if lighter { (l, 1.0) } else { (0.0, l) };
    let mut best = from_hsl(h, s, if lighter { 1.0 } else { 0.0 });
    for _ in 0..16 {
        let mid = (lo + hi) / 2.0;
        let candidate = from_hsl(h, s, mid);
        if contrast(candidate, bg) >= min {
            best = candidate;
            if lighter { hi = mid } else { lo = mid }
        } else if lighter {
            lo = mid
        } else {
            hi = mid
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_ranges_resolve_per_appearance() {
        assert_eq!(rgb(CellColor::Foreground, true), None);
        assert_eq!(rgb(CellColor::Indexed(1), true), Some(0xf87171));
        assert_eq!(rgb(CellColor::Indexed(1), false), Some(0xdc2626));
        assert_eq!(rgb(CellColor::Indexed(196), false), Some(0xff0000));
        assert_eq!(rgb(CellColor::Indexed(232), true), Some(0x080808));
        assert_eq!(rgb(CellColor::Indexed(232), false), Some(0xeeeeee));
    }

    #[test]
    fn unreadable_program_colors_are_lifted() {
        let navy = 0x000080;
        let lifted = ensure_contrast(navy, BG_DARK, MIN_CONTRAST);
        assert!(contrast(lifted, BG_DARK) >= MIN_CONTRAST);
        assert_ne!(lifted, navy);
        // Already readable: untouched.
        assert_eq!(ensure_contrast(0xf87171, BG_DARK, MIN_CONTRAST), 0xf87171);
        // Hidden text stays hidden.
        assert_eq!(ensure_contrast(BG_DARK, BG_DARK, MIN_CONTRAST), BG_DARK);
    }
}
