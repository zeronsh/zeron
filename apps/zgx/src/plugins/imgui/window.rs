//! A plugin's native windows: what a request becomes, and what the host
//! keeps of one while it is open.
//!
//! [`WindowSpec`] is the validated form of `zeron.imgui.open_window`: the
//! title, size and options a plugin asked for, with every field checked
//! before any window exists (the refusal names the field and the window id,
//! which is what a plugin author sees). [`WindowState`] is the bookkeeping
//! the controls change, kept apart from the native window so it can be
//! exercised without one: hiding does not close, closing happens once, and
//! a closed window's retained handle refuses every control instead of
//! acting on whatever the id means now.
//!
//! [`PluginWindow`] is the pair the host holds per `(plugin, window id)`:
//! one zgx [`WindowHandle`], one full-bleed GPU view, one Dear ImGui
//! context. The window belongs to the plugin that opened it: closing it
//! never unloads the plugin, and the plugin's own teardown closes its
//! windows (the registry's `close_all`), which is the other direction.

use std::cell::{Cell, RefCell};
use std::fmt;
use std::rc::Rc;

use zgx::compose::gpu_view;
use zgx::gpu_view::GpuView;
use zgx::reactive::Signal;
use zgx::style::Styled;
use zgx_desktop::{Tabbing, Titlebar, WindowBounds, WindowFactory, WindowHandle, WindowOptions};
use zgx_gpu::viewport::{Viewport, ViewportFrame};
use zgx_script::Value;

use super::frame::{FrameCalls, FrameReport, FrameState, FrameWindow};
use super::render::ImguiViewport;
use super::runtime::FrameSpec;
use super::runtime::WindowContext as ImGuiContext;
use dear_imgui_rs::Ui;

/// What a plugin hears when one of its windows closes: the window id it
/// named, once per window.
pub type CloseListener = Rc<dyn Fn(&str)>;

/// The smallest client a plugin window may ask for, in logical points.
pub const MIN_WIDTH: f64 = 120.0;
pub const MIN_HEIGHT: f64 = 80.0;
/// The largest, in logical points. A larger request is clamped, not refused:
/// a plugin asking for a big inspector still gets a usable window on a
/// smaller display.
pub const MAX_WIDTH: f64 = 3840.0;
pub const MAX_HEIGHT: f64 = 2160.0;
/// The most bytes a title may be (UTF-8, as the platform takes it).
pub const MAX_TITLE_BYTES: usize = 200;
/// The most bytes a window id may be.
pub const MAX_ID_BYTES: usize = 64;
/// A window with no size asked for opens at this, in logical points.
pub const DEFAULT_WIDTH: f64 = 720.0;
pub const DEFAULT_HEIGHT: f64 = 480.0;

/// One plugin window's name: unique per plugin, and only meaningful inside
/// it. `(plugin, id)` is the key every map and saved frame uses; the same id
/// in two plugins is two windows.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WindowKey {
    pub plugin: String,
    pub id: String,
}

impl WindowKey {
    pub fn new(plugin: impl Into<String>, id: impl Into<String>) -> Self {
        Self {
            plugin: plugin.into(),
            id: id.into(),
        }
    }

    /// The key as maps and saved-frame names hold it (`plugin/window`).
    pub fn global(&self) -> String {
        format!("{}/{}", self.plugin, self.id)
    }
}

impl fmt::Display for WindowKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}/{}", self.plugin, self.id)
    }
}

/// What `zeron.imgui.open_window` asked for, checked.
#[derive(Clone, Debug, PartialEq)]
pub struct WindowSpec {
    pub id: String,
    pub title: String,
    /// Logical client size.
    pub size: (f64, f64),
    /// Logical position of the window's frame, where the plugin asked for
    /// one (the compositor may place it itself, as Wayland does).
    pub position: Option<(i32, i32)>,
    pub min_size: Option<(f64, f64)>,
    pub max_size: Option<(f64, f64)>,
    pub resizable: bool,
    /// Shown as it opens; false opens hidden (its content laid out and its
    /// first frame drawn) and waits for `show`.
    pub visible: bool,
    /// The title bar style; `Standard` is the platform's own.
    pub titlebar: Titlebar,
    /// The platform's frame and buttons. Off with `titlebar = "standard"`
    /// keeps the title out of the chrome but the window resizable.
    pub decorations: bool,
    /// Where the plugin's own window id remembers its frame. Off by default:
    /// persistence is opt-in, so a requested initial size is the size a
    /// window opens at.
    pub remember: bool,
}

