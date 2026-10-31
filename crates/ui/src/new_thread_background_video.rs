//! Video wallpapers. A background thread decodes the file with
//! AVFoundation into BGRA frames; the newest frame is published as a
//! `RenderImage` and painted through the same masked hero path as a still
//! image, so cropping, zoom, the composer cutout, and fades all apply.
//!
//! Playback is demand-driven: the decoder only advances while something has
//! asked for a frame recently, so a hidden window stops costing decode work;
//! an idle decoder parks its thread until the next request.
//! Frames the renderer no longer needs are queued for `release_retired`, which
//! frees their GPU atlas entries from the painting window.
//!
//! Energy saver keeps the video moving but lighter: frames decode at a smaller
//! size and publish at a lower rate (see [`Quality`]). The painting window
//! alone is repainted at the video cadence, not every open window.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Extensions AVFoundation plays natively.
const VIDEO_EXTENSIONS: [&str; 3] = ["mp4", "mov", "m4v"];

/// Frames wider than this are downscaled before upload: a wallpaper
/// never needs 4K texels, and smaller frames keep per-frame copies cheap.
const MAX_FRAME_WIDTH: usize = 1920;

/// The decoder pauses when no frame was requested for this long.
const DEMAND_TIMEOUT: Duration = Duration::from_secs(2);

/// UI refresh cadence while a video is playing (roughly 30 fps).
const REFRESH_INTERVAL: Duration = Duration::from_millis(33);

/// Energy saver: a quarter of the pixels per frame (960 px wide) at 20 fps.
/// The wallpaper sits behind translucent UI, so the drop is barely visible.
const SAVER_FRAME_WIDTH: usize = 960;
const SAVER_REFRESH_INTERVAL: Duration = Duration::from_millis(50);

/// How hard playback works: decode width and publish cadence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Quality {
    max_width: usize,
    interval: Duration,
}

impl Quality {
    const FULL: Self = Self {
        max_width: MAX_FRAME_WIDTH,
        interval: REFRESH_INTERVAL,
    };
    const SAVER: Self = Self {
        max_width: SAVER_FRAME_WIDTH,
        interval: SAVER_REFRESH_INTERVAL,
    };

    fn for_energy_saver(saving: bool) -> Self {
        if saving { Self::SAVER } else { Self::FULL }
    }
}

/// Largest accepted video file (bytes).
pub(crate) const MAX_VIDEO_BYTES: u64 = 2 * 1024 * 1024 * 1024;

pub(crate) fn is_video(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| VIDEO_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()))
}

/// Total duration in seconds for the trim bar; cached per path.
pub(crate) fn duration(path: &Path) -> Option<f64> {
    static CACHE: OnceLock<Mutex<Option<(PathBuf, Option<f64>)>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some((cached, value)) = cache.lock().unwrap().as_ref()
        && cached == path
    {
        return *value;
    }
    #[cfg(target_os = "macos")]
    let value = macos::duration(path);
    #[cfg(not(target_os = "macos"))]
    let value = None;
    *cache.lock().unwrap() = Some((path.to_path_buf(), value));
    value
}

/// Quarter turn the file's own capture transform asks for. A portrait phone
/// video stores landscape pixels plus a rotation matrix; the decoder reads raw
/// pixels, so playback has to turn them itself. Mirrored captures (front
/// camera) report 0: guessing would be worse than leaving them untouched.
pub(crate) fn capture_rotation(path: &Path) -> u16 {
    static CACHE: OnceLock<Mutex<Vec<(PathBuf, u16)>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some((_, rotation)) = cache
        .lock()
        .unwrap()
        .iter()
        .find(|(cached, _)| cached == path)
    {
        return *rotation;
    }
    #[cfg(target_os = "macos")]
    let rotation = macos::capture_rotation(path);
    #[cfg(not(target_os = "macos"))]
    let rotation = 0;
    let mut cache = cache.lock().unwrap();
    cache.push((path.to_path_buf(), rotation));
    if cache.len() > 8 {
        cache.remove(0);
    }
    rotation
}

/// Quarter turn a track's capture transform asks for, or 0 when the transform
/// mirrors the frame: front-camera clips need a flip, not a turn, and guessing
/// would look worse than leaving the pixels alone.
fn rotation_from_transform(a: f64, b: f64, c: f64, d: f64) -> u16 {
    if a * d - b * c < 0.0 {
        return 0;
    }
    let degrees = b.atan2(a).to_degrees().round();
    if !degrees.is_finite() {
        return 0;
    }
    crate::settings::normalize_rotation(degrees.rem_euclid(360.0) as u16)
}

/// A still frame near `seconds`, for the trim bar's scrub preview. Decoded
/// off the UI thread and cached per (path, rotation, 0.2 s) while a handle is held.
pub(crate) fn frame_at(path: &Path, seconds: f64, rotation: u16) -> Option<Arc<gpui::RenderImage>> {
    let rotation = crate::settings::normalize_rotation(rotation);
    let quantized = (seconds.max(0.0) * 5.0).round() / 5.0;
    static CACHE: OnceLock<Mutex<Vec<(PathBuf, u16, f64, Arc<gpui::RenderImage>)>>> =
        OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some((_, _, _, frame)) =
        cache
            .lock()
            .unwrap()
            .iter()
            .find(|(cached, cached_rotation, at, _)| {
                cached == path && *cached_rotation == rotation && *at == quantized
            })
    {
        return Some(frame.clone());
    }
    #[cfg(target_os = "macos")]
    let pixels = macos::frame_at(path, quantized)
        .map(|frame| crate::new_thread_background_image::rotate(frame, rotation));
    #[cfg(not(target_os = "macos"))]
    let pixels = None;
    let frame = Arc::new(gpui::RenderImage::new([image::Frame::new(pixels?)]));
    let mut cache = cache.lock().unwrap();
    cache.push((path.to_path_buf(), rotation, quantized, frame.clone()));
    if cache.len() > 8 {
        cache.remove(0);
    }
    Some(frame)
}

/// `count` small stills spread evenly over the video, for the trim bar's
/// filmstrip. Slow (one decode per still): call off the UI thread.
pub(crate) fn filmstrip(path: &Path, count: usize, rotation: u16) -> Vec<Arc<gpui::RenderImage>> {
    #[cfg(target_os = "macos")]
    let frames = macos::filmstrip(path, count);
    #[cfg(not(target_os = "macos"))]
    let frames: Vec<image::RgbaImage> = {
        let _ = (path, count);
        Vec::new()
    };
    frames
        .into_iter()
        .map(|pixels| {
            Arc::new(gpui::RenderImage::new([image::Frame::new(
                crate::new_thread_background_image::rotate(pixels, rotation),
            )]))
        })
        .collect()
}

