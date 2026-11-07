//! The terminal panel: session-scoped tabs over engine PTYs.
//!
//! Feature-inventory §1.10: tabs are per selected chat (or the new-session
//! canvas, keyed per space) and restored on return (emulators — and their
//! server-side PTYs — survive navigation; detach is not close). Tab bar
//! supports pointer drag-reorder with 150 ms sliding transforms, middle-click
//! close, and a "+" new-tab button; Cmd/Ctrl+J toggles the panel (the shell
//! owns the height animation + persistence).
//!
//! Data path per tab: `OpenTerminal` → `SubscribeTerminal` stream; Data frames
//! (base64) feed the [`Emulator`]; query responses write back; the stream
//! reconnects with exponential backoff resuming from `afterSeq`; Exit appends
//! the "[process exited N]" line and stops. Keyboard bytes go to
//! `WriteTerminal` the moment they land, one write in flight per tab (keys
//! typed meanwhile ride the next); viewport-driven resizes debounce 80 ms
//! before `ResizeTerminal` (the emulator resizes immediately).

use std::collections::HashMap;
use std::time::{Duration, Instant};

use base64::Engine as _;
use gpui::{
    App, Context, Entity, FocusHandle, IntoElement, KeyBinding, KeyDownEvent, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Render, ScrollDelta, SharedString,
    Subscription, Task, Window, actions, div, prelude::*, px,
};

use zeron_proto::{TerminalEvent, TerminalSession};
use zeron_rpc::methods;

use crate::motion::{self, AnimationExt as _, TAB_SLIDE};
use crate::popover::{MenuScrollbarMetrics, MenuScrollbarState, ScrollRailHost};
use crate::settings::{TERMINAL_MAX_VH, TERMINAL_MIN_HEIGHT};
use crate::state::{AppState, CANVAS_PANEL_PREFIX, EngineHandle};
use crate::theme::Theme;

use super::emulator::{
    CellSnapshot, CursorShape, CursorSnapshot, Emulator, GridPoint, MouseTracking, SelectionType,
    Side,
};
use super::predict::Predictor;
use super::view::{
    InputQueue, KeyRoute, MOUSE_LEFT, MOUSE_MIDDLE, MOUSE_MOTION, MOUSE_NONE, MOUSE_RIGHT,
    MOUSE_WHEEL_DOWN, MOUSE_WHEEL_UP, RESIZE_DEBOUNCE_MS, SELECTION_DRAG_THRESHOLD,
    TerminalElement, cell_at, focus_report, mouse_report, paste_bytes, route_key,
    terminal_panel_bg, wheel_arrows,
};

/// Fixed tab width — drag-reorder math stays analytic.
pub const TAB_WIDTH: f32 = 118.0;
pub const TAB_BAR_HEIGHT: f32 = 40.0;
const SELECTION_SCROLL_TICK_MS: u64 = 24;
/// Cursor blink half-period; a keystroke holds the cursor solid this long.
const BLINK_MS: u64 = 500;
/// The blink rests (cursor solid) after this long without typing, so an
/// idle focused terminal stops scheduling frames; the next key resumes it.
const BLINK_IDLE: Duration = Duration::from_secs(15);
/// The copy flash's fade.
const COPY_FLASH: Duration = Duration::from_millis(320);

actions!(terminal, [ToggleTerminal]);

/// Bind the terminal keymap (global): Cmd+J on macOS, Ctrl+J elsewhere.
pub fn init(cx: &mut App) {
    let toggle = if cfg!(target_os = "macos") {
        "cmd-j"
    } else {
        "ctrl-j"
    };
    cx.bind_keys([KeyBinding::new(toggle, ToggleTerminal, None)]);
}

// ---------------------------------------------------------------------------
// Pure logic (unit-tested)
// ---------------------------------------------------------------------------

/// Panel height clamp: 160 px … 55 % of the viewport (§1.10).
pub fn clamp_terminal_height(height: f32, viewport_h: f32) -> f32 {
    let max = (viewport_h * TERMINAL_MAX_VH).max(TERMINAL_MIN_HEIGHT);
    if height.is_finite() {
        height.clamp(TERMINAL_MIN_HEIGHT, max)
    } else {
        TERMINAL_MIN_HEIGHT
    }
}

/// Reconnect backoff: 500 ms doubling to an 8 s ceiling.
pub fn backoff_ms(attempt: u32) -> u64 {
    (500u64 << attempt.min(4)).min(8_000)
}

/// Move a tab from `from` to `to` (indices into the same vec).
pub fn reorder_tabs<T>(tabs: &mut Vec<T>, from: usize, to: usize) {
    if from >= tabs.len() || to >= tabs.len() || from == to {
        return;
    }
    let tab = tabs.remove(from);
    tabs.insert(to, tab);
}

/// Where a drag hovering at `rel_x` inside the tab strip would land.
pub fn drop_index(rel_x: f32, tab_w: f32, count: usize) -> usize {
    if count == 0 || tab_w <= 0.0 {
        return 0;
    }
    ((rel_x / tab_w).floor().max(0.0) as usize).min(count - 1)
}

/// Sliding transform (in tab-width units) for tab `ix` while `from` is dragged
/// over `over`: tabs between the two shift one slot toward the vacated gap.
pub fn slide_offset(ix: usize, from: usize, over: usize) -> f32 {
    if from < over && ix > from && ix <= over {
        -1.0
    } else if over < from && ix >= over && ix < from {
        1.0
    } else {
        0.0
    }
}

/// Active index after a reorder commit.
pub fn active_after_reorder(active: usize, from: usize, to: usize) -> usize {
    if active == from {
        to
    } else if from < active && to >= active {
        active - 1
    } else if from > active && to <= active {
        active + 1
    } else {
        active
    }
}

/// Merge the `targetDeviceId` passthrough into RPC params (no-op for chats on
/// the connected engine's own device).
fn with_target(mut params: serde_json::Value, target: &Option<String>) -> serde_json::Value {
    if let (Some(target), Some(object)) = (target, params.as_object_mut()) {
        object.insert(
            "targetDeviceId".into(),
            serde_json::Value::String(target.clone()),
        );
    }
    params
}

/// Active index after closing `closed` (given the new, shorter length).
pub fn active_after_close(active: usize, closed: usize, len_after: usize) -> usize {
    let shifted = if closed < active { active - 1 } else { active };
    if len_after == 0 {
        0
    } else {
        shifted.min(len_after - 1)
    }
}

/// The `[process exited N]` trailer, dimmed (§1.10).
pub fn exit_message(code: i32) -> Vec<u8> {
    format!("\r\n\x1b[90m[process exited {code}]\x1b[0m\r\n").into_bytes()
}

/// Tab title from the session's shell path ("/bin/zsh" → "zsh").
pub fn shell_title(shell: &str) -> String {
    let name = shell.rsplit(['/', '\\']).next().unwrap_or(shell).trim();
    if name.is_empty() {
        "terminal".to_string()
    } else {
        name.to_string()
    }
}

fn decode_base64(data: &str) -> Vec<u8> {
    base64::engine::general_purpose::STANDARD
        .decode(data)
        .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(data))
        .unwrap_or_else(|err| {
            tracing::warn!(error = %err, "terminal: dropping undecodable data frame");
            Vec::new()
        })
}

fn encode_base64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

// ---------------------------------------------------------------------------
// Entity
// ---------------------------------------------------------------------------

/// A grid snapshot handed to the paint element.
pub struct GridSnapshot {
    pub lines: Vec<Vec<CellSnapshot>>,
    /// The history row a partial scroll reveals above `lines`.
    pub line_above: Option<Vec<CellSnapshot>>,
    /// Rows the grid is shifted down by (0..1) for pixel-smooth scrolling.
    pub scroll_frac: f32,
    pub cursor: Option<CursorSnapshot>,
    pub cursor_shape: CursorShape,
    /// False during a blinking cursor's off phase.
    pub cursor_on: bool,
    /// Input-method composition in progress, drawn at the cursor.
    pub marked_text: Option<String>,
    /// 1 at the moment of a copy, easing to 0.
    pub copy_flash: f32,
}

/// Where the grid landed this frame, in window coordinates.
///
/// Reported by element prepaint because that is the only place the measured
/// font metrics exist. Mouse events arrive on the wrapping div in window
/// space, so mapping a pointer to a cell needs the glyph origin and the cell
/// size the *current* frame used — a stale one puts the selection a row off
/// after a resize.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GridGeometry {
    /// Full terminal body bounds, used by edge scrolling and the scrollbar.
    pub bounds: gpui::Bounds<Pixels>,
    /// Top-left of the first glyph (bounds origin plus padding).
    pub origin: gpui::Point<Pixels>,
    pub cell_w: f32,
    pub line_h: f32,
    pub cols: u16,
    pub rows: u16,
}

/// An in-flight left-button gesture.
///
/// A press alone does not select. It arms this, and only pointer travel past
/// [`SELECTION_DRAG_THRESHOLD`] promotes it to a real selection — otherwise the
/// click that focuses the panel would leave a one-cell selection behind
/// whenever the hand moves a pixel.
#[derive(Debug, Clone, Copy)]
struct SelectionDrag {
    /// Press position, in window space: both the threshold origin and the
    /// selection's anchor, so the selection starts where the press landed
    /// rather than where the threshold happened to trip.
    origin: gpui::Point<Pixels>,
    /// Latest pointer sample. Edge scrolling keeps using it while the pointer
    /// is stationary, updating the selection after every scrollback step.
    position: gpui::Point<Pixels>,
    armed: bool,
}

/// The shared rail's geometry inputs from the grid's own extents, in px:
/// `(viewport, content, offset)` — viewport = visible rows, content =
/// history + visible rows, offset = position from the TOP of the scrollback.
/// The terminal's display offset counts from the live bottom, so the two are
/// mirrored.
fn rail_parts(line_h: f32, rows: usize, history: usize, display_offset: usize) -> (f32, f32, f32) {
    let from_top = history.saturating_sub(display_offset);
    (
        rows as f32 * line_h,
        (history + rows) as f32 * line_h,
        from_top as f32 * line_h,
    )
}

/// Terminal scroll direction for a selection near the grid edge.
///
/// Alacritty uses positive deltas for history (up) and negative deltas for the
/// live bottom. Speed is line-based because the terminal cannot expose partial
/// rows without breaking its fixed grid.
fn selection_scroll_lines(geometry: GridGeometry, position: gpui::Point<Pixels>) -> i32 {
    let grid_height = geometry.line_h * geometry.rows as f32;
    if grid_height <= 0.0 {
        return 0;
    }
    let edge = geometry.line_h.min(grid_height / 3.0);
    let y = f32::from(position.y);
    let top = f32::from(geometry.origin.y);
    let bottom = top + grid_height;
    let speed = |penetration: f32| {
        let t = (penetration / edge).clamp(0.0, 1.0);
        (1.0 + 2.0 * t * t).round() as i32
    };
    if y < top + edge {
        speed(top + edge - y)
    } else if y > bottom - edge {
        -speed(y - (bottom - edge))
    } else {
        0
    }
}

struct TerminalTab {
    key: u64,
    title: SharedString,
    terminal_id: Option<String>,
    target_device_id: Option<String>,
    emulator: Emulator,
    /// How far past `display_offset` the view sits, in rows (0..1): history
    /// scrolls by the pixel, so a trackpad glides instead of stepping.
    scroll_remainder: f32,
    /// Fractional wheel rows owed to a program (reports / arrow keys).
    wheel_remainder: f32,
    exited: Option<i32>,
    last_seq: u64,
    input: InputQueue,
    /// Local echo for a terminal on another device.
    predictor: Predictor,
    /// Withdraws stale predictions when the host stays silent.
    predict_task: Option<Task<()>>,
    /// Shows a synchronized update the program never closed.
    sync_task: Option<Task<()>>,
    /// Input-method composition in progress.
    marked_text: Option<String>,
    /// The host answers terminal queries itself; this emulator's answers
    /// would arrive second (and late, across a network) and read as input.
    host_replies: bool,
    resize_task: Option<Task<()>>,
    /// Open + subscribe/reconnect lifecycle; dropping it cancels the stream.
    _run: Option<Task<()>>,
}

#[derive(Default)]
struct ChatTabs {
    tabs: Vec<TerminalTab>,
    active: usize,
}