impl WindowSpec {
    /// Parses and checks a plugin's request.
    ///
    /// Every refusal names the field it is about and, when one was given,
    /// the window id: `zeron.imgui.open_window can't use size for window
    /// "inspector": width must be a number`. A refusal means no window is
    /// created at all.
    pub fn parse(plugin: &str, options: &Value) -> Result<Self, String> {
        let id = match options.field("id") {
            Some(Value::Str(id)) if !id.trim().is_empty() => id.to_string(),
            Some(Value::Str(_)) => {
                return Err(format!(
                    "zeron.imgui.open_window needs an id for plugin \"{plugin}\": it can't be empty"
                ));
            }
            Some(_) => {
                return Err(format!(
                    "zeron.imgui.open_window can't use id for plugin \"{plugin}\": id must be text, like \"inspector\""
                ));
            }
            None => {
                return Err(format!(
                    "zeron.imgui.open_window needs an id for plugin \"{plugin}\", like \"inspector\""
                ));
            }
        };
        if id.len() > MAX_ID_BYTES {
            return Err(refuse(
                "id",
                &id,
                &format!("id can't be longer than {MAX_ID_BYTES} bytes"),
            ));
        }
        if id.contains('/') || id.chars().any(char::is_control) {
            return Err(refuse(
                "id",
                &id,
                "id can't hold a slash or control characters",
            ));
        }
        let title = match options.field("title") {
            Some(Value::Str(title)) if !title.is_empty() => title.to_string(),
            Some(Value::Str(_)) => {
                return Err(refuse("title", &id, "title can't be empty"));
            }
            Some(_) => return Err(refuse("title", &id, "title must be text")),
            None => {
                return Err(format!(
                    "zeron.imgui.open_window needs a title for window \"{id}\""
                ));
            }
        };
        if title.len() > MAX_TITLE_BYTES {
            return Err(refuse(
                "title",
                &id,
                &format!("title can't be longer than {MAX_TITLE_BYTES} bytes"),
            ));
        }

        let size = match options.field("size") {
            Some(value) => parse_size(value, "size", &id)?,
            None => (DEFAULT_WIDTH, DEFAULT_HEIGHT),
        };
        let position = match options.field("position") {
            Some(value) => Some(parse_position(value, &id)?),
            None => None,
        };
        let min_size = match options.field("min_size") {
            Some(value) => Some(parse_size(value, "min_size", &id)?),
            None => None,
        };
        let max_size = match options.field("max_size") {
            Some(value) => Some(parse_size(value, "max_size", &id)?),
            None => None,
        };
        if let (Some(min), Some(max)) = (min_size, max_size)
            && (min.0 > max.0 || min.1 > max.1)
        {
            return Err(refuse(
                "min_size",
                &id,
                "min_size can't be larger than max_size",
            ));
        }
        let resizable = match options.field("resizable") {
            Some(Value::Bool(resizable)) => *resizable,
            Some(_) => return Err(refuse("resizable", &id, "resizable must be true or false")),
            None => true,
        };
        let visible = match options.field("visible") {
            Some(Value::Bool(visible)) => *visible,
            Some(_) => return Err(refuse("visible", &id, "visible must be true or false")),
            None => true,
        };
        let titlebar = match options.field("titlebar") {
            Some(Value::Str(style)) => match style.as_ref() {
                "standard" => Titlebar::Standard,
                "inset" => Titlebar::Inset,
                "hidden" => Titlebar::Hidden,
                _ => {
                    return Err(refuse(
                        "titlebar",
                        &id,
                        "titlebar must be \"standard\", \"inset\" or \"hidden\"",
                    ));
                }
            },
            Some(_) => {
                return Err(refuse(
                    "titlebar",
                    &id,
                    "titlebar must be \"standard\", \"inset\" or \"hidden\"",
                ));
            }
            None => Titlebar::Standard,
        };
        let decorations = match options.field("decorations") {
            Some(Value::Bool(decorations)) => *decorations,
            Some(_) => {
                return Err(refuse(
                    "decorations",
                    &id,
                    "decorations must be true or false",
                ));
            }
            None => true,
        };
        let remember = match options.field("remember") {
            Some(Value::Bool(remember)) => *remember,
            Some(_) => return Err(refuse("remember", &id, "remember must be true or false")),
            None => false,
        };

        // The initial size must sit inside the window's own bounds, then
        // inside the host's: a plugin asking for 100x60 gets the smallest
        // usable window rather than a refusal.
        let mut size = (size.0.max(MIN_WIDTH), size.1.max(MIN_HEIGHT));
        size = (size.0.min(MAX_WIDTH), size.1.min(MAX_HEIGHT));
        if let Some(min) = min_size {
            size = (size.0.max(min.0), size.1.max(min.1));
        }
        if let Some(max) = max_size {
            size = (size.0.min(max.0), size.1.min(max.1));
        }

        Ok(Self {
            id,
            title,
            size,
            position,
            min_size,
            max_size,
            resizable,
            visible,
            titlebar,
            decorations,
            remember,
        })
    }