/// The treatment a frame is painted under.
#[derive(Clone, Copy, PartialEq)]
struct Treatment {
    effect: crate::settings::NewThreadBackgroundEffect,
    light: bool,
    blur: f32,
    rotation: u16,
}

impl Treatment {
    /// Paints the frame exactly as decoded (no effect, blur or turn).
    fn is_identity(&self) -> bool {
        crate::new_thread_background_effects::frame_effect_is_identity(
            self.effect,
            self.blur,
            self.rotation,
        )
    }
}

/// A decoded frame ready to paint. Under a treatment the untouched source is
/// kept too, so a treatment change can re-render a held frame without
/// decoding. Without one the painted image *is* the source: no second
/// multi-megabyte copy per frame.
struct Slot {
    raw: Option<Arc<image::RgbaImage>>,
    painted: Arc<gpui::RenderImage>,
    treatment: Treatment,
}

impl Slot {
    fn new(frame: image::RgbaImage, treatment: Treatment) -> Self {
        if treatment.is_identity() {
            return Self {
                raw: None,
                painted: Arc::new(gpui::RenderImage::new([image::Frame::new(frame)])),
                treatment,
            };
        }
        let raw = Arc::new(frame);
        Self {
            painted: paint(&raw, treatment),
            raw: Some(raw),
            treatment,
        }
    }

    /// Rebuild from a source frame (`Arc` to avoid copying a kept one).
    fn from_source(source: Arc<image::RgbaImage>, treatment: Treatment) -> Self {
        if treatment.is_identity() {
            let frame = Arc::try_unwrap(source).unwrap_or_else(|shared| (*shared).clone());
            return Self::new(frame, treatment);
        }
        Self {
            painted: paint(&source, treatment),
            raw: Some(source),
            treatment,
        }
    }
}

/// The untouched pixels behind `painted`: the kept source, or the painted
/// image itself when it was painted without a treatment.
fn source_of(
    raw: Option<Arc<image::RgbaImage>>,
    painted: &gpui::RenderImage,
) -> Option<Arc<image::RgbaImage>> {
    if raw.is_some() {
        return raw;
    }
    let size = painted.size(0);
    let bytes = painted.as_bytes(0)?.to_vec();
    image::RgbaImage::from_raw(size.width.0 as u32, size.height.0 as u32, bytes).map(Arc::new)
}

struct Shared {
    latest: Mutex<Option<Slot>>,
    /// Frames replaced since the last `release_retired`.
    retired: Mutex<Vec<Arc<gpui::RenderImage>>>,
    /// Treatment requested by the UI thread; applied to every new frame.
    treatment: Mutex<Treatment>,
    /// Decode width and cadence; fixed for the life of a player.
    quality: Quality,
    /// Playback speed (f32 bits), refreshed from settings.
    speed: AtomicU32,
    /// Loop segment start in source seconds (f32 bits), refreshed from settings.
    start: AtomicU32,
    /// Loop segment end in source seconds (f32 bits); 0 plays to the end.
    end: AtomicU32,
    /// Source time of the newest published frame (f64 bits), so a
    /// replacement player (quality change) resumes instead of restarting.
    position: AtomicU64,
    /// A held frame is already being re-rendered for a new treatment.
    reprocessing: AtomicBool,
    generation: AtomicU64,
    stop: AtomicBool,
    /// Milliseconds since `epoch` of the latest frame request.
    demanded_at: AtomicU64,
    epoch: Instant,
    /// The decoder thread, once started; unparked by demand and stop.
    decoder: OnceLock<std::thread::Thread>,
    /// The decoder sleeps for lack of demand and needs an unpark to resume.
    parked: AtomicBool,
}

impl Shared {
    fn new(treatment: Treatment, quality: Quality) -> Self {
        Self {
            latest: Mutex::new(None),
            retired: Mutex::new(Vec::new()),
            treatment: Mutex::new(treatment),
            quality,
            speed: AtomicU32::new(crate::settings::VIDEO_SPEED_DEFAULT.to_bits()),
            start: AtomicU32::new(0.0f32.to_bits()),
            end: AtomicU32::new(0.0f32.to_bits()),
            position: AtomicU64::new(0.0f64.to_bits()),
            reprocessing: AtomicBool::new(false),
            generation: AtomicU64::new(0),
            stop: AtomicBool::new(false),
            demanded_at: AtomicU64::new(0),
            epoch: Instant::now(),
            decoder: OnceLock::new(),
            parked: AtomicBool::new(false),
        }
    }

    fn demand(&self) {
        self.demanded_at
            .store(self.epoch.elapsed().as_millis() as u64, Ordering::SeqCst);
        // Paired with the decoder's store-then-recheck before parking: either
        // it sees this demand, or this sees it parked and wakes it.
        if self.parked.load(Ordering::SeqCst) {
            self.wake();
        }
    }

    fn wake(&self) {
        if let Some(thread) = self.decoder.get() {
            thread.unpark();
        }
    }

    fn halt(&self) {
        self.stop.store(true, Ordering::SeqCst);
        self.wake();
    }

    fn demanded_recently(&self) -> bool {
        let last = Duration::from_millis(self.demanded_at.load(Ordering::SeqCst));
        self.epoch.elapsed().saturating_sub(last) < DEMAND_TIMEOUT
    }

    fn position(&self) -> f64 {
        f64::from_bits(self.position.load(Ordering::Relaxed))
    }

    /// Playback rate for the timeline; never frozen or reversed.
    fn speed(&self) -> f32 {
        f32::from_bits(self.speed.load(Ordering::Relaxed)).clamp(
            crate::settings::VIDEO_SPEED_MIN,
            crate::settings::VIDEO_SPEED_MAX,
        )
    }

    /// Loop segment start in source seconds.
    fn start(&self) -> f64 {
        f64::from(f32::from_bits(self.start.load(Ordering::Relaxed)).max(0.0))
    }

    /// Loop segment end in source seconds; 0 plays to the end.
    fn end(&self) -> f64 {
        f64::from(f32::from_bits(self.end.load(Ordering::Relaxed)).max(0.0))
    }
    fn treatment(&self) -> Treatment {
        *self.treatment.lock().unwrap()
    }

