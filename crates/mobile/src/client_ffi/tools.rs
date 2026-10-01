//! Developer tools over FFI: streaming host RPCs ([`CoreClient::host_watch`]),
//! engine terminals rendered by the shared emulator ([`TerminalScreen`]),
//! source highlighting and file icons for the file tree and editor.
//!
//! Streams and terminals ride [`zc::Client::host_watch`], so they reach the
//! phone's own engine over its IPC port and any other device over the relay,
//! exactly like `host_call`.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use base64::Engine as _;
use zeron_client as zc;
use zeron_vt::keys::{self, Key, Modifiers};
use zeron_vt::{CellColor, CellSnapshot, Emulator, SelectionType, Side};

use super::{CoreClient, on_runtime};
use crate::layout::display::ColorRole;

use super::types::{CoreError, CoreResult};

fn parse_params(params_json: &str) -> CoreResult<serde_json::Value> {
    if params_json.trim().is_empty() {
        return Ok(serde_json::json!({}));
    }
    serde_json::from_str(params_json).map_err(|e| CoreError::InvalidArgument {
        message: format!("params: {e}"),
    })
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

// ── host streams ────────────────────────────────────────────────────────────

/// Receives one host stream's items (JSON) on a client thread, in order.
/// Hop to the main thread before touching UI state.
#[uniffi::export(with_foreign)]
pub trait HostStreamListener: Send + Sync {
    fn on_item(&self, json: String);
    /// The host ended the stream (a terminal exited), the connection dropped
    /// or the client shut down. Not called after `cancel()`. Re-subscribe to
    /// resume.
    fn on_end(&self);
}

struct StreamSink(Arc<dyn HostStreamListener>);

impl zc::rpc::HostWatchSink for StreamSink {
    fn item(&self, value: serde_json::Value) {
        self.0.on_item(value.to_string());
    }

    fn ended(&self) {
        self.0.on_end();
    }
}

/// A running host stream. `cancel()` — or releasing the object — stops it
/// and cancels the host side.
#[derive(uniffi::Object)]
pub struct HostStream {
    watch: zc::rpc::HostWatch,
}

#[uniffi::export]
impl HostStream {
    pub fn cancel(&self) {
        self.watch.cancel();
    }

    pub fn is_active(&self) -> bool {
        self.watch.is_active()
    }
}

#[uniffi::export]
impl CoreClient {
    /// Untyped host stream (`WatchWorkspaceFiles`, `WatchWorkspaceGitStatus`,
    /// `WatchPreviews`, `SubscribeTerminal`…) from `device_id`'s engine.
    /// Returns once the host accepted it — an unknown method or rejected
    /// params fail here — then delivers items to `listener`.
    pub async fn host_watch(
        &self,
        device_id: String,
        method: String,
        params_json: String,
        listener: Arc<dyn HostStreamListener>,
    ) -> CoreResult<Arc<HostStream>> {
        let params = parse_params(&params_json)?;
        let client = self.client.clone();
        let sink: Arc<dyn zc::rpc::HostWatchSink> = Arc::new(StreamSink(listener));
        let watch =
            on_runtime(async move { client.host_watch(&device_id, &method, params, sink).await })
                .await?;
        Ok(Arc::new(HostStream { watch }))
    }

    /// Attach a view to engine terminal `terminal_id` on `device_id` (opened
    /// with `OpenTerminal`): replays its scrollback, then follows live output,
    /// re-subscribing from the last seen `seq` if the stream drops. Releasing
    /// the screen detaches (the shell keeps running); `kill()` ends it.
    pub fn terminal_screen(
        &self,
        device_id: String,
        terminal_id: String,
        cols: u16,
        rows: u16,
        palette: TerminalPalette,
        listener: Arc<dyn TerminalListener>,
    ) -> Arc<TerminalScreen> {
        TerminalScreen::start(
            self.client.clone(),
            device_id,
            terminal_id,
            cols,
            rows,
            palette,
            listener,
        )
    }
}

// ── terminal ────────────────────────────────────────────────────────────────

/// Notified when a terminal needs repainting. Coalesced: after `on_frame`
/// nothing more is sent until the next `frame()` pull.
#[uniffi::export(with_foreign)]
pub trait TerminalListener: Send + Sync {
    fn on_frame(&self);
    /// The shell exited (the frame already shows the exit line).
    fn on_exit(&self, code: i32);
}

/// Theme colors the emulator resolves cells against (ARGB).
#[derive(Debug, Clone, uniffi::Record)]
pub struct TerminalPalette {
    pub foreground: u32,
    pub background: u32,
    /// ANSI 0–15.
    pub ansi: Vec<u32>,
    pub cursor: u32,
    pub selection: u32,
    /// Light appearance mirrors the 232–255 grayscale ramp (as the desktop).
    pub light: bool,
}

/// A run of cells sharing a style, starting at `col` and spanning `width`
/// cells. Wide characters get their own run (width 2).
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct TerminalRun {
    pub col: u16,
    pub width: u16,
    pub text: String,
    pub fg: u32,
    /// `0` = the terminal background (nothing to paint).
    pub bg: u32,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct TerminalLine {
    pub runs: Vec<TerminalRun>,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct TerminalFrame {
    pub cols: u16,
    pub rows: u16,
    pub lines: Vec<TerminalLine>,
    /// `-1` when hidden or scrolled out of view.
    pub cursor_row: i32,
    pub cursor_col: i32,
    /// Lines scrolled back (0 = following live output).
    pub display_offset: u32,
    pub history: u32,
    pub title: Option<String>,
    pub exit_code: Option<i32>,
    pub has_selection: bool,
    /// Output hasn't arrived yet (the first replay is in flight).
    pub connecting: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum TerminalKey {
    Enter,
    Backspace,
    Tab,
    Escape,
    Up,
    Down,
    Right,
    Left,
    Home,
    End,
    Insert,
    Delete,
    PageUp,
    PageDown,
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
}

impl From<TerminalKey> for Key {
    fn from(key: TerminalKey) -> Self {
        use TerminalKey as T;
        match key {
            T::Enter => Key::Enter,
            T::Backspace => Key::Backspace,
            T::Tab => Key::Tab,
            T::Escape => Key::Escape,
            T::Up => Key::Up,
            T::Down => Key::Down,
            T::Right => Key::Right,
            T::Left => Key::Left,
            T::Home => Key::Home,
            T::End => Key::End,
            T::Insert => Key::Insert,
            T::Delete => Key::Delete,
            T::PageUp => Key::PageUp,
            T::PageDown => Key::PageDown,
            T::F1 => Key::F(1),
            T::F2 => Key::F(2),
            T::F3 => Key::F(3),
            T::F4 => Key::F(4),
            T::F5 => Key::F(5),
            T::F6 => Key::F(6),
            T::F7 => Key::F(7),
            T::F8 => Key::F(8),
            T::F9 => Key::F(9),
            T::F10 => Key::F(10),
            T::F11 => Key::F(11),
            T::F12 => Key::F(12),
        }
    }
}

/// Selection granularity for [`TerminalScreen::select_start`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum TerminalSelection {
    Simple,
    Word,
    Line,
}

/// Longest `WriteTerminal` payload (the engine caps one call at 64 KiB).
const WRITE_CHUNK: usize = 48 * 1024;

fn exit_message(code: i32) -> Vec<u8> {
    format!("\r\n\x1b[90m[process exited {code}]\x1b[0m\r\n").into_bytes()
}

fn backoff(attempt: u32) -> Duration {
    Duration::from_millis((250u64 << attempt.min(5)).min(5_000))
}

struct TerminalState {
    emulator: Emulator,
    palette: TerminalPalette,
    last_seq: u64,
    exit_code: Option<i32>,
    received: bool,
}

struct TerminalInner {
    client: zc::Client,
    device_id: String,
    terminal_id: String,
    state: Mutex<TerminalState>,
    listener: Arc<dyn TerminalListener>,
    /// A frame is waiting to be pulled: suppress further `on_frame`s.
    dirty: AtomicBool,
    closed: AtomicBool,
    watch: Mutex<Option<zc::rpc::HostWatch>>,
    /// Ordered PTY input (keys, pastes, query responses).
    input: tokio::sync::mpsc::UnboundedSender<Vec<u8>>,
    /// Bumped per resize; a stale debounce skips its RPC.
    resize_gen: AtomicU64,
}

impl TerminalInner {
    fn notify(&self) {
        if !self.dirty.swap(true, Ordering::AcqRel) {
            self.listener.on_frame();
        }
    }

    fn apply(&self, value: serde_json::Value) {
        let event: zeron_proto::TerminalEvent = match serde_json::from_value(value) {
            Ok(event) => event,
            Err(_) => return,
        };
        let mut state = lock(&self.state);
        state.received = true;
        match event {
            zeron_proto::TerminalEvent::Data { seq, data } => {
                if seq <= state.last_seq && state.last_seq != 0 {
                    return; // replay overlap after a resubscribe
                }
                state.last_seq = seq;
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(data.as_bytes())
                    .unwrap_or_default();
                let responses = state.emulator.feed(&bytes);
                drop(state);
                if !responses.is_empty() {
                    let _ = self.input.send(responses);
                }
                self.notify();
            }
            zeron_proto::TerminalEvent::Exit { seq, exit_code, .. } => {
                state.last_seq = seq;
                if state.exit_code.is_none() {
                    state.exit_code = Some(exit_code);
                    state.emulator.feed(&exit_message(exit_code));
                }
                drop(state);
                self.notify();
                self.listener.on_exit(exit_code);
            }
        }
    }

    fn subscribe(self: &Arc<Self>, attempt: u32) {
        let inner = self.clone();
        zc::runtime::shared().spawn(async move {
            if attempt > 0 {
                tokio::time::sleep(backoff(attempt)).await;
            }
            if inner.closed.load(Ordering::Acquire) {
                return;
            }
            let after = lock(&inner.state).last_seq;
            let sink: Arc<dyn zc::rpc::HostWatchSink> = Arc::new(TerminalSink {
                inner: Arc::downgrade(&inner),
                attempt,
            });
            let params = serde_json::json!({ "terminalId": inner.terminal_id, "afterSeq": after });
            match inner
                .client
                .host_watch(
                    &inner.device_id,
                    zc::rpc::methods::SUBSCRIBE_TERMINAL,
                    params,
                    sink,
                )
                .await
            {
                Ok(watch) => {
                    if inner.closed.load(Ordering::Acquire) {
                        watch.cancel();
                    } else {
                        *lock(&inner.watch) = Some(watch);
                    }
                }
                Err(err) => {
                    // An unknown terminal (engine restarted, or it was closed)
                    // never comes back: show it and stop.
                    if matches!(err, zc::ClientError::HostError(ref m) if m.contains("not found")) {
                        let mut state = lock(&inner.state);
                        state.received = true;
                        if state.exit_code.is_none() {
                            state.exit_code = Some(-1);
                            state
                                .emulator
                                .feed(b"\r\n\x1b[90m[terminal is no longer running]\x1b[0m\r\n");
                        }
                        drop(state);
                        inner.notify();
                        inner.listener.on_exit(-1);
                    } else {
                        inner.subscribe(attempt + 1);
                    }
                }
            }
        });
    }
}

struct TerminalSink {
    inner: Weak<TerminalInner>,
    attempt: u32,
}

impl zc::rpc::HostWatchSink for TerminalSink {
    fn item(&self, value: serde_json::Value) {
        if let Some(inner) = self.inner.upgrade() {
            inner.apply(value);
        }
    }

    fn ended(&self) {
        let Some(inner) = self.inner.upgrade() else {
            return;
        };
        if inner.closed.load(Ordering::Acquire) || lock(&inner.state).exit_code.is_some() {
            return;
        }
        // Dropped without an exit: resume after the last seen seq.
        let received = lock(&inner.state).received;
        inner.subscribe(if received { 1 } else { self.attempt + 1 });
    }
}

/// One engine terminal as a paintable grid. See [`CoreClient::terminal_screen`].
#[derive(uniffi::Object)]
pub struct TerminalScreen {
    inner: Arc<TerminalInner>,
}

impl TerminalScreen {
    fn start(
        client: zc::Client,
        device_id: String,
        terminal_id: String,
        cols: u16,
        rows: u16,
        palette: TerminalPalette,
        listener: Arc<dyn TerminalListener>,
    ) -> Arc<Self> {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
        let inner = Arc::new(TerminalInner {
            client,
            device_id,
            terminal_id,
            state: Mutex::new(TerminalState {
                emulator: Emulator::new(cols, rows),
                palette,
                last_seq: 0,
                exit_code: None,
                received: false,
            }),
            listener,
            dirty: AtomicBool::new(false),
            closed: AtomicBool::new(false),
            watch: Mutex::new(None),
            input: tx,
            resize_gen: AtomicU64::new(0),
        });
        // The writer: one task so keystrokes land in order; whatever queued
        // while a write was in flight goes out as one call.
        let writer = Arc::downgrade(&inner);
        zc::runtime::shared().spawn(async move {
            while let Some(first) = rx.recv().await {
                let mut bytes = first;
                while bytes.len() < WRITE_CHUNK {
                    match rx.try_recv() {
                        Ok(more) => bytes.extend(more),
                        Err(_) => break,
                    }
                }
                let Some(inner) = writer.upgrade() else {
                    return;
                };
                for chunk in bytes.chunks(WRITE_CHUNK) {
                    let params = serde_json::json!({
                        "terminalId": inner.terminal_id,
                        "data": base64::engine::general_purpose::STANDARD.encode(chunk),
                    });
                    let _ = inner
                        .client
                        .host_call(&inner.device_id, zc::rpc::methods::WRITE_TERMINAL, params)
                        .await;
                }
            }
        });
        inner.subscribe(0);
        Arc::new(Self { inner })
    }

    fn send(&self, bytes: Vec<u8>) {
        if bytes.is_empty() {
            return;
        }
        let mut state = lock(&self.inner.state);
        if state.exit_code.is_some() {
            return;
        }
        // Typing while scrolled back snaps to the live bottom (xterm).
        if state.emulator.display_offset() > 0 {
            state.emulator.scroll_to_bottom();
            drop(state);
            self.inner.notify();
        } else {
            drop(state);
        }
        let _ = self.inner.input.send(bytes);
    }
}

impl Drop for TerminalScreen {
    fn drop(&mut self) {
        self.inner.closed.store(true, Ordering::Release);
        if let Some(watch) = lock(&self.inner.watch).take() {
            watch.cancel();
        }
    }
}

fn resolve(color: CellColor, palette: &TerminalPalette) -> u32 {
    match color {
        CellColor::Foreground => palette.foreground,
        CellColor::Background => palette.background,
        CellColor::Indexed(ix @ 0..=15) => palette
            .ansi
            .get(ix as usize)
            .copied()
            .unwrap_or(palette.foreground),
        CellColor::Indexed(ix @ 16..=231) => {
            const LEVELS: [u32; 6] = [0, 95, 135, 175, 215, 255];
            let n = ix as usize - 16;
            0xFF00_0000 | LEVELS[n / 36] << 16 | LEVELS[(n / 6) % 6] << 8 | LEVELS[n % 6]
        }
        CellColor::Indexed(ix) => {
            let step = (ix - 232) as u32;
            let step = if palette.light { 23 - step } else { step };
            let v = 8 + 10 * step;
            0xFF00_0000 | v << 16 | v << 8 | v
        }
        CellColor::Rgb(r, g, b) => 0xFF00_0000 | (r as u32) << 16 | (g as u32) << 8 | b as u32,
    }
}

fn dim(argb: u32, background: u32) -> u32 {
    let mix = |shift: u32| {
        let a = (argb >> shift) & 0xFF;
        let b = (background >> shift) & 0xFF;
        (a * 2 + b) / 3
    };
    0xFF00_0000 | mix(16) << 16 | mix(8) << 8 | mix(0)
}

/// One row's cells as styled runs.
fn line_runs(cells: &[CellSnapshot], palette: &TerminalPalette) -> Vec<TerminalRun> {
    let mut runs: Vec<TerminalRun> = Vec::new();
    for (col, cell) in cells.iter().enumerate() {
        if cell.wide_spacer {
            continue;
        }
        let (fg, bg) = cell.display_colors();
        let mut fg = resolve(fg, palette);
        if cell.bold
            && let CellColor::Indexed(ix @ 0..=7) = cell.fg
            && !cell.inverse
        {
            // Bold as bright, like most terminals.
            fg = resolve(CellColor::Indexed(ix + 8), palette);
        }
        let mut bg = match bg {
            CellColor::Background => 0,
            other => resolve(other, palette),
        };
        if cell.dim {
            fg = dim(fg, if bg == 0 { palette.background } else { bg });
        }
        if cell.selected {
            bg = palette.selection;
        }
        let ch = if cell.ch == '\0' || cell.hidden {
            ' '
        } else {
            cell.ch
        };
        let width = if cell.wide { 2 } else { 1 };
        if let Some(last) = runs.last_mut()
            && !cell.wide
            && last.width as usize == last.text.chars().count()
            && last.col as usize + last.width as usize == col
            && last.fg == fg
            && last.bg == bg
            && last.bold == cell.bold
            && last.italic == cell.italic
            && last.underline == cell.underline
        {
            last.text.push(ch);
            last.width += 1;
            continue;
        }
        runs.push(TerminalRun {
            col: col as u16,
            width,
            text: ch.to_string(),
            fg,
            bg,
            bold: cell.bold,
            italic: cell.italic,
            underline: cell.underline,
        });
    }
    // Trailing blanks on the default background paint nothing.
    while let Some(last) = runs.last() {
        if last.bg == 0 && !last.underline && last.text.chars().all(|c| c == ' ') {
            runs.pop();
        } else {
            break;
        }
    }
    runs
}

#[uniffi::export]
impl TerminalScreen {
    pub fn terminal_id(&self) -> String {
        self.inner.terminal_id.clone()
    }

    /// The grid to paint (clears the pending-frame flag).
    pub fn frame(&self) -> TerminalFrame {
        self.inner.dirty.store(false, Ordering::Release);
        let state = lock(&self.inner.state);
        let emulator = &state.emulator;
        let lines = emulator
            .lines()
            .iter()
            .map(|cells| TerminalLine {
                runs: line_runs(cells, &state.palette),
            })
            .collect();
        let cursor = if state.exit_code.is_some() {
            None
        } else {
            emulator.cursor()
        };
        TerminalFrame {
            cols: emulator.cols() as u16,
            rows: emulator.rows() as u16,
            lines,
            cursor_row: cursor.map_or(-1, |c| c.row as i32),
            cursor_col: cursor.map_or(-1, |c| c.col as i32),
            display_offset: emulator.display_offset() as u32,
            history: emulator.history_lines() as u32,
            title: emulator.title().map(str::to_owned),
            exit_code: state.exit_code,
            has_selection: emulator.has_selection(),
            connecting: !state.received,
        }
    }

    pub fn set_palette(&self, palette: TerminalPalette) {
        lock(&self.inner.state).palette = palette;
        self.inner.notify();
    }

    /// Typed text; `ctrl`/`alt` are the extra-keys row's sticky modifiers.
    pub fn write_text(&self, text: String, ctrl: bool, alt: bool) {
        self.send(keys::text_bytes(
            &text,
            Modifiers {
                ctrl,
                alt,
                shift: false,
            },
        ));
    }

    pub fn write_key(&self, key: TerminalKey, ctrl: bool, alt: bool, shift: bool) {
        let app_cursor = lock(&self.inner.state).emulator.app_cursor_mode();
        self.send(keys::key_bytes(
            key.into(),
            Modifiers { ctrl, alt, shift },
            app_cursor,
        ));
    }

    /// Clipboard text (bracketed when the program asked for it).
    pub fn paste(&self, text: String) {
        let bracketed = lock(&self.inner.state).emulator.bracketed_paste_mode();
        self.send(keys::paste_bytes(&text, bracketed));
    }

    /// Resize the grid now; the engine's PTY follows after a short debounce
    /// (rotation and the IME animate through many sizes).
    pub fn resize(&self, cols: u16, rows: u16) {
        {
            let mut state = lock(&self.inner.state);
            if state.emulator.cols() == cols as usize && state.emulator.rows() == rows as usize {
                return;
            }
            state.emulator.resize(cols, rows);
        }
        self.inner.notify();
        let generation = self.inner.resize_gen.fetch_add(1, Ordering::AcqRel) + 1;
        let inner = Arc::downgrade(&self.inner);
        zc::runtime::shared().spawn(async move {
            tokio::time::sleep(Duration::from_millis(120)).await;
            let Some(inner) = inner.upgrade() else {
                return;
            };
            if inner.resize_gen.load(Ordering::Acquire) != generation {
                return;
            }
            let (cols, rows) = {
                let state = lock(&inner.state);
                (state.emulator.cols(), state.emulator.rows())
            };
            let params = serde_json::json!({
                "terminalId": inner.terminal_id, "cols": cols, "rows": rows,
            });
            let _ = inner
                .client
                .host_call(&inner.device_id, zc::rpc::methods::RESIZE_TERMINAL, params)
                .await;
        });
    }

    /// Scroll the view: positive = up into history.
    pub fn scroll(&self, lines: i32) {
        lock(&self.inner.state).emulator.scroll(lines);
        self.inner.notify();
    }

    pub fn scroll_to_bottom(&self) {
        lock(&self.inner.state).emulator.scroll_to_bottom();
        self.inner.notify();
    }

    /// Start a selection at a viewport cell (`right_half`: the touch landed
    /// on the cell's right half).
    pub fn select_start(&self, row: u32, col: u32, right_half: bool, kind: TerminalSelection) {
        let mut state = lock(&self.inner.state);
        let point = state.emulator.grid_point(row as usize, col as usize);
        let ty = match kind {
            TerminalSelection::Simple => SelectionType::Simple,
            TerminalSelection::Word => SelectionType::Semantic,
            TerminalSelection::Line => SelectionType::Lines,
        };
        let side = if right_half { Side::Right } else { Side::Left };
        state.emulator.start_selection(ty, point, side);
        drop(state);
        self.inner.notify();
    }

    pub fn select_update(&self, row: u32, col: u32, right_half: bool) {
        let mut state = lock(&self.inner.state);
        let point = state.emulator.grid_point(row as usize, col as usize);
        let side = if right_half { Side::Right } else { Side::Left };
        state.emulator.update_selection(point, side);
        drop(state);
        self.inner.notify();
    }

    pub fn clear_selection(&self) {
        lock(&self.inner.state).emulator.clear_selection();
        self.inner.notify();
    }

    pub fn selection_text(&self) -> Option<String> {
        lock(&self.inner.state).emulator.selection_text()
    }

    /// Everything on screen and in the client's scrollback, as text.
    pub fn all_text(&self) -> String {
        let mut state = lock(&self.inner.state);
        let emulator = &mut state.emulator;
        let offset = emulator.display_offset();
        let history = emulator.history_lines();
        let rows = emulator.rows();
        let cols = emulator.cols();
        emulator.scroll_to_offset(history);
        emulator.start_selection(SelectionType::Lines, emulator.grid_point(0, 0), Side::Left);
        emulator.scroll_to_bottom();
        emulator.update_selection(emulator.grid_point(rows - 1, cols - 1), Side::Right);
        let text = emulator.selection_text().unwrap_or_default();
        emulator.clear_selection();
        emulator.scroll_to_offset(offset);
        text.trim_end().to_owned()
    }

    /// End the shell (`CloseTerminal`) and stop following it. (Releasing the
    /// object only detaches; the shell keeps running.)
    pub async fn kill(&self) -> CoreResult<()> {
        self.inner.closed.store(true, Ordering::Release);
        if let Some(watch) = lock(&self.inner.watch).take() {
            watch.cancel();
        }
        let inner = self.inner.clone();
        let result = on_runtime(async move {
            inner
                .client
                .host_call(
                    &inner.device_id,
                    zc::rpc::methods::CLOSE_TERMINAL,
                    serde_json::json!({ "terminalId": inner.terminal_id }),
                )
                .await
        })
        .await;
        match result {
            Ok(_) => Ok(()),
            // Already gone is closed.
            Err(CoreError::HostError { message }) if message.contains("not found") => Ok(()),
            Err(err) => Err(err),
        }
    }
}

// ── highlighting & icons ────────────────────────────────────────────────────

/// A highlighted span within one line, in UTF-16 code units.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct CodeSpan {
    pub start: u32,
    pub end: u32,
    pub color: ColorRole,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct CodeLine {
    pub spans: Vec<CodeSpan>,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct HighlightedSource {
    pub language: String,
    /// One entry per `\n`-separated line of the source.
    pub lines: Vec<CodeLine>,
}

/// Largest source the editor highlights (bigger files show plain).
const HIGHLIGHT_LIMIT: usize = 1024 * 1024;

fn utf16_offset(line: &str, byte: usize) -> u32 {
    line[..byte.min(line.len())].encode_utf16().count() as u32
}

/// Tree-sitter highlighting (the desktop editor's grammars) for a file's
/// text, language picked from `path` (and a shebang). `None` for unknown
/// languages and oversized sources.
#[uniffi::export]
pub fn highlight_source(path: String, text: String) -> Option<HighlightedSource> {
    if text.len() > HIGHLIGHT_LIMIT {
        return None;
    }
    let doc = zeron_syntax::highlight(zeron_syntax::HighlightRequest {
        source: &text,
        path: Some(&path),
        fence_tag: None,
    })
    .ok()?;
    let lines = text
        .split('\n')
        .zip(doc.lines.iter().chain(std::iter::repeat(&Vec::new())))
        .map(|(line, spans)| CodeLine {
            spans: spans
                .iter()
                .filter(|s| s.range.end <= line.len())
                .map(|s| CodeSpan {
                    start: utf16_offset(line, s.range.start),
                    end: utf16_offset(line, s.range.end),
                    color: crate::layout::markdown_syntax_color(s.kind),
                })
                .collect(),
        })
        .collect();
    Some(HighlightedSource {
        language: format!("{:?}", doc.language),
        lines,
    })
}

/// GitHub-flavoured Markdown as an HTML fragment for the file viewer's
/// preview (tables, task lists, strikethrough, footnotes). Raw HTML passes
/// through; the viewer renders it with scripts off.
#[uniffi::export]
pub fn markdown_html(text: String) -> String {
    use pulldown_cmark::{Options, Parser, html};
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_GFM;
    let mut out = String::with_capacity(text.len() * 3 / 2);
    html::push_html(&mut out, Parser::new_ext(&text, options));
    out
}

/// Bundled icon asset for a file path (`fileicon-files-rust`), resolved like
/// the desktop's file tree.
#[uniffi::export]
pub fn file_icon_name(path: String) -> String {
    crate::layout::file_icon_asset(&path)
}

/// Bundled icon asset for a folder name (`fileicon-folders-folder-src`).
#[uniffi::export]
pub fn folder_icon_name(name: String) -> String {
    crate::layout::folder_icon_asset(&name)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicI64;
    use std::time::Instant;

    use serde_json::{Value, json};
    use zeron_rpc::methods as m;
    use zeron_rpc::{RpcError, RpcReply};

    use super::*;
    use crate::client_ffi::{ClientEvent, ClientListener, CoreConfig, Credentials};

    const PHONE: &str = "phone-engine";
    const TOKEN: &str = "ipc-secret-token";

    fn b64(bytes: &[u8]) -> String {
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    /// The phone's engine as the FFI sees it: a finite terminal replay that
    /// asks for a cursor report, a silent file watch counted while it lives,
    /// and the terminal input/resize/close calls recorded.
    #[derive(Default)]
    struct Engine {
        writes: Mutex<Vec<u8>>,
        resizes: Mutex<Vec<(u64, u64)>>,
        closed: Mutex<Vec<String>>,
        watches: Arc<AtomicI64>,
    }

    struct Live(Arc<AtomicI64>);

    impl Drop for Live {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }

    #[async_trait::async_trait]
    impl zeron_rpc::RpcService for Engine {
        async fn handle(&self, method: &str, params: Value) -> Result<RpcReply, RpcError> {
            Ok(RpcReply::Value(match method {
                m::EDGE_BEARER => json!({
                    "edgeUrl": "http://127.0.0.1:9", "userId": "local", "orgId": "local",
                    "bearer": "local@local",
                }),
                m::SUBSCRIBE_TERMINAL => {
                    if params["terminalId"] != "t1" {
                        return Err(RpcError::Failed("Terminal not found".into()));
                    }
                    let after = params["afterSeq"].as_u64().unwrap_or(0);
                    let frames = [
                        json!({ "type": "data", "seq": 1, "data": b64(b"$ echo hi\r\nhi\r\n") }),
                        // DSR: the emulator must answer with the cursor position.
                        json!({ "type": "data", "seq": 2, "data": b64(b"\x1b[6n$ ") }),
                    ];
                    let items: Vec<Value> = frames
                        .into_iter()
                        .filter(|f| f["seq"].as_u64().unwrap() > after)
                        .collect();
                    let stream = futures::StreamExt::chain(
                        futures::stream::iter(items),
                        futures::stream::pending(),
                    );
                    return Ok(RpcReply::Stream(Box::pin(stream)));
                }
                m::WRITE_TERMINAL => {
                    let data = params["data"].as_str().unwrap_or_default();
                    let bytes = base64::engine::general_purpose::STANDARD
                        .decode(data)
                        .unwrap();
                    lock(&self.writes).extend(bytes);
                    json!({ "ok": true })
                }
                m::RESIZE_TERMINAL => {
                    lock(&self.resizes).push((
                        params["cols"].as_u64().unwrap(),
                        params["rows"].as_u64().unwrap(),
                    ));
                    json!({ "ok": true })
                }
                m::CLOSE_TERMINAL => {
                    lock(&self.closed).push(params["terminalId"].as_str().unwrap().into());
                    json!({ "ok": true })
                }
                m::WATCH_WORKSPACE_FILES => {
                    if params["spaceId"] != "space-1" {
                        return Err(RpcError::Failed("unknown project".into()));
                    }
                    self.watches.fetch_add(1, Ordering::SeqCst);
                    let live = Live(self.watches.clone());
                    let first = json!({ "sequence": 1, "resyncRequired": true, "changes": [] });
                    let stream = futures::StreamExt::chain(
                        futures::stream::once(async move { first }),
                        futures::stream::unfold(live, |live| async move {
                            std::future::pending::<()>().await;
                            Some((Value::Null, live))
                        }),
                    );
                    return Ok(RpcReply::Stream(Box::pin(stream)));
                }
                other => return Err(RpcError::UnknownMethod(other.into())),
            }))
        }
    }

    struct NullListener;

    impl ClientListener for NullListener {
        fn on_event(&self, _event: ClientEvent) {}
    }

    fn wait_for(what: &str, mut done: impl FnMut() -> bool) {
        let start = Instant::now();
        while !done() {
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "timed out: {what}"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn block<T: Send + 'static>(fut: impl std::future::Future<Output = T> + Send + 'static) -> T {
        zc::runtime::shared().block_on(fut)
    }

    /// A `CoreClient` sharing its device with `engine` (the Android app).
    fn phone(engine: Arc<Engine>, dir: &std::path::Path) -> Arc<CoreClient> {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        zc::runtime::shared().spawn(async move {
            let listener = tokio::net::TcpListener::from_std(listener).unwrap();
            zeron_rpc::serve_ws_listener_with_token(listener, engine, Some(TOKEN.into())).await;
        });
        CoreClient::new(
            CoreConfig {
                edge_url: "http://127.0.0.1:9".into(),
                data_dir: dir.to_string_lossy().into_owned(),
                device_id: PHONE.into(),
                device_name: "Phone".into(),
                platform: "android".into(),
                app_version: "test".into(),
            },
            Credentials::Engine {
                ipc_url: format!("ws://127.0.0.1:{port}"),
                ipc_token: Some(TOKEN.into()),
                user_id: "local".into(),
                org_id: "local".into(),
            },
            Arc::new(NullListener),
        )
        .unwrap()
    }

    #[derive(Default)]
    struct Items {
        items: Mutex<Vec<Value>>,
        ended: AtomicBool,
    }

    impl HostStreamListener for Items {
        fn on_item(&self, json: String) {
            lock(&self.items).push(serde_json::from_str(&json).unwrap());
        }

        fn on_end(&self) {
            self.ended.store(true, Ordering::SeqCst);
        }
    }

    #[derive(Default)]
    struct Frames {
        frames: AtomicU64,
        exit: Mutex<Option<i32>>,
    }

    impl TerminalListener for Frames {
        fn on_frame(&self) {
            self.frames.fetch_add(1, Ordering::SeqCst);
        }

        fn on_exit(&self, code: i32) {
            *lock(&self.exit) = Some(code);
        }
    }

    fn palette() -> TerminalPalette {
        TerminalPalette {
            foreground: 0xFFEEEEEE,
            background: 0xFF101010,
            ansi: (0..16).map(|i| 0xFF000000 | i).collect(),
            cursor: 0xFFFFFFFF,
            selection: 0x40FFFFFF,
            light: false,
        }
    }

    fn row_text(line: &TerminalLine) -> String {
        let mut text = String::new();
        for run in &line.runs {
            while text.chars().count() < run.col as usize {
                text.push(' ');
            }
            text.push_str(&run.text);
        }
        text.trim_end().to_owned()
    }

    #[test]
    fn host_watch_streams_until_cancelled_and_rejects_bad_requests() {
        let engine = Arc::new(Engine::default());
        let dir = tempfile::tempdir().unwrap();
        let core = phone(engine.clone(), dir.path());

        let items = Arc::new(Items::default());
        let c = core.clone();
        let listener: Arc<dyn HostStreamListener> = items.clone();
        let stream = block(async move {
            c.host_watch(
                PHONE.into(),
                m::WATCH_WORKSPACE_FILES.into(),
                r#"{"spaceId":"space-1"}"#.into(),
                listener,
            )
            .await
        })
        .unwrap();
        wait_for("baseline frame", || lock(&items.items).len() == 1);
        assert_eq!(lock(&items.items)[0]["resyncRequired"], true);
        assert_eq!(engine.watches.load(Ordering::SeqCst), 1);
        assert!(stream.is_active());
        drop(stream); // releasing the object cancels the host's stream
        wait_for("host stream cancelled", || {
            engine.watches.load(Ordering::SeqCst) == 0
        });
        assert!(
            !items.ended.load(Ordering::SeqCst),
            "a cancelled stream is silent"
        );

        let c = core.clone();
        let err = block(async move {
            c.host_watch(
                PHONE.into(),
                m::WATCH_WORKSPACE_FILES.into(),
                r#"{"spaceId":"nope"}"#.into(),
                Arc::new(Items::default()),
            )
            .await
        })
        .err()
        .unwrap();
        assert!(err.to_string().contains("unknown project"), "{err}");
        let c = core.clone();
        let err = block(async move {
            c.host_watch(
                PHONE.into(),
                "FutureWatch".into(),
                String::new(),
                Arc::new(Items::default()),
            )
            .await
        })
        .err()
        .unwrap();
        assert!(matches!(err, CoreError::Unsupported { .. }), "{err:?}");
        let c = core.clone();
        let err = block(async move {
            c.host_watch(
                PHONE.into(),
                "X".into(),
                "{nope".into(),
                Arc::new(Items::default()),
            )
            .await
        })
        .err()
        .unwrap();
        assert!(matches!(err, CoreError::InvalidArgument { .. }), "{err:?}");
        core.shutdown();
    }

    #[test]
    fn terminal_screen_replays_answers_types_resizes_and_closes() {
        let engine = Arc::new(Engine::default());
        let dir = tempfile::tempdir().unwrap();
        let core = phone(engine.clone(), dir.path());
        let frames = Arc::new(Frames::default());
        let screen =
            core.terminal_screen(PHONE.into(), "t1".into(), 40, 6, palette(), frames.clone());

        wait_for("replay painted", || {
            frames.frames.load(Ordering::SeqCst) > 0 && row_text(&screen.frame().lines[2]) == "$"
        });
        let frame = screen.frame();
        assert!(!frame.connecting);
        assert_eq!((frame.cols, frame.rows), (40, 6));
        assert_eq!(row_text(&frame.lines[0]), "$ echo hi");
        assert_eq!(row_text(&frame.lines[1]), "hi");
        assert_eq!((frame.cursor_row, frame.cursor_col), (2, 2));
        // The DSR query was answered before any typing.
        wait_for("cursor report", || {
            lock(&engine.writes).starts_with(b"\x1b[3;1R")
        });

        screen.write_text("ls\n".into(), false, false);
        screen.write_text("c".into(), true, false);
        screen.write_key(TerminalKey::Up, false, false, false);
        screen.paste("a\nb".into());
        wait_for("typed input", || {
            lock(&engine.writes).len() >= 6 + 3 + 1 + 3 + 3
        });
        assert_eq!(&lock(&engine.writes)[6..], b"ls\r\x03\x1b[Aa\rb".as_slice());

        // Resizes debounce to the last size.
        screen.resize(50, 10);
        screen.resize(60, 12);
        assert_eq!((screen.frame().cols, screen.frame().rows), (60, 12));
        wait_for("resize sent", || !lock(&engine.resizes).is_empty());
        std::thread::sleep(Duration::from_millis(250));
        assert_eq!(*lock(&engine.resizes), vec![(60, 12)]);

        // Selection and copy.
        screen.select_start(1, 0, false, TerminalSelection::Word);
        assert_eq!(screen.selection_text().as_deref(), Some("hi"));
        assert!(screen.frame().has_selection);
        screen.clear_selection();
        assert!(screen.all_text().starts_with("$ echo hi\nhi"));

        let s = screen.clone();
        block(async move { s.kill().await }).unwrap();
        assert_eq!(*lock(&engine.closed), vec!["t1".to_string()]);
        core.shutdown();
    }

    #[test]
    fn unknown_terminals_end_instead_of_retrying() {
        let engine = Arc::new(Engine::default());
        let dir = tempfile::tempdir().unwrap();
        let core = phone(engine, dir.path());
        let frames = Arc::new(Frames::default());
        let screen = core.terminal_screen(
            PHONE.into(),
            "gone".into(),
            40,
            4,
            palette(),
            frames.clone(),
        );
        wait_for("gone", || lock(&frames.exit).is_some());
        let frame = screen.frame();
        assert_eq!(frame.exit_code, Some(-1));
        assert_eq!(frame.cursor_row, -1);
        assert!(
            frame
                .lines
                .iter()
                .any(|l| row_text(l).contains("no longer running"))
        );
        core.shutdown();
    }

    #[test]
    fn runs_merge_styles_and_resolve_colors() {
        let mut emulator = Emulator::new(20, 2);
        emulator.feed("ab\x1b[31mcd\x1b[0m宽\x1b[38;5;196mx\x1b[1;32my".as_bytes());
        let runs = line_runs(&emulator.lines()[0], &palette());
        let summary: Vec<(u16, u16, &str)> = runs
            .iter()
            .map(|r| (r.col, r.width, r.text.as_str()))
            .collect();
        assert_eq!(
            summary,
            vec![
                (0, 2, "ab"),
                (2, 2, "cd"),
                (4, 2, "宽"),
                (6, 1, "x"),
                (7, 1, "y")
            ]
        );
        assert_eq!(runs[0].fg, 0xFFEEEEEE);
        assert_eq!(runs[1].fg, 0xFF000001);
        assert_eq!(runs[3].fg, 0xFFFF0000);
        // Bold green renders as bright green.
        assert_eq!(runs[4].fg, 0xFF00000A);
        assert!(runs.iter().all(|r| r.bg == 0));
    }

    #[test]
    fn highlighting_speaks_utf16_per_line() {
        let source = "// é\nfn main() {\n    let s = \"ü\";\n}\n".to_string();
        let doc = highlight_source("src/main.rs".into(), source).unwrap();
        assert_eq!(doc.lines.len(), 5);
        assert_eq!(doc.lines[0].spans[0].color, ColorRole::SyntaxComment);
        assert_eq!(
            (doc.lines[0].spans[0].start, doc.lines[0].spans[0].end),
            (0, 4)
        );
        assert!(
            doc.lines[1]
                .spans
                .iter()
                .any(|s| s.color == ColorRole::SyntaxKeyword && (s.start, s.end) == (0, 2))
        );
        let string = doc.lines[2]
            .spans
            .iter()
            .find(|s| s.color == ColorRole::SyntaxString)
            .unwrap();
        assert_eq!((string.start, string.end), (12, 15));
        assert!(highlight_source("notes.unknownext".into(), "plain".into()).is_none());
        assert_eq!(file_icon_name("a/lib.rs".into()), "fileicon-files-rust");
        assert_eq!(
            folder_icon_name("nothing-special".into()),
            "fileicon-folders-folder"
        );
    }
}