    /// This request as zgx opens a window: `scale` turns the logical size
    /// and position into the physical pixels an initial frame uses.
    pub fn to_options(&self, key: &WindowKey, scale: f64) -> WindowOptions {
        let scale = if scale.is_finite() && scale > 0.0 {
            scale
        } else {
            1.0
        };
        let pixels = |points: f64| (points * scale).round().max(1.0) as u32;
        let bounds = self.position.map(|(x, y)| WindowBounds {
            position: Some((
                (f64::from(x) * scale).round() as i32,
                (f64::from(y) * scale).round() as i32,
            )),
            size: (pixels(self.size.0), pixels(self.size.1)),
        });
        WindowOptions {
            title: self.title.clone(),
            width: self.size.0,
            height: self.size.1,
            bounds,
            min_size: self.min_size,
            max_size: self.max_size,
            resizable: self.resizable,
            decorations: self.decorations,
            titlebar: self.titlebar,
            visible: self.visible,
            // A plugin's windows are its own: they never join AppKit's tabs
            // with the app's or another plugin's windows.
            tabbing: Tabbing::Disallowed,
            frame_autosave: self
                .remember
                .then(|| format!("plugin.{}.{}", key.plugin, key.id)),
            ..WindowOptions::default()
        }
    }
}

/// A refusal about `field`, saying which window it was for when the id is
/// known (`… can't use size for window "inspector": …`).
fn refuse(field: &str, id: &str, why: &str) -> String {
    format!("zeron.imgui.open_window can't use {field} for window \"{id}\": {why}")
}

/// A `{ width, height }` table of finite, positive numbers, clamped to the
/// host's usable range. `field` is `size`, `min_size` or `max_size`, and the
/// refusal names it.
fn parse_size(value: &Value, field: &str, id: &str) -> Result<(f64, f64), String> {
    let side = |name: &str| -> Result<f64, String> {
        match value.field(name) {
            Some(Value::Num(number)) if number.is_finite() && *number > 0.0 => Ok(*number),
            Some(Value::Num(_)) => {
                Err(refuse(field, id, &format!("{name} must be more than zero")))
            }
            Some(_) => Err(refuse(field, id, &format!("{name} must be a number"))),
            None => Err(refuse(
                field,
                id,
                &format!("{field} needs a width and a height, so {name} is missing"),
            )),
        }
    };
    let width = side("width")?;
    let height = side("height")?;
    Ok((
        width.clamp(MIN_WIDTH, MAX_WIDTH),
        height.clamp(MIN_HEIGHT, MAX_HEIGHT),
    ))
}

/// A `set_size` argument, checked the way `open_window`'s size is (a
/// refusal names `size` and the window it was for).
pub(crate) fn size_argument(value: &Value, id: &str) -> Result<(f64, f64), String> {
    parse_size(value, "size", id)
}

/// A `{ x, y }` table, both required: half a position would put the window
/// somewhere nobody asked for, so it is refused instead.
fn parse_position(value: &Value, id: &str) -> Result<(i32, i32), String> {
    let side = |name: &str| -> Result<i32, String> {
        match value.field(name) {
            Some(Value::Num(number)) if number.is_finite() => Ok(number.round() as i32),
            Some(Value::Num(_)) => Err(refuse("position", id, &format!("{name} must be a number"))),
            Some(_) => Err(refuse("position", id, &format!("{name} must be a number"))),
            None => Err(refuse(
                "position",
                id,
                "position needs both x and y, so one is missing",
            )),
        }
    };
    let x = side("x")?;
    let y = side("y")?;
    Ok((x, y))
}

/// The state every control keeps, apart from the native window: open or
/// closed, shown or hidden, minimized or not, and the title and size last
/// asked for.
///
/// It is deliberately testable without a window: the rules (hiding is not
/// closing, closing happens once, a closed window refuses every control) are
/// what the contract is about, and the native calls only carry them out.
#[derive(Debug)]
pub(crate) struct WindowState {
    key: WindowKey,
    title: RefCell<String>,
    size: Cell<(f64, f64)>,
    visible: Cell<bool>,
    minimized: Cell<bool>,
    open: Cell<bool>,
    /// Whether `begin_close` already ran: the notification happens once,
    /// however many paths close the window.
    closed_once: Cell<bool>,
}

impl WindowState {
    pub(crate) fn new(key: WindowKey, spec: &WindowSpec) -> Self {
        Self {
            key,
            title: RefCell::new(spec.title.clone()),
            size: Cell::new(spec.size),
            visible: Cell::new(spec.visible),
            minimized: Cell::new(false),
            open: Cell::new(true),
            closed_once: Cell::new(false),
        }
    }

    pub(crate) fn key(&self) -> &WindowKey {
        &self.key
    }

    pub(crate) fn title(&self) -> String {
        self.title.borrow().clone()
    }

    pub(crate) fn size(&self) -> (f64, f64) {
        self.size.get()
    }

    pub(crate) fn is_open(&self) -> bool {
        self.open.get()
    }

    pub(crate) fn is_shown(&self) -> bool {
        self.open.get() && self.visible.get() && !self.minimized.get()
    }

    pub(crate) fn is_minimized(&self) -> bool {
        self.minimized.get()
    }

    /// The refusal a window that is closed hands every control: it names the
    /// closed window, so a plugin holding an old handle knows which one it
    /// is, and opening it again is the fix.
    pub(crate) fn require_open(&self) -> Result<(), String> {
        if self.open.get() {
            Ok(())
        } else {
            Err(format!(
                "Window \"{}\" of plugin \"{}\" is closed. Open it again with zeron.imgui.open_window.",
                self.key.id, self.key.plugin
            ))
        }
    }

