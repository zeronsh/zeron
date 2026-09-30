//! Chat wallpaper (the desktop's "new thread background",
//! `ui/src/new_thread_background_effects.rs`): the artwork effects, ported
//! pixel-for-pixel, plus a contrast guard the platforms use to cap the
//! artwork's opacity so text over it stays readable.
//!
//! Pixels are straight RGBA8, row-major. Effects run once per (image, effect,
//! appearance) off the UI thread; platforms cache the result.

/// Artwork treatment (desktop `NewThreadBackgroundEffect`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum WallpaperEffect {
    None,
    Dither,
    Ascii,
    Halftone,
    Scanlines,
}

fn luma(r: u8, g: u8, b: u8) -> u8 {
    // image-rs `to_luma8` (Rec. 709 weights), as the desktop samples it.
    ((2126 * r as u32 + 7152 * g as u32 + 722 * b as u32) / 10000) as u8
}

struct Source<'a> {
    w: u32,
    h: u32,
    rgba: &'a [u8],
}

impl Source<'_> {
    fn color(&self, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * self.w + x) * 4) as usize;
        [self.rgba[i], self.rgba[i + 1], self.rgba[i + 2], self.rgba[i + 3]]
    }

    fn luma(&self, x: u32, y: u32) -> u8 {
        let [r, g, b, _] = self.color(x, y);
        luma(r, g, b)
    }
}

/// Apply `effect` to an RGBA image; `light` is the appearance it's drawn in
/// (effects print on white paper in light mode, black in dark).
#[uniffi::export]
pub fn wallpaper_render(rgba: Vec<u8>, width: u32, height: u32, effect: WallpaperEffect, light: bool) -> Vec<u8> {
    if width == 0 || height == 0 || rgba.len() < (width * height * 4) as usize {
        return rgba;
    }
    let src = Source { w: width, h: height, rgba: &rgba };
    match effect {
        WallpaperEffect::None => rgba.clone(),
        WallpaperEffect::Scanlines => scanlines(&src, light),
        WallpaperEffect::Ascii => ascii(&src, light),
        WallpaperEffect::Halftone => halftone(&src, light),
        WallpaperEffect::Dither => dither(&src),
    }
}

fn scanlines(s: &Source, light: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.rgba.len());
    for y in 0..s.h {
        let gain = if y % 3 == 0 { 0.52 } else { 1.0 };
        for x in 0..s.w {
            let [r, g, b, a] = s.color(x, y);
            let ch = |v: u8| {
                if light {
                    (v as f32 + (255.0 - v as f32) * (1.0 - gain)) as u8
                } else {
                    (v as f32 * gain) as u8
                }
            };
            out.extend_from_slice(&[ch(r), ch(g), ch(b), a]);
        }
    }
    out
}

fn ascii(s: &Source, light: bool) -> Vec<u8> {
    // Five-column bitmap glyphs, one column/row of spacing (desktop table).
    const GLYPHS: [[u8; 7]; 10] = [
        [0, 0, 0, 0, 0, 0, 0],
        [0, 0, 0, 0, 0, 4, 0],
        [0, 4, 0, 0, 4, 0, 0],
        [0, 0, 0, 14, 0, 0, 0],
        [0, 0, 14, 0, 14, 0, 0],
        [0, 4, 4, 31, 4, 4, 0],
        [0, 21, 14, 31, 14, 21, 0],
        [10, 10, 31, 10, 31, 10, 10],
        [17, 2, 4, 4, 8, 16, 17],
        [14, 17, 23, 21, 23, 16, 14],
    ];
    let mut out = Vec::with_capacity(s.rgba.len());
    for y in 0..s.h {
        for x in 0..s.w {
            let sx = (x / 6 * 6 + 3).min(s.w - 1);
            let sy = (y / 8 * 8 + 4).min(s.h - 1);
            let l = s.luma(sx, sy);
            let density = if light { 255 - l } else { l };
            let index = ((density as f32 / 255.0).sqrt() * 9.0) as usize;
            let ink = x % 6 < 5 && y % 8 < 7 && GLYPHS[index.min(9)][y as usize % 8] & (1 << (4 - x % 6)) != 0;
            let [r, g, b, a] = s.color(x, y);
            let [cr, cg, cb, _] = s.color(sx, sy);
            let paper = if light { 255.0 } else { 0.0 };
            let mix = |base: u8, glyph: u8| (base as f32 * 0.60 + if ink { glyph as f32 * 0.40 } else { paper * 0.40 }) as u8;
            out.extend_from_slice(&[mix(r, cr), mix(g, cg), mix(b, cb), a]);
        }
    }
    out
}

