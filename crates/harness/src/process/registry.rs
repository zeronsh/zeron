//! No agent outlives its engine, whatever way the engine dies.
//!
//! A clean shutdown already tears every agent down (`owned::Child`,
//! `shutdown_child`). An engine that is SIGKILLed or crashes runs none of
//! that, and its agents are adopted by init and keep running: a bench that
//! killed its engine found five `claude --print` still running eight
//! minutes later. Per OS:
//!
//! - **Windows:** each agent is created inside its own Job Object with
//!   `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`, whose only handle the engine holds
//!   (`process::windows`), so the kernel ends the job, descendants included,
//!   when the engine dies. Nothing to add here.
//! - **Linux:** `PR_SET_PDEATHSIG(SIGTERM)` on each agent ([`spawn`]): the
//!   kernel signals it the moment the engine dies. SIGTERM, not SIGKILL:
//!   Claude Code saves an in-flight login refresh on SIGTERM, and a SIGKILL
//!   mid-refresh loses the rotated refresh token, which logs the user out.
//!   The death signal follows the *thread* that forked, not the process, so
//!   agents are forked from one thread that lives as long as the process: a
//!   runtime's blocking-pool thread that retires would take its agents with it.
//! - **macOS, and the backstop everywhere on Unix:** there is no kernel
//!   equivalent. The engine records each agent it starts in its data dir
//!   (`agent-processes/<engine pid>-<engine start>.json`) and the next engine
//!   on that data dir ends what is left ([`sweep`]): only a process that is
//!   provably the one recorded (same pid and start time, still running what
//!   was launched), that
//!   init has adopted, and whose engine is gone, and only once no Claude
//!   login refresh is in flight for it.
//!
//! The ledger is per process, not per engine: every data dir this process
//! serves gets the whole list. A dir's engine lock means no other live
//! engine writes there, and a ledger whose engine still runs is never swept.
use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::sync::Mutex;
#[cfg(unix)]
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::{Child, Command};

/// The folder inside a data dir where each engine process keeps its ledger.
pub const LEDGER_DIR: &str = "agent-processes";

/// One agent as recorded at its start.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    pub pid: i32,
    /// When it started, as the process table reports it: tells it apart
    /// from an unrelated process that later gets the same pid.
    pub start: u64,
    /// The image it ran at launch.
    pub exe: PathBuf,
    /// The program it was launched as: for a `#!` script (or `/bin/sh`,
    /// which macOS runs as bash) the image is the interpreter, and the
    /// script shows in its arguments instead.
    #[serde(default)]
    pub program: String,
    /// Its process group (its own pid when it leads one).
    pub pgid: i32,
    /// Its working directory (the chat's worktree, for a chat's run).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
    /// Claude Code's config dir as the agent sees it: where a refresh of
    /// its login (or of a Claude it runs) takes its lock.
    pub claude_config: PathBuf,
}

#[cfg(unix)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Engine {
    pid: i32,
    start: u64,
}

#[cfg(unix)]
#[derive(Debug, Serialize, Deserialize)]
struct Ledger {
    engine: Engine,
    agents: Vec<Record>,
}

/// The data dirs this process serves, and the agents it started.
#[cfg(unix)]
struct State {
    dirs: Vec<PathBuf>,
    agents: Vec<Record>,
}

#[cfg(unix)]
static STATE: Mutex<State> = Mutex::new(State {
    dirs: Vec::new(),
    agents: Vec::new(),
});

#[cfg(unix)]
fn state() -> std::sync::MutexGuard<'static, State> {
    STATE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Start an agent CLI (or a process that runs one), so that it dies with