    pub(crate) fn show(&self) -> Result<(), String> {
        self.require_open()?;
        self.visible.set(true);
        self.minimized.set(false);
        Ok(())
    }

    pub(crate) fn hide(&self) -> Result<(), String> {
        self.require_open()?;
        self.visible.set(false);
        Ok(())
    }

    pub(crate) fn minimize(&self) -> Result<(), String> {
        self.require_open()?;
        self.visible.set(true);
        self.minimized.set(true);
        Ok(())
    }

    pub(crate) fn restore(&self) -> Result<(), String> {
        self.require_open()?;
        self.visible.set(true);
        self.minimized.set(false);
        Ok(())
    }

    pub(crate) fn set_title(&self, title: &str) -> Result<(), String> {
        self.require_open()?;
        if title.is_empty() {
            return Err(refuse("title", &self.key.id, "title can't be empty"));
        }
        if title.len() > MAX_TITLE_BYTES {
            return Err(refuse(
                "title",
                &self.key.id,
                &format!("title can't be longer than {MAX_TITLE_BYTES} bytes"),
            ));
        }
        *self.title.borrow_mut() = title.to_string();
        Ok(())
    }

    pub(crate) fn set_size(&self, size: (f64, f64)) -> Result<(), String> {
        self.require_open()?;
        if !size.0.is_finite() || !size.1.is_finite() || size.0 <= 0.0 || size.1 <= 0.0 {
            return Err(refuse(
                "size",
                &self.key.id,
                "width and height must be more than zero",
            ));
        }
        let size = (
            size.0.clamp(MIN_WIDTH, MAX_WIDTH),
            size.1.clamp(MIN_HEIGHT, MAX_HEIGHT),
        );
        self.size.set(size);
        Ok(())
    }

    /// Marks the window closed and reports whether this call was the one
    /// that did it. False on every later call, so the plugin's `on_close`
    /// runs exactly once whichever path closed it.
    pub(crate) fn begin_close(&self) -> bool {
        let first = !self.closed_once.get();
        self.closed_once.set(true);
        let was_open = self.open.replace(false);
        self.visible.set(false);
        self.minimized.set(false);
        first && was_open
    }
}

/// How a window went away, which decides whether its plugin is told.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseReason {
    /// The plugin closed it, or the person closed the OS window: its
    /// `on_close` runs.
    Closed,
    /// The plugin was removed or replaced: its windows go without callbacks.
    Removed,
    /// The plugin failed and is withdrawn: its windows go without callbacks.
    Failed,
    /// Zeron is closing: nothing is handed back to a script that is going.
    Shutdown,
}

impl CloseReason {
    fn notifies(self) -> bool {
        matches!(self, Self::Closed)
    }
}

/// One open plugin window: its native window, its GPU view, its Dear ImGui
/// context, and what every control changed.
pub struct PluginWindow {
    key: WindowKey,
    handle: WindowHandle,
    state: WindowState,
    view: GpuView,
    painter: Rc<RefCell<ImguiViewport>>,
    /// The window's Dear ImGui context. Scene-suspended between frames, so
    /// another window can take its turn; dropped when the window closes,
    /// which frees its font atlas and everything it owned.
    context: RefCell<Option<ImGuiContext>>,
    /// The frame pump's bookkeeping for this window: the live frame, what it
    /// has drawn, and why it last failed.
    frames: FrameState,
    /// Whether the plugin registered an `on_render`: only then is a frame
    /// worth asking for.
    renderable: Cell<bool>,
    /// Device pixels per logical point, as the app last reported it.
    scale: Cell<f64>,
    /// What to tell the plugin when the window closes (the bridge pushes an
    /// `imgui.close` call into the plugin's queue).
    notify: Option<CloseListener>,
}

impl PluginWindow {
    pub fn key(&self) -> &WindowKey {
        &self.key
    }

    pub fn id(&self) -> &str {
        &self.key.id
    }

    pub fn is_open(&self) -> bool {
        self.state.is_open() && !self.handle.is_closed()
    }

    /// Whether frames are being drawn: open, shown and not minimized.
    pub fn is_shown(&self) -> bool {
        self.state.is_shown() && !self.handle.is_closed()
    }

    pub fn is_minimized(&self) -> bool {
        self.state.is_minimized()
    }

    pub fn title(&self) -> String {
        self.state.title()
    }

    pub fn size(&self) -> (f64, f64) {
        self.state.size()
    }

    /// Frames this window's view has drawn. A hidden window's count stops.
    pub fn frames(&self) -> u64 {
        self.painter.borrow().frames()
    }

    /// The native handle, for what the frame pump and input need (never
    /// handed to Luau).
    pub fn handle(&self) -> &WindowHandle {
        &self.handle
    }

    /// The GPU view the window's content is, for invalidating and pacing.
    pub fn view(&self) -> &GpuView {
        &self.view
    }

