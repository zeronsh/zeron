//! Effects remain cached source-space images; a separate alpha mask follows layout.
use crate::settings::{NewThreadBackgroundAdjustment, NewThreadBackgroundEffect};
use crate::theme::Theme;
use gpui::{Pixels, px};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

/// Hold the displayed artwork during loading, then crossfade to the ready image.
/// Finish each blend before adopting another image to keep rapid changes smooth.
#[derive(Default)]
pub(crate) struct Readiness {
    current: Option<Arc<gpui::RenderImage>>,
    previous: Option<Arc<gpui::RenderImage>>,
    current_adjustment: NewThreadBackgroundAdjustment,
    previous_adjustment: NewThreadBackgroundAdjustment,
    current_path: Option<PathBuf>,
    previous_path: Option<PathBuf>,
    started: Option<Instant>,
}

pub(crate) struct ArtworkFrame {
    pub current: Option<Arc<gpui::RenderImage>>,
    pub previous: Option<Arc<gpui::RenderImage>>,
    pub current_adjustment: NewThreadBackgroundAdjustment,
    pub previous_adjustment: NewThreadBackgroundAdjustment,
    pub mix: f32,
    pub active: bool,
}

fn crossfade_mix(start: Instant, now: Instant) -> f32 {
    let spec = crate::motion::WALLPAPER_CROSSFADE;
    let duration = spec.total().mul_f32(crate::motion::speed_scale());
    spec.progress(now.saturating_duration_since(start).as_secs_f32() / duration.as_secs_f32())
}

impl Readiness {
    pub fn frame(
        &mut self,
        image: Option<Arc<gpui::RenderImage>>,
        path: Option<&Path>,
        adjustment: NewThreadBackgroundAdjustment,
        enabled: bool,
        reduced: bool,
        now: Instant,
    ) -> ArtworkFrame {
        let progress = self.started.map_or(1.0, |start| crossfade_mix(start, now));
        if reduced || progress >= 1.0 {
            self.previous = None;
            self.previous_path = None;
            self.started = None;
        }
        // Effects may still be loading, but a crop belongs to the source path,
        // not its effect raster. Replacement images keep their own framing.
        if path.is_some() && path == self.current_path.as_deref() {
            self.current_adjustment = adjustment.normalized();
        }
        if path.is_some() && path == self.previous_path.as_deref() {
            self.previous_adjustment = adjustment.normalized();
        }
        // None while enabled means "still loading", not "remove artwork".
        if self.started.is_none() && (!enabled || image.is_some()) {
            let target = if enabled { image } else { None };
            if self.current.as_ref().map(|image| image.id) != target.as_ref().map(|image| image.id)
            {
                self.previous = self.current.take();
                self.previous_adjustment = self.current_adjustment;
                self.previous_path = self.current_path.take();
                self.current = target;
                self.current_adjustment = adjustment.normalized();
                self.current_path = path.map(Path::to_path_buf);
                if reduced {
                    self.previous = None;
                    self.previous_path = None;
                } else {
                    self.started = Some(now);
                }
            }
        }
        ArtworkFrame {
            current: self.current.clone(),
            previous: self.previous.clone(),
            current_adjustment: self.current_adjustment,
            previous_adjustment: self.previous_adjustment,
            mix: self.started.map_or(1.0, |start| crossfade_mix(start, now)),
            active: self.started.is_some(),
        }
    }
}
type EffectEntry = (
    (NewThreadBackgroundEffect, bool, u8),
    Option<Arc<gpui::RenderImage>>,
);

/// Blur is quantized to whole pixels: nearby slider values share one raster.
fn blur_key(blur: f32) -> u8 {
    blur.clamp(0.0, 255.0).round() as u8
}

/// Gaussian-soften source colors so a treatment can composite over them.
fn blur_source(colors: &[[u8; 4]], width: u32, height: u32, sigma: f32) -> image::RgbaImage {
    let flat: Vec<u8> = colors.iter().flat_map(|pixel| *pixel).collect();
    blur_image(
        &image::RgbaImage::from_raw(width, height, flat).expect("source size"),
        sigma,
    )
}

/// Channel-order agnostic Gaussian blur. A Gaussian keeps only low
/// frequencies, so blurring a proportionally smaller copy and scaling it
/// back looks the same at a fraction of the cost (a full-size sigma 24 blur
/// of a 1080p frame took ~275 ms; this path takes a few ms).
fn blur_image(source: &image::RgbaImage, sigma: f32) -> image::RgbaImage {
    let (width, height) = source.dimensions();
    let factor = (sigma / 1.5).floor().max(2.0);
    let small_width = ((width as f32 / factor).round() as u32).max(1);
    let small_height = ((height as f32 / factor).round() as u32).max(1);
    if small_width < 2 || small_height < 2 {
        return image::imageops::blur(source, sigma);
    }
    let small = image::imageops::thumbnail(source, small_width, small_height);
    let blurred = image::imageops::blur(&small, sigma / factor);
    upscale_bilinear(&blurred, width, height)
}

