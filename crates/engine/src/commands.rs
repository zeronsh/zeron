//! Shell commands for agents, on this device or another of the person's
//! (Zeron MCP's `run_command` & co., forwarded by `targetDeviceId`): what an
//! agent's own shell tool does on its machine, on any of them. Owned by the
//! engine ([`crate::EngineCore`]) like its terminals, and shut down with it.
//!
//! - [`Commands::run`] (`RunCommand`): one command run to its end. Its
//!   output and exit code come back, and the folder it ended in (`cwd`, from
//!   a POSIX shell), so an agent's `cd` carries over to its next command.
//! - [`Commands::start`] (`RunCommand {background}`): one left running (a dev
//!   server, a watcher, a long build), read with [`Commands::read`], stopped
//!   with [`Commands::stop`], listed with [`Commands::list`] and watched with
//!   [`Commands::watch`] (`WatchBackgroundCommands`: how the chat's host
//!   keeps its background band current, wherever the command runs).
//!
//! No PTY and no session. The user's shell runs the command (`-c`;
//! PowerShell or cmd on Windows) with the PATH and variables their login
//! shell sets, captured once per process ([`zeron_harness::compose_user_path`]),
//! and empty stdin, so a prompt for input ends instead of hanging. Each runs
//! as an owned process ([`zeron_harness::process::owned`]): a process group
//! (Job Object on Windows) of its own, without the markers of a Claude
//! session Zeron may itself run in. What it costs is bounded:
//!
//! - **Memory**: a run keeps at most [`HEAD`] + [`TAIL`] bytes of output (its
//!   start and end); a background command its newest [`RING`] bytes. What's
//!   left out is counted, not kept. At most [`MAX_ENDED`] ended background
//!   commands are remembered, none for longer than [`KEEP_ENDED`].
//! - **Processes**: when a command ends, so does everything it started. At
//!   its timeout, on a stop, when the chat that started it is archived or
//!   deleted ([`Commands::follow_chats`]), and at engine shutdown, its group
//!   is asked to stop and killed after [`GRACE`]; a call abandoned mid-run
//!   kills it at once.
//! - **Concurrency**: at most [`MAX_RUNNING`] runs and [`MAX_BACKGROUND`]
//!   background commands at once per device; past that, refused, not queued.
//!
//! Running commands count as work ([`Commands::running`]): a Cloud machine
//! stays awake for them, and an update waits for them.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use futures::StreamExt as _;
use futures::stream::BoxStream;
use serde::Serialize;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::sync::{Notify, Semaphore, watch};
use zeron_harness::process::{Command, Stdio, owned};

pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);
pub const MAX_TIMEOUT: Duration = Duration::from_secs(600);
/// Output a run keeps from its start, and from its end.
const HEAD: usize = 16 * 1024;
const TAIL: usize = 48 * 1024;
/// Output a background command keeps (its newest), and the most one read
/// returns.
const RING: usize = 128 * 1024;
const MAX_READ: usize = 48 * 1024;
/// How long output is still read once a shell has exited: a process that
/// left the group (`setsid`) may hold the pipes open for good.
const DRAIN: Duration = Duration::from_millis(250);
/// Between asking a command's processes to stop and killing them.
pub const GRACE: Duration = Duration::from_secs(2);
pub const MAX_RUNNING: usize = 16;
pub const MAX_BACKGROUND: usize = 32;
const MAX_ENDED: usize = 32;
const KEEP_ENDED: Duration = Duration::from_secs(60 * 60);

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// This engine's agents' commands. Cheap to clone (shared state).
#[derive(Clone)]
pub struct Commands {
    inner: Arc<Inner>,
}

struct Inner {
    shell: String,
    kind: ShellKind,
    running: Semaphore,
    background: Mutex<HashMap<String, Arc<Background>>>,
    /// Bumped when a background command starts, ends or is forgotten.
    changes: watch::Sender<u64>,
    /// Commands shown on chats hosted here, followed wherever they run: one
    /// task per (chat, device), `None` being this one ([`Commands::show`]).
    followers: Mutex<HashMap<(String, Option<String>), tokio::task::AbortHandle>>,
}

impl Default for Commands {
    fn default() -> Self {
        Self::with_shell(crate::terminals::selected_shell())
    }
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

/// A background command, as [`Commands::start`], [`Commands::read`],
/// [`Commands::stop`] and [`Commands::list`] say it.
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
    /// [`Commands::read`]: output since the last read.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    /// [`Commands::read`]: bytes of that output no longer kept.
    #[serde(skip_serializing_if = "is_zero")]
    pub omitted_bytes: u64,
}

/// A background command its agent's chat shows with its runtime's own
/// background processes (`Session::background_tasks`), followed by the
/// chat's host wherever the command runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandTask {
    /// Where it runs; `None`: the chat's host itself.
    pub device: Option<String>,
    /// Its id there (`cmd-…`).
    pub id: String,
    /// As shown: `cmd-…@device` for one elsewhere, the id its agent was given.
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

struct Background {
    id: String,
    command: String,
    cwd: String,
    /// The chat that started it: it stops when that chat is archived or
    /// deleted ([`Commands::follow_chats`]).
    chat: Option<String>,
    started: Instant,
    output: Mutex<Ring>,
    end: watch::Sender<Option<End>>,
    stop: Notify,
}

