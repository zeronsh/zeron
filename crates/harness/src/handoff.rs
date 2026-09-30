//! The seam a harness run uses to survive a live update of the engine.
//!
//! The engine replaces itself with `execve`, keeping its PID, so agent
//! children stay children of the same process and only file descriptors
//! survive. To carry a running turn across that exec a harness must be able to
//! (1) stop reading the child's stdout at a line boundary without losing
//! bytes ([`crate::line_reader::LineReader`]), (2) finish queued stdin lines
//! without tearing one and then pause the writer ([`run_writer`]), (3) export its
//! small protocol state ([`HarnessHandoff`]), and (4) later adopt an existing
//! child from that state ([`ChildHandle::Adopted`], `Harness::adopt`).
//! See `docs/live-update.md`, "Agent runs".
//!
//! A freeze SUSPENDS a run; it does not end it. The engine `execve`s while
//! holding the frozen run, and exec runs no destructors, so the old image's
//! tasks, pipes and state simply stop existing with it. If the exec fails the
//! engine drops the [`FrozenRun`] and the run resumes exactly where it was.
//! Everything below is built for that: the reader keeps its buffer
//! ([`crate::line_reader::LineReader::leftover`]), the writer keeps its queue
//! ([`WriteMsg::Pause`]) and an adopted child's exit is watched by a single
//! waiter that a [`ReapGate`] can hold back.
//!
//! Nothing here changes behaviour until a [`FreezeRequest`] is sent: the
//! default `RunControls.freeze` never yields one. NOTE for run loops: with no
//! sender, `freeze.recv()` returns `None` immediately and forever — guard the
//! `select!` arm (e.g. `if freeze_open`, cleared on `None`) as the steering arm
//! already is, or it spins.

use std::io;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot};
use zeron_proto::HarnessId;

/// The state versions each adoptable harness can READ back, by harness id.
/// A new binary reports this in its handoff preflight so the running engine can
/// veto (never kill) a run whose exported state the successor could not adopt.
/// Add a version here, never remove one, when a harness's state schema changes.
pub fn adoptable_state_versions() -> Vec<(HarnessId, Vec<u32>)> {
    #[cfg(unix)]
    {
        let acp = vec![crate::acp::STATE_VERSION];
        vec![
            (HarnessId::ClaudeCode, vec![crate::claude::STATE_VERSION]),
            (HarnessId::Codex, vec![crate::codex::STATE_VERSION]),
            (HarnessId::Cursor, vec![crate::cursor::STATE_VERSION]),
            (HarnessId::Opencode, vec![crate::opencode::STATE_VERSION]),
            (HarnessId::Grok, acp.clone()),
            (HarnessId::Devin, acp.clone()),
            (HarnessId::Hermes, acp.clone()),
            (HarnessId::Antigravity, acp),
        ]
    }
    #[cfg(not(unix))]
    {
        Vec::new()
    }
}

/// Ask a live run to stop at a safe point and hand over its child.
///
/// The run answers through `reply`: [`FreezeRefusal`] when it is not at a safe
/// point (or cannot hand off at all), else a [`FrozenRun`]. Until the engine's
/// verdict arrives the run is suspended, not finished. After a COMMIT (only a
/// same-process successor does that; the exec path never commits) the harness
/// drains its event channel and ends its stream, and the engine treats
/// stream-end-after-commit as "frozen". There is deliberately no sentinel
/// `AgentEvent`.
#[derive(Debug)]
pub struct FreezeRequest {
    pub reply: oneshot::Sender<Result<FrozenRun, FreezeRefusal>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FreezeRefusal {
    /// Not at a safe point right now (why, for logs); the engine may retry.
    Busy(&'static str),
    /// This run can never be handed off; the engine falls back for it.
    Unsupported,
}

/// A run stopped at a safe point, waiting for the engine's verdict.
#[derive(Debug)]
pub struct FrozenRun {
    pub handoff: HarnessHandoff,
    /// Sending commits the freeze: the harness stops for good and its stream
    /// ends. Dropping it without sending thaws the run, which resumes as if
    /// nothing happened.
    pub commit: oneshot::Sender<()>,
}

/// Everything needed to adopt a running child in the next process image.
///
/// File descriptors are raw numbers valid in the next image because the
/// engine clears close-on-exec on them just before the exec.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct HarnessHandoff {
    pub harness: HarnessId,
    /// Version of `state`'s schema, owned by the harness; a harness refuses
    /// (and the engine falls back) for a version it cannot read.
    pub state_version: u32,
    pub pid: i32,
    pub stdin_fd: i32,
    pub stdout_fd: i32,
    pub stderr_fd: Option<i32>,
    /// Other descriptors this harness keeps across the exec (Cursor's store
    /// lease: an open, flock-ed file). The engine makes them inheritable next to
    /// the pipes and validates them as pipes or regular files.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extra_fds: Vec<i32>,
    /// Stdout bytes already read off the pipe but not yet consumed as lines
    /// (see [`crate::line_reader::LineReader::leftover`]). Base64 on the wire:
    /// a JSON array of numbers would be several times larger.
    #[serde(with = "base64_bytes")]
    pub stdout_leftover: Vec<u8>,
    /// Recent stderr lines, kept for diagnostics if the child later fails.
    pub stderr_tail: Vec<String>,
    /// The harness's serialized protocol state.
    pub state: serde_json::Value,
    /// Steers still in the run's mailbox when it froze, oldest first. The run
    /// drains them at the freeze (the engine cannot read a mailbox it only
    /// sends into) and keeps them: a thawed run reads them before its mailbox,
    /// and on adoption the engine re-queues them into the new run's mailbox.
    #[serde(default)]
    pub undrained_steers: Vec<SteerRecord>,
}

impl HarnessHandoff {
    /// The `stdin_fd` / `stdout_fd` of a child that has no such pipe: an
    /// OpenCode server runs with stdin and stdout on `/dev/null` and is
    /// reached over loopback HTTP. The engine carries, validates and
    /// duplicates only real descriptors (see [`Self::fds`]).
    pub const NO_PIPE: i32 = -1;

    /// Every descriptor this handoff carries across the exec — stdin, stdout,
    /// stderr, then [`Self::extra_fds`] — without the [`Self::NO_PIPE`]
    /// placeholders.
    pub fn fds(&self) -> Vec<i32> {
        [self.stdin_fd, self.stdout_fd]
            .into_iter()
            .filter(|fd| *fd != Self::NO_PIPE)
            .chain(self.stderr_fd)
            .chain(self.extra_fds.iter().copied())
            .collect()
    }
}

// `state` will carry secrets (an OpenCode server's password), so `Debug` — which
// ends up in logs and panic messages — shows sizes, not contents.
impl std::fmt::Debug for HarnessHandoff {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HarnessHandoff")
            .field("harness", &self.harness)
            .field("state_version", &self.state_version)
            .field("pid", &self.pid)
            .field("stdin_fd", &self.stdin_fd)
            .field("stdout_fd", &self.stdout_fd)
            .field("stderr_fd", &self.stderr_fd)
            .field("stdout_leftover_bytes", &self.stdout_leftover.len())
            .field("state", &"<redacted>")
            .field("undrained_steers", &self.undrained_steers.len())
            .finish()
    }
}

