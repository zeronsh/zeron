//! Image previews inside expanded tool chips: files a tool read, wrote or
//! printed. Pixels exist only while their chip is expanded. Opening reads the
//! file from the agent's device and decodes a bounded BGRA thumbnail off the
//! UI thread (handed straight to GPUI — no PNG re-encode, no asset cache).
//! Collapsing starts a short grace; when it lapses the CPU frame and the GPU
//! atlas tiles are released. Rapid toggling inside the grace reuses the
//! texture instead of re-reading and re-decoding the file.

use std::{
    collections::HashMap,
    io::Cursor,
    sync::Arc,
    time::{Duration, Instant},
};

use gpui::{Context, RenderImage, SharedString, Task};
use zeron_proto::ToolCall;

use crate::state::EngineHandle;

/// Previews per chip; further paths stay in the text output.
pub(crate) const MAX_IMAGES_PER_TOOL: usize = 4;
/// Decoded thumbnail bounds: the 560×220 frame at 2× density (≤ 1.9 MiB
/// BGRA per copy).
const THUMB_MAX: (u32, u32) = (1120, 440);
/// Collapsed chips keep pixels this long, so open/close spam never re-decodes.
pub(crate) const RELEASE_GRACE: Duration = Duration::from_millis(600);
/// Expanded-but-offscreen previews may be dropped past this (CPU + GPU bytes).
const BUDGET_BYTES: usize = 64 * 1024 * 1024;
/// Only previews unpainted this long count as offscreen. List overdraw paints
/// rows beyond the viewport, so a frame-based rule evicted visible previews
/// and re-decoded them in a loop once the painted set outgrew the budget.
const STALE_AFTER: Duration = Duration::from_secs(2);

const IMAGE_EXTENSIONS: [&str; 9] = [
    "png", "jpg", "jpeg", "gif", "webp", "bmp", "svg", "tif", "tiff",
];

pub(crate) fn is_image_path(path: &str) -> bool {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    name.rsplit_once('.').is_some_and(|(stem, ext)| {
        !stem.is_empty() && IMAGE_EXTENSIONS.iter().any(|e| ext.eq_ignore_ascii_case(e))
    })
}

