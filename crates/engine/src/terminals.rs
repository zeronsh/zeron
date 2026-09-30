//! Terminals — PTY sessions owned by this device (feature-inventory §3.4; port of
//! zeron's `terminals.ts` over `portable-pty`).
//!
//! - `open` spawns the user's login shell in the chat's cwd; `subscribe` replays a
//!   bounded 1MB window (resumable via `afterSeq`) then tails live output, batched
//!   at [`TERMINAL_OUTPUT_BATCH_MS`]; data rides base64 (PTY bytes ≠ UTF-8).
//! - Live shells survive subscriber detach — a detached session is the user's
//!   running process, kept until its tab is explicitly closed or the engine exits.
//!   Only EXITED sessions expire (30min TTL on their inert replay buffers), and
//!   [`MAX_TERMINALS`] bounds leakage from renderers that lost their tab state.
//! - Ownership: M5 is single-user local — every IPC/relay caller is the device
//!   owner, so the per-user owner re-checks from zeron's Router land with real
//!   multi-account auth in M6.

use std::collections::{HashMap, VecDeque};
#[cfg(windows)]
use std::io::Read;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use portable_pty::PtySize;
#[cfg(not(windows))]
use portable_pty::{CommandBuilder, native_pty_system};
#[cfg(unix)]
pub mod handoff;
#[cfg(unix)]
pub(crate) mod pty_unix;
#[cfg(windows)]
mod windows;
use tokio::sync::{mpsc, oneshot};

use zeron_doc::TERMINAL_OUTPUT_BATCH_MS;
use zeron_proto::{TerminalEvent, TerminalSession};

use crate::{EngineError, new_id};

const MAX_TERMINALS: usize = 32;
const MAX_INPUT_BYTES: usize = 64 * 1024;
const MAX_REPLAY_BYTES: usize = 1024 * 1024;
const EXITED_TTL: Duration = Duration::from_secs(30 * 60);
const REAPER_INTERVAL: Duration = Duration::from_secs(60);

/// The pty master handle. On unix a dup we own (so a handoff can carry the fd
/// across an `execve`); on Windows the ConPTY handle portable-pty gave us.
#[cfg(unix)]
type Master = pty_unix::PtyMaster;
#[cfg(windows)]
type Master = Box<dyn portable_pty::MasterPty + Send>;
#[cfg(unix)]
type PtyInput = pty_unix::PtyWriter;
#[cfg(windows)]
type PtyInput = Box<dyn Write + Send>;

/// Control messages for a terminal's output pump (see [`pump_output`]).
#[cfg_attr(windows, allow(dead_code))]
enum PumpCmd {
    /// The reader is stopped: emit everything read so far as a `Data` event,
    /// acknowledge, then park until [`PumpCmd::Resume`] (or the sender drops).
    FlushAndPause(oneshot::Sender<()>),
    Resume,
}

/// A running pty reader thread and the handle that stops it. The thread hands
/// back the raw-output sender when stopped (so a thaw can restart it) and
/// drops it on EOF.
#[cfg(unix)]
struct ReaderHandle {
    stop: pty_unix::ReaderStop,
    thread: std::thread::JoinHandle<Option<mpsc::UnboundedSender<Vec<u8>>>>,
}

struct LiveTerminal {
    // Keep the private action script alive until the shell exits or the tab is closed.
    initial_script: Option<tempfile::TempPath>,
    #[cfg(all(test, windows))]
    process_id: u32,
    master: Option<Master>,
    writer: Option<PtyInput>,
    killer: Box<dyn portable_pty::ChildKiller + Send + Sync>,
    // Read only by the unix handoff snapshot.
    #[cfg_attr(windows, allow(dead_code))]
    cwd: String,
    #[cfg_attr(windows, allow(dead_code))]
    shell: String,
    #[cfg_attr(windows, allow(dead_code))]
    pump: mpsc::UnboundedSender<PumpCmd>,
    #[cfg(unix)]
    pid: libc::pid_t,
    #[cfg(unix)]
    reap_gate: Arc<pty_unix::ReapGate>,
    #[cfg(unix)]
    reader: Option<ReaderHandle>,
    #[cfg(windows)]
    reader_thread: Option<std::thread::JoinHandle<()>>,
    #[cfg(windows)]
    cleanup: Option<Arc<windows::Cleanup>>,
    subscribers: Vec<mpsc::UnboundedSender<TerminalEvent>>,
    replay: VecDeque<TerminalEvent>,
    replay_bytes: usize,
    seq: u64,
    last_active_at: std::time::Instant,
    exited: bool,
}

impl LiveTerminal {
    /// Stamp a seq, append to the bounded replay window, and fan out to live
    /// subscribers. On `Exit` the subscriber senders are dropped so every
    /// attached stream ends after delivering the event.
    fn emit(&mut self, event: TerminalEvent) {
        self.last_active_at = std::time::Instant::now();
        self.replay_bytes += event_bytes(&event);
        self.replay.push_back(event.clone());
        while self.replay_bytes > MAX_REPLAY_BYTES && self.replay.len() > 1 {
            if let Some(dropped) = self.replay.pop_front() {
                self.replay_bytes -= event_bytes(&dropped);
            }
        }
        self.subscribers.retain(|tx| tx.send(event.clone()).is_ok());
        if matches!(event, TerminalEvent::Exit { .. }) {
            self.initial_script.take();
            self.exited = true;
            self.subscribers.clear();
        }
    }

    fn next_seq(&mut self) -> u64 {
        self.seq += 1;
        self.seq
    }
}

/// What an event costs against [`MAX_REPLAY_BYTES`].
fn event_bytes(event: &TerminalEvent) -> usize {
    match event {
        TerminalEvent::Data { data, .. } => data.len(),
        TerminalEvent::Exit { .. } => 16,
    }
}

struct TerminalsInner {
    sessions: Mutex<HashMap<String, Arc<Mutex<LiveTerminal>>>>,
    /// Terminals adopted from a predecessor stay "unarmed" until the adoption
    /// commits: dropping them must NOT hang the shells up (or delete their
    /// action scripts), because a failed adoption boot hands the very same
    /// shells back to the predecessor. See [`Terminals::arm`].
    unarmed: AtomicBool,
    /// Set while the terminals are frozen for an engine handoff: no terminal
    /// may be opened (the exec would kill it) or closed (the successor would
    /// resurrect it).
    frozen: AtomicBool,
}

