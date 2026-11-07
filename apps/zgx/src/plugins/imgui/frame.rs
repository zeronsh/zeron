//! The frame pump: one script-driven Dear ImGui frame per shown window.
//!
//! Dear ImGui is immediate mode, but Rust cannot call a Luau closure. The
//! host therefore opens a frame, asks the plugin to run its `on_render` over
//! the plugin's own call queue (`plugin.<id>.calls`), and closes the frame
//! when the script says it is done:
//!
//! ```text
//! pump tick
//!   ├─ begin: activate the window's context, set io, open the frame and
//!   │         the full-window ImGui window the plugin draws into
//!   ├─ push { kind = "imgui.frame", window, seq, budget }
//!   │    └─ the SDK runs on_render(ui)
//!   │         └─ ui.text / ui.button → plugin.<id>.imgui { op, window, seq }
//!   │              (applied to the open frame, synchronously)
//!   │         └─ plugin.<id>.imgui_end { window, seq, ok, error? }
//!   └─ finish: render_legacy(), copy the draw data, hand it to the
//!              window's GPU view, which invalidates so zgx paints it
//! ```
//!
//! The paint phase (a `ViewportFrame`) replays the copy and never touches the
//! context, so a slow script cannot stall presentation.
//!
//! ## What is enforced here
//!
//! * Only shown windows ask for frames: a hidden or minimised window gets no
//!   request, runs no callback and draws none ([`FrameState::wants_frame`],
//!   and the same check at the top of [`run`]).
//! * One frame in flight per window, and one frame per pump tick, so several
//!   plugin windows cannot add up to a stalled main window.
//! * A callback that spends more than [`BUDGET`] is aborted: its frame
//!   is thrown away, the last good frame keeps painting, and the reason is
//!   recorded and logged. Two frames are excused: the window's first
//!   attempted frame, and one that uploads a newly built font atlas. Both pay
//!   for the host's atlas work on the UI thread, not for the plugin's drawing;
//!   every later frame is held to the budget.
//! * A widget call with no live frame — or one made with another window's
//!   `ui`, or after the window closed — fails with a sentence naming the
//!   widget instead of touching Dear ImGui.
//!
//! ## Errors and the plugin
//!
//! An error inside `on_render` is recorded and the plugin stays loaded; three
//! consecutive failures close the window (the plugin keeps running). A window's
//! **first attempted** frame is different: one that cannot draw at all never
//! becomes visible, so that failure is terminal. It is reported once in a
//! sentence naming the window and the script's own words, handed to the bridge
//! when there is one (which withdraws the plugin), and the window is closed on
//! both the dev and installed routes (VAL-LIFE-012). "First" counts attempted
//! frames, not painted ones, so a window that never painted does not take that
//! path again and again. A refused widget is the one failure that never does
//! that: it is the plugin's own drawing mistake, said where the call was made,
//! and the frame is simply dropped ([`FrameEnd::refused`]). A **panic** inside
//! a draw is a host failure: it is caught at the frame boundary so the pump
//! keeps serving every window, recorded against the failing window, and never
//! withdraws the plugin.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::{CString, c_char};
use std::rc::Rc;
use std::time::{Duration, Instant};

use dear_imgui_rs::{Condition, MouseButton, StyleVarVec2, Ui, WindowFlags};
use zgx_script::Value;

use super::render::PreparedFrame;
use super::runtime::FrameSpec;

/// The one ImGui window each frame fills: the plugin's drawing surface. It is
/// named with a `##` id so the plugin's own labels are never confused with it.
pub const ROOT_WINDOW: &str = "##zeron-plugin-window";

/// Chrome the plugin's own OS window already provides, and no scrolling: a
/// plugin's window is its window, not one inside it.
const ROOT_FLAGS: WindowFlags = WindowFlags::NO_TITLE_BAR
    .union(WindowFlags::NO_RESIZE)
    .union(WindowFlags::NO_MOVE)
    .union(WindowFlags::NO_COLLAPSE)
    .union(WindowFlags::NO_SAVED_SETTINGS)
    .union(WindowFlags::NO_SCROLLBAR)
    .union(WindowFlags::NO_SCROLL_WITH_MOUSE);

/// Runs `draw` inside the full-window ImGui window a plugin's widgets land in,
/// so a widget called at the top of `on_render` draws in the plugin's own
/// window and nowhere else.
pub fn root_window(ui: &Ui, spec: &FrameSpec, draw: impl FnOnce()) {
    ui.window(ROOT_WINDOW)
        .position([0.0, 0.0], Condition::Always)
        .size(spec.display_size, Condition::Always)
        .flags(ROOT_FLAGS)
        .build(draw);
}

/// The most time one plugin's render callback may spend in a frame.
///
/// It is measured as elapsed time on the UI thread, which is where the
/// callback runs and what the app's responsiveness is spent from. (The
/// framework's CPU clock on Windows advances a scheduler tick at a time, so
/// it cannot see an 8 ms callback at all.) The SDK watches the same figure
/// from the script side, so a callback that keeps calling widgets stops
/// before the host has to abort it.
///
/// A window's first attempted frame, and any frame that uploads a newly built
/// font atlas, are excused: building and uploading the atlas is host work on
/// the UI thread. The exemption is one frame per window plus its atlas
/// uploads; every other frame is measured against this budget.
pub const BUDGET: Duration = Duration::from_millis(8);

/// The same budget as the number the plugin sees.
pub const BUDGET_MS: f64 = 8.0;

/// The most widget calls one frame may make (the canonical queued-operation
/// cap; the per-plugin counters are `quotas-and-budgets`' subject).
pub const MAX_OPS: u32 = 4096;

/// How many failed frames in a row close the window. The plugin stays on:
/// its windows are its own, and a bug in one must not take the plugin down.
pub const MAX_CONSECUTIVE_ERRORS: u32 = 3;

/// What one window's frames have done, for diagnostics and tests.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FrameReport {
    /// Frames begun.
    pub seq: u64,
    /// Frames that were finished and handed to the GPU view.
    pub frames: u64,
    /// Frames that failed, of any kind.
    pub errors: u64,
    pub consecutive_errors: u32,
    pub last_error: Option<String>,
    /// The error from the window's first attempted frame, when that frame
    /// failed.
    pub first_error: Option<String>,
    /// How long the last frame's callback took, in milliseconds.
    pub last_ms: f64,
    /// Whether a frame is open right now.
    pub live: bool,
}

/// How a frame ended, as the pump reports it.
#[derive(Clone, Debug, PartialEq)]
pub struct FrameEnd {
    pub seq: u64,
    /// Whether the frame was kept and handed to the GPU view.
    pub painted: bool,
    /// Why it was not, when it was not.
    pub error: Option<String>,
    /// Whether this was the window's first attempted frame (not its first
    /// painted one: a window that never paints still gets only one first
    /// attempt).
    pub first: bool,
    /// Whether the callback died on a widget call the host refused (drawn
    /// outside a live frame, or after the window closed), rather than on
    /// the plugin's own error. A refusal is the plugin's drawing mistake:
    /// it never takes the plugin down.
    pub refused: bool,
    pub consecutive_errors: u32,
    /// Time the callback spent, in milliseconds.
    pub ms: f64,
}

/// A window the pump can build frames for.
///
/// Implemented by the live [`PluginWindow`](super::window::PluginWindow), and
/// by tests with a context and no native window, so the whole protocol —
/// begin, widget calls, end, budget, errors — is exercised without a desktop.
pub trait FrameWindow {
    fn frame_plugin(&self) -> &str;
    fn frame_id(&self) -> &str;
    /// The window is still open (a closed window's `ui` errors).
    fn frame_open(&self) -> bool;
    /// The window is shown: open, visible and not minimized.
    fn frame_shown(&self) -> bool;
    /// The plugin registered an `on_render`, so a frame is worth asking for.
    fn frame_renderable(&self) -> bool;
    /// The window's client size in logical points.
    fn frame_size(&self) -> (f64, f64);
    /// Device pixels per logical point.
    fn frame_scale(&self) -> f64;
    /// The size in device pixels the last painted frame had, when known.
    fn frame_target(&self) -> Option<(u32, u32)>;
    /// Whether the next frame must carry the font atlas: the window's renderer
    /// has none yet (the window's first frame), or it dropped its GPU objects
    /// (a device change). Building and uploading that atlas is host work on
    /// the UI thread, so such a frame is excused the callback budget.
    fn frame_needs_atlas(&self) -> bool;
    /// Builds one frame: opens it, runs `draw` inside the full-window ImGui
    /// window the plugin draws into, then ends and copies it.
    fn frame_build(
        &self,
        spec: FrameSpec,
        draw: impl FnOnce(*const Ui),
    ) -> Result<PreparedFrame, String>;
    /// Hands a finished frame to the GPU view, which invalidates so it paints.
    fn frame_deliver(&self, frame: PreparedFrame);
    /// The window is beyond saving; close it (the plugin stays loaded).
    fn frame_close(&self);
    /// This window's frame bookkeeping.
    fn frame_state(&self) -> &FrameState;

    /// Whether a pump tick should ask this window for a frame.
    fn frame_wants(&self) -> bool
    where
        Self: Sized,
    {
        self.frame_state().wants_frame(self)
    }

    fn frame_prepare(&self) -> Result<(u64, FrameSpec), String>
    where
        Self: Sized,
    {
        self.frame_state().prepare(self)
    }

    fn frame_set_ui(&self, ui: *const Ui)
    where
        Self: Sized,
    {
        self.frame_state().set_ui(ui)
    }

    fn frame_script_end(&self, seq: u64, error: Option<String>, refused: bool) -> Result<(), String>
    where
        Self: Sized,
    {
        self.frame_state().script_end(seq, error, refused)
    }

    fn frame_conclude(&self, prepared: Result<PreparedFrame, String>) -> FrameEnd
    where
        Self: Sized,
    {
        self.frame_state().conclude(self, prepared)
    }

    /// Ends the frame a panic brought down. The panic is a host failure, not
    /// the plugin's: it counts against this window and is reported, but it
    /// never withdraws the plugin (unlike a script error on a first frame).
    fn frame_conclude_panic(&self, error: String) -> FrameEnd
    where
        Self: Sized,
    {
        self.frame_state().conclude_panic(self, error)
    }

    fn frame_take_end(&self) -> Option<FrameEnd> {
        self.frame_state().take_end()
    }

    fn frame_after_run(&self) {}

    fn frame_widget(&self, op: &str, seq: u64, arg: &Value) -> Result<Value, String>
    where
        Self: Sized,
    {
        self.frame_state().widget(self, op, seq, arg)
    }

    fn frame_close_regions(&self, ui: *const Ui) -> Option<String>
    where
        Self: Sized,
    {
        self.frame_state().close_regions(ui)
    }
}

/// One frame in flight: what the widgets are drawn into and what it has cost.
struct LiveFrame {
    seq: u64,
    /// Whether this was the window's first attempted frame. Recorded when
    /// the frame starts, because a window that fails it never paints and so
    /// has no painted frame to compare against later.
    first: bool,
    /// Whether this frame is excused the wall-clock budget: the window's
    /// first attempt, or one that uploads a newly built font atlas. That
    /// work is the host's on the UI thread, not the plugin's callback, so it
    /// must not cost the plugin its frame. Every other frame is held to
    /// [`BUDGET`], so the exemption can never buy unbounded work per frame.
    exempt: bool,
    /// The frame's `Ui`, owned by the context. Valid while the frame is open,
    /// on this thread only; [`FrameState`] never lets it outlive the frame.
    ui: *const Ui,
    started_wall: Instant,
    ops: u32,
    /// Set when the script side reports the callback overran its budget.
    over_budget: bool,
    /// Whether the script closed the frame (`imgui_end`).
    finished: bool,
    /// What the script's `on_render` raised, if it did.
    error: Option<String>,
    /// Whether that was a widget call the host refused.
    refused: bool,
    child_open: bool,
    group_open: bool,
    columns_open: bool,
    tables: Vec<TableState>,
    tab_bar_open: bool,
    tab_item_open: bool,
    menu_bar_open: bool,
    menu_open: bool,
    popup_open: bool,
    list_box_open: bool,
    tree_depth: u32,
    drag_source_open: bool,
    drag_target_open: bool,
}

/// One open table region: which columns the plugin declared, for a sort spec
/// to come back with the column's own label.
struct TableState {
    open: bool,
    columns: Vec<TableColumnInfo>,
}

struct TableColumnInfo {
    label: String,
    sortable: bool,
}

/// One window's frame bookkeeping: the live frame, the counters and the last
/// failure.
#[derive(Default)]
pub struct FrameState {
    seq: Cell<u64>,
    /// Frames begun, painted or not. This is what "first frame" means: a
    /// window that never painted still had its first attempt.
    attempted: Cell<u64>,
    live: RefCell<Option<LiveFrame>>,
    frames: Cell<u64>,
    errors: Cell<u64>,
    consecutive: Cell<u32>,
    last_error: RefCell<Option<String>>,
    first_error: RefCell<Option<String>>,
    last_ms: Cell<f64>,
    last_started: Cell<Option<Instant>>,
    /// The payloads a drag source set, by their type, kept across frames
    /// until a target accepts them (or a new source replaces them).
    drag_payloads: RefCell<HashMap<String, Value>>,
    /// The end of the last finished frame, for the pump to report.
    pending: RefCell<Option<FrameEnd>>,
}

