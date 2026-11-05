//! Agent visuals (`show_visual` Luau scripts) on mobile.
//!
//! Each visual in a transcript gets one worker thread that owns its Luau
//! runtime, lays its node tree out (text measured with the bundled faces),
//! draws canvases and charts as vector paint, and renders scenes and shaders
//! on the GPU. Nothing here runs on the platform's UI thread: the platform
//! receives finished display lists and frames through [`VisualListener`],
//! paints them, and forwards touches. The transcript learns each visual's
//! height from its worker, so rows are measured before they're painted.
//!
//! Semantics follow the desktop (`crates/ui/src/visuals.rs`); a touch
//! stands in for the pointer.

mod chart;
mod layout;
mod paint;
mod palette;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use zeron_text::WidthCache;
use zeron_visual::{DrawCmd, HandlerArg, HandlerId, Node, Visual};

use crate::layout::style::Typography;
use crate::layout::{StyleDesc, TextSystem};
use layout::{Action, ActionArg, Laid, SurfaceNode, SurfaceSpec};
use paint::{Face, Paint, TextCtx};
use palette::Palette;

// ---- FFI ----------------------------------------------------------------------

/// sRGB, straight alpha, 0–1.
#[derive(Debug, Clone, Copy, PartialEq, Default, uniffi::Record)]
pub struct Rgba {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

/// `text[start..start+len]` (UTF-16) in `style`, its origin at (x, baseline).
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct VisualRun {
    pub start: u32,
    pub len: u32,
    pub x: f32,
    pub baseline: f32,
    pub width: f32,
    pub style: u16,
    pub color: Rgba,
}

#[derive(Debug, Clone, PartialEq, uniffi::Enum)]
pub enum VisualShape {
    Rect {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        radius: f32,
        color: Rgba,
    },
    /// A polyline: `points` holds x, y pairs.
    Path {
        points: Vec<f32>,
        closed: bool,
        fill: Option<Rgba>,
        stroke: Option<Rgba>,
        line_width: f32,
        /// Dash and gap length.
        dash: Option<f32>,
    },
}

/// One layer of vector paint: shapes, then text over them.
#[derive(Debug, Clone, PartialEq, Default, uniffi::Record)]
pub struct VisualPaint {
    pub text: String,
    pub runs: Vec<VisualRun>,
    pub shapes: Vec<VisualShape>,
}

/// How a surface takes touches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum SurfaceGesture {
    /// Not interactive: drags scroll the transcript.
    None,
    /// Taps only.
    Tap,
    /// Takes drags in any direction (orbits, pointer-driven drawings).
    Drag,
    /// Takes mostly-horizontal drags (scrubbing a chart); vertical ones
    /// scroll.
    HorizontalDrag,
}

/// A region the worker redraws on its own (canvas, chart, scene, shader).
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct VisualSurface {
    /// Stable across renders: frames and touches address it.
    pub id: u64,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub gesture: SurfaceGesture,
}

#[derive(Debug, Clone, PartialEq, uniffi::Enum)]
pub enum ControlKind {
    Button {
        primary: bool,
    },
    Tab {
        selected: bool,
    },
    Toggle {
        on: bool,
    },
    /// The track runs from `track_x` for `track_w` points.
    Slider {
        value: f64,
        min: f64,
        max: f64,
        step: f64,
        fraction: f32,
        track_x: f32,
        track_w: f32,
        shown: String,
    },
    /// Opened by the platform.
    Link {
        url: String,
    },
    /// A tappable stack.
    Tap,
}

/// A touch target over painted control art.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct VisualControl {
    /// Stable across renders.
    pub id: u64,
    pub kind: ControlKind,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// Accessibility label.
    pub label: String,
}

/// A visual laid out at one width.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct VisualDisplay {
    pub revision: u64,
    pub width: f32,
    pub height: f32,
    pub dark: bool,
    pub paint: VisualPaint,
    pub surfaces: Vec<VisualSurface>,
    pub controls: Vec<VisualControl>,
    /// Style id → face and size, for every run so far.
    pub styles: Vec<StyleDesc>,
}

