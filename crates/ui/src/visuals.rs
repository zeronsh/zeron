//! Agent visuals: Luau scripts an agent shows inline in its reply through
//! Zeron's `show_visual` MCP tool (charts, tables, dashboards, simulations,
//! 3D views).
//!
//! A script returns a tree of `ui.*` nodes ([`zeron_visual`]), and this module
//! renders it with the same GPUI elements, type scale, and theme as the
//! transcript around it. Nothing foreign is hosted: a visual scrolls, clips,
//! fades under the glass composer, and takes the pointer exactly like a
//! markdown table, on every platform. Controls call the script's handlers,
//! which change its state, and the view renders again. Canvases and scenes
//! run their draw functions at paint time and are painted as GPUI paths and
//! shaped text in theme colors.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use gpui::AppContext as _;
use gpui::prelude::FluentBuilder as _;
use gpui::{
    AnyElement, App, Bounds, Context, EventEmitter, FontStyle, FontWeight, Hsla,
    InteractiveElement, IntoElement, MouseButton, MouseMoveEvent, ParentElement, PathBuilder,
    Pixels, Point, Render, SharedString, StatefulInteractiveElement, Styled, StyledText, Task,
    TextAlign, TextRun, Window, canvas, div, point, px,
};
use zeron_visual::{
    Align, ChartKind, DrawCmd, HandlerArg, HandlerId, Justify, Node, NodeKind, Token,
};

use crate::markdown::render::LinkUi;
use crate::state::EngineHandle;
use crate::theme::Theme;

/// The visual's tree changed shape; the transcript remeasures its row.
pub struct VisualLayoutChanged;

enum Load {
    Loading,
    Ready,
    Failed(SharedString),
}

/// One inline visual in a transcript.
pub struct VisualFrame {
    id: SharedString,
    title: SharedString,
    load: Load,
    runtime: Option<Rc<RefCell<zeron_visual::Visual>>>,
    tree: Option<Rc<Node>>,
    /// Bumped on every render of the tree: draw caches key on it.
    generation: u64,
    /// The latest handler or draw failure; the last good tree stays up.
    error: Option<SharedString>,
    link_ui: Option<LinkUi>,
    local: Rc<RefCell<Local>>,
    started: Instant,
    _tasks: Vec<Task<()>>,
}

impl EventEmitter<VisualLayoutChanged> for VisualFrame {}

/// View-local state the script doesn't own, keyed by node key or tree path.
#[derive(Default)]
struct Local {
    /// The pointer over a canvas, chart, or scene, in window coordinates.
    pointers: HashMap<SharedString, Point<Pixels>>,
    pressed: Option<SharedString>,
    /// Where the pointer went down on a shader, while it's held (iMouse.zw).
    presses: HashMap<SharedString, Point<Pixels>>,
    /// Where each chart's plot was painted, for hover hit-testing.
    charts: HashMap<SharedString, ChartGeometry>,
    cameras: HashMap<SharedString, Orbit>,
    canvases: HashMap<SharedString, CanvasCache>,
    scenes: HashMap<SharedString, SceneCache>,
    shaders: HashMap<SharedString, ShaderCache>,
    draw_error: Option<String>,
}

/// A shader's newest finished frame, and the next one in the works. Frames
/// render on a background thread; the view shows the newest one meanwhile.
#[derive(Default)]
struct ShaderCache {
    /// The newest frame and the uniforms it was drawn with.
    shown: Option<(zeron_visual::shader::ShaderUniforms, Arc<gpui::RenderImage>)>,
    /// Frames started, and the wall time the last one was started at.
    frame: u32,
    last_time: f64,
    /// Buffers carried between frames (feedback, simulations); away on the
    /// render thread while a frame renders.
    gpu: Option<zeron_visual_gpu::ShaderState>,
    rendering: bool,
}

struct CanvasCache {
    generation: u64,
    size: (f32, f32),
    pointer: Option<(f32, f32, bool)>,
    commands: Rc<Vec<DrawCmd>>,
}

/// A scene's drawing, its newest finished image, and the next one in the
/// works (rendered on a background thread, like shaders).
struct SceneCache {
    generation: u64,
    /// Bumped with every new drawing, so an image of an older one is stale.
    drawing: u64,
    scene: Arc<zeron_visual::Scene>,
    /// The newest image, and what it was rendered for.
    image: Option<(RasterKey, SceneImage)>,
    /// How long the last supersampled software render took: cheap scenes
    /// stay supersampled while the camera moves.
    final_cost: Option<std::time::Duration>,
    /// GPU targets kept between frames; away while a frame renders.
    gpu: Option<zeron_visual_gpu::SceneState>,
    rendering: bool,
}

/// What a scene image depends on besides the geometry.
#[derive(Clone, Copy, PartialEq)]
struct RasterKey {
    yaw: f32,
    pitch: f32,
    zoom: f32,
    size: (f32, f32),
    pixel_scale: f32,
    quality: zeron_visual::Quality,
    palette: u64,
    drawing: u64,
}

#[derive(Clone)]
struct SceneImage {
    image: Arc<gpui::RenderImage>,
    hints: Arc<Vec<([f32; 2], f32, String)>>,
    labels: Arc<Vec<zeron_visual::RasterLabel>>,
}

/// A rendered frame as an image GPUI can paint (off the main thread).
fn render_image(width: u32, height: u32, bgra: Vec<u8>) -> Option<Arc<gpui::RenderImage>> {
    let buffer = image::RgbaImage::from_raw(width, height, bgra)?;
    Some(Arc::new(gpui::RenderImage::new([image::Frame::new(
        buffer,
    )])))
}

#[derive(Clone, Copy)]
struct Orbit {
    yaw: f32,
    pitch: f32,
    /// Where the turntable started turning from `yaw`.
    spin_epoch: Instant,
    drag: Option<(Point<Pixels>, f32, f32)>,
}

#[derive(Clone, Copy, Default)]
struct ChartGeometry {
    plot: Bounds<Pixels>,
}

/// The drag payload for scene orbits and sliders: which node is dragging.
#[derive(Clone)]
struct VisualDrag(SharedString);

struct DragGhost;

impl Render for DragGhost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        gpui::Empty
    }
}

impl VisualFrame {
    pub fn new(
        id: SharedString,
        title: String,
        path: String,
        engine: Option<EngineHandle>,
        target_device_id: Option<String>,
        cx: &mut Context<Self>,
    ) -> Self {
        // Its drawings leave every window's sprite atlas with it.
        cx.on_release(|this, cx| {
            let images = this.local.borrow_mut().prune(None);
            free_images(images, cx);
        })
        .detach();
        let mut tasks = Vec::new();
        let load = match engine {
            Some(engine) => {
                tasks.push(cx.spawn(async move |this, cx| {
                    let read = |path: String| {
                        let (engine, target) = (engine.clone(), target_device_id.clone());
                        let executor = cx.background_executor().clone();
                        async move {
                            crate::attachments::read_attachment_bytes(
                                &engine,
                                &executor,
                                target.as_deref(),
                                &path,
                                None,
                            )
                            .await
                            .map(|(_, _, bytes)| bytes)
                        }
                    };
                    let script = read(path.clone())
                        .await
                        .and_then(|bytes| String::from_utf8(bytes).ok());
                    // Its files sit beside it, listed in `{id}-assets.json`
                    // (absent for a visual without any).
                    let mut files = HashMap::new();
                    if script.is_some()
                        && let Some(stem) = path.strip_suffix("-visual.luau")
                        && let Some(names) = read(format!("{stem}-assets.json"))
                            .await
                            .and_then(|bytes| serde_json::from_slice::<Vec<String>>(&bytes).ok())
                    {
                        for name in names
                            .into_iter()
                            .filter(|name| zeron_visual::assets::valid_name(name))
                            .take(zeron_visual::assets::MAX_ASSETS)
                        {
                            if let Some(bytes) = read(format!("{stem}-asset-{name}")).await {
                                files.insert(name, Arc::<[u8]>::from(bytes));
                            }
                        }
                    }
                    let assets = zeron_visual::Assets::new(files);
                    let _ = this.update(cx, |this, cx| match script {
                        Some(script) => this.start(&script, assets, cx),
                        None => {
                            this.load = Load::Failed("Visual unavailable".into());
                            cx.notify();
                        }
                    });
                }));
                Load::Loading
            }
            None => Load::Failed("Visual unavailable".into()),
        };
        Self {
            id,
            title: title.into(),
            load,
            runtime: None,
            tree: None,
            generation: 0,
            error: None,
            link_ui: None,
            local: Rc::default(),
            started: Instant::now(),
            _tasks: tasks,
        }
    }

    pub fn set_link_ui(&mut self, link_ui: Option<LinkUi>) {
        self.link_ui = link_ui;
    }

    /// Run a script's setup and first render (the engine already ran the
    /// same script before publishing it, so failures here are rare).
    fn start(&mut self, script: &str, assets: zeron_visual::Assets, cx: &mut Context<Self>) {
        // The row id ends with the part's visual id, which the engine
        // seeded its check with: the reader sees the same random data.
        let visual_id = self.id.rsplit('#').next().unwrap_or(&self.id);
        let seed = zeron_visual::seed_for(visual_id);
        match zeron_visual::Visual::load(script, seed) {
            Ok(visual) => {
                let visual = visual.with_assets(assets);
                self.runtime = Some(Rc::new(RefCell::new(visual)));
                self.load = Load::Ready;
                self.rerender(cx);
            }
            Err(error) => {
                tracing::warn!(visual = %self.id, %error, "visual script failed to load");
                self.load = Load::Failed(format!("Visual failed: {error}").into());
                cx.notify();
            }
        }
    }

    fn rerender(&mut self, cx: &mut Context<Self>) {
        let Some(runtime) = self.runtime.clone() else {
            return;
        };
        let rendered = runtime.borrow_mut().render();
        match rendered {
            Ok(tree) => {
                let changed = self.tree.as_deref() != Some(&tree);
                if changed {
                    // Nodes that left the tree take their drawings with them.
                    let mut keep = std::collections::HashSet::new();
                    Self::node_keys(&tree, &self.id, &mut keep);
                    let images = self.local.borrow_mut().prune(Some(&keep));
                    free_images(images, cx);
                }
                self.tree = Some(Rc::new(tree));
                self.generation += 1;
                self.error = None;
                if changed {
                    cx.emit(VisualLayoutChanged);
                }
            }
            Err(error) => self.fail(error, cx),
        }
        cx.notify();
    }

    fn fail(&mut self, error: zeron_visual::VisualError, cx: &mut Context<Self>) {
        tracing::warn!(visual = %self.id, %error, "visual script error");
        self.error = Some(error.message.into());
        cx.emit(VisualLayoutChanged);
        cx.notify();
    }