/// this engine: see the module docs.
pub fn spawn(command: &mut Command) -> std::io::Result<Child> {
    #[cfg(unix)]
    {
        let cwd = command.as_std().get_current_dir().map(Path::to_path_buf);
        let program = command
            .as_std()
            .get_program()
            .to_string_lossy()
            .into_owned();
        let claude_config = claude_config_of(command);
        #[cfg(target_os = "linux")]
        let child = linux::spawn(command)?;
        #[cfg(not(target_os = "linux"))]
        let child = command.spawn()?;
        if let Some(pid) = child.id() {
            record(pid as i32, program, cwd, claude_config);
        }
        Ok(child)
    }
    #[cfg(not(unix))]
    command.spawn()
}

/// The Claude config dir `command` will run with: its own environment over
/// this process's.
#[cfg(unix)]
fn claude_config_of(command: &Command) -> PathBuf {
    let var = |name: &str| match command
        .as_std()
        .get_envs()
        .find(|(key, _)| *key == std::ffi::OsStr::new(name))
    {
        Some((_, value)) => value.map(std::ffi::OsString::from),
        None => std::env::var_os(name),
    };
    crate::claude::refresh_lock::config_dir_with(
        var("CLAUDE_SECURESTORAGE_CONFIG_DIR"),
        var("CLAUDE_CONFIG_DIR"),
    )
}

/// This process, as its ledger names it.
#[cfg(unix)]
fn this_engine() -> Option<Engine> {
    static ENGINE: std::sync::OnceLock<Option<Engine>> = std::sync::OnceLock::new();
    *ENGINE.get_or_init(|| {
        let pid = std::process::id() as i32;
        super::proc_table::start_time(pid).map(|start| Engine { pid, start })
    })
}

#[cfg(unix)]
fn ledger_name(engine: Engine) -> String {
    format!("{}-{}.json", engine.pid, engine.start)
}

#[cfg(unix)]
fn record(pid: i32, program: String, cwd: Option<PathBuf>, claude_config: PathBuf) {
    let mut state = state();
    if state.dirs.is_empty() {
        // No engine in this process keeps a ledger (a CLI, a test).
        return;
    }
    let (Some(start), Some(exe)) = (
        super::proc_table::start_time(pid),
        super::proc_table::exe(pid),
    ) else {
        tracing::warn!(
            pid,
            "agent started, but the process table can't identify it"
        );
        return;
    };
    // SAFETY: getpgid only inspects our own, unreaped child.
    let pgid = unsafe { libc::getpgid(pid) };
    state.agents.retain(|agent| agent.pid != pid);
    state.agents.push(Record {
        pid,
        start,
        exe,
        program,
        pgid,
        cwd,
        claude_config,
    });
    write_ledgers(&mut state);
}

/// Drop the record of an agent that has exited.
pub(crate) fn forget(pid: u32) {
    #[cfg(unix)]
    {
        let mut state = state();
        let before = state.agents.len();
        state.agents.retain(|agent| agent.pid != pid as i32);
        if state.agents.len() != before {
            write_ledgers(&mut state);
        }
    }
    #[cfg(not(unix))]
    let _ = pid;
}

/// Rewrite this process's ledger in every data dir it serves, without the
/// agents that have exited since.
#[cfg(unix)]
fn write_ledgers(state: &mut State) {
    let Some(engine) = this_engine() else {
        return;
    };
    state
        .agents
        .retain(|agent| super::still_same(agent.pid, Some(agent.start)));
    let ledger = Ledger {
        engine,
        agents: state.agents.clone(),
    };
    let mut dirs = state.dirs.clone();
    dirs.sort();
    dirs.dedup();
    for dir in dirs {
        let path = dir.join(LEDGER_DIR).join(ledger_name(engine));
        if let Err(error) = write_ledger(&path, &ledger) {
            tracing::warn!(path = %path.display(), %error, "could not record the engine's agents");
        }
    }
}