mod base64_bytes {
    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&STANDARD.encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        let text = String::deserialize(deserializer)?;
        STANDARD.decode(text).map_err(serde::de::Error::custom)
    }
}

/// A steer prompt as carried across a handoff (see [`crate::SteerMessage`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SteerRecord {
    pub prompt: String,
    pub message_id: Option<String>,
}

impl From<crate::SteerMessage> for SteerRecord {
    fn from(message: crate::SteerMessage) -> Self {
        Self {
            prompt: message.prompt,
            message_id: message.message_id,
        }
    }
}

impl From<SteerRecord> for crate::SteerMessage {
    fn from(record: SteerRecord) -> Self {
        Self {
            prompt: record.prompt,
            message_id: record.message_id,
        }
    }
}

/// Take every steer waiting in a run's mailbox without blocking, oldest first
/// (a freeze exports them as [`HarnessHandoff::undrained_steers`]).
pub fn drain_steering(steering: &mut mpsc::Receiver<crate::SteerMessage>) -> Vec<SteerRecord> {
    let mut drained = Vec::new();
    while let Ok(message) = steering.try_recv() {
        drained.push(message.into());
    }
    drained
}

/// How a child ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExitOutcome {
    status: Option<std::process::ExitStatus>,
    not_our_child: bool,
}

impl ExitOutcome {
    /// The pid was not our child (already reaped elsewhere, or never ours), so
    /// the real exit status is unknown. `code()` is `None`.
    #[cfg(unix)]
    fn unknown() -> Self {
        Self {
            status: None,
            not_our_child: true,
        }
    }

    /// The exit code, or `None` if it was killed by a signal or its status is
    /// unknown ([`Self::not_our_child`]).
    pub fn code(&self) -> Option<i32> {
        self.status.and_then(|s| s.code())
    }

    /// True only for a known, successful exit.
    pub fn success(&self) -> bool {
        self.status.is_some_and(|s| s.success())
    }

    /// Terminating signal, when the child was killed by one.
    #[cfg(unix)]
    pub fn signal(&self) -> Option<i32> {
        use std::os::unix::process::ExitStatusExt;
        self.status.and_then(|s| s.signal())
    }

    /// The status could not be observed because the pid is not our child.
    pub fn not_our_child(&self) -> bool {
        self.not_our_child
    }

    /// The raw status, when known.
    pub fn status(&self) -> Option<std::process::ExitStatus> {
        self.status
    }
}

impl From<std::process::ExitStatus> for ExitOutcome {
    fn from(status: std::process::ExitStatus) -> Self {
        Self {
            status: Some(status),
            not_our_child: false,
        }
    }
}

/// Who may reap an adopted child's exit status: its waiter, except while the
/// run is frozen for a handoff.
///
/// A child that exits while its run is frozen must stay a zombie so the next
/// image (same parent PID after an `execve`) reaps it and reports the real
/// status; a waiter that reaped it first would take the status with it at the
/// exec. The waiter blocks in [`ReapGate::begin_reap`] while the gate is held.
#[cfg(unix)]
pub struct ReapGate {
    state: std::sync::Mutex<GateState>,
    released: std::sync::Condvar,
}

#[cfg(unix)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum GateState {
    Open,
    Held,
    /// The waiter has passed the gate and is reaping (or has reaped).
    Reaped,
}