impl FrameState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn live_ui(&self) -> Option<*const Ui> {
        self.live
            .borrow()
            .as_ref()
            .map(|frame| frame.ui)
            .filter(|ui| !ui.is_null())
    }

    /// Whether a pump tick should ask this window for a frame.
    pub fn wants_frame(&self, window: &impl FrameWindow) -> bool {
        window.frame_shown()
            && window.frame_renderable()
            && window.frame_open()
            && self.live.borrow().is_none()
    }

    /// Frames finished and painted so far.
    pub fn frames(&self) -> u64 {
        self.frames.get()
    }

    pub fn report(&self) -> FrameReport {
        FrameReport {
            seq: self.seq.get(),
            frames: self.frames.get(),
            errors: self.errors.get(),
            consecutive_errors: self.consecutive.get(),
            last_error: self.last_error.borrow().clone(),
            first_error: self.first_error.borrow().clone(),
            last_ms: self.last_ms.get(),
            live: self.live.borrow().is_some(),
        }
    }

    /// Starts the window's next frame: takes the next sequence number and
    /// works out the frame spec. The frame itself is opened by
    /// [`FrameWindow::frame_build`], which records its `Ui` with [`set_ui`].
    ///
    /// [`set_ui`]: FrameState::set_ui
    pub fn prepare(&self, window: &impl FrameWindow) -> Result<(u64, FrameSpec), String> {
        if !window.frame_open() || !window.frame_shown() {
            return Err(format!(
                "Window \"{}\" of plugin \"{}\" is not drawing.",
                window.frame_id(),
                window.frame_plugin()
            ));
        }
        if self.live.borrow().is_some() {
            return Err(format!(
                "Window \"{}\" already has a frame in flight.",
                window.frame_id()
            ));
        }
        let now = Instant::now();
        let delta = self.last_started.get().map_or(1.0 / 60.0, |last| {
            now.saturating_duration_since(last)
                .as_secs_f32()
                .clamp(0.0, 0.25)
        });
        let (width, height) = window.frame_size();
        let scale = window.frame_scale().max(0.5);
        let target = window.frame_target().unwrap_or_else(|| {
            (
                (width * scale).round().max(1.0) as u32,
                (height * scale).round().max(1.0) as u32,
            )
        });
        let spec = FrameSpec {
            display_size: [width as f32, height as f32],
            framebuffer_scale: scale as f32,
            target_size: target,
            delta_time: delta,
        };
        let seq = self.seq.get() + 1;
        self.seq.set(seq);
        let first = self.attempted.get() == 0;
        // The first frame pays for the font atlas, and a frame after a device
        // change uploads it again. Those costs are the host's, so those frames
        // are excused the budget; "first" counts attempts, so the window gets
        // exactly one of them and the atlas case is bounded by device changes.
        let exempt = first || window.frame_needs_atlas();
        self.attempted.set(self.attempted.get() + 1);
        self.last_started.set(Some(now));
        self.pending.borrow_mut().take();
        *self.live.borrow_mut() = Some(LiveFrame {
            seq,
            first,
            exempt,
            ui: std::ptr::null(),
            started_wall: now,
            ops: 0,
            over_budget: false,
            finished: false,
            error: None,
            refused: false,
            child_open: false,
            group_open: false,
            columns_open: false,
            tables: Vec::new(),
            tab_bar_open: false,
            tab_item_open: false,
            menu_bar_open: false,
            menu_open: false,
            popup_open: false,
            list_box_open: false,
            tree_depth: 0,
            drag_source_open: false,
            drag_target_open: false,
        });
        Ok((seq, spec))
    }

    /// Records the frame's `Ui`, once the context has opened the frame.
    pub fn set_ui(&self, ui: *const Ui) {
        if let Some(frame) = self.live.borrow_mut().as_mut() {
            frame.ui = ui;
        }
    }

    /// The script's `imgui_end`: the callback finished (or failed).
    ///
    /// `refused` says the failure was a widget call the host said no to, not
    /// the plugin's own error: the frame is dropped either way, but only the
    /// plugin's own error on a window's first frame ends the window's run.
    pub fn script_end(&self, seq: u64, error: Option<String>, refused: bool) -> Result<(), String> {
        let mut live = self.live.borrow_mut();
        let Some(frame) = live.as_mut() else {
            return Err("No frame is open to end.".to_string());
        };
        if frame.seq != seq {
            return Err(format!("Frame {seq} cannot end frame {}.", frame.seq));
        }
        frame.finished = true;
        frame.refused = refused;
        frame.error = error;
        Ok(())
    }

    pub fn close_regions(&self, ui: *const Ui) -> Option<String> {
        let (child_open, group_open, columns_open, tables, tree_depth, list_box_open, popup_open, menu_open, menu_bar_open, tab_item_open, tab_bar_open, drag_source_open, drag_target_open, error_absent) = {
            let live = self.live.borrow();
            let Some(frame) = live.as_ref() else {
                return None;
            };
            (
                frame.child_open,
                frame.group_open,
                frame.columns_open,
                frame.tables.len(),
                frame.tree_depth,
                frame.list_box_open,
                frame.popup_open,
                frame.menu_open,
                frame.menu_bar_open,
                frame.tab_item_open,
                frame.tab_bar_open,
                frame.drag_source_open,
                frame.drag_target_open,
                frame.error.is_none(),
            )
        };
        let unbalanced = if child_open {
            Some(unbalanced_region_sentence("begin_child", "end_child"))
        } else if group_open {
            Some(unbalanced_region_sentence("begin_group", "end_group"))
        } else if columns_open {
            Some(unbalanced_region_sentence("columns", "next_column"))
        } else if tables > 0 {
            Some(unbalanced_region_sentence("begin_table", "end_table"))
        } else if tree_depth > 0 {
            Some(unbalanced_region_sentence("tree_node", "tree_pop"))
        } else if list_box_open {
            Some(unbalanced_region_sentence("begin_list_box", "end_list_box"))
        } else if popup_open {
            Some(unbalanced_region_sentence("begin_popup", "end_popup"))
        } else if menu_open {
            Some(unbalanced_region_sentence("begin_menu", "end_menu"))
        } else if menu_bar_open {
            Some(unbalanced_region_sentence("begin_menu_bar", "end_menu_bar"))
        } else if tab_item_open {
            Some(unbalanced_region_sentence(
                "begin_tab_item",
                "end_tab_item",
            ))
        } else if tab_bar_open {
            Some(unbalanced_region_sentence("begin_tab_bar", "end_tab_bar"))
        } else {
            None
        };
        if !ui.is_null() {
            // SAFETY: the ui points into the frame that is still open here,
            // on this thread, inside the same turn that began it.
            let ui = unsafe { &*ui };
            ui.with_bound_context(|| {
                if drag_target_open {
                    unsafe { dear_imgui_rs::sys::igEndDragDropTarget() };
                }
                if drag_source_open {
                    unsafe { dear_imgui_rs::sys::igEndDragDropSource() };
                }
                for _ in 0..tree_depth {
                    unsafe { dear_imgui_rs::sys::igTreePop() };
                }
                if list_box_open {
                    unsafe { dear_imgui_rs::sys::igEndListBox() };
                }
                if popup_open {
                    unsafe { dear_imgui_rs::sys::igEndPopup() };
                }
                if menu_open {
                    unsafe { dear_imgui_rs::sys::igEndMenu() };
                }
                if menu_bar_open {
                    unsafe { dear_imgui_rs::sys::igEndMenuBar() };
                }
                if tab_item_open {
                    unsafe { dear_imgui_rs::sys::igEndTabItem() };
                }
                if tab_bar_open {
                    unsafe { dear_imgui_rs::sys::igEndTabBar() };
                }
                for _ in 0..tables {
                    unsafe { dear_imgui_rs::sys::igEndTable() };
                }
                if child_open {
                    unsafe { dear_imgui_rs::sys::igEndChild() };
                }
                if group_open {
                    unsafe { dear_imgui_rs::sys::igEndGroup() };
                }
                if columns_open {
                    unsafe { dear_imgui_rs::sys::igColumns(1, std::ptr::null(), false) };
                }
            });
        }
        {
            let mut live = self.live.borrow_mut();
            if let Some(frame) = live.as_mut() {
                frame.child_open = false;
                frame.group_open = false;
                frame.columns_open = false;
                frame.tables.clear();
                frame.tab_bar_open = false;
                frame.tab_item_open = false;
                frame.menu_bar_open = false;
                frame.menu_open = false;
                frame.popup_open = false;
                frame.list_box_open = false;
                frame.tree_depth = 0;
                frame.drag_source_open = false;
                frame.drag_target_open = false;
                if unbalanced.is_some() && error_absent {
                    frame.refused = true;
                    frame.error = unbalanced.clone();
                }
            }
        }
        unbalanced
    }

    /// Applies one widget call to the open frame.
    ///
    /// The refusal names the widget and what went wrong, because it reaches a
    /// plugin author through `ui.text` raising at their line.
    pub fn widget(
        &self,
        window: &impl FrameWindow,
        op: &str,
        seq: u64,
        arg: &Value,
    ) -> Result<Value, String> {
        let mut live = self.live.borrow_mut();
        let Some(frame) = live.as_mut() else {
            return Err(format!(
                "ui.{op} was called outside window \"{}\"'s render callback. Draw with the ui passed to that window's on_render, inside the callback.",
                window.frame_id()
            ));
        };
        if frame.seq != seq {
            return Err(format!(
                "ui.{op} was called from another window's render callback. A ui belongs to window \"{}\" and works only inside its own on_render.",
                window.frame_id()
            ));
        }
        if !window.frame_open() {
            return Err(format!(
                "ui.{op} was called after window \"{}\" was closed. Its ui does nothing once the window is gone.",
                window.frame_id()
            ));
        }
        if frame.ui.is_null() {
            return Err(format!(
                "ui.{op} was called before window \"{}\" began drawing.",
                window.frame_id()
            ));
        }
        frame.ops += 1;
        if frame.ops > MAX_OPS {
            return Err(format!(
                "ui.{op} was the {}th widget this frame; a frame may draw {MAX_OPS} at most.",
                frame.ops
            ));
        }
        if !frame.exempt && (frame.over_budget || frame.started_wall.elapsed() > BUDGET) {
            frame.over_budget = true;
            return Err(budget_sentence(window.frame_id()));
        }
        // SAFETY: `ui` points into the context the window owns, the frame is
        // still open (only `conclude` closes it), and every call here is on
        // the UI thread inside the same turn that opened it.
        let ui = unsafe { &*frame.ui };
        match op {
            "text" => {
                let text = string_arg(arg, "text", op)?;
                ui.text_wrapped(text.as_str());
                let min = ui.item_rect_min();
                let max = ui.item_rect_max();
                Ok(Value::record([
                    ("ok", Value::Bool(true)),
                    ("min", point(min)),
                    ("max", point(max)),
                ]))
            }
            "button" => {
                let label = string_arg(arg, "label", op)?;
                let clicked = ui.button(label.as_str());
                let min = ui.item_rect_min();
                let max = ui.item_rect_max();
                Ok(Value::record([
                    ("ok", Value::Bool(true)),
                    ("value", Value::Bool(clicked)),
                    ("min", point(min)),
                    ("max", point(max)),
                ]))
            }
            "text_colored" => {
                let text = string_arg(arg, "text", op)?;
                let colour = colour_arg(arg, op)?;
                ui.text_colored(colour, text.as_str());
                Ok(Value::record([
                    ("ok", Value::Bool(true)),
                    ("min", point(ui.item_rect_min())),
                    ("max", point(ui.item_rect_max())),
                ]))
            }
            "separator" => {
                ui.separator();
                Ok(Value::record([
                    ("ok", Value::Bool(true)),
                    ("min", point(ui.item_rect_min())),
                    ("max", point(ui.item_rect_max())),
                ]))
            }
            "same_line" => {
                ui.same_line();
                Ok(Value::record([("ok", Value::Bool(true))]))
            }
            "new_line" => {
                ui.new_line();
                Ok(Value::record([("ok", Value::Bool(true))]))
            }
            "spacing" => {
                ui.spacing();
                Ok(Value::record([("ok", Value::Bool(true))]))
            }
            "dummy" => {
                if let Some(key) = unknown_key(arg, &["op", "window", "seq", "width", "height"]) {
                    return Err(format!(
                        "ui.{op} can't use {key}: dummy takes width and height, like ui.dummy({{ width = 40, height = 60 }})"
                    ));
                }
                let size = size_fields(arg, op, "dummy")?;
                ui.dummy(size);
                Ok(Value::record([
                    ("ok", Value::Bool(true)),
                    ("min", point(ui.item_rect_min())),
                    ("max", point(ui.item_rect_max())),
                ]))
            }
            "indent" => {
                ui.indent();
                Ok(Value::record([("ok", Value::Bool(true))]))
            }
            "unindent" => {
                ui.unindent();
                Ok(Value::record([("ok", Value::Bool(true))]))
            }
            "begin_child" => begin_child(ui, frame, arg, op),
            "end_child" => end_child(ui, frame, op),
            "columns" => columns(ui, frame, arg, op),
            "next_column" => next_column(ui, frame, op),
            "begin_group" => begin_group(ui, frame, op),
            "end_group" => end_group(ui, frame, op),
            "set_next_item_width" => {
                if let Some(key) = unknown_key(arg, &["op", "window", "seq", "width"]) {
                    return Err(format!(
                        "ui.{op} can't use {key}: set_next_item_width takes a width, like ui.set_next_item_width(160)"
                    ));
                }
                let width = number_arg(arg, "width", op)?;
                if width <= 0.0 {
                    return Err(format!(
                        "ui.{op} needs width to be more than zero, not {width}."
                    ));
                }
                ui.set_next_item_width(width as f32);
                Ok(Value::record([("ok", Value::Bool(true))]))
            }
            "small_button" => {
                let label = string_arg(arg, "label", op)?;
                let clicked = ui.small_button(label.as_str());
                Ok(Value::record([
                    ("ok", Value::Bool(true)),
                    ("value", Value::Bool(clicked)),
                    ("min", point(ui.item_rect_min())),
                    ("max", point(ui.item_rect_max())),
                ]))
            }
            "checkbox" => {
                let label = string_arg(arg, "label", op)?;
                let mut value = bool_arg(arg, "value", op)?;
                ui.checkbox(label.as_str(), &mut value);
                Ok(Value::record([
                    ("ok", Value::Bool(true)),
                    ("value", Value::Bool(value)),
                    ("min", point(ui.item_rect_min())),
                    ("max", point(ui.item_rect_max())),
                ]))
            }
            "radio_button" => {
                let label = string_arg(arg, "label", op)?;
                let active = bool_arg(arg, "active", op)?;
                let clicked = ui.radio_button(label.as_str(), active);
                Ok(Value::record([
                    ("ok", Value::Bool(true)),
                    ("value", Value::Bool(clicked)),
                    ("min", point(ui.item_rect_min())),
                    ("max", point(ui.item_rect_max())),
                ]))
            }
            "input_text" => {
                let label = string_arg(arg, "label", op)?;
                let mut value = string_arg(arg, "value", op)?;
                let options = option_map(arg, op, &["hint", "password"])?;
                let hint = opt_string(options, "hint", op)?;
                let password = opt_bool(options, "password", op, false)?;
                ui.input_text(label.as_str(), &mut value)
                    .hint(hint.as_deref().unwrap_or(""))
                    .password(password)
                    .build();
                Ok(Value::record([
                    ("ok", Value::Bool(true)),
                    ("value", Value::from(value.as_str())),
                    ("min", point(ui.item_rect_min())),
                    ("max", point(ui.item_rect_max())),
                ]))
            }
            "input_text_multiline" => {
                let label = string_arg(arg, "label", op)?;
                let mut value = string_arg(arg, "value", op)?;
                let options = option_map(arg, op, &["width", "height"])?;
                let width = required_number(options, "width", op)? as f32;
                let height = required_number(options, "height", op)? as f32;
                ui.input_text_multiline(label.as_str(), &mut value, [width, height])
                    .build();
                Ok(Value::record([
                    ("ok", Value::Bool(true)),
                    ("value", Value::from(value.as_str())),
                    ("min", point(ui.item_rect_min())),
                    ("max", point(ui.item_rect_max())),
                ]))
            }
            "input_int" => {
                let label = string_arg(arg, "label", op)?;
                let value = number_arg(arg, "value", op)?;
                let options = option_map(arg, op, &["min", "max"])?;
                let min = required_number(options, "min", op)?;
                let max = required_number(options, "max", op)?;
                let mut edited = value.round() as i32;
                ui.input_int(label.as_str(), &mut edited);
                let clamped = (edited as f64).clamp(min, max);
                Ok(Value::record([
                    ("ok", Value::Bool(true)),
                    ("value", Value::from(clamped)),
                    ("min", point(ui.item_rect_min())),
                    ("max", point(ui.item_rect_max())),
                ]))
            }
            "input_float" => {
                let label = string_arg(arg, "label", op)?;
                let value = number_arg(arg, "value", op)?;
                let options = option_map(arg, op, &["min", "max"])?;
                let min = required_number(options, "min", op)?;
                let max = required_number(options, "max", op)?;
                let mut edited = value as f32;
                ui.input_float(label.as_str(), &mut edited);
                let clamped = (edited as f64).clamp(min, max);
                Ok(Value::record([
                    ("ok", Value::Bool(true)),
                    ("value", Value::from(clamped)),
                    ("min", point(ui.item_rect_min())),
                    ("max", point(ui.item_rect_max())),
                ]))
            }
            "slider_int" => {
                let label = string_arg(arg, "label", op)?;
                let value = number_arg(arg, "value", op)?;
                let options = option_map(arg, op, &["min", "max"])?;
                let min = required_number(options, "min", op)?;
                let max = required_number(options, "max", op)?;
                let mut edited = value.round() as i32;
                ui.slider_i32(
                    label.as_str(),
                    &mut edited,
                    min.round() as i32,
                    max.round() as i32,
                );
                let (track_min, track_max) = slider_rect(ui, true, min, max);
                Ok(Value::record([
                    ("ok", Value::Bool(true)),
                    ("value", Value::from(edited as f64)),
                    ("min", point(track_min)),
                    ("max", point(track_max)),
                ]))
            }
            "slider_float" => {
                let label = string_arg(arg, "label", op)?;
                let value = number_arg(arg, "value", op)?;
                let options = option_map(arg, op, &["min", "max"])?;
                let min = required_number(options, "min", op)?;
                let max = required_number(options, "max", op)?;
                let mut edited = value as f32;
                ui.slider_f32(label.as_str(), &mut edited, min as f32, max as f32);
                let (track_min, track_max) = slider_rect(ui, false, min, max);
                Ok(Value::record([
                    ("ok", Value::Bool(true)),
                    ("value", Value::from(edited as f64)),
                    ("min", point(track_min)),
                    ("max", point(track_max)),
                ]))
            }
            "drag_int" => {
                let label = string_arg(arg, "label", op)?;
                let value = number_arg(arg, "value", op)?;
                let options = option_map(arg, op, &["speed", "min", "max"])?;
                let speed = opt_number(options, "speed", op)?.unwrap_or(1.0) as f32;
                let min = opt_number(options, "min", op)?;
                let max = opt_number(options, "max", op)?;
                let mut edited = value.round() as i32;
                let mut drag = ui.drag_config(label.as_str());
                if let (Some(min), Some(max)) = (min, max) {
                    drag = drag.range(min.round() as i32, max.round() as i32);
                }
                drag.speed(speed).build(ui, &mut edited);
                Ok(Value::record([
                    ("ok", Value::Bool(true)),
                    ("value", Value::from(edited as f64)),
                    ("min", point(ui.item_rect_min())),
                    ("max", point(ui.item_rect_max())),
                ]))
            }
            "drag_float" => {
                let label = string_arg(arg, "label", op)?;
                let value = number_arg(arg, "value", op)?;
                let options = option_map(arg, op, &["speed", "min", "max"])?;
                let speed = opt_number(options, "speed", op)?.unwrap_or(1.0) as f32;
                let min = opt_number(options, "min", op)?;
                let max = opt_number(options, "max", op)?;
                let mut edited = value as f32;
                let mut drag = ui.drag_config(label.as_str());
                if let (Some(min), Some(max)) = (min, max) {
                    drag = drag.range(min as f32, max as f32);
                }
                drag.speed(speed).build(ui, &mut edited);
                Ok(Value::record([
                    ("ok", Value::Bool(true)),
                    ("value", Value::from(edited as f64)),
                    ("min", point(ui.item_rect_min())),
                    ("max", point(ui.item_rect_max())),
                ]))
            }
            "progress_bar" => {
                let fraction = number_arg(arg, "fraction", op)?;
                if !(0.0..=1.0).contains(&fraction) {
                    return Err(format!(
                        "ui.{op} needs a fraction from 0 to 1, not {fraction}."
                    ));
                }
                ui.progress_bar(fraction as f32).size([0.0, 0.0]).build();
                Ok(Value::record([
                    ("ok", Value::Bool(true)),
                    ("min", point(ui.item_rect_min())),
                    ("max", point(ui.item_rect_max())),
                ]))
            }
            "combo" => {
                let label = string_arg(arg, "label", op)?;
                let value = string_arg(arg, "value", op)?;
                let items = string_list_arg(arg, op)?;
                let mut current = items.iter().position(|item| *item == value).unwrap_or(0);
                let mut chosen = items.get(current).cloned().unwrap_or(value.clone());
                let token = ui.begin_combo(label.as_str(), chosen.as_str());
                let combo_min = ui.item_rect_min();
                let combo_max = ui.item_rect_max();
                let mut option_rects = Vec::new();
                if let Some(token) = token {
                    for (index, item) in items.iter().enumerate() {
                        let clicked = ui
                            .selectable_config(item.as_str())
                            .selected(index == current)
                            .build();
                        if clicked {
                            current = index;
                            chosen = item.clone();
                        }
                        option_rects.push(Value::record([
                            ("label", Value::from(item.as_str())),
                            ("min", point(ui.item_rect_min())),
                            ("max", point(ui.item_rect_max())),
                        ]));
                    }
                    token.end();
                }
                Ok(Value::record([
                    ("ok", Value::Bool(true)),
                    ("value", Value::from(chosen.as_str())),
                    ("open", Value::Bool(!option_rects.is_empty())),
                    ("items", crate::plugins::vlist(option_rects)),
                    ("min", point(combo_min)),
                    ("max", point(combo_max)),
                ]))
            }
            "selectable" => {
                if let Some(key) = unknown_key(
                    arg,
                    &[
                        "op",
                        "window",
                        "seq",
                        "label",
                        "selected",
                        "options",
                        "span_all_columns",
                    ],
                ) {
                    return Err(format!(
                        "ui.{op} can't use {key}: selectable takes label, selected and span_all_columns, like ui.selectable({{ label = \"Row A\", selected = true }})"
                    ));
                }
                let label = string_arg(arg, "label", op)?;
                let selected = bool_arg(arg, "selected", op)?;
                let options = option_map(arg, op, &["span_all_columns"])?;
                let span = opt_bool(options, "span_all_columns", op, false)?
                    || match arg.field("span_all_columns") {
                        Some(Value::Bool(value)) => *value,
                        None => false,
                        Some(other) => {
                            return Err(format!(
                                "ui.{op} can't use span_all_columns: it must be true or false, not {}.",
                                type_name(other)
                            ));
                        }
                    };
                let clicked = ui
                    .selectable_config(label.as_str())
                    .selected(selected)
                    .span_all_columns(span)
                    .build();
                Ok(Value::record([
                    ("ok", Value::Bool(true)),
                    ("value", Value::Bool(clicked)),
                    ("min", point(ui.item_rect_min())),
                    ("max", point(ui.item_rect_max())),
                ]))
            }
            "bullet_text" => {
                let text = string_arg(arg, "text", op)?;
                ui.bullet_text(text.as_str());
                Ok(Value::record([
                    ("ok", Value::Bool(true)),
                    ("min", point(ui.item_rect_min())),
                    ("max", point(ui.item_rect_max())),
                ]))
            }
            "invisible_button" => {
                if let Some(key) = unknown_key(
                    arg,
                    &["op", "window", "seq", "size", "width", "height", "id"],
                ) {
                    return Err(format!(
                        "ui.{op} can't use {key}: {op} takes an id and a size, like ui.{op}({{ id = \"drag\", size = {{ width = 300, height = 32 }} }})"
                    ));
                }
                let id = invisible_id(arg, op)?;
                let size = item_size(arg, op)?;
                let spacing = ui.push_style_var_y(StyleVarVec2::ItemSpacing, 0.0);
                let clicked = ui.invisible_button(id, size);
                spacing.pop();
                let hovered = ui.is_item_hovered();
                let active = ui.is_item_active();
                let min = ui.item_rect_min();
                let max = ui.item_rect_max();
                Ok(Value::record([
                    ("ok", Value::Bool(true)),
                    ("clicked", Value::Bool(clicked)),
                    ("hovered", Value::Bool(hovered)),
                    ("active", Value::Bool(active)),
                    ("min", point(min)),
                    ("max", point(max)),
                ]))
            }
            "is_item_hovered" => Ok(Value::record([
                ("ok", Value::Bool(true)),
                ("value", Value::Bool(ui.is_item_hovered())),
            ])),
            "is_item_active" => Ok(Value::record([
                ("ok", Value::Bool(true)),
                ("value", Value::Bool(ui.is_item_active())),
            ])),
            "mouse_delta" => {
                let delta = ui.io().mouse_delta();
                Ok(Value::record([
                    ("ok", Value::Bool(true)),
                    ("x", Value::from(f64::from(delta[0]))),
                    ("y", Value::from(f64::from(delta[1]))),
                ]))
            }
            "is_mouse_dragging" => {
                let button = mouse_button_arg(arg, op)?;
                Ok(Value::record([
                    ("ok", Value::Bool(true)),
                    ("value", Value::Bool(ui.is_mouse_dragging(button))),
                ]))
            }
            "begin_table" => begin_table(ui, frame, arg, op),
            "table_setup_column" => table_setup_column(ui, frame, arg, op),
            "table_headers_row" => table_headers_row(ui, frame, op),
            "table_next_row" => table_next_row(ui, frame, op),
            "table_next_column" => table_next_column(ui, frame, op),
            "table_sort_specs" => table_sort_specs(ui, frame, op),
            "end_table" => end_table(ui, frame, op),
            "begin_tab_bar" => begin_tab_bar(ui, frame, arg, op),
            "begin_tab_item" => begin_tab_item(ui, frame, arg, op),
            "end_tab_item" => end_tab_item(ui, frame, op),
            "end_tab_bar" => end_tab_bar(ui, frame, op),
            "begin_menu_bar" => begin_menu_bar(ui, frame, op),
            "end_menu_bar" => end_menu_bar(ui, frame, op),
            "begin_menu" => begin_menu(ui, frame, arg, op),
            "end_menu" => end_menu(ui, frame, op),
            "menu_item" => menu_item(ui, arg, op),
            "open_popup" => open_popup(ui, arg, op),
            "begin_popup" => begin_popup(ui, frame, arg, op),
            "begin_popup_modal" => begin_popup_modal(ui, frame, arg, op),
            "begin_popup_context_item" => begin_popup_context_item(ui, frame, arg, op),
            "end_popup" => end_popup(ui, frame, op),
            "set_tooltip" => set_tooltip(ui, arg, op),
            "tree_node" => tree_node(ui, frame, arg, op),
            "tree_pop" => tree_pop(ui, frame, op),
            "collapsing_header" => collapsing_header(ui, arg, op),
            "begin_list_box" => begin_list_box(ui, frame, arg, op),
            "end_list_box" => end_list_box(ui, frame, op),
            "color_edit3" => color_edit3(ui, arg, op),
            "color_button" => color_button(ui, arg, op),
            "begin_drag_drop_source" => begin_drag_drop_source(ui, frame, op),
            "set_drag_drop_payload" => set_drag_drop_payload(self, ui, frame, arg, op),
            "end_drag_drop_source" => end_drag_drop_source(ui, frame, op),
            "begin_drag_drop_target" => begin_drag_drop_target(ui, frame, op),
            "accept_drag_drop_payload" => accept_drag_drop_payload(self, ui, frame, arg, op),
            "end_drag_drop_target" => end_drag_drop_target(ui, frame, op),
            "shortcut" => shortcut(ui, arg, op),
            "date_picker" | "time_picker" => Err(unavailable_widget(op)),
            _ => Err(format!("zeron.imgui has no widget \"{op}\".")),
        }
    }

    /// Ends the frame the pump opened: paints it when it is clean, throws it
    /// away with the reason when it is not, and records the outcome.
    pub fn conclude(
        &self,
        window: &impl FrameWindow,
        prepared: Result<PreparedFrame, String>,
    ) -> FrameEnd {
        let Some(frame) = self.live.borrow_mut().take() else {
            let end = FrameEnd {
                seq: self.seq.get(),
                painted: false,
                error: Some("No frame was in flight.".to_string()),
                first: false,
                refused: false,
                consecutive_errors: self.consecutive.get(),
                ms: 0.0,
            };
            *self.pending.borrow_mut() = Some(end.clone());
            return end;
        };
        let seq = frame.seq;
        let ms = frame.started_wall.elapsed().as_secs_f64().max(0.0) * 1000.0;
        let first = frame.first;
        let refused = frame.refused;
        // The prepared frame is the authority on an atlas upload: a new glyph
        // bakes one mid-frame, after `frame_needs_atlas` was asked. That copy
        // is the host's work too, so the frame is excused as well.
        let uploaded_atlas = prepared
            .as_ref()
            .ok()
            .is_some_and(|prepared| prepared.atlas().is_some());
        let over_budget = !(frame.exempt || uploaded_atlas)
            && (frame.over_budget || frame.started_wall.elapsed() > BUDGET);
        let failure = if over_budget {
            Some(budget_sentence(window.frame_id()))
        } else if !frame.finished {
            Some("The render callback ended without finishing its frame.".to_string())
        } else {
            frame.error
        };
        let end = match failure {
            // A widget the host refused is the plugin's own drawing mistake:
            // already said where it happened, and it keeps the plugin loaded.
            Some(error) if refused => self.record_refusal(&error, first, ms),
            Some(error) => self.record_failure(&error, first, ms),
            None => match prepared {
                Ok(prepared) => {
                    window.frame_deliver(prepared);
                    self.frames.set(self.frames.get() + 1);
                    self.consecutive.set(0);
                    self.last_ms.set(ms);
                    FrameEnd {
                        seq,
                        painted: true,
                        error: None,
                        first,
                        refused: false,
                        consecutive_errors: 0,
                        ms,
                    }
                }
                Err(error) => self.record_failure(&error, first, ms),
            },
        };
        *self.pending.borrow_mut() = Some(end.clone());
        end
    }

    /// Ends a frame whose draw closure panicked.
    ///
    /// The context half of the unwind is [`runtime::build_frame`]'s job (it
    /// always suspends), so this only records the outcome. A panic is not a
    /// script error: [`run`] reports it and, after
    /// [`MAX_CONSECUTIVE_ERRORS`], closes the window, but the plugin is never
    /// withdrawn for one — a bug in the host must not unload a plugin.
    pub fn conclude_panic(&self, _window: &impl FrameWindow, error: String) -> FrameEnd {
        let Some(frame) = self.live.borrow_mut().take() else {
            let end = FrameEnd {
                seq: self.seq.get(),
                painted: false,
                error: Some(error),
                first: false,
                refused: false,
                consecutive_errors: self.consecutive.get(),
                ms: 0.0,
            };
            *self.pending.borrow_mut() = Some(end.clone());
            return end;
        };
        let ms = frame.started_wall.elapsed().as_secs_f64().max(0.0) * 1000.0;
        let first = frame.first;
        let end = self.record_failure(&error, first, ms);
        *self.pending.borrow_mut() = Some(end.clone());
        end
    }

    /// The end recorded by the last [`conclude`](Self::conclude), once.
    pub fn take_end(&self) -> Option<FrameEnd> {
        self.pending.borrow_mut().take()
    }

    fn record_failure(&self, error: &str, first: bool, ms: f64) -> FrameEnd {
        self.errors.set(self.errors.get() + 1);
        self.consecutive.set(self.consecutive.get() + 1);
        *self.last_error.borrow_mut() = Some(error.to_string());
        if first {
            let mut first_error = self.first_error.borrow_mut();
            if first_error.is_none() {
                *first_error = Some(error.to_string());
            }
        }
        self.last_ms.set(ms);
        FrameEnd {
            seq: self.seq.get(),
            painted: false,
            error: Some(error.to_string()),
            first,
            refused: false,
            consecutive_errors: self.consecutive.get(),
            ms,
        }
    }

    /// A frame a widget call the host refused brought down: counted and
    /// recorded, but it neither paints, nor counts toward the failed frames
    /// in a row that close a window, nor ends a window's run on its first
    /// frame. Redrawing without the bad call is the plugin author's fix.
    fn record_refusal(&self, error: &str, first: bool, ms: f64) -> FrameEnd {
        self.errors.set(self.errors.get() + 1);
        *self.last_error.borrow_mut() = Some(error.to_string());
        if first {
            let mut first_error = self.first_error.borrow_mut();
            if first_error.is_none() {
                *first_error = Some(error.to_string());
            }
        }
        self.last_ms.set(ms);
        FrameEnd {
            seq: self.seq.get(),
            painted: false,
            error: Some(error.to_string()),
            first,
            refused: true,
            consecutive_errors: self.consecutive.get(),
            ms,
        }
    }
}