#[derive(Clone, Copy)]
struct End {
    at: Instant,
    code: Option<i32>,
    stopped: bool,
}

impl Background {
    fn ended(&self) -> Option<End> {
        *self.end.borrow()
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
            duration_ms: end
                .map_or(self.started.elapsed(), |e| e.at - self.started)
                .as_millis() as u64,
            output,
            omitted_bytes,
        }
    }

    /// Ask it to stop, and wait (a bounded while) until it has.
    async fn stop(&self) {
        self.stop.notify_one();
        let mut end = self.end.subscribe();
        let _ = tokio::time::timeout(GRACE * 3, end.wait_for(Option::is_some)).await;
    }
}

impl Commands {
    pub(crate) fn with_shell(shell: String) -> Self {
        Self {
            inner: Arc::new(Inner {
                kind: ShellKind::of(&shell),
                shell,
                running: Semaphore::new(MAX_RUNNING),
                background: Mutex::new(HashMap::new()),
                changes: watch::Sender::new(0),
                followers: Mutex::new(HashMap::new()),
            }),
        }
    }

    /// Commands running now, run or background: work in progress.
    pub fn running(&self) -> usize {
        let background = lock(&self.inner.background)
            .values()
            .filter(|c| c.ended().is_none())
            .count();
        MAX_RUNNING - self.inner.running.available_permits() + background
    }

    /// Run `command` in `cwd` to its end, for at most `timeout`.
    pub async fn run(
        &self,
        cwd: &str,
        command: &str,
        timeout: Duration,
    ) -> Result<RunOutcome, String> {
        if command.trim().is_empty() {
            return Err("The command is empty".into());
        }
        let _slot = self.inner.running.try_acquire().map_err(|_| {
            format!(
                "{MAX_RUNNING} commands are already running on this device: wait for one to finish"
            )
        })?;
        let timeout = timeout.min(MAX_TIMEOUT);
        let started = Instant::now();
        // Where it ends up, written by the shell as it exits: a closed file
        // (Windows refuses a second writer while it's open here), removed
        // when this run is done.
        let ended_in = self
            .inner
            .kind
            .says_folder()
            .then(|| {
                tempfile::NamedTempFile::new()
                    .ok()
                    .map(|file| file.into_temp_path())
            })
            .flatten();
        let script = self.inner.kind.script(command, ended_in.is_some());
        let mut child = spawn(
            &self.inner.shell,
            self.inner.kind,
            cwd,
            &script,
            ended_in.as_deref(),
        )?;
        let (sender, mut chunks) = tokio::sync::mpsc::channel::<Vec<u8>>(16);
        let readers = pumps(&mut child, sender);

        let mut output = Capped::default();
        let deadline = tokio::time::Instant::now() + timeout;
        let mut status = None;
        // Until the shell exits (or time is up), taking output as it comes.
        let timed_out = loop {
            tokio::select! {
                exited = child.wait() => {
                    status = exited.ok();
                    break false;
                }
                Some(chunk) = chunks.recv() => output.push(&chunk),
                () = tokio::time::sleep_until(deadline) => break true,
            }
        };
        if timed_out {
            child.shutdown(GRACE).await;
        }
        drain(&mut chunks, |chunk| output.push(chunk)).await;
        for reader in readers {
            reader.abort();
        }
        // Everything it started goes with it.
        drop(child);

        let cwd = ended_in
            .filter(|_| !timed_out)
            .and_then(|path| std::fs::read_to_string(&path).ok())
            .map(|pwd| pwd.trim_end_matches(['\n', '\r']).to_owned())
            .filter(|pwd| !pwd.is_empty());
        let (output, omitted_bytes) = output.finish();
        Ok(RunOutcome {
            exit_code: if timed_out {
                None
            } else {
                status.and_then(|s| s.code())
            },
            output,
            omitted_bytes,
            timed_out,
            duration_ms: started.elapsed().as_millis() as u64,
            cwd,
        })
    }