impl Drop for TerminalsInner {
    fn drop(&mut self) {
        let unarmed = *self.unarmed.get_mut();
        let sessions = self
            .sessions
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner)
            .drain()
            .map(|(_, session)| session)
            .collect::<Vec<_>>();
        for session in sessions {
            if unarmed {
                // An adoption that never committed (its boot failed and the
                // predecessor is about to take the shells back): leave them
                // running and keep their action scripts on disk.
                keep_for_predecessor(&session);
            }
            dispose(&session, !unarmed);
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[derive(Clone)]
pub struct Terminals {
    inner: Arc<TerminalsInner>,
}

impl Default for Terminals {
    fn default() -> Self {
        Self::new()
    }
}

fn clamp_size(cols: u16, rows: u16) -> PtySize {
    PtySize {
        cols: cols.clamp(2, 500),
        rows: rows.clamp(1, 300),
        pixel_width: 0,
        pixel_height: 0,
    }
}

/// The user's interactive shell: `$SHELL`, else the platform default.
fn selected_shell() -> String {
    if cfg!(windows) {
        return std::env::var("COMSPEC").unwrap_or_else(|_| "powershell.exe".into());
    }
    std::env::var("SHELL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| {
            if cfg!(target_os = "macos") {
                "/bin/zsh".into()
            } else {
                "/bin/bash".into()
            }
        })
}

impl Terminals {
    /// Requires a tokio runtime (spawns the exited-session reaper).
    pub fn new() -> Self {
        Self::from_sessions(HashMap::new())
    }

    fn from_sessions(sessions: HashMap<String, Arc<Mutex<LiveTerminal>>>) -> Self {
        let terminals = Self {
            inner: Arc::new(TerminalsInner {
                sessions: Mutex::new(sessions),
                unarmed: AtomicBool::new(false),
                frozen: AtomicBool::new(false),
            }),
        };
        tokio::spawn(reaper_task(Arc::downgrade(&terminals.inner)));
        terminals
    }

    /// Open a login shell in `cwd`. The PTY outlives every subscriber; it dies on
    /// [`Self::close`], shell exit + TTL, or engine shutdown.
    pub fn open(&self, cwd: &str, cols: u16, rows: u16) -> Result<TerminalSession, EngineError> {
        self.open_with_environment(cwd, cols, rows, &HashMap::new())
    }

    /// Open a login shell with host-resolved environment overrides.
    pub fn open_with_environment(
        &self,
        cwd: &str,
        cols: u16,
        rows: u16,
        environment: &HashMap<String, String>,
    ) -> Result<TerminalSession, EngineError> {
        self.open_with_shell_and_environment(cwd, cols, rows, None, environment)
    }

    /// Explicit shell override (tests use `/bin/sh`).
    pub fn open_with_shell(
        &self,
        cwd: &str,
        cols: u16,
        rows: u16,
        shell: Option<&str>,
    ) -> Result<TerminalSession, EngineError> {
        self.open_with_shell_and_environment(cwd, cols, rows, shell, &HashMap::new())
    }

    /// Explicit shell and environment override for focused integration tests.
    pub fn open_with_shell_and_environment(
        &self,
        cwd: &str,
        cols: u16,
        rows: u16,
        shell: Option<&str>,
        environment: &HashMap<String, String>,
    ) -> Result<TerminalSession, EngineError> {
        self.open_session(cwd, cols, rows, shell, environment, None)
    }

    /// Run an exact command in a fresh interactive login shell. On Unix, only a
    /// short bootstrap crosses the PTY's limited initial canonical line buffer.
    pub fn open_with_command(
        &self,
        cwd: &str,
        cols: u16,
        rows: u16,
        environment: &HashMap<String, String>,
        command: &str,
    ) -> Result<TerminalSession, EngineError> {
        self.open_session(cwd, cols, rows, None, environment, Some(command))
    }

    #[allow(clippy::too_many_arguments)]
    fn open_session(
        &self,
        cwd: &str,
        cols: u16,
        rows: u16,
        shell: Option<&str>,
        environment: &HashMap<String, String>,
        command: Option<&str>,
    ) -> Result<TerminalSession, EngineError> {
        if self.inner.frozen.load(Ordering::SeqCst) {
            return Err(EngineError::Other(
                "Terminals are paused for an engine update; try again in a moment".into(),
            ));
        }
        if lock(&self.inner.sessions).len() >= MAX_TERMINALS {
            return Err(EngineError::Other(format!(
                "Too many open terminals (maximum {MAX_TERMINALS})"
            )));
        }
        if !std::fs::metadata(cwd).map(|m| m.is_dir()).unwrap_or(false) {
            return Err(EngineError::Other(
                "Session working directory is unavailable".into(),
            ));
        }

        let shell = shell.map(str::to_string).unwrap_or_else(selected_shell);
        let shell_name = std::path::Path::new(&shell)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| shell.clone());

        #[cfg(windows)]
        let (initial_script, bootstrap) = (None, command.map(|command| format!("{command}\r")));
        #[cfg(not(windows))]
        let (initial_script, bootstrap) = if let Some(command) = command {
            let (suffix, source) = match shell_name.as_str() {
                "fish" => (".fish", "source \"$ZERON_ACTION_SCRIPT\"\r"),
                _ => (".sh", ". \"$ZERON_ACTION_SCRIPT\"\r"),
            };
            let mut script = tempfile::Builder::new()
                .prefix("zeron-action-")
                .suffix(suffix)
                .tempfile()?;
            script.write_all(command.as_bytes())?;
            script.flush()?;
            // Same delete-on-drop lifetime, but a `TempPath` can be kept (not
            // deleted) and re-wrapped by the next process image at a handoff.
            (Some(script.into_temp_path()), Some(source.to_string()))
        } else {
            (None, None)
        };