#[cfg(unix)]
impl ReapGate {
    pub fn new() -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self {
            state: std::sync::Mutex::new(GateState::Open),
            released: std::sync::Condvar::new(),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, GateState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Stop the waiter from reaping. `false` means it already passed the gate:
    /// the child is gone and its status is (being) consumed here.
    pub fn hold(&self) -> bool {
        let mut state = self.lock();
        match *state {
            GateState::Reaped => false,
            _ => {
                *state = GateState::Held;
                true
            }
        }
    }

    /// Let the waiter reap again (a thawed run).
    pub fn release(&self) {
        let mut state = self.lock();
        if *state == GateState::Held {
            *state = GateState::Open;
        }
        self.released.notify_all();
    }

    /// For the waiter: block while the gate is held, then claim the reap.
    pub fn begin_reap(&self) {
        let mut state = self.lock();
        while *state == GateState::Held {
            state = self
                .released
                .wait(state)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        *state = GateState::Reaped;
    }
}

/// An adopted child: still OUR child (`execve` keeps the PID), watched by ONE
/// waiter thread shared by every `wait()`/`try_wait()` call.
///
/// `wait()` is called in loops and `select!` arms and its future is dropped at
/// will, so the wait itself must not belong to the call: a per-call waiter
/// would pile up threads, and an abandoned one could reap the child and take
/// the exit status from the caller that is still waiting.
#[cfg(unix)]
pub struct AdoptedChild {
    pid: i32,
    shared: std::sync::Arc<AdoptedShared>,
}

#[cfg(unix)]
struct AdoptedShared {
    gate: std::sync::Arc<ReapGate>,
    exit: tokio::sync::watch::Sender<Option<ExitOutcome>>,
    started: std::sync::atomic::AtomicBool,
}

#[cfg(unix)]
impl AdoptedChild {
    fn new(pid: i32) -> Self {
        // A pid that is not our child (already reaped, or never ours: a stale
        // manifest, a reused number) is settled NOW, synchronously, so nothing
        // is ever signalled or waited for on someone else's process. The probe
        // does not reap, so it is safe next to a held gate.
        let ours = probe_is_our_child(pid);
        Self {
            pid,
            shared: std::sync::Arc::new(AdoptedShared {
                gate: ReapGate::new(),
                exit: tokio::sync::watch::channel((!ours).then(ExitOutcome::unknown)).0,
                started: std::sync::atomic::AtomicBool::new(!ours),
            }),
        }
    }

    /// Start the one waiter, once. A dedicated thread, not the blocking pool: it
    /// lives as long as the child does.
    fn ensure_waiter(&self) {
        use std::sync::atomic::Ordering;
        if self.shared.started.swap(true, Ordering::SeqCst) {
            return;
        }
        let shared = self.shared.clone();
        let pid = self.pid;
        let spawned = std::thread::Builder::new()
            .name(format!("reap-{pid}"))
            .spawn(move || {
                let outcome = wait_pid_gated(pid, &shared.gate).unwrap_or_else(|error| {
                    tracing::warn!(pid, %error, "waiting for an adopted child failed");
                    ExitOutcome::unknown()
                });
                shared.exit.send_replace(Some(outcome));
            });
        if let Err(error) = spawned {
            tracing::error!(pid, %error, "could not start the adopted child's waiter");
            self.shared.exit.send_replace(Some(ExitOutcome::unknown()));
        }
    }

    fn reaped(&self) -> Option<ExitOutcome> {
        *self.shared.exit.borrow()
    }
}

/// Is `pid` a child of this process (running, or exited and not yet reaped)?
/// `waitid(WNOHANG | WNOWAIT)` answers without consuming a status.
#[cfg(unix)]
fn probe_is_our_child(pid: i32) -> bool {
    loop {
        // SAFETY: zeroed siginfo_t is a valid out-parameter for waitid.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        // SAFETY: waitid writes only into `info`.
        let rc = unsafe {
            libc::waitid(
                libc::P_PID,
                pid as libc::id_t,
                &mut info,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            )
        };
        if rc == 0 {
            return true;
        }
        match io::Error::last_os_error().raw_os_error() {
            Some(libc::EINTR) => continue,
            Some(libc::ECHILD) => return false,
            // Anything else is unexpected; keep trusting the manifest.
            _ => return true,
        }
    }
}

/// An agent child this image spawned, or one adopted across an exec.
///
/// Spawn every harness child with `kill_on_drop(false)`: [`Self::release`] must
/// never let a `Child` that would kill on drop go out of scope.
pub enum ChildHandle {
    Owned(crate::process::Child),
    #[cfg(unix)]
    Adopted(AdoptedChild),
}

impl ChildHandle {
    /// Adopt a child of this process by pid (from a [`HarnessHandoff`]). The
    /// pid comes off the wire, so anything that is not a plain pid is refused:
    /// 0 would signal our whole process group, -1 every process we may signal.
    #[cfg(unix)]
    pub fn adopt(pid: i32) -> io::Result<Self> {
        if pid <= 1 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("refusing to adopt process id {pid}"),
            ));
        }
        Ok(Self::Adopted(AdoptedChild::new(pid)))
    }

    /// The pid, while the child has not been reaped.
    pub fn id(&self) -> Option<u32> {
        match self {
            Self::Owned(child) => child.id(),
            #[cfg(unix)]
            Self::Adopted(child) => child.reaped().is_none().then_some(child.pid as u32),
        }
    }

    /// Wait for exit. Owned: `Child::wait`. Adopted: waits for the shared
    /// waiter, so it is cancel-safe and can be called again and again. If the
    /// pid is not our child the outcome says so instead of hanging.
    pub async fn wait(&mut self) -> io::Result<ExitOutcome> {
        match self {
            Self::Owned(child) => child.wait().await.map(ExitOutcome::from),
            #[cfg(unix)]
            Self::Adopted(child) => {
                child.ensure_waiter();
                let mut exit = child.shared.exit.subscribe();
                let outcome = exit
                    .wait_for(|outcome| outcome.is_some())
                    .await
                    .map_err(io::Error::other)?;
                Ok((*outcome).expect("waited for Some"))
            }
        }
    }

    pub fn try_wait(&mut self) -> io::Result<Option<ExitOutcome>> {
        match self {
            Self::Owned(child) => Ok(child.try_wait()?.map(ExitOutcome::from)),
            #[cfg(unix)]
            Self::Adopted(child) => {
                child.ensure_waiter();
                Ok(child.reaped())
            }
        }
    }

    /// Send SIGKILL (Owned: tokio's `start_kill`). A child that is already
    /// gone — or already reaped, when its pid may belong to someone else — is
    /// not an error and is not signalled.
    pub fn start_kill(&mut self) -> io::Result<()> {
        match self {
            Self::Owned(child) => child.start_kill(),
            #[cfg(unix)]
            Self::Adopted(child) => {
                if child.reaped().is_some() {
                    return Ok(());
                }
                // SAFETY: plain kill(2) on a validated pid; no memory involved.
                if unsafe { libc::kill(child.pid, libc::SIGKILL) } == 0 {
                    return Ok(());
                }
                let err = io::Error::last_os_error();
                if err.raw_os_error() == Some(libc::ESRCH) {
                    Ok(())
                } else {
                    Err(err)
                }
            }
        }
    }

    /// Keep this handle from reaping the child while its run is frozen (see
    /// [`ReapGate`]); `false` means the child is already gone. Owned children
    /// are reaped by tokio's signal-driven reaper, which cannot be held: one
    /// that exits inside the freeze window loses its status, and the successor
    /// finds "not our child" and falls back to crash recovery.
    #[cfg(unix)]
    pub fn hold_reaping(&self) -> bool {
        match self {
            Self::Owned(child) => child.id().is_some(),
            Self::Adopted(child) => child.shared.gate.hold(),
        }
    }

    /// Undo [`Self::hold_reaping`] (a thawed run).
    #[cfg(unix)]
    pub fn release_reaping(&self) {
        if let Self::Adopted(child) = self {
            child.shared.gate.release();
        }
    }

    /// What an interrupt escalation signals: the child's private process
    /// group when it leads one, else the child. `None` once it is reaped (its
    /// pid may be someone else's by then).
    #[cfg(unix)]
    pub(crate) fn signal_target(&self) -> Option<i32> {
        match self {
            Self::Owned(child) => crate::process::signal_target(child),
            Self::Adopted(_) => {
                let pid = self.id()? as i32;
                // SAFETY: getpgid only inspects our own, unreaped child.
                Some(if unsafe { libc::getpgid(pid) } == pid {
                    -pid
                } else {
                    pid
                })
            }
        }
    }

    #[cfg(windows)]
    pub(crate) fn signal_target(&self) -> Option<std::sync::Arc<crate::windows_process::Job>> {
        match self {
            Self::Owned(child) => crate::process::signal_target(child),
        }
    }

    /// Stop and reap the child, as [`crate::shutdown_child`] does for an
    /// owned one: SIGTERM, then SIGKILL after `kill_grace`.
    pub(crate) async fn shutdown(&mut self, kill_grace: std::time::Duration) {
        match self {
            Self::Owned(child) => crate::shutdown_child(child, kill_grace).await,
            #[cfg(unix)]
            Self::Adopted(_) => {
                use crate::{Signal, send_signal};
                let target = self.signal_target();
                if matches!(self.try_wait(), Ok(Some(_))) {
                    if let Some(group) = target.filter(|pid| *pid < 0) {
                        send_signal(&group, Signal::Kill);
                    }
                    return;
                }
                if let Some(pid) = target {
                    send_signal(&pid, Signal::Term);
                    if tokio::time::timeout(kill_grace, self.wait()).await.is_ok() {
                        if pid < 0 {
                            send_signal(&pid, Signal::Kill);
                        }
                        return;
                    }
                    send_signal(&pid, Signal::Kill);
                }
                let _ = self.start_kill();
                let _ = self.wait().await;
            }
        }
    }

    /// Give up ownership without killing or reaping, returning the pid for a
    /// hand-over. `None` when the child has already been reaped (nothing to
    /// hand over). ONLY for a same-process successor (tests standing in for an
    /// exec): the exec path never releases anything, and a run that may still
    /// thaw must not call this (the forgotten tokio `Child` leaks its reactor
    /// registration).
    ///
    /// The owned `Child` is forgotten, not dropped: tokio would otherwise queue
    /// a not-yet-reaped child on its orphan reaper, which reaps it and steals
    /// the exit status the next image must observe. Take `stdin`/`stdout`/
    /// `stderr` out of the `Child` first; forgetting it leaks any left inside.
    #[cfg(unix)]
    pub fn release(self) -> Option<i32> {
        match self {
            Self::Owned(child) => {
                let pid = child.id().map(|pid| pid as i32);
                std::mem::forget(child);
                pid
            }
            // Hold the gate so a waiter that already runs cannot reap the
            // child (and its status) out from under the successor.
            Self::Adopted(child) => {
                (child.reaped().is_none() && child.shared.gate.hold()).then_some(child.pid)
            }
        }
    }
}