/// Queues one call for a plugin: its id, its kind, and its fields.
pub type PushCall = dyn Fn(String, &'static str, Vec<(&'static str, Value)>);

/// How the pump hands a plugin one frame request, and takes it back.
///
/// The bridge's own queue is the production path; a dev plugin gets one made
/// over its `plugin.<id>.calls` shared state, so the same pump serves both.
#[derive(Clone)]
pub struct FrameCalls {
    push: Rc<PushCall>,
    take: Rc<dyn Fn(&str)>,
    failed: Rc<dyn Fn(&str)>,
    refused: Rc<dyn Fn(&str)>,
}

impl FrameCalls {
    pub fn new(
        push: impl Fn(String, &'static str, Vec<(&'static str, Value)>) + 'static,
        take: impl Fn(&str) + 'static,
        failed: impl Fn(&str) + 'static,
    ) -> Self {
        Self {
            push: Rc::new(push),
            take: Rc::new(take),
            failed: Rc::new(failed),
            refused: Rc::new(|_| {}),
        }
    }

    pub fn with_refused(mut self, refused: impl Fn(&str) + 'static) -> Self {
        self.refused = Rc::new(refused);
        self
    }

    pub fn report_refused(&self, error: &str) {
        (self.refused)(error)
    }

    /// Queues a call for the plugin. What the plugin does with it runs before
    /// this returns (the SDK drains the queue synchronously).
    pub fn push(&self, id: String, kind: &'static str, fields: Vec<(&'static str, Value)>) {
        (self.push)(id, kind, fields);
    }

    /// Drops a call the plugin never answered (or already did).
    pub fn take(&self, id: &str) {
        (self.take)(id)
    }

    /// Reports a plugin whose window could not draw its first frame, in a
    /// sentence naming the window and the script's own words. The bridge
    /// withdraws the plugin; a dev plugin writes it to the log.
    pub fn failed(&self, error: &str) {
        (self.failed)(error)
    }
}