    /// Start `command` in `cwd` and leave it running, for `chat`.
    pub fn start(
        &self,
        cwd: &str,
        command: &str,
        chat: Option<&str>,
    ) -> Result<CommandStatus, String> {
        if command.trim().is_empty() {
            return Err("The command is empty".into());
        }
        let mut commands = lock(&self.inner.background);
        forget_ended(&mut commands);
        if commands.values().filter(|c| c.ended().is_none()).count() >= MAX_BACKGROUND {
            return Err(format!(
                "{MAX_BACKGROUND} background commands are already running on this device: stop one first"
            ));
        }
        let script = self.inner.kind.script(command, false);
        let mut child = spawn(&self.inner.shell, self.inner.kind, cwd, &script, None)?;
        let id = format!("cmd-{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
        let background = Arc::new(Background {
            id: id.clone(),
            command: command.to_owned(),
            cwd: cwd.to_owned(),
            chat: chat.map(str::to_owned),
            started: Instant::now(),
            output: Mutex::new(Ring::default()),
            end: watch::Sender::new(None),
            stop: Notify::new(),
        });
        let (sender, mut chunks) = tokio::sync::mpsc::channel::<Vec<u8>>(16);
        let readers = pumps(&mut child, sender);
        let this = background.clone();
        let inner = Arc::downgrade(&self.inner);
        tokio::spawn(async move {
            let mut stopped = false;
            let status = loop {
                tokio::select! {
                    exited = child.wait() => break exited.ok(),
                    Some(chunk) = chunks.recv() => lock(&this.output).push(&chunk),
                    () = this.stop.notified() => {
                        stopped = true;
                        child.shutdown(GRACE).await;
                        break child.wait().await.ok();
                    }
                }
            };
            drain(&mut chunks, |chunk| lock(&this.output).push(chunk)).await;
            for reader in readers {
                reader.abort();
            }
            drop(child);
            this.end.send_replace(Some(End {
                at: Instant::now(),
                code: status.and_then(|s| s.code()),
                stopped,
            }));
            if let Some(inner) = inner.upgrade() {
                inner.changes.send_modify(|n| *n += 1);
            }
        });
        let status = background.status(false);
        commands.insert(id, background);
        drop(commands);
        self.inner.changes.send_modify(|n| *n += 1);
        Ok(status)
    }

    fn get(&self, id: &str) -> Result<Arc<Background>, String> {
        lock(&self.inner.background).get(id).cloned().ok_or_else(|| {
            format!("No command {id} on this device (it may have ended over an hour ago, or Zeron restarted there)")
        })
    }

    /// A background command's output since it was last read, and how it stands.
    pub fn read(&self, id: &str) -> Result<CommandStatus, String> {
        Ok(self.get(id)?.status(true))
    }

    /// The newest output a background command printed, for a person
    /// looking: what its agent reads next stays as it was.
    pub fn tail(&self, id: &str) -> Result<zeron_proto::BackgroundTaskOutput, String> {
        let command = self.get(id)?;
        let ring = lock(&command.output);
        let start = ring.bytes.len().saturating_sub(MAX_READ);
        let newest: Vec<u8> = ring.bytes.range(start..).copied().collect();
        Ok(zeron_proto::BackgroundTaskOutput {
            text: String::from_utf8_lossy(&newest).into_owned(),
            truncated: ring.written > newest.len() as u64,
        })
    }

    /// Stop a background command and everything it started; its output since
    /// the last read comes back.
    pub async fn stop(&self, id: &str) -> Result<CommandStatus, String> {
        let command = self.get(id)?;
        if command.ended().is_none() {
            command.stop().await;
        }
        Ok(command.status(true))
    }

    /// The background commands `chat` started here (every one, without a chat).
    pub fn list(&self, chat: Option<&str>) -> Vec<CommandStatus> {
        let mut commands = lock(&self.inner.background);
        if forget_ended(&mut commands) {
            self.inner.changes.send_modify(|n| *n += 1);
        }
        let mut listed: Vec<_> = commands
            .values()
            .filter(|c| chat.is_none_or(|chat| c.chat.as_deref() == Some(chat)))
            .map(|c| (c.started, c.status(false)))
            .collect();
        listed.sort_by_key(|(started, _)| *started);
        listed.into_iter().map(|(_, status)| status).collect()
    }

    /// [`Self::list`] for `chat` now, and again each time a background
    /// command starts or ends (`WatchBackgroundCommands`).
    pub fn watch(&self, chat: String) -> BoxStream<'static, serde_json::Value> {
        let changes = self.inner.changes.subscribe();
        let commands = Arc::downgrade(&self.inner);
        futures::stream::unfold(
            (changes, commands, true),
            move |(mut changes, commands, first)| {
                let chat = chat.clone();
                async move {
                    if !first {
                        changes.changed().await.ok()?;
                    }
                    changes.borrow_and_update();
                    let listed = Commands {
                        inner: commands.upgrade()?,
                    }
                    .list(Some(&chat));
                    let value = serde_json::to_value(listed).ok()?;
                    Some((value, (changes, commands, false)))
                }
            },
        )
        .boxed()
    }

    /// Show `task` on `chat`'s session row, and make sure something follows
    /// the commands of `chat` on `task.device`: `follow` starts that task
    /// when nothing does yet. The follower ends through [`Self::followed`].
    pub fn show(
        &self,
        sessions: &crate::sessions::SessionsEngine,
        chat: &str,
        task: CommandTask,
        follow: impl FnOnce() -> tokio::task::JoinHandle<()>,
    ) {
        let mut followers = lock(&self.inner.followers);
        let key = (chat.to_owned(), task.device.clone());
        sessions.show_command(chat, task);
        followers
            .entry(key)
            .or_insert_with(|| follow().abort_handle());
    }

    /// A follower's check before it ends: true while `chat` still shows
    /// commands on `device` (it goes on); otherwise it's forgotten (false).
    pub fn followed(
        &self,
        sessions: &crate::sessions::SessionsEngine,
        chat: &str,
        device: &Option<String>,
    ) -> bool {
        let mut followers = lock(&self.inner.followers);
        if sessions.shown_commands_on(chat, device) > 0 {
            return true;
        }
        followers.remove(&(chat.to_owned(), device.clone()));
        false
    }