fn halftone(s: &Source, light: bool) -> Vec<u8> {
    let paper = if light { 255u8 } else { 0 };
    let mut out = vec![0u8; s.rgba.len()];
    for px in out.as_chunks_mut::<4>().0 {
        *px = [paper, paper, paper, 255];
    }
    for y in (0..s.h).step_by(4) {
        for x in (0..s.w).step_by(4) {
            let l = s.luma(x, y);
            let l = if light { 255 - l } else { l };
            let radius = 2.0 * (0.3 + 0.7 * (l as f32 / 255.0).sqrt());
            let [r, g, b, a] = s.color((x + 2).min(s.w - 1), (y + 2).min(s.h - 1));
            for dy in 0..4.min(s.h - y) {
                for dx in 0..4.min(s.w - x) {
                    let distance = ((dx as f32 - 1.5).powi(2) + (dy as f32 - 1.5).powi(2)).sqrt();
                    let coverage = (radius + 0.5 - distance).clamp(0.0, 1.0) * a as f32 / 255.0;
                    let [sr, sg, sb, sa] = s.color(x + dx, y + dy);
                    let blend = |source: u8, dot: u8| (source as f32 * 0.60 + (dot as f32 * coverage + paper as f32 * (1.0 - coverage)) * 0.40) as u8;
                    let i = (((y + dy) * s.w + x + dx) * 4) as usize;
                    out[i..i + 4].copy_from_slice(&[blend(sr, r), blend(sg, g), blend(sb, b), sa]);
                }
            }
        }
    }
    out
}