/// Drag-reorder state; `epoch` keys the 150 ms slide animation restarts.
struct DragState {
    from: usize,
    over: usize,
    epoch: usize,
    prev_over: usize,
}

/// The dragged-tab payload (gpui drag-and-drop).
struct TabDragPayload {
    chat: String,
    from: usize,
    title: SharedString,
}

struct TabGhost {
    title: SharedString,
}

impl Render for TabGhost {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx);
        div()
            .w(px(TAB_WIDTH))
            .h(px(28.0))
            .px(px(Theme::SPACE_SM))
            .flex()
            .items_center()
            .rounded(px(Theme::CONTROL_RADIUS))
            .bg(theme.surface_raised)
            .border_1()
            .border_color(theme.border_strong)
            .text_size(px(12.0))
            .text_color(theme.text)
            .opacity(0.85)
            .child(div().truncate().child(self.title.clone()))
    }
}

pub struct TerminalPanel {
    state: Entity<AppState>,
    focus_handle: FocusHandle,
    focus_pending: bool,
    chats: HashMap<String, ChatTabs>,
    /// Shell-driven visibility gate: no RPC happens while closed (lazy).
    open: bool,
    /// Right-pane surface host mode: the SHELL owns the tab strip (surface
    /// tabs), so the internal bar hides, tabs are only ever created
    /// explicitly (no ensure-on-open/chat-switch), and closing the last tab
    /// must not dispatch the bottom drawer's [`ToggleTerminal`].
    embedded: bool,
    /// The right pane is in its width tween. Keep painting the retained grid
    /// through the changing clip, but do not feed transient widths into the
    /// emulator: alternate-screen rows truncate rather than reflow.
    resize_suspended: bool,
    /// Whether this docked panel's bottom-left/right corners sit at the
    /// WINDOW's corners — the shell sets these per frame so the panel's fill
    /// can carry the CSD window's rounded corners (gpui cannot clip children
    /// rounded; each full-bleed layer rounds itself).
    window_corner_bl: bool,
    window_corner_br: bool,
    tab_seq: u64,
    drag: Option<DragState>,
    last_selected: Option<String>,
    /// Last reported grid placement; `None` until the first prepaint.
    geometry: Option<GridGeometry>,
    /// Left-button gesture in flight, if any.
    selection_drag: Option<SelectionDrag>,
    /// One-shot timer rescheduled only while a live selection remains in an
    /// edge zone.
    selection_scroll_task: Option<Task<()>>,
    /// The tab the rail is bound to this frame; see [`Self::sync_rail_tab`].
    rail_tab_key: Option<u64>,
    /// The floating scrollbar rail. The shared model owns all of its state —
    /// hover, drag, and the linger/fade clock — so the panel keeps no
    /// parallel hover or drag flags. It is an on-demand affordance rather
    /// than a permanently painted rail beside the panel: the terminal owns
    /// the cursor.
    bar: MenuScrollbarState,
    /// Blink phase of the cursor (true = drawn).
    blink_on: bool,
    /// Ticks the blink while a focused, blinking cursor is on screen; any
    /// keystroke restarts it so the cursor holds solid while typing.
    blink_task: Option<Task<()>>,
    /// Render's verdict on whether the cursor should blink right now.
    blink_wanted: bool,
    /// Focus as of the last render, for focus reports.
    was_focused: bool,
    /// When the selection was last copied (drives the flash).
    copied_at: Option<Instant>,
    /// The button a program is being told is held, and the last cell
    /// reported, while mouse reporting owns the pointer.
    report_button: Option<u8>,
    report_cell: Option<(usize, usize)>,
    _observe: Subscription,
}

