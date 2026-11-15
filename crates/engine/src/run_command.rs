//! Shell commands for agents, on this device or another of the person's
//! (Zeron MCP's `run_command` & co., forwarded by `targetDeviceId`): what an
//! agent's own shell tool does on its machine, on any of them.
//!
//! - [`run`] (`RunCommand`): one command run to its end. Its output and exit
//!   code come back, and where it ended up (`cwd`, from a POSIX shell), so an
//!   agent's `cd` carries over to its next command.
//! - [`start`] (`RunCommand {background}`): one left running (a dev server,
//!   a watcher, a long build), read with [`read`] (`ReadCommandOutput`),
//!   stopped with [`stop`] (`StopCommand`), listed with [`list`]
//!   (`ListCommands`).
//!
//! No PTY and no session: the user's shell runs the command as a login shell
//! (`-l -c`, so it has the PATH their terminal has; PowerShell or cmd on
//! Windows), with empty stdin so a prompt for input ends instead of hanging.
//! What it costs is bounded:
//!
//! - **Memory**: a run keeps at most [`HEAD`] + [`TAIL`] bytes of output (its
//!   start and end); a background command its newest [`RING`] bytes. What's
//!   left out is counted, not kept. At most [`MAX_ENDED`] ended background
//!   commands are remembered, none for longer than [`KEEP_ENDED`].
//! - **Time**: a run has a timeout (at most [`MAX_TIMEOUT`]). Each command
//!   gets its own process group (Unix), which is killed at the timeout, when
//!   the call is abandoned, when a background command is stopped, when the
//!   chat that started it is archived or deleted ([`follow_chats`]), and when
//!   the engine shuts down ([`shutdown`]). Nothing it started lives on.
//! - **Concurrency**: at most [`MAX_RUNNING`] runs and [`MAX_BACKGROUND`]
//!   background commands at once per device; past that, refused, not queued.
//!
//! Running commands count as work ([`running_count`]): a Cloud machine stays
//! awake for them, and an update waits for them.

use std::collections::{HashMap, VecDeque};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::sync::Semaphore;

pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);
pub const MAX_TIMEOUT: Duration = Duration::from_secs(600);
/// Output a run keeps from its start, and from its end.
const HEAD: usize = 16 * 1024;
const TAIL: usize = 48 * 1024;
/// Output a background command keeps (its newest), and the most one read
/// returns.
const RING: usize = 128 * 1024;
const MAX_READ: usize = 48 * 1024;
/// How long output is still read once a run's shell has exited.
const DRAIN: Duration = Duration::from_millis(250);
pub const MAX_RUNNING: usize = 16;
pub const MAX_BACKGROUND: usize = 32;
const MAX_ENDED: usize = 32;
const KEEP_ENDED: Duration = Duration::from_secs(60 * 60);

static RUNNING: Semaphore = Semaphore::const_new(MAX_RUNNING);
static BACKGROUND: LazyLock<Mutex<HashMap<String, Arc<Background>>>> = LazyLock::new(Default::default);

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Commands running now, run or background: work in progress.
pub fn running_count() -> usize {
    let background = lock(&BACKGROUND).values().filter(|c| c.ended().is_none()).count();
    MAX_RUNNING - RUNNING.available_permits() + background
}

/// How one run ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunOutcome {
    /// `None` when it was killed (timed out, or by a signal).
    pub exit_code: Option<i32>,
    /// Stdout and stderr together, in the order they arrived. When it was
    /// too long, its start and end with a line saying how much was left out.
    pub output: String,
    /// Bytes of output left out of the middle.
    #[serde(skip_serializing_if = "is_zero")]
    pub omitted_bytes: u64,
    pub timed_out: bool,
    pub duration_ms: u64,
    /// The folder it ended in (a POSIX shell's `pwd`), when it finished.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

/// Run `command` in `cwd` with the user's shell, for at most `timeout`.
pub async fn run(cwd: &str, command: &str, timeout: Duration) -> Result<RunOutcome, String> {
    run_with_shell(&crate::terminals::selected_shell(), cwd, command, timeout).await
}