    fn dispatch(&mut self, handler: HandlerId, arg: HandlerArg, cx: &mut Context<Self>) {
        let Some(runtime) = self.runtime.clone() else {
            return;
        };
        let called = runtime.borrow_mut().call(handler, arg);
        match called {
            Ok(()) => self.rerender(cx),
            Err(error) => self.fail(error, cx),
        }
    }

    fn open_link(&self, url: &str, window: &mut Window, cx: &mut App) {
        crate::markdown::render::activate_link(
            crate::markdown::links::LinkTarget::new(url, url),
            crate::markdown::links::LinkAction::Primary,
            self.link_ui.as_ref(),
            window,
            cx,
        );
    }

    fn element_id(&self, path: &str) -> SharedString {
        format!("{}:{path}", self.id).into()
    }

    fn time(&self) -> f64 {
        self.started.elapsed().as_secs_f64()
    }
}

// ---- colors and type ------------------------------------------------------

/// Categorical chart colors: the theme's accent, then the five fixed hues
/// that sit farthest from it, so no two series read alike whatever the
/// accent. Solid, deeper on light and brighter on dark, so they hold on
/// glass and opaque surfaces alike.
fn chart_series(theme: &Theme) -> [Hsla; 6] {
    // (hue in turns, light tone, dark tone): teal, amber, rose, violet, lime, sky.
    const FIXED: [(f32, u32, u32); 6] = [
        (0.486, 0x0d9488, 0x2dd4bf),
        (0.106, 0xd97706, 0xfbbf24),
        (0.964, 0xe11d48, 0xfb7185),
        (0.728, 0x7c3aed, 0xa78bfa),
        (0.236, 0x65a30d, 0xa3e635),
        (0.553, 0x0284c7, 0x38bdf8),
    ];
    let accent = theme.accent_strong;
    let distance = |hue: f32| {
        let d = (hue - accent.h).rem_euclid(1.0);
        d.min(1.0 - d)
    };
    // A grey accent has no hue to collide with; drop the last fixed color.
    let closest = if accent.s < 0.15 {
        FIXED.len() - 1
    } else {
        (0..FIXED.len())
            .min_by(|a, b| distance(FIXED[*a].0).total_cmp(&distance(FIXED[*b].0)))
            .unwrap_or(0)
    };
    let light = theme.appearance.is_light();
    let mut out = [accent; 6];
    for (slot, (_, (_, l, d))) in FIXED
        .iter()
        .enumerate()
        .filter(|(ix, _)| *ix != closest)
        .enumerate()
    {
        out[slot + 1] = gpui::rgb(if light { *l } else { *d }).into();
    }
    out
}

/// Theme colors for one paint, resolved once.
#[derive(Clone)]
struct Palette {
    text: Hsla,
    muted: Hsla,
    faint: Hsla,
    accent: Hsla,
    on_accent: Hsla,
    success: Hsla,
    warning: Hsla,
    danger: Hsla,
    border: Hsla,
    chart: [Hsla; 6],
}

impl Palette {
    fn new(theme: &Theme) -> Self {
        Self {
            text: theme.text,
            muted: theme.text_muted,
            faint: theme.text_faint,
            accent: theme.accent,
            on_accent: theme.on_accent,
            success: theme.success,
            warning: theme.warning,
            danger: theme.danger,
            border: theme.border,
            chart: chart_series(theme),
        }
    }

    fn token(&self, token: Token) -> Hsla {
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
            Token::Rgb(rgb) => gpui::rgb(rgb).into(),
        }
    }

    fn color(&self, color: zeron_visual::Color) -> Hsla {
        let mut out = self.token(color.token);
        if let Some((other, t)) = color.mix {
            out = mix_rgb(out, self.token(other), t);
        }
        out.a *= color.alpha;
        out
    }
}

/// Blend in RGB, so a ramp between two hues doesn't swing through a third.
fn mix_rgb(a: Hsla, b: Hsla, t: f32) -> Hsla {
    let (a, b) = (a.to_rgb(), b.to_rgb());
    let lerp = |x: f32, y: f32| x + (y - x) * t;
    gpui::Rgba {
        r: lerp(a.r, b.r),
        g: lerp(a.g, b.g),
        b: lerp(a.b, b.b),
        a: lerp(a.a, b.a),
    }
    .into()
}

fn weight(weight: zeron_visual::Weight) -> FontWeight {
    match weight {
        zeron_visual::Weight::Regular => FontWeight::NORMAL,
        zeron_visual::Weight::Medium => FontWeight::MEDIUM,
        zeron_visual::Weight::Semibold => FontWeight::SEMIBOLD,
        zeron_visual::Weight::Bold => FontWeight::BOLD,
    }
}

/// Visuals set figures in tabular numerals, so columns of numbers align.
fn ui_font(theme: &Theme, mono: bool, w: FontWeight, italic: bool) -> gpui::Font {
    let mut font = gpui::font(if mono {
        theme.font_mono.clone()
    } else {
        theme.font_sans.clone()
    });
    font.weight = w;
    if italic {
        font.style = FontStyle::Italic;
    }
    font.features = gpui::FontFeatures(Arc::new(vec![("tnum".into(), 1)]));
    font
}

fn rems(size: zeron_visual::TextSize) -> (gpui::Rems, gpui::Rems) {
    let (size, line_height) = size.metrics();
    (
        crate::typography::ui_rems(size),
        crate::typography::ui_rems(line_height),
    )
}

/// A text size in window pixels at the reader's interface size.
fn text_px(size: zeron_visual::TextSize, window: &Window) -> (Pixels, Pixels) {
    let (size, line_height) = rems(size);
    (
        size.to_pixels(window.rem_size()),
        line_height.to_pixels(window.rem_size()),
    )
}

// ---- rendering ------------------------------------------------------------

impl Render for VisualFrame {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).clone();
        let body = match (&self.load, self.tree.clone()) {
            (Load::Failed(message), _) => status_line(message.clone(), theme.text_muted),
            (Load::Ready, Some(tree)) => {
                let palette = Palette::new(&theme);
                let root = self.id.to_string();
                self.node(&tree, &root, &theme, &palette, window, cx)
            }
            _ => status_line("Loading visual…".into(), theme.text_faint),
        };
        let error = self
            .error
            .clone()
            .or_else(|| self.local.borrow().draw_error.clone().map(Into::into));
        div()
            .id(self.element_id("frame"))
            .w_full()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .role(gpui::Role::Group)
            .aria_label(self.title.clone())
            .child(body)
            .when_some(error, |el, error| {
                el.child(status_line(
                    format!("Visual error: {error}").into(),
                    theme.danger,
                ))
            })
    }
}

fn status_line(message: SharedString, color: Hsla) -> AnyElement {
    div()
        .text_size(crate::typography::ui_rems(12.0))
        .line_height(crate::typography::ui_rems(18.0))
        .text_color(color)
        .child(message)
        .into_any_element()
}

impl Local {
    /// Forget the drawings of nodes not in `keep` (all of them, without
    /// one), returning their images to free.
    fn prune(
        &mut self,
        keep: Option<&std::collections::HashSet<SharedString>>,
    ) -> Vec<Arc<gpui::RenderImage>> {
        let kept = |key: &SharedString| keep.is_some_and(|keep| keep.contains(key));
        let mut images = Vec::new();
        self.shaders.retain(|key, cache| {
            kept(key) || {
                images.extend(cache.shown.as_ref().map(|(_, image)| image.clone()));
                false
            }
        });
        self.scenes.retain(|key, cache| {
            kept(key) || {
                images.extend(cache.image.as_ref().map(|(_, image)| image.image.clone()));
                false
            }
        });
        self.canvases.retain(|key, _| kept(key));
        self.cameras.retain(|key, _| kept(key));
        images
    }
}

fn free_images(images: Vec<Arc<gpui::RenderImage>>, cx: &mut App) {
    if !images.is_empty() {
        cx.defer(move |cx| {
            for image in images {
                cx.drop_image(image, None);
            }
        });
    }
}

impl VisualFrame {
    /// Every node's key, as [`VisualFrame::node`] walks the tree from `path`.
    fn node_keys(node: &Node, path: &str, out: &mut std::collections::HashSet<SharedString>) {
        out.insert(Self::key(node, path));
        let children: &[Node] = match &node.kind {
            NodeKind::Stack(stack) => &stack.children,
            NodeKind::Grid { children, .. } => children,
            NodeKind::Table(table) => {
                for (r, row) in table.rows.iter().enumerate() {
                    for (c, cell) in row.iter().enumerate() {
                        Self::node_keys(cell, &format!("{path}.{r}.{c}"), out);
                    }
                }
                &[]
            }
            _ => &[],
        };
        for (ix, child) in children.iter().enumerate() {
            Self::node_keys(child, &format!("{path}.{ix}"), out);
        }
    }

    fn key(node: &Node, path: &str) -> SharedString {
        match &node.key {
            Some(key) => format!("key:{key}").into(),
            None => path.to_owned().into(),
        }
    }