impl TerminalPanel {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&state, |this: &mut Self, _, cx| this.on_state_changed(cx));
        Self {
            state,
            focus_handle: cx.focus_handle(),
            focus_pending: false,
            chats: HashMap::new(),
            open: false,
            embedded: false,
            resize_suspended: false,
            window_corner_bl: false,
            window_corner_br: false,
            tab_seq: 0,
            drag: None,
            last_selected: None,
            geometry: None,
            selection_drag: None,
            selection_scroll_task: None,
            rail_tab_key: None,
            bar: MenuScrollbarState::default(),
            blink_on: true,
            blink_task: None,
            blink_wanted: false,
            was_focused: false,
            copied_at: None,
            report_button: None,
            report_cell: None,
            _observe: observe,
        }
    }

    /// A panel in right-pane surface-host mode (see the `embedded` field).
    pub fn new_embedded(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let mut panel = Self::new(state, cx);
        panel.embedded = true;
        panel
    }

    pub fn focus_handle(&self) -> FocusHandle {
        self.focus_handle.clone()
    }

    /// Claim focus once the terminal body mounts, after opening or selecting it.
    pub fn request_focus(&mut self, cx: &mut Context<Self>) {
        self.focus_pending = true;
        cx.notify();
    }

    pub fn set_resize_suspended(&mut self, suspended: bool) {
        self.resize_suspended = suspended;
    }

    /// Shell hook: whether the panel's bottom corners sit at the window's
    /// corners (Linux CSD floating window). Notifies only on change so the
    /// per-frame shell call stays cheap.
    pub fn set_window_corners(&mut self, bl: bool, br: bool, cx: &mut Context<Self>) {
        if self.window_corner_bl != bl || self.window_corner_br != br {
            self.window_corner_bl = bl;
            self.window_corner_br = br;
            cx.notify();
        }
    }

    /// Shell toggle hook. Opening lazily creates the first tab for the
    /// selected chat or new-session canvas (drawer mode; embedded tabs are
    /// explicit); closing keeps every session alive (detach ≠ close).
    pub fn set_open(&mut self, open: bool, cx: &mut Context<Self>) {
        self.open = open;
        if !open {
            self.focus_pending = false;
        }
        if open && !self.embedded {
            self.ensure_tab(cx);
        }
        cx.notify();
    }

    /// Whether the drawer is currently shown (the shell's hide-before-navigate
    /// checks this to avoid redundant updates).
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// A tab's display label: the live OSC 0/2 title when the running
    /// program set one (shells title themselves with the cwd / running
    /// command — the contextual name, user request), else the fixed
    /// "Terminal N".
    fn display_title(tab: &TerminalTab) -> SharedString {
        match tab.emulator.title().map(str::trim) {
            Some(title) if !title.is_empty() => title.to_string().into(),
            _ => tab.title.clone(),
        }
    }

    // ---- externally managed session API. Project Actions use these helpers
    // ---- in the bottom drawer; the right-pane host also uses the keyed tab
    // ---- operations because its surface strip lives in Shell.

    /// `(key, title, exited)` for the selected chat's tabs, in tab order.
    pub fn tab_summaries(&self, cx: &App) -> Vec<(u64, SharedString, bool)> {
        let chat = self.selected_chat(cx);
        self.chats
            .get(&chat)
            .map(|tabs| {
                tabs.tabs
                    .iter()
                    .map(|t| (t.key, Self::display_title(t), t.exited.is_some()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Open a fresh tab for the selected chat and return its key.
    pub fn open_tab_for_selected(&mut self, cx: &mut Context<Self>) -> Option<u64> {
        let chat = self.selected_chat(cx);
        self.open_tab(chat, cx);
        self.request_focus(cx);
        Some(self.tab_seq)
    }

    /// Create a named placeholder tab without opening a PTY. Project Actions
    /// use this before their host-side run RPC completes.
    pub fn reserve_tab_for_chat(
        &mut self,
        chat: String,
        title: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) -> u64 {
        self.tab_seq += 1;
        let key = self.tab_seq;
        let entry = self.chats.entry(chat).or_default();
        entry.tabs.push(TerminalTab {
            key,
            title: title.into(),
            terminal_id: None,
            target_device_id: None,
            emulator: Emulator::new(80, 24),
            scroll_remainder: 0.0,
            wheel_remainder: 0.0,
            exited: None,
            last_seq: 0,
            input: InputQueue::default(),
            predictor: Predictor::default(),
            predict_task: None,
            sync_task: None,
            marked_text: None,
            host_replies: false,
            resize_task: None,
            _run: None,
        });
        entry.active = entry.tabs.len() - 1;
        cx.notify();
        key
    }

    /// Attach and stream a PTY that was already opened by the owning engine.
    pub fn attach_reserved_session(
        &mut self,
        chat: &str,
        key: u64,
        session: TerminalSession,
        target_device_id: Option<String>,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.tab_mut(chat, key).is_none() {
            return false;
        }
        let Some(engine) = self.engine(cx) else {
            return false;
        };
        let run = Self::spawn_session(
            chat.to_string(),
            key,
            engine,
            target_device_id,
            Some(session),
            None,
            cx,
        );
        if let Some(tab) = self.tab_mut(chat, key) {
            tab._run = Some(run);
            true
        } else {
            false
        }
    }

    /// Turn a placeholder into a visible failed tab without opening a PTY.
    pub fn fail_reserved_tab(
        &mut self,
        chat: &str,
        key: u64,
        message: &str,
        cx: &mut Context<Self>,
    ) {
        if let Some(tab) = self.tab_mut(chat, key) {
            tab.emulator
                .feed(format!("\x1b[31mfailed to run action: {message}\x1b[0m\r\n").as_bytes());
            tab.exited = Some(-1);
            cx.notify();
        }
    }

    /// Feed PTY output to a tab directly, as the engine stream would; for
    /// offscreen fixtures that render a terminal without an engine. Returns
    /// the terminal's replies (DSR, DA, …) for the caller to write back.
    pub fn feed_tab(
        &mut self,
        chat: &str,
        key: u64,
        bytes: &[u8],
        cx: &mut Context<Self>,
    ) -> Vec<u8> {
        let Some(tab) = self.tab_mut(chat, key) else {
            return Vec::new();
        };
        let replies = tab.emulator.feed(bytes);
        cx.notify();
        replies
    }

    /// Make `key` the rendered tab of the selected chat.
    pub fn select_tab_by_key(&mut self, key: u64, cx: &mut Context<Self>) {
        let chat = self.selected_chat(cx);
        let Some(ix) = self
            .chats
            .get(&chat)
            .and_then(|tabs| tabs.tabs.iter().position(|t| t.key == key))
        else {
            return;
        };
        self.select_tab(&chat, ix, cx);
    }

    /// Close the selected chat's tab `key` (surface-tab ✕).
    pub fn close_tab_by_key(&mut self, key: u64, window: &mut Window, cx: &mut Context<Self>) {
        let chat = self.selected_chat(cx);
        self.close_tab(&chat, key, window, cx);
    }

    fn on_state_changed(&mut self, cx: &mut Context<Self>) {
        let selected_key = self.selected_chat(cx);
        let prev = self.last_selected.clone();
        let switched = Some(selected_key.clone()) != prev;
        if switched {
            self.last_selected = Some(selected_key.clone());
            self.drag = None;
        }
        if self.open && !self.embedded {
            // Returning to a chat with tabs restores them; a fresh chat (or an
            // engine that only just finished booting) gets its first tab —
            // ensure_tab is idempotent, so calling on every state change is safe.
            // Embedded: surface tabs are explicit — a chat switch just shows
            // that chat's own tabs (or the shell's surface picker).
            // Entering the canvas from a chat never auto-creates: the shell
            // hides the drawer on canvas entry, so creating here would open a
            // PTY just to hide it (observer order vs. the shell is not
            // guaranteed).
            let entering_canvas = switched
                && selected_key.starts_with(CANVAS_PANEL_PREFIX)
                && prev
                    .as_ref()
                    .is_some_and(|prev| !prev.starts_with(CANVAS_PANEL_PREFIX));
            if !entering_canvas {
                self.ensure_tab(cx);
            }
        }
        if switched {
            cx.notify();
        }
    }

    fn engine(&self, cx: &App) -> Option<EngineHandle> {
        self.state.read(cx).engine().cloned()
    }

    /// The chat's host device when it differs from the connected engine's own —
    /// the PTY lives on the chat's device (feature-inventory §2.1 "terminals
    /// live on the chat's host device"), so every terminal RPC for a remote
    /// chat needs the `targetDeviceId` passthrough. Without it the local
    /// engine checks the chat's cwd against its OWN filesystem and fails with
    /// "Session working directory is unavailable" (user report).
    fn chat_target(&self, chat: &str, cx: &App) -> Option<String> {
        self.state.read(cx).terminal_target_device(chat)
    }

    fn selected_chat(&self, cx: &App) -> String {
        self.state.read(cx).panel_session_key()
    }

    fn ensure_tab(&mut self, cx: &mut Context<Self>) {
        let chat = self.selected_chat(cx);
        if self.should_wait_for_canvas_project(&chat, cx) {
            return;
        }
        if self.chats.get(&chat).is_none_or(|c| c.tabs.is_empty()) {
            self.open_tab(chat, cx);
        }
    }

    /// A restored project id with no WatchSpaces row yet: opening now would
    /// send `~` and (before the engine resolved `space-canvas:` itself)
    /// stick a home-dir PTY on this canvas key.
    fn should_wait_for_canvas_project(&self, session_key: &str, cx: &App) -> bool {
        let state = self.state.read(cx);
        session_key.starts_with(CANVAS_PANEL_PREFIX)
            && session_key != CANVAS_PANEL_PREFIX
            && !state.spaces_synced
            && state.terminal_open_cwd_for(session_key).is_none()
    }

    fn tab_mut(&mut self, chat: &str, key: u64) -> Option<&mut TerminalTab> {
        self.chats
            .get_mut(chat)?
            .tabs
            .iter_mut()
            .find(|t| t.key == key)
    }

    fn active_tab(&self, cx: &App) -> Option<&TerminalTab> {
        let chat = self.selected_chat(cx);
        let tabs = self.chats.get(&chat)?;
        tabs.tabs.get(tabs.active)
    }

    // ---- open / stream lifecycle ----

    fn open_tab(&mut self, chat: String, cx: &mut Context<Self>) {
        let Some(engine) = self.engine(cx) else {
            return;
        };
        let tab_no = self
            .chats
            .get(&chat)
            .map_or(1, |entry| entry.tabs.len() + 1);
        let key = self.reserve_tab_for_chat(chat.clone(), format!("Terminal {tab_no}"), cx);
        let target = self.chat_target(&chat, cx);
        let cwd = self.state.read(cx).terminal_open_cwd_for(&chat);
        let run = Self::spawn_session(chat.clone(), key, engine, target, None, cwd, cx);
        if let Some(tab) = self.tab_mut(&chat, key) {
            tab._run = Some(run);
        }
        cx.notify();
    }

    /// OpenTerminal, then pump SubscribeTerminal with reconnect backoff.
    fn spawn_session(
        chat: String,
        key: u64,
        engine: EngineHandle,
        target: Option<String>,
        existing_session: Option<TerminalSession>,
        cwd: Option<String>,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        cx.spawn(async move |this, cx| {
            let (cols, rows) = this
                .update(cx, |panel, _| {
                    panel
                        .tab_mut(&chat, key)
                        .map(|t| (t.emulator.cols() as u16, t.emulator.rows() as u16))
                        .unwrap_or((80, 24))
                })
                .unwrap_or((80, 24));

            // A PTY someone else opened has whatever size they gave it.
            let opened_elsewhere = existing_session.is_some();
            let session = match existing_session {
                Some(session) => session,
                None => {
                    let mut params =
                        serde_json::json!({ "chatId": chat, "cols": cols, "rows": rows });
                    if let (Some(cwd), Some(object)) = (cwd, params.as_object_mut()) {
                        object.insert("cwd".into(), serde_json::Value::String(cwd));
                    }
                    match engine
                        .client()
                        .call_as::<TerminalSession>(
                            methods::OPEN_TERMINAL,
                            with_target(params, &target),
                        )
                        .await
                    {
                        Ok(session) => session,
                        Err(err) => {
                    tracing::warn!(error = %err, "OpenTerminal failed");
                    let _ = this.update(cx, |panel, cx| {
                        if let Some(tab) = panel.tab_mut(&chat, key) {
                            tab.emulator.feed(
                                format!("\x1b[31mfailed to open terminal: {err}\x1b[0m\r\n")
                                    .as_bytes(),
                            );
                            tab.exited = Some(-1);
                            cx.notify();
                        }
                    });
                    return;
                        }
                    }
                }
            };
            let terminal_id = session.id.clone();
            let attached = this
                .update(cx, |panel, cx| {
                    let tab = panel.tab_mut(&chat, key)?;
                    tab.terminal_id = Some(terminal_id.clone());
                    tab.target_device_id = target.clone();
                    tab.host_replies = session.host_replies;
                    let size = (tab.emulator.cols() as u16, tab.emulator.rows() as u16);
                    // Keys typed while the PTY was still opening.
                    panel.pump_input(&chat, key, cx);
                    cx.notify();
                    Some(size)
                })
                .ok()
                .flatten();
            // The grid may have been measured (or resized) while the open was
            // in flight — a remote open easily outlasts the resize debounce,
            // which has no terminal to address yet and gives up. Tell the PTY
            // the size actually on screen, or the shell wraps at the wrong
            // column (zsh's `%` end-of-line marker is the first casualty).
            if let Some((now_cols, now_rows)) = attached
                && (opened_elsewhere || (now_cols, now_rows) != (cols, rows))
            {
                let _ = engine
                    .client()
                    .call(
                        methods::RESIZE_TERMINAL,
                        with_target(
                            serde_json::json!({
                                "terminalId": terminal_id,
                                "cols": now_cols,
                                "rows": now_rows,
                            }),
                            &target,
                        ),
                    )
                    .await;
            }
            let attached = attached.is_some();
            if !attached {
                // Tab was closed before the open completed — release the PTY.
                let _ = engine
                    .client()
                    .call(
                        methods::CLOSE_TERMINAL,
                        with_target(
                            serde_json::json!({ "terminalId": terminal_id }),
                            &target,
                        ),
                    )
                    .await;
                return;
            }

            let mut attempt: u32 = 0;
            loop {
                let Ok(resume) = this.update(cx, |panel, _| {
                    panel
                        .tab_mut(&chat, key)
                        .map(|t| (t.last_seq, t.host_replies))
                }) else {
                    return; // entity released
                };
                let Some((after_seq, host_replies)) = resume else { return }; // tab closed

                let subscribed = engine
                    .client()
                    .subscribe(
                        methods::SUBSCRIBE_TERMINAL,
                        with_target(
                            serde_json::json!({
                                "terminalId": terminal_id,
                                "afterSeq": after_seq,
                                "hostReplies": host_replies,
                            }),
                            &target,
                        ),
                    )
                    .await;
                let mut rx = match subscribed {
                    Ok(rx) => rx,
                    Err(err) => {
                        tracing::debug!(error = %err, attempt, "SubscribeTerminal failed; backing off");
                        cx.background_executor()
                            .timer(Duration::from_millis(backoff_ms(attempt)))
                            .await;
                        attempt = attempt.saturating_add(1);
                        continue;
                    }
                };

                while let Some(value) = rx.recv().await {
                    let event: TerminalEvent = match serde_json::from_value(value) {
                        Ok(event) => event,
                        Err(err) => {
                            tracing::warn!(error = %err, "terminal: malformed stream frame");
                            continue;
                        }
                    };
                    attempt = 0;
                    let outcome = this.update(cx, |panel, cx| {
                        panel.apply_stream_event(&chat, key, event, cx)
                    });
                    match outcome {
                        Ok(StreamDisposition::Continue) => {}
                        Ok(StreamDisposition::Stop) => return,
                        Err(_) => return,
                    }
                }

                // Stream dropped without an exit — reconnect from afterSeq.
                let done = this
                    .update(cx, |panel, _| {
                        panel.tab_mut(&chat, key).map(|t| t.exited.is_some()).unwrap_or(true)
                    })
                    .unwrap_or(true);
                if done {
                    return;
                }
                cx.background_executor()
                    .timer(Duration::from_millis(backoff_ms(attempt)))
                    .await;
                attempt = attempt.saturating_add(1);
            }
        })
    }

    fn apply_stream_event(
        &mut self,
        chat: &str,
        key: u64,
        event: TerminalEvent,
        cx: &mut Context<Self>,
    ) -> StreamDisposition {
        let Some(tab) = self.tab_mut(chat, key) else {
            return StreamDisposition::Stop;
        };
        match event {
            TerminalEvent::Data { seq, data } => {
                tab.last_seq = seq;
                let responses = tab.emulator.feed(&decode_base64(&data));
                if tab.emulator.alt_screen() {
                    tab.predictor.reset();
                }
                tab.predictor.output(&tab.emulator, Instant::now());
                let sync_deadline = tab.emulator.sync_deadline();
                if !responses.is_empty() && !tab.host_replies {
                    // Query responses (DSR etc.) share the key queue, so they
                    // reach the PTY in order with what was typed. Hosts that
                    // answer queries themselves get none from here.
                    tab.input.push(&responses);
                    self.pump_input(chat, key, cx);
                }
                if let Some(deadline) = sync_deadline {
                    self.schedule_sync_flush(chat, key, deadline, cx);
                }
                cx.notify();
                StreamDisposition::Continue
            }
            TerminalEvent::Exit { seq, exit_code, .. } => {
                tab.last_seq = seq;
                tab.exited = Some(exit_code);
                tab.predictor.reset();
                tab.marked_text = None;
                tab.emulator.feed(&exit_message(exit_code));
                cx.notify();
                StreamDisposition::Stop
            }
        }
    }

    // ---- input ----

    /// Send keyboard bytes from the active tab now (or right after the write
    /// already in flight).
    fn queue_input(&mut self, bytes: &[u8], cx: &mut Context<Self>) {
        let chat = self.selected_chat(cx);
        let Some(tabs) = self.chats.get_mut(&chat) else {
            return;
        };
        let active = tabs.active;
        let Some(tab) = tabs.tabs.get_mut(active) else {
            return;
        };
        if tab.exited.is_some() {
            return;
        }
        // A keypress while scrolled back snaps to the live bottom (xterm).
        if tab.emulator.display_offset() > 0 || tab.scroll_remainder != 0.0 {
            tab.emulator.scroll_to_bottom();
            tab.scroll_remainder = 0.0;
        }
        let key = tab.key;
        tab.input.push(bytes);
        self.pump_input(&chat, key, cx);
        self.restart_blink(cx);
    }

    /// Send bytes the PROGRAM asked for (focus, pointer and wheel reports):
    /// in order with typing, but without a keystroke's side effects — the
    /// view stays where the user scrolled it and the cursor keeps blinking.
    fn send_report(&mut self, bytes: &[u8], cx: &mut Context<Self>) {
        let chat = self.selected_chat(cx);
        let Some(key) = self.with_active_tab(cx, |tab| {
            tab.input.push(bytes);
            tab.key
        }) else {
            return;
        };
        self.pump_input(&chat, key, cx);
    }

    /// Start the tab's next `WriteTerminal` unless one is in flight; its
    /// completion pumps again, so a burst drains as fast as the engine acks.
    fn pump_input(&mut self, chat: &str, key: u64, cx: &mut Context<Self>) {
        let Some(engine) = self.engine(cx) else {
            return;
        };
        let Some(tab) = self.tab_mut(chat, key) else {
            return;
        };
        // OpenTerminal still in flight: the attach pumps once it lands.
        let Some(id) = tab.terminal_id.clone() else {
            return;
        };
        let Some(bytes) = tab.input.start() else {
            return;
        };
        let target = tab.target_device_id.clone();
        if target.is_some() {
            tab.predictor.sent(Instant::now());
        }
        let data = encode_base64(&bytes);
        let chat = chat.to_string();
        cx.spawn(async move |this, cx| {
            let _ = engine
                .client()
                .call(
                    methods::WRITE_TERMINAL,
                    with_target(
                        serde_json::json!({ "terminalId": id, "data": data }),
                        &target,
                    ),
                )
                .await;
            let _ = this.update(cx, |panel, cx| {
                if let Some(tab) = panel.tab_mut(&chat, key) {
                    tab.input.finish();
                }
                panel.pump_input(&chat, key, cx);
            });
        })
        .detach();
    }

    fn paste_clipboard(&mut self, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else {
            return;
        };
        let bracketed = self
            .active_tab(cx)
            .map(|tab| tab.emulator.bracketed_paste_mode())
            .unwrap_or(false);
        let bytes = paste_bytes(&text, bracketed);
        self.with_active_tab(cx, |tab| tab.predictor.interrupt());
        self.queue_input(&bytes, cx);
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let ks = &event.keystroke;
        let mods = &ks.modifiers;
        // Paste: Cmd+V (macOS) / Ctrl+Shift+V.
        if ks.key == "v" && (mods.platform || (mods.control && mods.shift)) {
            self.paste_clipboard(cx);
            cx.stop_propagation();
            return;
        }
        // Copy: Cmd+C (macOS) / Ctrl+Shift+C. Only swallowed when it actually
        // copied — so Ctrl+Shift+C with nothing selected still falls through
        // to the interrupt, and plain Ctrl+C (no shift) never reaches here.
        if ks.key == "c"
            && (mods.platform || (mods.control && mods.shift))
            && self.copy_selection(cx)
        {
            cx.stop_propagation();
            return;
        }
        let route = self
            .active_tab(cx)
            .map(|tab| route_key(&ks.key, ks.key_char.as_deref(), mods, &tab.emulator));
        let Some(route) = route else { return };
        match route {
            // A plain space is typing, not an interruption: the predictor
            // keeps the run (and its trust) going across words.
            KeyRoute::Bytes(bytes) if bytes == b" " => {
                self.type_text(" ", cx);
                cx.stop_propagation();
            }
            KeyRoute::Bytes(bytes) => {
                self.with_active_tab(cx, |tab| {
                    if bytes == [0x7f] {
                        tab.predictor.backspace();
                    } else {
                        tab.predictor.interrupt();
                    }
                });
                self.queue_input(&bytes, cx);
                cx.stop_propagation();
            }
            // A held key repeats here rather than through the platform text
            // input, which would offer its accent picker instead (holding `j`
            // in an editor must move, not ask about `ĵ`).
            KeyRoute::Text if event.is_held => {
                if let Some(text) = ks.key_char.clone() {
                    self.type_text(&text, cx);
                    cx.stop_propagation();
                }
            }
            // Platform text input delivers it (see the input handler below),
            // after any input method has composed it.
            KeyRoute::Text | KeyRoute::NotOurs => {}
        }
    }

    /// Typed text from the keyboard or an input method.
    fn type_text(&mut self, text: &str, cx: &mut Context<Self>) {
        self.with_active_tab(cx, |tab| {
            let predictable = tab.target_device_id.is_some()
                && !tab.emulator.alt_screen()
                && tab.emulator.display_offset() == 0
                && tab.emulator.cursor().is_some();
            if predictable {
                let cursor = tab.emulator.live_cursor();
                let cols = tab.emulator.cols();
                tab.predictor.typed(text, cursor, cols, Instant::now());
            } else {
                tab.predictor.interrupt();
            }
        });
        self.schedule_prediction_expiry(cx);
        self.queue_input(text.as_bytes(), cx);
    }

    fn with_active_tab<R>(&mut self, cx: &App, f: impl FnOnce(&mut TerminalTab) -> R) -> Option<R> {
        let chat = self.selected_chat(cx);
        let tabs = self.chats.get_mut(&chat)?;
        let active = tabs.active;
        tabs.tabs.get_mut(active).map(f)
    }

    /// Withdraw the active tab's predictions if the host leaves them
    /// unconfirmed past their deadline.
    fn schedule_prediction_expiry(&mut self, cx: &mut Context<Self>) {
        let chat = self.selected_chat(cx);
        let Some(tabs) = self.chats.get_mut(&chat) else {
            return;
        };
        let active = tabs.active;
        let Some(tab) = tabs.tabs.get_mut(active) else {
            return;
        };
        let Some(deadline) = tab.predictor.next_deadline() else {
            return;
        };
        let key = tab.key;
        tab.predict_task = Some(cx.spawn(async move |this, cx| {
            let wait = deadline.saturating_duration_since(Instant::now());
            cx.background_executor()
                .timer(wait + Duration::from_millis(1))
                .await;
            let _ = this.update(cx, |panel, cx| {
                if let Some(tab) = panel.tab_mut(&chat, key)
                    && tab.predictor.expire(Instant::now())
                {
                    cx.notify();
                }
            });
        }));
    }

    /// Show a synchronized update at its deadline even if the program never
    /// ends it (a crashed TUI must not freeze the screen).
    fn schedule_sync_flush(
        &mut self,
        chat: &str,
        key: u64,
        deadline: std::time::Instant,
        cx: &mut Context<Self>,
    ) {
        let Some(tab) = self.tab_mut(chat, key) else {
            return;
        };
        let chat = chat.to_string();
        tab.sync_task = Some(cx.spawn(async move |this, cx| {
            let wait = deadline.saturating_duration_since(std::time::Instant::now());
            cx.background_executor()
                .timer(wait + Duration::from_millis(1))
                .await;
            let _ = this.update(cx, |panel, cx| {
                let Some(tab) = panel.tab_mut(&chat, key) else {
                    return;
                };
                if tab
                    .emulator
                    .sync_deadline()
                    .is_some_and(|due| due <= std::time::Instant::now())
                {
                    let responses = tab.emulator.finish_sync();
                    if !responses.is_empty() && !tab.host_replies {
                        tab.input.push(&responses);
                        panel.pump_input(&chat, key, cx);
                    }
                    cx.notify();
                }
            });
        }));
    }

    // ---- cursor blink / focus ----

    /// Hold the cursor solid and restart the blink clock.
    fn restart_blink(&mut self, cx: &mut Context<Self>) {
        self.blink_on = true;
        self.blink_task = None;
        if self.blink_wanted {
            self.blink_task = Some(Self::blink_loop(cx));
        }
    }

    fn blink_loop(cx: &mut Context<Self>) -> Task<()> {
        let started = Instant::now();
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(BLINK_MS))
                    .await;
                let keep = this.update(cx, |panel, cx| {
                    if !panel.blink_wanted || started.elapsed() >= BLINK_IDLE {
                        if !panel.blink_on {
                            panel.blink_on = true;
                            cx.notify();
                        }
                        return false;
                    }
                    panel.blink_on = !panel.blink_on;
                    cx.notify();
                    true
                });
                if !matches!(keep, Ok(true)) {
                    let _ = this.update(cx, |panel, _| panel.blink_task = None);
                    return;
                }
            }
        })
    }

    /// Render-time bookkeeping for focus: report focus changes to programs
    /// that asked (1004), and run the blink clock only while a focused,
    /// blinking cursor is on screen in an active window — an idle or hidden
    /// terminal schedules no frames.
    fn sync_focus(&mut self, focused: bool, window_active: bool, cx: &mut Context<Self>) {
        if focused != self.was_focused {
            self.was_focused = focused;
            let reporting = self
                .active_tab(cx)
                .is_some_and(|tab| tab.emulator.focus_reporting() && tab.exited.is_none());
            if reporting {
                self.send_report(focus_report(focused), cx);
            }
        }
        let wanted = focused
            && window_active
            && self.active_tab(cx).is_some_and(|tab| {
                tab.exited.is_none()
                    && tab.emulator.cursor_style().1
                    && tab.emulator.cursor().is_some()
            });
        if wanted != self.blink_wanted {
            self.blink_wanted = wanted;
            self.blink_on = true;
            self.blink_task = wanted.then(|| Self::blink_loop(cx));
        }
    }

    // ---- grid metrics / element hooks ----

    /// Called from element prepaint with the frame's grid placement. Resizes
    /// the emulator immediately; the `ResizeTerminal` RPC debounces 80 ms.
    pub fn on_grid_metrics(&mut self, geometry: GridGeometry, cx: &mut Context<Self>) {
        // Stash unconditionally, before the early returns below: pointer
        // mapping needs the placement even on frames where nothing resized,
        // which is almost all of them.
        self.geometry = Some(geometry);
        if self.resize_suspended {
            return;
        }
        let (cols, rows) = (geometry.cols, geometry.rows);
        let chat = self.selected_chat(cx);
        let engine = self.engine(cx);
        let Some(tabs) = self.chats.get_mut(&chat) else {
            return;
        };
        let active = tabs.active;
        let Some(tab) = tabs.tabs.get_mut(active) else {
            return;
        };
        if tab.emulator.cols() == cols as usize && tab.emulator.rows() == rows as usize {
            return;
        }
        tab.emulator.resize(cols, rows);
        // The shell redraws for the new width; nothing predicted still holds.
        tab.predictor.reset();
        let key = tab.key;
        let target = tab.target_device_id.clone();
        if let (Some(engine), Some(tab)) = (engine, self.tab_mut(&chat, key)) {
            let id = tab.terminal_id.clone();
            tab.resize_task = Some(cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(RESIZE_DEBOUNCE_MS))
                    .await;
                // Re-read the *current* size — later prepaints may have
                // resized again inside the debounce window.
                let Ok(current) = this.update(cx, |panel, _| {
                    panel
                        .tab_mut(&chat, key)
                        .map(|t| (t.terminal_id.clone(), t.emulator.cols(), t.emulator.rows()))
                }) else {
                    return;
                };
                let Some((stored_id, cols, rows)) = current else {
                    return;
                };
                let Some(id) = stored_id.or(id) else { return };
                let _ = engine
                    .client()
                    .call(
                        methods::RESIZE_TERMINAL,
                        with_target(
                            serde_json::json!({ "terminalId": id, "cols": cols, "rows": rows }),
                            &target,
                        ),
                    )
                    .await;
            }));
        }
        // Deliberately no cx.notify(): this runs during prepaint of the
        // current frame, which already paints the resized grid.
    }

    /// Rows are drawn `dy` lower than the measured grid (a partial scroll);
    /// pointer mapping follows them.
    pub fn shift_grid_origin(&mut self, dy: Pixels) {
        if let Some(geometry) = self.geometry.as_mut() {
            geometry.origin.y += dy;
        }
    }

    /// Snapshot for the paint element.
    pub fn active_grid_snapshot(&self, cx: &App) -> Option<GridSnapshot> {
        let tab = self.active_tab(cx)?;
        let mut lines = tab.emulator.lines();
        let mut cursor = tab.emulator.cursor();
        if tab.emulator.display_offset() == 0 {
            let (predicted, predicted_cursor) = tab.predictor.overlay();
            for (row, col, ch) in predicted {
                if let Some(cell) = lines.get_mut(row).and_then(|line| line.get_mut(col)) {
                    cell.ch = ch;
                    cell.predicted = true;
                }
            }
            if predicted_cursor.is_some() && cursor.is_some() {
                cursor = predicted_cursor;
            }
        }
        let copy_flash = self.copied_at.map_or(0.0, |at| {
            let t = at.elapsed().as_secs_f32() / COPY_FLASH.as_secs_f32();
            let left = (1.0 - t).clamp(0.0, 1.0);
            left * left
        });
        let scroll_frac = if tab.emulator.alt_screen()
            || tab.emulator.display_offset() >= tab.emulator.history_lines()
        {
            0.0
        } else {
            tab.scroll_remainder.clamp(0.0, 1.0)
        };
        Some(GridSnapshot {
            lines,
            line_above: (scroll_frac > 0.0)
                .then(|| tab.emulator.line_above())
                .flatten(),
            scroll_frac,
            cursor,
            cursor_shape: tab.emulator.cursor_style().0,
            cursor_on: self.blink_on || !self.blink_wanted,
            marked_text: tab.marked_text.clone(),
            copy_flash,
        })
    }

    /// Whether the copy flash still needs frames.
    pub fn copy_flash_running(&self) -> bool {
        self.copied_at.is_some_and(|at| at.elapsed() < COPY_FLASH)
    }

    // ---- selection ----

    /// Run `f` against the active tab's emulator.
    fn with_active_emulator<R>(
        &mut self,
        cx: &App,
        f: impl FnOnce(&mut Emulator) -> R,
    ) -> Option<R> {
        let chat = self.selected_chat(cx);
        let tabs = self.chats.get_mut(&chat)?;
        let active = tabs.active;
        tabs.tabs.get_mut(active).map(|tab| f(&mut tab.emulator))
    }

    /// Window position → grid point, using this frame's placement. `None`
    /// before the first prepaint, or when no tab is active.
    fn grid_point_at(
        &mut self,
        position: gpui::Point<Pixels>,
        cx: &App,
    ) -> Option<(GridPoint, Side)> {
        let geometry = self.geometry?;
        let hit = cell_at(
            f32::from(position.x - geometry.origin.x),
            f32::from(position.y - geometry.origin.y),
            geometry.cell_w,
            geometry.line_h,
            geometry.cols as usize,
            geometry.rows as usize,
        );
        let point = self.with_active_emulator(cx, |emu| emu.grid_point(hit.row, hit.col))?;
        Some((point, hit.side))
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle, cx);
        if self.report_mouse(MOUSE_LEFT, event.position, false, &event.modifiers, cx) {
            self.report_button = Some(MOUSE_LEFT);
            return;
        }
        let Some((point, side)) = self.grid_point_at(event.position, cx) else {
            return;
        };
        // Click count picks the granularity, the same mapping every terminal
        // uses: drag, word, line.
        let ty = match event.click_count {
            0 => return,
            1 => SelectionType::Simple,
            2 => SelectionType::Semantic,
            _ => SelectionType::Lines,
        };
        let shift = event.modifiers.shift;
        if ty == SelectionType::Simple {
            // Shift+click extends an existing selection instead of replacing
            // it — the one gesture that reaches text off the bottom of a long
            // drag without redoing the whole thing.
            let extended = shift
                && self
                    .with_active_emulator(cx, |emu| {
                        let extend = emu.has_selection();
                        if extend {
                            emu.update_selection(point, side);
                        }
                        extend
                    })
                    .unwrap_or(false);
            if extended {
                self.selection_drag = Some(SelectionDrag {
                    origin: event.position,
                    position: event.position,
                    armed: true,
                });
                cx.notify();
                return;
            }
            // A plain press clears and arms; the selection itself only begins
            // once the pointer travels far enough to mean it.
            self.with_active_emulator(cx, |emu| emu.clear_selection());
            self.selection_drag = Some(SelectionDrag {
                origin: event.position,
                position: event.position,
                armed: false,
            });
        } else {
            // Word and line selections are complete on the press, so they need
            // no threshold — but keep the drag live so the pointer can extend
            // them at that granularity.
            self.with_active_emulator(cx, |emu| emu.start_selection(ty, point, side));
            self.selection_drag = Some(SelectionDrag {
                origin: event.position,
                position: event.position,
                armed: true,
            });
        }
        cx.notify();
    }

    fn on_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let tracking = self.reporting(&event.modifiers, cx);
        let report = match tracking {
            Some(MouseTracking::Motion) => true,
            Some(MouseTracking::Drag) => self.report_button.is_some(),
            _ => false,
        };
        if report {
            let button = self.report_button.unwrap_or(MOUSE_NONE) + MOUSE_MOTION;
            self.report_mouse(button, event.position, false, &event.modifiers, cx);
            return;
        }
        if tracking.is_some() && self.report_button.is_some() {
            return; // a reported press owns this drag
        }
        if !event.dragging() {
            return;
        }
        let Some(mut drag) = self.selection_drag else {
            return;
        };
        drag.position = event.position;
        self.selection_drag = Some(drag);
        if !drag.armed {
            let dx = f32::from(event.position.x - drag.origin.x);
            let dy = f32::from(event.position.y - drag.origin.y);
            if dx.hypot(dy) < SELECTION_DRAG_THRESHOLD {
                return;
            }
            // Threshold tripped: anchor at the *press*, not here, so the
            // selection covers the whole gesture.
            let Some((anchor, side)) = self.grid_point_at(drag.origin, cx) else {
                return;
            };
            self.with_active_emulator(cx, |emu| {
                emu.start_selection(SelectionType::Simple, anchor, side)
            });
            self.selection_drag = Some(SelectionDrag {
                armed: true,
                ..drag
            });
        }
        let Some((point, side)) = self.grid_point_at(event.position, cx) else {
            return;
        };
        self.with_active_emulator(cx, |emu| emu.update_selection(point, side));
        cx.notify();
        self.schedule_selection_scroll(cx);
    }

    fn on_mouse_up(&mut self, event: &MouseUpEvent, _window: &mut Window, cx: &mut Context<Self>) {
        self.selection_drag = None;
        self.selection_scroll_task = None;
        if let Some(button) = self.report_button.take() {
            // Release whatever the program was told is held, even if the
            // modifiers changed mid-gesture.
            self.send_mouse(button, event.position, true, &event.modifiers, cx);
            self.report_cell = None;
        }
    }

    /// Middle and right presses only ever mean something to a program that
    /// asked for pointer reports.
    fn on_other_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle, cx);
        let button = match event.button {
            MouseButton::Middle => MOUSE_MIDDLE,
            MouseButton::Right => MOUSE_RIGHT,
            _ => return,
        };
        if self.report_mouse(button, event.position, false, &event.modifiers, cx) {
            self.report_button = Some(button);
            cx.stop_propagation();
        }
    }

    /// The program's pointer reporting mode, unless Shift is held: Shift
    /// always selects text, so a terminal running a mouse-aware program
    /// still lets you copy from it.
    fn reporting(&self, mods: &gpui::Modifiers, cx: &App) -> Option<MouseTracking> {
        if mods.shift {
            return None;
        }
        let tab = self.active_tab(cx)?;
        if tab.exited.is_some() {
            return None;
        }
        tab.emulator.mouse_tracking()
    }

    /// Report a pointer event if the program wants it; whether it was taken.
    fn report_mouse(
        &mut self,
        button: u8,
        position: gpui::Point<Pixels>,
        release: bool,
        mods: &gpui::Modifiers,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.reporting(mods, cx).is_none() {
            return false;
        }
        self.send_mouse(button, position, release, mods, cx);
        true
    }

    fn send_mouse(
        &mut self,
        button: u8,
        position: gpui::Point<Pixels>,
        release: bool,
        mods: &gpui::Modifiers,
        cx: &mut Context<Self>,
    ) {
        let Some(geometry) = self.geometry else {
            return;
        };
        let hit = cell_at(
            f32::from(position.x - geometry.origin.x),
            f32::from(position.y - geometry.origin.y),
            geometry.cell_w,
            geometry.line_h,
            geometry.cols as usize,
            geometry.rows as usize,
        );
        let motion = (MOUSE_MOTION..MOUSE_WHEEL_UP).contains(&button);
        if motion && self.report_cell == Some((hit.row, hit.col)) {
            return; // same cell: nothing new to tell
        }
        self.report_cell = Some((hit.row, hit.col));
        let sgr = self
            .active_tab(cx)
            .is_some_and(|tab| tab.emulator.sgr_mouse());
        if let Some(bytes) = mouse_report(button, hit.col, hit.row, release, mods, sgr) {
            self.send_report(&bytes, cx);
        }
    }

    /// Wheel movement in rows (positive = up): to the program as wheel
    /// reports when it tracks the mouse, as arrow keys to a full-screen
    /// program that doesn't (less, man), else through the scrollback — by
    /// the pixel, clamped flush to the live bottom and the history top.
    fn wheel(
        &mut self,
        rows: f32,
        position: gpui::Point<Pixels>,
        mods: &gpui::Modifiers,
        cx: &mut Context<Self>,
    ) {
        let reporting = self.reporting(mods, cx).is_some();
        let Some(tab) = self.with_active_tab(cx, |tab| {
            let arrows = tab.exited.is_none()
                && tab.emulator.alt_screen()
                && tab.emulator.alternate_scroll()
                && !mods.shift;
            if !(reporting || arrows) {
                let history = tab.emulator.history_lines() as f32;
                let target = (tab.emulator.display_offset() as f32 + tab.scroll_remainder + rows)
                    .clamp(0.0, history);
                let whole = target.floor();
                tab.emulator.scroll_to_offset(whole as usize);
                tab.scroll_remainder = target - whole;
                return None;
            }
            tab.wheel_remainder += rows;
            let step = tab.wheel_remainder.trunc() as i32;
            tab.wheel_remainder -= step as f32;
            Some((step, arrows, tab.emulator.app_cursor_mode()))
        }) else {
            return;
        };
        let Some((step, arrows, app_cursor)) = tab else {
            cx.notify();
            return;
        };
        if step == 0 {
            return;
        }
        if reporting {
            let button = if step > 0 {
                MOUSE_WHEEL_UP
            } else {
                MOUSE_WHEEL_DOWN
            };
            for _ in 0..step.unsigned_abs() {
                self.send_mouse(button, position, false, mods, cx);
            }
        } else if arrows {
            self.send_report(&wheel_arrows(step, app_cursor), cx);
        }
    }

    /// Copy the selection. Returns whether anything was copied, so the caller
    /// can decide whether to swallow the keystroke.
    fn copy_selection(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(text) = self
            .with_active_emulator(cx, |emu| emu.selection_text())
            .flatten()
        else {
            return false;
        };
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
        self.copied_at = Some(Instant::now());
        cx.notify();
        true
    }

    fn scroll_active(&mut self, delta_lines: i32, cx: &mut Context<Self>) {
        if delta_lines == 0 {
            return;
        }
        let chat = self.selected_chat(cx);
        let Some(tabs) = self.chats.get_mut(&chat) else {
            return;
        };
        let active = tabs.active;
        if let Some(tab) = tabs.tabs.get_mut(active) {
            tab.emulator.scroll(delta_lines);
            tab.scroll_remainder = 0.0;
            cx.notify();
        }
    }

    fn schedule_selection_scroll(&mut self, cx: &mut Context<Self>) {
        if self.selection_scroll_task.is_some() {
            return;
        }
        let (Some(drag), Some(geometry)) = (self.selection_drag, self.geometry) else {
            return;
        };
        if !drag.armed || selection_scroll_lines(geometry, drag.position) == 0 {
            return;
        }
        self.selection_scroll_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(SELECTION_SCROLL_TICK_MS))
                .await;
            let _ = this.update(cx, |panel, cx| {
                panel.selection_scroll_task = None;
                panel.step_selection_scroll(cx);
            });
        }));
    }

    fn step_selection_scroll(&mut self, cx: &mut Context<Self>) {
        let (Some(drag), Some(geometry)) = (self.selection_drag, self.geometry) else {
            return;
        };
        if !drag.armed {
            return;
        }
        let lines = selection_scroll_lines(geometry, drag.position);
        if lines == 0 {
            return;
        }
        self.scroll_active(lines, cx);
        if let Some((point, side)) = self.grid_point_at(drag.position, cx) {
            self.with_active_emulator(cx, |emu| emu.update_selection(point, side));
        }
        self.schedule_selection_scroll(cx);
    }

    /// Pin the rail to the rendered tab. The shared rail host methods run
    /// without an `&App`, so they cannot read the selected chat — render
    /// records the tab here and they resolve it by key. A switch also drops
    /// the rail's activity baseline, so the incoming tab's offset lands as a
    /// first observation rather than as scroll motion.
    fn sync_rail_tab(&mut self, cx: &App) {
        let key = self.active_tab(cx).map(|tab| tab.key);
        if self.rail_tab_key != key {
            self.rail_tab_key = key;
            self.bar.clear_scroll_baseline();
        }
    }

    fn rail_tab(&self) -> Option<&TerminalTab> {
        let key = self.rail_tab_key?;
        self.chats
            .values()
            .find_map(|tabs| tabs.tabs.iter().find(|tab| tab.key == key))
    }

    fn rail_tab_mut(&mut self) -> Option<&mut TerminalTab> {
        let key = self.rail_tab_key?;
        self.chats
            .values_mut()
            .find_map(|tabs| tabs.tabs.iter_mut().find(|tab| tab.key == key))
    }

    /// The rendered grid's rail geometry in the shared metrics domain, plus
    /// the track's window y (the grid bounds — the rail strip spans the
    /// terminal body) and the scroll position in px from the top of the
    /// scrollback.
    fn rail_frame(&self) -> Option<(MenuScrollbarMetrics, Pixels, f32)> {
        let geometry = self.geometry?;
        let tab = self.rail_tab()?;
        let (viewport, content, offset) = rail_parts(
            geometry.line_h,
            tab.emulator.rows(),
            tab.emulator.history_lines(),
            tab.emulator.display_offset(),
        );
        Some((
            MenuScrollbarMetrics::from_parts(viewport, content, offset)?,
            geometry.bounds.top(),
            offset,
        ))
    }

    /// Apply an engaged drag's target to the emulator. The shared target is a
    /// fraction of the scrollback from the TOP; the emulator's scroll-to API
    /// takes lines from the live bottom.
    fn apply_rail_drag(&mut self, pointer_y: Pixels) -> bool {
        let Some(line_h) = self.geometry.map(|geometry| geometry.line_h) else {
            return false;
        };
        let Some((metrics, track_top, _)) = self.rail_frame() else {
            return false;
        };
        let Some(fraction) = self.bar.drag_target_in(&metrics, track_top, pointer_y) else {
            return false;
        };
        let lines = (((1.0 - fraction) * metrics.max_scroll) / line_h).round() as usize;
        let Some(tab) = self.rail_tab_mut() else {
            return false;
        };
        tab.emulator.scroll_to_offset(lines);
        tab.scroll_remainder = 0.0;
        true
    }

    fn on_terminal_hover(&mut self, hovered: &bool, _window: &mut Window, cx: &mut Context<Self>) {
        if self.bar.set_list_hovered(*hovered) {
            cx.notify();
        }
    }

    fn render_scrollbar(
        &mut self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        self.sync_rail_tab(cx);
        crate::popover::rail(self, "terminal-scrollbar", theme, cx)
    }

    // ---- tab management ----

    fn select_tab(&mut self, chat: &str, ix: usize, cx: &mut Context<Self>) {
        if let Some(tabs) = self.chats.get_mut(chat)
            && ix < tabs.tabs.len()
            && tabs.active != ix
        {
            tabs.active = ix;
            cx.notify();
        }
    }

    fn close_tab(&mut self, chat: &str, key: u64, window: &mut Window, cx: &mut Context<Self>) {
        let engine = self.engine(cx);
        let Some(tabs) = self.chats.get_mut(chat) else {
            return;
        };
        let Some(ix) = tabs.tabs.iter().position(|t| t.key == key) else {
            return;
        };
        let tab = tabs.tabs.remove(ix);
        let target = tab.target_device_id.clone();
        tabs.active = active_after_close(tabs.active, ix, tabs.tabs.len());
        let now_empty = tabs.tabs.is_empty();
        self.drag = None;
        // Closing the LAST terminal closes the drawer too — an empty dock is
        // dead space (user request). Same path as the collapse chevron.
        // Embedded, the SHELL owns emptiness (it falls back to the surface
        // picker) — dispatching here would toggle the bottom drawer instead.
        if now_empty && self.open && !self.embedded {
            window.dispatch_action(Box::new(ToggleTerminal), cx);
        }
        if let (Some(engine), Some(id)) = (engine, tab.terminal_id.clone()) {
            cx.spawn(async move |_, _| {
                let _ = engine
                    .client()
                    .call(
                        methods::CLOSE_TERMINAL,
                        with_target(serde_json::json!({ "terminalId": id }), &target),
                    )
                    .await;
            })
            .detach();
        }
        cx.notify();
    }

    fn commit_reorder(&mut self, chat: &str, from: usize, to: usize, cx: &mut Context<Self>) {
        if let Some(tabs) = self.chats.get_mut(chat) {
            let active = tabs.active;
            reorder_tabs(&mut tabs.tabs, from, to);
            tabs.active = active_after_reorder(active, from, to);
        }
        self.drag = None;
        cx.notify();
    }

    fn update_drag_over(&mut self, from: usize, over: usize, cx: &mut Context<Self>) {
        match &mut self.drag {
            Some(drag) if drag.over != over => {
                drag.prev_over = drag.over;
                drag.over = over;
                drag.epoch += 1;
                cx.notify();
            }
            Some(_) => {}
            None => {
                self.drag = Some(DragState {
                    from,
                    over,
                    epoch: 0,
                    prev_over: from,
                });
                cx.notify();
            }
        }
    }

    // ---- render ----

    fn render_tab_bar(&mut self, chat: &str, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = Theme::of(cx).clone();
        let tabs = self.chats.get(chat);
        let (active, count) = tabs.map(|t| (t.active, t.tabs.len())).unwrap_or((0, 0));
        let drag = self
            .drag
            .as_ref()
            .map(|d| (d.from, d.over, d.epoch, d.prev_over));
        let chat_owned = chat.to_string();

        let tab_elements: Vec<_> = tabs
            .map(|tabs| {
                tabs.tabs
                    .iter()
                    .enumerate()
                    .map(|(ix, tab)| {
                        let selected = ix == active;
                        let key = tab.key;
                        // Contextual label (user request): the OSC title —
                        // the shell's own cwd/command name — wins over the
                        // fixed "Terminal N" fallback.
                        let title = Self::display_title(tab);
                        let exited = tab.exited.is_some();
                        (ix, key, title, selected, exited)
                    })
                    .collect()
            })
            .unwrap_or_default();

        let bar_chat = chat_owned.clone();
        let drop_chat = chat_owned.clone();
        // Zeron terminal-panel.tsx: `flex h-10 items-center border-b
        // border-white/[0.07] pl-2 pr-1.5` on the #090909 panel — no separate
        // bar fill.
        div()
            .id("terminal-tab-bar")
            .h(px(TAB_BAR_HEIGHT))
            .flex_none()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(4.0))
            .pl(px(8.0))
            .pr(px(6.0))
            .border_b_1()
            .border_color(crate::theme::hairline(0.07))
            .on_drag_move::<TabDragPayload>(cx.listener(
                move |this, event: &gpui::DragMoveEvent<TabDragPayload>, _, cx| {
                    let payload = event.drag(cx);
                    if payload.chat != bar_chat {
                        return;
                    }
                    let from = payload.from;
                    let rel_x = f32::from(event.event.position.x) - f32::from(event.bounds.left());
                    let over = drop_index(rel_x, TAB_WIDTH, count);
                    this.update_drag_over(from, over, cx);
                },
            ))
            .on_drop::<TabDragPayload>(cx.listener(move |this, payload: &TabDragPayload, _, cx| {
                if payload.chat != drop_chat {
                    this.drag = None;
                    cx.notify();
                    return;
                }
                let to = this.drag.as_ref().map(|d| d.over).unwrap_or(payload.from);
                let chat = drop_chat.clone();
                this.commit_reorder(&chat, payload.from, to, cx);
            }))
            .children(
                tab_elements
                    .into_iter()
                    .map(|(ix, key, title, selected, exited)| {
                        let chat_select = chat_owned.clone();
                        let chat_close = chat_owned.clone();
                        let chat_close2 = chat_owned.clone();
                        let chat_drag = chat_owned.clone();
                        let ghost_title = title.clone();
                        // Zeron tab: `h-7 rounded-lg pl-2 pr-1 gap-1.5 text-xs`,
                        // terminal glyph + label + close; active = white/8 wash.
                        let (text_color, bg, glyph_alpha) = if selected {
                            (theme.text, crate::theme::ink(0.08), 0.8)
                        } else {
                            (
                                theme.text_muted.opacity(0.6),
                                gpui::transparent_black(),
                                0.6,
                            )
                        };
                        let close_btn = div()
                            .id(("terminal-tab-close", key))
                            .size(px(20.0))
                            .flex_none()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(6.0))
                            .when(!selected, |el| el.invisible())
                            .cursor_pointer()
                            .hover(|s| s.bg(crate::theme::ink(0.09)))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.close_tab(&chat_close2, key, window, cx);
                            }))
                            .tooltip(crate::settings::widgets::text_tooltip("Close terminal"))
                            .child(
                                crate::icons::icon(crate::icons::CLOSE)
                                    .size(px(12.0))
                                    .text_color(theme.text_muted.opacity(0.8)),
                            );
                        let tab_el = div()
                            .id(("terminal-tab", key))
                            .w(px(TAB_WIDTH))
                            .h(px(28.0))
                            .flex_none()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap(px(6.0))
                            .pl(px(8.0))
                            .pr(px(4.0))
                            .rounded(px(8.0))
                            // zeron terminal-panel.tsx tab: `transition-colors`.
                            .bg(motion::hover_blend(
                                &format!("term-tab-{key}"),
                                bg,
                                theme.element_hover,
                            ))
                            .on_hover(motion::hover_listener(format!("term-tab-{key}")))
                            .text_size(px(12.0))
                            .text_color(text_color)
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.select_tab(&chat_select, ix, cx);
                                this.request_focus(cx);
                            }))
                            // Middle-click closes (§1.10).
                            .on_mouse_down(
                                MouseButton::Middle,
                                cx.listener(move |this, _, window, cx| {
                                    this.close_tab(&chat_close, key, window, cx);
                                }),
                            )
                            .on_drag(
                                TabDragPayload {
                                    chat: chat_drag,
                                    from: ix,
                                    title: ghost_title,
                                },
                                |payload, _point, _, cx| {
                                    let title = payload.title.clone();
                                    cx.stop_propagation();
                                    cx.new(|_| TabGhost { title })
                                },
                            )
                            .when(exited, |el| el.opacity(0.55))
                            .child(
                                crate::icons::icon(crate::icons::TERMINAL)
                                    .size(px(16.0))
                                    .text_color(text_color.opacity(glyph_alpha)),
                            )
                            .child(div().flex_1().min_w_0().truncate().child(title))
                            .child(close_btn);

                        // Sliding transform while a sibling is dragged over: animate
                        // 150 ms between committed offsets.
                        match drag {
                            Some((from, over, epoch, prev_over)) if ix != from => {
                                let target = slide_offset(ix, from, over) * TAB_WIDTH;
                                let start = slide_offset(ix, from, prev_over) * TAB_WIDTH;
                                div()
                                    .relative()
                                    .child(tab_el.with_animation(
                                        ("terminal-tab-slide", key | ((epoch as u64) << 32)),
                                        TAB_SLIDE.animation(),
                                        move |el, t| el.left(px(motion::lerp(start, target, t))),
                                    ))
                                    .into_any_element()
                            }
                            // Invisible spacer — the ghost carries the tab; a
                            // dimmed original overlapped the sibling that
                            // slides into the vacated slot.
                            Some((from, ..)) if ix == from => div()
                                .w(px(TAB_WIDTH))
                                .h(px(28.0))
                                .flex_none()
                                .into_any_element(),
                            _ => tab_el.into_any_element(),
                        }
                    }),
            )
            .child(
                div()
                    .id("terminal-new-tab")
                    .size(px(28.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(8.0))
                    .cursor_pointer()
                    // zeron terminal-panel.tsx icon buttons: `transition-colors`.
                    .bg(motion::hover_blend(
                        "term-new-tab",
                        gpui::transparent_black(),
                        crate::theme::ink(0.05),
                    ))
                    .on_hover(motion::hover_listener("term-new-tab"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        let chat = this.selected_chat(cx);
                        this.open_tab(chat, cx);
                        this.request_focus(cx);
                    }))
                    .tooltip(crate::settings::widgets::text_tooltip("New terminal"))
                    .child(
                        crate::icons::icon(crate::icons::PLUS)
                            .size(px(16.0))
                            .text_color(theme.text_muted.opacity(0.6)),
                    ),
            )
            // Collapse chevron pinned right (zeron "Hide terminal" ⌘J).
            .child(div().flex_1())
            .child(
                div()
                    .id("terminal-collapse")
                    .size(px(28.0))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(8.0))
                    .cursor_pointer()
                    .bg(motion::hover_blend(
                        "term-collapse",
                        gpui::transparent_black(),
                        crate::theme::ink(0.05),
                    ))
                    .on_hover(motion::hover_listener("term-collapse"))
                    .on_click(|_, window, cx| {
                        window.dispatch_action(Box::new(ToggleTerminal), cx);
                    })
                    .tooltip(crate::settings::widgets::text_tooltip("Hide terminal"))
                    .child(
                        crate::icons::icon(crate::icons::ALT_ARROW_DOWN)
                            .size(px(13.0))
                            .text_color(theme.text_muted.opacity(0.55)),
                    ),
            )
    }
}