/// UNC (`\\host\share`, `//host/share`), device and verbatim (`\\.\`,
/// `\\?\`, `\??\`) and drive-relative (`C:x.png`) forms. Agent output names
/// them freely, and merely opening one can reach another host (on Windows
/// with the user's NTLM credentials), so they are never previewed. The
/// engine's jail refuses them as well.
fn is_remote_or_device_path(path: &str) -> bool {
    let bytes = path.as_bytes();
    let separator = |b: Option<&u8>| matches!(b, Some(b'/' | b'\\'));
    (separator(bytes.first()) && separator(bytes.get(1)))
        || path.starts_with(r"\??\")
        || (bytes.len() >= 2
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && !separator(bytes.get(2)))
}

fn push_path(out: &mut Vec<SharedString>, path: &str) {
    if out.len() < MAX_IMAGES_PER_TOOL
        && is_image_path(path)
        && !is_remote_or_device_path(path)
        && !out.iter().any(|known| known.as_ref() == path)
    {
        out.push(SharedString::from(path.to_owned()));
    }
}

/// Image-looking local paths in free text (commands, output, JSON strings).
/// URLs and glob patterns are skipped; trailing sentence punctuation is not
/// part of a path.
fn push_text_paths(out: &mut Vec<SharedString>, text: &str) {
    let separators = |c: char| {
        c.is_whitespace()
            || matches!(
                c,
                '"' | '\''
                    | '`'
                    | '<'
                    | '>'
                    | '('
                    | ')'
                    | '['
                    | ']'
                    | '{'
                    | '}'
                    | ','
                    | ';'
                    | '|'
                    | '='
            )
    };
    for token in text.split(separators) {
        if out.len() >= MAX_IMAGES_PER_TOOL {
            return;
        }
        let token = token.trim_end_matches(['.', ':', '!', '?']);
        let token = token.strip_prefix("file://").unwrap_or(token);
        if token.contains("://") || token.starts_with("data:") || token.contains(['*', '?']) {
            continue;
        }
        push_path(out, token);
    }
}

fn push_json_paths(out: &mut Vec<SharedString>, value: &serde_json::Value) {
    match value {
        serde_json::Value::String(text) => push_text_paths(out, text),
        serde_json::Value::Array(items) => items.iter().for_each(|v| push_json_paths(out, v)),
        serde_json::Value::Object(map) => map.values().for_each(|v| push_json_paths(out, v)),
        _ => {}
    }
}

/// The image files a tool call touched, as reported (possibly relative to the
/// chat's working directory). Computed once per row build, never per paint.
pub(crate) fn image_paths(call: &ToolCall, output: Option<&str>) -> Arc<[SharedString]> {
    let mut out = Vec::new();
    match call {
        ToolCall::ReadFile { path }
        | ToolCall::WriteFile { path, .. }
        | ToolCall::EditFile { path, .. }
        | ToolCall::ApplyPatch { path: Some(path) } => push_path(&mut out, path),
        ToolCall::Exec { command } => push_text_paths(&mut out, command),
        ToolCall::Mcp {
            input: Some(input), ..
        }
        | ToolCall::Unknown {
            input: Some(input), ..
        } => push_json_paths(&mut out, input),
        _ => {}
    }
    if let Some(output) = output {
        push_text_paths(&mut out, output);
    }
    out.into()
}

/// Absolute path on the owning host. Relative paths join the chat's cwd in
/// that host's separator style (the UI may run on another OS).
pub(crate) fn resolve_path(path: &str, cwd: Option<&str>) -> String {
    let bytes = path.as_bytes();
    let absolute = path.starts_with(['/', '\\'])
        || (bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && matches!(bytes[2], b'/' | b'\\'));
    match cwd.filter(|_| !absolute) {
        Some(cwd) if cwd.contains('\\') => format!(
            "{}\\{}",
            cwd.trim_end_matches(['/', '\\']),
            path.trim_start_matches("./").replace('/', "\\")
        ),
        Some(cwd) => format!(
            "{}/{}",
            cwd.trim_end_matches('/'),
            path.trim_start_matches("./")
        ),
        None => path.to_owned(),
    }
}

/// One file on one device.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ImageKey {
    pub device: String,
    pub path: String,
}

/// What a frame paints for one key.
#[derive(Clone)]
pub(crate) enum Thumb {
    Loading,
    Ready {
        image: Arc<RenderImage>,
        aspect: f32,
    },
    Failed,
}

enum State {
    /// The read/decode task; dropping it cancels the load.
    Loading(#[allow(dead_code)] Task<()>),
    Ready {
        image: Arc<RenderImage>,
        aspect: f32,
        bytes: usize,
        painted: Instant,
    },
    Failed,
}

struct Entry {
    state: State,
    /// Set while no expanded chip holds the key.
    release_at: Option<Instant>,
}

/// Where loads go: the engine and this device's id (remote owners relay).
pub(crate) struct Loader {
    pub engine: Option<EngineHandle>,
    pub local_device: Option<String>,
}

#[derive(Default)]
pub(crate) struct ToolImages {
    entries: HashMap<ImageKey, Entry>,
    /// Expanded chip (`{row}#d{ix}`) → the keys it shows.
    holders: HashMap<SharedString, Vec<ImageKey>>,
    bytes: usize,
    sweeper: Option<Task<()>>,
}

impl ToolImages {
    pub(crate) fn is_idle(&self) -> bool {
        self.entries.is_empty()
    }

    #[cfg(test)]
    pub(crate) fn retained_bytes(&self) -> usize {
        self.bytes
    }

    #[cfg(feature = "appshots-fixture")]
    pub(crate) fn stats(&self) -> (usize, usize) {
        (self.entries.len(), self.bytes)
    }