/// Plain bilinear upscale in 8-bit fixed point; enough for an already-blurred
/// image and several times cheaper than `imageops::resize`'s separable filter.
fn upscale_bilinear(source: &image::RgbaImage, width: u32, height: u32) -> image::RgbaImage {
    let (sw, sh) = (source.width() as usize, source.height() as usize);
    let (width, height) = (width as usize, height as usize);
    let raw = source.as_raw();
    // Source sample and weight (0..=256) for each output coordinate.
    let taps = |out: usize, src: usize| -> Vec<(usize, usize, u32)> {
        let scale = src as f32 / out as f32;
        (0..out)
            .map(|i| {
                let f = ((i as f32 + 0.5) * scale - 0.5).clamp(0.0, (src - 1) as f32);
                let i0 = f as usize;
                (i0, (i0 + 1).min(src - 1), ((f - i0 as f32) * 256.0) as u32)
            })
            .collect()
    };
    let columns = taps(width, sw);
    let rows = taps(height, sh);
    let mut out = vec![0u8; width * height * 4];
    // One horizontally-filtered source row pair per output row.
    let mut top = vec![0u32; width * 4];
    let mut bottom = vec![0u32; width * 4];
    let filter_row = |row: usize, into: &mut [u32]| {
        let line = &raw[row * sw * 4..(row + 1) * sw * 4];
        for (x, &(x0, x1, w)) in columns.iter().enumerate() {
            for ch in 0..4 {
                let a = line[x0 * 4 + ch] as u32;
                let b = line[x1 * 4 + ch] as u32;
                into[x * 4 + ch] = a * (256 - w) + b * w;
            }
        }
    };
    let mut cached = (usize::MAX, usize::MAX);
    for (y, &(y0, y1, w)) in rows.iter().enumerate() {
        if cached != (y0, y1) {
            if cached.1 == y0 {
                std::mem::swap(&mut top, &mut bottom);
            } else {
                filter_row(y0, &mut top);
            }
            filter_row(y1, &mut bottom);
            cached = (y0, y1);
        }
        let line = &mut out[y * width * 4..(y + 1) * width * 4];
        for (o, (t, b)) in line.iter_mut().zip(top.iter().zip(bottom.iter())) {
            *o = ((t * (256 - w) + b * w + (1 << 15)) >> 16) as u8;
        }
    }
    image::RgbaImage::from_raw(width as u32, height as u32, out).expect("upscale size")
}
#[derive(Debug)]
struct BackgroundLuminance {
    width: u32,
    height: u32,
    pixels: Box<[u8]>,
    colors: Box<[[u8; 4]]>,
    effects: Mutex<Vec<EffectEntry>>,
}
impl BackgroundLuminance {
    fn raster_image(
        self: &Arc<Self>,
        effect: NewThreadBackgroundEffect,
        light: bool,
        blur: f32,
        cx: &mut gpui::App,
    ) -> Option<Arc<gpui::RenderImage>> {
        let mut effects = self.effects.lock().unwrap();
        let key = (
            effect,
            light
                && !matches!(
                    effect,
                    NewThreadBackgroundEffect::Dither | NewThreadBackgroundEffect::None
                ),
            blur_key(blur),
        );
        if let Some((_, image)) = effects.iter().find(|(cached, _)| *cached == key) {
            return image.clone();
        }
        // None marks the single pending job for this source/effect, not a viewport.
        effects.push((key, None));
        drop(effects);
        let source = self.clone();
        cx.spawn(async move |cx| {
            let worker = source.clone();
            let image = cx
                .background_executor()
                .spawn(async move { worker.render_image(effect, light, blur) })
                .await;
            cx.update(|cx| {
                if let Some((_, ready)) = source
                    .effects
                    .lock()
                    .unwrap()
                    .iter_mut()
                    .find(|(cached, _)| *cached == key)
                {
                    *ready = Some(image);
                }
                cx.refresh_windows();
            });
        })
        .detach();
        None
    }
    fn render_image(
        &self,
        effect: NewThreadBackgroundEffect,
        light: bool,
        blur: f32,
    ) -> Arc<gpui::RenderImage> {
        Arc::new(gpui::RenderImage::new([image::Frame::new(
            self.render_pixels(effect, light, blur),
        )]))
    }
    /// Effect pixels in BGRA, ready for `RenderImage`.
    fn render_pixels(
        &self,
        effect: NewThreadBackgroundEffect,
        light: bool,
        blur: f32,
    ) -> image::RgbaImage {
        let sigma = blur_key(blur) as f32;
        if sigma >= 1.0 {
            // The treatment composites over a softened backdrop
            // instead of softening its own texture.
            return from_rgba(blur_source(&self.colors, self.width, self.height, sigma))
                .render_pixels(effect, light, 0.0);
        }
        match effect {
            NewThreadBackgroundEffect::None => {
                image::RgbaImage::from_fn(self.width, self.height, |x, y| {
                    let [r, g, b, a] = self.colors[(y * self.width + x) as usize];
                    image::Rgba([b, g, r, a])
                })
            }
            NewThreadBackgroundEffect::Dither => self.dither_pixels(self.width, self.height),
            NewThreadBackgroundEffect::Halftone => {
                self.halftone_pixels(self.width, self.height, light)
            }
            NewThreadBackgroundEffect::Ascii => self.ascii_pixels(light),
            _ => self.scanline_pixels(light),
        }
    }
    fn scanline_pixels(&self, light: bool) -> image::RgbaImage {
        image::RgbaImage::from_fn(self.width, self.height, |x, y| {
            let [r, g, b, a] = self.colors[(y * self.width + x) as usize];
            let gain = if y % 3 == 0 { 0.52 } else { 1.0 };
            let channel = |value: u8| {
                if light {
                    (value as f32 + (255.0 - value as f32) * (1.0 - gain)) as u8
                } else {
                    (value as f32 * gain) as u8
                }
            };
            image::Rgba([channel(b), channel(g), channel(r), a])
        })
    }
    fn ascii_pixels(&self, light: bool) -> image::RgbaImage {
        // Five-column bitmap glyphs, one column/row of spacing. These are
        // artwork pixels rather than thousands of shaped UI text runs.
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
        image::RgbaImage::from_fn(self.width, self.height, |x, y| {
            let sx = (x / 6 * 6 + 3).min(self.width - 1);
            let sy = (y / 8 * 8 + 4).min(self.height - 1);
            let sample = (sy * self.width + sx) as usize;
            let ink_density = if light {
                255 - self.pixels[sample]
            } else {
                self.pixels[sample]
            };
            let index = ((ink_density as f32 / 255.0).sqrt() * 9.0) as usize;
            let ink =
                x % 6 < 5 && y % 8 < 7 && GLYPHS[index][y as usize % 8] & (1 << (4 - x % 6)) != 0;
            let [r, g, b, a] = self.colors[(y * self.width + x) as usize];
            let [cr, cg, cb, _] = self.colors[sample];
            let mix = |base: u8, glyph: u8| {
                let paper = if light { 255.0 } else { 0.0 };
                // Keep a colored image beneath the glyph texture in both themes.
                (base as f32 * 0.60
                    + if ink {
                        glyph as f32 * 0.40
                    } else {
                        paper * 0.40
                    }) as u8
            };
            image::Rgba([mix(b, cb), mix(g, cg), mix(r, cr), a])
        })
    }
    fn halftone_pixels(&self, width: u32, height: u32, light: bool) -> image::RgbaImage {
        let bounds = gpui::size(px(width as f32), px(height as f32));
        let paper = if light { 255 } else { 0 };
        let mut pixels =
            image::RgbaImage::from_pixel(width, height, image::Rgba([paper, paper, paper, 255]));
        for y in (0..height).step_by(4) {
            for x in (0..width).step_by(4) {
                let luma = self.sample_cover(bounds, x as f32, y as f32);
                let luma = if light { 255 - luma } else { luma };
                let radius = 2.0 * (0.3 + 0.7 * (luma as f32 / 255.0).sqrt());
                let [r, g, b, a] =
                    self.colors[self.cover_index(bounds, x as f32 + 2.0, y as f32 + 2.0)];
                for dy in 0..4.min(height - y) {
                    for dx in 0..4.min(width - x) {
                        let distance =
                            ((dx as f32 - 1.5).powi(2) + (dy as f32 - 1.5).powi(2)).sqrt();
                        let coverage = (radius + 0.5 - distance).clamp(0.0, 1.0) * a as f32 / 255.0;
                        let [sr, sg, sb, sa] = self.colors[((y + dy) * width + x + dx) as usize];
                        let blend = |source: u8, dot: u8| {
                            (source as f32 * 0.60
                                + (dot as f32 * coverage + paper as f32 * (1.0 - coverage)) * 0.40)
                                as u8
                        };
                        pixels.put_pixel(
                            x + dx,
                            y + dy,
                            image::Rgba([blend(sb, b), blend(sg, g), blend(sr, r), sa]),
                        );
                    }
                }
            }
        }
        pixels
    }