    fn node(
        &mut self,
        node: &Node,
        path: &str,
        theme: &Theme,
        palette: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let key = Self::key(node, path);
        match &node.kind {
            NodeKind::Stack(stack) => self.stack(stack, path, theme, palette, window, cx),
            NodeKind::Grid {
                columns,
                gap,
                children,
            } => {
                let mut grid = div().w_full().grid().grid_cols(*columns).gap(px(*gap));
                for (ix, child) in children.iter().enumerate() {
                    let child_path = format!("{path}.{ix}");
                    grid = grid.child(div().min_w_0().child(self.node(
                        child,
                        &child_path,
                        theme,
                        palette,
                        window,
                        cx,
                    )));
                }
                grid.into_any_element()
            }
            NodeKind::Text(text) => text_element(text, theme, palette),
            NodeKind::Rule => div()
                .h(px(1.0))
                .w_full()
                .bg(theme.border)
                .into_any_element(),
            NodeKind::Space(size) => div().flex_none().size(px(*size)).into_any_element(),
            NodeKind::Stat {
                value,
                label,
                delta,
                tone,
            } => stat(value, label, delta.as_deref(), *tone, theme, palette),
            NodeKind::Table(table) => self.table(table, path, theme, palette, window, cx),
            NodeKind::Chart(chart) => self.chart(chart, key, theme, palette, cx),
            NodeKind::Canvas(canvas) => self.canvas(canvas, key, cx),
            NodeKind::Scene(scene) => self.scene(scene, key, cx),
            NodeKind::Shader(shader) => self.shader(shader, key, cx),
            NodeKind::Button {
                label,
                on_click,
                primary,
            } => {
                let handler = *on_click;
                crate::settings::widgets::action_button(
                    theme,
                    if *primary {
                        crate::settings::widgets::ActionTone::Solid
                    } else {
                        crate::settings::widgets::ActionTone::Filled
                    },
                )
                .flex_none()
                .id(self.element_id(path))
                .role(gpui::Role::Button)
                .aria_label(SharedString::from(label.clone()))
                .child(label.clone())
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.dispatch(handler, HandlerArg::None, cx);
                }))
                .into_any_element()
            }
            NodeKind::Tabs {
                options,
                value,
                on_change,
            } => {
                let reduced = cx.reduce_motion();
                let mut row = div().flex().flex_row().flex_wrap().gap(px(2.0));
                for (ix, option) in options.iter().enumerate() {
                    let selected = option == value;
                    let tab_key = format!("{}-tab-{ix}", self.element_id(path));
                    let t = crate::settings::widgets::tab_selection_t(
                        window,
                        tab_key.clone(),
                        selected,
                        reduced,
                    );
                    let (handler, option_value) = (*on_change, option.clone());
                    row = row.child(
                        crate::settings::widgets::section_tab(
                            theme,
                            selected,
                            t,
                            SharedString::from(tab_key.clone()),
                            format!("{tab_key}-hover"),
                        )
                        .cursor_pointer()
                        .role(gpui::Role::Tab)
                        .aria_label(SharedString::from(option.clone()))
                        .child(option.clone())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.dispatch(handler, HandlerArg::Text(option_value.clone()), cx);
                        })),
                    );
                }
                row.into_any_element()
            }
            NodeKind::Toggle {
                label,
                value,
                on_change,
            } => {
                let (handler, next) = (*on_change, !*value);
                div()
                    .id(self.element_id(path))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(10.0))
                    .cursor_pointer()
                    .role(gpui::Role::Switch)
                    .aria_label(SharedString::from(label.clone()))
                    .child(crate::settings::widgets::toggle_switch(
                        theme,
                        *value,
                        self.element_id(path),
                    ))
                    .child(
                        div()
                            .text_size(crate::typography::ui_rems(14.0))
                            .line_height(crate::typography::ui_rems(22.0))
                            .text_color(theme.text)
                            .child(label.clone()),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.dispatch(handler, HandlerArg::Bool(next), cx);
                    }))
                    .into_any_element()
            }
            NodeKind::Slider {
                label,
                value,
                min,
                max,
                step,
                on_change,
                format,
            } => self.slider(
                SliderSpec {
                    label: label.clone(),
                    value: *value,
                    min: *min,
                    max: *max,
                    step: *step,
                    handler: *on_change,
                    shown: format.format(*value),
                },
                key,
                theme,
                cx,
            ),
            NodeKind::Link { label, url } => {
                let url = url.clone();
                div()
                    .id(self.element_id(path))
                    .text_size(crate::typography::ui_rems(14.0))
                    .line_height(crate::typography::ui_rems(22.0))
                    .text_color(theme.accent)
                    .cursor_pointer()
                    .hover(|s| s.underline())
                    .role(gpui::Role::Link)
                    .child(label.clone())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.open_link(&url, window, cx);
                    }))
                    .into_any_element()
            }
        }
    }

    fn stack(
        &mut self,
        stack: &zeron_visual::Stack,
        path: &str,
        theme: &Theme,
        palette: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let horizontal = stack.axis == zeron_visual::Axis::Horizontal;
        let mut el = div().flex().gap(px(stack.gap));
        el = if horizontal {
            el.flex_row().when(stack.wrap, |el| el.flex_wrap())
        } else {
            el.flex_col().w_full()
        };
        el = match stack.align {
            Align::Start if horizontal => el.items_start(),
            Align::Start => el,
            Align::Center => el.items_center(),
            Align::End => el.items_end(),
        };
        el = match stack.justify {
            Justify::Start => el,
            Justify::Center => el.justify_center(),
            Justify::End => el.justify_end(),
            Justify::Between => el.justify_between(),
        };
        if horizontal {
            el = el.w_full();
        }
        for (ix, child) in stack.children.iter().enumerate() {
            let child_path = format!("{path}.{ix}");
            let rendered = self.node(child, &child_path, theme, palette, window, cx);
            // Nodes with no natural width (a slider's track, a plot) fill a
            // row's spare width unless the script sized them.
            let fluid = matches!(
                child.kind,
                NodeKind::Slider { .. }
                    | NodeKind::Chart(_)
                    | NodeKind::Canvas(_)
                    | NodeKind::Scene(_)
                    | NodeKind::Table(_)
                    | NodeKind::Rule
            );
            let mut wrapper = div().min_w_0();
            wrapper = match (child.width, child.grow) {
                (Some(width), _) => wrapper.flex_none().w(px(width)),
                (None, true) => wrapper.flex_1(),
                (None, false) if horizontal && fluid => wrapper.flex_1(),
                (None, false) if horizontal => wrapper.flex_shrink(1.0),
                (None, false) => wrapper,
            };
            el = el.child(wrapper.child(rendered));
        }
        if stack.on_click.is_none() && !stack.hover {
            return el.into_any_element();
        }
        let hover = theme.glass_hover();
        let mut el = el
            .id(self.element_id(path))
            .rounded(px(6.0))
            .when(stack.hover || stack.on_click.is_some(), |el| {
                el.hover(move |s| s.bg(hover))
            });
        if let Some(handler) = stack.on_click {
            el = el
                .cursor_pointer()
                .role(gpui::Role::Button)
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    this.dispatch(handler, HandlerArg::None, cx);
                }));
        }
        el.into_any_element()
    }

    /// The markdown table's frameless treatment: a bold header and hairlines
    /// between rows, no outer box or fills.
    fn table(
        &mut self,
        table: &zeron_visual::TableNode,
        path: &str,
        theme: &Theme,
        palette: &Palette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        use crate::markdown::render::{TABLE_CELL_PADDING, TABLE_HEADER_WEIGHT, table_hairline};
        let columns = table
            .columns
            .len()
            .max(table.rows.iter().map(Vec::len).max().unwrap_or(0));
        let cell = |align: Align, width: Option<f32>| {
            let cell = div()
                .min_w_0()
                .px(px(TABLE_CELL_PADDING))
                .py(px(8.0))
                .flex()
                .flex_row();
            let cell = match width {
                Some(width) => cell.flex_none().w(px(width)),
                None => cell.flex_1().flex_basis(px(0.0)),
            };
            match align {
                Align::Start => cell,
                Align::Center => cell.justify_center(),
                Align::End => cell.justify_end(),
            }
        };
        let column = |c: usize| table.columns.get(c).cloned().unwrap_or_default();
        let mut el = div().flex().flex_col().w_full();
        if table.columns.iter().any(|c| !c.label.is_empty()) {
            let mut header = div().flex().flex_row();
            for c in 0..columns {
                let column = column(c);
                header = header.child(
                    cell(column.align, column.width).child(
                        div()
                            .text_size(crate::typography::ui_rems(14.0))
                            .line_height(crate::typography::ui_rems(22.0))
                            .font_weight(TABLE_HEADER_WEIGHT)
                            .text_color(theme.text)
                            .child(column.label.clone()),
                    ),
                );
            }
            el = el.child(header);
        }
        let hover = theme.glass_hover();
        for (r, row) in table.rows.iter().enumerate() {
            if r > 0 || !table.columns.is_empty() {
                el = el.child(div().flex_none().h(px(1.0)).w_full().bg(table_hairline()));
            }
            let mut row_el = div()
                .id(self.element_id(&format!("{path}.r{r}")))
                .flex()
                .flex_row()
                .when(table.hover, |el| el.hover(move |s| s.bg(hover)));
            for c in 0..columns {
                let column = column(c);
                let content = match row.get(c) {
                    Some(node) => {
                        let child_path = format!("{path}.{r}.{c}");
                        self.node(node, &child_path, theme, palette, window, cx)
                    }
                    None => gpui::Empty.into_any_element(),
                };
                row_el = row_el.child(cell(column.align, column.width).child(content));
            }
            el = el.child(row_el);
        }
        el.into_any_element()
    }
}

fn text_element(text: &zeron_visual::Text, theme: &Theme, palette: &Palette) -> AnyElement {
    let (size, line_height) = rems(text.size);
    let base_color = text.color.map(|c| palette.color(c)).unwrap_or(theme.text);
    let mut content = String::new();
    let mut runs = Vec::new();
    for span in &text.spans {
        if span.text.is_empty() {
            continue;
        }
        content.push_str(&span.text);
        runs.push(TextRun {
            len: span.text.len(),
            font: ui_font(
                theme,
                text.mono || span.mono,
                weight(span.weight.unwrap_or(text.weight)),
                text.italic || span.italic,
            ),
            color: span.color.map(|c| palette.color(c)).unwrap_or(base_color),
            background_color: None,
            underline: None,
            strikethrough: None,
        });
    }
    let el = div()
        .min_w_0()
        .text_size(size)
        .line_height(line_height)
        .text_color(base_color)
        .when(text.truncate, |el| el.truncate());
    let el = match text.align {
        Align::Start => el,
        Align::Center => el.text_center(),
        Align::End => el.text_right(),
    };
    el.child(StyledText::new(content).with_runs(runs))
        .into_any_element()
}

/// A headline figure over a short label: plain type, no tile.
fn stat(
    value: &str,
    label: &str,
    delta: Option<&str>,
    tone: Option<zeron_visual::Color>,
    theme: &Theme,
    palette: &Palette,
) -> AnyElement {
    let (size, line_height) = rems(zeron_visual::TextSize::Xxl);
    // A tone marks the change, not the figure; a figure with no change
    // takes the tone itself.
    let tone = tone.map(|c| palette.color(c));
    let has_delta = delta.is_some_and(|d| !d.is_empty());
    let value_color = tone.filter(|_| !has_delta).unwrap_or(theme.text);
    div()
        .flex()
        .flex_col()
        .gap(px(2.0))
        .child(
            div()
                .flex()
                .flex_row()
                .items_baseline()
                .gap(px(6.0))
                .child(
                    div()
                        .text_size(size)
                        .line_height(line_height)
                        .text_color(value_color)
                        .child(StyledText::new(value.to_owned()).with_runs(vec![TextRun {
                            len: value.len(),
                            font: ui_font(theme, false, FontWeight::SEMIBOLD, false),
                            color: value_color,
                            background_color: None,
                            underline: None,
                            strikethrough: None,
                        }])),
                )
                .when_some(delta.filter(|d| !d.is_empty()), |el, delta| {
                    el.child(
                        div()
                            .text_size(crate::typography::ui_rems(12.0))
                            .line_height(crate::typography::ui_rems(18.0))
                            .text_color(tone.unwrap_or(theme.text_muted))
                            .child(delta.to_owned()),
                    )
                }),
        )
        .when(!label.is_empty(), |el| {
            el.child(
                div()
                    .text_size(crate::typography::ui_rems(12.0))
                    .line_height(crate::typography::ui_rems(18.0))
                    .text_color(theme.text_muted)
                    .child(label.to_owned()),
            )
        })
        .into_any_element()
}