impl ScrollRailHost for TerminalPanel {
    fn rail_bar(&mut self) -> &mut MenuScrollbarState {
        &mut self.bar
    }

    /// Handle-less owner: the grid's own extents stand in for a scroll
    /// handle's bounds.
    fn rail_metrics(&mut self) -> Option<MenuScrollbarMetrics> {
        let (metrics, _, offset) = self.rail_frame()?;
        // The activity signal is the view's place in the scrollback, not the
        // thumb's: appended output grows history and display offset together
        // (the viewport stays anchored) and a resize trades history rows for
        // viewport rows, so neither moves this value — only real scrolling
        // lights the rail. Tab switches re-baseline in [`Self::sync_rail_tab`].
        self.bar.note_scroll_offset(offset);
        Some(metrics)
    }

    fn rail_press(&mut self, pointer_y: Pixels) -> bool {
        let Some((metrics, track_top, _)) = self.rail_frame() else {
            return false;
        };
        self.bar.begin_press_in(&metrics, track_top, pointer_y);
        // Pressing the rail claims terminal focus, as it did before the
        // shared rail; the next render's focus_pending pass applies it.
        self.focus_pending = true;
        self.apply_rail_drag(pointer_y);
        true
    }

    fn rail_drag_to(&mut self, pointer_y: Pixels) -> bool {
        self.apply_rail_drag(pointer_y)
    }
}