    fn dither_pixels(&self, width: u32, height: u32) -> image::RgbaImage {
        const BAYER: [[u8; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];
        let bounds = gpui::size(px(width as f32), px(height as f32));
        let mut pixels = image::RgbaImage::new(width, height);
        for y in (0..height).step_by(2) {
            for x in (0..width).step_by(2) {
                // Sample/quantize once per dot, not four times per 2x2 cell.
                let index = self.cover_index(bounds, (x + 1) as f32, (y + 1) as f32);
                let [r, g, b, a] = dither_color(
                    self.colors[index],
                    BAYER[y as usize / 2 % 4][x as usize / 2 % 4],
                );
                for dy in 0..2.min(height - y) {
                    for dx in 0..2.min(width - x) {
                        // RenderImage consumes BGRA.
                        pixels.put_pixel(x + dx, y + dy, image::Rgba([b, g, r, a]));
                    }
                }
            }
        }
        pixels
    }

    fn sample_cover(&self, bounds: gpui::Size<Pixels>, x: f32, y: f32) -> u8 {
        self.pixels[self.cover_index(bounds, x, y)]
    }

    fn cover_index(&self, bounds: gpui::Size<Pixels>, x: f32, y: f32) -> usize {
        let width = f32::from(bounds.width).max(1.0);
        let height = f32::from(bounds.height).max(1.0);
        let source_width = self.width as f32;
        let source_height = self.height as f32;
        let scale = (width / source_width).max(height / source_height);
        let visible_width = width / scale;
        let visible_height = height / scale;
        let source_x = ((source_width - visible_width) * 0.5 + x / scale)
            .clamp(0.0, source_width - 1.0) as u32;
        let source_y = ((source_height - visible_height) * 0.5 + y / scale)
            .clamp(0.0, source_height - 1.0) as u32;
        (source_y * self.width + source_x) as usize
    }
}

type Source = Arc<Mutex<Option<Arc<BackgroundLuminance>>>>;
type Cache = Vec<(PathBuf, u16, Source)>;
static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();

fn decode_source(bytes: &[u8], rotation: u16) -> Option<Arc<BackgroundLuminance>> {
    Some(source_from_image(
        &crate::new_thread_background_image::decode(bytes).ok()?,
        rotation,
    ))
}

fn source_from_image(image: &image::DynamicImage, rotation: u16) -> Arc<BackgroundLuminance> {
    let proxy = crate::new_thread_background_image::rotate(
        image.thumbnail(2048, 2048).to_rgba8(),
        rotation,
    );
    from_rgba(proxy)
}

/// Build effect source pixels from an RGBA frame.
fn from_rgba(proxy: image::RgbaImage) -> Arc<BackgroundLuminance> {
    let dynamic = image::DynamicImage::ImageRgba8(proxy);
    let gray = dynamic.to_luma8();
    Arc::new(BackgroundLuminance {
        width: gray.width(),
        height: gray.height(),
        pixels: gray.into_raw().into_boxed_slice(),
        colors: dynamic.to_rgba8().pixels().map(|pixel| pixel.0).collect(),
        effects: Mutex::new(Vec::new()),
    })
}

/// Apply a raster effect to one BGRA video frame, returning BGRA pixels
/// ready for `RenderImage`. `None` is identity.
/// Whether [`render_frame_effect`] would return the frame unchanged, so a
/// caller can skip the call (and its full-frame copy) entirely.
pub(super) fn frame_effect_is_identity(
    effect: NewThreadBackgroundEffect,
    blur: f32,
    rotation: u16,
) -> bool {
    matches!(effect, NewThreadBackgroundEffect::None)
        && (blur_key(blur) as f32) < 1.0
        && crate::settings::normalize_rotation(rotation) == 0
}

pub(super) fn render_frame_effect(
    frame: &image::RgbaImage,
    effect: NewThreadBackgroundEffect,
    light: bool,
    blur: f32,
    rotation: u16,
) -> image::RgbaImage {
    // Rotate first so the effect texture (scanlines, glyphs, halftone dots)
    // stays screen-aligned instead of turning on its side with the artwork.
    // Each step allocates only when it changes pixels: a video frame is
    // megabytes and this runs for every frame.
    let rotated = (crate::settings::normalize_rotation(rotation) != 0)
        .then(|| crate::new_thread_background_image::rotate(frame.clone(), rotation));
    let sigma = blur_key(blur) as f32;
    // Blur is per channel, so it runs on the BGRA frame as decoded.
    let softened = match (sigma >= 1.0, rotated) {
        (true, rotated) => blur_image(rotated.as_ref().unwrap_or(frame), sigma),
        (false, Some(rotated)) => rotated,
        (false, None) => frame.clone(),
    };
    if matches!(effect, NewThreadBackgroundEffect::None) {
        return softened;
    }
    // The effect pipeline is RGBA in, BGRA out.
    let mut rgba = softened;
    for pixel in rgba.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    from_rgba(rgba).render_pixels(effect, light, 0.0)
}

/// Fully decoded source and selected effect, built on the background executor.
pub(crate) struct PreloadedArtwork(Arc<BackgroundLuminance>);

impl PreloadedArtwork {
    /// Takes the image the caller already decoded to validate the candidate,
    /// so a full-resolution wallpaper is decoded once per preload.
    pub fn load(
        image: &image::DynamicImage,
        effect: NewThreadBackgroundEffect,
        light: bool,
        blur: f32,
        rotation: u16,
    ) -> Self {
        let source = source_from_image(image, rotation);
        let light = light
            && !matches!(
                effect,
                NewThreadBackgroundEffect::None | NewThreadBackgroundEffect::Dither
            );
        let image = source.render_image(effect, light, blur);
        source
            .effects
            .lock()
            .unwrap()
            .push(((effect, light, blur_key(blur)), Some(image)));
        Self(source)
    }