pub(crate) async fn run_with_shell(
    shell: &str,
    cwd: &str,
    command: &str,
    timeout: Duration,
) -> Result<RunOutcome, String> {
    if command.trim().is_empty() {
        return Err("The command is empty".into());
    }
    let _slot = RUNNING.try_acquire().map_err(|_| {
        format!("{MAX_RUNNING} commands are already running on this device: wait for one to finish")
    })?;
    let timeout = timeout.min(MAX_TIMEOUT);
    let started = Instant::now();
    // Where it ends up, written by the shell as it exits.
    let ended_in = posix(shell).then(|| tempfile::NamedTempFile::new().ok()).flatten();
    let script = match &ended_in {
        // On the way out, however it ends (an `exit` in the command too).
        Some(file) => format!(
            "trap '__zeron_status=$?; pwd -P >| {} 2>/dev/null; exit $__zeron_status' EXIT\n{command}\n",
            quote(&file.path().to_string_lossy()).replace('\'', r"'\''")
        ),
        None => command.to_owned(),
    };
    let (mut child, group) = spawn(shell, cwd, &script)?;
    let (sender, mut chunks) = tokio::sync::mpsc::channel::<Vec<u8>>(16);
    let readers = pumps(&mut child, sender);

    let mut output = Capped::default();
    let deadline = tokio::time::Instant::now() + timeout;
    let mut status = None;
    let mut timed_out = false;
    // Until the shell exits (or time is up), taking output as it comes.
    while status.is_none() {
        tokio::select! {
            exited = child.wait() => status = Some(exited.map_err(|e| e.to_string())?),
            Some(chunk) = chunks.recv() => output.push(&chunk),
            () = tokio::time::sleep_until(deadline) => {
                timed_out = true;
                group.kill();
                let _ = child.kill().await;
                status = child.wait().await.ok();
                break;
            }
        }
    }
    // What's still in the pipes, briefly: a process it left in the
    // background may keep them open for good.
    let drain = tokio::time::Instant::now() + DRAIN;
    loop {
        tokio::select! {
            chunk = chunks.recv() => match chunk {
                Some(chunk) => output.push(&chunk),
                None => break,
            },
            () = tokio::time::sleep_until(drain) => break,
        }
    }
    for reader in readers {
        reader.abort();
    }
    group.finished();

    let exit_code = if timed_out { None } else { status.and_then(|s| s.code()) };
    let cwd = ended_in
        .filter(|_| !timed_out)
        .and_then(|file| std::fs::read_to_string(file.path()).ok())
        .map(|pwd| pwd.trim_end_matches(['\n', '\r']).to_owned())
        .filter(|pwd| !pwd.is_empty());
    let (output, omitted_bytes) = output.finish();
    Ok(RunOutcome {
        exit_code,
        output,
        omitted_bytes,
        timed_out,
        duration_ms: started.elapsed().as_millis() as u64,
        cwd,
    })
}

/// A background command, as [`start`], [`read`], [`stop`] and [`list`] say it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandStatus {
    pub id: String,
    pub command: String,
    pub cwd: String,
    /// `running`, `exited` (on its own) or `stopped` (stopped, or its chat
    /// went away, or the engine shut down).
    pub status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
    /// [`read`]: output since the last read.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    /// [`read`]: bytes of that output no longer kept.
    #[serde(skip_serializing_if = "is_zero")]
    pub omitted_bytes: u64,
}

struct Background {
    id: String,
    command: String,
    cwd: String,
    /// The chat that started it: it stops when that chat is archived or
    /// deleted ([`follow_chats`]).
    chat: Option<String>,
    started: Instant,
    group: Group,
    output: Mutex<Ring>,
    end: Mutex<Option<End>>,
    /// Asked to stop: it ends as `stopped`, whatever its code.
    stopping: AtomicBool,
}

#[derive(Clone, Copy)]
struct End {
    at: Instant,
    code: Option<i32>,
    stopped: bool,
}

impl Background {
    fn ended(&self) -> Option<End> {
        *lock(&self.end)
    }