        #[cfg(windows)]
        let (master, mut child) =
            windows::open(&shell, cwd, clamp_size(cols, rows), environment)
                .map_err(|e| EngineError::Other(format!("could not open Windows terminal: {e}")))?;
        #[cfg(not(windows))]
        let (master, pid) = {
            let pty = native_pty_system();
            let pair = pty
                .openpty(clamp_size(cols, rows))
                .map_err(|e| EngineError::Other(format!("could not open a pty: {e}")))?;
            let mut cmd = CommandBuilder::new(&shell);
            cmd.arg("-l"); // login shell — the user's real PATH/profile
            cmd.cwd(cwd);
            cmd.env("TERM", "xterm-256color");
            cmd.env("COLORTERM", "truecolor");
            cmd.env("TERM_PROGRAM", "Zeron");
            for (name, value) in environment {
                cmd.env(name, value);
            }
            if let Some(script) = initial_script.as_ref() {
                cmd.env("ZERON_ACTION_SCRIPT", &**script);
            }
            let child = pair
                .slave
                .spawn_command(cmd)
                .map_err(|e| EngineError::Other(format!("could not spawn {shell_name}: {e}")))?;
            drop(pair.slave);
            let pid = child
                .process_id()
                .ok_or_else(|| EngineError::Other(format!("{shell_name} has no process id")))?
                as libc::pid_t;
            let raw = pair
                .master
                .as_raw_fd()
                .ok_or_else(|| EngineError::Other("pty master has no fd".into()))?;
            // From here on the pty is ours: a dup that can cross an exec.
            // Dropping portable-pty's master afterwards is safe (the dup keeps
            // the pty open) and its writer (which sends EOT on drop) is never
            // created. Dropping the std child handle neither kills nor reaps.
            let master = pty_unix::PtyMaster::from_raw_dup(raw)
                .map_err(|e| EngineError::Other(format!("pty master: {e}")))?;
            drop(pair.master);
            drop(child);
            (master, pid)
        };
        #[cfg(windows)]
        let killer = child.clone_killer();
        #[cfg(not(windows))]
        let killer: Box<dyn portable_pty::ChildKiller + Send + Sync> =
            Box::new(pty_unix::PidKiller(pid));
        #[cfg(windows)]
        let reader = master
            .try_clone_reader()
            .map_err(|e| EngineError::Other(format!("pty reader: {e}")))?;
        #[cfg(windows)]
        let writer = master
            .take_writer()
            .map_err(|e| EngineError::Other(format!("pty writer: {e}")))?;
        #[cfg(not(windows))]
        let (reader, reader_stop) = master
            .reader()
            .map_err(|e| EngineError::Other(format!("pty reader: {e}")))?;
        #[cfg(not(windows))]
        let writer = master
            .writer()
            .map_err(|e| EngineError::Other(format!("pty writer: {e}")))?;

        let id = new_id();
        let (pump_tx, pump_rx) = mpsc::unbounded_channel();
        #[cfg(unix)]
        let reap_gate = pty_unix::ReapGate::new();
        let session = Arc::new(Mutex::new(LiveTerminal {
            initial_script,
            #[cfg(all(test, windows))]
            process_id: child.process_id().expect("ConPTY child has a process id"),
            master: Some(master),
            writer: Some(writer),
            killer,
            cwd: cwd.to_string(),
            shell: shell_name.clone(),
            pump: pump_tx,
            #[cfg(unix)]
            pid,
            #[cfg(unix)]
            reap_gate: reap_gate.clone(),
            #[cfg(unix)]
            reader: None,
            #[cfg(windows)]
            reader_thread: None,
            #[cfg(windows)]
            cleanup: None,
            subscribers: Vec::new(),
            replay: VecDeque::new(),
            replay_bytes: 0,
            seq: 0,
            last_active_at: std::time::Instant::now(),
            exited: false,
        }));
        // Raw PTY bytes: blocking reader thread → batcher task (12ms windows).
        let (raw_tx, raw_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        #[cfg(windows)]
        {
            let reader_thread = std::thread::Builder::new()
                .name(format!("pty-read-{id}"))
                .spawn(move || read_pty(reader, raw_tx))
                .map_err(|e| {
                    lock(&self.inner.sessions).remove(&id);
                    dispose(&session, true);
                    EngineError::Other(format!("pty reader thread: {e}"))
                })?;
            lock(&session).reader_thread = Some(reader_thread);
        }
        #[cfg(not(windows))]
        {
            let handle = spawn_reader(&id, reader, reader_stop, raw_tx).map_err(|e| {
                lock(&self.inner.sessions).remove(&id);
                dispose(&session, true);
                EngineError::Other(format!("pty reader thread: {e}"))
            })?;
            lock(&session).reader = Some(handle);
        }
        #[cfg(not(windows))]
        let wait = tokio::task::spawn_blocking(move || pty_unix::wait_pid_gated(pid, &reap_gate));
        #[cfg(windows)]
        let wait = {
            // Shell lifetime must not consume workers needed by other Windows
            // process pipes, nor delay resuming a new terminal behind them.
            let (tx, rx) = tokio::sync::oneshot::channel();
            std::thread::Builder::new()
                .name(format!("pty-wait-{id}"))
                .spawn(move || {
                    let _ = tx.send(child.wait());
                })
                .map_err(|e| {
                    lock(&self.inner.sessions).remove(&id);
                    dispose(&session, true);
                    EngineError::Other(format!("pty wait thread: {e}"))
                })?;
            tokio::spawn(async move { rx.await.map_err(std::io::Error::other)? })
        };
        tokio::spawn(pump_output(Arc::downgrade(&session), raw_rx, wait, pump_rx));

        // Publish the terminal only once it is fully running (reader, waiter
        // and pump started), so a freeze can never snapshot a half-started
        // one. The frozen check and the insert are one step under the
        // sessions lock, as is `freeze`'s flag-and-copy: an open either lands
        // before a freeze (and is frozen with the rest) or is refused, never
        // inserted behind a snapshot that would miss it.
        {
            let mut sessions = lock(&self.inner.sessions);
            if self.inner.frozen.load(Ordering::SeqCst) {
                drop(sessions);
                // Hanging the shell up winds the rest down by itself: the
                // reader sees EOF, the waiter reaps it, and the pump ends when
                // its weak handle no longer upgrades.
                dispose(&session, true);
                return Err(EngineError::Other(
                    "Terminals are paused for an engine update; try again in a moment".into(),
                ));
            }
            sessions.insert(id.clone(), session.clone());
        }

        if let Some(bootstrap) = bootstrap
            && let Err(err) = self.write_bytes(&id, bootstrap.as_bytes())
        {
            let _ = self.close(&id);
            return Err(err);
        }

        Ok(TerminalSession {
            id,
            cwd: cwd.to_string(),
            shell: shell_name,
        })
    }