    pub fn color(&self) -> Option<zeron_theme::Color> {
        let stride = (self.0.colors.len() / 4096).max(1);
        crate::settings::wallpaper_colors::extract(self.0.colors.iter().step_by(stride).copied())
    }

    pub fn install(self, path: &Path, rotation: u16) {
        let mut cache = CACHE.get_or_init(|| Mutex::new(Vec::new())).lock().unwrap();
        cache.retain(|(key, cached_rotation, _)| {
            !(key == path && *cached_rotation == crate::settings::normalize_rotation(rotation))
        });
        cache.push((
            path.to_path_buf(),
            crate::settings::normalize_rotation(rotation),
            Arc::new(Mutex::new(Some(self.0))),
        ));
        if cache.len() > 4 {
            cache.remove(0);
        }
    }
}

fn background_luminance(
    path: &Path,
    rotation: u16,
    cx: &mut gpui::App,
) -> Option<Arc<BackgroundLuminance>> {
    let rotation = crate::settings::normalize_rotation(rotation);
    let cache = CACHE.get_or_init(|| Mutex::new(Vec::new()));
    if let Some(source) = cache
        .lock()
        .ok()?
        .iter()
        .find_map(|(key, cached_rotation, source)| {
            (key == path && *cached_rotation == rotation).then(|| source.clone())
        })
    {
        return source.lock().ok()?.clone();
    }
    let pending = Arc::new(Mutex::new(None));
    {
        let mut cache = cache.lock().ok()?;
        cache.push((path.to_path_buf(), rotation, pending.clone()));
        if cache.len() > 4 {
            cache.remove(0);
        }
    }
    let path = path.to_path_buf();
    cx.spawn(async move |cx| {
        let source = cx
            .background_executor()
            .spawn(async move {
                let bytes = std::fs::read(path).ok()?;
                decode_source(&bytes, rotation)
            })
            .await;
        cx.update(|cx| {
            *pending.lock().unwrap() = source;
            cx.refresh_windows();
        });
    })
    .detach();
    None
}

/// Safe to call on both routes: loading/decoding/effects happen once off-thread,
/// before the hero is requested, and never depend on composer/sidebar geometry.
pub(super) fn prepare(
    effect: NewThreadBackgroundEffect,
    theme: &Theme,
    path: &Path,
    cx: &mut gpui::App,
) -> Option<Arc<gpui::RenderImage>> {
    // A video background yields its live frame, with the selected
    // raster effect applied per frame off the UI thread.
    let light = matches!(theme.appearance, crate::theme::Appearance::Light);
    let blur = crate::settings::new_thread_background_blur_value(cx);
    let rotation = crate::settings::new_thread_background_rotation_value(cx);
    if crate::new_thread_background_video::is_video(path) {
        let animate = !crate::motion::video_held(cx);
        return crate::new_thread_background_video::current_frame(
            path, animate, effect, light, blur, rotation, cx,
        );
    }
    background_luminance(path, rotation, cx)
        .and_then(|source| source.raster_image(effect, light, blur, cx))
}
fn dither_color([r, g, b, a]: [u8; 4], threshold: u8) -> [u8; 4] {
    let peak = r.max(g).max(b) as f32;
    let bright = peak / 255.0 > (threshold as f32 + 0.5) / 16.0;
    let gain = if bright { 255.0 / peak.max(1.0) } else { 0.08 };
    [
        (r as f32 * gain).round() as u8,
        (g as f32 * gain).round() as u8,
        (b as f32 * gain).round() as u8,
        a,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn artwork() -> Arc<gpui::RenderImage> {
        Arc::new(gpui::RenderImage::new([image::Frame::new(
            image::RgbaImage::new(1, 1),
        )]))
    }

    #[test]
    fn wallpaper_crossfade_holds_while_loading_and_handles_rapid_changes() {
        let first = artwork();
        let next = artwork();
        let latest = artwork();
        let mut ready = Readiness::default();
        let now = Instant::now();
        let path = Some(Path::new("background.png"));
        let adjustment = NewThreadBackgroundAdjustment::default();
        ready.frame(Some(first.clone()), path, adjustment, true, true, now);
        let loading = ready.frame(None, path, adjustment, true, false, now);
        assert_eq!(loading.current.unwrap().id, first.id);
        assert!(!loading.active);
        let start = ready.frame(Some(next.clone()), path, adjustment, true, false, now);
        assert_eq!(start.mix, 0.0);
        assert_eq!(start.previous.unwrap().id, first.id);
        let halfway = now
            + std::time::Duration::from_secs_f32(
                crate::motion::WALLPAPER_CROSSFADE.total().as_secs_f32()
                    * crate::motion::speed_scale()
                    * 0.5,
            );
        let frame = ready.frame(Some(latest.clone()), path, adjustment, true, false, halfway);
        assert!((frame.mix - 0.875).abs() < 0.001);
        assert_eq!(frame.current.unwrap().id, next.id);
        let done = now + std::time::Duration::from_secs(1);
        let frame = ready.frame(Some(latest.clone()), path, adjustment, true, false, done);
        assert_eq!(frame.previous.unwrap().id, next.id);
        assert_eq!(frame.current.unwrap().id, latest.id);
        assert_eq!(frame.mix, 0.0);
        let frame = ready.frame(Some(latest), path, adjustment, true, true, done);
        assert!(!frame.active);
        assert!(frame.previous.is_none());
        assert_eq!(frame.mix, 1.0);
        let removed = ready.frame(None, None, adjustment, false, true, done);
        assert!(removed.current.is_none() && removed.previous.is_none());
    }

    #[test]
    fn wallpaper_replacement_and_removal_keep_the_departing_crop() {
        let first = artwork();
        let next = artwork();
        let first_path = Some(Path::new("first.png"));
        let next_path = Some(Path::new("next.png"));
        let cropped = NewThreadBackgroundAdjustment {
            focal_x: 0.2,
            focal_y: 0.8,
            zoom: 2.0,
            rotation: 0,
        };
        let centered = NewThreadBackgroundAdjustment::default();
        let mut ready = Readiness::default();
        let now = Instant::now();
        ready.frame(Some(first.clone()), first_path, cropped, true, true, now);
        let loading = ready.frame(None, next_path, centered, true, false, now);
        assert_eq!(loading.current.as_ref().unwrap().id, first.id);
        assert_eq!(loading.current_adjustment, cropped);
        let start = ready.frame(Some(next), next_path, centered, true, false, now);
        assert_eq!(start.previous_adjustment, cropped);
        assert_eq!(start.current_adjustment, centered);
        let latest = artwork();
        let latest_path = Some(Path::new("latest.png"));
        let waiting = ready.frame(Some(latest.clone()), latest_path, cropped, true, false, now);
        assert_eq!(waiting.previous_adjustment, cropped);
        assert_eq!(waiting.current_adjustment, centered);
        let done = now + std::time::Duration::from_secs(1);
        let adopted = ready.frame(Some(latest), latest_path, cropped, true, false, done);
        assert_eq!(adopted.previous_adjustment, centered);
        assert_eq!(adopted.current_adjustment, cropped);

        let mut ready = Readiness::default();
        ready.frame(Some(first), first_path, cropped, true, true, now);
        let removed = ready.frame(None, None, centered, false, false, now);
        assert!(removed.current.is_none());
        assert!(removed.previous.is_some());
        assert_eq!(removed.previous_adjustment, cropped);
    }

    #[test]
    fn source_rotation_swaps_dimensions_before_effects() {
        let image = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            4,
            2,
            image::Rgba([10, 20, 30, 255]),
        ));
        let rotated = source_from_image(&image, 90);
        let flush = source_from_image(&image, 180);
        let base = source_from_image(&image, 0);
        assert_eq!((rotated.width, rotated.height), (base.height, base.width));
        assert_eq!((flush.width, flush.height), (base.width, base.height));
    }