    /// Stop background commands whose chat is archived or deleted, as
    /// `chats` (this engine's) change and as commands start (one started
    /// for a chat already archived, or deleted before the chats next change,
    /// is caught too). A chat counts as deleted only once these chats have
    /// shown it, so one not synced here yet isn't taken as gone. Ends with
    /// the engine.
    pub fn follow_chats(&self, mut chats: watch::Receiver<Vec<zeron_proto::Chat>>) {
        let inner: Weak<Inner> = Arc::downgrade(&self.inner);
        let mut started = self.inner.changes.subscribe();
        tokio::spawn(async move {
            // Commands whose chat these chats have shown.
            let mut seen = std::collections::HashSet::<String>::new();
            loop {
                let Some(live) = inner.upgrade() else { return };
                let commands: Vec<Arc<Background>> = lock(&live.background)
                    .values()
                    .filter(|c| c.chat.is_some() && c.ended().is_none())
                    .cloned()
                    .collect();
                drop(live);
                seen.retain(|id| commands.iter().any(|c| &c.id == id));
                let gone: Vec<Arc<Background>> = {
                    let chats = chats.borrow_and_update();
                    commands
                        .into_iter()
                        .filter(|command| {
                            match command
                                .chat
                                .as_deref()
                                .and_then(|id| chats.iter().find(|c| c.id == id))
                            {
                                Some(chat) => {
                                    seen.insert(command.id.clone());
                                    chat.archived
                                }
                                None => seen.contains(&command.id),
                            }
                        })
                        .collect()
                };
                for command in gone {
                    tracing::info!(command = %command.id, "its chat went away: stopping it");
                    command.stop.notify_one();
                }
                let next = tokio::select! {
                    changed = chats.changed() => changed,
                    changed = started.changed() => changed,
                };
                if next.is_err() {
                    return;
                }
            }
        });
    }

    /// Stop every background command (and what it started) and every
    /// follower: the engine is going.
    pub async fn shutdown(&self) {
        for (_, follower) in lock(&self.inner.followers).drain() {
            follower.abort();
        }
        let running: Vec<Arc<Background>> = lock(&self.inner.background)
            .values()
            .filter(|c| c.ended().is_none())
            .cloned()
            .collect();
        futures::future::join_all(running.iter().map(|c| c.stop())).await;
    }
}

/// What the user's shell is, for how a command is handed to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShellKind {
    /// sh, bash, zsh, …: `-c`, and it says where it ended (`trap … EXIT`).
    Posix,
    /// PowerShell 7 or Windows PowerShell: `-EncodedCommand` (no quoting to
    /// get wrong), a native command's exit code passed on, and where it
    /// ended.
    PowerShell,
    /// cmd.exe, on a Windows without PowerShell: `/D /S /C`.
    Cmd,
    /// Any other Unix shell (fish, …): `-c`.
    Other,
}

/// The longest PowerShell command line Windows takes (32 767 characters),
/// with room for the shell's path and flags.
const MAX_ENCODED_COMMAND: usize = 32_000;

impl ShellKind {
    fn of(shell: &str) -> Self {
        let name = shell.rsplit(['/', '\\']).next().unwrap_or(shell);
        let stem = name
            .rsplit_once('.')
            .map_or(name, |(stem, _)| stem)
            .to_ascii_lowercase();
        if crate::terminals::is_powershell(shell) {
            Self::PowerShell
        } else if matches!(
            stem.as_str(),
            "sh" | "bash" | "zsh" | "dash" | "ksh" | "mksh" | "ash"
        ) {
            Self::Posix
        } else if cfg!(windows) || stem == "cmd" {
            Self::Cmd
        } else {
            Self::Other
        }
    }

    fn says_folder(self) -> bool {
        matches!(self, Self::Posix | Self::PowerShell)
    }

    /// `command` as this shell runs it; with `says_folder`, it also writes
    /// the folder it ended in to the file [`FOLDER_FILE_ENV`] names (in
    /// PowerShell, not after an `exit`). The path is never in the script,
    /// so nothing in it needs quoting.
    fn script(self, command: &str, says_folder: bool) -> String {
        match (self, says_folder) {
            (Self::Posix, true) => format!(
                "trap '__zeron_status=$?; pwd -P >| \"${FOLDER_FILE_ENV}\" 2>/dev/null; exit $__zeron_status' EXIT\n{command}\n"
            ),
            (Self::PowerShell, says_folder) => {
                let say_folder = if says_folder {
                    format!(
                        "try {{ [System.IO.File]::WriteAllText($env:{FOLDER_FILE_ENV}, (Get-Location).ProviderPath) }} catch {{}}\n"
                    )
                } else {
                    String::new()
                };
                // Out in UTF-8 (as it's read back), without progress bars;
                // a failure exits with a native command's own code, else 1.
                format!(
                    "$ProgressPreference = 'SilentlyContinue'\n\
                     try {{ [Console]::OutputEncoding = [System.Text.Encoding]::UTF8 }} catch {{}}\n\
                     $OutputEncoding = [System.Text.Encoding]::UTF8\n\
                     {command}\n\
                     $__zeronOk = $?\n\
                     $__zeronCode = $global:LASTEXITCODE\n\
                     {say_folder}\
                     if (-not $__zeronOk) {{ if ($__zeronCode) {{ exit $__zeronCode }}; exit 1 }}\n\
                     exit 0\n"
                )
            }
            _ => command.to_owned(),
        }
    }