struct SliderSpec {
    label: Option<String>,
    value: f64,
    min: f64,
    max: f64,
    step: f64,
    handler: HandlerId,
    shown: String,
}

impl SliderSpec {
    /// The value under `x` on a track painted inset by half a thumb.
    fn value_at(&self, x: Pixels, bounds: Bounds<Pixels>) -> f64 {
        let inset = px(SLIDER_THUMB / 2.0);
        let width = (bounds.size.width - inset * 2.0).max(px(1.0));
        let t = f64::from(((x - bounds.origin.x - inset) / width).clamp(0.0, 1.0));
        let mut value = self.min + (self.max - self.min) * t;
        if self.step > 0.0 {
            value = self.min + ((value - self.min) / self.step).round() * self.step;
        }
        value.clamp(self.min, self.max)
    }
}

const SLIDER_TRACK: f32 = 4.0;
const SLIDER_THUMB: f32 = 14.0;

impl VisualFrame {
    fn slider(
        &mut self,
        spec: SliderSpec,
        key: SharedString,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let fraction = ((spec.value - spec.min) / (spec.max - spec.min)).clamp(0.0, 1.0) as f32;
        let spec = Rc::new(spec);
        let (track, fill, thumb) = (theme.border, theme.accent, theme.text);
        let press = {
            let (spec, key) = (spec.clone(), key.clone());
            cx.listener(move |this, event: &gpui::MouseDownEvent, window, cx| {
                cx.stop_propagation();
                this.local.borrow_mut().pressed = Some(key.clone());
                let bounds = this.slider_bounds(&key, window);
                if let Some(bounds) = bounds {
                    let value = spec.value_at(event.position.x, bounds);
                    this.dispatch(spec.handler, HandlerArg::Number(value), cx);
                }
            })
        };
        let drag_key = key.clone();
        let drag = {
            let (spec, key) = (spec.clone(), key.clone());
            cx.listener(
                move |this, event: &gpui::DragMoveEvent<VisualDrag>, _, cx| {
                    if event.drag(cx).0 != key {
                        return;
                    }
                    let value = spec.value_at(event.event.position.x, event.bounds);
                    if (value - spec.value).abs() > f64::EPSILON {
                        this.dispatch(spec.handler, HandlerArg::Number(value), cx);
                    }
                },
            )
        };
        let local = self.local.clone();
        let bounds_key = key.clone();
        div()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .w_full()
            .child(
                div()
                    .flex()
                    .flex_row()
                    .justify_between()
                    .gap(px(12.0))
                    .text_size(crate::typography::ui_rems(12.0))
                    .line_height(crate::typography::ui_rems(18.0))
                    .child(
                        div()
                            .text_color(theme.text_muted)
                            .child(spec.label.clone().unwrap_or_default()),
                    )
                    .child(div().text_color(theme.text).child(spec.shown.clone())),
            )
            .child(
                div()
                    .id(SharedString::from(format!(
                        "{}-slider",
                        self.element_id(&key)
                    )))
                    .h(px(SLIDER_THUMB + 4.0))
                    .w_full()
                    .cursor_pointer()
                    .role(gpui::Role::Slider)
                    .on_mouse_down(MouseButton::Left, press)
                    .on_drag(VisualDrag(drag_key), |_, _, _, cx| cx.new(|_| DragGhost))
                    .on_drag_move(drag)
                    .child(
                        canvas(
                            move |bounds, _, _| {
                                local
                                    .borrow_mut()
                                    .charts
                                    .insert(bounds_key.clone(), ChartGeometry { plot: bounds });
                                bounds
                            },
                            move |_, bounds, window, _| {
                                let mid = bounds.center().y;
                                let left = bounds.origin.x + px(SLIDER_THUMB / 2.0);
                                let width = bounds.size.width - px(SLIDER_THUMB);
                                let at = left + width * fraction;
                                let bar =
                                    |from: Pixels, to: Pixels, color: Hsla, window: &mut Window| {
                                        window.paint_quad(
                                            gpui::fill(
                                                Bounds::from_corners(
                                                    point(from, mid - px(SLIDER_TRACK / 2.0)),
                                                    point(to, mid + px(SLIDER_TRACK / 2.0)),
                                                ),
                                                color,
                                            )
                                            .corner_radii(px(SLIDER_TRACK / 2.0)),
                                        );
                                    };
                                bar(left, left + width, track, window);
                                bar(left, at, fill, window);
                                window.paint_quad(
                                    gpui::fill(
                                        Bounds::centered_at(
                                            point(at, mid),
                                            gpui::size(px(SLIDER_THUMB), px(SLIDER_THUMB)),
                                        ),
                                        thumb,
                                    )
                                    .corner_radii(px(SLIDER_THUMB / 2.0)),
                                );
                            },
                        )
                        .size_full(),
                    ),
            )
            .into_any_element()
    }

    fn slider_bounds(&self, key: &SharedString, _: &Window) -> Option<Bounds<Pixels>> {
        self.local.borrow().charts.get(key).map(|g| g.plot)
    }

    /// Track the pointer over a drawn node (canvas, chart, scene).
    fn pointer_tracking(
        &self,
        el: gpui::Stateful<gpui::Div>,
        key: &SharedString,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let (move_key, hover_key) = (key.clone(), key.clone());
        el.on_mouse_move(cx.listener(move |this, event: &MouseMoveEvent, _, cx| {
            this.local
                .borrow_mut()
                .pointers
                .insert(move_key.clone(), event.position);
            cx.notify();
        }))
        .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
            if !*hovered {
                this.local.borrow_mut().pointers.remove(&hover_key);
                cx.notify();
            }
        }))
    }

    fn canvas(
        &mut self,
        node: &zeron_visual::Canvas,
        key: SharedString,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(runtime) = self.runtime.clone() else {
            return gpui::Empty.into_any_element();
        };
        let (local, generation, time) = (self.local.clone(), self.generation, self.time());
        let (draw, animate) = (node.draw, node.animate);
        let paint_palette = Rc::new(Palette::new(Theme::of(cx)));
        let cache_key = key.clone();
        let mut el = div()
            .id(self.element_id(&format!("{key}-canvas")))
            .w_full()
            .h(px(node.height));
        el = self.pointer_tracking(el, &key, cx);
        let (down_key, up_key) = (key.clone(), key.clone());
        el = el
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _: &gpui::MouseDownEvent, _, cx| {
                    this.local.borrow_mut().pressed = Some(down_key.clone());
                    cx.notify();
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |this, _: &gpui::MouseUpEvent, _, cx| {
                    let mut local = this.local.borrow_mut();
                    if local.pressed.as_ref() == Some(&up_key) {
                        local.pressed = None;
                    }
                    drop(local);
                    cx.notify();
                }),
            );
        if let Some(handler) = node.on_click {
            el = el.cursor_pointer().on_click(cx.listener(
                move |this, event: &gpui::ClickEvent, _, cx| {
                    let origin = this
                        .local
                        .borrow()
                        .charts
                        .get(&cache_key)
                        .map(|g| g.plot.origin)
                        .unwrap_or_default();
                    let at = event.position() - origin;
                    this.dispatch(handler, HandlerArg::Point(at.x.into(), at.y.into()), cx);
                },
            ));
        }
        let geometry_key = key.clone();
        el.child(
            canvas(
                move |bounds, window, _| {
                    let mut state = local.borrow_mut();
                    state
                        .charts
                        .insert(geometry_key.clone(), ChartGeometry { plot: bounds });
                    let size = (f32::from(bounds.size.width), f32::from(bounds.size.height));
                    let pointer = state.pointers.get(&key).map(|p| {
                        let at = *p - bounds.origin;
                        (
                            f32::from(at.x),
                            f32::from(at.y),
                            state.pressed.as_ref() == Some(&key),
                        )
                    });
                    let fresh = state.canvases.get(&key).filter(|cached| {
                        !animate
                            && cached.generation == generation
                            && cached.size == size
                            && cached.pointer == pointer
                    });
                    if let Some(cached) = fresh {
                        return Some(cached.commands.clone());
                    }
                    drop(state);
                    if animate {
                        window.request_animation_frame();
                    }
                    let drawn = runtime.borrow_mut().draw_canvas(
                        draw,
                        size.0,
                        size.1,
                        time,
                        pointer.map(|(x, y, down)| zeron_visual::Pointer { x, y, down }),
                    );
                    let mut state = local.borrow_mut();
                    match drawn {
                        Ok(commands) => {
                            let commands = Rc::new(commands);
                            state.canvases.insert(
                                key.clone(),
                                CanvasCache {
                                    generation,
                                    size,
                                    pointer,
                                    commands: commands.clone(),
                                },
                            );
                            state.draw_error = None;
                            Some(commands)
                        }
                        Err(error) => {
                            state.draw_error = Some(error.message);
                            None
                        }
                    }
                },
                move |bounds, commands, window, cx| {
                    if let Some(commands) = commands {
                        paint_commands(&commands, bounds.origin, &paint_palette, window, cx);
                    }
                },
            )
            .size_full(),
        )
        .into_any_element()
    }
}

// ---- painting -------------------------------------------------------------

fn at(origin: Point<Pixels>, [x, y]: [f32; 2]) -> Point<Pixels> {
    point(origin.x + px(x), origin.y + px(y))
}

fn stroke_path(
    points: impl IntoIterator<Item = Point<Pixels>>,
    closed: bool,
    width: f32,
    dash: Option<f32>,
    color: Hsla,
    window: &mut Window,
) {
    let mut builder = PathBuilder::stroke(px(width));
    if let Some(dash) = dash {
        builder = builder.dash_array(&[px(dash), px(dash)]);
    }
    let mut points = points.into_iter();
    let Some(first) = points.next() else {
        return;
    };
    builder.move_to(first);
    for p in points {
        builder.line_to(p);
    }
    if closed {
        builder.close();
    }
    if let Ok(path) = builder.build() {
        window.paint_path(path, color);
    }
}