    #[test]
    fn video_frames_rotate_before_the_effect() {
        // Two BGRA pixels side by side become a vertical pair after a clockwise turn.
        let frame = image::RgbaImage::from_raw(2, 1, vec![1, 2, 3, 255, 4, 5, 6, 255]).unwrap();
        let rotated = render_frame_effect(&frame, NewThreadBackgroundEffect::None, false, 0.0, 90);
        assert_eq!(rotated.dimensions(), (1, 2));
        assert_eq!(rotated.get_pixel(0, 0).0, [1, 2, 3, 255]);
        assert_eq!(rotated.get_pixel(0, 1).0, [4, 5, 6, 255]);
    }

    #[test]
    fn applying_crop_during_effect_loading_updates_the_same_source_without_a_fade() {
        let first = artwork();
        let effect = artwork();
        let path = Some(Path::new("background.png"));
        let centered = NewThreadBackgroundAdjustment::default();
        let cropped = NewThreadBackgroundAdjustment {
            zoom: 2.0,
            ..centered
        };
        let latest = NewThreadBackgroundAdjustment {
            zoom: 3.0,
            ..centered
        };
        let mut ready = Readiness::default();
        let now = Instant::now();
        ready.frame(Some(first), path, centered, true, true, now);
        let loading = ready.frame(None, path, cropped, true, false, now);
        assert_eq!(loading.current_adjustment, cropped);
        assert!(!loading.active);
        let start = ready.frame(Some(effect.clone()), path, cropped, true, false, now);
        assert_eq!(start.previous_adjustment, cropped);
        assert_eq!(start.current_adjustment, cropped);
        let applied = ready.frame(Some(effect.clone()), path, latest, true, false, now);
        assert_eq!(applied.previous_adjustment, latest);
        assert_eq!(applied.current_adjustment, latest);
        let done = now + std::time::Duration::from_secs(1);
        let applied = ready.frame(Some(effect), path, cropped, true, false, done);
        assert_eq!(applied.current_adjustment, cropped);
        assert!(!applied.active);
    }