/// Write (or, empty, remove) a ledger in one step: never a torn file. Not
/// synced to disk: it is on an agent's launch path, and a power loss ends
/// every agent anyway.
#[cfg(unix)]
fn write_ledger(path: &Path, ledger: &Ledger) -> std::io::Result<()> {
    if ledger.agents.is_empty() {
        return match std::fs::remove_file(path) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        };
    }
    let parent = path.parent().expect("ledger path has a parent");
    std::fs::create_dir_all(parent)?;
    let mut temp = path.as_os_str().to_owned();
    temp.push(".tmp");
    let temp = PathBuf::from(temp);
    let written = (|| {
        let file = std::fs::File::create(&temp)?;
        serde_json::to_writer(&file, ledger)?;
        std::fs::rename(&temp, path)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    written
}

/// While held, the agents this process starts are recorded in `data_dir`.
/// Held by the engine that owns that data dir.
#[must_use]
pub struct Attachment {
    #[cfg_attr(not(unix), allow(dead_code))]
    dir: PathBuf,
}

/// Record this process's agents in `data_dir` from now on (see the module
/// docs). Call it with the data dir's engine lock held.
pub fn attach(data_dir: &Path) -> Attachment {
    let dir = data_dir.to_path_buf();
    #[cfg(unix)]
    {
        let mut state = state();
        state.dirs.push(dir.clone());
        if !state.agents.is_empty() {
            write_ledgers(&mut state);
        }
    }
    Attachment { dir }
}

impl Drop for Attachment {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            let mut state = state();
            if let Some(index) = state.dirs.iter().position(|dir| *dir == self.dir) {
                state.dirs.remove(index);
            }
            // Its agents may outlive it (torn down after its engine); a
            // ledger still listing them stays for the next engine to check.
            state
                .agents
                .retain(|agent| super::still_same(agent.pid, Some(agent.start)));
            if state.agents.is_empty()
                && !state.dirs.contains(&self.dir)
                && let Some(engine) = this_engine()
            {
                let _ = std::fs::remove_file(self.dir.join(LEDGER_DIR).join(ledger_name(engine)));
            }
        }
    }
}

/// What the sweep made of one recorded agent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Provably the recorded agent, orphaned: ended.
    End,
    /// No longer running.
    Gone,
    /// Its pid now belongs to a process that started at another time.
    Reused,
    /// Its pid runs another image.
    OtherExecutable,
    /// Its engine still runs (or is still its parent): the engine's to end.
    NotOrphaned,
    /// A Claude login refresh is in flight: wait, never signal it now.
    LoginRefreshing,
}

/// A recorded pid as the process table describes it now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Observed {
    pub start: Option<u64>,
    pub exe: Option<PathBuf>,
    /// Its arguments (empty when they can't be read).
    pub argv: Vec<String>,
    pub ppid: Option<i32>,
}

/// The engine that recorded an agent, as things stand now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Recorder {
    pub pid: i32,
    /// Still running (same pid and start time).
    pub alive: bool,
}

/// Whether the sweep may end `record`, given the process now at its pid
/// (`None`: no live process), the engine that recorded it, and whether a
/// Claude login refresh is in flight in its config dir.
///
/// Orphaned means its engine is gone and it was adopted: by init (ppid 1)
/// or, on Linux, by a subreaper (`systemd --user`, a container's init),
/// whose pid can be anything. While its engine runs, the agent is that
/// engine's, whoever its parent.
pub fn judge(
    record: &Record,
    now: Option<&Observed>,
    engine: Recorder,
    login_refreshing: bool,
) -> Verdict {
    let Some(now) = now else {
        return Verdict::Gone;
    };
    if now.start != Some(record.start) {
        return Verdict::Reused;
    }
    if now.exe.as_deref() != Some(record.exe.as_path()) && !launched_as(&now.argv, &record.program)
    {
        return Verdict::OtherExecutable;
    }
    let orphaned = !engine.alive && now.ppid.is_some_and(|ppid| ppid == 1 || ppid != engine.pid);
    if !orphaned {
        return Verdict::NotOrphaned;
    }
    if login_refreshing {
        return Verdict::LoginRefreshing;
    }
    Verdict::End
}