    /// An expanded chip shows `keys`: cancel their release and start any
    /// missing loads. Returns one thumb per key.
    pub(crate) fn hold<V: 'static>(
        &mut self,
        chip: &SharedString,
        keys: Vec<ImageKey>,
        loader: &Loader,
        get: fn(&mut V) -> &mut ToolImages,
        cx: &mut Context<V>,
    ) -> Vec<Thumb> {
        let now = cx.background_executor().now();
        let thumbs = keys
            .iter()
            .map(|key| {
                if !self.entries.contains_key(key) {
                    self.start_load(key.clone(), loader, get, cx);
                }
                let entry = self.entries.get_mut(key).expect("inserted above");
                entry.release_at = None;
                match &mut entry.state {
                    State::Loading(_) => Thumb::Loading,
                    State::Ready {
                        image,
                        aspect,
                        painted,
                        ..
                    } => {
                        *painted = now;
                        Thumb::Ready {
                            image: image.clone(),
                            aspect: *aspect,
                        }
                    }
                    State::Failed => Thumb::Failed,
                }
            })
            .collect();
        if self.holders.get(chip) != Some(&keys) {
            // Keys that dropped out of the chip (its output changed) start the
            // grace like a collapse; ones it still shows stay held.
            if let Some(previous) = self.holders.insert(chip.clone(), keys) {
                self.start_grace(previous, get, cx);
            }
        }
        thumbs
    }