/// Runs one frame of one window: begin, push the request, end, report.
///
/// The plugin's `on_render` runs inside [`FrameCalls::push`], on this stack,
/// while the frame is open; by the time it returns the frame has been painted
/// or aborted. A callback that never answers its `imgui_end` (an error outside
/// the SDK's guard) has its frame aborted with a sentence saying so. A panic
/// anywhere inside the draw is caught here: it fails this window's frame
/// without taking the pump — or any other window — down with it.
pub fn run(window: &impl FrameWindow, calls: &FrameCalls) -> Option<FrameEnd> {
    if !window.frame_open() || !window.frame_shown() || !window.frame_renderable() {
        return None;
    }
    let (seq, spec) = match window.frame_prepare() {
        Ok(prepared) => prepared,
        Err(error) => {
            eprintln!(
                "zeron-zgx: plugin {}: window \"{}\": {error}",
                window.frame_plugin(),
                window.frame_id()
            );
            return None;
        }
    };
    let call = format!("imgui-frame-{}-{seq}", window.frame_id());
    // A panic inside the draw (which includes the script's `on_render` and
    // any host code it reaches) must not kill the pump: the pump is one task
    // for every plugin window, so an escaping panic would freeze them all
    // until restart. Catch it here, where the frame boundary is, and let the
    // failure path below treat it as this window's failed frame. The context
    // is already suspended again by `build_frame`'s unwinding guard.
    let attempt = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        window.frame_build(spec, |ui| {
            window.frame_set_ui(ui);
            calls.push(
                call.clone(),
                "imgui.frame",
                vec![
                    ("window", Value::from(window.frame_id())),
                    ("seq", Value::from(seq as f64)),
                    ("budget", Value::from(BUDGET_MS)),
                ],
            );
            if let Some(error) = window.frame_close_regions(ui) {
                calls.report_refused(&error);
            }
        })
    }));
    let (built, panic) = match attempt {
        Ok(built) => (built, None),
        Err(payload) => {
            let sentence = panic_sentence(window.frame_id(), payload.as_ref());
            (Err(sentence.clone()), Some(sentence))
        }
    };
    let end = match &panic {
        Some(error) => window.frame_conclude_panic(error.clone()),
        None => window.frame_conclude(built),
    };
    window.frame_after_run();
    calls.take(&call);
    if let Some(error) = &end.error {
        if end.refused {
            // Said where the call was made (the widget op reports each
            // refusal naming its widget), so the pump only records it.
        } else if panic.is_some() {
            // A host-side panic fails this window's frame. It is reported
            // and, after the same run of failures, closes the window; the
            // plugin itself stays loaded and its other windows keep drawing.
            eprintln!(
                "zeron-zgx: plugin {}: window \"{}\": {error}",
                window.frame_plugin(),
                window.frame_id()
            );
            if end.consecutive_errors >= MAX_CONSECUTIVE_ERRORS {
                eprintln!(
                    "zeron-zgx: plugin {}: closing window \"{}\" after {} failed frames",
                    window.frame_plugin(),
                    window.frame_id(),
                    end.consecutive_errors
                );
                window.frame_close();
            }
        } else if end.first {
            // A window that cannot draw its first attempted frame is a window
            // that never becomes visible: retrying cannot help, so this is
            // terminal. Report it once, with the window's id and the script's
            // own words, hand it to the bridge when there is one (which
            // withdraws the plugin), and close the window on both routes —
            // even when that report has no plugin manager to reach.
            let sentence = first_frame_sentence(window.frame_id(), error);
            calls.failed(&sentence);
            window.frame_close();
        } else {
            eprintln!(
                "zeron-zgx: plugin {}: window \"{}\": {error}",
                window.frame_plugin(),
                window.frame_id()
            );
            if end.consecutive_errors >= MAX_CONSECUTIVE_ERRORS {
                eprintln!(
                    "zeron-zgx: plugin {}: closing window \"{}\" after {} failed frames",
                    window.frame_plugin(),
                    window.frame_id(),
                    end.consecutive_errors
                );
                window.frame_close();
            }
        }
    }
    Some(end)
}

/// The sentence a plugin gets for an overrun callback.
pub fn budget_sentence(window: &str) -> String {
    format!(
        "Window \"{window}\"'s render callback took too long: a frame may spend {BUDGET_MS:.0} ms of CPU. Draw less, or do the work outside on_render."
    )
}

/// The sentence a first-frame failure is reported with: which window, and
/// the script's own words. One window's failure never hides another's.
fn first_frame_sentence(window: &str, error: &str) -> String {
    format!("Window \"{window}\"'s first frame failed: {error}")
}

/// The sentence a panicking draw is recorded with. The payload is whatever
/// the panic carried: a `&str`, a `String`, or nothing readable.
fn panic_sentence(window: &str, payload: &(dyn std::any::Any + Send)) -> String {
    let message = payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("no message");
    format!("Window \"{window}\"'s render callback panicked: {message}")
}

fn point(at: [f32; 2]) -> Value {
    Value::record([
        ("x", Value::from(f64::from(at[0]))),
        ("y", Value::from(f64::from(at[1]))),
    ])
}

fn type_name(value: &Value) -> &'static str {
    match value {
        Value::Nil => "nil",
        Value::Bool(_) => "a boolean",
        Value::Num(_) => "a number",
        Value::Str(_) | Value::Text(_) => "a string",
        Value::List(_) => "a list",
        Value::Map(_) | Value::Folded(_) => "a table",
        Value::Element(_) => "an element",
        Value::Image(_) => "an image",
        _ => "a value",
    }
}

fn string_arg(arg: &Value, field: &str, op: &str) -> Result<String, String> {
    match arg.field(field) {
        Some(Value::Str(text)) => Ok(text.to_string()),
        Some(Value::Text(text)) => Ok(text.to_string()),
        Some(other) => Err(format!(
            "ui.{op} needs {field} to be text, not {}.",
            type_name(other)
        )),
        None => Err(format!(
            "ui.{op} needs {field} to draw, like ui.{op}(\"Hello\")."
        )),
    }
}

