//! Video wallpapers. A background thread decodes the file with
//! AVFoundation into BGRA frames; the newest frame is published as a
//! `RenderImage` and painted through the same masked hero path as a still
//! image, so cropping, zoom, the composer cutout, and fades all apply.
//!
//! Playback is demand-driven: the decoder only advances while something has
//! asked for a frame recently, so a hidden window stops costing decode work.
//! Frames the renderer no longer needs are queued for `release_retired`, which
//! frees their GPU atlas entries from the painting window.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Extensions AVFoundation plays natively.
const VIDEO_EXTENSIONS: [&str; 3] = ["mp4", "mov", "m4v"];

/// Frames wider than this are box-downscaled before upload: a wallpaper
/// never needs 4K texels, and smaller frames keep per-frame copies cheap.
const MAX_FRAME_WIDTH: usize = 1920;

/// The decoder pauses when no frame was requested for this long.
const DEMAND_TIMEOUT: Duration = Duration::from_secs(2);

/// UI refresh cadence while a video is playing (roughly 30 fps).
const REFRESH_INTERVAL: Duration = Duration::from_millis(33);

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

/// A still frame near `seconds`, for the trim bar's scrub preview. Decoded
/// off the UI thread and cached per (path, 0.2 s) while a handle is held.
pub(crate) fn frame_at(path: &Path, seconds: f64) -> Option<Arc<gpui::RenderImage>> {
    let quantized = (seconds.max(0.0) * 5.0).round() / 5.0;
    static CACHE: OnceLock<Mutex<Vec<(PathBuf, f64, Arc<gpui::RenderImage>)>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some((_, _, frame)) = cache
        .lock()
        .unwrap()
        .iter()
        .find(|(cached, at, _)| cached == path && *at == quantized)
    {
        return Some(frame.clone());
    }
    #[cfg(target_os = "macos")]
    let pixels = macos::frame_at(path, quantized);
    #[cfg(not(target_os = "macos"))]
    let pixels = None;
    let frame = Arc::new(gpui::RenderImage::new([image::Frame::new(pixels?)]));
    let mut cache = cache.lock().unwrap();
    cache.push((path.to_path_buf(), quantized, frame.clone()));
    if cache.len() > 8 {
        cache.remove(0);
    }
    Some(frame)
}

/// `count` small stills spread evenly over the video, for the trim bar's
/// filmstrip. Slow (one decode per still): call off the UI thread.
pub(crate) fn filmstrip(path: &Path, count: usize) -> Vec<Arc<gpui::RenderImage>> {
    #[cfg(target_os = "macos")]
    let frames = macos::filmstrip(path, count);
    #[cfg(not(target_os = "macos"))]
    let frames: Vec<image::RgbaImage> = {
        let _ = (path, count);
        Vec::new()
    };
    frames
        .into_iter()
        .map(|pixels| Arc::new(gpui::RenderImage::new([image::Frame::new(pixels)])))
        .collect()
}

/// The treatment a frame is painted under.
#[derive(Clone, Copy, PartialEq)]
struct Treatment {
    effect: crate::settings::NewThreadBackgroundEffect,
    light: bool,
    blur: f32,
}

/// A decoded frame in two forms: the untouched source (so a treatment change
/// can re-render a held frame without decoding) and the painted image.
struct Slot {
    raw: Arc<image::RgbaImage>,
    painted: Arc<gpui::RenderImage>,
    treatment: Treatment,
}

struct Shared {
    latest: Mutex<Option<Slot>>,
    /// Frames replaced since the last `release_retired`.
    retired: Mutex<Vec<Arc<gpui::RenderImage>>>,
    /// Treatment requested by the UI thread; applied to every new frame.
    treatment: Mutex<Treatment>,
    /// Playback speed (f32 bits), refreshed from settings.
    speed: AtomicU32,
    /// Loop segment start in source seconds (f32 bits), refreshed from settings.
    start: AtomicU32,
    /// Loop segment end in source seconds (f32 bits); 0 plays to the end.
    end: AtomicU32,
    /// A held frame is already being re-rendered for a new treatment.
    reprocessing: AtomicBool,
    generation: AtomicU64,
    stop: AtomicBool,
    /// Milliseconds since `epoch` of the latest frame request.
    demanded_at: AtomicU64,
    epoch: Instant,
}