    /// Changes what the next frame draws, and asks the window for one.
    pub fn set_prepared(&self, frame: super::render::PreparedFrame) {
        self.painter.borrow_mut().set_prepared(frame);
        if self.is_shown() {
            self.view.invalidate();
        }
    }

    /// Asks for one frame, where a frame is worth drawing.
    pub fn request_frame(&self) {
        if self.is_shown() {
            self.view.invalidate();
        }
    }

    /// Feeds one event this window's content received into its Dear ImGui IO,
    /// and asks for a frame when what is drawn changed. The framework routes
    /// each window's document events to that window's own view, so input from
    /// one plugin window never reaches another's.
    pub fn pointer(&self, event: &zgx::input::InputEvent) {
        if !self.is_shown() {
            return;
        }
        let changed = self
            .with_context(|context| super::input::pointer(context, event))
            .unwrap_or(false);
        if changed {
            self.request_frame();
        }
    }

    /// Runs `f` with the window's Dear ImGui context. None when the window
    /// closed and its context went with it.
    pub fn with_context<R>(&self, f: impl FnOnce(&mut ImGuiContext) -> R) -> Option<R> {
        self.context.borrow_mut().as_mut().map(f)
    }

    /// The context's frame count, as the diagnostics surface will show it.
    pub fn imgui_frames(&self) -> u64 {
        self.context
            .borrow()
            .as_ref()
            .map_or(0, ImGuiContext::frames)
    }

    /// What the frame pump has done with this window: frames painted, errors,
    /// the last frame's CPU time.
    pub fn frame_report(&self) -> FrameReport {
        self.frames.report()
    }

    /// Marks that the plugin registered an `on_render`, so the pump asks for
    /// frames. A registered callback draws what is already there at once.
    pub fn set_renderable(&self, on: bool) {
        self.renderable.set(on);
        if on {
            self.request_frame();
        }
    }

    /// Whether the pump should ask this window for a frame now: it is shown,
    /// the plugin draws it, and no frame is in flight.
    pub fn wants_frame(&self) -> bool {
        self.frame_wants()
    }

    /// Applies one widget call from the plugin's render callback.
    pub fn widget(&self, op: &str, seq: u64, arg: &Value) -> Result<Value, String> {
        self.frame_widget(op, seq, arg)
    }

    /// Ends the frame the render callback finished (or failed). `refused` says
    /// the failure was a widget call the host said no to.
    pub fn end_frame(&self, seq: u64, error: Option<String>, refused: bool) -> Result<(), String> {
        self.frame_script_end(seq, error, refused)
    }

    pub fn show(&self) -> Result<(), String> {
        self.state.show()?;
        self.handle.set_visible(true);
        self.handle.request_focus();
        Ok(())
    }

    pub fn hide(&self) -> Result<(), String> {
        self.state.hide()?;
        self.handle.set_visible(false);
        Ok(())
    }

    pub fn focus(&self) -> Result<(), String> {
        self.state.require_open()?;
        self.handle.request_focus();
        Ok(())
    }

    pub fn minimize(&self) -> Result<(), String> {
        self.state.minimize()?;
        self.handle.set_minimized(true);
        Ok(())
    }

    pub fn restore(&self) -> Result<(), String> {
        self.state.restore()?;
        self.handle.set_minimized(false);
        self.handle.request_focus();
        Ok(())
    }

    pub fn set_title(&self, title: &str) -> Result<(), String> {
        self.state.set_title(title)?;
        self.handle.set_title(title);
        Ok(())
    }

    pub fn set_size(&self, size: (f64, f64)) -> Result<(), String> {
        self.state.set_size(size)?;
        let size = self.state.size();
        self.handle.set_inner_size(size.0, size.1);
        Ok(())
    }

    /// Closes the window and tells the plugin once. Closing again is a
    /// no-op that returns `Ok`: a plugin may close a window it already
    /// closed (from `on_close`, say) without an error.
    pub fn close(&self) -> Result<(), String> {
        self.close_with_reason(CloseReason::Closed);
        Ok(())
    }

    /// Closes the window for `reason`, telling the plugin only when the
    /// reason is the plugin's own (`Closed`).
    pub(crate) fn close_with_reason(&self, reason: CloseReason) {
        if self.state.begin_close()
            && reason.notifies()
            && let Some(notify) = &self.notify
        {
            notify(&self.key.id);
        }
        self.view.animate(false);
        self.handle.close();
    }

    /// Called once the native window is gone: releases the Dear ImGui
    /// context and leaves the state closed (a retained handle then refuses
    /// every control).
    pub(crate) fn finish_close(&self) {
        self.state.begin_close();
        self.context.borrow_mut().take();
    }
}

/// A window as the frame pump sees it: the host's half of `frame::FrameWindow`.
impl FrameWindow for PluginWindow {
    fn frame_plugin(&self) -> &str {
        &self.key.plugin
    }

    fn frame_id(&self) -> &str {
        &self.key.id
    }