/// Typed text arrives here from the platform's text input, after any input
/// method (CJK composition, dead keys, layout symbols) has done its work.
/// Composition shows at the cursor until it commits.
impl gpui::EntityInputHandler for TerminalPanel {
    fn text_for_range(
        &mut self,
        range: std::ops::Range<usize>,
        adjusted_range: &mut Option<std::ops::Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<String> {
        let marked = self.active_tab(cx)?.marked_text.clone()?;
        let utf16: Vec<u16> = marked.encode_utf16().collect();
        let range = range.start.min(utf16.len())..range.end.min(utf16.len());
        *adjusted_range = Some(range.clone());
        Some(String::from_utf16_lossy(&utf16[range]))
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::UTF16Selection> {
        let end = self
            .active_tab(cx)
            .and_then(|tab| tab.marked_text.as_ref())
            .map_or(0, |text| text.encode_utf16().count());
        Some(gpui::UTF16Selection {
            range: end..end,
            reversed: false,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<std::ops::Range<usize>> {
        let text = self.active_tab(cx)?.marked_text.as_ref()?;
        Some(0..text.encode_utf16().count())
    }

    /// The platform accepts the composition as typed (focus moving away
    /// mid-composition, for one): it goes to the PTY rather than vanishing.
    fn unmark_text(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let marked = self
            .with_active_tab(cx, |tab| tab.marked_text.take())
            .flatten();
        if let Some(text) = marked.filter(|text| !text.is_empty()) {
            self.type_text(&text, cx);
        }
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        _range: Option<std::ops::Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.with_active_tab(cx, |tab| tab.marked_text = None);
        if !text.is_empty() {
            self.type_text(text, cx);
        }
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range: Option<std::ops::Range<usize>>,
        new_text: &str,
        _new_selected_range: Option<std::ops::Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let marked = (!new_text.is_empty()).then(|| new_text.to_string());
        self.with_active_tab(cx, |tab| tab.marked_text = marked);
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        _range_utf16: std::ops::Range<usize>,
        _element_bounds: gpui::Bounds<Pixels>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<gpui::Bounds<Pixels>> {
        // The candidate window opens under the cursor cell.
        let geometry = self.geometry?;
        let cursor = self.active_tab(cx)?.emulator.cursor()?;
        Some(gpui::Bounds::new(
            gpui::point(
                geometry.origin.x + px(geometry.cell_w * cursor.col as f32),
                geometry.origin.y + px(geometry.line_h * cursor.row as f32),
            ),
            gpui::size(px(geometry.cell_w), px(geometry.line_h)),
        ))
    }

    fn character_index_for_point(
        &mut self,
        _point: gpui::Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}

enum StreamDisposition {
    Continue,
    Stop,
}

impl Render for TerminalPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::of(cx).clone();
        // Heal drag state if the pointer was released outside the bar.
        if self.drag.is_some() && !cx.has_active_drag() {
            self.drag = None;
        }
        // Embedded, the RIGHT PANE's own surface shows through — a second
        // fill here stacked another shade on the pane (user report); the
        // drawer keeps its own tone.
        let panel_bg: Option<gpui::Hsla> = (!self.embedded).then(|| terminal_panel_bg(&theme));
        // Docked at the window's bottom edge, the panel's own fill carries
        // the CSD window's bottom corners when it sits at them (the shell
        // decides per frame; embedded panels never do).
        let corner_bl = self.window_corner_bl;
        let corner_br = self.window_corner_br;
        let corner = px(crate::shell::LINUX_WINDOW_CORNER_RADIUS);
        let chat = self.selected_chat(cx);
        if std::mem::take(&mut self.focus_pending) && self.open {
            window.focus(&self.focus_handle, cx);
        }
        let focused = self.focus_handle.is_focused(window);
        self.sync_focus(focused, window.is_window_active(), cx);
        let scrollbar = self.render_scrollbar(&theme, cx);

        // Embedded (right-pane surface host): the shell's surface tabs
        // replace the internal bar.
        let tab_bar: Option<gpui::AnyElement> =
            (!self.embedded).then(|| self.render_tab_bar(&chat, cx).into_any_element());
        div()
            .size_full()
            .flex()
            .flex_col()
            // Terminal chrome is fixed Geist; TerminalElement measures and
            // paints its viewport independently with the technical mono role.
            .font_family(theme.font_sans_fixed.clone())
            .when_some(panel_bg, |el, bg| el.bg(bg))
            .when(corner_bl, |el| el.rounded_bl(corner))
            .when(corner_br, |el| el.rounded_br(corner))
            .children(tab_bar)
            .child(
                div()
                    .id("terminal-body")
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .key_context("Terminal")
                    .track_focus(&self.focus_handle)
                    .on_hover(cx.listener(Self::on_terminal_hover))
                    .on_key_down(cx.listener(Self::on_key_down))
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
                    .on_mouse_move(cx.listener(Self::on_mouse_move))
                    // Bound on the window, not the element: a drag that ends
                    // outside the panel still has to end the gesture, or the
                    // next unrelated pointer move keeps extending a selection
                    // the user let go of.
                    .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
                    .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
                    .on_mouse_down(MouseButton::Middle, cx.listener(Self::on_other_mouse_down))
                    .on_mouse_down(MouseButton::Right, cx.listener(Self::on_other_mouse_down))
                    .on_mouse_up(MouseButton::Middle, cx.listener(Self::on_mouse_up))
                    .on_mouse_up(MouseButton::Right, cx.listener(Self::on_mouse_up))
                    // A reported press released outside the panel must still
                    // tell the program the button went up.
                    .on_mouse_up_out(MouseButton::Middle, cx.listener(Self::on_mouse_up))
                    .on_mouse_up_out(MouseButton::Right, cx.listener(Self::on_mouse_up))
                    .on_scroll_wheel(cx.listener(|this, event: &gpui::ScrollWheelEvent, _, cx| {
                        let lines = match event.delta {
                            ScrollDelta::Lines(delta) => delta.y,
                            // Against the measured row height, not the default:
                            // a user-chosen font size changes how many lines a
                            // pixel delta covers.
                            ScrollDelta::Pixels(delta) => {
                                let line_h = this
                                    .geometry
                                    .map(|g| g.line_h)
                                    .unwrap_or(super::view::TERM_LINE_HEIGHT);
                                f32::from(delta.y) / line_h
                            }
                        };
                        // A fresh gesture starts from where the view rests:
                        // the partial row is on screen, so dropping it would
                        // jump. Only the rows owed to a program start over.
                        if event.touch_phase == gpui::TouchPhase::Started {
                            this.with_active_tab(cx, |tab| tab.wheel_remainder = 0.0);
                        }
                        this.wheel(lines, event.position, &event.modifiers, cx);
                        cx.stop_propagation();
                    }))
                    .child(TerminalElement::new(cx.entity(), focused))
                    .children(scrollbar),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A focused panel with one live tab (no engine: writes stay queued, so
    /// the test reads exactly what would reach the PTY).
    fn focused_panel(
        cx: &mut gpui::TestAppContext,
    ) -> (Entity<TerminalPanel>, &mut gpui::VisualTestContext) {
        use gpui::AppContext;
        cx.update(|cx| {
            gpui_base::init(cx);
            cx.set_global(Theme::default());
        });
        let (panel, cx) = cx.add_window_view(|_, cx| {
            let state = cx.new(|_| {
                let mut state = AppState::new();
                state.selected_chat = Some("chat".into());
                state
            });
            let mut panel = TerminalPanel::new(state, cx);
            let key = panel.reserve_tab_for_chat("chat".into(), "Shell", cx);
            panel.tab_mut("chat", key).unwrap().emulator.feed(b"$ ");
            panel.open = true;
            panel
        });
        cx.update(|window, cx| {
            let focus = panel.read(cx).focus_handle();
            window.focus(&focus, cx);
            window.draw(cx).clear();
        });
        (panel, cx)
    }

    fn sent(panel: &Entity<TerminalPanel>, cx: &mut gpui::VisualTestContext) -> Vec<u8> {
        panel.read_with(cx, |panel, cx| {
            panel.active_tab(cx).unwrap().input.pending().to_vec()
        })
    }

    #[gpui::test]
    fn typing_reaches_the_pty_through_text_input_in_order(cx: &mut gpui::TestAppContext) {
        let (panel, cx) = focused_panel(cx);
        cx.simulate_keystrokes("l s space - a enter");
        assert_eq!(sent(&panel, cx), b"ls -a\r");
        cx.simulate_keystrokes("ctrl-c alt-b");
        assert_eq!(sent(&panel, cx), b"ls -a\r\x03\x1bb");
    }

    #[gpui::test]
    fn a_mouse_aware_program_gets_reports_and_shift_still_selects(cx: &mut gpui::TestAppContext) {
        use gpui::Modifiers;
        let (panel, cx) = focused_panel(cx);
        panel.update(cx, |panel, cx| {
            panel.with_active_tab(cx, |tab| tab.emulator.feed(b"\x1b[?1000h\x1b[?1006h"));
        });
        cx.update(|window, cx| window.draw(cx).clear());
        let geometry = panel.read_with(cx, |panel, _| panel.geometry.unwrap());
        let at =
            geometry.origin + gpui::point(px(geometry.cell_w * 2.5), px(geometry.line_h * 1.5));
        cx.simulate_mouse_down(at, MouseButton::Left, Modifiers::default());
        cx.simulate_mouse_up(at, MouseButton::Left, Modifiers::default());
        assert_eq!(sent(&panel, cx), b"\x1b[<0;3;2M\x1b[<0;3;2m");
        // Shift: the press is ours (selection), not the program's.
        cx.simulate_mouse_down(at, MouseButton::Left, Modifiers::shift());
        cx.simulate_mouse_up(at, MouseButton::Left, Modifiers::shift());
        assert_eq!(sent(&panel, cx), b"\x1b[<0;3;2M\x1b[<0;3;2m");
    }

    #[gpui::test]
    fn focus_changes_are_reported_to_programs_that_ask(cx: &mut gpui::TestAppContext) {
        let (panel, cx) = focused_panel(cx);
        panel.update(cx, |panel, cx| {
            panel.with_active_tab(cx, |tab| {
                tab.emulator.feed(b"\x1b[?1004h");
                for _ in 0..100 {
                    tab.emulator.feed(b"history\r\n");
                }
                tab.emulator.scroll(20);
            });
        });
        cx.update(|window, cx| {
            window.blur();
            window.draw(cx).clear();
        });
        assert!(sent(&panel, cx).ends_with(b"\x1b[O"));
        panel.read_with(cx, |panel, cx| {
            assert_eq!(
                panel.active_tab(cx).unwrap().emulator.display_offset(),
                20,
                "a focus report must not snap the scrollback"
            );
        });
        cx.update(|window, cx| {
            let focus = panel.read(cx).focus_handle();
            window.focus(&focus, cx);
            window.draw(cx).clear();
        });
        assert!(sent(&panel, cx).ends_with(b"\x1b[O\x1b[I"));
    }

    #[gpui::test]
    fn slow_trackpad_scroll_accumulates_per_terminal(cx: &mut gpui::TestAppContext) {
        use gpui::{AppContext, Modifiers, ScrollWheelEvent, TouchPhase, point};

        cx.update(|cx| {
            gpui_base::init(cx);
            cx.set_global(Theme::default());
        });
        let (panel, cx) = cx.add_window_view(|_, cx| {
            let state = cx.new(|_| {
                let mut state = AppState::new();
                state.selected_chat = Some("chat".into());
                state
            });
            let mut panel = TerminalPanel::new(state, cx);
            for title in ["First", "Second"] {
                let key = panel.reserve_tab_for_chat("chat".into(), title, cx);
                let tab = panel.tab_mut("chat", key).unwrap();
                for _ in 0..200 {
                    tab.emulator.feed(b"scrollback\r\n");
                }
            }
            panel
        });
        cx.update(|window, cx| window.draw(cx).clear());
        let geometry = panel.read_with(cx, |panel, _| panel.geometry.unwrap());
        let mut scroll = |delta, phase| {
            cx.simulate_event(ScrollWheelEvent {
                position: geometry.bounds.center(),
                delta,
                modifiers: Modifiers::default(),
                touch_phase: phase,
            })
        };
        // Every event is smaller than half a row: rounding each one loses all movement.
        for _ in 0..40 {
            scroll(
                ScrollDelta::Pixels(point(px(0.0), px(geometry.line_h / 4.0))),
                TouchPhase::Moved,
            );
        }
        panel.read_with(cx, |panel, cx| {
            assert_eq!(panel.active_tab(cx).unwrap().emulator.display_offset(), 10);
        });
        // Signed fractions work in reverse too; mouse-wheel rows stay exact.
        let mut scroll = |delta, phase| {
            cx.simulate_event(ScrollWheelEvent {
                position: geometry.bounds.center(),
                delta,
                modifiers: Modifiers::default(),
                touch_phase: phase,
            })
        };
        for _ in 0..8 {
            scroll(
                ScrollDelta::Pixels(point(px(0.0), px(-geometry.line_h / 4.0))),
                TouchPhase::Moved,
            );
        }
        scroll(ScrollDelta::Lines(point(0.0, 3.0)), TouchPhase::Moved);
        scroll(ScrollDelta::Lines(point(0.0, 0.75)), TouchPhase::Moved);
        panel.read_with(cx, |panel, cx| {
            assert_eq!(panel.active_tab(cx).unwrap().emulator.display_offset(), 11);
        });
        // The partial row is on screen, so a fresh gesture continues from it.
        cx.simulate_event(ScrollWheelEvent {
            position: geometry.bounds.center(),
            delta: ScrollDelta::Lines(point(0.0, 0.5)),
            modifiers: Modifiers::default(),
            touch_phase: TouchPhase::Started,
        });
        panel.update(cx, |panel, cx| {
            assert_eq!(panel.active_tab(cx).unwrap().emulator.display_offset(), 12);
            assert_eq!(panel.active_tab(cx).unwrap().scroll_remainder, 0.25);
            // Scrolling past the live bottom rests flush with it.
            for _ in 0..30 {
                panel.wheel(-0.7, geometry.bounds.center(), &Modifiers::default(), cx);
            }
            assert_eq!(panel.active_tab(cx).unwrap().emulator.display_offset(), 0);
            assert_eq!(panel.active_tab(cx).unwrap().scroll_remainder, 0.0);
            panel.wheel(11.25, geometry.bounds.center(), &Modifiers::default(), cx);
            assert_eq!(panel.active_tab(cx).unwrap().emulator.display_offset(), 11);
            panel.select_tab_by_key(1, cx);
        });
        cx.update(|window, cx| window.draw(cx).clear());
        cx.simulate_event(ScrollWheelEvent {
            position: geometry.bounds.center(),
            delta: ScrollDelta::Lines(point(0.0, 0.75)),
            modifiers: Modifiers::default(),
            touch_phase: TouchPhase::Moved,
        });
        panel.read_with(cx, |panel, cx| {
            assert_eq!(panel.active_tab(cx).unwrap().emulator.display_offset(), 0);
            assert_eq!(panel.active_tab(cx).unwrap().scroll_remainder, 0.75);
        });
    }

    #[test]
    fn height_clamps_between_160_and_55vh() {
        assert_eq!(clamp_terminal_height(300.0, 900.0), 300.0);
        assert_eq!(clamp_terminal_height(10.0, 900.0), 160.0);
        assert_eq!(clamp_terminal_height(4000.0, 900.0), 900.0 * 0.55);
        // Tiny windows: min wins over the 55vh cap.
        assert_eq!(clamp_terminal_height(200.0, 100.0), 160.0);
        assert_eq!(clamp_terminal_height(f32::NAN, 900.0), 160.0);
    }

    #[test]
    fn backoff_doubles_and_caps() {
        assert_eq!(backoff_ms(0), 500);
        assert_eq!(backoff_ms(1), 1000);
        assert_eq!(backoff_ms(2), 2000);
        assert_eq!(backoff_ms(3), 4000);
        assert_eq!(backoff_ms(4), 8000);
        assert_eq!(backoff_ms(10), 8000);
        assert_eq!(backoff_ms(u32::MAX), 8000);
    }

    fn test_geometry() -> GridGeometry {
        GridGeometry {
            bounds: gpui::Bounds::new(
                gpui::point(px(10.0), px(20.0)),
                gpui::size(px(300.0), px(200.0)),
            ),
            origin: gpui::point(px(18.0), px(28.0)),
            cell_w: 8.0,
            line_h: 20.0,
            cols: 35,
            rows: 9,
        }
    }

    #[test]
    fn selection_edge_scroll_uses_terminal_direction() {
        let geometry = test_geometry();
        assert!(selection_scroll_lines(geometry, gpui::point(px(20.0), px(28.0))) > 0);
        assert_eq!(
            selection_scroll_lines(geometry, gpui::point(px(20.0), px(100.0))),
            0
        );
        assert!(selection_scroll_lines(geometry, gpui::point(px(20.0), px(208.0))) < 0);
    }

    #[test]
    fn rail_parts_map_history_top_and_bottom() {
        // No scrollback → the shared metrics report nothing to scroll.
        assert_eq!(rail_parts(18.0, 20, 0, 0), (360.0, 360.0, 0.0));
        assert!(MenuScrollbarMetrics::from_parts(360.0, 360.0, 0.0).is_none());

        // Live bottom: the display offset counts from the bottom, so the
        // scrollback position from the top is the whole history.
        let (viewport, content, bottom) = rail_parts(18.0, 20, 80, 0);
        assert_eq!((viewport, content, bottom), (360.0, 1800.0, 1440.0));
        let metrics = MenuScrollbarMetrics::from_parts(viewport, content, bottom).unwrap();
        assert!((metrics.thumb_height - 70.4).abs() < 0.01);
        assert!((metrics.thumb_top - metrics.travel()).abs() < 0.01);

        // Top of the history: the thumb rides the top of the track.
        let top = rail_parts(18.0, 20, 80, 80).2;
        assert_eq!(
            MenuScrollbarMetrics::from_parts(viewport, content, top)
                .unwrap()
                .thumb_top,
            0.0
        );

        // Output appended while scrolled up grows history and display offset
        // together, so the position from the top — the rail's activity signal
        // — does not move: no false flash.
        assert_eq!(
            rail_parts(18.0, 20, 80, 40).2,
            rail_parts(18.0, 20, 120, 80).2
        );
    }

    #[test]
    fn reorder_moves_forward_and_backward() {
        let mut v = vec!["a", "b", "c", "d"];
        reorder_tabs(&mut v, 0, 2);
        assert_eq!(v, ["b", "c", "a", "d"]);
        reorder_tabs(&mut v, 3, 0);
        assert_eq!(v, ["d", "b", "c", "a"]);
        // Out-of-range / no-op moves leave the vec untouched.
        reorder_tabs(&mut v, 9, 0);
        reorder_tabs(&mut v, 1, 1);
        assert_eq!(v, ["d", "b", "c", "a"]);
    }

    #[test]
    fn drop_index_quantizes_and_clamps() {
        assert_eq!(drop_index(-10.0, 150.0, 3), 0);
        assert_eq!(drop_index(0.0, 150.0, 3), 0);
        assert_eq!(drop_index(149.0, 150.0, 3), 0);
        assert_eq!(drop_index(150.0, 150.0, 3), 1);
        assert_eq!(drop_index(700.0, 150.0, 3), 2);
        assert_eq!(drop_index(50.0, 150.0, 0), 0);
    }

    #[test]
    fn slide_offsets_shift_toward_the_gap() {
        // Dragging 0 over 2: tabs 1 and 2 slide left one slot.
        assert_eq!(slide_offset(0, 0, 2), 0.0);
        assert_eq!(slide_offset(1, 0, 2), -1.0);
        assert_eq!(slide_offset(2, 0, 2), -1.0);
        assert_eq!(slide_offset(3, 0, 2), 0.0);
        // Dragging 3 over 1: tabs 1 and 2 slide right.
        assert_eq!(slide_offset(0, 3, 1), 0.0);
        assert_eq!(slide_offset(1, 3, 1), 1.0);
        assert_eq!(slide_offset(2, 3, 1), 1.0);
        assert_eq!(slide_offset(3, 3, 1), 0.0);
        // Hovering the origin: nothing moves.
        for ix in 0..4 {
            assert_eq!(slide_offset(ix, 2, 2), 0.0);
        }
    }

    #[test]
    fn active_index_tracks_reorders() {
        // The active tab itself moves.
        assert_eq!(active_after_reorder(1, 1, 3), 3);
        // A tab hopping over the active one from the left shifts it down.
        assert_eq!(active_after_reorder(2, 0, 3), 1);
        // …and from the right shifts it up.
        assert_eq!(active_after_reorder(1, 3, 0), 2);
        // Disjoint moves leave it alone.
        assert_eq!(active_after_reorder(0, 2, 3), 0);
    }

    #[test]
    fn active_index_tracks_closes() {
        assert_eq!(active_after_close(2, 0, 3), 1); // close left of active
        assert_eq!(active_after_close(1, 1, 2), 1); // close active mid-list
        assert_eq!(active_after_close(2, 2, 2), 1); // close active at tail
        assert_eq!(active_after_close(0, 0, 0), 0); // last tab closed
    }

    #[test]
    fn exit_message_format() {
        let text = String::from_utf8(exit_message(0)).unwrap();
        assert!(text.contains("[process exited 0]"));
        let text = String::from_utf8(exit_message(137)).unwrap();
        assert!(text.contains("[process exited 137]"));
        assert!(text.starts_with("\r\n"));
        assert!(text.ends_with("\r\n"));
    }

    #[test]
    fn shell_titles() {
        assert_eq!(shell_title("/bin/zsh"), "zsh");
        assert_eq!(shell_title("/usr/local/bin/fish"), "fish");
        assert_eq!(shell_title("C:\\Windows\\System32\\cmd.exe"), "cmd.exe");
        assert_eq!(shell_title("bash"), "bash");
        assert_eq!(shell_title(""), "terminal");
    }

    #[test]
    fn stream_events_deserialize_per_contract() {
        let data: TerminalEvent =
            serde_json::from_str(r#"{"type":"data","seq":7,"data":"aGk="}"#).unwrap();
        assert_eq!(
            data,
            TerminalEvent::Data {
                seq: 7,
                data: "aGk=".into()
            }
        );
        let exit: TerminalEvent =
            serde_json::from_str(r#"{"type":"exit","seq":8,"exitCode":130}"#).unwrap();
        assert_eq!(
            exit,
            TerminalEvent::Exit {
                seq: 8,
                exit_code: 130,
                signal: None
            }
        );
        let session: TerminalSession =
            serde_json::from_str(r#"{"id":"t1","cwd":"/w","shell":"/bin/zsh"}"#).unwrap();
        assert_eq!(session.id, "t1");
        assert_eq!(session.shell, "/bin/zsh");
    }

    #[test]
    fn base64_round_trip_and_tolerance() {
        assert_eq!(decode_base64("aGk="), b"hi".to_vec());
        assert_eq!(
            decode_base64("aGk"),
            b"hi".to_vec(),
            "unpadded input tolerated"
        );
        assert_eq!(
            decode_base64("!!!"),
            Vec::<u8>::new(),
            "garbage decodes to nothing"
        );
        assert_eq!(encode_base64(b"hi"), "aGk=");
    }

    #[test]
    fn exit_message_feeds_cleanly_through_the_emulator() {
        let mut emulator = Emulator::new(40, 4);
        emulator.feed(b"$ done");
        emulator.feed(&exit_message(1));
        assert_eq!(emulator.row_text(1), "[process exited 1]");
    }
}