    /// The chip collapsed (or left the transcript): its keys start the grace.
    pub(crate) fn release<V: 'static>(
        &mut self,
        chip: &SharedString,
        get: fn(&mut V) -> &mut ToolImages,
        cx: &mut Context<V>,
    ) {
        let Some(keys) = self.holders.remove(chip) else {
            return;
        };
        self.start_grace(keys, get, cx);
    }

    /// Keys no expanded chip holds any more start the release grace.
    fn start_grace<V: 'static>(
        &mut self,
        keys: Vec<ImageKey>,
        get: fn(&mut V) -> &mut ToolImages,
        cx: &mut Context<V>,
    ) {
        let at = cx.background_executor().now() + RELEASE_GRACE;
        for key in keys {
            if self.holders.values().any(|held| held.contains(&key)) {
                continue;
            }
            if let Some(entry) = self.entries.get_mut(&key) {
                entry.release_at = Some(at);
            }
        }
        self.schedule_sweep(get, cx);
    }

    /// Release every chip of a row whose body is hidden.
    pub(crate) fn release_row<V: 'static>(
        &mut self,
        row: &str,
        get: fn(&mut V) -> &mut ToolImages,
        cx: &mut Context<V>,
    ) {
        if self.holders.is_empty() {
            return;
        }
        let chips: Vec<SharedString> = self
            .holders
            .keys()
            .filter(|chip| chip_row(chip) == row)
            .cloned()
            .collect();
        for chip in chips {
            self.release(&chip, get, cx);
        }
    }

    /// Release chips whose row left the transcript (compact folds, resets).
    pub(crate) fn retain_rows<V: 'static>(
        &mut self,
        live: impl Fn(&str) -> bool,
        get: fn(&mut V) -> &mut ToolImages,
        cx: &mut Context<V>,
    ) {
        if self.holders.is_empty() {
            return;
        }
        let gone: Vec<SharedString> = self
            .holders
            .keys()
            .filter(|chip| !live(chip_row(chip)))
            .cloned()
            .collect();
        for chip in gone {
            self.release(&chip, get, cx);
        }
    }

    /// Drop everything now (chat switch).
    pub(crate) fn clear(&mut self, cx: &mut gpui::App) {
        self.holders.clear();
        self.sweeper = None;
        let released: Vec<_> = self.entries.drain().map(|(_, e)| e.state).collect();
        self.bytes = 0;
        free(released, cx);
    }

    fn start_load<V: 'static>(
        &mut self,
        key: ImageKey,
        loader: &Loader,
        get: fn(&mut V) -> &mut ToolImages,
        cx: &mut Context<V>,
    ) {
        let Some(engine) = loader.engine.clone() else {
            self.entries.insert(
                key,
                Entry {
                    state: State::Failed,
                    release_at: None,
                },
            );
            return;
        };
        let target = (loader.local_device.as_deref() != Some(key.device.as_str()))
            .then(|| key.device.clone());
        let task_key = key.clone();
        let task = cx.spawn(async move |view, cx| {
            let executor = cx.background_executor().clone();
            let raw = crate::attachments::read_attachment_bytes(
                &engine,
                &executor,
                target.as_deref(),
                &task_key.path,
                None,
            )
            .await;
            let decoded = match raw {
                Some((_, mime, bytes)) => {
                    executor
                        .spawn(async move { decode_thumbnail(&mime, bytes) })
                        .await
                }
                None => None,
            };
            view.update(cx, |view, cx| {
                get(view).finish(task_key, decoded, cx);
                cx.notify();
            })
            .ok();
        });
        self.entries.insert(
            key,
            Entry {
                state: State::Loading(task),
                release_at: None,
            },
        );
    }

    fn finish(
        &mut self,
        key: ImageKey,
        decoded: Option<(Arc<RenderImage>, f32)>,
        cx: &mut gpui::App,
    ) {
        // Released while loading: the result is simply dropped (never uploaded).
        let Some(entry) = self.entries.get_mut(&key) else {
            return;
        };
        if !matches!(entry.state, State::Loading(_)) {
            return;
        }
        entry.state = match decoded {
            Some((image, aspect)) => {
                let bytes = retained_bytes(&image);
                self.bytes += bytes;
                State::Ready {
                    image,
                    aspect,
                    bytes,
                    painted: cx.background_executor().now(),
                }
            }
            None => State::Failed,
        };
        tracing::debug!(path = %key.path, retained = self.bytes, "tool image decoded");
        self.enforce_budget(&key, cx);
    }

    /// Over budget: drop the least recently painted offscreen previews. The
    /// painted set itself may exceed the budget; it is never evicted. A held
    /// preview reloads when it is painted again.
    fn enforce_budget(&mut self, keep: &ImageKey, cx: &mut gpui::App) {
        let mut released = Vec::new();
        let Some(stale) = cx.background_executor().now().checked_sub(STALE_AFTER) else {
            return;
        };
        while self.bytes > BUDGET_BYTES {
            let oldest = self
                .entries
                .iter()
                .filter_map(|(key, entry)| match entry.state {
                    State::Ready { painted, .. } if key != keep && painted < stale => {
                        Some((painted, key.clone()))
                    }
                    _ => None,
                })
                .min_by_key(|(painted, _)| *painted);
            let Some((_, key)) = oldest else { break };
            if let Some(entry) = self.entries.remove(&key) {
                self.bytes -= state_bytes(&entry.state);
                released.push(entry.state);
            }
        }
        free(released, cx);
    }

    fn sweep(&mut self, cx: &mut gpui::App) {
        let now = cx.background_executor().now();
        let expired: Vec<ImageKey> = self
            .entries
            .iter()
            .filter(|(_, entry)| entry.release_at.is_some_and(|at| at <= now))
            .map(|(key, _)| key.clone())
            .collect();
        let mut released = Vec::with_capacity(expired.len());
        for key in expired {
            if let Some(entry) = self.entries.remove(&key) {
                self.bytes -= state_bytes(&entry.state);
                released.push(entry.state);
            }
        }
        if !released.is_empty() {
            tracing::debug!(
                released = released.len(),
                retained = self.bytes,
                "tool images released"
            );
        }
        free(released, cx);
    }

    /// One timer for the earliest pending release. Grace is constant, so a
    /// later release never precedes the one already scheduled.
    fn schedule_sweep<V: 'static>(
        &mut self,
        get: fn(&mut V) -> &mut ToolImages,
        cx: &mut Context<V>,
    ) {
        if self.sweeper.is_some() {
            return;
        }
        let Some(at) = self.entries.values().filter_map(|e| e.release_at).min() else {
            return;
        };
        let delay = at.saturating_duration_since(cx.background_executor().now());
        self.sweeper = Some(cx.spawn(async move |view, cx| {
            cx.background_executor().timer(delay).await;
            view.update(cx, |view, cx| {
                let images = get(view);
                images.sweeper = None;
                images.sweep(cx);
                images.schedule_sweep(get, cx);
            })
            .ok();
        }));
    }
}

fn chip_row(chip: &str) -> &str {
    chip.rsplit_once("#d").map_or(chip, |(row, _)| row)
}

fn retained_bytes(image: &RenderImage) -> usize {
    // GPUI keeps the CPU frame for atlas re-uploads plus the GPU copy.
    image.as_bytes(0).map_or(0, |b| b.len() * 2)
}

fn state_bytes(state: &State) -> usize {
    match state {
        State::Ready { bytes, .. } => *bytes,
        _ => 0,
    }
}