    /// Publish a freshly decoded BGRA frame at source time `time` under the
    /// current treatment. A stopped player drops it: its slot may already
    /// have been handed off, and nothing would ever release a late frame.
    fn publish(&self, frame: image::RgbaImage, time: f64) {
        let slot = Slot::new(frame, self.treatment());
        let mut latest = self.latest.lock().unwrap();
        if self.stop.load(Ordering::SeqCst) {
            return;
        }
        if let Some(previous) = latest.replace(slot) {
            // Lock order everywhere: `latest`, then `retired`.
            self.retired.lock().unwrap().push(previous.painted);
        }
        drop(latest);
        self.position.store(time.to_bits(), Ordering::Relaxed);
        self.generation.fetch_add(1, Ordering::Release);
    }
}

/// Render one BGRA frame under a treatment, ready for the GPU.
fn paint(raw: &Arc<image::RgbaImage>, treatment: Treatment) -> Arc<gpui::RenderImage> {
    Arc::new(gpui::RenderImage::new([image::Frame::new(
        crate::new_thread_background_effects::render_frame_effect(
            raw,
            treatment.effect,
            treatment.light,
            treatment.blur,
            treatment.rotation,
        ),
    )]))
}

struct Player {
    path: PathBuf,
    shared: Arc<Shared>,
}

impl Drop for Player {
    fn drop(&mut self) {
        self.shared.halt();
    }
}

#[derive(Default)]
struct Registry {
    player: Option<Player>,
    /// Frames of a stopped player still awaiting atlas release.
    orphaned: Vec<Arc<gpui::RenderImage>>,
    refresh_task_running: bool,
    /// Windows that painted the video recently; only these are repainted at
    /// the video cadence (not the pet, not other windows).
    painting: Vec<(gpui::AnyWindowHandle, Instant)>,
}

fn registry() -> &'static Mutex<Registry> {
    static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
    REGISTRY.get_or_init(Default::default)
}

/// The frame to paint for the video at `path`, starting playback on first
/// use. `None` until the first frame has decoded. With `animate` false the
/// current frame is held still (reduced motion / unfocused window). Energy
/// saver plays a lighter stream instead of stopping.
pub(crate) fn current_frame(
    path: &Path,
    animate: bool,
    effect: crate::settings::NewThreadBackgroundEffect,
    light: bool,
    blur: f32,
    rotation: u16,
    cx: &mut gpui::App,
) -> Option<Arc<gpui::RenderImage>> {
    // The trim stills come out of AVFoundation already turned upright, so the
    // played frames get the same turn here and the two always agree.
    let treatment = Treatment {
        effect,
        light,
        blur,
        rotation: crate::settings::normalize_rotation(
            rotation.wrapping_add(capture_rotation(path)),
        ),
    };
    let quality = Quality::for_energy_saver(crate::motion::energy_saver(cx));
    let (speed, start, end) = crate::settings::new_thread_video_playback(cx);
    let mut registry = registry().lock().unwrap();
    if registry
        .player
        .as_ref()
        .is_none_or(|player| player.path != path || player.shared.quality != quality)
    {
        let shared = Arc::new(Shared::new(treatment, quality));
        let mut resume_at = None;
        if let Some(old) = registry.player.take() {
            if old.path == path {
                // Same video, new quality: keep the held frame on screen and
                // continue from it rather than flashing blank and restarting.
                old.shared.halt();
                let held = old.shared.latest.lock().unwrap().take();
                let retired = std::mem::take(&mut *old.shared.retired.lock().unwrap());
                registry.orphaned.extend(retired);
                if let Some(held) = held {
                    resume_at = Some(old.shared.position());
                    *shared.latest.lock().unwrap() = Some(held);
                    shared.generation.fetch_add(1, Ordering::Release);
                }
            } else {
                orphan(&mut registry, &old);
            }
        }
        shared.speed.store(speed.to_bits(), Ordering::Relaxed);
        shared.start.store(start.to_bits(), Ordering::Relaxed);
        shared.end.store(end.to_bits(), Ordering::Relaxed);
        shared.demand();
        spawn_decoder(path.to_path_buf(), shared.clone(), resume_at);
        registry.player = Some(Player {
            path: path.to_path_buf(),
            shared,
        });
    }
    let player = registry.player.as_ref().unwrap();
    let shared = player.shared.clone();
    *shared.treatment.lock().unwrap() = treatment;
    shared.speed.store(speed.to_bits(), Ordering::Relaxed);
    shared.start.store(start.to_bits(), Ordering::Relaxed);
    shared.end.store(end.to_bits(), Ordering::Relaxed);
    let (frame, stale) = {
        let latest = shared.latest.lock().unwrap();
        (
            latest.as_ref().map(|slot| slot.painted.clone()),
            latest
                .as_ref()
                .is_some_and(|slot| slot.treatment != treatment),
        )
    };
    // Without demand the decoder idles after its current frame: that is
    // also how a still (non-animated) frame is held.
    if animate || frame.is_none() {
        shared.demand();
    }
    // A first frame must reach the screen even when held still, so the
    // refresh task also runs until it lands (it exits once demand lapses).
    let reprocess = stale && !shared.reprocessing.swap(true, Ordering::Relaxed);
    let mut refresh = false;
    if (animate || frame.is_none()) && !registry.refresh_task_running {
        registry.refresh_task_running = true;
        refresh = true;
    }
    drop(registry);
    if reprocess {
        spawn_reprocess(shared, cx);
    }
    if refresh {
        spawn_refresh_task(cx);
    }
    frame
}

/// Re-render the held frame after the treatment changed, without decoding.
/// The decoder applies the new treatment to its next frame on its own.
fn spawn_reprocess(shared: Arc<Shared>, cx: &mut gpui::App) {
    let treatment = shared.treatment();
    let held = shared
        .latest
        .lock()
        .unwrap()
        .as_ref()
        .map(|slot| (slot.raw.clone(), slot.painted.clone()));
    let Some((raw, painted)) = held else {
        shared.reprocessing.store(false, Ordering::Relaxed);
        return;
    };
    cx.spawn(async move |cx| {
        let source = painted.clone();
        let slot = cx
            .background_executor()
            .spawn(async move {
                source_of(raw, &source).map(|source| Slot::from_source(source, treatment))
            })
            .await;
        cx.update(|cx| {
            {
                let mut latest = shared.latest.lock().unwrap();
                if let Some(current) = latest.as_mut()
                    && let Some(slot) = slot
                    && Arc::ptr_eq(&current.painted, &painted)
                    && current.treatment != treatment
                {
                    let previous = std::mem::replace(current, slot);
                    shared.retired.lock().unwrap().push(previous.painted);
                }
            }
            shared.reprocessing.store(false, Ordering::Relaxed);
            cx.refresh_windows();
        });
    })
    .detach();
}