impl Shared {
    fn new(treatment: Treatment) -> Self {
        Self {
            latest: Mutex::new(None),
            retired: Mutex::new(Vec::new()),
            treatment: Mutex::new(treatment),
            speed: AtomicU32::new(crate::settings::VIDEO_SPEED_DEFAULT.to_bits()),
            start: AtomicU32::new(0.0f32.to_bits()),
            end: AtomicU32::new(0.0f32.to_bits()),
            reprocessing: AtomicBool::new(false),
            generation: AtomicU64::new(0),
            stop: AtomicBool::new(false),
            demanded_at: AtomicU64::new(0),
            epoch: Instant::now(),
        }
    }

    fn demand(&self) {
        self.demanded_at
            .store(self.epoch.elapsed().as_millis() as u64, Ordering::Relaxed);
    }

    fn demanded_recently(&self) -> bool {
        let last = Duration::from_millis(self.demanded_at.load(Ordering::Relaxed));
        self.epoch.elapsed().saturating_sub(last) < DEMAND_TIMEOUT
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

    /// Publish a freshly decoded BGRA frame under the current treatment.
    fn publish(&self, frame: image::RgbaImage) {
        self.publish_with(frame, self.treatment());
    }

    fn publish_with(&self, frame: image::RgbaImage, treatment: Treatment) {
        let raw = Arc::new(frame);
        let slot = Slot {
            painted: paint(&raw, treatment),
            raw,
            treatment,
        };
        let previous = self.latest.lock().unwrap().replace(slot);
        if let Some(previous) = previous {
            self.retired.lock().unwrap().push(previous.painted);
        }
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
        ),
    )]))
}

struct Player {
    path: PathBuf,
    shared: Arc<Shared>,
}

impl Drop for Player {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
    }
}

#[derive(Default)]
struct Registry {
    player: Option<Player>,
    /// Frames of a stopped player still awaiting atlas release.
    orphaned: Vec<Arc<gpui::RenderImage>>,
    refresh_task_running: bool,
}

fn registry() -> &'static Mutex<Registry> {
    static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
    REGISTRY.get_or_init(Default::default)
}