fn fill_path(points: impl IntoIterator<Item = Point<Pixels>>, color: Hsla, window: &mut Window) {
    let mut builder = PathBuilder::fill();
    let mut points = points.into_iter();
    let Some(first) = points.next() else {
        return;
    };
    builder.move_to(first);
    for p in points {
        builder.line_to(p);
    }
    builder.close();
    if let Ok(path) = builder.build() {
        window.paint_path(path, color);
    }
}

/// A convex polygon as a triangle fan, skipping tessellation (3D faces).
fn fill_convex(points: &[Point<Pixels>], color: Hsla, window: &mut Window) {
    if points.len() < 3 {
        return;
    }
    let mut path = gpui::Path::new(points[0]);
    for p in &points[1..] {
        path.line_to(*p);
    }
    window.paint_path(path, color);
}

#[allow(clippy::too_many_arguments)]
fn paint_text(
    text: &str,
    at: Point<Pixels>,
    size: zeron_visual::TextSize,
    w: FontWeight,
    mono: bool,
    color: Hsla,
    align: Align,
    baseline: Align,
    window: &mut Window,
    cx: &mut App,
) -> Pixels {
    let theme = Theme::of(cx);
    let (font_size, line_height) = text_px(size, window);
    let run = TextRun {
        len: text.len(),
        font: ui_font(theme, mono, w, false),
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let line = window.text_system().shape_line(
        SharedString::from(text.to_owned()),
        font_size,
        &[run],
        None,
    );
    let width = line.width();
    let x = match align {
        Align::Start => at.x,
        Align::Center => at.x - width / 2.0,
        Align::End => at.x - width,
    };
    let y = match baseline {
        Align::Start => at.y,
        Align::Center => at.y - line_height / 2.0,
        Align::End => at.y - line_height,
    };
    let _ = line.paint(point(x, y), line_height, TextAlign::Left, None, window, cx);
    width
}

fn paint_commands(
    commands: &[DrawCmd],
    origin: Point<Pixels>,
    palette: &Palette,
    window: &mut Window,
    cx: &mut App,
) {
    for command in commands {
        match command {
            DrawCmd::Stroke {
                points,
                closed,
                color,
                width,
                dash,
            } => stroke_path(
                points.iter().map(|p| at(origin, *p)),
                *closed,
                *width,
                *dash,
                palette.color(*color),
                window,
            ),
            DrawCmd::Fill { points, color } => fill_path(
                points.iter().map(|p| at(origin, *p)),
                palette.color(*color),
                window,
            ),
            DrawCmd::Text(text) => {
                paint_text(
                    &text.text,
                    at(origin, [text.x, text.y]),
                    text.size,
                    weight(text.weight),
                    text.mono,
                    palette.color(text.color),
                    text.align,
                    text.baseline,
                    window,
                    cx,
                );
            }
        }
    }
}

// ---- 3D -------------------------------------------------------------------

impl VisualFrame {
    /// Drag to orbit a scene's or shader's camera, picking up from where a
    /// turntable `spin` has turned it.
    fn orbit_handlers(
        &self,
        mut el: gpui::Stateful<gpui::Div>,
        key: &SharedString,
        spin: f32,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let (down_key, drag_key, move_key, up_key, out_key) = (
            key.clone(),
            key.clone(),
            key.clone(),
            key.clone(),
            key.clone(),
        );
        let release = move |this: &mut VisualFrame, key: &SharedString| {
            if let Some(orbit) = this.local.borrow_mut().cameras.get_mut(key)
                && orbit.drag.take().is_some()
            {
                orbit.spin_epoch = Instant::now();
            }
        };
        el = el
            .cursor_grab()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    if let Some(orbit) = this.local.borrow_mut().cameras.get_mut(&down_key) {
                        orbit.yaw += spin * orbit.spin_epoch.elapsed().as_secs_f32();
                        orbit.drag = Some((event.position, orbit.yaw, orbit.pitch));
                    }
                    cx.notify();
                }),
            )
            .on_drag(VisualDrag(drag_key), |_, _, _, cx| cx.new(|_| DragGhost))
            .on_drag_move(cx.listener(
                move |this, event: &gpui::DragMoveEvent<VisualDrag>, _, cx| {
                    if event.drag(cx).0 != move_key {
                        return;
                    }
                    if let Some(orbit) = this.local.borrow_mut().cameras.get_mut(&move_key)
                        && let Some((start, yaw, pitch)) = orbit.drag
                    {
                        let delta = event.event.position - start;
                        orbit.yaw = yaw - f32::from(delta.x) * 0.01;
                        orbit.pitch = (pitch + f32::from(delta.y) * 0.01).clamp(-1.5, 1.5);
                    }
                    cx.notify();
                },
            ))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |this, _: &gpui::MouseUpEvent, _, cx| {
                    release(this, &up_key);
                    cx.notify();
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(move |this, _: &gpui::MouseUpEvent, _, cx| {
                    release(this, &out_key);
                    cx.notify();
                }),
            );
        el
    }

    /// A `ui.shader`: the script's GLSL run on the GPU every frame it
    /// animates, painted as an image like any other.
    fn shader(
        &mut self,
        node: &zeron_visual::ShaderNode,
        key: SharedString,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let spin = if cx.reduce_motion() { 0.0 } else { node.spin };
        let animate = node.animate && !cx.reduce_motion();
        let orbit = *self
            .local
            .borrow_mut()
            .cameras
            .entry(key.clone())
            .or_insert(Orbit {
                yaw: node.yaw,
                pitch: node.pitch,
                spin_epoch: Instant::now(),
                drag: None,
            });
        let yaw = match orbit.drag {
            Some(_) => orbit.yaw,
            None => orbit.yaw + spin * orbit.spin_epoch.elapsed().as_secs_f32(),
        };
        let moving = animate || (spin != 0.0 && orbit.drag.is_none());
        let mut el = div()
            .id(self.element_id(&format!("{key}-shader")))
            .w_full()
            .h(px(node.height));
        el = self.pointer_tracking(el, &key, cx);
        let (down_key, up_key, out_key) = (key.clone(), key.clone(), key.clone());
        el = el
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                    this.local
                        .borrow_mut()
                        .presses
                        .insert(down_key.clone(), event.position);
                    cx.notify();
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(move |this, _: &gpui::MouseUpEvent, _, cx| {
                    this.local.borrow_mut().presses.remove(&up_key);
                    cx.notify();
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(move |this, _: &gpui::MouseUpEvent, _, cx| {
                    this.local.borrow_mut().presses.remove(&out_key);
                    cx.notify();
                }),
            );
        if node.orbit {
            el = self.orbit_handlers(el, &key, spin, cx);
        }
        let names: Vec<String> = node.inputs.iter().map(|(n, _)| n.clone()).collect();
        let mut inputs = [[0.0f32; 4]; zeron_visual::shader::MAX_INPUTS / 4];
        for (i, (_, value)) in node.inputs.iter().enumerate() {
            inputs[i / 4][i % 4] = *value;
        }
        let assets = self
            .runtime
            .as_ref()
            .map(|runtime| runtime.borrow().assets().clone())
            .unwrap_or_default();
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
        let (code, buffers, blend, bloom, zoom) = (
            node.code.clone(),
            node.buffers.clone(),
            node.blend,
            node.bloom,
            node.zoom,
        );
        // Buffers evolve every frame, so a shader with any keeps running.
        let moving = moving || !buffers.is_empty() && !cx.reduce_motion();
        let (local, time, entity) = (self.local.clone(), self.time(), cx.entity().downgrade());
        el.child(
            canvas(
                move |bounds, window, cx| {
                    if moving {
                        window.request_animation_frame();
                    }
                    let scale = window.scale_factor();
                    let size = (
                        (f32::from(bounds.size.width) * scale).round().max(1.0) as u32,
                        (f32::from(bounds.size.height) * scale).round().max(1.0) as u32,
                    );
                    let mut state = local.borrow_mut();
                    // Shadertoy's mouse: pixels from the bottom-left; xy where
                    // the pointer is, zw where it went down while it's held
                    // (negative otherwise).
                    let device = |p: Point<Pixels>| {
                        let at = p - bounds.origin;
                        [
                            f32::from(at.x) * scale,
                            size.1 as f32 - f32::from(at.y) * scale,
                        ]
                    };
                    let mouse = match (state.presses.get(&key), state.pointers.get(&key)) {
                        (Some(start), Some(now)) => {
                            let ([x, y], [sx, sy]) = (device(*now), device(*start));
                            [x, y, sx, sy]
                        }
                        (None, Some(now)) => {
                            let [x, y] = device(*now);
                            [x, y, -1.0, -1.0]
                        }
                        _ => [0.0, 0.0, -1.0, -1.0],
                    };
                    let entry = state.shaders.entry(key.clone()).or_default();
                    // A still shader keeps the time it first showed.
                    let shown_time = if animate {
                        time
                    } else {
                        entry
                            .shown
                            .as_ref()
                            .map_or(0.0, |(u, _)| f64::from(u.time[0]))
                    };
                    let delta = if entry.frame == 0 {
                        0.0
                    } else {
                        (time - entry.last_time).max(0.0)
                    };
                    let uniforms = zeron_visual::shader::ShaderUniforms {
                        resolution: [size.0 as f32, size.1 as f32, scale, 0.0],
                        mouse,
                        camera: [yaw, orbit.pitch, zoom, 0.0],
                        time: [shown_time as f32, delta as f32, entry.frame as f32, 0.0],
                        inputs,
                        channels: Default::default(),
                    };
                    let unchanged = buffers.is_empty()
                        && entry.shown.as_ref().is_some_and(|(p, _)| {
                            p.resolution == uniforms.resolution
                                && p.mouse == uniforms.mouse
                                && p.camera == uniforms.camera
                                && p.inputs == uniforms.inputs
                                && (!animate || p.time[0] == uniforms.time[0])
                        });
                    let shown = entry.shown.as_ref().map(|(_, image)| image.clone());
                    // One frame in flight at a time; when it lands the view
                    // paints again and starts the next if anything moved.
                    if !unchanged && !entry.rendering {
                        entry.rendering = true;
                        entry.frame += 1;
                        entry.last_time = time;
                        let mut gpu_state = entry.gpu.take();
                        let (code, buffers, names, textures) = (
                            code.clone(),
                            buffers.clone(),
                            names.clone(),
                            textures.clone(),
                        );
                        let render = cx.background_executor().spawn(async move {
                            let rendered = zeron_visual_gpu::render_shader(
                                &zeron_visual_gpu::ShaderSpec {
                                    image: &code,
                                    buffers: &buffers,
                                    inputs: &names,
                                    blend,
                                    bloom,
                                    textures: &textures,
                                },
                                &uniforms,
                                size.0,
                                size.1,
                                &mut gpu_state,
                            )
                            .and_then(|frame| {
                                render_image(frame.width, frame.height, frame.bgra)
                                    .ok_or_else(|| "the shader's frame was malformed".to_owned())
                            });
                            (rendered, gpu_state)
                        });
                        let (local, key, entity) = (local.clone(), key.clone(), entity.clone());
                        cx.spawn(async move |cx| {
                            let (rendered, gpu_state) = render.await;
                            cx.update(|cx| {
                                let mut freed = Vec::new();
                                let mut state = local.borrow_mut();
                                match (state.shaders.get_mut(&key), rendered) {
                                    (Some(entry), Ok(image)) => {
                                        entry.rendering = false;
                                        entry.gpu = gpu_state;
                                        freed.extend(
                                            entry
                                                .shown
                                                .replace((uniforms, image))
                                                .map(|(_, old)| old),
                                        );
                                        state.draw_error = None;
                                    }
                                    (Some(entry), Err(error)) => {
                                        entry.rendering = false;
                                        entry.gpu = gpu_state;
                                        state.draw_error = Some(error);
                                    }
                                    // The node left the tree meanwhile.
                                    (None, Ok(image)) => freed.push(image),
                                    (None, Err(_)) => {}
                                }
                                drop(state);
                                free_images(freed, cx);
                                let _ = entity.update(cx, |_, cx| cx.notify());
                            })
                        })
                        .detach();
                    }
                    shown
                },
                move |bounds, image, window, _| {
                    if let Some(image) = image {
                        let _ =
                            window.paint_image(bounds, gpui::Corners::default(), image, 0, false);
                    }
                },
            )
            .size_full(),
        )
        .into_any_element()
    }

    fn scene(
        &mut self,
        node: &zeron_visual::SceneNode,
        key: SharedString,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(runtime) = self.runtime.clone() else {
            return gpui::Empty.into_any_element();
        };
        let spin = if cx.reduce_motion() { 0.0 } else { node.spin };
        let orbit = *self
            .local
            .borrow_mut()
            .cameras
            .entry(key.clone())
            .or_insert(Orbit {
                yaw: node.yaw,
                pitch: node.pitch,
                spin_epoch: Instant::now(),
                drag: None,
            });
        let yaw = match orbit.drag {
            Some(_) => orbit.yaw,
            None => orbit.yaw + spin * orbit.spin_epoch.elapsed().as_secs_f32(),
        };
        let camera = zeron_visual::Camera {
            yaw,
            pitch: orbit.pitch,
            zoom: node.zoom,
        };
        let moving = (spin != 0.0 && orbit.drag.is_none()) || node.animate;
        let dragging = orbit.drag.is_some();
        let (local, generation, time) = (self.local.clone(), self.generation, self.time());
        let entity = cx.entity().downgrade();
        let (draw, animate) = (node.draw, node.animate);
        let paint_palette = Rc::new(Palette::new(Theme::of(cx)));
        let mut el = div()
            .id(self.element_id(&format!("{key}-scene")))
            .w_full()
            .h(px(node.height));
        el = self.pointer_tracking(el, &key, cx);
        if node.orbit {
            el = self.orbit_handlers(el, &key, spin, cx);
        }
        let pointer_key = key.clone();
        let render_palette = paint_palette.clone();
        el.child(
            canvas(
                move |bounds, window, cx| {
                    let paint_palette = &render_palette;
                    if moving {
                        window.request_animation_frame();
                    }
                    let mut state = local.borrow_mut();
                    let fresh = state
                        .scenes
                        .get(&key)
                        .is_some_and(|c| !animate && c.generation == generation);
                    if !fresh {
                        drop(state);
                        let drawn = runtime.borrow_mut().draw_scene(draw, time);
                        state = local.borrow_mut();
                        match drawn {
                            Ok(scene) => {
                                let scene = Arc::new(scene);
                                // A new drawing keeps the old image up (and the
                                // GPU targets) until its own frame lands.
                                match state.scenes.get_mut(&key) {
                                    Some(entry) => {
                                        entry.generation = generation;
                                        entry.drawing += 1;
                                        entry.scene = scene;
                                    }
                                    None => {
                                        state.scenes.insert(
                                            key.clone(),
                                            SceneCache {
                                                generation,
                                                drawing: 0,
                                                scene,
                                                image: None,
                                                final_cost: None,
                                                gpu: None,
                                                rendering: false,
                                            },
                                        );
                                    }
                                }
                                state.draw_error = None;
                            }
                            Err(error) => {
                                state.draw_error = Some(error.message);
                                return None;
                            }
                        }
                    }
                    // Always at full density. While the camera moves, a scene
                    // too heavy to supersample within a frame drops to one
                    // sample per pixel; at rest it is always supersampled.
                    let pixel_scale = window.scale_factor();
                    let cheap = state
                        .scenes
                        .get(&key)
                        .and_then(|c| c.final_cost)
                        .is_some_and(|cost| cost < std::time::Duration::from_millis(8));
                    // Quality only steers the software renderer; on the GPU
                    // a finished drag mustn't render the same frame again.
                    let quality =
                        if (moving || dragging) && !cheap && !zeron_visual_gpu::available() {
                            zeron_visual::Quality::Draft
                        } else {
                            zeron_visual::Quality::Final
                        };
                    let size = (f32::from(bounds.size.width), f32::from(bounds.size.height));
                    let entry = state.scenes.get_mut(&key)?;
                    let raster_key = RasterKey {
                        yaw: camera.yaw,
                        pitch: camera.pitch,
                        zoom: camera.zoom,
                        size,
                        pixel_scale,
                        quality,
                        palette: palette_key(paint_palette),
                        drawing: entry.drawing,
                    };
                    let shown = entry.image.as_ref().map(|(_, image)| image.clone());
                    // One frame in flight at a time, on a background thread;
                    // when it lands the view paints again.
                    if entry.image.as_ref().map(|(k, _)| *k) != Some(raster_key) && !entry.rendering
                    {
                        entry.rendering = true;
                        let (scene, mut gpu_state) = (entry.scene.clone(), entry.gpu.take());
                        let palette = (**paint_palette).clone();
                        let render = cx.background_executor().spawn(async move {
                            let rgba = move |color| {
                                let rgba = palette.color(color).to_rgb();
                                [rgba.r, rgba.g, rgba.b, rgba.a]
                            };
                            // On the GPU when there is one (materials, shadows,
                            // glass, bloom); the software renderer otherwise.
                            let started = Instant::now();
                            let mut cost = None;
                            let (width, height, bgra, hints, labels) =
                                match zeron_visual_gpu::render_scene(
                                    &scene,
                                    camera,
                                    size.0,
                                    size.1,
                                    pixel_scale,
                                    &rgba,
                                    &mut gpu_state,
                                ) {
                                    Some(frame) => {
                                        let (hints, labels) =
                                            scene.annotations(camera, size.0, size.1);
                                        (frame.width, frame.height, frame.bgra, hints, labels)
                                    }
                                    None => {
                                        let raster = scene.render(
                                            camera,
                                            size.0,
                                            size.1,
                                            pixel_scale,
                                            quality,
                                            &rgba,
                                        );
                                        if quality == zeron_visual::Quality::Final {
                                            cost = Some(started.elapsed());
                                        }
                                        (
                                            raster.width,
                                            raster.height,
                                            raster.bgra,
                                            raster.hints,
                                            raster.labels,
                                        )
                                    }
                                };
                            let image = render_image(width, height, bgra).map(|image| SceneImage {
                                image,
                                hints: Arc::new(hints),
                                labels: Arc::new(labels),
                            });
                            (image, gpu_state, cost)
                        });
                        let (local, key, entity) = (local.clone(), key.clone(), entity.clone());
                        cx.spawn(async move |cx| {
                            let (image, gpu_state, cost) = render.await;
                            cx.update(|cx| {
                                let mut freed = Vec::new();
                                let mut state = local.borrow_mut();
                                match state.scenes.get_mut(&key) {
                                    Some(entry) => {
                                        entry.rendering = false;
                                        entry.gpu = gpu_state;
                                        if cost.is_some() {
                                            entry.final_cost = cost;
                                        }
                                        if let Some(image) = image {
                                            freed.extend(
                                                entry
                                                    .image
                                                    .replace((raster_key, image))
                                                    .map(|(_, old)| old.image),
                                            );
                                        }
                                    }
                                    // The node left the tree meanwhile.
                                    None => freed.extend(image.map(|image| image.image)),
                                }
                                drop(state);
                                free_images(freed, cx);
                                let _ = entity.update(cx, |_, cx| cx.notify());
                            })
                        })
                        .detach();
                    }
                    let image = shown?;
                    let hovered = state.pointers.get(&pointer_key).and_then(|p| {
                        let at = *p - bounds.origin;
                        let (x, y) = (f32::from(at.x), f32::from(at.y));
                        image
                            .hints
                            .iter()
                            .map(|([hx, hy], r, text)| ((hx - x).hypot(hy - y), *r, text))
                            .filter(|(d, r, _)| *d <= r + 6.0)
                            .min_by(|a, b| a.0.total_cmp(&b.0))
                            .map(|(_, _, text)| text.clone())
                    });
                    Some((image, hovered))
                },
                move |bounds, rendered, window, cx| {
                    let Some((image, hovered)) = rendered else {
                        return;
                    };
                    let _ = window.paint_image(
                        bounds,
                        gpui::Corners::default(),
                        image.image.clone(),
                        0,
                        false,
                    );
                    for label in image.labels.iter() {
                        paint_text(
                            &label.text,
                            at(bounds.origin, label.at),
                            label.size,
                            FontWeight::NORMAL,
                            false,
                            paint_palette.color(label.color),
                            Align::Center,
                            Align::End,
                            window,
                            cx,
                        );
                    }
                    if let Some(hint) = hovered {
                        paint_text(
                            &hint,
                            bounds.origin,
                            zeron_visual::TextSize::Sm,
                            FontWeight::MEDIUM,
                            false,
                            paint_palette.text,
                            Align::Start,
                            Align::Start,
                            window,
                            cx,
                        );
                    }
                },
            )
            .size_full(),
        )
        .into_any_element()
    }
}