/// Stop any running playback (the background is no longer a video).
pub(crate) fn stop(cx: &mut gpui::App) {
    let _ = cx;
    let mut registry = registry().lock().unwrap();
    if let Some(old) = registry.player.take() {
        orphan(&mut registry, &old);
    }
    registry.painting.clear();
}

fn orphan(registry: &mut Registry, player: &Player) {
    player.shared.halt();
    // `latest` before `retired`, like `publish`: once stopped (checked under
    // the `latest` lock) the decoder adds nothing either list could miss.
    let latest = player.shared.latest.lock().unwrap().take();
    let retired = std::mem::take(&mut *player.shared.retired.lock().unwrap());
    registry.orphaned.extend(retired);
    if let Some(latest) = latest {
        registry.orphaned.push(latest.painted);
    }
}

/// Free atlas entries of frames that are no longer painted. Call from the
/// window that paints the video, before painting the next frame; that window
/// is then repainted at the video cadence.
pub(crate) fn release_retired(window: &mut gpui::Window) {
    let retired = {
        let mut registry = registry().lock().unwrap();
        let mut retired = std::mem::take(&mut registry.orphaned);
        if let Some(player) = registry.player.as_ref() {
            retired.extend(std::mem::take(&mut *player.shared.retired.lock().unwrap()));
            let handle = window.window_handle();
            let now = Instant::now();
            match registry
                .painting
                .iter_mut()
                .find(|(painting, _)| *painting == handle)
            {
                Some((_, at)) => *at = now,
                None => registry.painting.push((handle, now)),
            }
        }
        retired
    };
    for frame in retired {
        let _ = window.drop_image(frame);
    }
}

/// Repaint the painting windows while a video is playing, at the video
/// cadence rather than the display refresh rate. Exits once playback stops
/// or loses demand.
fn spawn_refresh_task(cx: &mut gpui::App) {
    cx.spawn(async move |cx| {
        let mut seen = u64::MAX;
        let mut interval = REFRESH_INTERVAL;
        loop {
            cx.background_executor().timer(interval).await;
            let state = {
                let mut registry = registry().lock().unwrap();
                // A window that stopped painting the video (closed, other
                // route) drops off after the demand window.
                registry
                    .painting
                    .retain(|(_, at)| at.elapsed() < DEMAND_TIMEOUT);
                let windows: Vec<_> = registry
                    .painting
                    .iter()
                    .map(|(handle, _)| *handle)
                    .collect();
                registry.player.as_ref().map(|player| {
                    (
                        player.shared.generation.load(Ordering::Acquire),
                        player.shared.demanded_recently(),
                        player.shared.quality.interval,
                        windows,
                    )
                })
            };
            match state {
                Some((generation, true, cadence, windows)) => {
                    interval = cadence;
                    if generation != seen {
                        seen = generation;
                        cx.update(|cx| {
                            if windows.is_empty() {
                                // Nothing painted yet (first frame): any
                                // window may be the one waiting for it.
                                cx.refresh_windows();
                            }
                            for handle in windows {
                                let _ = handle.update(cx, |_, window, _| window.refresh());
                            }
                        });
                    }
                }
                _ => {
                    registry().lock().unwrap().refresh_task_running = false;
                    // A frame request may have raced the exit; the next
                    // `current_frame` call restarts the task.
                    break;
                }
            }
        }
    })
    .detach();
}

/// Decode the first frame (validation and wallpaper color sampling).
pub(crate) fn first_frame(path: &Path) -> Result<image::RgbaImage, String> {
    #[cfg(target_os = "macos")]
    {
        let mut decoder = macos::Decoder::open(path, MAX_FRAME_WIDTH)?;
        let frame = decoder
            .next_frame()
            .ok_or_else(|| "This video has no decodable frames.".to_string())?;
        // BGRA → RGBA for color sampling.
        let mut rgba = frame.pixels;
        for pixel in rgba.pixels_mut() {
            pixel.0.swap(0, 2);
        }
        Ok(rgba)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = path;
        Err("Video backgrounds are only supported on macOS.".into())
    }
}