    fn session(&self, terminal_id: &str) -> Result<Arc<Mutex<LiveTerminal>>, EngineError> {
        lock(&self.inner.sessions)
            .get(terminal_id)
            .cloned()
            .ok_or_else(|| EngineError::Other("Terminal not found".into()))
    }

    /// Replay (from `after_seq`, bounded 1MB window) then live tail. The stream
    /// ends after `Exit`; detaching (dropping the stream) leaves the PTY running.
    pub fn subscribe(
        &self,
        terminal_id: &str,
        after_seq: Option<u64>,
    ) -> Result<mpsc::UnboundedReceiver<TerminalEvent>, EngineError> {
        let session = self.session(terminal_id)?;
        let mut session = lock(&session);
        session.last_active_at = std::time::Instant::now();
        let (tx, rx) = mpsc::unbounded_channel();
        let after = after_seq.unwrap_or(0);
        for event in &session.replay {
            let seq = match event {
                TerminalEvent::Data { seq, .. } | TerminalEvent::Exit { seq, .. } => *seq,
            };
            if seq > after {
                let _ = tx.send(event.clone());
            }
        }
        if !session.exited {
            session.subscribers.push(tx);
        }
        // On an exited session `tx` drops here: the stream ends after the replay.
        Ok(rx)
    }

    /// Write input bytes; `data` is base64 (matching `Data` events), with a plain
    /// UTF-8 fallback for lenient callers.
    pub fn write(&self, terminal_id: &str, data: &str) -> Result<(), EngineError> {
        let bytes = BASE64
            .decode(data)
            .unwrap_or_else(|_| data.as_bytes().to_vec());
        self.write_bytes(terminal_id, &bytes)
    }

    /// Write trusted host-side bytes without applying the RPC base64 decoder.
    pub fn write_bytes(&self, terminal_id: &str, bytes: &[u8]) -> Result<(), EngineError> {
        if bytes.len() > MAX_INPUT_BYTES {
            return Err(EngineError::Other("Terminal input is too large".into()));
        }
        let session = self.session(terminal_id)?;
        let mut session = lock(&session);
        if session.exited {
            return Err(EngineError::Other("Terminal has exited".into()));
        }
        session.last_active_at = std::time::Instant::now();
        let writer = session
            .writer
            .as_mut()
            .ok_or_else(|| EngineError::Other("Terminal has exited".into()))?;
        writer
            .write_all(&bytes)
            .and_then(|_| writer.flush())
            .map_err(|e| EngineError::Other(format!("Terminal write failed: {e}")))
    }

    pub fn resize(&self, terminal_id: &str, cols: u16, rows: u16) -> Result<(), EngineError> {
        let session = self.session(terminal_id)?;
        let mut session = lock(&session);
        session.last_active_at = std::time::Instant::now();
        if session.exited {
            return Ok(());
        }
        let Some(master) = session.master.as_ref() else {
            return Ok(());
        };
        master
            .resize(clamp_size(cols, rows))
            .map_err(|e| EngineError::Other(format!("Terminal resize failed: {e}")))
    }

    /// Kill the shell (if still running) and drop the session + replay buffer.
    pub fn close(&self, terminal_id: &str) -> Result<(), EngineError> {
        // Checked and removed under one lock (see `open_session`): a terminal
        // a freeze already copied is never closed behind its snapshot.
        let session = {
            let mut sessions = lock(&self.inner.sessions);
            if self.inner.frozen.load(Ordering::SeqCst) {
                return Err(EngineError::Other(
                    "Terminals are paused for an engine update; try again in a moment".into(),
                ));
            }
            sessions
                .remove(terminal_id)
                .ok_or_else(|| EngineError::Other("Terminal not found".into()))?
        };
        if dispose(&session, true) {
            Ok(())
        } else {
            Err(EngineError::Other(
                "Windows terminal reader did not finish cleanup".into(),
            ))
        }
    }

    /// Any live PTY (the reaper prunes exited ones) — restarts kill shells, so
    /// the auto-updater waits for none.
    pub fn any_open(&self) -> bool {
        !lock(&self.inner.sessions).is_empty()
    }

    /// Any shell still running. Unlike [`Self::any_open`] this ignores exited
    /// sessions awaiting their TTL, which a handoff carries across intact.
    pub fn any_live(&self) -> bool {
        lock(&self.inner.sessions)
            .values()
            .any(|session| !lock(session).exited)
    }