/// Whether `argv` runs `program`: as itself, or as the script an
/// interpreter (`sh`, `env node`) runs.
fn launched_as(argv: &[String], program: &str) -> bool {
    if program.is_empty() {
        return false;
    }
    let name = |path: &str| {
        Path::new(path)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
    };
    argv.iter().take(3).any(|arg| {
        arg == program || (!program.contains('/') && name(arg).as_deref() == Some(program))
    })
}

#[cfg(unix)]
fn observe(pid: i32) -> Option<Observed> {
    let start = super::proc_table::start_time(pid)?;
    if super::proc_table::zombie(pid) {
        return None;
    }
    Some(Observed {
        start: Some(start),
        exe: super::proc_table::exe(pid),
        argv: super::proc_table::argv(pid).unwrap_or_default(),
        ppid: super::proc_table::ppid(pid),
    })
}

/// SIGTERM, then SIGKILL after this long.
#[cfg(unix)]
const GRACE: Duration = Duration::from_secs(5);
/// The longest the sweep waits out a login refresh before leaving the
/// agent for the next engine start.
#[cfg(unix)]
const LOGIN_WAIT: Duration = Duration::from_secs(300);

/// [`sweep`] on a thread of its own: the engine starts without waiting
/// for it.
pub fn sweep_in_background(data_dir: &Path) {
    #[cfg(unix)]
    {
        let dir = data_dir.to_path_buf();
        let started = std::thread::Builder::new()
            .name("zeron-agent-sweep".into())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        tracing::warn!(%error, "no runtime to sweep leftover agents");
                        return;
                    }
                };
                runtime.block_on(sweep(&dir));
            });
        if let Err(error) = started {
            tracing::warn!(%error, "could not start the leftover-agent sweep");
        }
    }
    #[cfg(not(unix))]
    let _ = data_dir;
}

/// End the agents a dead engine left running on `data_dir` (see the
/// module docs) and return what was decided for each, by pid. Ledgers of a
/// live engine, this one included, are left alone.
pub async fn sweep(data_dir: &Path) -> Vec<(i32, Verdict)> {
    #[cfg(unix)]
    {
        let folder = data_dir.join(LEDGER_DIR);
        let Ok(entries) = std::fs::read_dir(&folder) else {
            return Vec::new();
        };
        let ours = this_engine().map(ledger_name);
        let mut outcomes = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if ours
                .as_ref()
                .is_some_and(|ours| name.starts_with(ours.as_str()))
            {
                continue;
            }
            if name.ends_with(".json.tmp") {
                // A dead engine's half-written ledger: the one it replaced was swept.
                let _ = std::fs::remove_file(&path);
                continue;
            }
            if !name.ends_with(".json") {
                continue;
            }
            let ledger = match std::fs::read(&path)
                .map_err(|error| error.to_string())
                .and_then(|bytes| {
                    serde_json::from_slice::<Ledger>(&bytes).map_err(|error| error.to_string())
                }) {
                Ok(ledger) => ledger,
                Err(error) => {
                    tracing::warn!(path = %path.display(), %error, "unreadable agent ledger; removed");
                    let _ = std::fs::remove_file(&path);
                    continue;
                }
            };
            if super::still_same(ledger.engine.pid, Some(ledger.engine.start)) {
                // Its engine still runs (somehow, on this data dir): its agents are its own.
                continue;
            }
            let engine = ledger.engine;
            let verdicts =
                futures::future::join_all(ledger.agents.iter().map(|agent| end(agent, engine)))
                    .await;
            let left: Vec<Record> = ledger
                .agents
                .iter()
                .zip(&verdicts)
                .filter(|(_, verdict)| **verdict == Verdict::LoginRefreshing)
                .map(|(agent, _)| agent.clone())
                .collect();
            for (agent, verdict) in ledger.agents.iter().zip(&verdicts) {
                if *verdict == Verdict::End {
                    tracing::info!(pid = agent.pid, exe = %agent.exe.display(),
                        cwd = ?agent.cwd, engine = ledger.engine.pid,
                        "ended an agent its engine left running");
                }
                outcomes.push((agent.pid, *verdict));
            }
            let rest = Ledger {
                engine: ledger.engine,
                agents: left,
            };
            if let Err(error) = write_ledger(&path, &rest) {
                tracing::warn!(path = %path.display(), %error, "could not update a swept agent ledger");
            }
        }
        outcomes
    }
    #[cfg(not(unix))]
    {
        let _ = data_dir;
        Vec::new()
    }
}