/// Identifies a palette's colors, so a theme change re-renders scenes.
fn palette_key(palette: &Palette) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for color in [
        palette.text,
        palette.muted,
        palette.faint,
        palette.accent,
        palette.on_accent,
        palette.success,
        palette.warning,
        palette.danger,
        palette.border,
    ]
    .iter()
    .chain(palette.chart.iter())
    {
        for channel in [color.h, color.s, color.l, color.a] {
            channel.to_bits().hash(&mut hasher);
        }
    }
    hasher.finish()
}

// ---- charts ---------------------------------------------------------------

/// Gaps between the plot and its axis labels, in pixels.
const AXIS_GAP: f32 = 8.0;

struct ChartPlan {
    plot: Bounds<Pixels>,
    /// Value-axis domain and ticks.
    lo: f64,
    hi: f64,
    ticks: Vec<f64>,
    /// Scatter's numeric x domain and ticks.
    x_lo: f64,
    x_hi: f64,
    x_ticks: Vec<f64>,
    label_stride: usize,
    hover: Option<usize>,
    hover_point: Option<(usize, usize)>,
}

fn domain(lo: f64, hi: f64, include_zero: bool) -> (f64, f64, Vec<f64>) {
    let (mut lo, mut hi) = (lo, hi);
    if include_zero {
        lo = lo.min(0.0);
        hi = hi.max(0.0);
    }
    if !lo.is_finite() || !hi.is_finite() {
        return (0.0, 1.0, vec![0.0, 1.0]);
    }
    if (hi - lo).abs() < f64::EPSILON {
        hi = lo + lo.abs().max(1.0);
    }
    let ticks = zeron_visual::nice_ticks(lo, hi, 4);
    let step = if ticks.len() > 1 {
        ticks[1] - ticks[0]
    } else {
        (hi - lo).max(1.0)
    };
    let lo = (lo / step).floor() * step;
    let hi = (hi / step).ceil() * step;
    (lo, hi, zeron_visual::nice_ticks(lo, hi, 4))
}

