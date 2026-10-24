//! Terminals on a chat's host, as the phone shows them.
//!
//! A [`TerminalSession`] joins three things: the client's [`TerminalLink`]
//! (output stream with resume, batched numbered input), the desktop's own
//! emulator and echo predictor (`zeron-term`), and the platform's
//! [`TerminalListener`]. The platform pulls a [`TerminalFrame`] — rows of
//! styled runs with colors resolved for the appearance — and paints it on a
//! monospace grid; keys, touches and scrolling come back through here so
//! their encoding (cursor modes, Ctrl/Alt folding, bracketed paste, pointer
//! and wheel reports) is decided in one place, as on the desktop:
//!
//! - **Typing feels local.** Over the relay a keystroke's echo is a round
//!   trip away; once the host has proven it echoes, typed characters paint
//!   ahead of it with a faint underline, and the host's screen always wins
//!   (a password prompt never echoes, so it never shows a guess).
//! - **Scrollback glides** by the pixel: the frame carries the fraction of a
//!   row the view sits past its line, plus the row that reveals.
//! - **Programs get what they asked for**: wheel reports or arrow keys on a
//!   full-screen program's screen, taps as clicks when it tracks the
//!   pointer, focus changes when it wants them.
//!
//! [`TerminalLink`]: zeron_client::TerminalLink

mod color;
mod keys;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Instant;

pub use keys::{KeyModifiers, TerminalKey};
use zeron_client as zc;
use zeron_term::emulator::{CellColor, CellSnapshot, CursorShape, Emulator, SelectionType, Side};
use zeron_term::predict::Predictor;

use crate::client_ffi::{CoreClient, CoreError, CoreResult};

/// Where a terminal runs: enough to reattach after a relaunch.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct TerminalRef {
    pub id: String,
    pub device_id: String,
    pub cwd: String,
    pub shell: String,
    /// The host answers terminal queries itself.
    pub host_replies: bool,
}

impl From<zc::TerminalInfo> for TerminalRef {
    fn from(i: zc::TerminalInfo) -> Self {
        Self {
            id: i.id,
            device_id: i.device_id,
            cwd: i.cwd,
            shell: i.shell,
            host_replies: i.host_replies,
        }
    }
}

impl From<TerminalRef> for zc::TerminalInfo {
    fn from(r: TerminalRef) -> Self {
        Self {
            id: r.id,
            device_id: r.device_id,
            cwd: r.cwd,
            shell: r.shell,
            host_replies: r.host_replies,
        }
    }
}