    fn status(&self, read: bool) -> CommandStatus {
        let end = self.ended();
        let (output, omitted_bytes) = if read {
            let (output, omitted) = lock(&self.output).read();
            (Some(output), omitted)
        } else {
            (None, 0)
        };
        CommandStatus {
            id: self.id.clone(),
            command: self.command.clone(),
            cwd: self.cwd.clone(),
            status: match end {
                None => "running",
                Some(end) if end.stopped => "stopped",
                Some(_) => "exited",
            },
            exit_code: end.and_then(|e| e.code),
            duration_ms: end.map_or(self.started.elapsed(), |e| e.at - self.started).as_millis() as u64,
            output,
            omitted_bytes,
        }
    }

    fn stop(&self) {
        self.stopping.store(true, Ordering::Release);
        self.group.kill();
    }
}

/// Start `command` in `cwd` and leave it running, for `chat`.
pub fn start(cwd: &str, command: &str, chat: Option<&str>) -> Result<CommandStatus, String> {
    start_with_shell(&crate::terminals::selected_shell(), cwd, command, chat)
}

pub(crate) fn start_with_shell(
    shell: &str,
    cwd: &str,
    command: &str,
    chat: Option<&str>,
) -> Result<CommandStatus, String> {
    let mut commands = lock(&BACKGROUND);
    forget_ended(&mut commands);
    if commands.values().filter(|c| c.ended().is_none()).count() >= MAX_BACKGROUND {
        return Err(format!(
            "{MAX_BACKGROUND} background commands are already running on this device: stop one first"
        ));
    }
    let (mut child, group) = spawn(shell, cwd, command)?;
    let id = format!("cmd-{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
    let background = Arc::new(Background {
        id: id.clone(),
        command: command.to_owned(),
        cwd: cwd.to_owned(),
        chat: chat.map(str::to_owned),
        started: Instant::now(),
        group,
        output: Mutex::new(Ring::default()),
        end: Mutex::new(None),
        stopping: AtomicBool::new(false),
    });
    let (sender, mut chunks) = tokio::sync::mpsc::channel::<Vec<u8>>(16);
    let readers = pumps(&mut child, sender);
    let weak = Arc::downgrade(&background);
    tokio::spawn(async move {
        let collect = {
            let weak = weak.clone();
            async move {
                while let Some(chunk) = chunks.recv().await {
                    let Some(this) = weak.upgrade() else { return };
                    lock(&this.output).push(&chunk);
                }
            }
        };
        let mut collecting = tokio::spawn(collect);
        let status = child.wait().await.ok();
        // Its last words, briefly (as a run's).
        if tokio::time::timeout(DRAIN, &mut collecting).await.is_err() {
            collecting.abort();
        }
        let Some(this) = weak.upgrade() else { return };
        for reader in readers {
            reader.abort();
        }
        this.group.finished();
        *lock(&this.end) = Some(End {
            at: Instant::now(),
            code: status.and_then(|s| s.code()),
            stopped: this.stopping.load(Ordering::Acquire),
        });
    });
    let status = background.status(false);
    commands.insert(id, background);
    Ok(status)
}

/// A background command its agent's chat shows with its runtime's own
/// background processes (`Session::background_tasks`), tracked by the chat's
/// host wherever the command runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandTask {
    /// Where it runs; `None`: the chat's host itself.
    pub device: Option<String>,
    /// Its id there (`cmd-…`).
    pub id: String,
    /// As shown: its id is `cmd-…@device` for one elsewhere, the `id` its
    /// agent was given.
    pub task: zeron_proto::BackgroundTask,
}

impl CommandTask {
    /// A task id that names a command (`cmd-…`, `cmd-…@device`): its id and
    /// device. `None` for a runtime's own task.
    pub fn parse(task_id: &str) -> Option<(String, Option<String>)> {
        if !task_id.starts_with("cmd-") {
            return None;
        }
        Some(match task_id.rsplit_once('@') {
            Some((id, device)) => (id.to_owned(), Some(device.to_owned())),
            None => (task_id.to_owned(), None),
        })
    }
}