    fn args(self, script: &str) -> Result<Vec<String>, String> {
        Ok(match self {
            Self::PowerShell => {
                use base64::Engine as _;
                let utf16: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
                let encoded = base64::engine::general_purpose::STANDARD.encode(utf16);
                if encoded.len() > MAX_ENCODED_COMMAND {
                    return Err(format!(
                        "This command is too long for PowerShell's command line ({} characters): put it in a script file and run that",
                        script.len()
                    ));
                }
                [
                    "-NoLogo",
                    "-NoProfile",
                    "-NonInteractive",
                    "-EncodedCommand",
                ]
                .into_iter()
                .map(str::to_owned)
                .chain([encoded])
                .collect()
            }
            // `/S`: cmd strips the quotes around the command itself.
            Self::Cmd => vec!["/D".into(), "/S".into(), "/C".into(), script.into()],
            Self::Posix | Self::Other => vec!["-c".into(), script.into()],
        })
    }
}

/// The variable naming the file a command writes the folder it ended in to
/// ([`ShellKind::script`]).
const FOLDER_FILE_ENV: &str = "ZERON_COMMAND_FOLDER_FILE";

/// Start the shell on `script` in `cwd` as an owned process. Zeron's own
/// variables (its edge token, a Cloud machine's enrollment code, its ports)
/// stay out of the command's environment, as for an agent installer
/// (crates/harness/src/install.rs): a command that prints its environment
/// must not put them in the transcript.
fn spawn(
    shell: &str,
    kind: ShellKind,
    cwd: &str,
    script: &str,
    folder_file: Option<&std::path::Path>,
) -> Result<owned::Child, String> {
    if !std::fs::metadata(cwd).is_ok_and(|m| m.is_dir()) {
        return Err(format!("No folder {cwd} on this device"));
    }
    let mut cmd = Command::new(shell);
    cmd.args(kind.args(script)?);
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("ZERON_") {
            cmd.env_remove(key);
        }
    }
    if let Some(file) = folder_file {
        cmd.env(FOLDER_FILE_ENV, file);
    }
    cmd.current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // Nothing waits on a pager or a password prompt.
        .env("PAGER", "cat")
        .env("GIT_PAGER", "cat")
        .env("GIT_TERMINAL_PROMPT", "0");
    zeron_harness::compose_user_path(&mut cmd);
    owned::configure(&mut cmd);
    let child = cmd
        .spawn()
        .map_err(|e| format!("Couldn't start {shell}: {e}"))?;
    Ok(owned::Child::new(child))
}