/// Free released pixels: dropping a Loading task cancels its read; a Ready
/// image leaves every window's sprite atlas once the current update ends.
fn free(states: Vec<State>, cx: &mut gpui::App) {
    let images: Vec<Arc<RenderImage>> = states
        .into_iter()
        .filter_map(|state| match state {
            State::Ready { image, .. } => Some(image),
            _ => None,
        })
        .collect();
    if !images.is_empty() {
        cx.defer(move |cx| {
            for image in images {
                cx.drop_image(image, None);
            }
        });
    }
}

/// Decode one static frame and downsample to [`THUMB_MAX`], as BGRA.
fn decode_thumbnail(mime: &str, bytes: Vec<u8>) -> Option<(Arc<RenderImage>, f32)> {
    if mime == "image/svg+xml" {
        let media = crate::image_media::decode_image(mime, bytes).ok()?;
        let image = media
            .image
            .to_image_data(gpui::SvgRenderer::new(Arc::new(crate::icons::Assets)))
            .ok()?;
        return Some((image, media.width / media.height.max(1.0)));
    }
    let mut reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    // Shared preview bounds: several chips may decode at once.
    reader.limits(crate::image_media::raster_limits());
    let decoded = reader.decode().ok()?;
    let (width, height) = (decoded.width(), decoded.height());
    if width == 0 || height == 0 {
        return None;
    }
    let decoded = if width > THUMB_MAX.0 || height > THUMB_MAX.1 {
        decoded.thumbnail(THUMB_MAX.0, THUMB_MAX.1)
    } else {
        decoded
    };
    let mut pixels = decoded.into_rgba8();
    for pixel in pixels.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    Some((
        Arc::new(RenderImage::new(vec![image::Frame::new(pixels)])),
        width as f32 / height as f32,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::AppContext as _;

    fn paths(call: ToolCall, output: Option<&str>) -> Vec<String> {
        image_paths(&call, output)
            .iter()
            .map(|p| p.to_string())
            .collect()
    }

    #[test]
    fn finds_images_in_paths_commands_output_and_json() {
        assert_eq!(
            paths(
                ToolCall::ReadFile {
                    path: "shots/A.PNG".into()
                },
                None
            ),
            ["shots/A.PNG"]
        );
        assert!(
            paths(
                ToolCall::ReadFile {
                    path: "src/main.rs".into()
                },
                None
            )
            .is_empty()
        );
        assert_eq!(
            paths(
                ToolCall::Exec {
                    command: "python plot.py --out=chart.png".into()
                },
                Some("Saved to /tmp/x/chart.png.\nsee https://a.b/c.png and **/*.jpg")
            ),
            ["chart.png", "/tmp/x/chart.png"]
        );
        assert_eq!(
            paths(
                ToolCall::Mcp {
                    server: "browser".into(),
                    tool: "screenshot".into(),
                    input: Some(serde_json::json!({"opts": {"file": r"C:\shots\home.webp"}}))
                },
                None
            ),
            [r"C:\shots\home.webp"]
        );
        let many = (0..9).map(|i| format!("{i}.jpg ")).collect::<String>();
        assert_eq!(
            paths(ToolCall::Exec { command: many }, None).len(),
            MAX_IMAGES_PER_TOOL
        );
    }

    #[test]
    fn relative_paths_join_the_owning_hosts_cwd() {
        assert_eq!(resolve_path("a/b.png", Some("/w")), "/w/a/b.png");
        assert_eq!(resolve_path("./b.png", Some("/w/")), "/w/b.png");
        assert_eq!(resolve_path("a/b.png", Some(r"C:\w")), r"C:\w\a\b.png");
        assert_eq!(resolve_path(r"D:\x.png", Some(r"C:\w")), r"D:\x.png");
        assert_eq!(resolve_path("/x.png", Some("/w")), "/x.png");
        assert_eq!(resolve_path("x.png", None), "x.png");
        assert_eq!(resolve_path("C:x.png", Some("/w")), "/w/C:x.png");
    }

    #[test]
    fn network_device_and_drive_relative_paths_are_never_previewed() {
        let output = [
            r"\\attacker\share\a.png",
            "//attacker/share/b.png",
            r"\/attacker\share\c.png",
            "file:////attacker/share/d.png",
            r"\\?\UNC\attacker\share\e.png",
            r"\\?\C:\shots\f.png",
            r"\\.\pipe\g.png",
            r"\??\UNC\attacker\share\h.png",
            "C:i.png",
        ]
        .join("\n");
        let found = paths(
            ToolCall::Exec {
                command: "open ok.png".into(),
            },
            Some(&output),
        );
        assert_eq!(found, ["ok.png"]);
        assert!(
            paths(
                ToolCall::ReadFile {
                    path: r"\\attacker\share\a.png".into()
                },
                None
            )
            .is_empty()
        );
        // Ordinary absolute paths in both styles still preview.
        assert_eq!(
            paths(
                ToolCall::Exec {
                    command: r"cp /tmp/a.png C:\shots\b.png D:/c.png".into()
                },
                None
            ),
            ["/tmp/a.png", r"C:\shots\b.png", "D:/c.png"]
        );
    }

    #[test]
    fn thumbnails_are_bounded_bgra() {
        let mut png = Cursor::new(Vec::new());
        image::RgbaImage::from_pixel(4000, 1000, image::Rgba([255, 0, 0, 255]))
            .write_to(&mut png, image::ImageFormat::Png)
            .unwrap();
        let (image, aspect) = decode_thumbnail("image/png", png.into_inner()).unwrap();
        let size = image.size(0);
        assert!(size.width.0 as u32 <= THUMB_MAX.0 && size.height.0 as u32 <= THUMB_MAX.1);
        assert_eq!(aspect, 4.0);
        // Red in RGBA is [0, 0, 255, 255] in BGRA.
        assert_eq!(&image.as_bytes(0).unwrap()[..4], &[0, 0, 255, 255]);
        assert!(decode_thumbnail("image/png", b"nope".to_vec()).is_none());
        // Past the shared decoder bounds: refused, never fully allocated.
        let mut wide = Cursor::new(Vec::new());
        image::GrayImage::new(4097, 1)
            .write_to(&mut wide, image::ImageFormat::Png)
            .unwrap();
        assert!(decode_thumbnail("image/png", wide.into_inner()).is_none());
    }

    struct Host {
        images: ToolImages,
    }

    fn get(host: &mut Host) -> &mut ToolImages {
        &mut host.images
    }

    #[gpui::test]
    fn collapse_spam_keeps_pixels_and_settled_collapse_frees_them(cx: &mut gpui::TestAppContext) {
        let host = cx.new(|_| Host {
            images: ToolImages::default(),
        });
        let key = ImageKey {
            device: "d".into(),
            path: "/w/a.png".into(),
        };
        let chip = SharedString::from("row#d0");
        let loader = Loader {
            engine: None,
            local_device: None,
        };
        host.update(cx, |host, cx| {
            let mut pixels = image::RgbaImage::new(100, 50);
            pixels.fill(1);
            host.images.entries.insert(
                key.clone(),
                Entry {
                    state: State::Loading(Task::ready(())),
                    release_at: None,
                },
            );
            host.images.finish(
                key.clone(),
                Some((
                    Arc::new(RenderImage::new(vec![image::Frame::new(pixels)])),
                    2.0,
                )),
                cx,
            );
            assert_eq!(host.images.retained_bytes(), 100 * 50 * 4 * 2);
            // Spam: every reopen inside the grace reuses the same texture.
            for _ in 0..50 {
                let thumbs = host.images.hold(&chip, vec![key.clone()], &loader, get, cx);
                assert!(matches!(thumbs[0], Thumb::Ready { aspect: 2.0, .. }));
                host.images.release(&chip, get, cx);
            }
            assert!(!host.images.is_idle());
        });
        cx.executor().advance_clock(RELEASE_GRACE / 2);
        cx.run_until_parked();
        host.update(cx, |host, _| assert!(!host.images.is_idle()));
        cx.executor().advance_clock(RELEASE_GRACE);
        cx.run_until_parked();
        host.update(cx, |host, _| {
            assert!(host.images.is_idle(), "collapsed previews are freed");
            assert_eq!(host.images.retained_bytes(), 0);
        });
        // A failed load (no engine) is retried after collapse + grace.
        host.update(cx, |host, cx| {
            let thumbs = host.images.hold(&chip, vec![key.clone()], &loader, get, cx);
            assert!(matches!(thumbs[0], Thumb::Failed));
            host.images.release(&chip, get, cx);
        });
        cx.executor().advance_clock(RELEASE_GRACE * 2);
        cx.run_until_parked();
        host.update(cx, |host, _| assert!(host.images.is_idle()));
    }

    #[gpui::test]
    fn shared_keys_stay_while_another_chip_holds_them_and_rows_release(
        cx: &mut gpui::TestAppContext,
    ) {
        let host = cx.new(|_| Host {
            images: ToolImages::default(),
        });
        let key = ImageKey {
            device: "d".into(),
            path: "/w/a.png".into(),
        };
        let loader = Loader {
            engine: None,
            local_device: None,
        };
        host.update(cx, |host, cx| {
            host.images
                .hold(&"r1#d0".into(), vec![key.clone()], &loader, get, cx);
            host.images
                .hold(&"r2#d3".into(), vec![key.clone()], &loader, get, cx);
            host.images.release(&"r1#d0".into(), get, cx);
            assert!(host.images.entries[&key].release_at.is_none());
            host.images.retain_rows(|row| row == "r1", get, cx);
            assert!(host.images.entries[&key].release_at.is_some());
            host.images
                .hold(&"r2#d3".into(), vec![key.clone()], &loader, get, cx);
            host.images.release_row("r2", get, cx);
            assert!(host.images.holders.is_empty());
        });
        cx.executor().advance_clock(RELEASE_GRACE * 2);
        cx.run_until_parked();
        host.update(cx, |host, _| assert!(host.images.is_idle()));
    }

    #[gpui::test]
    fn keys_dropped_from_a_held_chip_are_released(cx: &mut gpui::TestAppContext) {
        let host = cx.new(|_| Host {
            images: ToolImages::default(),
        });
        let key = |path: &str| ImageKey {
            device: "d".into(),
            path: path.into(),
        };
        let (a, b) = (key("/w/a.png"), key("/w/b.png"));
        let chip = SharedString::from("row#d0");
        let loader = Loader {
            engine: None,
            local_device: None,
        };
        host.update(cx, |host, cx| {
            host.images
                .hold(&chip, vec![a.clone(), b.clone()], &loader, get, cx);
            // The chip's output changed while expanded: `a` is gone from it.
            host.images.hold(&chip, vec![b.clone()], &loader, get, cx);
            assert!(host.images.entries[&a].release_at.is_some());
            assert!(host.images.entries[&b].release_at.is_none());
        });
        cx.executor().advance_clock(RELEASE_GRACE * 2);
        cx.run_until_parked();
        host.update(cx, |host, _| {
            assert!(!host.images.entries.contains_key(&a));
            assert!(host.images.entries.contains_key(&b));
        });
    }

    #[gpui::test]
    fn budget_drops_only_previews_unpainted_for_a_while(cx: &mut gpui::TestAppContext) {
        let host = cx.new(|_| Host {
            images: ToolImages::default(),
        });
        let insert = |host: &mut Host, i: usize, cx: &mut Context<Host>| {
            let key = ImageKey {
                device: "d".into(),
                path: format!("/{i}.png"),
            };
            host.images.entries.insert(
                key.clone(),
                Entry {
                    state: State::Loading(Task::ready(())),
                    release_at: None,
                },
            );
            // 2048² BGRA = 16 MiB CPU + 16 MiB GPU per preview.
            let image =
                RenderImage::new(vec![image::Frame::new(image::RgbaImage::new(2048, 2048))]);
            host.images.finish(key, Some((Arc::new(image), 1.0)), cx);
        };
        host.update(cx, |host, cx| {
            for i in 0..3 {
                insert(host, i, cx);
            }
            // Recently painted previews survive even past the budget.
            assert_eq!(host.images.entries.len(), 3);
            assert!(host.images.retained_bytes() > BUDGET_BYTES);
        });
        cx.executor().advance_clock(STALE_AFTER * 2);
        host.update(cx, |host, cx| {
            insert(host, 3, cx);
            assert!(host.images.retained_bytes() <= BUDGET_BYTES);
            assert!(host.images.entries.contains_key(&ImageKey {
                device: "d".into(),
                path: "/3.png".into(),
            }));
        });
    }
}