    fn frame_open(&self) -> bool {
        self.is_open()
    }

    fn frame_shown(&self) -> bool {
        self.is_shown()
    }

    fn frame_renderable(&self) -> bool {
        self.renderable.get()
    }

    fn frame_size(&self) -> (f64, f64) {
        self.state.size()
    }

    fn frame_scale(&self) -> f64 {
        self.scale.get()
    }

    fn frame_target(&self) -> Option<(u32, u32)> {
        self.painter.borrow().last_target_size()
    }

    fn frame_build(
        &self,
        spec: FrameSpec,
        draw: impl FnOnce(*const Ui),
    ) -> Result<super::render::PreparedFrame, String> {
        // The view asks for the atlas on its first frame and after a device
        // rebuild; the context copies it whenever it changed as well.
        let want_atlas = self
            .painter
            .borrow()
            .renderer()
            .is_none_or(|renderer| renderer.needs_atlas());
        self.with_context(|context| {
            context.build_frame(spec, want_atlas, |ui| {
                // SAFETY: `build_frame` keeps the context alive and its frame
                // open for exactly this closure, on this thread.
                let root = unsafe { &*ui };
                // The plugin draws into one window filling the client area:
                // its widgets land in the plugin's window, never Zeron's.
                super::frame::root_window(root, &spec, || draw(ui));
            })
        })
        .ok_or_else(|| {
            format!(
                "Window \"{}\" of plugin \"{}\" is closed.",
                self.key.id, self.key.plugin
            )
        })?
        .map_err(|error| error.to_string())
    }

    fn frame_deliver(&self, frame: super::render::PreparedFrame) {
        self.set_prepared(frame);
    }

    fn frame_close(&self) {
        let _ = self.close();
    }

    fn frame_state(&self) -> &FrameState {
        &self.frames
    }
}

/// Draws a plugin window's prepared frame: the paint half of the frame
/// pump, shared with the host so the update half can set what is drawn.
struct WindowPainter(Rc<RefCell<ImguiViewport>>);

impl Viewport for WindowPainter {
    fn render(&mut self, frame: &mut ViewportFrame<'_>) {
        self.0.borrow_mut().render(frame);
    }

    fn device_changed(&mut self) {
        self.0.borrow_mut().device_changed();
    }
}

/// Builds the content of a plugin window: one GPU view filling it, focusable
/// so keyboard input reaches the plugin (the pump forwards the events).
fn build_content(
    cx: &mut zgx_desktop::WindowContext,
    view: &GpuView,
    slot: &Rc<RefCell<Option<Rc<PluginWindow>>>>,
    open: &Rc<RefCell<std::collections::BTreeMap<String, Rc<PluginWindow>>>>,
    key: &WindowKey,
) {
    let _ = cx.render(
        gpu_view("", view)
            .w_full()
            .h_full()
            .focusable(true)
            // The window's own pointer events go to its Dear ImGui context:
            // only this window's view is mounted in this window's document.
            .on_document_event({
                let slot = slot.clone();
                move |event: &zgx::input::InputEvent| {
                    if let Some(window) = slot.borrow().clone() {
                        window.pointer(event);
                    }
                }
            })
            // The content, not a control inside it, takes the keyboard: focus
            // once it is laid out (a node nothing has laid out cannot take
            // focus yet).
            .on_mount(|handle| {
                let focus = handle.clone();
                handle.after_layout(move || {
                    focus.focus();
                });
            }),
    );
    let open = open.clone();
    let global = key.global();
    cx.on_closed(move || {
        let removed = open.borrow_mut().remove(&global);
        if let Some(window) = removed {
            window.finish_close();
        }
    });
    cx.window.request_focus();
    // The record is filled in right after `WindowFactory::open` returns,
    // before the loop makes this window (the build runs on the next turn).
    let Some(record) = slot.borrow().clone() else {
        return;
    };
    // The title bar's close button (and the window manager's) asks this
    // first: the plugin's `on_close` runs once, then the window goes.
    let closing = record;
    cx.on_close_requested(move || {
        closing.close_with_reason(CloseReason::Closed);
        true
    });
}

/// A window as the host holds it, for the reports diagnostics show.
#[derive(Clone, Debug, PartialEq)]
pub struct WindowReport {
    pub plugin: String,
    pub id: String,
    pub title: String,
    pub open: bool,
    pub shown: bool,
    pub minimized: bool,
    pub size: (f64, f64),
    pub frames: u64,
}

/// Everything a window needs that only the application has: the factory
/// that makes native windows, and the main window's size and physical
/// client size (their ratio is the display scale an initial frame uses).
pub struct Windows {
    factory: WindowFactory,
    viewport: Signal<(f32, f32)>,
    bounds: Signal<WindowBounds>,
    open: Rc<RefCell<std::collections::BTreeMap<String, Rc<PluginWindow>>>>,
    /// What to tell each plugin when one of its windows closes.
    listeners: RefCell<std::collections::HashMap<String, CloseListener>>,
    /// How to ask each plugin for a frame (its own call queue), set when its
    /// actions are served.
    calls: RefCell<std::collections::HashMap<String, FrameCalls>>,
    /// Where the pump's round robin resumes: one window a tick, so several
    /// plugin windows can never add up to a stalled main window.
    pump_cursor: Cell<usize>,
    /// Whether the pump task is running already.
    pumping: Cell<bool>,
    /// The window's UI thread, where the pump and the plugin's scripts run.
    tasks: zgx_desktop::TaskSpawner,
}