    /// Engine shutdown: kill every live shell. (Terminals adopted from a
    /// predecessor stay untouched until the adoption commits — see `arm`.)
    pub fn shutdown(&self) {
        let unarmed = self.inner.unarmed.load(Ordering::SeqCst);
        let sessions: Vec<_> = lock(&self.inner.sessions).drain().map(|(_, s)| s).collect();
        for session in sessions {
            if unarmed {
                keep_for_predecessor(&session);
            }
            dispose(&session, !unarmed);
        }
    }
}

/// An adoption that never committed lets go of a shell without touching it:
/// keep its action script, and keep this graph's (still running) waiter from
/// reaping it — a shell that exits before the predecessor takes it back must
/// stay a zombie so the predecessor reports its real exit status.
fn keep_for_predecessor(session: &Arc<Mutex<LiveTerminal>>) {
    let mut session = lock(session);
    if let Some(script) = session.initial_script.take() {
        let _ = script.keep();
    }
    #[cfg(unix)]
    session.reap_gate.hold();
}

fn dispose(session: &Arc<Mutex<LiveTerminal>>, kill: bool) -> bool {
    let mut session = lock(session);
    session.subscribers.clear();
    if kill
        && !session.exited
        && let Err(err) = session.killer.kill()
    {
        tracing::debug!(error = %err, "terminal kill failed (already exited?)");
    }
    session.initial_script.take();
    #[cfg(windows)]
    {
        let cleanup = windows::Cleanup::start(&mut session);
        drop(session);
        if !cleanup.wait(Duration::from_secs(5)) {
            tracing::warn!("Windows terminal cleanup did not acknowledge completion");
            return false;
        }
    }
    true
}

/// Start a unix pty reader thread.
#[cfg(unix)]
fn spawn_reader(
    id: &str,
    reader: pty_unix::PtyReader,
    stop: pty_unix::ReaderStop,
    tx: mpsc::UnboundedSender<Vec<u8>>,
) -> std::io::Result<ReaderHandle> {
    let thread = std::thread::Builder::new()
        .name(format!("pty-read-{id}"))
        .spawn(move || read_pty_unix(reader, tx))?;
    Ok(ReaderHandle { stop, thread })
}

/// Unix PTY reader: forwards raw chunks until EOF or a stop request. On a stop
/// the sender is handed back (a thaw restarts reading with it); on EOF it is
/// dropped, which is how the output pump learns the shell's output is over.
#[cfg(unix)]
fn read_pty_unix(
    mut reader: pty_unix::PtyReader,
    tx: mpsc::UnboundedSender<Vec<u8>>,
) -> Option<mpsc::UnboundedSender<Vec<u8>>> {
    let mut buf = [0u8; 8192];
    loop {
        match reader.read_chunk(&mut buf) {
            Ok(pty_unix::ReadOutcome::Data(n)) => {
                if tx.send(buf[..n].to_vec()).is_err() {
                    return None;
                }
            }
            Ok(pty_unix::ReadOutcome::Stopped) => return Some(tx),
            Ok(pty_unix::ReadOutcome::Eof) | Err(_) => return None,
        }
    }
}

/// Blocking PTY reader: forwards raw chunks until EOF. A closed PTY reads as an
/// error on some platforms (EIO on Linux once the shell exits) — both end the loop.
#[cfg(windows)]
fn read_pty(mut reader: Box<dyn Read + Send>, tx: mpsc::UnboundedSender<Vec<u8>>) {
    let mut buf = [0u8; 8192];
    loop {
        match reader.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if tx.send(buf[..n].to_vec()).is_err() {
                    // ConPTY must finish writing even when the output pump is
                    // gone; stopping this reader can deadlock ClosePseudoConsole.
                    #[cfg(not(windows))]
                    break;
                }
            }
        }
    }
}

/// Batches raw chunks into `Data` events every [`TERMINAL_OUTPUT_BATCH_MS`].
/// ConPTY keeps its output pipe open while the pseudoconsole handle is alive, even after
/// the child exits, so child wait and output reads must be driven concurrently. Once the
/// child is gone we close the PTY handles, drain its final buffered output, then emit `Exit`.
/// Holds only a weak session handle so a closed terminal tears this task down.
async fn pump_output(
    session: Weak<Mutex<LiveTerminal>>,
    mut raw_rx: mpsc::UnboundedReceiver<Vec<u8>>,
    mut wait: tokio::task::JoinHandle<Result<portable_pty::ExitStatus, std::io::Error>>,
    mut ctl: mpsc::UnboundedReceiver<PumpCmd>,
) {
    let batch = Duration::from_millis(TERMINAL_OUTPUT_BATCH_MS);
    let emit = |buffer: Vec<u8>| -> bool {
        let Some(session) = session.upgrade() else {
            return false;
        };
        let mut session = lock(&session);
        let seq = session.next_seq();
        session.emit(TerminalEvent::Data {
            seq,
            data: BASE64.encode(&buffer),
        });
        true
    };

    // Start a batch only when output arrives; idle terminals need no timer.
    let flush = tokio::time::sleep(batch);
    tokio::pin!(flush);
    let mut buffer = Vec::new();
    let mut raw_open = true;
    let mut exit_code = None;
    let mut ctl_open = true;

    while raw_open || exit_code.is_none() {
        tokio::select! {
            cmd = ctl.recv(), if ctl_open => match cmd {
                Some(PumpCmd::FlushAndPause(ack)) => {
                    // The reader is stopped, so everything it read is already in
                    // `raw_rx` or `buffer`: publish it (it gets a seq and lands
                    // in the replay window), then park. `wait` is not polled
                    // while parked, and its thread cannot reap a held shell.
                    while let Ok(chunk) = raw_rx.try_recv() {
                        buffer.extend_from_slice(&chunk);
                    }
                    if !buffer.is_empty() && !emit(std::mem::take(&mut buffer)) {
                        return;
                    }
                    let _ = ack.send(());
                    loop {
                        match ctl.recv().await {
                            Some(PumpCmd::Resume) | None => break,
                            Some(PumpCmd::FlushAndPause(ack)) => {
                                let _ = ack.send(());
                            }
                        }
                    }
                }
                Some(PumpCmd::Resume) => {}
                None => ctl_open = false,
            },
            chunk = raw_rx.recv(), if raw_open => match chunk {
                Some(chunk) => {
                    if buffer.is_empty() {
                        flush.as_mut().reset(tokio::time::Instant::now() + batch);
                    }
                    buffer.extend_from_slice(&chunk);
                },
                None => raw_open = false,
            },
            result = &mut wait, if exit_code.is_none() => {
                exit_code = Some(match result {
                    Ok(Ok(status)) => status.exit_code() as i32,
                    Ok(Err(err)) => {
                        tracing::debug!(error = %err, "terminal wait failed");
                        -1
                    }
                    Err(err) => {
                        tracing::debug!(error = %err, "terminal wait task failed");
                        -1
                    }
                });

                let Some(session) = session.upgrade() else {
                    return;
                };
                #[cfg(windows)]
                {
                    let cleanup = windows::Cleanup::start(&mut lock(&session));
                    if !cleanup.wait_async().await {
                        tracing::warn!("Windows terminal cleanup failed");
                    }
                }
                #[cfg(not(windows))]
                {
                    let (master, writer) = {
                        let mut session = lock(&session);
                        (session.master.take(), session.writer.take())
                    };
                    let _ = tokio::task::spawn_blocking(move || {
                        drop(writer);
                        drop(master);
                    }).await;
                }
            }
            _ = &mut flush, if !buffer.is_empty() => {
                if !emit(std::mem::take(&mut buffer)) {
                    return;
                }
            }
        }
    }

    if !buffer.is_empty() && !emit(buffer) {
        return;
    }
    if let Some(session) = session.upgrade() {
        let mut session = lock(&session);
        let seq = session.next_seq();
        session.emit(TerminalEvent::Exit {
            seq,
            exit_code: exit_code.unwrap_or(-1),
            signal: None,
        });
    }
}