/// Premultiplied BGRA8, top row first (CoreGraphics' native
/// `byteOrder32Little | premultipliedFirst`).
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct FrameImage {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

/// A surface's latest picture: an image (scenes, shaders) under vector
/// paint (canvas drawings, charts, scene labels), in surface coordinates.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct SurfaceFrame {
    pub surface: u64,
    pub image: Option<FrameImage>,
    pub paint: VisualPaint,
    pub styles: Vec<StyleDesc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum PointerPhase {
    Down,
    Move,
    Up,
    Cancel,
}

/// Where a visual's output goes (called on its worker thread).
#[uniffi::export(with_foreign)]
pub trait VisualListener: Send + Sync {
    fn display(&self, display: VisualDisplay);
    fn frame(&self, frame: SurfaceFrame);
}

// ---- transcript side --------------------------------------------------------

/// A visual row's link to its worker: the height it measured.
pub(crate) struct VisualSlot {
    pub id: String,
    pub path: String,
    pub title: String,
    measured: Mutex<Option<(f32, f32)>>,
    epoch: AtomicU64,
}

impl VisualSlot {
    pub fn new(id: &str, path: &str, title: &str) -> Arc<Self> {
        Arc::new(Self {
            id: id.to_owned(),
            path: path.to_owned(),
            title: title.to_owned(),
            measured: Mutex::new(None),
            epoch: AtomicU64::new(0),
        })
    }

    /// The measured height (at the latest width the worker laid out).
    pub fn height(&self) -> Option<f32> {
        self.snapshot().1
    }

    /// The epoch and the height it belongs to, read together.
    pub fn snapshot(&self) -> (u64, Option<f32>) {
        let measured = self.measured.lock().unwrap();
        (self.epoch.load(Ordering::Relaxed), measured.map(|(_, h)| h))
    }

    fn set(&self, width: f32, height: f32) -> bool {
        let mut measured = self.measured.lock().unwrap();
        if measured.is_some_and(|(w, h)| w == width && (h - height).abs() < 0.01) {
            return false;
        }
        let changed = measured.is_none_or(|(_, h)| (h - height).abs() >= 0.01);
        *measured = Some((width, height));
        if changed {
            self.epoch.fetch_add(1, Ordering::Relaxed);
        }
        changed
    }
}

/// Reads a file beside the chat (the script, its manifest and assets).
type Fetch = Arc<dyn Fn(&str) -> Option<Vec<u8>> + Send + Sync>;

#[derive(Clone)]
struct Source {
    fetch: Fetch,
    pixel_scale: f32,
}

/// One transcript's visuals: their workers, by visual id.
pub(crate) struct VisualHub {
    text: Arc<TextSystem>,
    /// Set once the transcript closes: no new workers after that.
    closed: std::sync::atomic::AtomicBool,
    source: Mutex<Option<Source>>,
    sessions: Mutex<HashMap<String, Arc<VisualSession>>>,
    /// Ask the transcript to remeasure a row (its key).
    changed: Arc<dyn Fn(u64) + Send + Sync>,
}

impl VisualHub {
    pub fn new(text: Arc<TextSystem>, changed: Arc<dyn Fn(u64) + Send + Sync>) -> Self {
        Self {
            text,
            closed: std::sync::atomic::AtomicBool::new(false),
            source: Mutex::new(None),
            sessions: Mutex::new(HashMap::new()),
            changed,
        }
    }

    /// Where scripts load from: the chat's host device.
    pub fn set_source(&self, client: zeron_client::Client, device: String, pixel_scale: f32) {
        let fetch: Fetch = Arc::new(move |path: &str| {
            let (client, device, path) = (client.clone(), device.clone(), path.to_owned());
            block_on(zeron_client::runtime::run(async move {
                client.read_attachment(&device, &path).await
            }))
            .ok()
            .map(|bytes| bytes.as_ref().clone())
        });
        *self.source.lock().unwrap() = Some(Source { fetch, pixel_scale });
    }

    #[cfg(test)]
    pub(crate) fn set_fetch(&self, fetch: Fetch, pixel_scale: f32) {
        *self.source.lock().unwrap() = Some(Source { fetch, pixel_scale });
    }

    /// Start (or keep) a visual's worker and lay it out at `width`.
    pub fn ensure(&self, slot: &Arc<VisualSlot>, row_key: u64, width: f32, text_scale: f32) {
        if self.closed.load(Ordering::Acquire) {
            return;
        }
        let Some(source) = self.source.lock().unwrap().clone() else {
            return;
        };
        let session = self
            .sessions
            .lock()
            .unwrap()
            .entry(slot.id.clone())
            .or_insert_with(|| {
                VisualSession::spawn(
                    &self.text,
                    slot.clone(),
                    row_key,
                    source,
                    self.changed.clone(),
                )
            })
            .clone();
        session.send(Cmd::Layout {
            width,
            scale: text_scale,
        });
    }

    pub fn session(&self, id: &str) -> Option<Arc<VisualSession>> {
        self.sessions.lock().unwrap().get(id).cloned()
    }

    pub fn shutdown(&self) {
        self.closed.store(true, Ordering::Release);
        for (_, session) in self.sessions.lock().unwrap().drain() {
            session.send(Cmd::Shutdown);
        }
    }
}

impl Drop for VisualHub {
    fn drop(&mut self) {
        self.shutdown();
    }
}

// ---- session ------------------------------------------------------------------

enum Cmd {
    Layout {
        width: f32,
        scale: f32,
    },
    Attach {
        listener: Arc<dyn VisualListener>,
        dark: bool,
        reduce_motion: bool,
    },
    Detach,
    Appearance {
        dark: bool,
    },
    Pointer {
        surface: u64,
        phase: PointerPhase,
        x: f32,
        y: f32,
    },
    Activate {
        control: u64,
    },
    Slide {
        control: u64,
        fraction: f32,
    },
    Shutdown,
}

/// One visual: the handle the platform paints and touches through.
#[derive(uniffi::Object)]
pub struct VisualSession {
    tx: Mutex<Sender<Cmd>>,
    title: String,
}

impl VisualSession {
    fn spawn(
        text: &Arc<TextSystem>,
        slot: Arc<VisualSlot>,
        row_key: u64,
        source: Source,
        changed: Arc<dyn Fn(u64) + Send + Sync>,
    ) -> Arc<Self> {
        let (tx, rx) = mpsc::channel();
        let title = slot.title.clone();
        let text = text.clone();
        std::thread::Builder::new()
            .name("zeron-visual".into())
            .spawn(move || Worker::new(&text, rx, slot, row_key, source, changed).run())
            .expect("spawn visual thread");
        Arc::new(Self {
            tx: Mutex::new(tx),
            title,
        })
    }

    fn send(&self, cmd: Cmd) {
        let _ = self.tx.lock().unwrap().send(cmd);
    }
}

#[uniffi::export]
impl VisualSession {
    /// Start delivering displays and frames (replacing any listener).
    pub fn attach(&self, listener: Arc<dyn VisualListener>, dark: bool, reduce_motion: bool) {
        self.send(Cmd::Attach {
            listener,
            dark,
            reduce_motion,
        });
    }

    /// Stop delivering (offscreen); animation pauses.
    pub fn detach(&self) {
        self.send(Cmd::Detach);
    }

    pub fn set_appearance(&self, dark: bool) {
        self.send(Cmd::Appearance { dark });
    }

    /// A touch on a surface, in its coordinates.
    pub fn pointer(&self, surface: u64, phase: PointerPhase, x: f32, y: f32) {
        self.send(Cmd::Pointer {
            surface,
            phase,
            x,
            y,
        });
    }

    /// Tap a button, tab, toggle or tappable stack.
    pub fn activate(&self, control: u64) {
        self.send(Cmd::Activate { control });
    }

    /// Move a slider to `fraction` (0–1) of its track.
    pub fn slide(&self, control: u64, fraction: f32) {
        self.send(Cmd::Slide { control, fraction });
    }

    pub fn title(&self) -> String {
        self.title.clone()
    }
}

// ---- worker -----------------------------------------------------------------------

/// Frames for animated surfaces come this often at most.
const FRAME: Duration = Duration::from_micros(16_667);
/// GPU surfaces render at most this dense (3× phones would quadruple the
/// pixels for little visible gain, and every frame crosses to the platform).
const MAX_GPU_SCALE: f32 = 2.0;

enum Load {
    Loading,
    Ready,
    Failed(String),
}

struct Orbit {
    yaw: f32,
    pitch: f32,
    spin_epoch: Instant,
    /// Touch start and the camera then.
    drag: Option<([f32; 2], f32, f32)>,
}

impl Orbit {
    fn yaw(&self, spin: f32) -> f32 {
        match self.drag {
            Some(_) => self.yaw,
            None => self.yaw + spin * self.spin_epoch.elapsed().as_secs_f32(),
        }
    }
}

/// One surface's state between frames.
struct SurfaceState {
    spec: SurfaceSpec,
    dirty: bool,
    /// Touch position and whether it's down.
    pointer: Option<[f32; 2]>,
    down: bool,
    press: Option<[f32; 2]>,
    moved: bool,
    reads_pointer: bool,
    orbit: Option<Orbit>,
    shader: Option<zeron_visual_gpu::ShaderState>,
    /// A scene's GPU targets, kept while its size holds.
    scene_gpu: Option<zeron_visual_gpu::SceneState>,
    shader_frame: u32,
    shader_time: f64,
    scene: Option<(u64, zeron_visual::Scene)>,
    hints: Vec<([f32; 2], f32, String)>,
    hint: Option<String>,
    hover: chart::Hover,
}

impl SurfaceState {
    fn new(spec: SurfaceSpec) -> Self {
        let orbit = match &spec.node {
            SurfaceNode::Scene(s) => Some((s.yaw, s.pitch)),
            SurfaceNode::Shader(s) => Some((s.yaw, s.pitch)),
            _ => None,
        }
        .map(|(yaw, pitch)| Orbit {
            yaw,
            pitch,
            spin_epoch: Instant::now(),
            drag: None,
        });
        Self {
            spec,
            dirty: true,
            pointer: None,
            down: false,
            press: None,
            moved: false,
            reads_pointer: false,
            orbit,
            shader: None,
            scene_gpu: None,
            shader_frame: 0,
            shader_time: 0.0,
            scene: None,
            hints: Vec::new(),
            hint: None,
            hover: chart::Hover::default(),
        }
    }
}

struct Worker {
    rx: Receiver<Cmd>,
    slot: Arc<VisualSlot>,
    row_key: u64,
    changed: Arc<dyn Fn(u64) + Send + Sync>,
    source: Source,
    typo: Typography,
    cache: WidthCache,
    load: Load,
    visual: Option<Visual>,
    tree: Option<Node>,
    /// Bumped on every tree render: scene drawings key on it.
    generation: u64,
    error: Option<String>,
    width: f32,
    scale: f32,
    palette: Palette,
    reduce_motion: bool,
    laid: Laid,
    revision: u64,
    display_dirty: bool,
    listener: Option<Arc<dyn VisualListener>>,
    surfaces: HashMap<u64, SurfaceState>,
    started: Instant,
    last_frame: Instant,
    /// When to try loading again after the host couldn't be reached, and
    /// how many tries so far.
    retry: Option<(Instant, u32)>,
}

impl Worker {
    fn new(
        text: &TextSystem,
        rx: Receiver<Cmd>,
        slot: Arc<VisualSlot>,
        row_key: u64,
        source: Source,
        changed: Arc<dyn Fn(u64) + Send + Sync>,
    ) -> Self {
        Self {
            rx,
            slot,
            row_key,
            changed,
            source,
            typo: crate::layout::typography(text),
            cache: WidthCache::new(),
            load: Load::Loading,
            visual: None,
            tree: None,
            generation: 0,
            error: None,
            width: 0.0,
            scale: 1.0,
            palette: Palette::new(false),
            reduce_motion: false,
            laid: Laid::default(),
            revision: 0,
            display_dirty: false,
            listener: None,
            surfaces: HashMap::new(),
            started: Instant::now(),
            last_frame: Instant::now(),
            retry: None,
        }
    }

    fn run(mut self) {
        // Measure the loading line first, so the row has a height while the
        // script and its files arrive.
        let queued: Vec<Cmd> = self.rx.try_iter().collect();
        if !self.drain(queued) {
            return;
        }
        self.relayout();
        self.load();
        self.relayout();
        loop {
            if self.retry.is_some_and(|(at, _)| Instant::now() >= at) {
                self.load();
                self.relayout();
            }
            self.publish();
            let first = match self.next_frame() {
                Some(wait) => match self.rx.recv_timeout(wait) {
                    Ok(cmd) => Some(cmd),
                    Err(RecvTimeoutError::Timeout) => None,
                    Err(RecvTimeoutError::Disconnected) => return,
                },
                None => match self.rx.recv() {
                    Ok(cmd) => Some(cmd),
                    Err(_) => return,
                },
            };
            let cmds: Vec<Cmd> = first.into_iter().chain(self.rx.try_iter()).collect();
            if !self.drain(cmds) {
                return;
            }
        }
    }

    /// Apply commands; false on shutdown.
    fn drain(&mut self, cmds: Vec<Cmd>) -> bool {
        let mut relayout = false;
        for cmd in cmds {
            match cmd {
                Cmd::Shutdown => return false,
                Cmd::Layout { width, scale } => {
                    if (width - self.width).abs() > 0.01
                        || (scale - self.scale).abs() > f32::EPSILON
                    {
                        if (scale - self.typo.scale).abs() > f32::EPSILON {
                            self.typo.scale = scale;
                            self.cache = WidthCache::new();
                        }
                        self.width = width;
                        self.scale = scale;
                        relayout = true;
                    }
                }
                Cmd::Attach {
                    listener,
                    dark,
                    reduce_motion,
                } => {
                    self.listener = Some(listener);
                    self.reduce_motion = reduce_motion;
                    if self.palette.dark != dark {
                        self.palette = Palette::new(dark);
                        relayout = true;
                    }
                    self.display_dirty = true;
                    for state in self.surfaces.values_mut() {
                        state.dirty = true;
                    }
                }
                Cmd::Detach => {
                    self.listener = None;
                }
                Cmd::Appearance { dark } => {
                    if self.palette.dark != dark {
                        self.palette = Palette::new(dark);
                        relayout = true;
                    }
                }
                Cmd::Pointer {
                    surface,
                    phase,
                    x,
                    y,
                    // A handler re-renders the tree; the next command must find
                    // its controls (and handler ids) in that tree.
                } => {
                    if self.pointer(surface, phase, [x, y]) {
                        self.relayout();
                    }
                }
                Cmd::Activate { control } => {
                    if self.activate(control) {
                        self.relayout();
                    }
                }
                Cmd::Slide { control, fraction } => {
                    if self.slide(control, fraction) {
                        self.relayout();
                    }
                }
            }
        }
        if relayout {
            self.relayout();
        }
        true
    }

    // ---- loading ---------------------------------------------------------------

    fn fetch(&self, path: String) -> Option<Vec<u8>> {
        (self.source.fetch)(&path)
    }

    fn load(&mut self) {
        let path = self.slot.path.clone();
        let Some(script) = self
            .fetch(path.clone())
            .and_then(|b| String::from_utf8(b).ok())
        else {
            // The host may be briefly out of reach: try again, backing off.
            let tries = self.retry.map_or(0, |(_, tries)| tries) + 1;
            let wait = Duration::from_secs([2, 5, 15, 30][(tries as usize - 1).min(3)]);
            self.retry = Some((Instant::now() + wait, tries));
            self.load = Load::Failed("Visual unavailable".into());
            return;
        };
        self.retry = None;
        // Its files sit beside it, listed in `{id}-assets.json` (absent for
        // a visual without any).
        let mut files = HashMap::new();
        if let Some(stem) = path.strip_suffix("-visual.luau")
            && let Some(names) = self
                .fetch(format!("{stem}-assets.json"))
                .and_then(|bytes| serde_json::from_slice::<Vec<String>>(&bytes).ok())
        {
            for name in names
                .into_iter()
                .filter(|name| zeron_visual::assets::valid_name(name))
                .take(zeron_visual::assets::MAX_ASSETS)
            {
                if let Some(bytes) = self.fetch(format!("{stem}-asset-{name}")) {
                    files.insert(name, Arc::<[u8]>::from(bytes));
                }
            }
        }
        // Seeded like the engine's check, so the reader sees the same data.
        let seed = zeron_visual::seed_for(&self.slot.id);
        match Visual::load(&script, seed) {
            Ok(visual) => {
                self.visual = Some(visual.with_assets(zeron_visual::Assets::new(files)));
                self.load = Load::Ready;
                self.rerender();
            }
            Err(error) => self.load = Load::Failed(format!("Visual failed: {error}")),
        }
    }

    // ---- tree, layout, display -------------------------------------------------

    /// Re-run the view function after state changed.
    fn rerender(&mut self) {
        let Some(visual) = self.visual.as_mut() else {
            return;
        };
        match visual.render() {
            Ok(tree) => {
                self.tree = Some(tree);
                self.generation += 1;
                self.error = None;
            }
            Err(error) => self.error = Some(error.message),
        }
    }

    fn dispatch(&mut self, handler: HandlerId, arg: HandlerArg) -> bool {
        let Some(visual) = self.visual.as_mut() else {
            return false;
        };
        match visual.call(handler, arg) {
            Ok(()) => self.rerender(),
            Err(error) => self.error = Some(error.message),
        }
        true
    }

    fn status(&mut self, message: &str, color: Rgba, laid: &mut Laid, y: f32) -> f32 {
        let mut ctx = TextCtx {
            typo: &mut self.typo,
            cache: &mut self.cache,
        };
        let para = ctx.line(
            message,
            Face::new(zeron_visual::TextSize::Sm, zeron_visual::Weight::Regular),
            color,
        );
        laid.paint.para(
            &mut ctx,
            &para,
            0.0,
            y,
            self.width,
            zeron_visual::Align::Start,
            false,
        )
    }

    fn relayout(&mut self) {
        if self.width <= 0.0 {
            return;
        }
        let mut laid = match (&self.load, &self.tree) {
            (Load::Ready, Some(tree)) => {
                let mut ctx = TextCtx {
                    typo: &mut self.typo,
                    cache: &mut self.cache,
                };
                layout::lay_out(&mut ctx, &self.palette, tree, self.width)
            }
            (Load::Failed(message), _) => {
                let (message, color) = (message.clone(), self.palette.muted);
                let mut laid = Laid::default();
                laid.height = self.status(&message, color, &mut laid, 0.0);
                laid
            }
            _ => {
                let color = self.palette.faint;
                let mut laid = Laid::default();
                laid.height = self.status("Loading visual…", color, &mut laid, 0.0);
                laid
            }
        };
        if let Some(error) = self.error.clone() {
            let top = if laid.height > 0.0 {
                laid.height + 8.0
            } else {
                0.0
            };
            let color = self.palette.danger;
            laid.height =
                top + self.status(&format!("Visual error: {error}"), color, &mut laid, top);
        }
        // Keep each surface's state (camera, buffers, touch) across renders.
        let mut surfaces = HashMap::with_capacity(laid.surfaces.len());
        for spec in &laid.surfaces {
            let state = match self.surfaces.remove(&spec.id) {
                Some(mut state) => {
                    if state.spec != *spec {
                        state.spec = spec.clone();
                        state.dirty = true;
                    }
                    state
                }
                None => SurfaceState::new(spec.clone()),
            };
            surfaces.insert(spec.id, state);
        }
        // A new tree or palette changes what every surface draws.
        for state in surfaces.values_mut() {
            state.dirty = true;
        }
        self.surfaces = surfaces;
        let height = laid.height.ceil();
        self.laid = laid;
        self.display_dirty = true;
        if self.slot.set(self.width, height) {
            (self.changed)(self.row_key);
        }
    }

    fn display(&self) -> VisualDisplay {
        VisualDisplay {
            revision: self.revision,
            width: self.width,
            height: self.laid.height.ceil(),
            dark: self.palette.dark,
            paint: self.laid.paint.snapshot(),
            surfaces: self
                .laid
                .surfaces
                .iter()
                .map(|spec| {
                    let reads = self.surfaces.get(&spec.id).is_some_and(|s| s.reads_pointer);
                    VisualSurface {
                        id: spec.id,
                        x: spec.rect[0],
                        y: spec.rect[1],
                        w: spec.rect[2],
                        h: spec.rect[3],
                        gesture: layout::gesture(&spec.node, reads),
                    }
                })
                .collect(),
            controls: self
                .laid
                .controls
                .iter()
                .map(|c| c.control.clone())
                .collect(),
            styles: self.typo.table().to_vec(),
        }
    }

    /// Send what changed: the display, then due frames.
    fn publish(&mut self) {
        let Some(listener) = self.listener.clone() else {
            return;
        };
        // Frames first learn whether a canvas reads the pointer, which the
        // display reports; a draw error is laid out under the visual.
        let error = self.error.clone();
        let mut frames = self.frames();
        if self.error != error {
            self.relayout();
            frames.extend(self.frames());
        }
        if self.display_dirty {
            self.display_dirty = false;
            self.revision += 1;
            listener.display(self.display());
        }
        for frame in frames {
            listener.frame(frame);
        }
    }

    fn time(&self) -> f64 {
        self.started.elapsed().as_secs_f64()
    }

    fn moving(&self, state: &SurfaceState) -> bool {
        if self.reduce_motion {
            return false;
        }
        let dragging = state.orbit.as_ref().is_some_and(|o| o.drag.is_some());
        match &state.spec.node {
            SurfaceNode::Canvas(canvas) => canvas.animate,
            SurfaceNode::Scene(scene) => scene.animate || (scene.spin != 0.0 && !dragging),
            SurfaceNode::Shader(shader) => {
                shader.animate || !shader.buffers.is_empty() || (shader.spin != 0.0 && !dragging)
            }
            SurfaceNode::Chart { .. } => false,
        }
    }

    /// How long to wait for commands before the next animation frame (or
    /// load retry).
    fn next_frame(&self) -> Option<Duration> {
        let retry = self
            .retry
            .map(|(at, _)| at.saturating_duration_since(Instant::now()));
        let frame = self.listener.as_ref().and_then(|_| {
            self.surfaces
                .values()
                .any(|s| self.moving(s))
                .then(|| FRAME.saturating_sub(self.last_frame.elapsed()))
        });
        match (frame, retry) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    // ---- frames -------------------------------------------------------------------

    fn frames(&mut self) -> Vec<SurfaceFrame> {
        let ids: Vec<u64> = self.laid.surfaces.iter().map(|s| s.id).collect();
        let mut out = Vec::new();
        let mut rendered = false;
        for id in ids {
            let Some(state) = self.surfaces.get(&id) else {
                continue;
            };
            // Moving surfaces draw at the frame rate, however often commands
            // wake the worker; changed ones draw now.
            if !(state.dirty || self.moving(state) && self.last_frame.elapsed() >= FRAME) {
                continue;
            }
            rendered = true;
            if let Some(frame) = self.frame(id) {
                out.push(frame);
            }
        }
        if rendered {
            self.last_frame = Instant::now();
        }
        out
    }

    fn frame(&mut self, id: u64) -> Option<SurfaceFrame> {
        let mut state = self.surfaces.remove(&id)?;
        state.dirty = false;
        let frame = match state.spec.node.clone() {
            SurfaceNode::Canvas(canvas) => self.canvas_frame(&mut state, &canvas),
            SurfaceNode::Chart { chart, legend } => {
                let mut ctx = TextCtx {
                    typo: &mut self.typo,
                    cache: &mut self.cache,
                };
                let paint = chart::paint(
                    &mut ctx,
                    &self.palette,
                    &chart,
                    state.spec.rect[2],
                    legend,
                    state.hover,
                );
                Some((None, paint))
            }
            SurfaceNode::Scene(scene) => self.scene_frame(&mut state, &scene),
            SurfaceNode::Shader(shader) => self.shader_frame(&mut state, &shader),
        };
        self.surfaces.insert(id, state);
        let (image, paint) = frame?;
        Some(SurfaceFrame {
            surface: id,
            image,
            paint: paint.finish(),
            styles: self.typo.table().to_vec(),
        })
    }

    fn canvas_frame(
        &mut self,
        state: &mut SurfaceState,
        canvas: &zeron_visual::Canvas,
    ) -> Option<(Option<FrameImage>, Paint)> {
        let (w, h) = state.spec.size();
        let pointer = state.pointer.map(|[x, y]| zeron_visual::Pointer {
            x,
            y,
            down: state.down,
        });
        let time = self.time();
        let drawn = self
            .visual
            .as_mut()?
            .draw_canvas_traced(canvas.draw, w, h, time, pointer);
        let drawing = match drawn {
            Ok(drawing) => drawing,
            Err(error) => {
                self.error = Some(error.message);
                return None;
            }
        };
        if drawing.reads_pointer != state.reads_pointer {
            state.reads_pointer = drawing.reads_pointer;
            self.display_dirty = true;
        }
        let mut ctx = TextCtx {
            typo: &mut self.typo,
            cache: &mut self.cache,
        };
        let mut paint = Paint::default();
        for command in &drawing.commands {
            match command {
                DrawCmd::Stroke {
                    points,
                    closed,
                    color,
                    width,
                    dash,
                } => paint.stroke(
                    points.iter().copied(),
                    *closed,
                    *width,
                    *dash,
                    self.palette.color(*color),
                ),
                DrawCmd::Fill { points, color } => {
                    paint.fill(points.iter().copied(), self.palette.color(*color))
                }
                DrawCmd::Text(text) => {
                    let face = Face {
                        size: text.size,
                        weight: text.weight,
                        mono: text.mono,
                        italic: false,
                    };
                    paint.label(
                        &mut ctx,
                        &text.text,
                        face,
                        self.palette.color(text.color),
                        [text.x, text.y],
                        text.align,
                        text.baseline,
                    );
                }
            }
        }
        Some((None, paint))
    }

    fn scene_frame(
        &mut self,
        state: &mut SurfaceState,
        node: &zeron_visual::SceneNode,
    ) -> Option<(Option<FrameImage>, Paint)> {
        let (w, h) = state.spec.size();
        let animate = node.animate && !self.reduce_motion;
        let fresh = state
            .scene
            .as_ref()
            .is_some_and(|(generation, _)| *generation == self.generation && !animate);
        if !fresh {
            let time = self.time();
            match self.visual.as_mut()?.draw_scene(node.draw, time) {
                Ok(scene) => state.scene = Some((self.generation, scene)),
                Err(error) => {
                    self.error = Some(error.message);
                    return None;
                }
            }
        }
        let (_, scene) = state.scene.as_ref()?;
        let spin = if self.reduce_motion { 0.0 } else { node.spin };
        let orbit = state.orbit.as_ref()?;
        let camera = zeron_visual::Camera {
            yaw: orbit.yaw(spin),
            pitch: orbit.pitch,
            zoom: node.zoom,
        };
        let palette = self.palette;
        let rgba = move |color| {
            let c = palette.color(color);
            [c.r, c.g, c.b, c.a]
        };
        let pixel_scale = self.source.pixel_scale.min(MAX_GPU_SCALE);
        let (image, hints, labels) =
            match zeron_visual_gpu::render_scene(
                scene,
                camera,
                w,
                h,
                pixel_scale,
                &rgba,
                &mut state.scene_gpu,
            ) {
                Some(frame) => {
                    let (hints, labels) = scene.annotations(camera, w, h);
                    (
                        image_of(frame.width, frame.height, frame.bgra),
                        hints,
                        labels,
                    )
                }
                None => {
                    // No GPU: the software renderer, rougher while it moves.
                    let quality = if self.moving(state) || orbit.drag.is_some() {
                        zeron_visual::Quality::Draft
                    } else {
                        zeron_visual::Quality::Final
                    };
                    let raster = scene.render(camera, w, h, pixel_scale, quality, &rgba);
                    (
                        image_of(raster.width, raster.height, raster.bgra),
                        raster.hints,
                        raster.labels,
                    )
                }
            };
        state.hints = hints;
        let mut ctx = TextCtx {
            typo: &mut self.typo,
            cache: &mut self.cache,
        };
        let mut paint = Paint::default();
        for label in &labels {
            let face = Face::new(label.size, zeron_visual::Weight::Regular);
            paint.label(
                &mut ctx,
                &label.text,
                face,
                palette.color(label.color),
                label.at,
                zeron_visual::Align::Center,
                zeron_visual::Align::End,
            );
        }
        if let Some(hint) = &state.hint {
            let face = Face::new(zeron_visual::TextSize::Sm, zeron_visual::Weight::Medium);
            paint.label(
                &mut ctx,
                hint,
                face,
                palette.text,
                [0.0, 0.0],
                zeron_visual::Align::Start,
                zeron_visual::Align::Start,
            );
        }
        Some((Some(image), paint))
    }

    fn shader_frame(
        &mut self,
        state: &mut SurfaceState,
        node: &zeron_visual::ShaderNode,
    ) -> Option<(Option<FrameImage>, Paint)> {
        let (w, h) = state.spec.size();
        let scale = self.source.pixel_scale.min(MAX_GPU_SCALE);
        let size = (
            (w * scale).round().max(1.0) as u32,
            (h * scale).round().max(1.0) as u32,
        );
        let animate = node.animate && !self.reduce_motion;
        let spin = if self.reduce_motion { 0.0 } else { node.spin };
        let orbit = state.orbit.as_ref()?;
        let yaw = orbit.yaw(spin);
        // Shadertoy's mouse: pixels from the bottom-left; xy where the touch
        // is, zw where it went down while it's down (negative after).
        let device = |[x, y]: [f32; 2]| [x * scale, size.1 as f32 - y * scale];
        let mouse = match (state.pointer, state.press) {
            (Some(now), Some(start)) if state.down => {
                let ([x, y], [sx, sy]) = (device(now), device(start));
                [x, y, sx, sy]
            }
            (Some(now), _) => {
                let [x, y] = device(now);
                [x, y, -1.0, -1.0]
            }
            _ => [0.0, 0.0, -1.0, -1.0],
        };
        let mut inputs = [[0.0f32; 4]; zeron_visual::shader::MAX_INPUTS / 4];
        for (i, (_, value)) in node.inputs.iter().enumerate() {
            inputs[i / 4][i % 4] = *value;
        }
        let time = self.time();
        let shown_time = if animate { time } else { state.shader_time };
        let delta = (time - state.shader_time).max(0.0);
        let uniforms = zeron_visual::shader::ShaderUniforms {
            resolution: [size.0 as f32, size.1 as f32, scale, 0.0],
            mouse,
            camera: [yaw, orbit.pitch, node.zoom, 0.0],
            time: [
                shown_time as f32,
                delta as f32,
                state.shader_frame as f32,
                0.0,
            ],
            inputs,
            channels: Default::default(),
        };
        let names: Vec<String> = node.inputs.iter().map(|(n, _)| n.clone()).collect();
        let assets = self.visual.as_ref()?.assets().clone();
        let textures: Vec<zeron_visual::TextureData> = node
            .textures
            .iter()
            // A missing one keeps its slot (and samples as clear).
            .map(|name| {
                zeron_visual::TextureData::Encoded(
                    assets.get(name).unwrap_or_else(|| Arc::from([])),
                )
            })
            .collect();
        let rendered = zeron_visual_gpu::render_shader(
            &zeron_visual_gpu::ShaderSpec {
                image: &node.code,
                buffers: &node.buffers,
                inputs: &names,
                blend: node.blend,
                bloom: node.bloom,
                textures: &textures,
            },
            &uniforms,
            size.0,
            size.1,
            &mut state.shader,
        );
        state.shader_frame += 1;
        if animate || !node.buffers.is_empty() {
            state.shader_time = time;
        }
        match rendered {
            Ok(frame) => Some((
                Some(image_of(frame.width, frame.height, frame.bgra)),
                Paint::default(),
            )),
            Err(error) => {
                self.error = Some(error);
                self.display_dirty = true;
                None
            }
        }
    }

    // ---- input ------------------------------------------------------------------

    /// A touch on a surface; true when the tree must be laid out again.
    fn pointer(&mut self, id: u64, phase: PointerPhase, at: [f32; 2]) -> bool {
        let Some(mut state) = self.surfaces.remove(&id) else {
            return false;
        };
        state.dirty = true;
        let mut call = None;
        match phase {
            PointerPhase::Down => {
                state.pointer = Some(at);
                state.down = true;
                state.press = Some(at);
                state.moved = false;
                let spin = match &state.spec.node {
                    SurfaceNode::Scene(s) => s.spin,
                    SurfaceNode::Shader(s) => s.spin,
                    _ => 0.0,
                };
                let orbits = matches!(&state.spec.node, SurfaceNode::Scene(s) if s.orbit)
                    || matches!(&state.spec.node, SurfaceNode::Shader(s) if s.orbit);
                if let Some(orbit) = state.orbit.as_mut()
                    && orbits
                {
                    orbit.yaw = orbit.yaw(if self.reduce_motion { 0.0 } else { spin });
                    orbit.drag = Some((at, orbit.yaw, orbit.pitch));
                }
            }
            PointerPhase::Move => {
                state.pointer = Some(at);
                if let Some([sx, sy]) = state.press
                    && (at[0] - sx).hypot(at[1] - sy) > 8.0
                {
                    state.moved = true;
                }
                if let Some(orbit) = state.orbit.as_mut()
                    && let Some(([sx, sy], yaw, pitch)) = orbit.drag
                {
                    orbit.yaw = yaw - (at[0] - sx) * 0.01;
                    orbit.pitch = (pitch + (at[1] - sy) * 0.01).clamp(-1.5, 1.5);
                }
            }
            PointerPhase::Up | PointerPhase::Cancel => {
                state.pointer = Some(at);
                state.down = false;
                if let Some(orbit) = state.orbit.as_mut()
                    && orbit.drag.take().is_some()
                {
                    orbit.spin_epoch = Instant::now();
                }
                let tap = phase == PointerPhase::Up && !state.moved;
                match &state.spec.node {
                    SurfaceNode::Canvas(canvas) if tap => {
                        call = canvas
                            .on_click
                            .map(|h| (h, HandlerArg::Point(at[0], at[1])));
                    }
                    SurfaceNode::Scene(_) if tap => {
                        // Labels a touch lands near, a fingertip wide.
                        state.hint = state
                            .hints
                            .iter()
                            .map(|([hx, hy], r, text)| ((hx - at[0]).hypot(hy - at[1]), *r, text))
                            .filter(|(d, r, _)| *d <= r + 16.0)
                            .min_by(|a, b| a.0.total_cmp(&b.0))
                            .map(|(_, _, text)| text.clone());
                    }
                    _ => {}
                }
            }
        }
        if let SurfaceNode::Chart { chart, legend } = &state.spec.node {
            state.hover = if state.down {
                let mut ctx = TextCtx {
                    typo: &mut self.typo,
                    cache: &mut self.cache,
                };
                chart::hover_at(&mut ctx, chart, state.spec.rect[2], *legend, at)
            } else {
                chart::Hover::default()
            };
        }
        self.surfaces.insert(id, state);
        match call {
            Some((handler, arg)) => self.dispatch(handler, arg),
            None => false,
        }
    }

    fn control(&self, id: u64) -> Option<layout::ControlSpec> {
        self.laid
            .controls
            .iter()
            .find(|c| c.control.id == id)
            .cloned()
    }

    fn activate(&mut self, id: u64) -> bool {
        let Some(control) = self.control(id) else {
            return false;
        };
        match control.action {
            Action::Call(handler, ActionArg::None) => self.dispatch(handler, HandlerArg::None),
            Action::Call(handler, ActionArg::Bool(value)) => {
                self.dispatch(handler, HandlerArg::Bool(value))
            }
            Action::Call(handler, ActionArg::Text(value)) => {
                self.dispatch(handler, HandlerArg::Text(value))
            }
            Action::Call(_, ActionArg::Slider { .. }) | Action::Link => false,
        }
    }

    fn slide(&mut self, id: u64, fraction: f32) -> bool {
        let Some(control) = self.control(id) else {
            return false;
        };
        let (
            Action::Call(handler, ActionArg::Slider { min, max, step }),
            ControlKind::Slider { value: current, .. },
        ) = (control.action, control.control.kind)
        else {
            return false;
        };
        let value = slider_value(min, max, step, fraction);
        if (value - current).abs() <= f64::EPSILON {
            return false;
        }
        self.dispatch(handler, HandlerArg::Number(value))
    }
}

/// The value at `fraction` of a slider's track, on its steps.
fn slider_value(min: f64, max: f64, step: f64, fraction: f32) -> f64 {
    let mut value = min + (max - min) * f64::from(fraction.clamp(0.0, 1.0));
    if step > 0.0 {
        value = min + ((value - min) / step).round() * step;
    }
    value.clamp(min, max)
}

/// Straight BGRA from the renderers → premultiplied for the platform.
fn image_of(width: u32, height: u32, mut bgra: Vec<u8>) -> FrameImage {
    for px in bgra.chunks_exact_mut(4) {
        let a = u16::from(px[3]);
        if a < 255 {
            for c in &mut px[..3] {
                *c = ((u16::from(*c) * a + 127) / 255) as u8;
            }
        }
    }
    FrameImage {
        width,
        height,
        bgra,
    }
}

/// Drive a future to completion on this (non-async) thread.
fn block_on<F: std::future::Future>(future: F) -> F::Output {
    use std::task::{Context, Poll, Wake, Waker};
    struct Unpark(std::thread::Thread);
    impl Wake for Unpark {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }
    let waker = Waker::from(Arc::new(Unpark(std::thread::current())));
    let mut cx = Context::from_waker(&waker);
    let mut future = std::pin::pin!(future);
    loop {
        if let Poll::Ready(out) = future.as_mut().poll(&mut cx) {
            return out;
        }
        std::thread::park();
    }
}

#[cfg(test)]
mod tests;