impl Windows {
    /// The factory and scale a window context gives.
    pub fn new(cx: &zgx_desktop::WindowContext) -> Rc<Self> {
        Rc::new(Self {
            factory: cx.windows.clone(),
            viewport: cx.viewport.clone(),
            bounds: cx.window_info.bounds.clone(),
            open: Rc::default(),
            listeners: RefCell::default(),
            calls: RefCell::default(),
            pump_cursor: Cell::new(0),
            pumping: Cell::new(false),
            tasks: cx.tasks.clone(),
        })
    }

    /// How many device pixels a logical point is, as the main window last
    /// reported it. 1.0 until the window says (a headless or not-yet-shown
    /// window), which is also what a 100% display uses.
    fn scale(&self) -> f64 {
        let logical = self.viewport.with_untracked(|(width, _)| f64::from(*width));
        let physical = self
            .bounds
            .with_untracked(|bounds| f64::from(bounds.size.0));
        if logical > 0.0 && physical > 0.0 {
            (physical / logical).clamp(0.5, 4.0)
        } else {
            1.0
        }
    }

    /// Where a plugin's closed-window calls go. One per plugin, set when
    /// the plugin's actions are served; a plugin that never set one is told
    /// nothing (a script that has no `on_close`).
    pub fn listen(&self, plugin: &str, handler: impl Fn(&str) + 'static) {
        self.listeners
            .borrow_mut()
            .insert(plugin.to_string(), Rc::new(handler));
    }

    /// Where the frame pump asks `plugin` for a frame. Set when the plugin's
    /// actions are served; a plugin with none (a dev fixture without the
    /// SDK) simply never has a frame asked for.
    pub fn listen_calls(&self, plugin: &str, calls: FrameCalls) {
        self.calls.borrow_mut().insert(plugin.to_string(), calls);
    }

    /// Starts the pump: one window a tick, sixteen milliseconds apart while
    /// any plugin window is shown, a slow beat when none is, so an app with
    /// no plugin windows does no per-frame work.
    ///
    /// The task holds the registry weakly: it stops when the app's window
    /// context goes away.
    pub fn start_pump(self: &Rc<Self>) {
        if self.pumping.replace(true) {
            return;
        }
        let weak = Rc::downgrade(self);
        let tasks = self.tasks.clone();
        tasks.spawn(async move {
            loop {
                let Some(windows) = weak.upgrade() else {
                    return;
                };
                let shown = windows.pump();
                drop(windows);
                let wait = if shown { 16 } else { 120 };
                zgx::timer::sleep(std::time::Duration::from_millis(wait)).await;
            }
        });
    }

    /// Asks one shown window for a frame, in round-robin order. Returns
    /// whether any window is waiting for one, which decides the next tick's
    /// pace.
    pub fn pump(&self) -> bool {
        let waiting: Vec<Rc<PluginWindow>> = self
            .open
            .borrow()
            .values()
            .filter(|window| window.wants_frame())
            .cloned()
            .collect();
        if waiting.is_empty() {
            self.pump_cursor.set(0);
            return false;
        }
        let at = self.pump_cursor.get() % waiting.len();
        self.pump_cursor.set((at + 1) % waiting.len());
        let window = &waiting[at];
        let calls = self.calls.borrow().get(window.key.plugin.as_str()).cloned();
        if let Some(calls) = calls {
            super::frame::run(window.as_ref(), &calls);
        }
        true
    }

    /// Every open window, for diagnostics and tests.
    pub fn all(&self) -> Vec<Rc<PluginWindow>> {
        self.open.borrow().values().cloned().collect()
    }