fn dither(s: &Source) -> Vec<u8> {
    const BAYER: [[u8; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];
    let mut out = vec![0u8; s.rgba.len()];
    for y in (0..s.h).step_by(2) {
        for x in (0..s.w).step_by(2) {
            let [r, g, b, a] = s.color((x + 1).min(s.w - 1), (y + 1).min(s.h - 1));
            let threshold = BAYER[y as usize / 2 % 4][x as usize / 2 % 4];
            let peak = r.max(g).max(b) as f32;
            let bright = peak / 255.0 > (threshold as f32 + 0.5) / 16.0;
            let gain = if bright { 255.0 / peak.max(1.0) } else { 0.08 };
            let c = [(r as f32 * gain).round() as u8, (g as f32 * gain).round() as u8, (b as f32 * gain).round() as u8, a];
            for dy in 0..2.min(s.h - y) {
                for dx in 0..2.min(s.w - x) {
                    let i = (((y + dy) * s.w + x + dx) * 4) as usize;
                    out[i..i + 4].copy_from_slice(&c);
                }
            }
        }
    }
    out
}

// MARK: - Contrast guard

fn linear(c: u8) -> f32 {
    let c = c as f32 / 255.0;
    if c <= 0.040_45 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

fn rel_luminance(rgb: u32) -> f32 {
    let (r, g, b) = ((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8);
    0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b)
}

fn contrast(a: f32, b: f32) -> f32 {
    let (hi, lo) = if a > b { (a, b) } else { (b, a) };
    (hi + 0.05) / (lo + 0.05)
}

/// The highest opacity the artwork may be drawn at over `background_rgb` so
/// `text_rgb` keeps `min_contrast` (WCAG) against the *worst* part of the top
/// `region` (0–1, fraction of the image height where text sits): the bright
/// tail when text is light, the dark tail when text is dark. Clamped to
/// [0, `max_opacity`].
#[uniffi::export]
#[allow(clippy::too_many_arguments)] // a flat FFI signature: Swift/Kotlin call it positionally
pub fn wallpaper_safe_opacity(
    rgba: Vec<u8>,
    width: u32,
    height: u32,
    text_rgb: u32,
    background_rgb: u32,
    region: f32,
    min_contrast: f32,
    max_opacity: f32,
) -> f32 {
    if width == 0 || height == 0 || rgba.len() < (width * height * 4) as usize {
        return 0.0;
    }
    let rows = ((height as f32 * region.clamp(0.05, 1.0)).ceil() as u32).min(height);
    // (luminance, rgb) of every other pixel: plenty for a percentile.
    let mut samples: Vec<(f32, u32)> = Vec::with_capacity((width * rows) as usize / 4 + 1);
    for y in (0..rows).step_by(2) {
        for x in (0..width).step_by(2) {
            let i = ((y * width + x) * 4) as usize;
            let rgb = (rgba[i] as u32) << 16 | (rgba[i + 1] as u32) << 8 | rgba[i + 2] as u32;
            samples.push((rel_luminance(rgb), rgb));
        }
    }
    if samples.is_empty() {
        return 0.0;
    }
    samples.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let text = rel_luminance(text_rgb);
    // Light text is threatened by the image's brightest areas, dark text by
    // its darkest: take the 95th / 5th percentile pixel.
    let light_text = text > rel_luminance(background_rgb);
    let pct = |p: f32| samples[((samples.len() - 1) as f32 * p).round() as usize].1;
    let worst = if light_text { pct(0.95) } else { pct(0.05) };
    // Composite like the screen does (gamma-encoded sRGB), then measure.
    let over = |alpha: f32| {
        let ch = |shift: u32| {
            let a = (worst >> shift & 0xFF) as f32;
            let b = (background_rgb >> shift & 0xFF) as f32;
            (alpha * a + (1.0 - alpha) * b).round().clamp(0.0, 255.0) as u32
        };
        ch(16) << 16 | ch(8) << 8 | ch(0)
    };
    let ok = |alpha: f32| contrast(text, rel_luminance(over(alpha))) >= min_contrast;
    if ok(max_opacity) {
        return max_opacity;
    }
    let (mut lo, mut hi) = (0.0f32, max_opacity);
    for _ in 0..24 {
        let mid = (lo + hi) / 2.0;
        if ok(mid) { lo = mid } else { hi = mid }
    }
    lo
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: u32, h: u32, rgb: [u8; 3]) -> Vec<u8> {
        (0..w * h).flat_map(|_| [rgb[0], rgb[1], rgb[2], 255]).collect()
    }

    #[test]
    fn effects_keep_size_and_differ_from_source() {
        let src: Vec<u8> = (0..32 * 24).flat_map(|i| [(i % 255) as u8, (i * 3 % 255) as u8, 200, 255]).collect();
        for effect in [WallpaperEffect::Dither, WallpaperEffect::Ascii, WallpaperEffect::Halftone, WallpaperEffect::Scanlines] {
            for light in [false, true] {
                let out = wallpaper_render(src.clone(), 32, 24, effect, light);
                assert_eq!(out.len(), src.len());
                assert_ne!(out, src, "{effect:?} light={light}");
            }
        }
        assert_eq!(wallpaper_render(src.clone(), 32, 24, WallpaperEffect::None, false), src);
    }

    #[test]
    fn scanlines_match_desktop_gain() {
        let out = wallpaper_render(solid(3, 3, [200, 100, 50]), 3, 3, WallpaperEffect::Scanlines, false);
        assert_eq!(&out[0..3], &[104, 52, 26]); // row 0: × 0.52
        assert_eq!(&out[12..15], &[200, 100, 50]); // row 1 untouched
    }

    #[test]
    fn safe_opacity_keeps_text_readable() {
        // Dark theme (light text on near-black): a white image must be faded
        // hard; a black image can show fully.
        let text = 0xE8E8EA;
        let bg = 0x060606;
        let white = wallpaper_safe_opacity(solid(8, 8, [255, 255, 255]), 8, 8, text, bg, 1.0, 4.5, 1.0);
        assert!(white < 0.5, "{white}");
        let g = |v: f32| (v * 255.0 + (1.0 - v) * 6.0).round() as u32;
        let over = g(white) << 16 | g(white) << 8 | g(white);
        assert!(contrast(rel_luminance(text), rel_luminance(over)) >= 4.5 - 0.05);
        let black = wallpaper_safe_opacity(solid(8, 8, [0, 0, 0]), 8, 8, text, bg, 1.0, 4.5, 1.0);
        assert!((black - 1.0).abs() < 1e-6);
        // Light theme (dark text on white): a dark image is the threat.
        let dark = wallpaper_safe_opacity(solid(8, 8, [20, 20, 40]), 8, 8, 0x303035, 0xF3F3F5, 1.0, 4.5, 1.0);
        assert!(dark < 0.75, "{dark}");
        assert!(dark > 0.2, "{dark}");
    }
}