/// Block until `pid` exits WITHOUT reaping, wait out any hold on `gate`, then
/// reap it. The two steps mean a waiter that goes away between them — an exec
/// mid-freeze — never takes the status with it.
#[cfg(unix)]
fn wait_pid_gated(pid: i32, gate: &ReapGate) -> io::Result<ExitOutcome> {
    loop {
        // SAFETY: zeroed siginfo_t is a valid out-parameter for waitid.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        // SAFETY: waitid writes only into `info`.
        let rc = unsafe {
            libc::waitid(
                libc::P_PID,
                pid as libc::id_t,
                &mut info,
                libc::WEXITED | libc::WNOWAIT,
            )
        };
        if rc == 0 {
            break;
        }
        let err = io::Error::last_os_error();
        match err.raw_os_error() {
            Some(libc::EINTR) => continue,
            Some(libc::ECHILD) => return Ok(ExitOutcome::unknown()),
            _ => return Err(err),
        }
    }
    gate.begin_reap();
    reap(pid).map(|outcome| outcome.unwrap_or_else(ExitOutcome::unknown))
}

/// `waitpid(pid, 0)`; ECHILD becomes a "not our child" outcome.
#[cfg(unix)]
fn reap(pid: i32) -> io::Result<Option<ExitOutcome>> {
    use std::os::unix::process::ExitStatusExt;
    loop {
        let mut status: libc::c_int = 0;
        // SAFETY: waitpid writes only into `status`.
        let rc = unsafe { libc::waitpid(pid, &mut status, 0) };
        if rc == pid {
            return Ok(Some(ExitOutcome::from(std::process::ExitStatus::from_raw(
                status,
            ))));
        }
        let err = io::Error::last_os_error();
        match err.raw_os_error() {
            Some(libc::EINTR) => continue,
            Some(libc::ECHILD) => return Ok(Some(ExitOutcome::unknown())),
            _ => return Err(err),
        }
    }
}

/// A close-on-exec duplicate of a descriptor inherited across an exec,
/// refusing anything that is not open or is a standard stream. Adoption works
/// on duplicates: the inherited originals stay untouched for a rollback and
/// the engine closes them when the adoption commits.
#[cfg(unix)]
pub(crate) fn dup_inherited(fd: i32) -> Result<std::os::fd::OwnedFd, crate::HarnessError> {
    use std::os::fd::FromRawFd;
    // SAFETY: fcntl on a plain descriptor number; no memory is involved.
    if fd < 3 || unsafe { libc::fcntl(fd, libc::F_GETFD) } < 0 {
        return Err(crate::HarnessError::Protocol(format!(
            "inherited descriptor {fd} is not usable"
        )));
    }
    // SAFETY: as above; the result is a fresh descriptor we own.
    let dup = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 3) };
    if dup < 0 {
        return Err(io::Error::last_os_error().into());
    }
    // SAFETY: `dup` was just created and is owned by nobody else.
    Ok(unsafe { std::os::fd::OwnedFd::from_raw_fd(dup) })
}

/// Best effort: give a child's output pipe 1 MiB so a chatty agent does not
/// block on it while nobody reads (a freeze, the exec gap). Linux only; an
/// unprivileged process may be capped lower (`/proc/sys/fs/pipe-max-size`).
#[cfg(unix)]
pub(crate) fn grow_pipe(fd: i32) {
    #[cfg(target_os = "linux")]
    // SAFETY: fcntl on a descriptor we own; no memory is involved.
    unsafe {
        libc::fcntl(fd, libc::F_SETPIPE_SZ, 1 << 20);
    }
    #[cfg(not(target_os = "linux"))]
    let _ = fd;
}

/// The descriptor behind a child's output pipe, for the hand-over manifest.
pub(crate) trait PipeFd {
    /// `None` where there is no descriptor to hand over (Windows).
    fn pipe_fd(&self) -> Option<i32>;
    /// Give the descriptor up WITHOUT closing it (a same-process successor
    /// owns it now). Where there is nothing to hand over this just drops.
    fn leak_pipe(self);
}

#[cfg(unix)]
macro_rules! unix_pipe_fd {
    ($($ty:ty),*) => {$(
        impl PipeFd for $ty {
            fn pipe_fd(&self) -> Option<i32> {
                use std::os::fd::AsRawFd;
                Some(self.as_raw_fd())
            }
            fn leak_pipe(self) {
                use std::os::fd::IntoRawFd;
                if let Ok(fd) = self.into_owned_fd() {
                    let _ = fd.into_raw_fd();
                }
            }
        }
    )*};
}
#[cfg(unix)]
unix_pipe_fd!(tokio::process::ChildStdout, tokio::process::ChildStderr);

#[cfg(windows)]
impl PipeFd for tokio::fs::File {
    fn pipe_fd(&self) -> Option<i32> {
        None
    }
    fn leak_pipe(self) {}
}

/// Drains a child's stderr into its [`crate::StderrTail`] for crash messages.
///
/// It keeps draining while the run is frozen, so the agent never blocks on a
/// full stderr pipe, and [`Self::abandon`] lets go of the pipe without
/// closing it for a same-process successor. Dropping the handle changes
/// nothing: the drain runs to EOF, then marks the tail closed.
pub(crate) struct StderrDrain {
    fd: Option<i32>,
    stop: Option<oneshot::Sender<oneshot::Sender<()>>>,
}