fn value_range(chart: &zeron_visual::Chart) -> (f64, f64) {
    let (mut lo, mut hi) = (f64::INFINITY, f64::NEG_INFINITY);
    if chart.kind == ChartKind::Scatter {
        for (_, y) in chart.series.iter().flat_map(|s| &s.points) {
            lo = lo.min(*y);
            hi = hi.max(*y);
        }
    } else if chart.stacked {
        let n = chart
            .series
            .iter()
            .map(|s| s.values.len())
            .max()
            .unwrap_or(0);
        for i in 0..n {
            let (mut pos, mut neg) = (0.0, 0.0);
            for s in &chart.series {
                match s.values.get(i).copied().flatten() {
                    Some(v) if v >= 0.0 => pos += v,
                    Some(v) => neg += v,
                    None => {}
                }
            }
            lo = lo.min(neg);
            hi = hi.max(pos);
        }
    } else {
        for v in chart.series.iter().flat_map(|s| s.values.iter().flatten()) {
            lo = lo.min(*v);
            hi = hi.max(*v);
        }
    }
    (chart.y_min.unwrap_or(lo), chart.y_max.unwrap_or(hi))
}

fn categories(chart: &zeron_visual::Chart) -> usize {
    chart
        .series
        .iter()
        .map(|s| s.values.len())
        .max()
        .unwrap_or(0)
        .max(chart.x.len())
}

fn measure(text: &str, size: zeron_visual::TextSize, window: &Window, cx: &App) -> f32 {
    let theme = Theme::of(cx);
    let (font_size, _) = text_px(size, window);
    let run = TextRun {
        len: text.len(),
        font: ui_font(theme, false, FontWeight::NORMAL, false),
        color: theme.text,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    f32::from(
        window
            .text_system()
            .shape_line(SharedString::from(text.to_owned()), font_size, &[run], None)
            .width(),
    )
}

impl VisualFrame {
    fn chart(
        &mut self,
        chart: &zeron_visual::Chart,
        key: SharedString,
        theme: &Theme,
        palette: &Palette,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let chart = Rc::new(chart.clone());
        let n = categories(&chart);
        // The hovered category, from the plot the last paint laid out.
        let (hover, hover_point) = {
            let local = self.local.borrow();
            let geometry = local.charts.get(&key).copied();
            let pointer = local.pointers.get(&key).copied();
            match (geometry, pointer) {
                (Some(g), Some(p)) if g.plot.size.width > px(0.0) => {
                    let along = if chart.horizontal && chart.kind == ChartKind::Bar {
                        (p.y - g.plot.origin.y) / g.plot.size.height
                    } else {
                        (p.x - g.plot.origin.x) / g.plot.size.width
                    };
                    let ix = if chart.kind == ChartKind::Bar || n <= 1 {
                        (along * n as f32).floor()
                    } else {
                        (along * (n - 1) as f32).round()
                    };
                    let hover = (0.0..n as f32).contains(&ix).then_some(ix as usize);
                    (
                        hover.filter(|_| chart.kind != ChartKind::Scatter),
                        (chart.kind == ChartKind::Scatter)
                            .then(|| nearest_point(&chart, g.plot, p))
                            .flatten(),
                    )
                }
                _ => (None, None),
            }
        };

        // Legend and readout: plain type above the plot, the hovered values
        // inline with their series swatches.
        let readout_label = hover.and_then(|i| chart.x.get(i).cloned());
        let mut legend = div()
            .flex()
            .flex_row()
            .flex_wrap()
            .items_center()
            .gap_x(px(14.0))
            .gap_y(px(2.0))
            .text_size(crate::typography::ui_rems(12.0))
            .line_height(crate::typography::ui_rems(18.0))
            .text_color(theme.text_muted)
            .min_h(crate::typography::ui_rems(18.0));
        if let Some(label) = &readout_label {
            legend = legend.child(
                div()
                    .text_color(theme.text)
                    .font_weight(FontWeight::MEDIUM)
                    .child(label.clone()),
            );
        }
        if let Some((s, p)) = hover_point {
            let (x, y) = chart.series[s].points[p];
            legend = legend.child(div().text_color(theme.text).child(format!(
                "{} · {}",
                chart.x_format.format(x),
                chart.format.format(y)
            )));
        }
        let show_legend = chart.legend
            && (chart.series.len() > 1 || hover.is_some())
            && chart.series.iter().any(|s| !s.name.is_empty());
        if show_legend || hover.is_some() {
            for series in chart.series.iter() {
                let value = hover.and_then(|i| series.values.get(i).copied().flatten());
                let mut entry = div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(6.0))
                    .child(
                        div()
                            .flex_none()
                            .size(px(8.0))
                            .rounded(px(2.0))
                            .bg(palette.color(series.color)),
                    )
                    .child(series.name.clone());
                if let Some(value) = value {
                    entry = entry.child(
                        div()
                            .text_color(theme.text)
                            .child(chart.format.format(value)),
                    );
                }
                legend = legend.child(entry);
            }
        }

        let local = self.local.clone();
        let paint_palette = Rc::new(Palette::new(theme));
        let (plan_key, paint_chart) = (key.clone(), chart.clone());
        let mut plot = div()
            .id(self.element_id(&format!("{key}-chart")))
            .w_full()
            .h(px(chart.height));
        plot = self.pointer_tracking(plot, &key, cx);
        div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .w_full()
            .child(legend)
            .child(
                plot.child(
                    canvas(
                        move |bounds, window, cx| {
                            let plan = plan_chart(&chart, bounds, hover, hover_point, window, cx);
                            local
                                .borrow_mut()
                                .charts
                                .insert(plan_key.clone(), ChartGeometry { plot: plan.plot });
                            plan
                        },
                        move |_, plan, window, cx| {
                            paint_chart_plan(&paint_chart, &plan, &paint_palette, window, cx)
                        },
                    )
                    .size_full(),
                ),
            )
            .into_any_element()
    }
}

fn nearest_point(
    chart: &zeron_visual::Chart,
    plot: Bounds<Pixels>,
    pointer: Point<Pixels>,
) -> Option<(usize, usize)> {
    let (x_lo, x_hi) = x_extent(chart);
    let (lo, hi) = value_range(chart);
    let (x_lo, x_hi, _) = domain(x_lo, x_hi, false);
    let (lo, hi, _) = domain(lo, hi, false);
    let mut best = None;
    for (s, series) in chart.series.iter().enumerate() {
        for (p, (x, y)) in series.points.iter().enumerate() {
            let sx = plot.origin.x + plot.size.width * ((x - x_lo) / (x_hi - x_lo)) as f32;
            let sy = plot.origin.y + plot.size.height * (1.0 - ((y - lo) / (hi - lo)) as f32);
            let d = f32::from((sx - pointer.x).abs()).hypot(f32::from((sy - pointer.y).abs()));
            if d < 12.0 && best.is_none_or(|(bd, _, _)| d < bd) {
                best = Some((d, s, p));
            }
        }
    }
    best.map(|(_, s, p)| (s, p))
}

fn x_extent(chart: &zeron_visual::Chart) -> (f64, f64) {
    chart
        .series
        .iter()
        .flat_map(|s| &s.points)
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), (x, _)| {
            (lo.min(*x), hi.max(*x))
        })
}

fn plan_chart(
    chart: &zeron_visual::Chart,
    bounds: Bounds<Pixels>,
    hover: Option<usize>,
    hover_point: Option<(usize, usize)>,
    window: &mut Window,
    cx: &mut App,
) -> ChartPlan {
    use zeron_visual::TextSize::Xs;
    let (_, tick_line) = text_px(Xs, window);
    let tick_line = f32::from(tick_line);
    let scatter = chart.kind == ChartKind::Scatter;
    let bars = chart.kind == ChartKind::Bar;
    let horizontal = bars && chart.horizontal;
    let (lo, hi) = value_range(chart);
    let (lo, hi, ticks) = domain(lo, hi, !scatter);
    let (x_lo, x_hi, x_ticks) = if scatter {
        let (a, b) = x_extent(chart);
        domain(a, b, false)
    } else {
        (0.0, 1.0, Vec::new())
    };
    let n = categories(chart).max(1);
    // The left gutter fits the widest value tick, or the category labels on
    // a horizontal bar chart.
    let left = if horizontal {
        chart
            .x
            .iter()
            .map(|l| measure(l, Xs, window, cx))
            .fold(0.0, f32::max)
            .min(f32::from(bounds.size.width) * 0.35)
    } else {
        ticks
            .iter()
            .map(|t| measure(&chart.format.format(*t), Xs, window, cx))
            .fold(0.0, f32::max)
    } + AXIS_GAP;
    let top = tick_line / 2.0;
    let bottom = tick_line + AXIS_GAP / 2.0;
    let right = if horizontal || scatter { 12.0 } else { 4.0 };
    let plot = Bounds::from_corners(
        bounds.origin + point(px(left), px(top)),
        point(
            bounds.origin.x + bounds.size.width - px(right),
            bounds.origin.y + bounds.size.height - px(bottom),
        ),
    );
    // Thin the category labels until they don't collide.
    let label_stride = if horizontal || scatter {
        1
    } else {
        let widest = chart
            .x
            .iter()
            .map(|l| measure(l, Xs, window, cx))
            .fold(0.0, f32::max)
            + 12.0;
        let band = f32::from(plot.size.width) / n as f32;
        ((widest / band.max(1.0)).ceil() as usize).max(1)
    };
    ChartPlan {
        plot,
        lo,
        hi,
        ticks,
        x_lo,
        x_hi,
        x_ticks,
        label_stride,
        hover,
        hover_point,
    }
}