fn number_arg(arg: &Value, field: &str, op: &str) -> Result<f64, String> {
    match arg.field(field) {
        Some(Value::Num(value)) if value.is_finite() => Ok(*value),
        Some(Value::Num(_)) => Err(format!("ui.{op} needs {field} to be a finite number.")),
        Some(other) => Err(format!(
            "ui.{op} needs {field} to be a number, not {}.",
            type_name(other)
        )),
        None => Err(format!("ui.{op} needs {field} to be a number.")),
    }
}

fn bool_arg(arg: &Value, field: &str, op: &str) -> Result<bool, String> {
    match arg.field(field) {
        None => Ok(false),
        Some(Value::Bool(value)) => Ok(*value),
        Some(other) => Err(format!(
            "ui.{op} needs {field} to be true or false, not {}.",
            type_name(other)
        )),
    }
}

fn colour_arg(arg: &Value, op: &str) -> Result<[f32; 4], String> {
    match arg.field("colour") {
        Some(Value::Str(text)) => parse_colour(op, text),
        Some(Value::Text(text)) => parse_colour(op, &text.to_string()),
        Some(other) => Err(format!(
            "ui.{op} needs colour to be \"#rrggbb\" or \"#rrggbbaa\", not {}.",
            type_name(other)
        )),
        None => Err(format!(
            "ui.{op} needs a colour, like ui.{op}(\"Alert\", \"#ff0000\")."
        )),
    }
}

fn parse_colour(op: &str, text: &str) -> Result<[f32; 4], String> {
    let Some(digits) = text.strip_prefix('#') else {
        return Err(format!(
            "ui.{op} can't use colour \"{text}\": colour must be \"#rrggbb\" or \"#rrggbbaa\"."
        ));
    };
    if (digits.len() != 6 && digits.len() != 8)
        || !digits.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(format!(
            "ui.{op} can't use colour \"{text}\": colour must be \"#rrggbb\" or \"#rrggbbaa\"."
        ));
    }
    let component = |at: usize| {
        u8::from_str_radix(&digits[at..at + 2], 16).expect("two hex digits") as f32 / 255.0
    };
    let alpha = if digits.len() == 8 { component(6) } else { 1.0 };
    Ok([component(0), component(2), component(4), alpha])
}

fn is_empty_map(value: &Value) -> bool {
    matches!(value, Value::List(items) if items.is_empty())
}

fn option_map<'a>(arg: &'a Value, op: &str, allowed: &[&str]) -> Result<Option<&'a Value>, String> {
    match arg.field("options") {
        None => Ok(None),
        Some(options) if matches!(options, Value::Map(_)) || is_empty_map(options) => {
            if let Some(key) = unknown_key(options, allowed) {
                return Err(format!(
                    "ui.{op} can't use option \"{key}\": {op} accepts {}.",
                    accepted_keys(allowed)
                ));
            }
            Ok(Some(options))
        }
        Some(other) => Err(format!(
            "ui.{op} can't use options: options must be a table, not {}.",
            type_name(other)
        )),
    }
}

fn opt_string(options: Option<&Value>, key: &str, op: &str) -> Result<Option<String>, String> {
    match options.and_then(|options| options.field(key)) {
        None => Ok(None),
        Some(Value::Str(text)) => Ok(Some(text.to_string())),
        Some(Value::Text(text)) => Ok(Some(text.to_string())),
        Some(other) => Err(format!(
            "ui.{op} can't use {key}: {key} must be text, not {}.",
            type_name(other)
        )),
    }
}

fn opt_bool(options: Option<&Value>, key: &str, op: &str, default: bool) -> Result<bool, String> {
    match options.and_then(|options| options.field(key)) {
        None => Ok(default),
        Some(Value::Bool(value)) => Ok(*value),
        Some(other) => Err(format!(
            "ui.{op} can't use {key}: {key} must be true or false, not {}.",
            type_name(other)
        )),
    }
}

fn opt_number(options: Option<&Value>, key: &str, op: &str) -> Result<Option<f64>, String> {
    match options.and_then(|options| options.field(key)) {
        None => Ok(None),
        Some(Value::Num(value)) if value.is_finite() => Ok(Some(*value)),
        Some(Value::Num(_)) => Err(format!(
            "ui.{op} can't use {key}: {key} must be a finite number."
        )),
        Some(other) => Err(format!(
            "ui.{op} can't use {key}: {key} must be a number, not {}.",
            type_name(other)
        )),
    }
}

fn required_number(options: Option<&Value>, key: &str, op: &str) -> Result<f64, String> {
    match opt_number(options, key, op)? {
        Some(value) => Ok(value),
        None => Err(format!(
            "ui.{op} needs {key} in its options, like {{ {key} = 0 }}."
        )),
    }
}

fn string_list_arg(arg: &Value, op: &str) -> Result<Vec<String>, String> {
    match arg.field("options") {
        Some(Value::List(items)) if !items.is_empty() => items
            .iter()
            .map(|item| match item {
                Value::Str(text) => Ok(text.to_string()),
                Value::Text(text) => Ok(text.to_string()),
                other => Err(format!(
                    "ui.{op} needs every option to be text, not {}.",
                    type_name(other)
                )),
            })
            .collect(),
        Some(Value::List(_)) => Err(format!(
            "ui.{op} needs at least one option, like ui.{op}(\"theme\", current, {{ \"dark\", \"light\" }})."
        )),
        Some(other) => Err(format!(
            "ui.{op} needs options to be a list of text, not {}.",
            type_name(other)
        )),
        None => Err(format!(
            "ui.{op} needs options, like ui.{op}(\"theme\", current, {{ \"dark\", \"light\" }})."
        )),
    }
}

fn accepted_keys(allowed: &[&str]) -> String {
    match allowed {
        [] => "no options".to_string(),
        [only] => format!("\"{only}\""),
        [head @ .., last] => {
            let mut keys = head
                .iter()
                .map(|key| format!("\"{key}\""))
                .collect::<Vec<_>>()
                .join(", ");
            keys.push_str(&format!(" and \"{last}\""));
            keys
        }
    }
}

fn slider_rect(ui: &Ui, is_int: bool, min: f64, max: f64) -> ([f32; 2], [f32; 2]) {
    const GRAB_PADDING: f32 = 2.0;
    let frame_min = ui.item_rect_min();
    let frame_max_x = frame_min[0] + ui.calc_item_width();
    let mut grab = ui.clone_style().grab_min_size();
    let slider_sz = (frame_max_x - frame_min[0]) - 2.0 * GRAB_PADDING;
    if is_int {
        let range = (max - min).abs() as f32;
        if range >= 0.0 {
            grab = grab.max(slider_sz / (range + 1.0));
        }
    }
    grab = grab.min(slider_sz).max(0.0);
    let usable_min = frame_min[0] + GRAB_PADDING + grab * 0.5;
    let usable_max = frame_max_x - GRAB_PADDING - grab * 0.5;
    (
        [usable_min, frame_min[1]],
        [usable_max, ui.item_rect_max()[1]],
    )
}

fn unknown_key(value: &Value, allowed: &[&str]) -> Option<String> {
    let Value::Map(fields) = value else {
        return None;
    };
    fields
        .iter()
        .map(|(key, _)| key.as_ref())
        .find(|key| !allowed.contains(key))
        .map(str::to_string)
}

fn invisible_id(arg: &Value, op: &str) -> Result<String, String> {
    match arg.field("id") {
        Some(Value::Str(id)) if !id.is_empty() => Ok(id.to_string()),
        Some(Value::Str(_)) => Err(format!("ui.{op} can't use id: id can't be empty")),
        Some(_) => Err(format!(
            "ui.{op} can't use id: id must be text, like \"drag\""
        )),
        None => Err(format!(
            "ui.{op} needs an id, like ui.{op}({{ id = \"drag\", size = {{ width = 300, height = 32 }} }})"
        )),
    }
}

fn item_size(arg: &Value, op: &str) -> Result<[f32; 2], String> {
    let from = match arg.field("size") {
        None => arg,
        Some(size @ Value::Map(_)) => size,
        Some(_) => {
            return Err(format!(
                "ui.{op} can't use size: size must be a table with width and height"
            ));
        }
    };
    let axis = |name: &str| -> Result<f32, String> {
        match from.field(name) {
            Some(Value::Num(number)) if number.is_finite() && *number > 0.0 => Ok(*number as f32),
            Some(Value::Num(_)) => Err(format!(
                "ui.{op} can't use {name}: {name} must be more than zero"
            )),
            Some(_) => Err(format!("ui.{op} can't use {name}: {name} must be a number")),
            None => Err(format!(
                "ui.{op} needs a size, like ui.{op}({{ id = \"drag\", size = {{ width = 300, height = 32 }} }})"
            )),
        }
    };
    Ok([axis("width")?, axis("height")?])
}

fn mouse_button_arg(arg: &Value, op: &str) -> Result<MouseButton, String> {
    match arg.field("button") {
        None => Ok(MouseButton::Left),
        Some(Value::Str(name)) => match name.as_ref() {
            "left" => Ok(MouseButton::Left),
            "right" => Ok(MouseButton::Right),
            "middle" => Ok(MouseButton::Middle),
            "extra1" => Ok(MouseButton::Extra1),
            "extra2" => Ok(MouseButton::Extra2),
            other => Err(format!(
                "ui.{op} can't use button \"{other}\": button must be \"left\", \"right\", \"middle\", \"extra1\" or \"extra2\""
            )),
        },
        Some(_) => Err(format!(
            "ui.{op} can't use button: button must be \"left\", \"right\", \"middle\", \"extra1\" or \"extra2\""
        )),
    }
}

fn size_fields(arg: &Value, op: &str, example: &str) -> Result<[f32; 2], String> {
    let axis = |name: &str| -> Result<f32, String> {
        match arg.field(name) {
            Some(Value::Num(number)) if number.is_finite() && *number > 0.0 => Ok(*number as f32),
            Some(Value::Num(_)) => Err(format!(
                "ui.{op} can't use {name}: {name} must be more than zero"
            )),
            Some(other) => Err(format!(
                "ui.{op} can't use {name}: {name} must be a number, not {}.",
                type_name(other)
            )),
            None => Err(format!("ui.{op} needs {name}, like {example}")),
        }
    };
    Ok([axis("width")?, axis("height")?])
}

fn child_id(arg: &Value, op: &str) -> Result<String, String> {
    match arg.field("id") {
        Some(Value::Str(id)) if !id.is_empty() => Ok(id.to_string()),
        Some(Value::Str(_)) => Err(format!("ui.{op} can't use id: id can't be empty")),
        Some(other) => Err(format!(
            "ui.{op} can't use id: id must be text, not {}.",
            type_name(other)
        )),
        None => Err(format!(
            "ui.{op} needs an id, like ui.begin_child(\"log\", {{ width = 240, height = 80 }})"
        )),
    }
}

fn unbalanced_region_sentence(open: &str, close: &str) -> String {
    format!(
        "ui.{open} was called without ui.{close}; close the region before the callback returns."
    )
}

fn begin_child(ui: &Ui, frame: &mut LiveFrame, arg: &Value, op: &str) -> Result<Value, String> {
    if let Some(key) = unknown_key(
        arg,
        &["op", "window", "seq", "id", "width", "height", "border"],
    ) {
        return Err(format!(
            "ui.{op} can't use {key}: begin_child takes an id and a size, like ui.begin_child(\"log\", {{ width = 240, height = 80 }})"
        ));
    }
    if frame.child_open {
        return Err(format!(
            "ui.{op} was called while another child region is open; close it with ui.end_child first."
        ));
    }
    let id = child_id(arg, op)?;
    let size = size_fields(
        arg,
        op,
        "ui.begin_child(\"log\", { width = 240, height = 80 })",
    )?;
    let border = match arg.field("border") {
        Some(Value::Bool(value)) => *value,
        Some(other) => {
            return Err(format!(
                "ui.{op} can't use border: border must be true or false, not {}.",
                type_name(other)
            ));
        }
        None => false,
    };
    let id =
        CString::new(id).map_err(|_| format!("ui.{op} can't use id: id can't hold NUL bytes"))?;
    let flags = if border {
        dear_imgui_rs::sys::ImGuiChildFlags_Borders
    } else {
        dear_imgui_rs::sys::ImGuiChildFlags_None
    };
    let size_sys = dear_imgui_rs::sys::ImVec2_c {
        x: size[0],
        y: size[1],
    };
    let began = ui.with_bound_context(|| unsafe {
        dear_imgui_rs::sys::igBeginChild_Str(id.as_ptr(), size_sys, flags, 0)
    });
    if !began {
        ui.with_bound_context(|| unsafe { dear_imgui_rs::sys::igEndChild() });
        return Err(format!("ui.{op} couldn't open the child region here."));
    }
    frame.child_open = true;
    let min = ui.window_pos();
    let max = [min[0] + ui.window_size()[0], min[1] + ui.window_size()[1]];
    Ok(Value::record([
        ("ok", Value::Bool(true)),
        ("min", point(min)),
        ("max", point(max)),
    ]))
}

fn end_child(ui: &Ui, frame: &mut LiveFrame, op: &str) -> Result<Value, String> {
    if !frame.child_open {
        return Err(format!(
            "ui.{op} was called without a ui.begin_child to close."
        ));
    }
    frame.child_open = false;
    ui.with_bound_context(|| unsafe { dear_imgui_rs::sys::igEndChild() });
    Ok(Value::record([("ok", Value::Bool(true))]))
}