/// Stdout and stderr, read into `to` as they come.
fn pumps(
    child: &mut owned::Child,
    to: tokio::sync::mpsc::Sender<Vec<u8>>,
) -> Vec<tokio::task::JoinHandle<()>> {
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

/// What's still in the pipes once the shell has exited, for at most [`DRAIN`].
async fn drain(chunks: &mut tokio::sync::mpsc::Receiver<Vec<u8>>, mut take: impl FnMut(&[u8])) {
    let until = tokio::time::Instant::now() + DRAIN;
    loop {
        tokio::select! {
            chunk = chunks.recv() => match chunk {
                Some(chunk) => take(&chunk),
                None => return,
            },
            () = tokio::time::sleep_until(until) => return,
        }
    }
}

/// Ended commands past [`KEEP_ENDED`], and the oldest past [`MAX_ENDED`];
/// whether any went.
fn forget_ended(commands: &mut HashMap<String, Arc<Background>>) -> bool {
    let before = commands.len();
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
    commands.len() != before
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
            format!(
                "{head}\n… {} bytes of output left out …\n{tail}",
                self.omitted
            ),
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
        let from = self
            .read
            .max(kept_from)
            .max(self.written.saturating_sub(MAX_READ as u64));
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

    fn sh() -> Commands {
        Commands::with_shell("/bin/sh".into())
    }

    async fn run(command: &str, timeout: Duration) -> RunOutcome {
        let dir = std::env::temp_dir();
        sh().run(dir.to_str().unwrap(), command, timeout)
            .await
            .unwrap()
    }

    async fn until(what: &str, mut done: impl FnMut() -> bool) {
        let give_up = Instant::now() + Duration::from_secs(10);
        while !done() {
            assert!(Instant::now() < give_up, "{what}");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    #[tokio::test]
    async fn a_command_gives_its_output_and_exit_code() {
        let ran = run("echo out; echo err >&2; exit 3", DEFAULT_TIMEOUT).await;
        assert_eq!(ran.exit_code, Some(3));
        assert!(
            ran.output.contains("out\n") && ran.output.contains("err\n"),
            "{ran:?}"
        );
        assert!(!ran.timed_out);
        assert_eq!(run("false", DEFAULT_TIMEOUT).await.exit_code, Some(1));
        // No input to wait for: a prompt reads end of input.
        assert_eq!(
            run("read line; echo \"got [$line]\"", DEFAULT_TIMEOUT)
                .await
                .output,
            "got []\n"
        );
        // Not a nested Claude session's child.
        let ran = run("echo \"[${CLAUDECODE:-}]\"", DEFAULT_TIMEOUT).await;
        assert_eq!(ran.output, "[]\n");
    }

    #[tokio::test]
    async fn it_runs_where_it_is_asked_and_says_where_it_ended() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        let here = std::fs::canonicalize(dir.path()).unwrap();
        for shell in ["/bin/sh", "/bin/bash", "/bin/zsh"]
            .into_iter()
            .filter(|s| std::path::Path::new(s).exists())
        {
            let commands = Commands::with_shell(shell.into());
            let ran = commands
                .run(dir.path().to_str().unwrap(), "pwd; cd sub", DEFAULT_TIMEOUT)
                .await
                .unwrap();
            assert_eq!(
                std::fs::canonicalize(ran.output.trim()).unwrap(),
                here,
                "{shell}"
            );
            assert_eq!(
                ran.cwd.map(std::path::PathBuf::from),
                Some(here.join("sub")),
                "{shell}"
            );
            // An `exit` keeps its code and still says where it ended.
            let ran = commands
                .run(
                    dir.path().to_str().unwrap(),
                    "cd sub && exit 3",
                    DEFAULT_TIMEOUT,
                )
                .await
                .unwrap();
            assert_eq!(
                (ran.exit_code, ran.cwd.map(std::path::PathBuf::from)),
                (Some(3), Some(here.join("sub"))),
                "{shell}"
            );
        }
        assert!(
            sh().run("/no/such/folder", "pwd", DEFAULT_TIMEOUT)
                .await
                .is_err()
        );
        assert!(sh().run("/tmp", "  ", DEFAULT_TIMEOUT).await.is_err());
    }

    #[tokio::test]
    async fn a_timeout_ends_everything_the_command_started() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("survived");
        let command = format!("(sleep 1; touch {}) & sleep 30", marker.display());
        let started = Instant::now();
        let ran = run(&command, Duration::from_millis(300)).await;
        assert!(ran.timed_out);
        assert_eq!(ran.exit_code, None);
        assert!(
            started.elapsed() < GRACE + Duration::from_secs(2),
            "{:?}",
            started.elapsed()
        );
        tokio::time::sleep(Duration::from_millis(1500)).await;
        assert!(!marker.exists(), "a child outlived the timeout");
    }

    #[tokio::test]
    async fn an_abandoned_run_ends_everything_it_started() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("survived");
        let command = format!("sleep 1; touch {}", marker.display());
        assert!(
            tokio::time::timeout(Duration::from_millis(200), run(&command, DEFAULT_TIMEOUT))
                .await
                .is_err()
        );
        tokio::time::sleep(Duration::from_millis(1500)).await;
        assert!(!marker.exists(), "the command outlived its caller");
    }

    #[tokio::test]
    async fn what_a_run_leaves_behind_ends_with_it_and_never_holds_the_reply() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("survived");
        let command = format!("(sleep 1; touch {}) & echo started", marker.display());
        let started = Instant::now();
        let ran = run(&command, DEFAULT_TIMEOUT).await;
        assert_eq!((ran.exit_code, ran.output.as_str()), (Some(0), "started\n"));
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "{:?}",
            started.elapsed()
        );
        tokio::time::sleep(Duration::from_millis(1500)).await;
        assert!(
            !marker.exists(),
            "left running: background: true is for that"
        );
    }

    #[tokio::test]
    async fn long_output_keeps_its_start_and_end_within_bounds() {
        let ran = run(
            "echo FIRST; head -c 1000000 /dev/zero | tr '\\0' x; echo; echo LAST",
            DEFAULT_TIMEOUT,
        )
        .await;
        assert!(ran.output.starts_with("FIRST\n") && ran.output.ends_with("LAST\n"));
        assert!(ran.omitted_bytes > 900_000);
        assert!(ran.output.len() < HEAD + TAIL + 100);
    }

    #[tokio::test]
    async fn a_background_command_is_read_as_it_goes_and_stopped_with_all_it_started() {
        let commands = sh();
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("survived");
        let command = format!(
            "echo ready; (sleep 1; touch {}) & while true; do sleep 0.05; done",
            marker.display()
        );
        let id = commands
            .start(dir.path().to_str().unwrap(), &command, Some("chat-a"))
            .unwrap()
            .id;
        assert_eq!(commands.running(), 1);
        let mut seen = String::new();
        until("its first output", || {
            seen.push_str(&commands.read(&id).unwrap().output.unwrap());
            seen.contains("ready")
        })
        .await;
        // A person looking doesn't take what the agent reads next.
        assert!(commands.tail(&id).unwrap().text.contains("ready"));
        assert_eq!(commands.read(&id).unwrap().output.as_deref(), Some(""));
        assert_eq!(commands.list(Some("chat-a")).len(), 1);
        assert!(commands.list(Some("chat-b")).is_empty());

        let stopped = commands.stop(&id).await.unwrap();
        assert_eq!(stopped.status, "stopped");
        assert_eq!(commands.running(), 0);
        tokio::time::sleep(Duration::from_millis(1500)).await;
        assert!(!marker.exists(), "a child outlived its stop");
        assert!(commands.stop("cmd-nope").await.is_err());
    }

    #[tokio::test]
    async fn a_background_command_that_ends_says_how_and_is_watched_ending() {
        let commands = sh();
        let mut watched = commands.watch("chat-w".into());
        assert_eq!(watched.next().await.unwrap(), serde_json::json!([]));
        let id = commands
            .start("/tmp", "echo done; exit 4", Some("chat-w"))
            .unwrap()
            .id;
        let mut ended = false;
        while !ended {
            let listed = tokio::time::timeout(Duration::from_secs(5), watched.next())
                .await
                .unwrap()
                .unwrap();
            ended = listed[0]["status"] == "exited";
        }
        let read = commands.read(&id).unwrap();
        assert_eq!(
            (read.exit_code, read.output.as_deref()),
            (Some(4), Some("done\n"))
        );
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
        let commands = sh();
        let (chats, watched) = watch::channel(Vec::new());
        commands.follow_chats(watched);
        let start = |chat: &str| commands.start("/tmp", "sleep 30", Some(chat)).unwrap().id;
        let (kept, archived, deleted, unsynced) = (
            start("kept"),
            start("archived"),
            start("deleted"),
            start("not-here-yet"),
        );
        chats.send_replace(vec![
            chat("kept", false),
            chat("archived", false),
            chat("deleted", false),
        ]);
        // Let the first list land before the second replaces it.
        tokio::time::sleep(Duration::from_millis(200)).await;
        chats.send_replace(vec![chat("kept", false), chat("archived", true)]);
        for id in [&archived, &deleted] {
            until("stopped", || {
                commands.read(id).is_ok_and(|c| c.status == "stopped")
            })
            .await;
        }
        for id in [&kept, &unsynced] {
            assert_eq!(commands.read(id).unwrap().status, "running", "{id}");
        }
        commands.shutdown().await;
        for id in [&kept, &unsynced] {
            assert_eq!(commands.read(id).unwrap().status, "stopped", "{id}");
        }
    }

    #[tokio::test]
    async fn a_command_for_a_chat_already_gone_is_stopped_as_it_starts() {
        let chat = |id: &str, archived: bool| -> zeron_proto::Chat {
            serde_json::from_value(serde_json::json!({
                "id": id, "deviceId": "d", "title": null, "archived": archived, "cwd": null, "branch": null,
                "checkoutId": null, "config": null, "lastMessagePreview": null, "lastMessageAt": null,
                "createdAt": "2026-11-15T00:00:00Z",
            }))
            .unwrap()
        };
        let commands = sh();
        let (chats, watched) =
            watch::channel(vec![chat("archived", true), chat("short-lived", false)]);
        commands.follow_chats(watched);
        tokio::time::sleep(Duration::from_millis(50)).await;
        // Archived before its command started: stopped without the chats
        // changing again.
        let archived = commands
            .start("/tmp", "sleep 30", Some("archived"))
            .unwrap()
            .id;
        until("stopped", || {
            commands
                .read(&archived)
                .is_ok_and(|c| c.status == "stopped")
        })
        .await;
        // Seen as its command starts, then deleted: stopped.
        let short = commands
            .start("/tmp", "sleep 30", Some("short-lived"))
            .unwrap()
            .id;
        tokio::time::sleep(Duration::from_millis(100)).await;
        chats.send_replace(Vec::new());
        until("stopped", || {
            commands.read(&short).is_ok_and(|c| c.status == "stopped")
        })
        .await;
        commands.shutdown().await;
    }

    #[tokio::test]
    async fn zerons_own_variables_stay_out_of_a_command() {
        // SAFETY: a name no other test reads or writes.
        unsafe { std::env::set_var("ZERON_TEST_SECRET_9F3A", "leaked") };
        let ran = run("echo \"[${ZERON_TEST_SECRET_9F3A:-}]\"", DEFAULT_TIMEOUT).await;
        assert_eq!(ran.output, "[]\n");
    }

    #[tokio::test]
    async fn commands_are_each_engines_own() {
        let (one, two) = (sh(), sh());
        let id = one.start("/tmp", "sleep 30", None).unwrap().id;
        assert!(two.read(&id).is_err());
        two.shutdown().await;
        assert_eq!(one.read(&id).unwrap().status, "running");
        one.shutdown().await;
    }

    #[test]
    fn capped_output_counts_what_it_leaves_out() {
        let mut capped = Capped::default();
        capped.push(&vec![b'a'; HEAD]);
        capped.push(&vec![b'b'; TAIL + 10]);
        capped.push(b"end");
        let (output, omitted) = capped.finish();
        assert_eq!(omitted, 13);
        assert!(output.starts_with(&"a".repeat(HEAD)) && output.ends_with("bend"));
    }

    #[test]
    fn a_ring_keeps_the_newest_and_reads_each_byte_once() {
        let mut ring = Ring::default();
        ring.push(b"hello ");
        assert_eq!(ring.read(), ("hello ".into(), 0));
        ring.push(&vec![b'x'; RING + 100]);
        ring.push(b"end");
        let (output, omitted) = ring.read();
        assert!(
            output.ends_with("xend") && output.len() == MAX_READ,
            "{}",
            output.len()
        );
        assert_eq!(omitted, (RING + 100 + 3 - MAX_READ) as u64);
        assert_eq!(ring.read(), (String::new(), 0));
        assert!(ring.bytes.len() <= RING);
    }

    #[test]
    fn task_ids_name_their_command_and_device() {
        assert_eq!(
            CommandTask::parse("cmd-1@dev-2"),
            Some(("cmd-1".into(), Some("dev-2".into())))
        );
        assert_eq!(CommandTask::parse("cmd-1"), Some(("cmd-1".into(), None)));
        assert_eq!(CommandTask::parse("bash_1"), None);
    }

    #[test]
    fn each_shell_is_handed_its_command_its_own_way() {
        assert_eq!(ShellKind::of("/bin/zsh"), ShellKind::Posix);
        assert_eq!(ShellKind::of("/usr/bin/bash"), ShellKind::Posix);
        assert_eq!(ShellKind::of("/usr/bin/fish"), ShellKind::Other);
        assert_eq!(
            ShellKind::of(r"C:\Program Files\PowerShell\7\pwsh.exe"),
            ShellKind::PowerShell
        );
        assert_eq!(
            ShellKind::of(r"C:\Windows\system32\cmd.exe"),
            ShellKind::Cmd
        );
        // PowerShell takes it encoded (UTF-16LE base64): no quoting to break.
        let args = ShellKind::PowerShell.args("echo \"it's\"").unwrap();
        assert_eq!(
            args[..4],
            [
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-EncodedCommand"
            ]
        );
        use base64::Engine as _;
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(&args[4])
            .unwrap();
        let utf16: Vec<u16> = decoded
            .chunks(2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .collect();
        assert_eq!(String::from_utf16(&utf16).unwrap(), "echo \"it's\"");
        assert!(
            ShellKind::PowerShell
                .args(&"x".repeat(MAX_ENCODED_COMMAND))
                .is_err()
        );
        // Its script passes a failure's code on, and says where it ended to
        // the file the environment names: no path in it to quote.
        let script = ShellKind::PowerShell.script("git status", true);
        assert!(script.contains("\ngit status\n$__zeronOk = $?"), "{script}");
        assert!(
            script.contains("WriteAllText($env:ZERON_COMMAND_FOLDER_FILE,"),
            "{script}"
        );
        assert!(script.contains("exit $__zeronCode"), "{script}");
        let posix = ShellKind::Posix.script("ls", true);
        assert!(
            posix.contains("pwd -P >| \"$ZERON_COMMAND_FOLDER_FILE\""),
            "{posix}"
        );
        assert_eq!(ShellKind::Posix.script("ls", false), "ls");
    }
}