/// The frame to paint for the video at `path`, starting playback on first
/// use. `None` until the first frame has decoded. With `animate` false the
/// current frame is held still (reduced motion / unfocused window).
pub(crate) fn current_frame(
    path: &Path,
    animate: bool,
    effect: crate::settings::NewThreadBackgroundEffect,
    light: bool,
    blur: f32,
    cx: &mut gpui::App,
) -> Option<Arc<gpui::RenderImage>> {
    let treatment = Treatment {
        effect,
        light,
        blur,
    };
    let (speed, start, end) = crate::settings::new_thread_video_playback(cx);
    let mut registry = registry().lock().unwrap();
    if registry
        .player
        .as_ref()
        .is_none_or(|player| player.path != path)
    {
        if let Some(old) = registry.player.take() {
            orphan(&mut registry, &old);
        }
        let shared = Arc::new(Shared::new(treatment));
        shared.demand();
        spawn_decoder(path.to_path_buf(), shared.clone());
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
    // A held frame keeps its old treatment until re-rendered in place.
    let reprocess = stale && !shared.reprocessing.swap(true, Ordering::Relaxed);
    let mut refresh = false;
    if animate && !registry.refresh_task_running {
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
    let raw = shared
        .latest
        .lock()
        .unwrap()
        .as_ref()
        .map(|slot| slot.raw.clone());
    let Some(raw) = raw else {
        shared.reprocessing.store(false, Ordering::Relaxed);
        return;
    };
    cx.spawn(async move |cx| {
        let source = raw.clone();
        let painted = cx
            .background_executor()
            .spawn(async move { paint(&source, treatment) })
            .await;
        cx.update(|cx| {
            {
                let mut latest = shared.latest.lock().unwrap();
                if let Some(slot) = latest.as_mut()
                    && Arc::ptr_eq(&slot.raw, &raw)
                    && slot.treatment != treatment
                {
                    slot.treatment = treatment;
                    let previous = std::mem::replace(&mut slot.painted, painted);
                    shared.retired.lock().unwrap().push(previous);
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
}

fn orphan(registry: &mut Registry, player: &Player) {
    player.shared.stop.store(true, Ordering::Relaxed);
    let retired = std::mem::take(&mut *player.shared.retired.lock().unwrap());
    registry.orphaned.extend(retired);
    if let Some(latest) = player.shared.latest.lock().unwrap().take() {
        registry.orphaned.push(latest.painted);
    }
}

/// Free atlas entries of frames that are no longer painted. Call from the
/// window that paints the video, before painting the next frame.
pub(crate) fn release_retired(window: &mut gpui::Window) {
    let retired = {
        let mut registry = registry().lock().unwrap();
        let mut retired = std::mem::take(&mut registry.orphaned);
        if let Some(player) = registry.player.as_ref() {
            retired.extend(std::mem::take(&mut *player.shared.retired.lock().unwrap()));
        }
        retired
    };
    for frame in retired {
        let _ = window.drop_image(frame);
    }
}

/// Repaint windows while a video is playing, at the video cadence rather than
/// the display refresh rate. Exits once playback stops or loses demand.
fn spawn_refresh_task(cx: &mut gpui::App) {
    cx.spawn(async move |cx| {
        let mut seen = u64::MAX;
        loop {
            cx.background_executor().timer(REFRESH_INTERVAL).await;
            let state = {
                let registry = registry().lock().unwrap();
                registry.player.as_ref().map(|player| {
                    (
                        player.shared.generation.load(Ordering::Acquire),
                        player.shared.demanded_recently(),
                    )
                })
            };
            match state {
                Some((generation, true)) => {
                    if generation != seen {
                        seen = generation;
                        cx.update(|cx| cx.refresh_windows());
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
        let mut decoder = macos::Decoder::open(path)?;
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

fn spawn_decoder(path: PathBuf, shared: Arc<Shared>) {
    let spawned = std::thread::Builder::new()
        .name("zeron-video-wallpaper".into())
        .spawn(move || {
            #[cfg(target_os = "macos")]
            macos::run(&path, &shared);
            #[cfg(not(target_os = "macos"))]
            let _ = (&path, &shared);
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
        CGBitmapContextCreate, CGColorSpaceCreateDeviceRGB, CGContextDrawImage, CGContextScaleCTM,
        CGContextTranslateCTM, CGImage, CGImageAlphaInfo, CGImageByteOrderInfo, CGImageGetHeight,
        CGImageGetWidth,
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
        pub time: f64,
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
    fn rasterize(image: &CGImage) -> Option<image::RgbaImage> {
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
        // CG draws bottom-up; flip so memory row 0 is the frame's top.
        CGContextTranslateCTM(Some(&context), 0.0, height as f64);
        CGContextScaleCTM(Some(&context), 1.0, -1.0);
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
    }

    // AVFoundation objects are used from the one decoder thread that owns them.
    unsafe impl Send for Decoder {}

    impl Decoder {
        pub fn open(path: &Path) -> Result<Self, String> {
            let url = NSURL::from_file_path(path)
                .ok_or_else(|| "The video path is not valid.".to_string())?;
            let asset: Retained<AVAsset> =
                unsafe { Retained::into_super(AVURLAsset::assetWithURL(&url)) };
            Self::start(asset)
        }

        fn start(asset: Retained<AVAsset>) -> Result<Self, String> {
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
            // Wide (4K+) sources: let VideoToolbox scale while decoding instead
            // of box-downscaling every frame on the CPU.
            #[allow(deprecated)]
            let natural = unsafe { track.naturalSize() };
            if natural.width > MAX_FRAME_WIDTH as f64 && natural.height > 0.0 {
                let width = MAX_FRAME_WIDTH as f64;
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
            })
        }

        /// Restart from the first frame (looping).
        pub fn rewind(self) -> Result<Self, String> {
            unsafe { self.reader.cancelReading() };
            Self::start(self.asset.clone())
        }

        pub fn completed(&self) -> bool {
            (unsafe { self.reader.status() }) == AVAssetReaderStatus::Completed
        }

        /// The next decoded frame, or `None` at the end of the file / on error.
        pub fn next_frame(&mut self) -> Option<Frame> {
            loop {
                let next = autoreleasepool(|_| {
                    let sample = unsafe { self.output.copyNextSampleBuffer() }?;
                    let time = unsafe { sample.presentation_time_stamp().seconds() };
                    let Some(buffer) = (unsafe { sample.image_buffer() }) else {
                        // Marker-only sample: keep reading.
                        return Some(None);
                    };
                    Some(copy_pixels(&buffer).map(|pixels| Frame {
                        pixels,
                        time: if time.is_finite() { time } else { 0.0 },
                    }))
                })?;
                if let Some(frame) = next {
                    return Some(frame);
                }
            }
        }
    }

    /// Copy a locked BGRA pixel buffer into an owned image, box-downscaling
    /// wide frames by an integer factor.
    fn copy_pixels(buffer: &objc2_core_video::CVPixelBuffer) -> Option<image::RgbaImage> {
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
                downscale(source, width, height, stride)
            });
        unsafe { CVPixelBufferUnlockBaseAddress(buffer, flags) };
        result
    }

    fn downscale(source: &[u8], width: usize, height: usize, stride: usize) -> image::RgbaImage {
        let factor = width.div_ceil(MAX_FRAME_WIDTH).max(1);
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
    /// at the end, idle without demand, exit on stop or a broken file.
    pub(super) fn run(path: &Path, shared: &Shared) {
        let mut decoder = match Decoder::open(path) {
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
        while !shared.stop.load(Ordering::Relaxed) {
            if !shared.demanded_recently() && shared.latest.lock().unwrap().is_some() {
                paused = true;
                std::thread::sleep(Duration::from_millis(100));
                continue;
            }
            if paused {
                // Resume where playback stopped rather than fast-forwarding.
                paused = false;
                let segment = (last_time - shared.start()).max(0.0);
                loop_start =
                    Instant::now() - Duration::from_secs_f64(segment / shared.speed() as f64);
            }
            let Some(frame) = decoder.next_frame() else {
                if !decoder.completed() || frames_this_loop == 0 {
                    // Read error, or a file without frames: never spin.
                    return;
                }
                decoder = match decoder.rewind() {
                    Ok(decoder) => decoder,
                    Err(_) => return,
                };
                loop_start = Instant::now();
                frames_this_loop = 0;
                continue;
            };
            // The trim bar sets the loop segment: skip its head, restart at its tail.
            if frame.time < shared.start() {
                continue;
            }
            let end = shared.end();
            if end > shared.start() && frame.time >= end {
                if frames_this_loop == 0 {
                    // The segment holds no frames; never spin on a rewind loop.
                    return;
                }
                decoder = match decoder.rewind() {
                    Ok(decoder) => decoder,
                    Err(_) => return,
                };
                loop_start = Instant::now();
                frames_this_loop = 0;
                last_time = shared.start();
                continue;
            }
            frames_this_loop += 1;
            last_time = frame.time;
            let segment = (frame.time - shared.start()).max(0.0);
            let due = loop_start + Duration::from_secs_f64(segment / shared.speed() as f64);
            // The UI repaints at REFRESH_INTERVAL: a frame due sooner than that after
            // the last published one would never be seen, so skip its effect work.
            if last_due.is_some_and(|last| {
                due.checked_duration_since(last)
                    .is_some_and(|gap| gap + Duration::from_millis(4) < REFRESH_INTERVAL)
            }) && shared.latest.lock().unwrap().is_some()
            {
                continue;
            }
            last_due = Some(due);
            let now = Instant::now();
            if due > now {
                std::thread::sleep(due - now);
            } else if now - due > Duration::from_millis(100)
                && shared.latest.lock().unwrap().is_some()
            {
                // Running behind (e.g. after a stall): drop this frame.
                continue;
            }
            shared.publish(frame.pixels);
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
        let frame = frame_at(path, 0.5).expect("frame near 0.5 s");
        assert!(frame.size(0).width.0 > 0);
        // The cache answers a repeated request with the very same frame.
        assert!(Arc::ptr_eq(
            &frame,
            &frame_at(path, 0.5).expect("cached frame")
        ));
    }

    #[test]
    fn filmstrip_spans_the_video() {
        let path = Path::new(SAMPLE);
        if !path.exists() {
            return;
        }
        let frames = filmstrip(path, 4);
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

        let mut decoder = macos::Decoder::open(path).unwrap();
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

    #[test]
    fn rejects_non_video_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fake.mp4");
        std::fs::write(&path, b"definitely not a movie").unwrap();
        assert!(first_frame(&path).is_err());
    }
}