fn columns(ui: &Ui, frame: &mut LiveFrame, arg: &Value, op: &str) -> Result<Value, String> {
    if let Some(key) = unknown_key(arg, &["op", "window", "seq", "count"]) {
        return Err(format!(
            "ui.{op} can't use {key}: columns takes a count, like ui.columns(2)"
        ));
    }
    let count = number_arg(arg, "count", op)?;
    if count.fract() != 0.0 || !(1.0..=64.0).contains(&count) {
        return Err(format!(
            "ui.{op} needs count to be a whole number from 1 to 64, not {count}."
        ));
    }
    if count == 1.0 {
        if frame.columns_open {
            ui.with_bound_context(|| unsafe {
                dear_imgui_rs::sys::igColumns(1, std::ptr::null(), false)
            });
            frame.columns_open = false;
        }
        return Ok(Value::record([("ok", Value::Bool(true))]));
    }
    if frame.columns_open {
        return Err(format!(
            "ui.{op} was called while a columns block is open; call ui.next_column until the last column."
        ));
    }
    ui.with_bound_context(|| unsafe {
        dear_imgui_rs::sys::igColumns(count as i32, std::ptr::null(), false)
    });
    frame.columns_open = true;
    Ok(Value::record([("ok", Value::Bool(true))]))
}

fn next_column(ui: &Ui, frame: &mut LiveFrame, op: &str) -> Result<Value, String> {
    if !frame.columns_open {
        return Err(format!("ui.{op} was called outside a ui.columns block."));
    }
    ui.with_bound_context(|| unsafe { dear_imgui_rs::sys::igNextColumn() });
    Ok(Value::record([("ok", Value::Bool(true))]))
}

fn begin_group(ui: &Ui, frame: &mut LiveFrame, op: &str) -> Result<Value, String> {
    if frame.group_open {
        return Err(format!(
            "ui.{op} was called while another group is open; close it with ui.end_group first."
        ));
    }
    ui.with_bound_context(|| unsafe { dear_imgui_rs::sys::igBeginGroup() });
    frame.group_open = true;
    Ok(Value::record([("ok", Value::Bool(true))]))
}

fn end_group(ui: &Ui, frame: &mut LiveFrame, op: &str) -> Result<Value, String> {
    if !frame.group_open {
        return Err(format!(
            "ui.{op} was called without a ui.begin_group to close."
        ));
    }
    frame.group_open = false;
    ui.with_bound_context(|| unsafe { dear_imgui_rs::sys::igEndGroup() });
    Ok(Value::record([
        ("ok", Value::Bool(true)),
        ("min", point(ui.item_rect_min())),
        ("max", point(ui.item_rect_max())),
    ]))
}

fn c_label(value: &str) -> Result<CString, String> {
    CString::new(value)
        .map_err(|_| format!("zeron.imgui can't use a label with a NUL byte in it"))
}

fn table_flags(arg: &Value, op: &str) -> Result<i32, String> {
    let mut flags = 0_i32;
    let Some(items) = arg.field("flags") else {
        return Ok(flags);
    };
    let Value::List(items) = items else {
        return Err(format!(
            "ui.{op} can't use flags: flags must be a list of text, not {}.",
            type_name(items)
        ));
    };
    for item in items.iter() {
        let name = match item {
            Value::Str(text) => text.to_string(),
            Value::Text(text) => text.to_string(),
            _ => {
                return Err(format!(
                    "ui.{op} can't use flags: every flag must be text, like \"borders\"."
                ));
            }
        };
        let bit = match name.as_str() {
            "borders" => dear_imgui_rs::sys::ImGuiTableFlags_Borders,
            "row_bg" => dear_imgui_rs::sys::ImGuiTableFlags_RowBg,
            "resizable" => dear_imgui_rs::sys::ImGuiTableFlags_Resizable,
            "sortable" => dear_imgui_rs::sys::ImGuiTableFlags_Sortable,
            "borders_outer_v" => dear_imgui_rs::sys::ImGuiTableFlags_BordersOuterV,
            "borders_inner_v" => dear_imgui_rs::sys::ImGuiTableFlags_BordersInnerV,
            "borders_outer_h" => dear_imgui_rs::sys::ImGuiTableFlags_BordersOuterH,
            "borders_inner_h" => dear_imgui_rs::sys::ImGuiTableFlags_BordersInnerH,
            "no_pad_outer_x" => dear_imgui_rs::sys::ImGuiTableFlags_NoPadOuterX,
            other => {
                return Err(format!(
                    "ui.{op} can't use flag \"{other}\": {op} accepts borders, row_bg, resizable, sortable and the border_* flags."
                ));
            }
        };
        flags |= bit;
    }
    Ok(flags)
}

fn begin_table(ui: &Ui, frame: &mut LiveFrame, arg: &Value, op: &str) -> Result<Value, String> {
    if let Some(key) = unknown_key(arg, &["op", "window", "seq", "id", "columns", "flags"]) {
        return Err(format!(
            "ui.{op} can't use {key}: {op} takes id, columns and flags, like ui.begin_table({{ id = \"files\", columns = 3, flags = {{ \"borders\" }} }})"
        ));
    }
    let id = string_arg(arg, "id", op)?;
    let id = c_label(&id).map_err(|error| format!("ui.{op} can't use id: {error}"))?;
    let columns = number_arg(arg, "columns", op)?;
    if columns.fract() != 0.0 || !(1.0..=64.0).contains(&columns) {
        return Err(format!(
            "ui.{op} needs columns to be a whole number from 1 to 64, not {columns}."
        ));
    }
    let flags = table_flags(arg, op)?;
    if !frame.tables.is_empty() && frame.tables.last().is_some_and(|table| table.open) {
        return Err(format!(
            "ui.{op} was called inside another table; end it with ui.end_table first."
        ));
    }
    let open = ui.with_bound_context(|| unsafe {
        dear_imgui_rs::sys::igBeginTable(
            id.as_ptr(),
            columns as i32,
            flags,
            dear_imgui_rs::sys::ImVec2_c { x: 0.0, y: 0.0 },
            0.0,
        )
    });
    let min = ui.cursor_pos();
    frame.tables.push(TableState {
        open,
        columns: Vec::new(),
    });
    Ok(Value::record([
        ("ok", Value::Bool(true)),
        ("open", Value::Bool(open)),
        ("min", point(min)),
    ]))
}

fn current_table_open(frame: &LiveFrame) -> bool {
    frame.tables.last().is_some_and(|table| table.open)
}

fn table_setup_column(
    ui: &Ui,
    frame: &mut LiveFrame,
    arg: &Value,
    op: &str,
) -> Result<Value, String> {
    if let Some(key) = unknown_key(
        arg,
        &["op", "window", "seq", "label", "width", "sortable"],
    ) {
        return Err(format!(
            "ui.{op} can't use {key}: {op} takes label, width and sortable, like ui.table_setup_column({{ label = \"Name\", width = 200 }})"
        ));
    }
    let label = string_arg(arg, "label", op)?;
    let sortable = match arg.field("sortable") {
        Some(Value::Bool(value)) => *value,
        None => true,
        Some(other) => {
            return Err(format!(
                "ui.{op} can't use sortable: sortable must be true or false, not {}.",
                type_name(other)
            ));
        }
    };
    let width = match arg.field("width") {
        Some(Value::Num(number)) if number.is_finite() && *number > 0.0 => Some(*number as f32),
        Some(Value::Num(_)) => {
            return Err(format!(
                "ui.{op} can't use width: width must be more than zero"
            ));
        }
        Some(other) => {
            return Err(format!(
                "ui.{op} can't use width: width must be a number, not {}.",
                type_name(other)
            ));
        }
        None => None,
    };
    if !current_table_open(frame) {
        return Ok(Value::record([("ok", Value::Bool(true))]));
    }
    let label_c = c_label(&label).map_err(|error| format!("ui.{op} can't use label: {error}"))?;
    let index = frame.tables.last().map_or(0, |table| table.columns.len());
    let flags = if sortable {
        0
    } else {
        dear_imgui_rs::sys::ImGuiTableColumnFlags_NoSort
    } | if width.is_some() {
        dear_imgui_rs::sys::ImGuiTableColumnFlags_WidthFixed
    } else {
        dear_imgui_rs::sys::ImGuiTableColumnFlags_WidthStretch
    };
    ui.with_bound_context(|| unsafe {
        dear_imgui_rs::sys::igTableSetupColumn(
            label_c.as_ptr(),
            flags,
            width.unwrap_or(0.0),
            index as u32,
        )
    });
    if let Some(table) = frame.tables.last_mut() {
        table.columns.push(TableColumnInfo { label, sortable });
    }
    Ok(Value::record([("ok", Value::Bool(true))]))
}

fn table_headers_row(ui: &Ui, frame: &mut LiveFrame, _op: &str) -> Result<Value, String> {
    if !current_table_open(frame) {
        return Ok(Value::record([("ok", Value::Bool(true))]));
    }
    let min = ui.cursor_pos();
    ui.with_bound_context(|| unsafe { dear_imgui_rs::sys::igTableHeadersRow() });
    let height = ui.with_bound_context(|| unsafe { dear_imgui_rs::sys::igTableGetHeaderRowHeight() });
    Ok(Value::record([
        ("ok", Value::Bool(true)),
        ("min", point(min)),
        ("height", Value::from(f64::from(height))),
    ]))
}

fn table_next_row(ui: &Ui, frame: &mut LiveFrame, _op: &str) -> Result<Value, String> {
    if !current_table_open(frame) {
        return Ok(Value::record([("ok", Value::Bool(true))]));
    }
    ui.with_bound_context(|| unsafe {
        dear_imgui_rs::sys::igTableNextRow(
            dear_imgui_rs::sys::ImGuiTableRowFlags_None,
            0.0,
        )
    });
    Ok(Value::record([("ok", Value::Bool(true))]))
}

fn table_next_column(ui: &Ui, frame: &mut LiveFrame, _op: &str) -> Result<Value, String> {
    if !current_table_open(frame) {
        return Ok(Value::record([("ok", Value::Bool(true))]));
    }
    ui.with_bound_context(|| unsafe { dear_imgui_rs::sys::igTableNextColumn() });
    Ok(Value::record([("ok", Value::Bool(true))]))
}

fn table_sort_specs(ui: &Ui, frame: &mut LiveFrame, _op: &str) -> Result<Value, String> {
    if !current_table_open(frame) {
        return Ok(Value::record([("ok", Value::Bool(true)), ("value", Value::Nil)]));
    }
    let Some((column_index, direction)) = ui.with_bound_context(|| {
        let specs = unsafe { dear_imgui_rs::sys::igTableGetSortSpecs() };
        if specs.is_null() {
            return None;
        }
        let count = unsafe { (*specs).SpecsCount };
        if count <= 0 {
            return None;
        }
        let spec = unsafe { &*(*specs).Specs };
        Some((spec.ColumnUserID as usize, spec.SortDirection as u8))
    }) else {
        return Ok(Value::record([("ok", Value::Bool(true)), ("value", Value::Nil)]));
    };
    let Some(label) = frame
        .tables
        .last()
        .and_then(|table| table.columns.get(column_index))
        .map(|column| column.label.clone())
    else {
        return Ok(Value::record([("ok", Value::Bool(true)), ("value", Value::Nil)]));
    };
    let direction = match direction {
        value if value == dear_imgui_rs::sys::ImGuiSortDirection_Ascending as u8 => "ascending",
        value if value == dear_imgui_rs::sys::ImGuiSortDirection_Descending as u8 => "descending",
        _ => return Ok(Value::record([("ok", Value::Bool(true)), ("value", Value::Nil)])),
    };
    Ok(Value::record([
        ("ok", Value::Bool(true)),
        (
            "value",
            Value::record([
                ("column", Value::from(label.to_lowercase().as_str())),
                ("sort", Value::from(direction)),
            ]),
        ),
    ]))
}

fn end_table(ui: &Ui, frame: &mut LiveFrame, _op: &str) -> Result<Value, String> {
    let was_open = frame.tables.pop().is_some_and(|table| table.open);
    if was_open {
        ui.with_bound_context(|| unsafe { dear_imgui_rs::sys::igEndTable() });
    }
    Ok(Value::record([("ok", Value::Bool(true))]))
}

fn begin_tab_bar(ui: &Ui, frame: &mut LiveFrame, arg: &Value, op: &str) -> Result<Value, String> {
    if let Some(key) = unknown_key(arg, &["op", "window", "seq", "id"]) {
        return Err(format!(
            "ui.{op} can't use {key}: {op} takes an id, like ui.begin_tab_bar(\"tabs\")"
        ));
    }
    let id = string_arg(arg, "id", op)?;
    let id = c_label(&id).map_err(|error| format!("ui.{op} can't use id: {error}"))?;
    if frame.tab_bar_open {
        return Err(format!(
            "ui.{op} was called while another tab bar is open; close it with ui.end_tab_bar first."
        ));
    }
    let open = ui.with_bound_context(|| unsafe {
        dear_imgui_rs::sys::igBeginTabBar(
            id.as_ptr(),
            dear_imgui_rs::sys::ImGuiTabBarFlags_None,
        )
    });
    frame.tab_bar_open = open;
    Ok(Value::record([
        ("ok", Value::Bool(true)),
        ("open", Value::Bool(open)),
    ]))
}

fn begin_tab_item(ui: &Ui, frame: &mut LiveFrame, arg: &Value, op: &str) -> Result<Value, String> {
    if let Some(key) = unknown_key(arg, &["op", "window", "seq", "label"]) {
        return Err(format!(
            "ui.{op} can't use {key}: {op} takes a label, like ui.begin_tab_item(\"Alpha\")"
        ));
    }
    let label = string_arg(arg, "label", op)?;
    let label_c = c_label(&label).map_err(|error| format!("ui.{op} can't use label: {error}"))?;
    let open = ui.with_bound_context(|| unsafe {
        dear_imgui_rs::sys::igBeginTabItem(
            label_c.as_ptr(),
            std::ptr::null_mut(),
            dear_imgui_rs::sys::ImGuiTabItemFlags_None,
        )
    });
    frame.tab_item_open = open;
    Ok(Value::record([
        ("ok", Value::Bool(true)),
        ("open", Value::Bool(open)),
        ("min", point(ui.item_rect_min())),
        ("max", point(ui.item_rect_max())),
    ]))
}