fn paint_chart_plan(
    chart: &zeron_visual::Chart,
    plan: &ChartPlan,
    palette: &Palette,
    window: &mut Window,
    cx: &mut App,
) {
    use zeron_visual::TextSize::Xs;
    let plot = plan.plot;
    if plot.size.width <= px(4.0) || plot.size.height <= px(4.0) {
        return;
    }
    let horizontal = chart.kind == ChartKind::Bar && chart.horizontal;
    let n = categories(chart).max(1);
    let (pw, ph) = (f32::from(plot.size.width), f32::from(plot.size.height));
    let (ox, oy) = (f32::from(plot.origin.x), f32::from(plot.origin.y));
    let value_t = |v: f64| ((v - plan.lo) / (plan.hi - plan.lo)) as f32;
    // Value -> screen along the value axis.
    let vy = |v: f64| oy + ph * (1.0 - value_t(v));
    let vx = |v: f64| ox + pw * value_t(v);
    let grid = palette.border;
    let muted = palette.muted;
    let hover_wash = Theme::of(cx).glass_hover();
    let p = |x: f32, y: f32| point(px(x), px(y));

    // Gridlines and value ticks.
    for tick in &plan.ticks {
        let label = chart.format.format(*tick);
        if horizontal {
            let x = vx(*tick);
            stroke_path([p(x, oy), p(x, oy + ph)], false, 1.0, None, grid, window);
            paint_text(
                &label,
                p(x, oy + ph + AXIS_GAP / 2.0),
                Xs,
                FontWeight::NORMAL,
                false,
                muted,
                Align::Center,
                Align::Start,
                window,
                cx,
            );
        } else {
            let y = vy(*tick);
            stroke_path([p(ox, y), p(ox + pw, y)], false, 1.0, None, grid, window);
            paint_text(
                &label,
                p(ox - AXIS_GAP, y),
                Xs,
                FontWeight::NORMAL,
                false,
                muted,
                Align::End,
                Align::Center,
                window,
                cx,
            );
        }
    }

    match chart.kind {
        ChartKind::Bar => {
            let band = if horizontal { ph } else { pw } / n as f32;
            if let Some(i) = plan.hover {
                let start = if horizontal { oy } else { ox } + band * i as f32;
                let rect = if horizontal {
                    Bounds::from_corners(p(ox, start), p(ox + pw, start + band))
                } else {
                    Bounds::from_corners(p(start, oy), p(start + band, oy + ph))
                };
                window.paint_quad(gpui::fill(rect, hover_wash).corner_radii(px(4.0)));
            }
            let groups = if chart.stacked {
                1
            } else {
                chart.series.len().max(1)
            };
            let inner = band * 0.72;
            let bar = (inner / groups as f32).max(1.0);
            for i in 0..n {
                let start = band * i as f32 + (band - inner) / 2.0;
                let (mut pos, mut neg) = (0.0f64, 0.0f64);
                for (s, series) in chart.series.iter().enumerate() {
                    let Some(v) = series.values.get(i).copied().flatten() else {
                        continue;
                    };
                    let (from, to) = if chart.stacked {
                        if v >= 0.0 {
                            pos += v;
                            (pos - v, pos)
                        } else {
                            neg += v;
                            (neg - v, neg)
                        }
                    } else {
                        (0.0f64.clamp(plan.lo, plan.hi), v)
                    };
                    let offset = if chart.stacked { 0.0 } else { bar * s as f32 };
                    let gap = if groups > 1 && !chart.stacked {
                        1.0
                    } else {
                        0.0
                    };
                    let rect = if horizontal {
                        let y = oy + start + offset;
                        let (a, b) = (vx(from), vx(to));
                        Bounds::from_corners(p(a.min(b), y), p(a.max(b), y + bar - gap))
                    } else {
                        let x = ox + start + offset;
                        let (a, b) = (vy(from), vy(to));
                        Bounds::from_corners(p(x, a.min(b)), p(x + bar - gap, a.max(b)))
                    };
                    let radius = if chart.stacked { 0.0 } else { 2.0 };
                    window.paint_quad(
                        gpui::fill(rect, palette.color(series.color)).corner_radii(px(radius)),
                    );
                }
            }
            // Category labels.
            for (i, label) in chart.x.iter().enumerate().take(n) {
                if i % plan.label_stride != 0 {
                    continue;
                }
                let center = band * (i as f32 + 0.5);
                if horizontal {
                    paint_text(
                        label,
                        p(ox - AXIS_GAP, oy + center),
                        Xs,
                        FontWeight::NORMAL,
                        false,
                        muted,
                        Align::End,
                        Align::Center,
                        window,
                        cx,
                    );
                } else {
                    paint_text(
                        label,
                        p(ox + center, oy + ph + AXIS_GAP / 2.0),
                        Xs,
                        FontWeight::NORMAL,
                        false,
                        muted,
                        Align::Center,
                        Align::Start,
                        window,
                        cx,
                    );
                }
            }
        }
        ChartKind::Line | ChartKind::Area => {
            let x_at = |i: usize| {
                if n <= 1 {
                    ox + pw / 2.0
                } else {
                    ox + pw * i as f32 / (n - 1) as f32
                }
            };
            if let Some(i) = plan.hover {
                let x = x_at(i);
                stroke_path(
                    [p(x, oy), p(x, oy + ph)],
                    false,
                    1.0,
                    None,
                    Theme::of(cx).border_strong,
                    window,
                );
            }
            let base = vy(0.0f64.clamp(plan.lo, plan.hi));
            for series in &chart.series {
                let color = palette.color(series.color);
                // Gaps split a series into runs.
                let mut runs: Vec<Vec<Point<Pixels>>> = vec![Vec::new()];
                for (i, v) in series.values.iter().enumerate() {
                    match v {
                        Some(v) => runs.last_mut().unwrap().push(p(x_at(i), vy(*v))),
                        None => runs.push(Vec::new()),
                    }
                }
                for run in runs.iter().filter(|r| !r.is_empty()) {
                    if chart.kind == ChartKind::Area && run.len() > 1 {
                        let mut area = run.clone();
                        area.push(point(run.last().unwrap().x, px(base)));
                        area.push(point(run[0].x, px(base)));
                        fill_path(
                            area,
                            Hsla {
                                a: color.a * 0.22,
                                ..color
                            },
                            window,
                        );
                    }
                    if run.len() == 1 {
                        fill_convex(&dot(run[0], 2.5), color, window);
                    } else {
                        stroke_path(run.iter().copied(), false, 1.75, None, color, window);
                    }
                }
                if let Some(i) = plan.hover
                    && let Some(Some(v)) = series.values.get(i)
                {
                    fill_convex(&dot(p(x_at(i), vy(*v)), 3.5), color, window);
                }
            }
            for (i, label) in chart.x.iter().enumerate().take(n) {
                if i % plan.label_stride != 0 {
                    continue;
                }
                let align = if n > 1 && i == 0 {
                    Align::Start
                } else if n > 1 && i == n - 1 {
                    Align::End
                } else {
                    Align::Center
                };
                paint_text(
                    label,
                    p(x_at(i), oy + ph + AXIS_GAP / 2.0),
                    Xs,
                    FontWeight::NORMAL,
                    false,
                    muted,
                    align,
                    Align::Start,
                    window,
                    cx,
                );
            }
        }
        ChartKind::Scatter => {
            let sx = |x: f64| ox + pw * ((x - plan.x_lo) / (plan.x_hi - plan.x_lo)) as f32;
            for tick in &plan.x_ticks {
                let x = sx(*tick);
                stroke_path([p(x, oy), p(x, oy + ph)], false, 1.0, None, grid, window);
                paint_text(
                    &chart.x_format.format(*tick),
                    p(x, oy + ph + AXIS_GAP / 2.0),
                    Xs,
                    FontWeight::NORMAL,
                    false,
                    muted,
                    Align::Center,
                    Align::Start,
                    window,
                    cx,
                );
            }
            for (s, series) in chart.series.iter().enumerate() {
                let color = palette.color(series.color);
                for (ix, (x, y)) in series.points.iter().enumerate() {
                    let r = if plan.hover_point == Some((s, ix)) {
                        5.0
                    } else {
                        3.0
                    };
                    fill_convex(&dot(p(sx(*x), vy(*y)), r), color, window);
                }
            }
        }
    }
}

fn dot(center: Point<Pixels>, radius: f32) -> Vec<Point<Pixels>> {
    (0..14)
        .map(|i| {
            let a = std::f32::consts::TAU * i as f32 / 14.0;
            center + point(px(radius * a.cos()), px(radius * a.sin()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chart_series_drops_the_fixed_hue_nearest_the_accent() {
        let theme = Theme::dark();
        let series = chart_series(&theme);
        assert_eq!(series[0], theme.accent_strong);
        // Six distinct colors.
        for (i, a) in series.iter().enumerate() {
            for b in &series[i + 1..] {
                assert_ne!(a, b);
            }
        }
    }

    #[test]
    fn every_token_resolves_against_the_theme() {
        for theme in [Theme::dark(), Theme::light()] {
            let palette = Palette::new(&theme);
            for name in zeron_visual::TOKEN_NAMES {
                let token = Token::parse(name).unwrap();
                let color = palette.token(token);
                assert!(color.a > 0.0, "{name}");
            }
            assert_eq!(palette.token(Token::Text), theme.text);
            let half = palette.color(zeron_visual::Color::token(Token::Accent).with_alpha(0.5));
            assert!((half.a - theme.accent.a * 0.5).abs() < 1e-6);
        }
    }

    #[test]
    fn domains_include_zero_for_bars_and_land_on_ticks() {
        let (lo, hi, ticks) = domain(12.0, 87.0, true);
        assert_eq!(lo, 0.0);
        assert_eq!(hi, 100.0);
        assert_eq!(ticks.first(), Some(&0.0));
        assert_eq!(ticks.last(), Some(&100.0));
        let (lo, hi, _) = domain(-3.0, 7.0, false);
        assert!(lo <= -3.0 && hi >= 7.0);
    }
}