/// Live shells never expire on idleness — a detached session is the user's running
/// process. Only EXITED sessions are swept after [`EXITED_TTL`]: they're inert
/// replay buffers held so a returning viewer can show the tail + exit status.
async fn reaper_task(inner: Weak<TerminalsInner>) {
    let mut tick = tokio::time::interval(REAPER_INTERVAL);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    tick.tick().await; // consume the immediate first tick
    loop {
        tick.tick().await;
        let Some(inner) = inner.upgrade() else { break };
        let mut sessions = lock(&inner.sessions);
        sessions.retain(|_, session| {
            let session = lock(session);
            !(session.exited && session.last_active_at.elapsed() > EXITED_TTL)
        });
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD as BASE64;
    use zeron_proto::TerminalEvent;

    use super::Terminals;

    const EVENT_TIMEOUT: Duration = Duration::from_secs(15);
    const PROCESS_TIMEOUT: Duration = Duration::from_secs(10);

    #[test]
    fn conpty_close_joins_natural_cleanup_without_blocking_pool_capacity() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .max_blocking_threads(1)
            .build()
            .unwrap();
        runtime.block_on(async {
            // Hold the entire blocking pool before launch. Windows shell launch,
            // exit observation, console close and reader join must still progress.
            let (release_pool, pool_gate) = std::sync::mpsc::channel();
            let (started, pool_started) = tokio::sync::oneshot::channel();
            let blocker = tokio::task::spawn_blocking(move || {
                let _ = started.send(());
                let _ = pool_gate.recv_timeout(Duration::from_secs(20));
            });
            pool_started.await.unwrap();
            let tmp = tempfile::tempdir().unwrap();
            let terminals = Terminals::new();
            let terminal = terminals
                .open_with_shell(
                    tmp.path().to_str().unwrap(),
                    80,
                    24,
                    Some(powershell().to_str().unwrap()),
                )
                .unwrap();
            let session = terminals.session(&terminal.id).unwrap();
            let (release_reader, reader_gate) = std::sync::mpsc::channel();
            let (drained, reader_drained) = tokio::sync::oneshot::channel();
            {
                let mut live = super::lock(&session);
                let reader = live.reader_thread.take().unwrap();
                // Keep reader completion pending after real ConPTY EOF. This
                // exposes the resource-transferred-but-not-finished interval.
                live.reader_thread = Some(std::thread::spawn(move || {
                    reader.join().unwrap();
                    let _ = drained.send(());
                    let _ = reader_gate.recv_timeout(Duration::from_secs(20));
                }));
            }
            wait_for_shell_prompt(&terminals, &terminal.id).await;
            write_line(&terminals, &terminal.id, "exit 0");
            tokio::time::timeout(EVENT_TIMEOUT, reader_drained)
                .await
                .unwrap()
                .unwrap();
            {
                let live = super::lock(&session);
                assert!(
                    live.master.is_none() && live.writer.is_none() && live.reader_thread.is_none()
                );
                assert!(!live.cleanup.as_ref().unwrap().wait(Duration::ZERO));
            }
            let (close_started, close_start) = tokio::sync::oneshot::channel();
            let (done, mut closed) = tokio::sync::oneshot::channel();
            let close_thread = std::thread::spawn(move || {
                let _ = close_started.send(());
                let _ = done.send(terminals.close(&terminal.id));
            });
            close_start.await.unwrap();
            let premature = tokio::time::timeout(Duration::from_millis(250), &mut closed).await;
            // Release gates before asserting, including on the regression path.
            release_reader.send(()).unwrap();
            release_pool.send(()).unwrap();
            blocker.await.unwrap();
            assert!(
                premature.is_err(),
                "close acknowledged an unfinished reader join"
            );
            tokio::time::timeout(Duration::from_secs(5), closed)
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            close_thread.join().unwrap();
        });
    }

    fn powershell() -> PathBuf {
        let path = PathBuf::from(std::env::var_os("SystemRoot").expect("SystemRoot is set"))
            .join("System32")
            .join("WindowsPowerShell")
            .join("v1.0")
            .join("powershell.exe");
        assert!(
            path.is_file(),
            "PowerShell fixture missing: {}",
            path.display()
        );
        path
    }

    fn decoded(events: &[TerminalEvent]) -> String {
        let mut output = Vec::new();
        for event in events {
            if let TerminalEvent::Data { data, .. } = event {
                output.extend(BASE64.decode(data).expect("terminal data is base64"));
            }
        }
        String::from_utf8_lossy(&output).into_owned()
    }

    async fn collect_until(
        rx: &mut tokio::sync::mpsc::UnboundedReceiver<TerminalEvent>,
        events: &mut Vec<TerminalEvent>,
        predicate: impl Fn(&[TerminalEvent]) -> bool,
    ) {
        let deadline = tokio::time::Instant::now() + EVENT_TIMEOUT;
        while !predicate(events) {
            let event = match tokio::time::timeout_at(deadline, rx.recv()).await {
                Ok(Some(event)) => event,
                Ok(None) => panic!(
                    "terminal stream closed before predicate; transcript: {:?}",
                    decoded(events)
                ),
                Err(_) => panic!(
                    "terminal event timed out; transcript: {:?}",
                    decoded(events)
                ),
            };
            events.push(event);
        }
    }

    async fn wait_for_shell_prompt(terminals: &Terminals, terminal_id: &str) {
        let mut rx = terminals
            .subscribe(terminal_id, None)
            .expect("subscribe to startup");
        let mut events = Vec::new();
        collect_until(&mut rx, &mut events, |events| {
            let output = decoded(events);
            output.contains("PS ") && output.contains("> ")
        })
        .await;
    }

    fn write_line(terminals: &Terminals, terminal_id: &str, command: &str) {
        terminals
            .write(terminal_id, &BASE64.encode(format!("{command}\r")))
            .expect("write PowerShell command");
    }

    fn event_seq(event: &TerminalEvent) -> u64 {
        match event {
            TerminalEvent::Data { seq, .. } | TerminalEvent::Exit { seq, .. } => *seq,
        }
    }

    async fn process_exists(pid: u32) -> bool {
        use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
        use windows_sys::Win32::Foundation::{ERROR_INVALID_PARAMETER, STILL_ACTIVE};
        use windows_sys::Win32::System::Threading::{
            GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        };

        // Launching another PowerShell for each poll measures shell startup,
        // not cleanup. Query the OS directly and never treat access errors as exit.
        let raw = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if raw.is_null() {
            let error = std::io::Error::last_os_error();
            assert_eq!(
                error.raw_os_error(),
                Some(ERROR_INVALID_PARAMETER as i32),
                "could not inspect process {pid}: {error}"
            );
            return false;
        }
        let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
        let mut code = 0;
        assert_ne!(
            unsafe { GetExitCodeProcess(handle.as_raw_handle(), &mut code) },
            0,
            "could not read process {pid} exit status: {}",
            std::io::Error::last_os_error()
        );
        code == STILL_ACTIVE as u32
    }

    async fn wait_for_process_exit(pid: u32) {
        let deadline = tokio::time::Instant::now() + PROCESS_TIMEOUT;
        while process_exists(pid).await {
            assert!(
                tokio::time::Instant::now() < deadline,
                "PowerShell child {pid} survived terminal cleanup"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    async fn open_pid_fixture(cwd: &Path) -> (Terminals, String, u32) {
        let terminals = Terminals::new();
        let shell = powershell();
        let session = terminals
            .open_with_shell(
                &cwd.to_string_lossy(),
                80,
                24,
                Some(&shell.to_string_lossy()),
            )
            .expect("open PowerShell in ConPTY");
        // Lifecycle tests identify the process through the launcher, not a
        // command typed into PSReadLine while PowerShell is still starting.
        let pid = super::lock(&terminals.session(&session.id).unwrap()).process_id;
        assert!(process_exists(pid).await, "PowerShell child is running");
        wait_for_shell_prompt(&terminals, &session.id).await;
        (terminals, session.id, pid)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn conpty_unicode_cwd_resize_replay_and_exit() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let cwd = tmp.path().join("conpty-cwd-雪");
        std::fs::create_dir(&cwd).expect("Unicode cwd");
        let terminals = Terminals::new();
        let shell = powershell();
        let session = terminals
            .open_with_shell(
                &cwd.to_string_lossy(),
                80,
                24,
                Some(&shell.to_string_lossy()),
            )
            .expect("open PowerShell in ConPTY");
        assert_eq!(session.cwd, cwd.to_string_lossy());
        assert_eq!(session.shell.to_ascii_lowercase(), "powershell.exe");

        let mut live = terminals.subscribe(&session.id, None).expect("subscribe");
        wait_for_shell_prompt(&terminals, &session.id).await;
        write_line(
            &terminals,
            &session.id,
            "[Console]::OutputEncoding = [Text.UTF8Encoding]::new(); [Console]::WriteLine(('UNICODE:{0}{1}' -f [char]0x96EA,[char]0x2603)); [Console]::WriteLine(('CWD:{0}' -f (Split-Path -Leaf (Get-Location))))",
        );
        let mut first_events = Vec::new();
        collect_until(&mut live, &mut first_events, |events| {
            let output = decoded(events);
            output.contains("UNICODE:雪☃") && output.contains("CWD:conpty-cwd-雪")
        })
        .await;

        terminals
            .resize(&session.id, 111, 37)
            .expect("resize ConPTY");
        write_line(
            &terminals,
            &session.id,
            "[Console]::WriteLine(('SIZE:{0}x{1}' -f [Console]::WindowWidth,[Console]::WindowHeight))",
        );
        collect_until(&mut live, &mut first_events, |events| {
            decoded(events).contains("SIZE:111x37")
        })
        .await;
        let last_seen = first_events
            .iter()
            .map(event_seq)
            .max()
            .expect("at least one terminal event");

        drop(live);
        let mut replay = terminals
            .subscribe(&session.id, None)
            .expect("full replay subscribe");
        let mut replayed = Vec::new();
        collect_until(&mut replay, &mut replayed, |events| {
            let output = decoded(events);
            output.contains("UNICODE:雪☃")
                && output.contains("CWD:conpty-cwd-雪")
                && output.contains("SIZE:111x37")
        })
        .await;
        assert_eq!(event_seq(replayed.first().expect("replayed event")), 1);
        drop(replay);

        let mut resumed = terminals
            .subscribe(&session.id, Some(last_seen))
            .expect("resume subscribe");
        write_line(
            &terminals,
            &session.id,
            "[Console]::WriteLine(('AFTER:{0}' -f (6 * 7)))",
        );
        let mut resumed_events = Vec::new();
        collect_until(&mut resumed, &mut resumed_events, |events| {
            decoded(events).contains("AFTER:42")
        })
        .await;
        assert!(
            resumed_events
                .iter()
                .all(|event| event_seq(event) > last_seen)
        );
        assert!(!decoded(&resumed_events).contains("UNICODE:雪☃"));

        write_line(&terminals, &session.id, "exit 7");
        collect_until(&mut resumed, &mut resumed_events, |events| {
            events
                .iter()
                .any(|event| matches!(event, TerminalEvent::Exit { .. }))
        })
        .await;
        assert!(matches!(
            resumed_events.last(),
            Some(TerminalEvent::Exit { exit_code: 7, .. })
        ));
        assert!(
            tokio::time::timeout(EVENT_TIMEOUT, resumed.recv())
                .await
                .expect("stream closure before deadline")
                .is_none(),
            "stream closes after Exit"
        );
        terminals
            .close(&session.id)
            .expect("remove exited terminal");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn close_and_final_owner_drop_kill_conpty_children() {
        let tmp = tempfile::tempdir().expect("tempdir");

        let (terminals, terminal_id, close_pid) = open_pid_fixture(tmp.path()).await;
        terminals.close(&terminal_id).expect("close live terminal");
        wait_for_process_exit(close_pid).await;

        let (terminals, _terminal_id, drop_pid) = open_pid_fixture(tmp.path()).await;
        drop(terminals);
        wait_for_process_exit(drop_pid).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn conpty_cleanup_kills_descendants_on_close_shutdown_and_drop() {
        for operation in ["close", "shutdown", "drop"] {
            let tmp = tempfile::tempdir().unwrap();
            let (terminals, id, root) = open_pid_fixture(tmp.path()).await;
            let path = tmp.path().join("descendants.txt");
            let script = format!(
                "$leaf = Start-Process -FilePath (Join-Path $PSHOME 'powershell.exe') -ArgumentList '-NoProfile','-NonInteractive','-Command','Start-Sleep -Seconds 20' -WindowStyle Hidden -PassThru; [IO.File]::WriteAllText('{}', ('{{0}},{{1}}' -f $PID,$leaf.Id)); Start-Sleep -Seconds 20",
                path.to_string_lossy().replace('\'', "''")
            );
            let encoded = BASE64.encode(
                script
                    .encode_utf16()
                    .flat_map(u16::to_le_bytes)
                    .collect::<Vec<_>>(),
            );
            write_line(
                &terminals,
                &id,
                &format!(
                    "Start-Process -FilePath (Join-Path $PSHOME 'powershell.exe') -ArgumentList '-NoProfile','-NonInteractive','-EncodedCommand','{encoded}' -WindowStyle Hidden | Out-Null"
                ),
            );
            let descendants = tokio::time::timeout(EVENT_TIMEOUT, async {
                loop {
                    if let Ok(text) = std::fs::read_to_string(&path) {
                        if let Ok(pids) = text
                            .split(',')
                            .map(str::parse::<u32>)
                            .collect::<Result<Vec<_>, _>>()
                        {
                            if pids.len() == 2 {
                                break pids;
                            }
                        }
                    }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            })
            .await
            .expect("terminal descendant startup");
            for &pid in &descendants {
                assert!(process_exists(pid).await);
            }
            // Keep another session alive to prove cleanup targets only this job.
            let (unrelated, other_id, other_pid) = open_pid_fixture(tmp.path()).await;
            match operation {
                "close" => terminals.close(&id).unwrap(),
                "shutdown" => terminals.shutdown(),
                _ => drop(terminals),
            }
            wait_for_process_exit(root).await;
            for pid in descendants {
                wait_for_process_exit(pid).await;
            }
            assert!(
                process_exists(other_pid).await,
                "cleanup killed an unrelated terminal"
            );
            unrelated.close(&other_id).unwrap();
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn conpty_final_output_is_drained_before_exit() {
        let tmp = tempfile::tempdir().unwrap();
        let (terminals, id, _) = open_pid_fixture(tmp.path()).await;
        let mut rx = terminals.subscribe(&id, None).unwrap();
        write_line(
            &terminals,
            &id,
            "[Console]::Write(('Z' * 131072)); [Console]::WriteLine(('FINAL' + '-TAIL')); exit 9",
        );
        let mut events = Vec::new();
        collect_until(&mut rx, &mut events, |events| {
            events
                .iter()
                .any(|e| matches!(e, TerminalEvent::Exit { .. }))
        })
        .await;
        let output = decoded(&events);
        assert!(
            output.matches('Z').count() >= 131072,
            "buffered terminal output was lost"
        );
        assert!(output.contains("FINAL-TAIL"));
        assert!(matches!(
            events.last(),
            Some(TerminalEvent::Exit { exit_code: 9, .. })
        ));
        assert!(rx.recv().await.is_none());
        terminals.close(&id).unwrap();
    }
}

#[cfg(all(test, unix))]
mod initial_command_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[tokio::test]
    async fn long_command_survives_delayed_shell_startup_and_script_is_cleaned_up() {
        let root = tempfile::tempdir().unwrap();
        let terminals = Terminals::new();
        for shell in ["/bin/sh", "/bin/bash", "/bin/zsh"] {
            if !std::path::Path::new(shell).exists() {
                continue;
            }
            // Deliberately leave the PTY in canonical mode while the bootstrap
            // is written, instead of relying on a scheduler-dependent race.
            let wrapper = root.path().join("slow-shell");
            std::fs::write(
                &wrapper,
                format!("#!/bin/sh\nsleep 0.2\nexec {shell} \"$@\"\n"),
            )
            .unwrap();
            std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o700)).unwrap();
            let output = root.path().join("result");
            let payload = "a".repeat(6000);
            let command = format!("printf '%s' '{payload}' > result");
            let session = terminals
                .open_session(
                    root.path().to_str().unwrap(),
                    80,
                    24,
                    wrapper.to_str(),
                    &HashMap::new(),
                    Some(&command),
                )
                .unwrap();
            let script = lock(&terminals.session(&session.id).unwrap())
                .initial_script
                .as_ref()
                .unwrap()
                .to_path_buf();
            assert_eq!(std::fs::read_to_string(&script).unwrap(), command);
            assert_eq!(
                std::fs::metadata(&script).unwrap().permissions().mode() & 0o077,
                0
            );
            tokio::time::timeout(Duration::from_secs(10), async {
                loop {
                    if std::fs::read(&output).ok().as_deref() == Some(payload.as_bytes()) {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            })
            .await
            .unwrap_or_else(|_| panic!("long command did not execute in {shell}"));
            terminals.close(&session.id).unwrap();
            assert!(
                !script.exists(),
                "closing the terminal must remove the script"
            );
            std::fs::remove_file(output).unwrap();
        }
        assert!(!terminals.any_open());
    }

    #[tokio::test]
    async fn initial_script_is_removed_on_shell_exit_and_spawn_failure() {
        let root = tempfile::tempdir().unwrap();
        let terminals = Terminals::new();
        let session = terminals
            .open_session(
                root.path().to_str().unwrap(),
                80,
                24,
                Some("/bin/sh"),
                &HashMap::new(),
                Some("sleep 0.1; exit 7"),
            )
            .unwrap();
        let script = lock(&terminals.session(&session.id).unwrap())
            .initial_script
            .as_ref()
            .unwrap()
            .to_path_buf();
        let mut rx = terminals.subscribe(&session.id, None).unwrap();
        tokio::time::timeout(Duration::from_secs(10), async {
            while let Some(event) = rx.recv().await {
                if matches!(event, TerminalEvent::Exit { .. }) {
                    break;
                }
            }
        })
        .await
        .unwrap();
        assert!(!script.exists());
        terminals.close(&session.id).unwrap();
        assert!(
            terminals
                .open_session(
                    root.path().to_str().unwrap(),
                    80,
                    24,
                    Some("/nonexistent/zeron-test-shell"),
                    &HashMap::new(),
                    Some("echo test"),
                )
                .is_err()
        );
        assert!(!terminals.any_open());
    }
}