/// End one recorded agent and everything it started, if [`judge`] allows:
/// SIGTERM, then SIGKILL after [`GRACE`]. A login refresh in flight is
/// waited out first (up to [`LOGIN_WAIT`]; then it is left for later).
#[cfg(unix)]
async fn end(record: &Record, engine: Engine) -> Verdict {
    let deadline = tokio::time::Instant::now() + LOGIN_WAIT;
    loop {
        let refreshing = crate::claude::refresh_lock::refresh_in_flight(&record.claude_config);
        let recorder = Recorder {
            pid: engine.pid,
            alive: super::still_same(engine.pid, Some(engine.start)),
        };
        match judge(record, observe(record.pid).as_ref(), recorder, refreshing) {
            Verdict::LoginRefreshing => {
                let left = deadline.saturating_duration_since(tokio::time::Instant::now());
                if left.is_zero() {
                    return Verdict::LoginRefreshing;
                }
                crate::claude::refresh_lock::settle(&record.claude_config, left).await;
            }
            Verdict::End => {
                // What it started, while it is still there to trace them
                // from (shells in groups and sessions of their own).
                let mut tree = vec![super::Tracked {
                    pid: record.pid,
                    start: Some(record.start),
                }];
                tree.extend(super::descendants(record.pid as u32).await);
                // A refresh may have begun meanwhile.
                if crate::claude::refresh_lock::refresh_in_flight(&record.claude_config) {
                    continue;
                }
                super::terminate_tree(&tree, GRACE).await;
                return Verdict::End;
            }
            other => return other,
        }
    }
}

/// Linux: agents get `PR_SET_PDEATHSIG`, forked from a thread that lives as
/// long as the process.
#[cfg(target_os = "linux")]
mod linux {
    use super::{Child, Command};
    use std::sync::mpsc;

    struct Job {
        runtime: Option<tokio::runtime::Handle>,
        command: Command,
        reply: mpsc::SyncSender<(Command, std::io::Result<Child>)>,
    }