fn spawn_decoder(path: PathBuf, shared: Arc<Shared>, resume_at: Option<f64>) {
    let spawned = std::thread::Builder::new()
        .name("zeron-video-wallpaper".into())
        .spawn(move || {
            let _ = shared.decoder.set(std::thread::current());
            #[cfg(target_os = "macos")]
            macos::run(&path, &shared, resume_at);
            #[cfg(not(target_os = "macos"))]
            let _ = (&path, &shared, resume_at);
        });
    if let Err(error) = spawned {
        tracing::warn!(%error, "could not start the video wallpaper decoder");
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use core::ffi::c_void;
    use objc2::rc::{Retained, autoreleasepool};
    use objc2::runtime::AnyObject;
    use objc2_av_foundation::{
        AVAsset, AVAssetImageGenerator, AVAssetReader, AVAssetReaderOutput, AVAssetReaderStatus,
        AVAssetReaderTrackOutput, AVMediaTypeVideo, AVURLAsset,
    };
    use objc2_core_foundation::{CGPoint, CGRect, CGSize};
    use objc2_core_graphics::{
        CGBitmapContextCreate, CGColorSpaceCreateDeviceRGB, CGContextDrawImage, CGImage,
        CGImageAlphaInfo, CGImageByteOrderInfo, CGImageGetHeight, CGImageGetWidth,
    };
    use objc2_core_media::CMTime;
    use objc2_core_video::{
        CVPixelBufferGetBaseAddress, CVPixelBufferGetBytesPerRow, CVPixelBufferGetHeight,
        CVPixelBufferGetWidth, CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags,
        CVPixelBufferUnlockBaseAddress, kCVPixelBufferHeightKey, kCVPixelBufferPixelFormatTypeKey,
        kCVPixelBufferWidthKey, kCVPixelFormatType_32BGRA,
    };
    use objc2_foundation::{NSDictionary, NSNumber, NSString, NSURL};

    pub(super) struct Frame {
        /// BGRA pixels, as `RenderImage` expects.
        pub pixels: image::RgbaImage,
        /// Presentation time in seconds from the start of the file.
        #[cfg_attr(not(test), allow(dead_code))]
        pub time: f64,
    }

    /// Quarter turn encoded in the track's capture transform, or 0.
    pub(super) fn capture_rotation(path: &Path) -> u16 {
        let Some(url) = NSURL::from_file_path(path) else {
            return 0;
        };
        let asset: Retained<AVAsset> =
            unsafe { Retained::into_super(AVURLAsset::assetWithURL(&url)) };
        let Some(media_type) = (unsafe { AVMediaTypeVideo }) else {
            return 0;
        };
        #[allow(deprecated)]
        let tracks = unsafe { asset.tracksWithMediaType(media_type) };
        let Some(track) = tracks.firstObject() else {
            return 0;
        };
        #[allow(deprecated)]
        let transform = unsafe { track.preferredTransform() };
        let (a, b, c, d) = (transform.a, transform.b, transform.c, transform.d);
        super::rotation_from_transform(a, b, c, d)
    }

    /// Media duration without decoding; used by the trim bar.
    pub(super) fn duration(path: &Path) -> Option<f64> {
        let url = NSURL::from_file_path(path)?;
        let asset: Retained<AVAsset> =
            unsafe { Retained::into_super(AVURLAsset::assetWithURL(&url)) };
        #[allow(deprecated)]
        let time = unsafe { asset.duration() };
        let seconds = unsafe { time.seconds() };
        seconds.is_finite().then_some(seconds)
    }

    /// An image generator that returns the frame at the requested time, not
    /// the nearest keyframe (the default tolerance), so previews match trims.
    #[allow(deprecated)]
    fn generator(path: &Path, max_side: f64) -> Option<Retained<AVAssetImageGenerator>> {
        let url = NSURL::from_file_path(path)?;
        let asset: Retained<AVAsset> =
            unsafe { Retained::into_super(AVURLAsset::assetWithURL(&url)) };
        let generator = unsafe { AVAssetImageGenerator::assetImageGeneratorWithAsset(&asset) };
        unsafe {
            generator.setAppliesPreferredTrackTransform(true);
            generator.setMaximumSize(CGSize {
                width: max_side,
                height: max_side,
            });
            let exact = CMTime::new(0, 1000);
            generator.setRequestedTimeToleranceBefore(exact);
            generator.setRequestedTimeToleranceAfter(exact);
        }
        Some(generator)
    }

    #[allow(deprecated)]
    fn still(generator: &AVAssetImageGenerator, seconds: f64) -> Option<image::RgbaImage> {
        let time = unsafe { CMTime::new((seconds.max(0.0) * 1000.0) as i64, 1000) };
        let image =
            unsafe { generator.copyCGImageAtTime_actualTime_error(time, std::ptr::null_mut()) }
                .ok()?;
        rasterize(&image)
    }

    /// A still at `seconds`, rasterized through AVAssetImageGenerator.
    pub(super) fn frame_at(path: &Path, seconds: f64) -> Option<image::RgbaImage> {
        autoreleasepool(|_| still(&*generator(path, 320.0)?, seconds))
    }

    /// `count` stills centred in equal slices of the video.
    pub(super) fn filmstrip(path: &Path, count: usize) -> Vec<image::RgbaImage> {
        let Some(total) = duration(path).filter(|total| *total > 0.0) else {
            return Vec::new();
        };
        autoreleasepool(|_| {
            let Some(generator) = generator(path, 96.0) else {
                return Vec::new();
            };
            (0..count)
                .filter_map(|ix| still(&generator, total * (ix as f64 + 0.5) / count as f64))
                .collect()
        })
    }

    /// Rasterize a CGImage into BGRA pixels for `RenderImage`.
    pub(super) fn rasterize(image: &CGImage) -> Option<image::RgbaImage> {
        let width = CGImageGetWidth(Some(image));
        let height = CGImageGetHeight(Some(image));
        if width == 0 || height == 0 {
            return None;
        }
        let colors = CGColorSpaceCreateDeviceRGB()?;
        let mut data = vec![0u8; width * height * 4];
        let context = unsafe {
            CGBitmapContextCreate(
                data.as_mut_ptr() as *mut c_void,
                width,
                height,
                8,
                width * 4,
                Some(&colors),
                CGImageAlphaInfo::PremultipliedLast.0 | CGImageByteOrderInfo::Order32Big.0,
            )
        }?;
        // A bitmap context stores row 0 at the top, so drawing as-is keeps the
        // frame upright (a CTM flip here turned stills upside down relative to
        // the frames the player decodes).
        CGContextDrawImage(
            Some(&context),
            CGRect {
                origin: CGPoint { x: 0.0, y: 0.0 },
                size: CGSize {
                    width: width as f64,
                    height: height as f64,
                },
            },
            Some(image),
        );
        drop(context);
        // RGBA -> BGRA for `RenderImage`.
        for pixel in data.chunks_exact_mut(4) {
            pixel.swap(0, 2);
        }
        image::RgbaImage::from_raw(width as u32, height as u32, data)
    }

    pub(super) struct Decoder {
        asset: Retained<AVAsset>,
        reader: Retained<AVAssetReader>,
        output: Retained<AVAssetReaderTrackOutput>,
        /// Frames are decoded and copied at most this wide.
        max_width: usize,
    }

    /// A decoded frame not yet copied out of its pixel buffer: frames the UI
    /// would never show are dropped without touching their pixels.
    pub(super) struct Sample {
        buffer: objc2_core_foundation::CFRetained<objc2_core_video::CVPixelBuffer>,
        /// Presentation time in seconds from the start of the file.
        pub time: f64,
        max_width: usize,
    }

    impl Sample {
        pub fn pixels(&self) -> Option<image::RgbaImage> {
            copy_pixels(&self.buffer, self.max_width)
        }
    }

    // AVFoundation objects are used from the one decoder thread that owns them.
    unsafe impl Send for Decoder {}

    impl Decoder {
        pub fn open(path: &Path, max_width: usize) -> Result<Self, String> {
            let url = NSURL::from_file_path(path)
                .ok_or_else(|| "The video path is not valid.".to_string())?;
            let asset: Retained<AVAsset> =
                unsafe { Retained::into_super(AVURLAsset::assetWithURL(&url)) };
            Self::start(asset, max_width.max(2))
        }

        fn start(asset: Retained<AVAsset>, max_width: usize) -> Result<Self, String> {
            let unsupported =
                || "This video is unsupported or damaged. Choose an MP4 or MOV file.".to_string();
            let media_type = unsafe { AVMediaTypeVideo }.ok_or_else(unsupported)?;
            #[allow(deprecated)]
            let tracks = unsafe { asset.tracksWithMediaType(media_type) };
            let track = tracks.firstObject().ok_or_else(unsupported)?;
            let cf_key = |key: &objc2_core_foundation::CFString| -> &NSString {
                unsafe { &*(key as *const _ as *const NSString) }
            };
            let format = NSNumber::new_u32(kCVPixelFormatType_32BGRA);
            let mut keys = vec![cf_key(unsafe { kCVPixelBufferPixelFormatTypeKey })];
            let mut values = vec![format];
            // Wide sources (4K+, or anything above the energy saver width):
            // let VideoToolbox scale while decoding instead of
            // box-downscaling every frame on the CPU.
            #[allow(deprecated)]
            let natural = unsafe { track.naturalSize() };
            if natural.width > max_width as f64 && natural.height > 0.0 {
                let width = max_width as f64;
                let height = (natural.height * width / natural.width / 2.0).round() * 2.0;
                keys.push(cf_key(unsafe { kCVPixelBufferWidthKey }));
                values.push(NSNumber::new_u32(width as u32));
                keys.push(cf_key(unsafe { kCVPixelBufferHeightKey }));
                values.push(NSNumber::new_u32(height.max(2.0) as u32));
            }
            let values: Vec<&AnyObject> =
                values.iter().map(|value| &**value as &AnyObject).collect();
            let settings = NSDictionary::<NSString, AnyObject>::from_slices(&keys, &values);
            let output = unsafe {
                AVAssetReaderTrackOutput::assetReaderTrackOutputWithTrack_outputSettings(
                    &track,
                    Some(&settings),
                )
            };
            unsafe { output.setAlwaysCopiesSampleData(false) };
            let reader = unsafe { AVAssetReader::assetReaderWithAsset_error(&asset) }
                .map_err(|_| unsupported())?;
            let as_output: &AVAssetReaderOutput = &output;
            if !unsafe { reader.canAddOutput(as_output) } {
                return Err(unsupported());
            }
            unsafe { reader.addOutput(as_output) };
            if !unsafe { reader.startReading() } {
                return Err(unsupported());
            }
            Ok(Self {
                asset,
                reader,
                output,
                max_width,
            })
        }

        /// Restart from the first frame (looping).
        pub fn rewind(self) -> Result<Self, String> {
            unsafe { self.reader.cancelReading() };
            Self::start(self.asset.clone(), self.max_width)
        }

        pub fn completed(&self) -> bool {
            (unsafe { self.reader.status() }) == AVAssetReaderStatus::Completed
        }

        /// The next decoded sample, pixels not yet copied, or `None` at the
        /// end of the file / on error.
        pub fn next_sample(&mut self) -> Option<Sample> {
            loop {
                let next = autoreleasepool(|_| {
                    let sample = unsafe { self.output.copyNextSampleBuffer() }?;
                    let time = unsafe { sample.presentation_time_stamp().seconds() };
                    // Marker-only samples carry no image: keep reading.
                    Some(unsafe { sample.image_buffer() }.map(|buffer| Sample {
                        buffer,
                        time: if time.is_finite() { time } else { 0.0 },
                        max_width: self.max_width,
                    }))
                })?;
                if let Some(sample) = next {
                    return Some(sample);
                }
            }
        }

        /// The next decoded frame, or `None` at the end of the file / on error.
        pub fn next_frame(&mut self) -> Option<Frame> {
            loop {
                let sample = self.next_sample()?;
                if let Some(pixels) = sample.pixels() {
                    return Some(Frame {
                        pixels,
                        time: sample.time,
                    });
                }
            }
        }
    }

    /// Copy a locked BGRA pixel buffer into an owned image, box-downscaling
    /// frames wider than `max_width` by an integer factor.
    fn copy_pixels(
        buffer: &objc2_core_video::CVPixelBuffer,
        max_width: usize,
    ) -> Option<image::RgbaImage> {
        let flags = CVPixelBufferLockFlags::ReadOnly;
        if unsafe { CVPixelBufferLockBaseAddress(buffer, flags) } != 0 {
            return None;
        }
        let width = CVPixelBufferGetWidth(buffer);
        let height = CVPixelBufferGetHeight(buffer);
        let stride = CVPixelBufferGetBytesPerRow(buffer);
        let base = CVPixelBufferGetBaseAddress(buffer) as *const u8;
        let result =
            (!base.is_null() && width > 0 && height > 0 && stride >= width * 4).then(|| {
                let source = unsafe { std::slice::from_raw_parts(base, stride * height) };
                downscale(source, width, height, stride, max_width)
            });
        unsafe { CVPixelBufferUnlockBaseAddress(buffer, flags) };
        result
    }

    fn downscale(
        source: &[u8],
        width: usize,
        height: usize,
        stride: usize,
        max_width: usize,
    ) -> image::RgbaImage {
        let factor = width.div_ceil(max_width.max(1)).max(1);
        let out_w = (width / factor).max(1);
        let out_h = (height / factor).max(1);
        let mut out = vec![0u8; out_w * out_h * 4];
        if factor == 1 {
            for y in 0..out_h {
                let row = &source[y * stride..y * stride + width * 4];
                out[y * width * 4..(y + 1) * width * 4].copy_from_slice(row);
            }
        } else {
            let area = (factor * factor) as u32;
            for oy in 0..out_h {
                for ox in 0..out_w {
                    let mut sum = [0u32; 4];
                    for dy in 0..factor {
                        let row = (oy * factor + dy) * stride;
                        for dx in 0..factor {
                            let at = row + (ox * factor + dx) * 4;
                            for c in 0..4 {
                                sum[c] += source[at + c] as u32;
                            }
                        }
                    }
                    let at = (oy * out_w + ox) * 4;
                    for c in 0..4 {
                        out[at + c] = (sum[c] / area) as u8;
                    }
                }
            }
        }
        image::RgbaImage::from_raw(out_w as u32, out_h as u32, out).expect("frame buffer size")
    }

    /// Decoder thread body: publish frames at their presentation times, loop
    /// at the end, park without demand, exit on stop or a broken file.
    /// `resume_at` (source seconds) continues a replaced player's playback.
    pub(super) fn run(path: &Path, shared: &Shared, resume_at: Option<f64>) {
        let quality = shared.quality;
        let mut decoder = match Decoder::open(path, quality.max_width) {
            Ok(decoder) => decoder,
            Err(error) => {
                tracing::warn!(%error, path = %path.display(), "video wallpaper failed to open");
                return;
            }
        };
        let mut loop_start = Instant::now();
        let mut frames_this_loop = 0usize;
        let mut last_time = 0.0f64;
        let mut paused = false;
        let mut last_due: Option<Instant> = None;
        // Frames before this are skipped without copying their pixels.
        let mut resume = resume_at.filter(|at| at.is_finite() && *at > 0.0);
        let held = |shared: &Shared| shared.latest.lock().unwrap().is_some();
        while !shared.stop.load(Ordering::SeqCst) {
            if !shared.demanded_recently() && held(shared) {
                // Idle: park until demand or stop wakes the thread. The
                // recheck after raising `parked` closes the race with
                // `Shared::demand`; the timeout is only a safety net.
                paused = true;
                shared.parked.store(true, Ordering::SeqCst);
                if !shared.demanded_recently() && !shared.stop.load(Ordering::SeqCst) {
                    std::thread::park_timeout(Duration::from_secs(5));
                }
                shared.parked.store(false, Ordering::SeqCst);
                continue;
            }
            if paused {
                // Resume where playback stopped rather than fast-forwarding.
                paused = false;
                let segment = (last_time - shared.start()).max(0.0);
                loop_start =
                    Instant::now() - Duration::from_secs_f64(segment / shared.speed() as f64);
            }
            let Some(sample) = decoder.next_sample() else {
                if !decoder.completed() || (frames_this_loop == 0 && resume.is_none()) {
                    // Read error, or a file without frames: never spin.
                    return;
                }
                decoder = match decoder.rewind() {
                    Ok(decoder) => decoder,
                    Err(_) => return,
                };
                resume = None;
                loop_start = Instant::now();
                frames_this_loop = 0;
                continue;
            };
            // The trim bar sets the loop segment: skip its head, restart at its tail.
            if sample.time < shared.start() {
                continue;
            }
            let end = shared.end();
            if end > shared.start() && sample.time >= end {
                if frames_this_loop == 0 && resume.is_none() {
                    // The segment holds no frames; never spin on a rewind loop.
                    return;
                }
                decoder = match decoder.rewind() {
                    Ok(decoder) => decoder,
                    Err(_) => return,
                };
                resume = None;
                loop_start = Instant::now();
                frames_this_loop = 0;
                last_time = shared.start();
                continue;
            }
            if let Some(at) = resume {
                if sample.time < at {
                    continue;
                }
                // Continue the timeline from here, as if it never stopped.
                resume = None;
                let segment = (sample.time - shared.start()).max(0.0);
                loop_start =
                    Instant::now() - Duration::from_secs_f64(segment / shared.speed() as f64);
            }
            frames_this_loop += 1;
            last_time = sample.time;
            let segment = (sample.time - shared.start()).max(0.0);
            let due = loop_start + Duration::from_secs_f64(segment / shared.speed() as f64);
            // The UI repaints at the quality's cadence: a frame due sooner than
            // that after the last published one would never be seen, so skip
            // its pixel copy and effect work.
            if last_due.is_some_and(|last| {
                due.checked_duration_since(last)
                    .is_some_and(|gap| gap + Duration::from_millis(4) < quality.interval)
            }) && held(shared)
            {
                continue;
            }
            last_due = Some(due);
            let now = Instant::now();
            if due > now {
                std::thread::sleep(due - now);
            } else if now - due > Duration::from_millis(100) && held(shared) {
                // Running behind (e.g. after a stall): drop this frame.
                continue;
            }
            if let Some(pixels) = sample.pixels() {
                shared.publish(pixels, sample.time);
            }
        }
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    /// A system movie shipped with macOS; the test is skipped without it.
    const SAMPLE: &str = "/System/Library/CoreServices/ControlCenter.app/Contents/Resources/BentoGalleryIntroduction.mov";

    #[test]
    fn recognizes_video_extensions() {
        assert!(is_video(Path::new("a.MP4")));
        assert!(is_video(Path::new("b.mov")));
        assert!(is_video(Path::new("c.m4v")));
        assert!(!is_video(Path::new("d.png")));
        assert!(!is_video(Path::new("e")));
    }

    #[test]
    fn scrub_preview_rasterizes_a_still_near_the_requested_time() {
        let path = Path::new(SAMPLE);
        if !path.exists() {
            return;
        }
        let frame = frame_at(path, 0.5, 0).expect("frame near 0.5 s");
        assert!(frame.size(0).width.0 > 0);
        // The cache answers a repeated request with the very same frame.
        assert!(Arc::ptr_eq(
            &frame,
            &frame_at(path, 0.5, 0).expect("cached frame")
        ));
    }

    #[test]
    fn filmstrip_spans_the_video() {
        let path = Path::new(SAMPLE);
        if !path.exists() {
            return;
        }
        let frames = filmstrip(path, 4, 0);
        assert_eq!(frames.len(), 4);
        assert!(frames.iter().all(|frame| frame.size(0).width.0 > 0));
    }

    #[test]
    fn decodes_frames_and_loops() {
        let path = Path::new(SAMPLE);
        if !path.exists() {
            return;
        }
        let frame = first_frame(path).expect("first frame");
        assert!(frame.width() > 0 && frame.width() as usize <= MAX_FRAME_WIDTH);
        assert!(frame.height() > 0);

        let mut decoder = macos::Decoder::open(path, MAX_FRAME_WIDTH).unwrap();
        let mut count = 0;
        let mut last = -1.0;
        while let Some(frame) = decoder.next_frame() {
            assert!(frame.time >= last, "timestamps advance");
            last = frame.time;
            count += 1;
            if count > 5000 {
                break;
            }
        }
        assert!(count > 1, "decoded {count} frames");
        assert!(decoder.completed());
        let mut rewound = decoder.rewind().unwrap();
        assert!(rewound.next_frame().is_some(), "loops back to the start");
    }

    fn treatment(effect: crate::settings::NewThreadBackgroundEffect, rotation: u16) -> Treatment {
        Treatment {
            effect,
            light: false,
            blur: 0.0,
            rotation,
        }
    }

    fn pixels() -> image::RgbaImage {
        image::RgbaImage::from_fn(3, 2, |x, y| image::Rgba([x as u8, y as u8, 7, 255]))
    }

    #[test]
    fn untreated_frames_are_painted_without_a_second_copy() {
        let plain = treatment(crate::settings::NewThreadBackgroundEffect::None, 0);
        let slot = Slot::new(pixels(), plain);
        assert!(slot.raw.is_none(), "the painted image is the source");
        // A later treatment change still recovers the exact source pixels.
        let source = source_of(slot.raw.clone(), &slot.painted).expect("source");
        assert_eq!(*source, pixels());

        let turned = Slot::from_source(source, treatment(plain.effect, 90));
        assert!(turned.raw.is_some(), "a treated frame keeps its source");
        assert_eq!(turned.painted.size(0).width.0, 2, "turned a quarter");
        let back = Slot::from_source(turned.raw.clone().unwrap(), plain);
        assert_eq!(
            back.painted.as_bytes(0).unwrap(),
            pixels().as_raw().as_slice()
        );
    }

    #[test]
    fn a_stopped_player_drops_late_frames() {
        let plain = treatment(crate::settings::NewThreadBackgroundEffect::None, 0);
        let shared = Shared::new(plain, Quality::FULL);
        shared.publish(pixels(), 1.5);
        assert_eq!(shared.position(), 1.5);
        shared.publish(pixels(), 2.0);
        assert_eq!(shared.retired.lock().unwrap().len(), 1);
        shared.halt();
        let before = shared
            .latest
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .painted
            .clone();
        shared.publish(pixels(), 3.0);
        let after = shared
            .latest
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .painted
            .clone();
        assert!(
            Arc::ptr_eq(&before, &after),
            "nothing replaces a handed-off frame"
        );
        assert_eq!(shared.position(), 2.0);
    }

    #[test]
    fn energy_saver_quality_is_lighter() {
        assert_eq!(Quality::for_energy_saver(false), Quality::FULL);
        let saver = Quality::for_energy_saver(true);
        assert!(saver.max_width * 2 <= Quality::FULL.max_width);
        assert!(saver.interval > Quality::FULL.interval);
    }

    #[test]
    fn energy_saver_decodes_smaller_frames() {
        let path = Path::new(SAMPLE);
        if !path.exists() {
            return;
        }
        let mut decoder = macos::Decoder::open(path, SAVER_FRAME_WIDTH).unwrap();
        let frame = decoder.next_frame().expect("a frame");
        assert!(frame.pixels.width() as usize <= SAVER_FRAME_WIDTH);
        assert!(frame.pixels.height() > 0);
    }

    #[test]
    fn idle_decoder_parks_and_wakes_on_demand() {
        let path = Path::new(SAMPLE);
        if !path.exists() {
            return;
        }
        let plain = treatment(crate::settings::NewThreadBackgroundEffect::None, 0);
        let shared = Arc::new(Shared::new(plain, Quality::SAVER));
        shared.demand();
        spawn_decoder(path.to_path_buf(), shared.clone(), None);
        let wait = |what: &str, done: &dyn Fn() -> bool| {
            let deadline = Instant::now() + Duration::from_secs(10);
            while !done() {
                assert!(Instant::now() < deadline, "timed out waiting for {what}");
                std::thread::sleep(Duration::from_millis(20));
            }
        };
        wait("the first frame", &|| {
            shared.generation.load(Ordering::Acquire) > 0
        });
        // Without demand the decoder parks instead of polling.
        wait("the decoder to park", &|| {
            shared.parked.load(Ordering::SeqCst)
        });
        let parked_at = shared.generation.load(Ordering::Acquire);
        shared.demand();
        wait("playback to resume", &|| {
            shared.generation.load(Ordering::Acquire) > parked_at
        });
        shared.halt();
    }

    #[test]
    fn rejects_non_video_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fake.mp4");
        std::fs::write(&path, b"definitely not a movie").unwrap();
        assert!(first_frame(&path).is_err());
    }

    #[test]
    fn capture_transforms_map_to_quarter_turns() {
        // AVFoundation's preferred transform for each upright orientation.
        for (label, transform, expected) in [
            ("upright", (1.0, 0.0, 0.0, 1.0), 0),
            ("turned right", (0.0, 1.0, -1.0, 0.0), 90),
            ("upside down", (-1.0, 0.0, 0.0, -1.0), 180),
            ("turned left", (0.0, -1.0, 1.0, 0.0), 270),
            // Front-camera clips mirror; no quarter turn describes them.
            ("mirrored", (-1.0, 0.0, 0.0, 1.0), 0),
            ("mirrored and turned", (0.0, 1.0, 1.0, 0.0), 0),
        ] {
            let (a, b, c, d) = transform;
            assert_eq!(
                rotation_from_transform(a, b, c, d),
                expected,
                "{label} ({a}, {b}, {c}, {d})"
            );
        }
    }

    /// The player copies pixel-buffer rows top-down. Painting a still through
    /// a flipped context put row 0 at the bottom, so the trim preview and the
    /// filmstrip showed the video upside down.
    #[test]
    fn stills_keep_the_frame_upright() {
        use objc2_core_graphics::{
            CGBitmapInfo, CGColorRenderingIntent, CGColorSpaceCreateDeviceRGB, CGDataProvider,
            CGImage, CGImageAlphaInfo, CGImageByteOrderInfo,
        };
        // RGBA rows: the frame's top row is red, its bottom row is blue.
        let red = [230u8, 40, 40, 255];
        let blue = [40u8, 80, 220, 255];
        let source: Vec<u8> = [red, red, blue, blue].concat();
        let provider = unsafe {
            CGDataProvider::with_data(
                std::ptr::null_mut(),
                source.as_ptr().cast(),
                source.len(),
                None,
            )
        }
        .expect("data provider");
        let space = CGColorSpaceCreateDeviceRGB().expect("device RGB");
        let bitmap_info = CGBitmapInfo(
            CGImageAlphaInfo::PremultipliedLast.0 | CGImageByteOrderInfo::Order32Big.0,
        );
        let image = unsafe {
            CGImage::new(
                2,
                2,
                8,
                32,
                8,
                Some(&space),
                bitmap_info,
                Some(&provider),
                std::ptr::null(),
                false,
                CGColorRenderingIntent::RenderingIntentDefault,
            )
        }
        .expect("two by two image");

        // The raster is BGRA, the order `RenderImage` expects.
        let rasterized = macos::rasterize(&image).expect("rasterized still");
        assert_eq!(rasterized.dimensions(), (2, 2));
        assert_eq!(
            rasterized.get_pixel(0, 0).0,
            [40, 40, 230, 255],
            "the source's top row stays on top"
        );
        assert_eq!(
            rasterized.get_pixel(0, 1).0,
            [220, 80, 40, 255],
            "the source's bottom row stays at the bottom"
        );
    }
}