fn end_tab_item(ui: &Ui, frame: &mut LiveFrame, _op: &str) -> Result<Value, String> {
    if frame.tab_item_open {
        ui.with_bound_context(|| unsafe { dear_imgui_rs::sys::igEndTabItem() });
    }
    frame.tab_item_open = false;
    Ok(Value::record([("ok", Value::Bool(true))]))
}

fn end_tab_bar(ui: &Ui, frame: &mut LiveFrame, _op: &str) -> Result<Value, String> {
    if frame.tab_bar_open {
        ui.with_bound_context(|| unsafe { dear_imgui_rs::sys::igEndTabBar() });
    }
    frame.tab_bar_open = false;
    Ok(Value::record([("ok", Value::Bool(true))]))
}

fn begin_menu_bar(ui: &Ui, frame: &mut LiveFrame, op: &str) -> Result<Value, String> {
    if frame.menu_bar_open {
        return Err(format!(
            "ui.{op} was called while a menu bar is open; close it with ui.end_menu_bar first."
        ));
    }
    let open = ui.with_bound_context(|| unsafe { dear_imgui_rs::sys::igBeginMenuBar() });
    frame.menu_bar_open = open;
    Ok(Value::record([
        ("ok", Value::Bool(true)),
        ("open", Value::Bool(open)),
    ]))
}

fn end_menu_bar(ui: &Ui, frame: &mut LiveFrame, _op: &str) -> Result<Value, String> {
    if frame.menu_bar_open {
        ui.with_bound_context(|| unsafe { dear_imgui_rs::sys::igEndMenuBar() });
    }
    frame.menu_bar_open = false;
    Ok(Value::record([("ok", Value::Bool(true))]))
}

fn begin_menu(ui: &Ui, frame: &mut LiveFrame, arg: &Value, op: &str) -> Result<Value, String> {
    if let Some(key) = unknown_key(arg, &["op", "window", "seq", "label"]) {
        return Err(format!(
            "ui.{op} can't use {key}: {op} takes a label, like ui.begin_menu(\"File\")"
        ));
    }
    let label = string_arg(arg, "label", op)?;
    let label_c = c_label(&label).map_err(|error| format!("ui.{op} can't use label: {error}"))?;
    let open = ui.with_bound_context(|| unsafe {
        dear_imgui_rs::sys::igBeginMenu(label_c.as_ptr(), true)
    });
    frame.menu_open = open;
    Ok(Value::record([
        ("ok", Value::Bool(true)),
        ("open", Value::Bool(open)),
        ("min", point(ui.item_rect_min())),
        ("max", point(ui.item_rect_max())),
    ]))
}

fn end_menu(ui: &Ui, frame: &mut LiveFrame, _op: &str) -> Result<Value, String> {
    if frame.menu_open {
        ui.with_bound_context(|| unsafe { dear_imgui_rs::sys::igEndMenu() });
    }
    frame.menu_open = false;
    Ok(Value::record([("ok", Value::Bool(true))]))
}

fn menu_item(ui: &Ui, arg: &Value, op: &str) -> Result<Value, String> {
    if let Some(key) = unknown_key(arg, &["op", "window", "seq", "label"]) {
        return Err(format!(
            "ui.{op} can't use {key}: {op} takes a label, like ui.menu_item(\"New\")"
        ));
    }
    let label = string_arg(arg, "label", op)?;
    let label_c = c_label(&label).map_err(|error| format!("ui.{op} can't use label: {error}"))?;
    let clicked = ui.with_bound_context(|| unsafe {
        dear_imgui_rs::sys::igMenuItemEx(
            label_c.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            false,
            true,
        )
    });
    Ok(Value::record([
        ("ok", Value::Bool(true)),
        ("value", Value::Bool(clicked)),
        ("min", point(ui.item_rect_min())),
        ("max", point(ui.item_rect_max())),
    ]))
}

fn open_popup(ui: &Ui, arg: &Value, op: &str) -> Result<Value, String> {
    if let Some(key) = unknown_key(arg, &["op", "window", "seq", "id"]) {
        return Err(format!(
            "ui.{op} can't use {key}: {op} takes an id, like ui.open_popup(\"confirm\")"
        ));
    }
    let id = string_arg(arg, "id", op)?;
    let id_c = c_label(&id).map_err(|error| format!("ui.{op} can't use id: {error}"))?;
    let opened = ui.with_bound_context(|| unsafe {
        dear_imgui_rs::sys::igOpenPopup_Str(id_c.as_ptr(), dear_imgui_rs::sys::ImGuiPopupFlags_None)
    });
    Ok(Value::record([
        ("ok", Value::Bool(true)),
        ("opened", Value::Bool(opened)),
    ]))
}

fn begin_popup(ui: &Ui, frame: &mut LiveFrame, arg: &Value, op: &str) -> Result<Value, String> {
    if let Some(key) = unknown_key(arg, &["op", "window", "seq", "id"]) {
        return Err(format!(
            "ui.{op} can't use {key}: {op} takes an id, like ui.begin_popup(\"ctx\")"
        ));
    }
    let id = string_arg(arg, "id", op)?;
    let id_c = c_label(&id).map_err(|error| format!("ui.{op} can't use id: {error}"))?;
    let open = ui.with_bound_context(|| unsafe {
        dear_imgui_rs::sys::igBeginPopup(id_c.as_ptr(), dear_imgui_rs::sys::ImGuiWindowFlags_None)
    });
    frame.popup_open = open;
    Ok(Value::record([
        ("ok", Value::Bool(true)),
        ("open", Value::Bool(open)),
    ]))
}

fn popup_flags(options: Option<&Value>, op: &str) -> Result<i32, String> {
    let Some(options) = options else {
        return Ok(dear_imgui_rs::sys::ImGuiWindowFlags_None);
    };
    let always_auto_resize = match options.field("always_auto_resize") {
        Some(Value::Bool(value)) => *value,
        None => false,
        Some(other) => {
            return Err(format!(
                "ui.{op} can't use always_auto_resize: it must be true or false, not {}.",
                type_name(other)
            ));
        }
    };
    Ok(if always_auto_resize {
        dear_imgui_rs::sys::ImGuiWindowFlags_AlwaysAutoResize
    } else {
        dear_imgui_rs::sys::ImGuiWindowFlags_None
    })
}

fn begin_popup_modal(
    ui: &Ui,
    frame: &mut LiveFrame,
    arg: &Value,
    op: &str,
) -> Result<Value, String> {
    if let Some(key) = unknown_key(arg, &["op", "window", "seq", "id", "options"]) {
        return Err(format!(
            "ui.{op} can't use {key}: {op} takes id and options, like ui.begin_popup_modal(\"confirm\", {{ always_auto_resize = true }})"
        ));
    }
    let id = string_arg(arg, "id", op)?;
    let id_c = c_label(&id).map_err(|error| format!("ui.{op} can't use id: {error}"))?;
    let options = option_map(arg, op, &["always_auto_resize"])?;
    let flags = popup_flags(options, op)?;
    let open = ui.with_bound_context(|| unsafe {
        dear_imgui_rs::sys::igBeginPopupModal(id_c.as_ptr(), std::ptr::null_mut(), flags)
    });
    frame.popup_open = open;
    Ok(Value::record([
        ("ok", Value::Bool(true)),
        ("open", Value::Bool(open)),
    ]))
}

fn begin_popup_context_item(
    ui: &Ui,
    frame: &mut LiveFrame,
    arg: &Value,
    op: &str,
) -> Result<Value, String> {
    if let Some(key) = unknown_key(arg, &["op", "window", "seq", "id"]) {
        return Err(format!(
            "ui.{op} can't use {key}: {op} takes an id, like ui.begin_popup_context_item(\"ctx\")"
        ));
    }
    let id = string_arg(arg, "id", op)?;
    let id_c = c_label(&id).map_err(|error| format!("ui.{op} can't use id: {error}"))?;
    let open = ui.with_bound_context(|| unsafe {
        dear_imgui_rs::sys::igBeginPopupContextItem(
            id_c.as_ptr(),
            dear_imgui_rs::sys::ImGuiPopupFlags_MouseButtonRight,
        )
    });
    frame.popup_open = open;
    Ok(Value::record([
        ("ok", Value::Bool(true)),
        ("open", Value::Bool(open)),
    ]))
}

fn end_popup(ui: &Ui, frame: &mut LiveFrame, _op: &str) -> Result<Value, String> {
    if frame.popup_open {
        ui.with_bound_context(|| unsafe { dear_imgui_rs::sys::igEndPopup() });
    }
    frame.popup_open = false;
    Ok(Value::record([("ok", Value::Bool(true))]))
}

fn set_tooltip(ui: &Ui, arg: &Value, op: &str) -> Result<Value, String> {
    if let Some(key) = unknown_key(arg, &["op", "window", "seq", "text"]) {
        return Err(format!(
            "ui.{op} can't use {key}: {op} takes text, like ui.set_tooltip(\"Writes the file\")"
        ));
    }
    let text = string_arg(arg, "text", op)?;
    ui.set_tooltip(text.as_str());
    Ok(Value::record([("ok", Value::Bool(true))]))
}

fn tree_node(ui: &Ui, frame: &mut LiveFrame, arg: &Value, op: &str) -> Result<Value, String> {
    if let Some(key) = unknown_key(arg, &["op", "window", "seq", "label"]) {
        return Err(format!(
            "ui.{op} can't use {key}: {op} takes a label, like ui.tree_node(\"src\")"
        ));
    }
    let label = string_arg(arg, "label", op)?;
    let label_c = c_label(&label).map_err(|error| format!("ui.{op} can't use label: {error}"))?;
    let open = ui.with_bound_context(|| unsafe {
        dear_imgui_rs::sys::igTreeNodeEx_Str(label_c.as_ptr(), dear_imgui_rs::sys::ImGuiTreeNodeFlags_None)
    });
    if open {
        frame.tree_depth += 1;
    }
    Ok(Value::record([
        ("ok", Value::Bool(true)),
        ("open", Value::Bool(open)),
    ]))
}

fn tree_pop(ui: &Ui, frame: &mut LiveFrame, op: &str) -> Result<Value, String> {
    if frame.tree_depth == 0 {
        return Err(format!("ui.{op} was called without a ui.tree_node open."));
    }
    frame.tree_depth -= 1;
    ui.with_bound_context(|| unsafe { dear_imgui_rs::sys::igTreePop() });
    Ok(Value::record([("ok", Value::Bool(true))]))
}

fn collapsing_header(ui: &Ui, arg: &Value, op: &str) -> Result<Value, String> {
    if let Some(key) = unknown_key(arg, &["op", "window", "seq", "label"]) {
        return Err(format!(
            "ui.{op} can't use {key}: {op} takes a label, like ui.collapsing_header(\"Advanced\")"
        ));
    }
    let label = string_arg(arg, "label", op)?;
    let label_c = c_label(&label).map_err(|error| format!("ui.{op} can't use label: {error}"))?;
    let open = ui.with_bound_context(|| unsafe {
        dear_imgui_rs::sys::igCollapsingHeader_TreeNodeFlags(
            label_c.as_ptr(),
            dear_imgui_rs::sys::ImGuiTreeNodeFlags_None,
        )
    });
    Ok(Value::record([
        ("ok", Value::Bool(true)),
        ("open", Value::Bool(open)),
    ]))
}

fn begin_list_box(ui: &Ui, frame: &mut LiveFrame, arg: &Value, op: &str) -> Result<Value, String> {
    if let Some(key) = unknown_key(
        arg,
        &["op", "window", "seq", "id", "width", "height"],
    ) {
        return Err(format!(
            "ui.{op} can't use {key}: {op} takes id, width and height, like ui.begin_list_box(\"##kinds\", {{ width = 180, height = 120 }})"
        ));
    }
    let id = string_arg(arg, "id", op)?;
    let id_c = c_label(&id).map_err(|error| format!("ui.{op} can't use id: {error}"))?;
    let size = size_fields(
        arg,
        op,
        "ui.begin_list_box(\"##kinds\", { width = 180, height = 120 })",
    )?;
    let open = ui.with_bound_context(|| unsafe {
        dear_imgui_rs::sys::igBeginListBox(
            id_c.as_ptr(),
            dear_imgui_rs::sys::ImVec2_c {
                x: size[0],
                y: size[1],
            },
        )
    });
    frame.list_box_open = open;
    Ok(Value::record([
        ("ok", Value::Bool(true)),
        ("open", Value::Bool(open)),
    ]))
}

fn end_list_box(ui: &Ui, frame: &mut LiveFrame, _op: &str) -> Result<Value, String> {
    if frame.list_box_open {
        ui.with_bound_context(|| unsafe { dear_imgui_rs::sys::igEndListBox() });
    }
    frame.list_box_open = false;
    Ok(Value::record([("ok", Value::Bool(true))]))
}

fn color_table(arg: &Value, op: &str) -> Result<[f32; 3], String> {
    let component = |name: &str| -> Result<f32, String> {
        match arg.field(name) {
            Some(Value::Num(number)) if number.is_finite() => Ok(*number as f32),
            Some(Value::Num(_)) => Err(format!(
                "ui.{op} can't use {name}: {name} must be a finite number"
            )),
            Some(other) => Err(format!(
                "ui.{op} can't use {name}: {name} must be a number, not {}.",
                type_name(other)
            )),
            None => Err(format!(
                "ui.{op} can't use the colour: it needs r, g and b, so {name} is missing"
            )),
        }
    };
    Ok([component("r")?, component("g")?, component("b")?])
}