#[derive(Debug, Clone, PartialEq, uniffi::Enum)]
pub enum TerminalPhase {
    Connecting,
    Live,
    Reconnecting {
        retry_in_ms: u64,
    },
    Exited {
        code: i32,
        signal: Option<String>,
    },
    /// The host no longer has it.
    Gone,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum TerminalCursorShape {
    Block,
    Beam,
    Underline,
    HollowBlock,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct TerminalCursor {
    pub row: u16,
    pub col: u16,
    pub shape: TerminalCursorShape,
}

/// One painted run. Colors are `0xRRGGBBAA`; 0 = the default (the
/// platform's foreground, or no background fill).
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct TerminalRun {
    pub col: u16,
    pub cells: u16,
    pub text: String,
    pub fg: u32,
    pub bg: u32,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub wide: bool,
    pub selected: bool,
    /// Typed here, painted ahead of the host's echo.
    pub predicted: bool,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct TerminalLine {
    pub runs: Vec<TerminalRun>,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct TerminalFrame {
    pub revision: u64,
    pub cols: u16,
    pub rows: u16,
    pub lines: Vec<TerminalLine>,
    /// The history row a partial scroll reveals above `lines`.
    pub above: Option<TerminalLine>,
    /// How far past its line the view sits (0..1 of a row): rows paint this
    /// much lower, so scrollback glides instead of stepping.
    pub scroll_frac: f32,
    pub cursor: Option<TerminalCursor>,
    /// The program wants a blinking cursor.
    pub cursor_blinks: bool,
    /// Lines scrolled back (0 = following the live bottom).
    pub display_offset: u32,
    pub history: u32,
    /// What the running program called itself (OSC title).
    pub title: Option<String>,
    pub alt_screen: bool,
    /// The program takes touches as pointer events.
    pub mouse: bool,
    pub phase: TerminalPhase,
    /// The terminal rides a direct link to its host (not the relay).
    pub direct: bool,
}

/// The selection's viewport bounds, for placing the edit menu.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct TerminalSelection {
    pub text: String,
    pub start_row: u16,
    pub start_col: u16,
    pub end_row: u16,
    pub end_col: u16,
}

/// Change notifications, on a client thread: hop to the main thread and
/// pull `frame()` there (bursts coalesce on the platform's run loop).
#[uniffi::export(with_foreign)]
pub trait TerminalListener: Send + Sync {
    fn changed(&self);
    /// BEL rang (a light haptic).
    fn bell(&self);
}

struct State {
    emu: Emulator,
    predictor: Predictor,
    phase: TerminalPhase,
    direct: bool,
    /// Fraction of a row the view sits past `display_offset` (0..1).
    scroll_frac: f32,
    /// Wheel rows owed to a program (reports / arrows), below one.
    wheel_remainder: f32,
    /// Bumped per expiry check scheduled, so only the latest one runs.
    expiry: u64,
}

impl State {
    /// Predictions only where they're safe: the primary screen, at the live
    /// bottom, with a visible cursor (full-screen programs draw what they
    /// like).
    fn predicting(&self) -> bool {
        !self.emu.alt_screen() && self.emu.display_offset() == 0 && self.emu.cursor().is_some()
    }
}

#[derive(uniffi::Object)]
pub struct TerminalSession {
    info: TerminalRef,
    state: Arc<Mutex<State>>,
    link: Mutex<Option<zc::TerminalLink>>,
    listener: Arc<dyn TerminalListener>,
    revision: Arc<AtomicU64>,
    dark: AtomicBool,
}

struct Sink {
    state: Weak<Mutex<State>>,
    listener: Arc<dyn TerminalListener>,
    revision: Arc<AtomicU64>,
    /// Set for a host that leaves terminal queries to its viewers: this
    /// emulator's answers go back to the shell.
    replies: Mutex<Option<zc::TerminalReplies>>,
}

fn lock(state: &Mutex<State>) -> std::sync::MutexGuard<'_, State> {
    state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl zc::TerminalSink for Sink {
    fn update(&self, update: zc::TerminalUpdate) {
        let Some(state) = self.state.upgrade() else {
            return;
        };
        let mut bell = false;
        let mut sync_deadline = None;
        {
            let mut s = lock(&state);
            match update {
                zc::TerminalUpdate::Connected => s.phase = TerminalPhase::Live,
                zc::TerminalUpdate::Path(path) => s.direct = path == zc::TerminalPath::Direct,
                zc::TerminalUpdate::Output(bytes) => {
                    let answers = s.emu.feed(&bytes);
                    let State { emu, predictor, .. } = &mut *s;
                    predictor.output(emu, Instant::now());
                    bell = s.emu.take_bell();
                    sync_deadline = s.emu.sync_deadline();
                    s.phase = TerminalPhase::Live;
                    // A host that answers queries itself already did (and
                    // ours would arrive second, late, and read as input).
                    if !answers.is_empty()
                        && let Some(replies) = self
                            .replies
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .as_ref()
                    {
                        replies.write(answers);
                    }
                }
                zc::TerminalUpdate::Exited { code, signal } => {
                    s.predictor.reset();
                    s.phase = TerminalPhase::Exited { code, signal };
                }
                zc::TerminalUpdate::Reconnecting { retry_in_ms } => {
                    s.phase = TerminalPhase::Reconnecting { retry_in_ms }
                }
                zc::TerminalUpdate::Gone => s.phase = TerminalPhase::Gone,
            }
        }
        if bell {
            self.listener.bell();
        }
        // A synchronized update the program never ends shows at its
        // deadline (a crashed TUI must not freeze the screen).
        if let Some(deadline) = sync_deadline {
            let (state, listener, revision) = (
                self.state.clone(),
                self.listener.clone(),
                self.revision.clone(),
            );
            zc::runtime::shared().spawn(async move {
                tokio::time::sleep_until(deadline.into()).await;
                let Some(state) = state.upgrade() else { return };
                let mut s = lock(&state);
                if s.emu.sync_deadline().is_some_and(|d| d <= Instant::now()) {
                    let _ = s.emu.finish_sync();
                    drop(s);
                    revision.fetch_add(1, Ordering::AcqRel);
                    listener.changed();
                }
            });
        }
        self.revision.fetch_add(1, Ordering::AcqRel);
        self.listener.changed();
    }
}

fn resolve_fg(cell: &CellSnapshot, fg: CellColor, bg: CellColor, dark: bool) -> u32 {
    let alpha = if cell.dim { 0xa0 } else { 0xff };
    let back = color::rgb(bg, dark).unwrap_or(if dark {
        color::BG_DARK
    } else {
        color::BG_LIGHT
    });
    match color::rgb(fg, dark) {
        // Defaults are the platform's: only dim needs saying.
        None if !cell.dim && fg == CellColor::Foreground => 0,
        None => {
            let base = match fg {
                CellColor::Background => back,
                _ if dark => color::FG_DARK,
                _ => color::FG_LIGHT,
            };
            color::rgba(base, alpha)
        }
        Some(rgb) => color::rgba(
            color::ensure_contrast(rgb, back, color::MIN_CONTRAST),
            alpha,
        ),
    }
}

/// One viewport row as runs: consecutive cells sharing a style merge; a
/// wide char is its own run (centered across its two cells).
fn runs(cells: &[CellSnapshot], dark: bool) -> TerminalLine {
    let mut out: Vec<TerminalRun> = Vec::new();
    for (col, cell) in cells.iter().enumerate() {
        if cell.wide_spacer {
            continue;
        }
        let (fg, bg) = cell.display_colors();
        let run = TerminalRun {
            col: col as u16,
            cells: if cell.wide { 2 } else { 1 },
            text: if cell.ch == '\0' {
                " ".into()
            } else {
                cell.ch.to_string()
            },
            fg: resolve_fg(cell, fg, bg, dark),
            bg: color::rgb(bg, dark).map_or(0, |rgb| color::rgba(rgb, 0xff)),
            bold: cell.bold,
            italic: cell.italic,
            underline: cell.underline,
            wide: cell.wide,
            selected: cell.selected,
            predicted: cell.predicted,
        };
        if let Some(last) = out.last_mut()
            && !run.wide
            && !last.wide
            && last.col + last.cells == run.col
            && (
                last.fg,
                last.bg,
                last.bold,
                last.italic,
                last.underline,
                last.selected,
                last.predicted,
            ) == (
                run.fg,
                run.bg,
                run.bold,
                run.italic,
                run.underline,
                run.selected,
                run.predicted,
            )
        {
            last.text.push_str(&run.text);
            last.cells += 1;
            continue;
        }
        out.push(run);
    }
    // Trailing blanks with nothing behind them paint nothing.
    while out
        .last()
        .is_some_and(|r| r.bg == 0 && !r.selected && !r.underline && r.text.trim().is_empty())
    {
        out.pop();
    }
    TerminalLine { runs: out }
}

impl TerminalSession {
    fn start(
        client: &zc::Client,
        info: zc::TerminalInfo,
        cols: u16,
        rows: u16,
        listener: Arc<dyn TerminalListener>,
    ) -> Arc<Self> {
        let state = Arc::new(Mutex::new(State {
            emu: Emulator::new(cols, rows),
            predictor: Predictor::default(),
            phase: TerminalPhase::Connecting,
            direct: false,
            scroll_frac: 0.0,
            wheel_remainder: 0.0,
            expiry: 0,
        }));
        let revision = Arc::new(AtomicU64::new(1));
        let sink = Arc::new(Sink {
            state: Arc::downgrade(&state),
            listener: listener.clone(),
            revision: revision.clone(),
            replies: Mutex::new(None),
        });
        let host_replies = info.host_replies;
        let link = client.attach_terminal(info.clone(), 0, sink.clone());
        if !host_replies {
            *sink
                .replies
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(link.replies());
        }
        Arc::new(Self {
            info: info.into(),
            state,
            link: Mutex::new(Some(link)),
            listener,
            revision,
            dark: AtomicBool::new(true),
        })
    }

    fn write(&self, bytes: Vec<u8>) {
        if let Some(link) = self
            .link
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
        {
            link.write(bytes);
        }
    }

    fn changed(&self) {
        self.revision.fetch_add(1, Ordering::AcqRel);
        self.listener.changed();
    }

    /// Keys go to the live bottom, like every terminal.
    fn follow(s: &mut State) -> bool {
        let moved = s.emu.display_offset() > 0 || s.scroll_frac > 0.0;
        if moved {
            s.emu.scroll_to_bottom();
            s.scroll_frac = 0.0;
        }
        moved
    }

    /// Withdraw predictions the host leaves unconfirmed past their deadline
    /// (a silent host gets no stale guesses on screen).
    fn schedule_expiry(&self, s: &mut State) {
        let Some(deadline) = s.predictor.next_deadline() else {
            return;
        };
        s.expiry += 1;
        let ticket = s.expiry;
        let (state, listener, revision) = (
            Arc::downgrade(&self.state),
            self.listener.clone(),
            self.revision.clone(),
        );
        zc::runtime::shared().spawn(async move {
            tokio::time::sleep_until(deadline.into()).await;
            let Some(state) = state.upgrade() else { return };
            let mut s = lock(&state);
            if s.expiry == ticket && s.predictor.expire(Instant::now()) {
                drop(s);
                revision.fetch_add(1, Ordering::AcqRel);
                listener.changed();
            }
        });
    }

    fn send(&self, bytes: Vec<u8>, s: &mut State) {
        s.predictor.sent(Instant::now());
        self.write(bytes);
    }
}

#[uniffi::export]
impl TerminalSession {
    pub fn info(&self) -> TerminalRef {
        self.info.clone()
    }

    pub fn revision(&self) -> u64 {
        self.revision.load(Ordering::Acquire)
    }

    /// Colors resolve for this appearance from the next frame.
    pub fn set_dark(&self, dark: bool) {
        if self.dark.swap(dark, Ordering::AcqRel) != dark {
            self.changed();
        }
    }

    pub fn frame(&self) -> TerminalFrame {
        let dark = self.dark.load(Ordering::Acquire);
        let s = lock(&self.state);
        let mut lines = s.emu.lines();
        let mut cursor = s.emu.cursor();
        if s.emu.display_offset() == 0 {
            let (predicted, predicted_cursor) = s.predictor.overlay();
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
        let scroll_frac = if s.emu.alt_screen() || s.emu.display_offset() >= s.emu.history_lines() {
            0.0
        } else {
            s.scroll_frac.clamp(0.0, 1.0)
        };
        let (shape, blinks) = s.emu.cursor_style();
        TerminalFrame {
            revision: self.revision.load(Ordering::Acquire),
            cols: s.emu.cols() as u16,
            rows: s.emu.rows() as u16,
            lines: lines.iter().map(|cells| runs(cells, dark)).collect(),
            above: (scroll_frac > 0.0)
                .then(|| s.emu.line_above())
                .flatten()
                .map(|cells| runs(&cells, dark)),
            scroll_frac,
            cursor: cursor.map(|c| TerminalCursor {
                row: c.row as u16,
                col: c.col as u16,
                shape: match shape {
                    CursorShape::Block => TerminalCursorShape::Block,
                    CursorShape::Beam => TerminalCursorShape::Beam,
                    CursorShape::Underline => TerminalCursorShape::Underline,
                    CursorShape::HollowBlock => TerminalCursorShape::HollowBlock,
                },
            }),
            cursor_blinks: blinks,
            display_offset: s.emu.display_offset() as u32,
            history: s.emu.history_lines() as u32,
            title: s.emu.title().map(str::to_owned),
            alt_screen: s.emu.alt_screen(),
            mouse: s.emu.mouse_tracking().is_some(),
            phase: s.phase.clone(),
            direct: s.direct,
        }
    }

    /// What the running program calls itself (cheap: no frame).
    pub fn title(&self) -> Option<String> {
        lock(&self.state).emu.title().map(str::to_owned)
    }

    /// Riding a direct link to the host right now.
    pub fn is_direct(&self) -> bool {
        lock(&self.state).direct
    }

    pub fn phase(&self) -> TerminalPhase {
        lock(&self.state).phase.clone()
    }

    /// Typed text (the software keyboard, a hardware key's character).
    pub fn type_text(&self, text: String, modifiers: KeyModifiers) {
        let mut s = lock(&self.state);
        Self::follow(&mut s);
        let plain = !modifiers.ctrl && !modifiers.alt && !text.contains(['\n', '\r']);
        if plain && s.predicting() {
            let cursor = s.emu.live_cursor();
            let cols = s.emu.cols();
            s.predictor.typed(&text, cursor, cols, Instant::now());
        } else {
            s.predictor.interrupt();
        }
        self.send(keys::text_bytes(&text, modifiers), &mut s);
        self.schedule_expiry(&mut s);
        drop(s);
        self.changed();
    }

    pub fn press(&self, key: TerminalKey, modifiers: KeyModifiers) {
        let mut s = lock(&self.state);
        Self::follow(&mut s);
        if key == TerminalKey::Backspace && modifiers == KeyModifiers::default() {
            s.predictor.backspace();
        } else {
            // Enter, arrows, control keys: the line may change under us.
            s.predictor.interrupt();
        }
        let bytes = keys::key_bytes(key, modifiers, s.emu.app_cursor_mode());
        self.send(bytes, &mut s);
        drop(s);
        self.changed();
    }

    pub fn paste(&self, text: String) {
        let mut s = lock(&self.state);
        Self::follow(&mut s);
        s.predictor.interrupt();
        let bytes = keys::paste_bytes(&text, s.emu.bracketed_paste_mode());
        self.send(bytes, &mut s);
        drop(s);
        self.changed();
    }

    /// The view's grid changed size: the emulator reflows now, the host's
    /// PTY follows (its next output repaints at the new size).
    pub fn resize(&self, cols: u16, rows: u16) {
        {
            let mut s = lock(&self.state);
            if s.emu.cols() == cols as usize && s.emu.rows() == rows as usize {
                return;
            }
            s.emu.resize(cols, rows);
            // The shell redraws for the new width; no guess still holds.
            s.predictor.reset();
        }
        if let Some(link) = self
            .link
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
        {
            link.resize(cols, rows);
        }
        self.changed();
    }

    /// Put the view `back` rows above the live bottom (fractional: the
    /// scrollback glides by the pixel). Clamped to the history.
    pub fn scroll_to_position(&self, back: f32) {
        let mut s = lock(&self.state);
        if s.emu.alt_screen() {
            return;
        }
        let target = back.clamp(0.0, s.emu.history_lines() as f32);
        let whole = target.floor();
        let frac = target - whole;
        if s.emu.display_offset() == whole as usize && (s.scroll_frac - frac).abs() < 0.001 {
            return;
        }
        s.emu.scroll_to_offset(whole as usize);
        s.scroll_frac = frac;
        drop(s);
        self.changed();
    }

    /// Wheel movement in rows (positive = up) over cell `row`/`col`: pointer
    /// reports to a program tracking the pointer, arrow keys to a full-screen
    /// program that isn't (less, man), else nothing — the scrollback is the
    /// view's own (`scroll_to_position`). Returns whether a program took it.
    pub fn wheel(&self, rows: f32, row: u16, col: u16) -> bool {
        let mut s = lock(&self.state);
        let reporting = s.emu.mouse_tracking().is_some();
        let arrows = s.emu.alt_screen() && s.emu.alternate_scroll();
        if !(reporting || arrows) {
            return false;
        }
        s.wheel_remainder += rows;
        let step = s.wheel_remainder.trunc() as i32;
        s.wheel_remainder -= step as f32;
        if step == 0 {
            return true;
        }
        let bytes = if reporting {
            let button = if step > 0 {
                keys::MOUSE_WHEEL_UP
            } else {
                keys::MOUSE_WHEEL_DOWN
            };
            let sgr = s.emu.sgr_mouse();
            (0..step.unsigned_abs().min(20))
                .filter_map(|_| keys::mouse_report(button, col as usize, row as usize, false, sgr))
                .flatten()
                .collect()
        } else {
            keys::wheel_arrows(step.clamp(-20, 20), s.emu.app_cursor_mode())
        };
        self.write(bytes);
        true
    }

    /// A tap at a cell: a click for a program tracking the pointer (htop,
    /// vim with mouse on). Returns whether it took it.
    pub fn click(&self, row: u16, col: u16) -> bool {
        let s = lock(&self.state);
        if s.emu.mouse_tracking().is_none() {
            return false;
        }
        let sgr = s.emu.sgr_mouse();
        let mut bytes =
            keys::mouse_report(keys::MOUSE_LEFT, col as usize, row as usize, false, sgr)
                .unwrap_or_default();
        bytes.extend(
            keys::mouse_report(keys::MOUSE_LEFT, col as usize, row as usize, true, sgr)
                .unwrap_or_default(),
        );
        drop(s);
        self.write(bytes);
        true
    }

    /// The keyboard came up or went away: programs that asked are told.
    pub fn set_focused(&self, focused: bool) {
        let s = lock(&self.state);
        if s.emu.focus_reporting() {
            drop(s);
            self.write(keys::focus_report(focused).to_vec());
        }
    }

    /// Start a selection at a cell (`word`: a long press takes the word).
    pub fn select(&self, row: u16, col: u16, word: bool) {
        let mut s = lock(&self.state);
        let point = s.emu.grid_point(row as usize, col as usize);
        let ty = if word {
            SelectionType::Semantic
        } else {
            SelectionType::Simple
        };
        s.emu.start_selection(ty, point, Side::Left);
        drop(s);
        self.changed();
    }

    pub fn extend_selection(&self, row: u16, col: u16) {
        let mut s = lock(&self.state);
        let point = s.emu.grid_point(row as usize, col as usize);
        s.emu.update_selection(point, Side::Right);
        drop(s);
        self.changed();
    }

    pub fn clear_selection(&self) {
        lock(&self.state).emu.clear_selection();
        self.changed();
    }

    pub fn selection(&self) -> Option<TerminalSelection> {
        let s = lock(&self.state);
        let text = s.emu.selection_text()?;
        let ((sr, sc), (er, ec)) = s.emu.selection_bounds()?;
        Some(TerminalSelection {
            text,
            start_row: sr as u16,
            start_col: sc as u16,
            end_row: er as u16,
            end_col: ec as u16,
        })
    }

    /// Plain text on screen (`all`: the whole scrollback).
    pub fn text(&self, all: bool) -> String {
        lock(&self.state).emu.text(all)
    }

    /// End the shell on the host.
    pub fn close(&self) {
        if let Some(link) = self
            .link
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
        {
            link.close();
        }
    }

    /// Stop following (the shell keeps running on the host).
    pub fn detach(&self) {
        self.link
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
    }
}

#[uniffi::export]
impl CoreClient {
    /// Open a shell for `chat_id` on its host, sized `cols`×`rows`.
    pub async fn open_terminal(
        &self,
        chat_id: String,
        cols: u16,
        rows: u16,
        listener: Arc<dyn TerminalListener>,
    ) -> CoreResult<Arc<TerminalSession>> {
        let client = self.client.clone();
        let opener = client.clone();
        let info =
            zc::runtime::run(async move { opener.open_terminal(&chat_id, cols, rows).await })
                .await
                .map_err(CoreError::from)?;
        Ok(TerminalSession::start(&client, info, cols, rows, listener))
    }

    /// Follow a terminal opened earlier (its output replays from the host's
    /// window; a terminal the host no longer has reports `Gone`).
    pub fn attach_terminal(
        &self,
        terminal: TerminalRef,
        cols: u16,
        rows: u16,
        listener: Arc<dyn TerminalListener>,
    ) -> Arc<TerminalSession> {
        TerminalSession::start(&self.client, terminal.into(), cols, rows, listener)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_become_styled_runs() {
        let mut e = Emulator::new(40, 3);
        e.feed(b"\x1b[31mred\x1b[0m plain \x1b[1;44mbold\x1b[0m   ");
        let line = runs(&e.lines()[0], true);
        let texts: Vec<&str> = line.runs.iter().map(|r| r.text.as_str()).collect();
        assert_eq!(texts, ["red", " plain ", "bold"], "trailing blanks dropped");
        assert_eq!(line.runs[0].fg, 0xf87171ff);
        assert_eq!(line.runs[1].fg, 0, "default foreground is the platform's");
        assert!(line.runs[2].bold && line.runs[2].bg == 0x60a5faff && line.runs[2].col == 10);
    }

    #[test]
    fn wide_chars_and_predictions_split_runs() {
        let mut e = Emulator::new(10, 2);
        e.feed("a宽b".as_bytes());
        let mut cells = e.lines()[0].clone();
        cells[3].predicted = true;
        let line = runs(&cells, true);
        assert_eq!(line.runs[1].text, "宽");
        assert!(line.runs[1].wide && line.runs[1].cells == 2);
        assert!(line.runs[2].predicted && line.runs[2].col == 3);
    }
}