impl StderrDrain {
    pub(crate) fn spawn<R>(stderr: R, tail: crate::StderrTail, harness: &'static str) -> Self
    where
        R: tokio::io::AsyncRead + PipeFd + Unpin + Send + 'static,
    {
        Self::spawn_observed(stderr, tail, harness, |_| {})
    }

    /// [`Self::spawn`], also showing every line to `observe` (ACP watches
    /// stderr for a sign-in prompt).
    pub(crate) fn spawn_observed<R>(
        stderr: R,
        tail: crate::StderrTail,
        harness: &'static str,
        observe: impl Fn(&str) + Send + 'static,
    ) -> Self
    where
        R: tokio::io::AsyncRead + PipeFd + Unpin + Send + 'static,
    {
        let fd = stderr.pipe_fd();
        let (stop, mut stopped) = oneshot::channel::<oneshot::Sender<()>>();
        tokio::spawn(async move {
            let mut lines = crate::line_reader::LineReader::new(stderr);
            let mut stoppable = true;
            loop {
                tokio::select! {
                    ack = &mut stopped, if stoppable => match ack {
                        Ok(ack) => {
                            lines.into_parts().0.leak_pipe();
                            let _ = ack.send(());
                            return;
                        }
                        // The handle went away: drain to EOF as usual.
                        Err(_) => stoppable = false,
                    },
                    line = lines.next_line() => match line {
                        Ok(Some(line)) => {
                            tracing::debug!(target: "zeron_harness::stderr", harness, "stderr: {line}");
                            tail.push(&line);
                            observe(&line);
                        }
                        Ok(None) | Err(_) => {
                            tail.close();
                            // The descriptor may be named in a handoff manifest
                            // (the child can exit inside the freeze window): keep
                            // it open until the owner lets go, or a reused number
                            // would be inherited as "stderr".
                            if stoppable && let Ok(ack) = stopped.await {
                                lines.into_parts().0.leak_pipe();
                                let _ = ack.send(());
                            }
                            return;
                        }
                    },
                }
            }
        });
        Self {
            fd,
            stop: Some(stop),
        }
    }

    /// The stderr descriptor number, for the manifest.
    pub(crate) fn fd(&self) -> Option<i32> {
        self.fd
    }

    /// A same-process successor took the pipe: stop draining WITHOUT closing
    /// it. Returns once the drain has let go. The exec path never calls this.
    pub(crate) async fn abandon(mut self) {
        let (ack_tx, ack_rx) = oneshot::channel();
        if let Some(stop) = self.stop.take()
            && stop.send(ack_tx).is_ok()
        {
            let _ = ack_rx.await;
        }
    }
}

/// The child stdin handle a run's writer owns.
pub type Stdin = crate::process::ChildStdin;

/// The descriptor number behind a writer, for the hand-over manifest.
pub trait WriterFd {
    /// `None` where there is no descriptor to hand over (tests, Windows).
    fn writer_fd(&self) -> Option<i32>;

    /// Give the descriptor up WITHOUT closing it (a same-process successor
    /// owns it now). Where there is nothing to hand over this just drops.
    fn leak_fd(self)
    where
        Self: Sized,
    {
    }
}

impl WriterFd for Stdin {
    fn writer_fd(&self) -> Option<i32> {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            Some(self.as_raw_fd())
        }
        #[cfg(not(unix))]
        {
            None
        }
    }

    fn leak_fd(self) {
        #[cfg(unix)]
        if let Ok(fd) = self.into_owned_fd() {
            use std::os::fd::IntoRawFd;
            let _ = fd.into_raw_fd();
        }
    }
}

/// Messages for the task that owns a child's stdin.
pub enum WriteMsg {
    /// One protocol line, written without its terminator.
    Line(String),
    /// Close stdin (end of steering input).
    Close,
    /// Finish every line queued so far, then PAUSE: reply with a
    /// [`PausedWriter`] and stop writing until it is dropped. The writer keeps
    /// stdin and its queue — lines sent while paused are written, in order,
    /// after the resume; none is lost — so a thawed run needs no new writer and
    /// every existing sender keeps working.
    Pause(oneshot::Sender<PausedWriter>),
}

/// A writer paused for a handoff. Dropping it resumes the writer (a thaw); the
/// exec path holds it until the image is replaced.
#[derive(Debug)]
pub struct PausedWriter {
    /// The stdin descriptor number, for the manifest (`None` if it has none).
    pub stdin_fd: Option<i32>,
    resume: oneshot::Sender<Resume>,
}

impl PausedWriter {
    /// A same-process successor took over the pipe: end the writer WITHOUT
    /// closing the descriptor (closing it would break the successor's
    /// ownership: `OwnedFd` aborts on a double close). The exec path never
    /// calls this.
    ///
    /// Returns once the writer has let go of the descriptor, so the caller may
    /// close or reuse the number.
    pub async fn abandon(self) {
        let (ack_tx, ack_rx) = oneshot::channel();
        if self.resume.send(Resume::Abandon(ack_tx)).is_ok() {
            let _ = ack_rx.await;
        }
    }
}

/// What a paused writer was told (a dropped guard, a thaw, sends nothing).
#[derive(Debug)]
enum Resume {
    Abandon(oneshot::Sender<()>),
}