fn color_edit3(ui: &Ui, arg: &Value, op: &str) -> Result<Value, String> {
    if let Some(key) = unknown_key(arg, &["op", "window", "seq", "label", "value", "options"]) {
        return Err(format!(
            "ui.{op} can't use {key}: {op} takes a label, a colour and options, like ui.color_edit3(\"Tint\", tint, {{ display_mode = \"rgb\" }})"
        ));
    }
    let label = string_arg(arg, "label", op)?;
    let label_c = c_label(&label).map_err(|error| format!("ui.{op} can't use label: {error}"))?;
    let value = match arg.field("value") {
        Some(value) => color_table(value, op)?,
        None => return Err(format!("ui.{op} needs a colour value, like {{ r = 0.2, g = 0.4, b = 0.9 }}.")),
    };
    let options = option_map(arg, op, &["display_mode"])?;
    let mode = opt_string(options, "display_mode", op)?.unwrap_or_else(|| "rgb".to_string());
    let flags = if mode == "rgb" {
        dear_imgui_rs::sys::ImGuiColorEditFlags_DisplayRGB
    } else {
        return Err(format!(
            "ui.{op} can't use display_mode \"{mode}\": {op} accepts \"rgb\"."
        ));
    };
    let mut edited = value;
    let changed = ui.with_bound_context(|| unsafe {
        dear_imgui_rs::sys::igColorEdit3(label_c.as_ptr(), edited.as_mut_ptr(), flags)
    });
    Ok(Value::record([
        ("ok", Value::Bool(true)),
        ("changed", Value::Bool(changed)),
        ("value", Value::record([
            ("r", Value::from(f64::from(edited[0]))),
            ("g", Value::from(f64::from(edited[1]))),
            ("b", Value::from(f64::from(edited[2]))),
        ])),
    ]))
}

fn color_button(ui: &Ui, arg: &Value, op: &str) -> Result<Value, String> {
    if let Some(key) = unknown_key(
        arg,
        &["op", "window", "seq", "id", "value", "width", "height"],
    ) {
        return Err(format!(
            "ui.{op} can't use {key}: {op} takes id, a colour and a size, like ui.color_button({{ id = \"swatch\", value = colour, width = 40, height = 40 }})"
        ));
    }
    let id = string_arg(arg, "id", op)?;
    let id_c = c_label(&id).map_err(|error| format!("ui.{op} can't use id: {error}"))?;
    let colour = match arg.field("value") {
        Some(value) => color_table(value, op)?,
        None => return Err(format!("ui.{op} needs a colour, like {{ r = 0.2, g = 0.4, b = 0.9 }}.")),
    };
    let width = match arg.field("width") {
        Some(Value::Num(number)) if number.is_finite() && *number >= 0.0 => *number as f32,
        None => 0.0,
        Some(other) => {
            return Err(format!(
                "ui.{op} can't use width: width must be a number, not {}.",
                type_name(other)
            ));
        }
    };
    let height = match arg.field("height") {
        Some(Value::Num(number)) if number.is_finite() && *number >= 0.0 => *number as f32,
        None => 0.0,
        Some(other) => {
            return Err(format!(
                "ui.{op} can't use height: height must be a number, not {}.",
                type_name(other)
            ));
        }
    };
    let clicked = ui.with_bound_context(|| unsafe {
        dear_imgui_rs::sys::igColorButton(
            id_c.as_ptr(),
            dear_imgui_rs::sys::ImVec4_c {
                x: colour[0],
                y: colour[1],
                z: colour[2],
                w: 1.0,
            },
            dear_imgui_rs::sys::ImGuiColorEditFlags_None,
            dear_imgui_rs::sys::ImVec2_c {
                x: width,
                y: height,
            },
        )
    });
    Ok(Value::record([
        ("ok", Value::Bool(true)),
        ("clicked", Value::Bool(clicked)),
        ("min", point(ui.item_rect_min())),
        ("max", point(ui.item_rect_max())),
    ]))
}

fn begin_drag_drop_source(ui: &Ui, frame: &mut LiveFrame, _op: &str) -> Result<Value, String> {
    let open = ui.with_bound_context(|| unsafe {
        dear_imgui_rs::sys::igBeginDragDropSource(dear_imgui_rs::sys::ImGuiDragDropFlags_None)
    });
    frame.drag_source_open = open;
    Ok(Value::record([
        ("ok", Value::Bool(true)),
        ("open", Value::Bool(open)),
    ]))
}

fn set_drag_drop_payload(
    state: &FrameState,
    ui: &Ui,
    frame: &mut LiveFrame,
    arg: &Value,
    op: &str,
) -> Result<Value, String> {
    if let Some(key) = unknown_key(arg, &["op", "window", "seq", "type", "payload"]) {
        return Err(format!(
            "ui.{op} can't use {key}: {op} takes type and payload, like ui.set_drag_drop_payload(\"FILE\", {{ name = row.name }})"
        ));
    }
    let kind = string_arg(arg, "type", op)?;
    if kind.is_empty() {
        return Err(format!("ui.{op} can't use type: type can't be empty"));
    }
    let kind_c = c_label(&kind).map_err(|error| format!("ui.{op} can't use type: {error}"))?;
    let payload = arg.field("payload").cloned().unwrap_or(Value::Nil);
    if !frame.drag_source_open {
        return Err(format!(
            "ui.{op} must be called inside ui.begin_drag_drop_source."
        ));
    }
    let set = ui.with_bound_context(|| unsafe {
        dear_imgui_rs::sys::igSetDragDropPayload(
            kind_c.as_ptr(),
            1_usize as *const std::os::raw::c_void,
            0,
            dear_imgui_rs::sys::ImGuiCond_None,
        )
    });
    if set {
        state
            .drag_payloads
            .borrow_mut()
            .insert(kind, payload);
    }
    Ok(Value::record([
        ("ok", Value::Bool(true)),
        ("set", Value::Bool(set)),
    ]))
}

fn end_drag_drop_source(ui: &Ui, frame: &mut LiveFrame, _op: &str) -> Result<Value, String> {
    if frame.drag_source_open {
        ui.with_bound_context(|| unsafe { dear_imgui_rs::sys::igEndDragDropSource() });
    }
    frame.drag_source_open = false;
    Ok(Value::record([("ok", Value::Bool(true))]))
}

fn begin_drag_drop_target(ui: &Ui, frame: &mut LiveFrame, _op: &str) -> Result<Value, String> {
    let open = ui.with_bound_context(|| unsafe {
        dear_imgui_rs::sys::igBeginDragDropTarget()
    });
    frame.drag_target_open = open;
    Ok(Value::record([
        ("ok", Value::Bool(true)),
        ("open", Value::Bool(open)),
    ]))
}

fn accept_drag_drop_payload(
    state: &FrameState,
    ui: &Ui,
    frame: &mut LiveFrame,
    arg: &Value,
    op: &str,
) -> Result<Value, String> {
    if let Some(key) = unknown_key(arg, &["op", "window", "seq", "type"]) {
        return Err(format!(
            "ui.{op} can't use {key}: {op} takes a type, like ui.accept_drag_drop_payload(\"FILE\")"
        ));
    }
    let kind = string_arg(arg, "type", op)?;
    let kind_c = c_label(&kind).map_err(|error| format!("ui.{op} can't use type: {error}"))?;
    if !frame.drag_target_open {
        return Ok(Value::record([
            ("ok", Value::Bool(true)),
            ("value", Value::Nil),
        ]));
    }
    let accepted = ui.with_bound_context(|| unsafe {
        !dear_imgui_rs::sys::igAcceptDragDropPayload(
            kind_c.as_ptr(),
            dear_imgui_rs::sys::ImGuiDragDropFlags_None,
        )
        .is_null()
    });
    let payload = if accepted {
        state.drag_payloads.borrow().get(&kind).cloned().unwrap_or(Value::Nil)
    } else {
        Value::Nil
    };
    Ok(Value::record([
        ("ok", Value::Bool(true)),
        ("accepted", Value::Bool(accepted)),
        ("value", payload),
    ]))
}

fn end_drag_drop_target(ui: &Ui, frame: &mut LiveFrame, _op: &str) -> Result<Value, String> {
    if frame.drag_target_open {
        ui.with_bound_context(|| unsafe { dear_imgui_rs::sys::igEndDragDropTarget() });
    }
    frame.drag_target_open = false;
    Ok(Value::record([("ok", Value::Bool(true))]))
}

fn shortcut_key(value: &str, op: &str) -> Result<dear_imgui_rs::Key, String> {
    if value.len() == 1 {
        let ch = value.chars().next().expect("one character");
        return match ch.to_ascii_uppercase() {
            'A' => Ok(dear_imgui_rs::Key::A),
            'B' => Ok(dear_imgui_rs::Key::B),
            'C' => Ok(dear_imgui_rs::Key::C),
            'D' => Ok(dear_imgui_rs::Key::D),
            'E' => Ok(dear_imgui_rs::Key::E),
            'F' => Ok(dear_imgui_rs::Key::F),
            'G' => Ok(dear_imgui_rs::Key::G),
            'H' => Ok(dear_imgui_rs::Key::H),
            'I' => Ok(dear_imgui_rs::Key::I),
            'J' => Ok(dear_imgui_rs::Key::J),
            'K' => Ok(dear_imgui_rs::Key::K),
            'L' => Ok(dear_imgui_rs::Key::L),
            'M' => Ok(dear_imgui_rs::Key::M),
            'N' => Ok(dear_imgui_rs::Key::N),
            'O' => Ok(dear_imgui_rs::Key::O),
            'P' => Ok(dear_imgui_rs::Key::P),
            'Q' => Ok(dear_imgui_rs::Key::Q),
            'R' => Ok(dear_imgui_rs::Key::R),
            'S' => Ok(dear_imgui_rs::Key::S),
            'T' => Ok(dear_imgui_rs::Key::T),
            'U' => Ok(dear_imgui_rs::Key::U),
            'V' => Ok(dear_imgui_rs::Key::V),
            'W' => Ok(dear_imgui_rs::Key::W),
            'X' => Ok(dear_imgui_rs::Key::X),
            'Y' => Ok(dear_imgui_rs::Key::Y),
            'Z' => Ok(dear_imgui_rs::Key::Z),
            '0' => Ok(dear_imgui_rs::Key::Key0),
            '1' => Ok(dear_imgui_rs::Key::Key1),
            '2' => Ok(dear_imgui_rs::Key::Key2),
            '3' => Ok(dear_imgui_rs::Key::Key3),
            '4' => Ok(dear_imgui_rs::Key::Key4),
            '5' => Ok(dear_imgui_rs::Key::Key5),
            '6' => Ok(dear_imgui_rs::Key::Key6),
            '7' => Ok(dear_imgui_rs::Key::Key7),
            '8' => Ok(dear_imgui_rs::Key::Key8),
            '9' => Ok(dear_imgui_rs::Key::Key9),
            _ => Err(format!(
                "ui.{op} can't use key \"{value}\": a shortcut key is a letter, digit or F1..F24."
            )),
        };
    }
    if let Some(number) = value.strip_prefix('F').and_then(|rest| rest.parse::<u8>().ok()) {
        return match number {
            1 => Ok(dear_imgui_rs::Key::F1),
            2 => Ok(dear_imgui_rs::Key::F2),
            3 => Ok(dear_imgui_rs::Key::F3),
            4 => Ok(dear_imgui_rs::Key::F4),
            5 => Ok(dear_imgui_rs::Key::F5),
            6 => Ok(dear_imgui_rs::Key::F6),
            7 => Ok(dear_imgui_rs::Key::F7),
            8 => Ok(dear_imgui_rs::Key::F8),
            9 => Ok(dear_imgui_rs::Key::F9),
            10 => Ok(dear_imgui_rs::Key::F10),
            11 => Ok(dear_imgui_rs::Key::F11),
            12 => Ok(dear_imgui_rs::Key::F12),
            _ => Err(format!(
                "ui.{op} can't use key \"{value}\": a shortcut key is a letter, digit or F1..F12."
            )),
        };
    }
    Err(format!(
        "ui.{op} can't use key \"{value}\": a shortcut key is a letter, digit or F1..F12."
    ))
}

fn shortcut(ui: &Ui, arg: &Value, op: &str) -> Result<Value, String> {
    if let Some(key) = unknown_key(arg, &["op", "window", "seq", "mods", "key"]) {
        return Err(format!(
            "ui.{op} can't use {key}: {op} takes mods and key, like ui.shortcut({{ mods = {{ \"ctrl\" }}, key = \"K\" }})"
        ));
    }
    let key = string_arg(arg, "key", op)?;
    let key = shortcut_key(&key, op)?;
    let mut modifiers = dear_imgui_rs::KeyMods::empty();
    if let Some(mods) = arg.field("mods") {
        let Value::List(mods) = mods else {
            return Err(format!(
                "ui.{op} can't use mods: mods must be a list of text, not {}.",
                type_name(mods)
            ));
        };
        for item in mods.iter() {
            let name = match item {
                Value::Str(text) => text.to_string(),
                Value::Text(text) => text.to_string(),
                _ => {
                    return Err(format!(
                        "ui.{op} can't use mods: every modifier must be text, like \"ctrl\"."
                    ));
                }
            };
            match name.as_str() {
                "ctrl" => modifiers |= dear_imgui_rs::KeyMods::CTRL,
                "shift" => modifiers |= dear_imgui_rs::KeyMods::SHIFT,
                "alt" => modifiers |= dear_imgui_rs::KeyMods::ALT,
                "super" => modifiers |= dear_imgui_rs::KeyMods::SUPER,
                other => {
                    return Err(format!(
                        "ui.{op} can't use modifier \"{other}\": modifiers are ctrl, shift, alt and super."
                    ));
                }
            }
        }
    }
    let chord = dear_imgui_rs::KeyChord::new(key).with_mods(modifiers);
    let pressed = ui.shortcut(chord);
    Ok(Value::record([
        ("ok", Value::Bool(true)),
        ("value", Value::Bool(pressed)),
    ]))
}

fn unavailable_widget(op: &str) -> String {
    format!(
        "ui.{op} is not available in Dear ImGui 1.93.1; Zeron has no {op} widget. Track the value yourself with ui.input_text or a calendar of your own."
    )
}