    #[gpui::test]
    fn prewarming_decodes_off_thread_and_reuses_artwork_without_hero_geometry(
        cx: &mut gpui::TestAppContext,
    ) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("background.png");
        image::RgbaImage::from_pixel(32, 24, image::Rgba([173, 89, 231, 180]))
            .save(&path)
            .unwrap();
        cx.update(|cx| assert!(background_luminance(&path, 0, cx).is_none()));
        cx.run_until_parked();
        let source = cx.update(|cx| background_luminance(&path, 0, cx).unwrap());
        cx.update(|cx| {
            assert!(
                source
                    .raster_image(NewThreadBackgroundEffect::None, false, 0.0, cx)
                    .is_none()
            )
        });
        cx.run_until_parked();
        cx.update(|cx| {
            let warm = background_luminance(&path, 0, cx).unwrap();
            assert!(Arc::ptr_eq(&source, &warm));
            let image = warm
                .raster_image(NewThreadBackgroundEffect::None, false, 0.0, cx)
                .unwrap();
            assert_eq!(image.as_bytes(0).unwrap()[..4], [231, 89, 173, 180]);
            for _ in 0..20 {
                assert!(Arc::ptr_eq(
                    &image,
                    &warm
                        .raster_image(NewThreadBackgroundEffect::None, false, 0.0, cx)
                        .unwrap()
                ));
            }
        });
    }
    fn fixture() -> Arc<BackgroundLuminance> {
        Arc::new(BackgroundLuminance {
            width: 60,
            height: 32,
            pixels: vec![128; 1920].into_boxed_slice(),
            colors: vec![[128, 64, 32, 200]; 1920].into_boxed_slice(),
            effects: Mutex::new(Vec::new()),
        })
    }
    #[gpui::test]
    fn every_effect_is_generated_once_independently_of_viewport(cx: &mut gpui::TestAppContext) {
        let source = fixture();
        for effect in [
            NewThreadBackgroundEffect::Dither,
            NewThreadBackgroundEffect::Ascii,
            NewThreadBackgroundEffect::Halftone,
            NewThreadBackgroundEffect::Scanlines,
        ] {
            cx.update(|cx| {
                for _ in 0..100 {
                    assert!(source.raster_image(effect, false, 0.0, cx).is_none());
                }
                assert_eq!(
                    source
                        .effects
                        .lock()
                        .unwrap()
                        .iter()
                        .filter(|(key, _)| *key == (effect, false, 0))
                        .count(),
                    1
                );
            });
            cx.run_until_parked();
            cx.update(|cx| {
                let first = source.raster_image(effect, false, 0.0, cx).unwrap();
                assert_eq!(first.size(0).width.0, 60);
                assert_eq!(first.size(0).height.0, 32);
                for _ in 0..100 {
                    assert!(Arc::ptr_eq(
                        &first,
                        &source.raster_image(effect, false, 0.0, cx).unwrap()
                    ));
                }
            });
        }
    }
    #[test]
    fn light_treatments_use_light_paper_without_inverting_source_hues() {
        let source = fixture();
        for (light, dark) in [
            (source.ascii_pixels(true), source.ascii_pixels(false)),
            (source.scanline_pixels(true), source.scanline_pixels(false)),
            (
                source.halftone_pixels(60, 32, true),
                source.halftone_pixels(60, 32, false),
            ),
        ] {
            let brightness = |image: &image::RgbaImage| -> u64 {
                image
                    .pixels()
                    .map(|p| p.0[..3].iter().map(|c| u64::from(*c)).sum::<u64>())
                    .sum()
            };
            assert!(brightness(&light) > brightness(&dark));
            assert_eq!(light.dimensions(), dark.dimensions());
            // Raster output is BGRA; the warm source remains warm on light paper.
            assert!(light.pixels().all(|p| p.0[2] >= p.0[1] && p.0[1] >= p.0[0]));
        }
    }

    #[gpui::test]
    fn appearance_changes_cache_both_variants_and_share_unchanged_dither(
        cx: &mut gpui::TestAppContext,
    ) {
        let source = fixture();
        for effect in [
            NewThreadBackgroundEffect::Ascii,
            NewThreadBackgroundEffect::Halftone,
            NewThreadBackgroundEffect::Scanlines,
            NewThreadBackgroundEffect::Dither,
        ] {
            cx.update(|cx| {
                source.raster_image(effect, false, 0.0, cx);
                source.raster_image(effect, true, 0.0, cx);
            });
            cx.run_until_parked();
            cx.update(|cx| {
                let dark = source.raster_image(effect, false, 0.0, cx).unwrap();
                let light = source.raster_image(effect, true, 0.0, cx).unwrap();
                assert_eq!(
                    Arc::ptr_eq(&dark, &light),
                    effect == NewThreadBackgroundEffect::Dither
                );
                for _ in 0..100 {
                    assert!(Arc::ptr_eq(
                        &dark,
                        &source.raster_image(effect, false, 0.0, cx).unwrap()
                    ));
                    assert!(Arc::ptr_eq(
                        &light,
                        &source.raster_image(effect, true, 0.0, cx).unwrap()
                    ));
                }
            });
        }
        assert_eq!(source.effects.lock().unwrap().len(), 7);
    }

    #[test]
    fn downscaled_blur_matches_a_full_size_blur() {
        // Smooth gradient plus a hard edge: what a wallpaper blur sees.
        let source = image::RgbaImage::from_fn(320, 180, |x, y| {
            let edge = if x > 160 { 200 } else { 40 };
            image::Rgba([edge, (y * 255 / 179) as u8, (x * 255 / 319) as u8, 255])
        });
        for sigma in [3.0, 8.0, 24.0] {
            let fast = blur_image(&source, sigma);
            let exact = image::imageops::blur(&source, sigma);
            assert_eq!(fast.dimensions(), exact.dimensions());
            // Ignore the borders, where both kernels clamp differently.
            let margin = (sigma * 2.0) as u32;
            let mut worst = 0i32;
            for y in margin..180 - margin {
                for x in margin..320 - margin {
                    for ch in 0..4 {
                        let diff =
                            fast.get_pixel(x, y).0[ch] as i32 - exact.get_pixel(x, y).0[ch] as i32;
                        worst = worst.max(diff.abs());
                    }
                }
            }
            assert!(worst <= 12, "sigma {sigma}: max channel error {worst}");
        }
    }

    #[test]
    fn bilinear_upscale_keeps_flat_color_and_corners() {
        let flat = image::RgbaImage::from_pixel(3, 2, image::Rgba([10, 20, 30, 40]));
        let up = upscale_bilinear(&flat, 17, 9);
        assert_eq!(up.dimensions(), (17, 9));
        assert!(up.pixels().all(|pixel| pixel.0 == [10, 20, 30, 40]));
        let ramp = image::RgbaImage::from_fn(2, 1, |x, _| image::Rgba([x as u8 * 200, 0, 0, 255]));
        let up = upscale_bilinear(&ramp, 8, 1);
        assert_eq!(up.get_pixel(0, 0).0[0], 0);
        assert_eq!(up.get_pixel(7, 0).0[0], 200);
        assert!(up.get_pixel(3, 0).0[0] < up.get_pixel(4, 0).0[0]);
    }

    #[test]
    fn raster_treatments_preserve_source_dimensions_and_alpha() {
        let source = fixture();
        for image in [
            source.dither_pixels(60, 32),
            source.ascii_pixels(false),
            source.scanline_pixels(false),
            source.ascii_pixels(true),
            source.scanline_pixels(true),
        ] {
            assert_eq!(image.dimensions(), (60, 32));
            assert!(image.pixels().all(|pixel| pixel.0[3] == 200));
        }
        assert_eq!(source.halftone_pixels(60, 32, false).dimensions(), (60, 32));
    }
}