    /// Opens `spec` as `plugin`'s window.
    ///
    /// Nothing is left behind when this fails: the Dear ImGui context is
    /// made before the native window (so a context failure creates none),
    /// and a factory that hands back a closed handle leaves no entry
    /// either.
    pub fn open(&self, plugin: &str, spec: WindowSpec) -> Result<Rc<PluginWindow>, String> {
        let key = WindowKey::new(plugin, spec.id.clone());
        let global = key.global();
        {
            let open = self.open.borrow();
            if open.contains_key(&global) {
                return Err(format!(
                    "Window \"{}\" of plugin \"{plugin}\" is already open. Close it before opening it again.",
                    spec.id
                ));
            }
            let mine = open.values().filter(|w| w.key.plugin == plugin).count();
            check_quota(plugin, mine, open.len())?;
        }

        let painter = Rc::new(RefCell::new(ImguiViewport::new()));
        let view = zgx_gpu::viewport::view(WindowPainter(painter.clone()));
        // Before the window exists: a context that cannot be made fails the
        // open with nothing created, which is the no-half-window rule.
        let context = ImGuiContext::try_create(format!("plugin {plugin} window {}", spec.id))
            .map_err(|error| {
                format!(
                    "zeron.imgui.open_window couldn't make window \"{}\": {error}",
                    spec.id
                )
            })?;
        let options = spec.to_options(&key, self.scale());
        let slot: Rc<RefCell<Option<Rc<PluginWindow>>>> = Rc::default();
        let handle = {
            let (slot, view, open, key) =
                (slot.clone(), view.clone(), self.open.clone(), key.clone());
            self.factory.open(options, move |cx| {
                build_content(cx, &view, &slot, &open, &key);
            })
        };
        if handle.is_closed() {
            return Err(format!(
                "zeron.imgui.open_window couldn't open window \"{}\": the application is closed",
                spec.id
            ));
        }
        let record = Rc::new(PluginWindow {
            state: WindowState::new(key.clone(), &spec),
            key,
            handle,
            view,
            painter,
            context: RefCell::new(Some(context)),
            frames: FrameState::new(),
            renderable: Cell::new(false),
            scale: Cell::new(self.scale()),
            notify: self.listeners.borrow().get(plugin).cloned(),
        });
        *slot.borrow_mut() = Some(record.clone());
        self.open.borrow_mut().insert(global, record.clone());
        Ok(record)
    }

    /// `plugin`'s window `id`, while it is open.
    pub fn window(&self, plugin: &str, id: &str) -> Option<Rc<PluginWindow>> {
        self.open
            .borrow()
            .get(&WindowKey::new(plugin, id).global())
            .cloned()
    }

    pub fn windows_of(&self, plugin: &str) -> Vec<Rc<PluginWindow>> {
        self.open
            .borrow()
            .values()
            .filter(|window| window.key.plugin == plugin)
            .cloned()
            .collect()
    }

    pub fn count(&self, plugin: &str) -> usize {
        self.open
            .borrow()
            .values()
            .filter(|window| window.key.plugin == plugin)
            .count()
    }

    pub fn total(&self) -> usize {
        self.open.borrow().len()
    }

    /// Closes every window `plugin` has. Its windows are its own: removing
    /// or failing the plugin takes them, and the plugin itself is untouched
    /// by a window closing.
    pub fn close_all(&self, plugin: &str, reason: CloseReason) {
        let records: Vec<Rc<PluginWindow>> = {
            let mut open = self.open.borrow_mut();
            let keys: Vec<String> = open
                .values()
                .filter(|window| window.key.plugin == plugin)
                .map(|window| window.key.global())
                .collect();
            keys.into_iter()
                .filter_map(|key| open.remove(&key))
                .collect()
        };
        for record in records {
            record.close_with_reason(reason);
        }
    }

    /// Closes every plugin window: Zeron is going, or the window that hosted
    /// the scripts is.
    pub fn close_all_all(&self, reason: CloseReason) {
        let records: Vec<Rc<PluginWindow>> = self.open.borrow_mut().values().cloned().collect();
        self.open.borrow_mut().clear();
        for record in records {
            record.close_with_reason(reason);
        }
    }

    /// Every window, in key order: what diagnostics and tests read.
    pub fn report(&self) -> Vec<WindowReport> {
        self.open
            .borrow()
            .values()
            .map(|window| WindowReport {
                plugin: window.key.plugin.clone(),
                id: window.key.id.clone(),
                title: window.title(),
                open: window.is_open(),
                shown: window.is_shown(),
                minimized: window.is_minimized(),
                size: window.size(),
                frames: window.frames(),
            })
            .collect()
    }
}

/// The per-plugin and whole-app caps. The per-plugin cap is 8 and the whole
/// app's 32, as the architecture pins them.
pub const MAX_WINDOWS_PER_PLUGIN: usize = 8;
pub const MAX_WINDOWS_TOTAL: usize = 32;

/// Whether `plugin`, which has `mine` windows open while the app has
/// `total`, may open one more. The refusal names the cap, so a plugin's
/// author sees the number.
pub(crate) fn check_quota(plugin: &str, mine: usize, total: usize) -> Result<(), String> {
    if mine >= MAX_WINDOWS_PER_PLUGIN {
        return Err(format!(
            "Plugin \"{plugin}\" can have {MAX_WINDOWS_PER_PLUGIN} native windows open at once. Close one before opening another."
        ));
    }
    if total >= MAX_WINDOWS_TOTAL {
        return Err(format!(
            "Zeron can have {MAX_WINDOWS_TOTAL} plugin windows open at once. Close one before opening another."
        ));
    }
    Ok(())
}

impl fmt::Debug for PluginWindow {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PluginWindow")
            .field("key", &self.key)
            .field("title", &self.title())
            .field("size", &self.size())
            .field("open", &self.is_open())
            .field("shown", &self.is_shown())
            .finish_non_exhaustive()
    }
}