    fn spawner() -> Option<&'static mpsc::Sender<Job>> {
        static SPAWNER: std::sync::OnceLock<Option<mpsc::Sender<Job>>> = std::sync::OnceLock::new();
        SPAWNER
            .get_or_init(|| {
                let (sender, jobs) = mpsc::channel::<Job>();
                std::thread::Builder::new()
                    .name("zeron-agent-spawner".into())
                    .spawn(move || {
                        // Never returns: the sender lives in a static.
                        for mut job in jobs {
                            let entered = job.runtime.as_ref().map(|runtime| runtime.enter());
                            let spawned = job.command.spawn();
                            drop(entered);
                            let _ = job.reply.send((job.command, spawned));
                        }
                    })
                    .map_err(|error| {
                        tracing::warn!(%error, "no agent spawner thread; agents won't die with the engine")
                    })
                    .ok()
                    .map(|_| sender)
            })
            .as_ref()
    }

    pub(super) fn spawn(command: &mut Command) -> std::io::Result<Child> {
        let Some(spawner) = spawner() else {
            return command.spawn();
        };
        let engine = std::process::id() as libc::pid_t;
        // SAFETY: prctl and getppid are async-signal-safe, and nothing here
        // allocates.
        unsafe {
            command.pre_exec(move || {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                // The engine died before the death signal was armed.
                if libc::getppid() != engine {
                    return Err(std::io::Error::from_raw_os_error(libc::ESRCH));
                }
                Ok(())
            });
        }
        let (reply, answer) = mpsc::sync_channel(1);
        let job = Job {
            runtime: tokio::runtime::Handle::try_current().ok(),
            command: std::mem::replace(command, Command::new("")),
            reply,
        };
        if let Err(mpsc::SendError(job)) = spawner.send(job) {
            *command = job.command;
            return command.spawn();
        }
        let (returned, spawned) = answer
            .recv()
            .map_err(|_| std::io::Error::other("the agent spawner thread is gone"))?;
        *command = returned;
        spawned
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record() -> Record {
        Record {
            pid: 4242,
            start: 1_700_000_000_000_000,
            exe: "/bin/bash".into(),
            program: "/home/me/.local/bin/claude".into(),
            pgid: 4242,
            cwd: Some("/tmp/worktree".into()),
            claude_config: "/tmp/home/.claude".into(),
        }
    }

    fn orphan() -> Observed {
        Observed {
            start: Some(record().start),
            exe: Some(record().exe),
            argv: vec!["/bin/sh".into(), record().program, "--print".into()],
            ppid: Some(1),
        }
    }

    /// The engine that recorded it, gone.
    const DEAD: Recorder = Recorder {
        pid: 900,
        alive: false,
    };

    #[test]
    fn only_the_recorded_orphan_is_ended() {
        assert_eq!(judge(&record(), Some(&orphan()), DEAD, false), Verdict::End);
    }

    #[test]
    fn an_exited_agent_is_left_alone() {
        assert_eq!(judge(&record(), None, DEAD, false), Verdict::Gone);
    }

    #[test]
    fn a_reused_pid_is_left_alone() {
        let now = Observed {
            start: Some(record().start + 1),
            ..orphan()
        };
        assert_eq!(judge(&record(), Some(&now), DEAD, false), Verdict::Reused);
        // A start time that can't be read proves nothing either.
        let unknown = Observed {
            start: None,
            ..orphan()
        };
        assert_eq!(
            judge(&record(), Some(&unknown), DEAD, false),
            Verdict::Reused
        );
    }

    #[test]
    fn another_executable_is_left_alone() {
        let now = Observed {
            exe: Some("/bin/zsh".into()),
            argv: vec!["zsh".into(), "-l".into()],
            ..orphan()
        };
        assert_eq!(
            judge(&record(), Some(&now), DEAD, false),
            Verdict::OtherExecutable
        );
        let unreadable = Observed {
            exe: None,
            argv: Vec::new(),
            ..orphan()
        };
        assert_eq!(
            judge(&record(), Some(&unreadable), DEAD, false),
            Verdict::OtherExecutable
        );
    }

    /// A script's interpreter changed image after launch (`env` became
    /// node, macOS `/bin/sh` became bash): the script it runs still names it.
    #[test]
    fn a_script_is_known_by_its_arguments() {
        let now = Observed {
            exe: Some("/opt/homebrew/bin/node".into()),
            argv: vec!["node".into(), record().program, "--print".into()],
            ..orphan()
        };
        assert_eq!(judge(&record(), Some(&now), DEAD, false), Verdict::End);
        // Not a mention further down its arguments.
        let mention = Observed {
            exe: Some("/usr/bin/grep".into()),
            argv: vec!["grep".into(), "-r".into(), "x".into(), record().program],
            ..orphan()
        };
        assert_eq!(
            judge(&record(), Some(&mention), DEAD, false),
            Verdict::OtherExecutable
        );
    }

    /// While its engine runs, an agent is that engine's: whoever its parent
    /// is, init included.
    #[test]
    fn an_agent_of_a_live_engine_is_left_alone() {
        let alive = Recorder {
            pid: 900,
            alive: true,
        };
        let child = Observed {
            ppid: Some(900),
            ..orphan()
        };
        assert_eq!(
            judge(&record(), Some(&child), alive, false),
            Verdict::NotOrphaned
        );
        assert_eq!(
            judge(&record(), Some(&orphan()), alive, false),
            Verdict::NotOrphaned
        );
        // Its parent unknown: not provably orphaned.
        let unknown = Observed {
            ppid: None,
            ..orphan()
        };
        assert_eq!(
            judge(&record(), Some(&unknown), DEAD, false),
            Verdict::NotOrphaned
        );
    }

    /// Adopted by a subreaper rather than init (Linux `systemd --user`): its
    /// ppid isn't 1, but its engine is gone.
    #[test]
    fn an_agent_adopted_by_a_subreaper_is_still_an_orphan() {
        let adopted = Observed {
            ppid: Some(1234),
            ..orphan()
        };
        assert_eq!(judge(&record(), Some(&adopted), DEAD, false), Verdict::End);
    }

    #[test]
    fn a_login_refresh_in_flight_holds_the_signal() {
        assert_eq!(
            judge(&record(), Some(&orphan()), DEAD, true),
            Verdict::LoginRefreshing
        );
        // Never a reason to end something that isn't ours.
        let reused = Observed {
            start: Some(1),
            ..orphan()
        };
        assert_eq!(judge(&record(), Some(&reused), DEAD, true), Verdict::Reused);
    }

    #[cfg(unix)]
    #[test]
    fn the_claude_config_follows_the_commands_own_environment() {
        let mut command = Command::new("claude");
        command.env("CLAUDE_CONFIG_DIR", "/tmp/cfg");
        assert_eq!(claude_config_of(&command), PathBuf::from("/tmp/cfg"));
        command.env("CLAUDE_SECURESTORAGE_CONFIG_DIR", "/tmp/secure");
        assert_eq!(claude_config_of(&command), PathBuf::from("/tmp/secure"));
    }

    /// The death signal is armed on Linux: an agent of a process that is
    /// SIGKILLed gets SIGTERM from the kernel, even one forked while the
    /// caller ran on a thread that has since exited.
    #[cfg(target_os = "linux")]
    #[test]
    fn linux_agents_get_the_engines_death_signal() {
        if std::env::var_os("ZERON_PDEATHSIG_CHILD").is_some() {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async {
                let child = tokio::task::spawn_blocking(|| {
                    let mut command = Command::new("sleep");
                    command.arg("300");
                    spawn(&mut command).unwrap()
                })
                .await
                .unwrap();
                println!("agent {}", child.id().unwrap());
                std::mem::forget(child);
                // Let the blocking thread retire: the agent must survive it.
                tokio::time::sleep(Duration::from_secs(600)).await;
            });
            return;
        }
        let mut engine = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "process::registry::tests::linux_agents_get_the_engines_death_signal",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("ZERON_PDEATHSIG_CHILD", "1")
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let mut out = std::io::BufReader::new(engine.stdout.take().unwrap());
        let agent = loop {
            let mut line = String::new();
            assert!(
                std::io::BufRead::read_line(&mut out, &mut line).unwrap() > 0,
                "engine exited"
            );
            // (After the harness's own "test … " on the same line.)
            if let Some((_, pid)) = line.trim().rsplit_once("agent ") {
                break pid.parse::<i32>().unwrap();
            }
        };
        // Past tokio's 10 s blocking-thread keep-alive: the thread that
        // asked for the spawn is gone, the agent is not.
        std::thread::sleep(Duration::from_secs(12));
        assert!(
            observe(agent).is_some(),
            "agent died with its calling thread"
        );
        engine.kill().unwrap();
        engine.wait().unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while observe(agent).is_some() {
            if std::time::Instant::now() > deadline {
                // SAFETY: the test's own sleep.
                unsafe { libc::kill(agent, libc::SIGKILL) };
                panic!("agent outlived its engine");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}