/// The newest [`MAX_READ`] bytes a background command printed, for a person
/// looking (it doesn't move what its agent reads next).
pub fn tail(id: &str) -> Result<zeron_proto::BackgroundTaskOutput, String> {
    let command = lock(&BACKGROUND).get(id).cloned().ok_or_else(|| unknown(id))?;
    let ring = lock(&command.output);
    let start = ring.bytes.len().saturating_sub(MAX_READ);
    let newest: Vec<u8> = ring.bytes.range(start..).copied().collect();
    Ok(zeron_proto::BackgroundTaskOutput {
        text: String::from_utf8_lossy(&newest).into_owned(),
        truncated: ring.written > newest.len() as u64,
    })
}

/// A background command's output since it was last read, and how it stands.
pub fn read(id: &str) -> Result<CommandStatus, String> {
    let command = lock(&BACKGROUND).get(id).cloned().ok_or_else(|| unknown(id))?;
    Ok(command.status(true))
}

/// Stop a background command and everything it started; its output since
/// the last read comes back.
pub async fn stop(id: &str) -> Result<CommandStatus, String> {
    let command = lock(&BACKGROUND).get(id).cloned().ok_or_else(|| unknown(id))?;
    if command.ended().is_none() {
        command.stop();
        let wait = Instant::now() + Duration::from_secs(5);
        while command.ended().is_none() && Instant::now() < wait {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
    Ok(command.status(true))
}

/// The background commands `chat` started here (every one, without a chat).
pub fn list(chat: Option<&str>) -> Vec<CommandStatus> {
    let mut commands = lock(&BACKGROUND);
    forget_ended(&mut commands);
    let mut listed: Vec<_> = commands
        .values()
        .filter(|c| chat.is_none_or(|chat| c.chat.as_deref() == Some(chat)))
        .map(|c| (c.started, c.status(false)))
        .collect();
    listed.sort_by_key(|(started, _)| *started);
    listed.into_iter().map(|(_, status)| status).collect()
}

fn unknown(id: &str) -> String {
    format!("No command {id} on this device (it may have ended over an hour ago, or the engine restarted)")
}

/// Ended commands past [`KEEP_ENDED`], and the oldest past [`MAX_ENDED`].
fn forget_ended(commands: &mut HashMap<String, Arc<Background>>) {
    commands.retain(|_, c| c.ended().is_none_or(|end| end.at.elapsed() < KEEP_ENDED));
    let mut ended: Vec<(Instant, String)> = commands
        .values()
        .filter_map(|c| c.ended().map(|end| (end.at, c.id.clone())))
        .collect();
    if ended.len() > MAX_ENDED {
        ended.sort();
        for (_, id) in &ended[..ended.len() - MAX_ENDED] {
            commands.remove(id);
        }
    }
}

/// Stop background commands whose chat is archived or deleted, as `chats`
/// (one engine's) change. A chat counts as deleted only once these chats
/// have shown it, so one not synced here yet isn't taken as gone. Runs while
/// `chats` does.
pub fn follow_chats(mut chats: tokio::sync::watch::Receiver<Vec<zeron_proto::Chat>>) {
    tokio::spawn(async move {
        // Commands whose chat these chats have shown.
        let mut seen = std::collections::HashSet::<String>::new();
        loop {
            {
                let commands: Vec<Arc<Background>> = lock(&BACKGROUND)
                    .values()
                    .filter(|c| c.chat.is_some() && c.ended().is_none())
                    .cloned()
                    .collect();
                seen.retain(|id| commands.iter().any(|c| &c.id == id));
                if !commands.is_empty() {
                    let chats = chats.borrow_and_update();
                    for command in commands {
                        let chat = command.chat.as_deref().and_then(|id| chats.iter().find(|c| c.id == id));
                        let gone = match chat {
                            Some(chat) => {
                                seen.insert(command.id.clone());
                                chat.archived
                            }
                            None => seen.contains(&command.id),
                        };
                        if gone {
                            tracing::info!(command = %command.id, "its chat went away: stopping it");
                            command.stop();
                        }
                    }
                }
            }
            if chats.changed().await.is_err() {
                return;
            }
        }
    });
}

/// Stop every background command (the engine is going).
pub fn shutdown() {
    for command in lock(&BACKGROUND).values() {
        if command.ended().is_none() {
            command.stop();
        }
    }
}

/// Start the shell on `script` in `cwd`, in a process group of its own.
fn spawn(shell: &str, cwd: &str, script: &str) -> Result<(tokio::process::Child, Group), String> {
    if script.trim().is_empty() {
        return Err("The command is empty".into());
    }
    if !std::fs::metadata(cwd).is_ok_and(|m| m.is_dir()) {
        return Err(format!("No folder {cwd} on this device"));
    }
    let mut cmd = tokio::process::Command::new(shell);
    if crate::terminals::is_powershell(shell) {
        cmd.args(["-NoLogo", "-NoProfile", "-NonInteractive", "-Command", script]);
    } else if cfg!(windows) {
        cmd.args(["/C", script]);
    } else {
        cmd.args(["-l", "-c", script]);
    }
    cmd.current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // Nothing waits on a pager or a password prompt.
        .env("PAGER", "cat")
        .env("GIT_PAGER", "cat")
        .env("GIT_TERMINAL_PROMPT", "0")
        .kill_on_drop(true);
    #[cfg(unix)]
    cmd.process_group(0);
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    let child = cmd.spawn().map_err(|e| format!("Couldn't start {shell}: {e}"))?;
    let group = Group::new(child.id());
    Ok((child, group))
}

/// Stdout and stderr, read into `to` as they come.
fn pumps(child: &mut tokio::process::Child, to: tokio::sync::mpsc::Sender<Vec<u8>>) -> Vec<tokio::task::JoinHandle<()>> {
    let mut readers = Vec::with_capacity(2);
    if let Some(out) = child.stdout.take() {
        readers.push(tokio::spawn(pump(out, to.clone())));
    }
    if let Some(err) = child.stderr.take() {
        readers.push(tokio::spawn(pump(err, to)));
    }
    readers
}

async fn pump(mut from: impl AsyncRead + Unpin, to: tokio::sync::mpsc::Sender<Vec<u8>>) {
    let mut buf = vec![0u8; 8 * 1024];
    loop {
        match from.read(&mut buf).await {
            Ok(0) | Err(_) => return,
            Ok(n) => {
                if to.send(buf[..n].to_vec()).await.is_err() {
                    return;
                }
            }
        }
    }
}

/// A shell whose `$?`, `pwd -P` and `>|` this module's cwd line uses.
fn posix(shell: &str) -> bool {
    let name = shell.rsplit(['/', '\\']).next().unwrap_or(shell);
    matches!(name, "sh" | "bash" | "zsh" | "dash" | "ksh" | "mksh" | "ash")
}

fn quote(path: &str) -> String {
    format!("'{}'", path.replace('\'', r"'\''"))
}

/// A command's process group. Dropped before [`Group::finished`] (the call
/// was abandoned), on a timeout, or when stopped, everything in it is killed.
struct Group {
    #[cfg_attr(not(unix), allow(dead_code))]
    pid: Option<u32>,
    done: AtomicBool,
}

impl Group {
    fn new(pid: Option<u32>) -> Self {
        Self { pid, done: false.into() }
    }

    fn kill(&self) {
        #[cfg(unix)]
        if let Some(pid) = self.pid.and_then(|p| i32::try_from(p).ok()).filter(|p| *p > 0) {
            // SAFETY: signals the group this module made (process_group(0)).
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
        }
        // Windows has no process groups to signal: end the tree.
        #[cfg(windows)]
        if let Some(pid) = self.pid {
            use std::os::windows::process::CommandExt as _;
            let _ = std::process::Command::new("taskkill")
                .args(["/T", "/F", "/PID", &pid.to_string()])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .creation_flags(0x0800_0000)
                .spawn();
        }
    }

    fn finished(&self) {
        self.done.store(true, Ordering::Release);
    }
}

impl Drop for Group {
    fn drop(&mut self) {
        if !self.done.load(Ordering::Acquire) {
            self.kill();
        }
    }
}

/// A run's output within [`HEAD`] + [`TAIL`] bytes: the first bytes, the
/// last bytes, and a count of the rest.
#[derive(Default)]
struct Capped {
    head: Vec<u8>,
    tail: VecDeque<u8>,
    omitted: u64,
}

impl Capped {
    fn push(&mut self, mut bytes: &[u8]) {
        if self.head.len() < HEAD {
            let take = bytes.len().min(HEAD - self.head.len());
            self.head.extend_from_slice(&bytes[..take]);
            bytes = &bytes[take..];
        }
        if bytes.len() >= TAIL {
            self.omitted += (self.tail.len() + bytes.len() - TAIL) as u64;
            self.tail.clear();
            bytes = &bytes[bytes.len() - TAIL..];
        }
        let overflow = (self.tail.len() + bytes.len()).saturating_sub(TAIL);
        self.tail.drain(..overflow);
        self.omitted += overflow as u64;
        self.tail.extend(bytes);
    }

    fn finish(self) -> (String, u64) {
        let head = String::from_utf8_lossy(&self.head).into_owned();
        let tail: Vec<u8> = self.tail.into();
        let tail = String::from_utf8_lossy(&tail).into_owned();
        if self.omitted == 0 {
            return (head + &tail, 0);
        }
        (
            format!("{head}\n… {} bytes of output left out …\n{tail}", self.omitted),
            self.omitted,
        )
    }
}

/// A background command's newest [`RING`] bytes, and how far a reader got.
#[derive(Default)]
struct Ring {
    bytes: VecDeque<u8>,
    /// Bytes ever written, and ever read (or passed over).
    written: u64,
    read: u64,
}

impl Ring {
    fn push(&mut self, bytes: &[u8]) {
        self.written += bytes.len() as u64;
        let bytes = &bytes[bytes.len().saturating_sub(RING)..];
        let overflow = (self.bytes.len() + bytes.len()).saturating_sub(RING);
        self.bytes.drain(..overflow);
        self.bytes.extend(bytes);
    }

    /// What's unread (its newest [`MAX_READ`] bytes), and how much unread
    /// output was lost or left out.
    fn read(&mut self) -> (String, u64) {
        let kept_from = self.written - self.bytes.len() as u64;
        let from = self.read.max(kept_from).max(self.written.saturating_sub(MAX_READ as u64));
        let omitted = from - self.read;
        let start = (from - kept_from) as usize;
        let unread: Vec<u8> = self.bytes.range(start..).copied().collect();
        self.read = self.written;
        (String::from_utf8_lossy(&unread).into_owned(), omitted)
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    async fn sh(command: &str, timeout: Duration) -> RunOutcome {
        let dir = std::env::temp_dir();
        run_with_shell("/bin/sh", dir.to_str().unwrap(), command, timeout).await.unwrap()
    }

    #[tokio::test]
    async fn a_command_gives_its_output_and_exit_code() {
        let ran = sh("echo out; echo err >&2; exit 3", DEFAULT_TIMEOUT).await;
        assert_eq!(ran.exit_code, Some(3));
        assert!(ran.output.contains("out\n") && ran.output.contains("err\n"), "{ran:?}");
        assert!(!ran.timed_out);
        let ran = sh("false", DEFAULT_TIMEOUT).await;
        assert_eq!(ran.exit_code, Some(1));
        // No input to wait for: a prompt reads end of input.
        let ran = sh("read line; echo \"got [$line]\"", DEFAULT_TIMEOUT).await;
        assert_eq!(ran.output, "got []\n");
    }

    #[tokio::test]
    async fn it_runs_where_it_is_asked_and_says_where_it_ended() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        let here = std::fs::canonicalize(dir.path()).unwrap();
        for shell in ["/bin/sh", "/bin/bash", "/bin/zsh"].into_iter().filter(|s| std::path::Path::new(s).exists()) {
            let ran = run_with_shell(shell, dir.path().to_str().unwrap(), "pwd; cd sub", DEFAULT_TIMEOUT)
                .await
                .unwrap();
            assert_eq!(std::fs::canonicalize(ran.output.trim()).unwrap(), here, "{shell}");
            assert_eq!(ran.cwd.as_deref().map(std::path::PathBuf::from), Some(here.join("sub")), "{shell}");
            assert_eq!(ran.exit_code, Some(0), "{shell}");
            // An `exit` keeps its code and still says where it ended.
            let ran = run_with_shell(shell, dir.path().to_str().unwrap(), "cd sub && exit 3", DEFAULT_TIMEOUT)
                .await
                .unwrap();
            assert_eq!((ran.exit_code, ran.cwd.map(std::path::PathBuf::from)), (Some(3), Some(here.join("sub"))), "{shell}");
        }
        assert!(run_with_shell("/bin/sh", "/no/such/folder", "pwd", DEFAULT_TIMEOUT).await.is_err());
        assert!(run_with_shell("/bin/sh", "/tmp", "  ", DEFAULT_TIMEOUT).await.is_err());
    }

    #[tokio::test]
    async fn a_timeout_kills_everything_the_command_started() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("survived");
        let command = format!("(sleep 1; touch {}) & sleep 30", marker.display());
        let started = Instant::now();
        let ran = sh(&command, Duration::from_millis(300)).await;
        assert!(ran.timed_out);
        assert_eq!(ran.exit_code, None);
        assert!(started.elapsed() < Duration::from_secs(5), "{:?}", started.elapsed());
        tokio::time::sleep(Duration::from_millis(1500)).await;
        assert!(!marker.exists(), "a child outlived the timeout");
    }

    #[tokio::test]
    async fn an_abandoned_run_kills_everything_it_started() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("survived");
        let command = format!("sleep 1; touch {}", marker.display());
        let run = sh(&command, DEFAULT_TIMEOUT);
        assert!(tokio::time::timeout(Duration::from_millis(200), run).await.is_err());
        tokio::time::sleep(Duration::from_millis(1500)).await;
        assert!(!marker.exists(), "the command outlived its caller");
    }

    #[tokio::test]
    async fn something_left_in_the_background_never_holds_the_reply() {
        let started = Instant::now();
        let ran = sh("sleep 30 & echo started", DEFAULT_TIMEOUT).await;
        assert_eq!(ran.exit_code, Some(0));
        assert_eq!(ran.output, "started\n");
        assert!(started.elapsed() < Duration::from_secs(3), "{:?}", started.elapsed());
    }

    #[tokio::test]
    async fn long_output_keeps_its_start_and_end_within_bounds() {
        let ran = sh("echo FIRST; head -c 1000000 /dev/zero | tr '\\0' x; echo; echo LAST", DEFAULT_TIMEOUT).await;
        assert!(ran.output.starts_with("FIRST\n"));
        assert!(ran.output.ends_with("LAST\n"));
        assert!(ran.omitted_bytes > 900_000);
        assert!(ran.output.len() < HEAD + TAIL + 100);
    }

    async fn until(what: &str, mut done: impl FnMut() -> bool) {
        let give_up = Instant::now() + Duration::from_secs(5);
        while !done() {
            assert!(Instant::now() < give_up, "{what}");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    #[tokio::test]
    async fn a_background_command_is_read_as_it_goes_and_stopped_with_all_it_started() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("survived");
        let command = format!("echo ready; (sleep 1; touch {}) & while true; do sleep 0.05; done", marker.display());
        let started = start_with_shell("/bin/sh", dir.path().to_str().unwrap(), &command, Some("chat-a")).unwrap();
        assert_eq!(started.status, "running");
        let id = started.id;
        let mut seen = String::new();
        until("its first output", || {
            seen.push_str(&read(&id).unwrap().output.unwrap());
            seen.contains("ready")
        })
        .await;
        // Read once: not again.
        assert_eq!(read(&id).unwrap().output.as_deref(), Some(""));
        assert!(list(Some("chat-a")).iter().any(|c| c.id == id && c.status == "running"));
        assert!(list(Some("chat-b")).iter().all(|c| c.id != id));
        assert!(running_count() >= 1);

        let stopped = stop(&id).await.unwrap();
        assert_eq!(stopped.status, "stopped");
        tokio::time::sleep(Duration::from_millis(1500)).await;
        assert!(!marker.exists(), "a child outlived its stop");
        assert!(stop("cmd-nope").await.is_err());
    }

    #[tokio::test]
    async fn a_background_command_that_ends_says_how() {
        let started = start_with_shell("/bin/sh", "/tmp", "echo done; exit 4", None).unwrap();
        until("it ends", || read(&started.id).is_ok_and(|c| c.status == "exited")).await;
        let ended = read(&started.id).unwrap();
        assert_eq!(ended.exit_code, Some(4));
    }

    #[tokio::test]
    async fn a_background_command_stops_when_its_chat_goes() {
        let chat = |id: &str, archived: bool| -> zeron_proto::Chat {
            serde_json::from_value(serde_json::json!({
                "id": id, "deviceId": "d", "title": null, "archived": archived, "cwd": null, "branch": null,
                "checkoutId": null, "config": null, "lastMessagePreview": null, "lastMessageAt": null,
                "createdAt": "2026-11-15T00:00:00Z",
            }))
            .unwrap()
        };
        let (chats, watched) = tokio::sync::watch::channel(Vec::new());
        follow_chats(watched);
        // Another engine's chats, which never had these: it stops none.
        let (others, other) = tokio::sync::watch::channel(Vec::new());
        follow_chats(other);
        others.send_replace(vec![chat("unrelated", false)]);
        let kept = start_with_shell("/bin/sh", "/tmp", "sleep 30", Some("kept")).unwrap().id;
        let archived = start_with_shell("/bin/sh", "/tmp", "sleep 30", Some("archived")).unwrap().id;
        let deleted = start_with_shell("/bin/sh", "/tmp", "sleep 30", Some("deleted")).unwrap().id;
        let unsynced = start_with_shell("/bin/sh", "/tmp", "sleep 30", Some("not-here-yet")).unwrap().id;
        chats.send_replace(vec![chat("kept", false), chat("archived", false), chat("deleted", false)]);
        // Let the first list land before the second replaces it.
        tokio::time::sleep(Duration::from_millis(200)).await;
        chats.send_replace(vec![chat("kept", false), chat("archived", true)]);
        for id in [&archived, &deleted] {
            until("stopped", || read(id).is_ok_and(|c| c.status == "stopped")).await;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
        for id in [&kept, &unsynced] {
            assert_eq!(read(id).unwrap().status, "running", "{id}");
            stop(id).await.unwrap();
        }
    }

    #[test]
    fn capped_output_counts_what_it_leaves_out() {
        let mut capped = Capped::default();
        capped.push(&vec![b'a'; HEAD]);
        capped.push(&vec![b'b'; TAIL + 10]);
        capped.push(b"end");
        let (output, omitted) = capped.finish();
        assert_eq!(omitted, 13);
        assert!(output.starts_with(&"a".repeat(HEAD)));
        assert!(output.ends_with("bend"));
    }

    #[test]
    fn a_ring_keeps_the_newest_and_reads_each_byte_once() {
        let mut ring = Ring::default();
        ring.push(b"hello ");
        assert_eq!(ring.read(), ("hello ".into(), 0));
        ring.push(&vec![b'x'; RING + 100]);
        ring.push(b"end");
        let (output, omitted) = ring.read();
        assert!(output.ends_with("xend") && output.len() == MAX_READ, "{}", output.len());
        assert_eq!(omitted, (RING + 100 + 3 - MAX_READ) as u64);
        assert_eq!(ring.read(), (String::new(), 0));
        assert!(ring.bytes.len() <= RING);
    }

    #[test]
    fn only_posix_shells_report_their_folder() {
        assert!(posix("/bin/zsh") && posix("/usr/bin/bash") && posix("sh"));
        assert!(!posix("/usr/bin/fish") && !posix(r"C:\pwsh.exe"));
        assert_eq!(quote("/tmp/a'b"), r"'/tmp/a'\''b'");
    }
}