/// The same guarantees through Windows' own shell (PowerShell, as
/// [`crate::terminals::selected_shell`] picks it), run by Windows CI.
#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;

    fn here() -> String {
        std::env::temp_dir().to_string_lossy().into_owned()
    }

    #[tokio::test]
    async fn powershell_gives_output_exit_codes_and_where_it_ended() {
        let commands = Commands::default();
        assert_eq!(commands.inner.kind, ShellKind::PowerShell);
        let ran = commands
            .run(
                &here(),
                "Write-Output \"it's \"\"quoted\"\"\"; exit 3",
                DEFAULT_TIMEOUT,
            )
            .await
            .unwrap();
        assert_eq!(ran.exit_code, Some(3), "{ran:?}");
        assert!(ran.output.contains("it's \"quoted\""), "{ran:?}");
        // A native command's own code, not PowerShell's 1.
        let ran = commands
            .run(&here(), "cmd /c exit 5", DEFAULT_TIMEOUT)
            .await
            .unwrap();
        assert_eq!(ran.exit_code, Some(5), "{ran:?}");
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("sub")).unwrap();
        let ran = commands
            .run(
                dir.path().to_str().unwrap(),
                "Set-Location sub",
                DEFAULT_TIMEOUT,
            )
            .await
            .unwrap();
        assert_eq!(ran.exit_code, Some(0), "{ran:?}");
        let ended = std::fs::canonicalize(ran.cwd.expect("where it ended")).unwrap();
        assert_eq!(
            ended,
            std::fs::canonicalize(dir.path().join("sub")).unwrap()
        );
    }

    #[tokio::test]
    async fn powershell_commands_time_out_and_stop_with_all_they_started() {
        let commands = Commands::default();
        let started = Instant::now();
        let ran = commands
            .run(&here(), "ping -n 30 127.0.0.1", Duration::from_millis(500))
            .await
            .unwrap();
        assert!(ran.timed_out, "{ran:?}");
        assert!(started.elapsed() < GRACE + Duration::from_secs(5));

        let id = commands
            .start(&here(), "ping -n 60 127.0.0.1", None)
            .unwrap()
            .id;
        let stopped = commands.stop(&id).await.unwrap();
        assert_eq!(stopped.status, "stopped");
        assert_eq!(commands.running(), 0);
    }
}