/// Owns the child's stdin and writes queued lines to it.
///
/// Each [`WriteMsg::Line`] is written as ONE buffer (`line + "\n"`, a single
/// `write_all`), so our own code never leaves half a line in the pipe. A write
/// failure (EPIPE after the child died) is tolerated, logged and ends the task,
/// dropping the receiver so later sends fail. [`WriteMsg::Pause`] is handled
/// strictly after the lines ahead of it in the queue.
pub async fn run_writer<W>(
    mut stdin: W,
    mut rx: mpsc::UnboundedReceiver<WriteMsg>,
    target: &'static str,
) where
    W: AsyncWrite + WriterFd + Unpin,
{
    while let Some(msg) = rx.recv().await {
        match msg {
            WriteMsg::Line(mut line) => {
                line.push('\n');
                let write = async {
                    stdin.write_all(line.as_bytes()).await?;
                    stdin.flush().await
                };
                if let Err(e) = write.await {
                    tracing::debug!(target: "zeron_harness::stdin", harness = target, "stdin write failed (tolerated): {e}");
                    return;
                }
            }
            WriteMsg::Close => {
                let _ = stdin.shutdown().await;
                return;
            }
            WriteMsg::Pause(reply) => {
                let (resume_tx, resume_rx) = oneshot::channel::<Resume>();
                let paused = PausedWriter {
                    stdin_fd: stdin.writer_fd(),
                    resume: resume_tx,
                };
                // A dropped receiver means the freeze was abandoned: the guard
                // is dropped with it, and writing simply carries on.
                if reply.send(paused).is_err() {
                    continue;
                }
                // Parked until the guard is dropped (a thaw: the sender going
                // away is the signal) or abandoned (a same-process hand-over).
                if let Ok(Resume::Abandon(ack)) = resume_rx.await {
                    stdin.leak_fd();
                    let _ = ack.send(());
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The child exits inside a freeze window: its stderr hits EOF while the
    /// descriptor number is already in the handoff manifest. The drain must
    /// keep it open until its owner lets go (a reused number would otherwise be
    /// inherited as "stderr" by the next image).
    #[cfg(unix)]
    #[tokio::test]
    async fn the_stderr_drain_keeps_an_exported_fd_open_past_eof_until_its_owner_lets_go() {
        use crate::process::{Command, Stdio};
        let mut command = Command::new("sh");
        command
            .args(["-c", "echo last words >&2"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(false);
        let mut child = command.spawn().unwrap();
        let stderr = child.stderr.take().unwrap();
        let tail = crate::StderrTail::default();
        let drain = StderrDrain::spawn(stderr, tail.clone(), "test");
        let fd = drain.fd().expect("a descriptor to export");
        child.wait().await.unwrap();
        tail.wait_closed().await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert_eq!(tail.lines(), ["last words"]);
        // SAFETY: fcntl on a plain descriptor number.
        assert_ne!(
            unsafe { libc::fcntl(fd, libc::F_GETFD) },
            -1,
            "the drain closed the exported descriptor at EOF"
        );
        // (Closing on drop is not asserted: a concurrent test may already have
        // reused the number.)
        drop(drain);
    }
    use std::time::Duration;
    use tokio::io::AsyncReadExt;

    impl WriterFd for tokio::io::DuplexStream {
        fn writer_fd(&self) -> Option<i32> {
            None
        }
    }

    /// A child of this process that has exited but not been reaped.
    #[cfg(unix)]
    fn zombie_with_code(code: i32) -> i32 {
        let child = std::process::Command::new("sh")
            .args(["-c", &format!("exit {code}")])
            .spawn()
            .unwrap();
        let pid = child.id() as i32;
        std::mem::forget(child);
        pid
    }

    /// Wait (bounded) until `pid` has exited but not been reaped.
    #[cfg(unix)]
    async fn wait_for_zombie(pid: i32) {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
            let rc = unsafe {
                libc::waitid(
                    libc::P_PID,
                    pid as libc::id_t,
                    &mut info,
                    libc::WEXITED | libc::WNOWAIT | libc::WNOHANG,
                )
            };
            if rc == 0 && unsafe { info.si_pid() } == pid {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "pid {pid} never exited"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn adopted_child_reports_exit_status_of_a_zombie() {
        let pid = zombie_with_code(3);
        wait_for_zombie(pid).await;
        let mut h = ChildHandle::adopt(pid).unwrap();
        assert_eq!(h.wait().await.unwrap().code(), Some(3));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn adopted_child_wait_blocks_until_a_running_child_exits() {
        let child = std::process::Command::new("sh")
            .args(["-c", "sleep 0.3; exit 5"])
            .spawn()
            .unwrap();
        let pid = child.id() as i32;
        std::mem::forget(child);
        let mut h = ChildHandle::adopt(pid).unwrap();
        // (No assertion about `try_wait` being None here: on a loaded machine
        // the child may already be gone.)
        let out = h.wait().await.unwrap();
        assert_eq!(out.code(), Some(5));
        assert!(!out.not_our_child());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn waiting_again_and_again_starts_one_waiter_and_keeps_the_real_status() {
        // The harnesses call `wait()` in `select!` arms and loops, dropping the
        // future each time. Every call must share ONE waiter (not a thread per
        // call) and none may steal the status from the caller that finally sees it.
        let child = std::process::Command::new("sh")
            .args(["-c", "sleep 0.6; exit 9"])
            .spawn()
            .unwrap();
        let pid = child.id() as i32;
        std::mem::forget(child);
        let mut h = ChildHandle::adopt(pid).unwrap();
        for _ in 0..200 {
            // Abandoned almost immediately, like a select! arm that lost.
            let _ = tokio::time::timeout(Duration::from_millis(1), h.wait()).await;
            assert!(h.try_wait().unwrap().is_none() || h.id().is_none());
        }
        #[cfg(target_os = "linux")]
        {
            let waiters = std::fs::read_dir("/proc/self/task")
                .unwrap()
                .filter_map(|entry| {
                    std::fs::read_to_string(entry.unwrap().path().join("comm")).ok()
                })
                .filter(|name| {
                    name.trim() == format!("reap-{pid}").chars().take(15).collect::<String>()
                })
                .count();
            assert_eq!(waiters, 1, "one waiter thread for the child's whole life");
        }
        let out = h.wait().await.unwrap();
        assert_eq!(
            out.code(),
            Some(9),
            "the real status survives the abandoned waits"
        );
        assert!(!out.not_our_child());
        // Repeatable after the fact.
        assert_eq!(h.wait().await.unwrap().code(), Some(9));
        assert_eq!(h.try_wait().unwrap().unwrap().code(), Some(9));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_held_gate_keeps_an_exited_child_reapable_by_the_next_image() {
        let pid = zombie_with_code(4);
        let mut h = ChildHandle::adopt(pid).unwrap();
        assert!(h.hold_reaping(), "the child is not reaped yet");
        h.try_wait().unwrap(); // starts the waiter, which parks at the gate
        wait_for_zombie(pid).await;
        tokio::time::sleep(Duration::from_millis(150)).await;
        // Still a zombie with its status intact: exactly what an exec needs.
        wait_for_zombie(pid).await;
        assert!(h.try_wait().unwrap().is_none(), "not reaped while frozen");
        h.release_reaping(); // a thaw
        assert_eq!(h.wait().await.unwrap().code(), Some(4));
        assert!(!h.hold_reaping(), "nothing left to hold once reaped");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn adopted_child_try_wait_and_start_kill() {
        let child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let pid = child.id() as i32;
        std::mem::forget(child);
        let mut h = ChildHandle::adopt(pid).unwrap();
        assert_eq!(h.id(), Some(pid as u32));
        assert!(h.try_wait().unwrap().is_none());
        h.start_kill().unwrap();
        let out = h.wait().await.unwrap();
        assert_eq!(out.code(), None);
        assert_eq!(out.signal(), Some(libc::SIGKILL));
        // Reaped: the pid could be anyone's now, so it is neither reported nor signalled.
        assert_eq!(h.id(), None);
        h.start_kill().unwrap();
        assert_eq!(h.release(), None);
    }

    #[cfg(unix)]
    #[test]
    fn a_pid_from_the_wire_that_is_not_a_plain_pid_is_refused() {
        for pid in [0, 1, -1, -2, i32::MIN] {
            assert!(ChildHandle::adopt(pid).is_err(), "pid {pid}");
        }
        assert!(ChildHandle::adopt(2).is_ok());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_pid_that_is_not_our_child_is_settled_at_adoption_and_never_signalled() {
        let mut child = std::process::Command::new("sh")
            .args(["-c", "exit 0"])
            .spawn()
            .unwrap();
        let pid = child.id() as i32;
        child.wait().unwrap();
        let mut h = ChildHandle::adopt(pid).unwrap();
        // Synchronously, before any waiter ran: callers use this to decide
        // whether there is anything to stop.
        assert!(h.try_wait().unwrap().expect("settled").not_our_child());
        assert_eq!(h.id(), None);
        h.start_kill().unwrap(); // must not signal a pid that is not ours
        assert_eq!(h.release(), None);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn releasing_an_adopted_child_holds_its_gate_against_a_running_waiter() {
        let child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let pid = child.id() as i32;
        std::mem::forget(child);
        let mut h = ChildHandle::adopt(pid).unwrap();
        h.try_wait().unwrap(); // the waiter is running now
        let gate = match &h {
            ChildHandle::Adopted(child) => child.shared.gate.clone(),
            _ => unreachable!(),
        };
        assert_eq!(h.release(), Some(pid));
        assert!(gate.hold(), "still holdable: nothing reaped it");
        unsafe { libc::kill(pid, libc::SIGKILL) };
        tokio::time::sleep(Duration::from_millis(200)).await;
        // Exited but NOT reaped: the successor can still take its status.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        let rc = unsafe {
            libc::waitid(
                libc::P_PID,
                pid as libc::id_t,
                &mut info,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            )
        };
        assert_eq!(rc, 0);
        assert_eq!(unsafe { info.si_pid() }, pid, "still a zombie");
        let mut status = 0;
        unsafe { libc::waitpid(pid, &mut status, 0) };
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn adopted_pid_that_is_not_our_child_reports_instead_of_hanging() {
        // Reap it ourselves first so the pid is no longer our child.
        let mut child = std::process::Command::new("sh")
            .args(["-c", "exit 0"])
            .spawn()
            .unwrap();
        let pid = child.id() as i32;
        child.wait().unwrap();
        let mut h = ChildHandle::adopt(pid).unwrap();
        let out = tokio::time::timeout(Duration::from_secs(5), h.wait())
            .await
            .expect("must not hang")
            .unwrap();
        assert!(out.not_our_child());
        assert_eq!(out.code(), None);
        let out = h.try_wait().unwrap().expect("gone");
        assert!(out.not_our_child());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn owned_child_behaves_like_tokio_child() {
        use crate::process::{Command, Stdio};
        let mut command = Command::new("sh");
        command
            .args(["-c", "exit 7"])
            .stdin(Stdio::null())
            .kill_on_drop(false);
        let child = command.spawn().unwrap();
        let pid = child.id().unwrap();
        let mut h = ChildHandle::Owned(child);
        assert_eq!(h.id(), Some(pid));
        let out = h.wait().await.unwrap();
        assert_eq!(out.code(), Some(7));
        assert!(!out.not_our_child());
        assert!(h.try_wait().unwrap().is_some());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn owned_child_release_hands_over_a_live_child() {
        use crate::process::{Command, Stdio};
        let mut command = Command::new("sleep");
        command.arg("30").stdin(Stdio::null()).kill_on_drop(false);
        let mut h = ChildHandle::Owned(command.spawn().unwrap());
        assert!(h.try_wait().unwrap().is_none());
        assert!(h.hold_reaping());
        let pid = h.release().expect("live child has a pid");
        // Released: still alive (nobody killed or reaped it), now adoptable.
        assert_eq!(unsafe { libc::kill(pid, 0) }, 0);
        let mut adopted = ChildHandle::adopt(pid).unwrap();
        adopted.start_kill().unwrap();
        assert_eq!(adopted.wait().await.unwrap().signal(), Some(libc::SIGKILL));
    }

    #[tokio::test]
    async fn run_controls_defaults_are_inert() {
        let (steer_tx, steer_rx) = tokio::sync::mpsc::channel(1);
        let mut controls = crate::RunControls::new(
            Box::new(|_| tokio::sync::oneshot::channel().1),
            steer_rx,
            crate::CancellationToken::new(),
        );
        drop(steer_tx);
        // No sender exists: the freeze channel reports closed, never a request.
        assert!(controls.freeze.recv().await.is_none());
        let rx = (controls.rebind_input)("req-1".into());
        assert!(
            rx.await.is_err(),
            "rebind default hands back a dead receiver"
        );
    }

    fn handoff_for_tests() -> HarnessHandoff {
        HarnessHandoff {
            harness: HarnessId::Mock,
            state_version: 3,
            pid: 4242,
            stdin_fd: 5,
            stdout_fd: 6,
            stderr_fd: Some(7),
            extra_fds: Vec::new(),
            stdout_leftover: vec![0, 1, 2, 0xff, b'\n', b'x'],
            stderr_tail: vec!["boom".into()],
            state: serde_json::json!({ "password": "hunter2", "turn": 1 }),
            undrained_steers: vec![SteerRecord {
                prompt: "steer me".into(),
                message_id: Some("m1".into()),
            }],
        }
    }

    #[test]
    fn a_handoff_round_trips_through_json_with_compact_leftover_bytes() {
        let handoff = handoff_for_tests();
        let text = serde_json::to_string(&handoff).unwrap();
        // Base64 text, not a JSON array of numbers.
        assert!(text.contains("\"stdoutLeftover\"") || text.contains("stdout_leftover"));
        assert!(
            text.contains("AAEC/wp4"),
            "base64 of the leftover bytes: {text}"
        );
        assert_eq!(
            serde_json::from_str::<HarnessHandoff>(&text).unwrap(),
            handoff
        );
        let mut bad: serde_json::Value = serde_json::from_str(&text).unwrap();
        bad["stdout_leftover"] = serde_json::json!("not base64 !!");
        assert!(serde_json::from_value::<HarnessHandoff>(bad).is_err());
    }

    #[test]
    fn fds_lists_every_carried_descriptor_but_no_pipe_placeholders() {
        let mut handoff = handoff_for_tests();
        handoff.extra_fds = vec![9];
        assert_eq!(handoff.fds(), [5, 6, 7, 9]);
        handoff.stdin_fd = HarnessHandoff::NO_PIPE;
        handoff.stdout_fd = HarnessHandoff::NO_PIPE;
        assert_eq!(handoff.fds(), [7, 9]);
    }

    #[test]
    fn debug_output_never_shows_the_protocol_state() {
        let shown = format!("{:?}", handoff_for_tests());
        assert!(!shown.contains("hunter2"), "{shown}");
        assert!(!shown.contains("steer me"), "{shown}");
        assert!(
            shown.contains("<redacted>") && shown.contains("pid: 4242"),
            "{shown}"
        );
    }

    async fn pause(
        tx: &tokio::sync::mpsc::UnboundedSender<WriteMsg>,
    ) -> tokio::sync::oneshot::Receiver<PausedWriter> {
        let (ptx, prx) = tokio::sync::oneshot::channel();
        tx.send(WriteMsg::Pause(ptx)).unwrap();
        prx
    }

    #[tokio::test]
    async fn writer_finishes_queued_lines_then_pauses_and_resumes_with_nothing_lost() {
        let (client, mut server) = tokio::io::duplex(4096);
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let task = tokio::spawn(run_writer(client, rx, "test"));
        for i in 0..50 {
            tx.send(WriteMsg::Line(format!("line-{i}"))).unwrap();
        }
        let paused = pause(&tx).await.await.expect("pause is acknowledged");
        assert_eq!(paused.stdin_fd, None);
        // Sent WHILE PAUSED, through the same sender every clone shares.
        tx.send(WriteMsg::Line("during-freeze".into())).unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            !task.is_finished(),
            "a pause suspends the writer, it does not end it"
        );
        let mut before = vec![0u8; 4096];
        let n = server.read(&mut before).await.unwrap();
        let expected: String = (0..50).map(|i| format!("line-{i}\n")).collect();
        assert_eq!(
            &before[..n],
            expected.as_bytes(),
            "only the lines queued before the pause"
        );
        // Thaw: drop the guard and the queued line follows, then normal service.
        drop(paused);
        tx.send(WriteMsg::Line("after-thaw".into())).unwrap();
        tx.send(WriteMsg::Close).unwrap();
        task.await.unwrap();
        let mut rest = String::new();
        server.read_to_string(&mut rest).await.unwrap();
        assert_eq!(
            rest, "during-freeze\nafter-thaw\n",
            "nothing was lost, order kept"
        );
    }

    #[tokio::test]
    async fn a_pause_can_be_repeated_after_a_thaw() {
        let (client, mut server) = tokio::io::duplex(4096);
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let task = tokio::spawn(run_writer(client, rx, "test"));
        for round in 0..3 {
            tx.send(WriteMsg::Line(format!("a{round}"))).unwrap();
            let paused = pause(&tx).await.await.unwrap();
            tx.send(WriteMsg::Line(format!("b{round}"))).unwrap();
            drop(paused);
        }
        tx.send(WriteMsg::Close).unwrap();
        task.await.unwrap();
        let mut all = String::new();
        server.read_to_string(&mut all).await.unwrap();
        assert_eq!(all, "a0\nb0\na1\nb1\na2\nb2\n");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn abandoning_a_paused_writer_lets_go_of_the_descriptor_without_closing_it() {
        use std::os::fd::AsRawFd;
        impl WriterFd for std::fs::File {
            fn writer_fd(&self) -> Option<i32> {
                Some(self.as_raw_fd())
            }
        }
        // Not used through run_writer: File is not AsyncWrite. Use a real pipe
        // through the production type instead.
        let mut command = crate::process::Command::new("cat");
        command
            .stdin(crate::process::Stdio::piped())
            .stdout(crate::process::Stdio::null())
            .kill_on_drop(true);
        let mut child = command.spawn().unwrap();
        let stdin = child.stdin.take().unwrap();
        let fd = stdin.as_raw_fd();
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let task = tokio::spawn(run_writer(stdin, rx, "test"));
        let paused = pause(&tx).await.await.unwrap();
        assert_eq!(paused.stdin_fd, Some(fd));
        paused.abandon().await;
        task.await.unwrap();
        // Still open: the descriptor was leaked, not closed.
        assert_ne!(unsafe { libc::fcntl(fd, libc::F_GETFD) }, -1);
        unsafe { libc::close(fd) };
    }

    #[tokio::test]
    async fn an_abandoned_pause_does_not_stop_the_writer() {
        let (client, mut server) = tokio::io::duplex(4096);
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let task = tokio::spawn(run_writer(client, rx, "test"));
        drop(pause(&tx).await); // the freeze gave up before the answer arrived
        tx.send(WriteMsg::Line("still-here".into())).unwrap();
        tx.send(WriteMsg::Close).unwrap();
        task.await.unwrap();
        let mut all = String::new();
        server.read_to_string(&mut all).await.unwrap();
        assert_eq!(all, "still-here\n");
    }

    #[tokio::test]
    async fn writer_never_tears_a_line_across_a_slow_reader() {
        // A pipe far smaller than a line forces partial writes; every line
        // must still arrive whole and in order, and the pause must be
        // acknowledged only after the last byte of the last queued line was
        // accepted.
        let (client, mut server) = tokio::io::duplex(16);
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let task = tokio::spawn(run_writer(client, rx, "test"));
        let lines: Vec<String> = (0..20)
            .map(|i| format!("{{\"n\":{i},\"pad\":\"{}\"}}", "z".repeat(100)))
            .collect();
        for line in &lines {
            tx.send(WriteMsg::Line(line.clone())).unwrap();
        }
        let mut prx = pause(&tx).await;
        // Reader is stalled: the pause cannot be acknowledged yet.
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            prx.try_recv().is_err(),
            "the pause must wait for queued lines"
        );
        let mut paused = None;
        let mut got = Vec::new();
        let mut buf = [0u8; 7];
        while paused.is_none() {
            tokio::select! {
                n = server.read(&mut buf) => {
                    let n = n.unwrap();
                    assert!(n > 0, "writer closed before the pause completed");
                    got.extend_from_slice(&buf[..n]);
                }
                p = &mut prx => paused = Some(p.unwrap()),
            }
        }
        drop(paused);
        tx.send(WriteMsg::Close).unwrap();
        task.await.unwrap();
        server.read_to_end(&mut got).await.unwrap();
        let expected: String = lines.iter().map(|l| format!("{l}\n")).collect();
        assert_eq!(String::from_utf8(got).unwrap(), expected);
    }

    #[tokio::test]
    async fn writer_close_shuts_stdin_down() {
        let (client, mut server) = tokio::io::duplex(64);
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let task = tokio::spawn(run_writer(client, rx, "test"));
        tx.send(WriteMsg::Line("hi".into())).unwrap();
        tx.send(WriteMsg::Close).unwrap();
        task.await.unwrap();
        let mut all = String::new();
        server.read_to_string(&mut all).await.unwrap();
        assert_eq!(all, "hi\n");
    }

    #[tokio::test]
    async fn writer_tolerates_a_dead_reader() {
        let (client, server) = tokio::io::duplex(64);
        drop(server);
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let task = tokio::spawn(run_writer(client, rx, "test"));
        tx.send(WriteMsg::Line("x".into())).unwrap();
        // Writer gives up quietly; a later pause sees a closed reply.
        task.await.unwrap();
        let (ptx, prx) = tokio::sync::oneshot::channel();
        assert!(tx.send(WriteMsg::Pause(ptx)).is_err());
        assert!(prx.await.is_err());
    }
}
