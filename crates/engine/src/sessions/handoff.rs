//! Run handoff — the sessions half of an engine handoff.
//!
//! The engine replaces itself with `execve` (same PID), so an agent child
//! stays our child and only its pipes survive. A running agent turn is
//! carried across like this (see `docs/live-update.md`, "Agent runs"):
//!
//! 1. [`SessionsEngine::freeze_runs`] raises the `frozen` flag (under the
//!    `runs` lock, then through every run's steer-ledger lock), so nothing
//!    starts a run, routes a prompt, interrupts one or answers a question
//!    behind the snapshot. Each run whose harness can hand over is asked to
//!    freeze: its task forwards a `FreezeRequest` to the harness, which
//!    answers at a safe point (or `Busy`), drains what the harness emitted
//!    before it stopped, flushes the fold into the docs and reports a
//!    [`FoldSnapshot`]. All or nothing: one refusal thaws every run.
//!    Runs of harnesses that cannot hand over are retired when parked (the
//!    next message resumes them, `--resume` / `thread/resume` /
//!    `session/load`, as after any restart) and make the freeze busy when
//!    mid-turn.
//! 2. The coordinator writes [`FrozenRuns::handoffs`] into the manifest,
//!    makes the pipes inheritable and execs while still holding
//!    [`FrozenRuns`]. Exec runs no destructors, so every run is merely
//!    suspended; if the exec fails, dropping [`FrozenRuns`] thaws them and
//!    each continues in this image. [`FrozenRuns::commit`] is the one-way
//!    alternative for a successor in the same process (tests).
//! 3. [`SessionsEngine::adopt_runs`] — in the successor, before stale-run
//!    recovery: registers each run under its old id with its steer ledger,
//!    runtime configuration and parked questions (same request ids, no
//!    second prompt), re-queues the steers its mailbox held, and starts its
//!    task in adopt mode (`Harness::adopt`, fold seeded from the snapshot).
//!    A run that cannot be adopted is stopped and left to stale-run
//!    recovery, today's crash path.

use std::os::fd::RawFd;

use serde::{Deserialize, Serialize};
use zeron_harness::{ChildHandle, HarnessHandoff, SteerRecord};

use super::*;

/// How long a run task may take to answer a freeze.
const FREEZE_ANSWER: std::time::Duration = std::time::Duration::from_secs(5);
/// A harness's `Busy` answer stays a handoff veto this long at most (it is
/// cleared as soon as the run emits anything).
const REFUSAL_TTL: std::time::Duration = std::time::Duration::from_secs(30);

/// One running agent turn in the handoff manifest.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunHandoff {
    pub chat_id: String,
    pub run_id: String,
    pub request: RunRequest,
    /// So the first same-config prompt after adoption routes into the run
    /// instead of looking like a config change that restarts it.
    pub runtime_config: RuntimeConfig,
    /// Accepted steers not yet confirmed by a `Steered` boundary.
    pub routed_steers: Vec<RoutedSteer>,
    /// Steers the harness had not read from its mailbox; re-queued in order.
    pub undrained_steers: Vec<SteerRecord>,
    /// Questions parked on the user, re-registered under the same ids.
    pub pending_inputs: Vec<PendingInputRecord>,
    pub fork_history_sent: bool,
    /// The session row as it stood (status, elapsed-timer base).
    pub session: Option<Session>,
    pub fold: FoldSnapshot,
    pub harness: HarnessHandoff,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingInputRecord {
    pub request_id: String,
    pub questions: Vec<UserInputQuestion>,
}

impl RunHandoff {
    /// The inherited descriptors this run's adoption is responsible for
    /// (never a [`HarnessHandoff::NO_PIPE`] placeholder).
    pub fn fds(&self) -> Vec<RawFd> {
        self.harness.fds()
    }
}

struct FrozenEntry {
    handoff: RunHandoff,
    /// To the run task: send = commit, drop = thaw.
    verdict: Option<oneshot::Sender<()>>,
}

/// Every run, frozen. Dropping this without [`Self::commit`] thaws.
pub struct FrozenRuns {
    inner: Arc<Inner>,
    runs: Vec<FrozenEntry>,
    finished: bool,
}

impl FrozenRuns {
    /// The manifest entries for the frozen runs.
    pub fn handoffs(&self) -> Vec<RunHandoff> {
        self.runs.iter().map(|run| run.handoff.clone()).collect()
    }

    /// Let the runs' pipes survive an `execve` (clear `FD_CLOEXEC`). Called
    /// as late as possible: while inheritable, any child the engine spawns
    /// would inherit them too. [`Self::thaw`] undoes it.
    pub fn make_inheritable(&self) -> std::io::Result<()> {
        for run in &self.runs {
            for fd in run.handoff.fds() {
                crate::handoff::set_inheritable(fd, true)?;
            }
        }
        Ok(())
    }

    /// Undo the freeze: every run carries on in this image.
    pub fn thaw(mut self) {
        self.thaw_inner();
    }

    fn thaw_inner(&mut self) {
        if std::mem::replace(&mut self.finished, true) {
            return;
        }
        for run in self.runs.drain(..) {
            for fd in run.handoff.fds() {
                if let Err(err) = crate::handoff::set_inheritable(fd, false) {
                    tracing::error!(
                        chat = %run.handoff.chat_id,
                        fd,
                        error = %err,
                        "could not make an agent pipe close-on-exec again"
                    );
                }
            }
            // Dropping the verdict sender thaws the run task and harness.
            drop(run.verdict);
        }
        self.inner
            .frozen
            .store(false, std::sync::atomic::Ordering::SeqCst);
        // Commands that arrived while frozen were left pending (the doc
        // host defers its command drain during a freeze): apply them now.
        if tokio::runtime::Handle::try_current().is_ok()
            && let Some(host) = self.inner.doc_host()
        {
            host.kick_drains();
        }
    }

    /// Give the runs to a successor living in this same process image's
    /// stead. ONE-WAY: each harness gives up its child and pipes without
    /// closing, killing or reaping anything and ends its stream, and each run
    /// task ends without settling anything; this image stays frozen. An
    /// `execve` does NOT need this (exec runs no destructors): the engine's
    /// handoff keeps the `FrozenRuns` and thaws only if the exec fails. Use
    /// `commit` only when the old image keeps running without its runs
    /// (tests standing in for an exec).
    pub fn commit(mut self) -> Vec<RunHandoff> {
        self.finished = true;
        self.runs
            .drain(..)
            .map(|mut run| {
                if let Some(verdict) = run.verdict.take() {
                    let _ = verdict.send(());
                }
                run.handoff
            })
            .collect()
    }
}

impl std::fmt::Debug for FrozenRuns {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FrozenRuns")
            .field(
                "chats",
                &self
                    .runs
                    .iter()
                    .map(|run| run.handoff.chat_id.as_str())
                    .collect::<Vec<_>>(),
            )
            .field("finished", &self.finished)
            .finish()
    }
}

impl Drop for FrozenRuns {
    fn drop(&mut self) {
        self.thaw_inner();
    }
}

/// One live run as the freeze found it.
struct FreezeTarget {
    chat_id: String,
    run_id: String,
    adoptable: bool,
    started: bool,
    interrupting: bool,
    freeze_tx: mpsc::UnboundedSender<EngineFreeze>,
    runtime_config: RuntimeConfig,
    ledger: Arc<Mutex<std::collections::VecDeque<RoutedSteer>>>,
    pending: PendingInputs,
    fork_history_sent: Arc<std::sync::atomic::AtomicBool>,
}

impl FreezeTarget {
    fn of(chat_id: &str, handle: &RunHandle) -> Self {
        Self {
            chat_id: chat_id.to_string(),
            run_id: handle.run_id.clone(),
            adoptable: handle.adoptable,
            started: handle.started.load(std::sync::atomic::Ordering::SeqCst),
            interrupting: *handle.cancel.borrow(),
            freeze_tx: handle.freeze_tx.clone(),
            runtime_config: handle.runtime_config.clone(),
            ledger: handle.routed_steers.clone(),
            pending: handle.pending_inputs.clone(),
            fork_history_sent: handle.fork_history_sent.clone(),
        }
    }
}

fn busy(reasons: Vec<String>) -> EngineError {
    EngineError::Other(format!("busy: {}", reasons.join("; ")))
}

impl SessionsEngine {
    /// Why this run cannot freeze, as far as the engine alone can tell.
    fn engine_refusal(&self, target: &FreezeTarget) -> Option<&'static str> {
        if target.interrupting {
            Some("the run is being interrupted")
        } else if !target.started {
            Some("the run is starting")
        } else if !target.adoptable
            && (self.turn_in_flight(&target.chat_id) || !lock(&target.ledger).is_empty())
        {
            // An accepted steer (in the ledger) owns a turn too: `steer()`
            // pushes it before it marks the chat Working, and retiring the run
            // would drop it with the interrupted run's ledger.
            Some("an agent turn is running on a harness that cannot hand a run over yet")
        } else {
            None
        }
    }

    /// Reasons a handoff cannot proceed right now, one per blocking run:
    /// what the engine can see (interrupting, starting, a turn in flight on a
    /// harness without adoption) plus a harness's recent `Busy` answer to a
    /// freeze (dropped once that run emits anything, or after a while).
    pub fn unfreezable_runs(&self) -> Vec<String> {
        let targets: Vec<FreezeTarget> = lock(&self.inner.runs)
            .iter()
            .map(|(chat_id, handle)| FreezeTarget::of(chat_id, handle))
            .collect();
        let mut reasons = Vec::new();
        for target in &targets {
            if let Some(reason) = self.engine_refusal(target) {
                reasons.push(format!("chat {}: busy: {reason}", target.chat_id));
                continue;
            }
            let refusals = lock(&self.inner.freeze_refusals);
            if let Some(note) = refusals.get(&target.chat_id)
                && note.run_id == target.run_id
                && note.at.elapsed() < REFUSAL_TTL
            {
                reasons.push(format!("chat {}: busy: {}", target.chat_id, note.reason));
            }
        }
        reasons
    }

    /// Freeze every run for a handoff, or refuse (`busy: …`) leaving all of
    /// them running. See the module docs. Do not cancel this future partway;
    /// it bounds itself.
    pub async fn freeze_runs(&self) -> Result<FrozenRuns, EngineError> {
        // The flag and the snapshot are taken under the `runs` lock, where
        // `dispatch` registers runs: every run is in this list or refused.
        let targets: Vec<FreezeTarget> = {
            let runs = lock(&self.inner.runs);
            if self
                .inner
                .frozen
                .swap(true, std::sync::atomic::Ordering::SeqCst)
            {
                return Err(EngineError::Other("runs are already frozen".into()));
            }
            runs.iter()
                .map(|(chat_id, handle)| FreezeTarget::of(chat_id, handle))
                .collect()
        };
        // From here `frozen` owns the flag: dropping it (on any refusal
        // below) thaws what froze and lowers the flag.
        let mut frozen = FrozenRuns {
            inner: self.inner.clone(),
            runs: Vec::new(),
            finished: false,
        };
        // Barrier: a steer that saw the flag down checked it under its run's
        // ledger lock and pushes before releasing it, so once each lock has
        // been taken here every accepted steer is in its mailbox and ledger.
        for target in &targets {
            drop(lock(&target.ledger));
            // Likewise an answer: `respond_input` checks the flag, takes the
            // resolver and sends the answer under this lock, so once it has
            // been taken here every accepted answer is already with the run.
            drop(lock(&target.pending));
        }
        let mut reasons: Vec<String> = targets
            .iter()
            .filter_map(|target| {
                self.engine_refusal(target)
                    .map(|reason| format!("chat {}: {reason}", target.chat_id))
            })
            .collect();
        if !reasons.is_empty() {
            return Err(busy(reasons));
        }

        let adoptable: Vec<&FreezeTarget> = targets.iter().filter(|t| t.adoptable).collect();
        let answers = futures::future::join_all(adoptable.iter().map(|target| async move {
            let (reply, answer) = oneshot::channel();
            if target.freeze_tx.send(EngineFreeze { reply }).is_err() {
                return Err("the run has ended".to_string());
            }
            match tokio::time::timeout(FREEZE_ANSWER, answer).await {
                Ok(Ok(result)) => result,
                Ok(Err(_)) => Err("the run ended while freezing".to_string()),
                Err(_) => Err("the run did not answer the freeze in time".to_string()),
            }
        }))
        .await;
        for (target, answer) in adoptable.into_iter().zip(answers) {
            match answer {
                Ok(mut run) => {
                    let undrained_steers = std::mem::take(&mut run.harness.undrained_steers);
                    let pending_inputs = lock(&target.pending)
                        .iter()
                        .map(|(request_id, input)| PendingInputRecord {
                            request_id: request_id.clone(),
                            questions: input.questions.clone(),
                        })
                        .collect();
                    frozen.runs.push(FrozenEntry {
                        handoff: RunHandoff {
                            chat_id: target.chat_id.clone(),
                            run_id: target.run_id.clone(),
                            request: run.request,
                            runtime_config: target.runtime_config.clone(),
                            routed_steers: lock(&target.ledger).iter().cloned().collect(),
                            undrained_steers,
                            pending_inputs,
                            fork_history_sent: target
                                .fork_history_sent
                                .load(std::sync::atomic::Ordering::Acquire),
                            session: self.session_status(&target.chat_id),
                            fold: run.fold,
                            harness: run.harness,
                        },
                        verdict: Some(run.verdict),
                    });
                }
                Err(reason) => reasons.push(format!("chat {}: {reason}", target.chat_id)),
            }
        }
        if !reasons.is_empty() {
            return Err(busy(reasons));
        }

        // Runs this build cannot carry are parked between turns (a turn in
        // flight refused above): retire them through the ordinary interrupt,
        // their conversation resumes on the next message. Irreversible, so
        // only once everything else froze.
        for target in targets.iter().filter(|t| !t.adoptable) {
            if let Err(err) = self.interrupt_run(&target.chat_id).await {
                tracing::warn!(chat = %target.chat_id, error = %err, "retiring a parked run for a handoff failed");
            }
        }
        Ok(frozen)
    }

    /// Register the runs a predecessor handed over, before stale-run recovery
    /// looks at the journals (it skips chats with a live run). A run that
    /// cannot be adopted is stopped and left to that recovery.
    pub fn adopt_runs(&self, runs: Vec<RunHandoff>) {
        for run in runs {
            let chat_id = run.chat_id.clone();
            let pid = run.harness.pid;
            if let Err(err) = self.adopt_run(run) {
                tracing::error!(chat = %chat_id, error = %err, "could not adopt a handed-over run; recovering it from its journal");
                // Stale-run recovery (right after this) resumes the chat only
                // once this stop has finished.
                if let Some(stop) = abandon_child(pid) {
                    lock(&self.inner.stopping_children).insert(chat_id, stop);
                }
            }
        }
    }

    fn adopt_run(&self, mut run: RunHandoff) -> Result<(), EngineError> {
        let harness = self.inner.registry.resolve(run.harness.harness)?;
        if !harness.supports_adoption() {
            return Err(EngineError::Other(format!(
                "{} cannot adopt a run",
                harness.display_name()
            )));
        }
        let handle = self.doc_handle(&run.chat_id)?;
        let chat_id = run.chat_id.clone();
        // Duplicate the inherited descriptors NOW, while the originals are
        // certainly open: the harness adopts later, inside the run task, and the
        // adoption commit closes the originals as soon as boot is done.
        let fds = dup_harness_fds(&mut run.harness)?;

        let (steer_tx, steer_rx) = mpsc::channel::<SteerMessage>(32);
        for steer in run.undrained_steers {
            if steer_tx.try_send(steer.into()).is_err() {
                tracing::warn!(chat = %chat_id, "an undrained steer did not fit the adopted mailbox");
            }
        }
        let (cancel_tx, cancel_rx) = watch::channel(false);
        let (engine_tx, engine_rx) = mpsc::unbounded_channel::<AgentEvent>();
        // Parked questions keep their ids: a resolver is registered now (so
        // an answer can land before the harness rebinds) and its receiving
        // end waits for the harness's `rebind_input`.
        let pending_inputs: PendingInputs = Arc::new(Mutex::new(HashMap::new()));
        let mut slots = HashMap::new();
        for record in &run.pending_inputs {
            let (tx, rx) = oneshot::channel();
            lock(&pending_inputs).insert(
                record.request_id.clone(),
                PendingInput {
                    tx,
                    questions: record.questions.clone(),
                },
            );
            slots.insert(record.request_id.clone(), rx);
        }
        let request_input = input_bridge(pending_inputs.clone(), engine_tx.clone());
        let rebind_input = rebind_bridge(
            pending_inputs.clone(),
            engine_tx.clone(),
            slots,
            run.pending_inputs,
        );
        let interrupt_token = CancellationToken::new();
        let (link, freeze_tx, harness_freeze) = FreezeLink::new(true);
        let controls = RunControls {
            execution_lease: None,
            request_input,
            steering: steer_rx,
            interrupt: interrupt_token.clone(),
            freeze: harness_freeze,
            rebind_input,
        };
        let fork_history_sent = Arc::new(std::sync::atomic::AtomicBool::new(run.fork_history_sent));
        {
            let mut runs = lock(&self.inner.runs);
            if runs.contains_key(&chat_id) {
                return Err(EngineError::Other("the chat already has a live run".into()));
            }
            runs.insert(
                chat_id.clone(),
                RunHandle {
                    run_id: run.run_id.clone(),
                    steerable: harness.supports_steering(),
                    runtime_config: run.runtime_config,
                    steer_tx,
                    interrupt_token,
                    cancel: cancel_tx,
                    engine_tx,
                    pending_inputs,
                    routed_steers: Arc::new(Mutex::new(run.routed_steers.into())),
                    fork_history_sent: fork_history_sent.clone(),
                    freeze_tx,
                    adoptable: true,
                    started: link.started.clone(),
                },
            );
        }
        if let Some(session) = run.session {
            self.inner.restore_status(session);
        }
        lock(&self.inner.last_requests).insert(chat_id.clone(), run.request.clone());
        let resume_state = RunResumeState {
            user_message_id: run.fold.user_message_id.clone(),
            resume_injected: run.fold.resume_injected,
            startup_retry: run.fold.startup_retry,
            fork_history_sent,
        };
        tracing::info!(chat = %chat_id, run = %run.run_id, "adopting a handed-over run");
        tokio::spawn(drive_run(
            self.inner.clone(),
            chat_id,
            run.run_id,
            harness,
            run.request,
            handle.writer(),
            controls,
            engine_rx,
            cancel_rx,
            resume_state,
            link,
            RunMode::Adopt(Box::new(AdoptSeed {
                harness: run.harness,
                fold: run.fold,
                fds,
            })),
        ));
        Ok(())
    }
}

/// Replace every descriptor number `handoff` names with an engine-owned
/// close-on-exec duplicate (returned so the caller keeps them open), refusing
/// standard streams and anything not open. A [`HarnessHandoff::NO_PIPE`]
/// stdin/stdout (a child without such a pipe) stays as it is.
fn dup_harness_fds(
    handoff: &mut zeron_harness::HarnessHandoff,
) -> Result<Vec<std::os::fd::OwnedFd>, EngineError> {
    use std::os::fd::{FromRawFd, OwnedFd};
    let mut owned = Vec::new();
    let mut dup = |fd: i32| -> Result<i32, EngineError> {
        // SAFETY: fcntl on a plain descriptor number; no memory is involved.
        if fd < 3 || unsafe { libc::fcntl(fd, libc::F_GETFD) } < 0 {
            return Err(EngineError::Other(format!(
                "the handed-over descriptor {fd} is not usable"
            )));
        }
        // SAFETY: as above; the result is a fresh descriptor we own.
        let copy = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 3) };
        if copy < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        // SAFETY: `copy` was just created and is owned by nobody else.
        owned.push(unsafe { OwnedFd::from_raw_fd(copy) });
        Ok(copy)
    };
    for fd in [&mut handoff.stdin_fd, &mut handoff.stdout_fd] {
        if *fd != HarnessHandoff::NO_PIPE {
            *fd = dup(*fd)?;
        }
    }
    if let Some(fd) = handoff.stderr_fd {
        handoff.stderr_fd = Some(dup(fd)?);
    }
    for fd in &mut handoff.extra_fds {
        *fd = dup(*fd)?;
    }
    Ok(owned)
}

/// `RunControls.rebind_input` for an adopted run: hand the harness the
/// answer to a question the predecessor parked, under the SAME engine
/// request id, without a second `InputRequested`. The harness passes the
/// engine request id — or, since the bridge mints that id and a harness
/// never learns it, the id of any question in the parked set.
fn rebind_bridge(
    pending: PendingInputs,
    engine_tx: mpsc::UnboundedSender<AgentEvent>,
    slots: HashMap<String, oneshot::Receiver<Vec<UserInputAnswer>>>,
    records: Vec<PendingInputRecord>,
) -> zeron_harness::RebindInput {
    let slots = Mutex::new(slots);
    Box::new(move |key: String| {
        let (request_id, slot) = {
            let mut slots = lock(&slots);
            // The engine request id itself; else the one still-parked set
            // holding a question with this id (never a guess between two).
            let request_id = if slots.contains_key(&key) {
                Some(key.clone())
            } else {
                let mut matching = records.iter().filter(|record| {
                    slots.contains_key(&record.request_id)
                        && record.questions.iter().any(|q| q.id == key)
                });
                match (matching.next(), matching.next()) {
                    (Some(record), None) => Some(record.request_id.clone()),
                    _ => None,
                }
            };
            let slot = request_id.as_ref().and_then(|id| slots.remove(id));
            (request_id, slot)
        };
        match (request_id, slot) {
            (Some(request_id), Some(answer_rx)) => {
                forward_answer(pending.clone(), engine_tx.clone(), request_id, answer_rx)
            }
            _ => {
                tracing::warn!(key = %key, "a harness rebound a question the handoff did not carry");
                oneshot::channel().1
            }
        }
    })
}

impl Inner {
    /// Put a handed-over session row back as it stood.
    fn restore_status(&self, session: Session) {
        let mut statuses = lock(&self.statuses);
        statuses.insert(session.chat_id.clone(), session);
        let mut list: Vec<Session> = statuses.values().cloned().collect();
        list.sort_by(|a, b| a.chat_id.cmp(&b.chat_id));
        self.sessions_tx.send_replace(list);
    }
}

/// A handed-over agent nobody will drive: stop it (a stale-run recovery may
/// resume its conversation with a fresh one — never two agents on one
/// session) and reap it. It is still our child: `execve` kept the PID. The
/// stop is bounded (SIGTERM, SIGKILL after 5 s); await the handle to know the
/// agent is gone.
pub(crate) fn abandon_child(pid: i32) -> Option<tokio::task::JoinHandle<()>> {
    if pid <= 1 {
        return None;
    }
    let runtime = tokio::runtime::Handle::try_current().ok()?;
    Some(runtime.spawn(async move {
        let Ok(mut child) = ChildHandle::adopt(pid) else {
            return;
        };
        if matches!(child.try_wait(), Ok(Some(_))) {
            return; // already gone (or never ours): nothing to stop
        }
        // SAFETY: kill(2) on a pid that is still our unreaped child.
        unsafe { libc::kill(pid, libc::SIGTERM) };
        if tokio::time::timeout(std::time::Duration::from_secs(5), child.wait())
            .await
            .is_err()
        {
            let _ = child.start_kill();
            let _ = child.wait().await;
        }
    }))
}

#[cfg(test)]
mod tests {
    use std::os::fd::AsRawFd;
    use std::time::Duration;

    use tempfile::TempDir;
    use zeron_harness::mock::FreezableMock;

    use super::*;
    use crate::instance_lock::InstanceLock;
    use crate::{AdoptedState, EngineCore, EngineProfile};

    const WAIT: Duration = Duration::from_secs(15);

    fn request(prompt: &str) -> RunRequest {
        RunRequest {
            mcp: None,
            prompt: prompt.into(),
            harness: None,
            model: None,
            reasoning: None,
            model_options: Default::default(),
            cwd: std::env::temp_dir().to_string_lossy().into_owned(),
            sandbox: zeron_proto::SandboxLevel::WorkspaceWrite,
            auto_approve: false,
            attachments: Vec::new(),
            resume: None,
            worktree: None,
        }
    }

    fn registry(harness: Arc<dyn Harness>) -> Arc<HarnessRegistry> {
        let registry = HarnessRegistry::new();
        registry.register(harness);
        Arc::new(registry)
    }

    struct Rig {
        dir: TempDir,
        core: EngineCore,
    }

    fn rig_with(harness: Arc<dyn Harness>) -> Rig {
        let dir = tempfile::tempdir().unwrap();
        let core = EngineCore::assemble_with_profile(
            EngineProfile::local(dir.path()).unwrap(),
            registry(harness),
            HarnessId::Mock,
            None,
        )
        .unwrap();
        Rig { dir, core }
    }

    fn rig() -> Rig {
        rig_with(Arc::new(FreezableMock))
    }

    /// An OpenCode run: its server's stdin and stdout are /dev/null, so only
    /// stderr is a pipe to carry.
    #[test]
    fn a_run_without_stdio_pipes_carries_and_duplicates_only_its_real_descriptors() {
        let mut pipe = [0; 2];
        assert_eq!(unsafe { libc::pipe(pipe.as_mut_ptr()) }, 0);
        let request = request("x");
        let run = RunHandoff {
            chat_id: "c1".into(),
            run_id: "r1".into(),
            runtime_config: RuntimeConfig::from_request(HarnessId::Opencode, &request),
            request,
            routed_steers: Vec::new(),
            undrained_steers: Vec::new(),
            pending_inputs: Vec::new(),
            fork_history_sent: false,
            session: None,
            fold: FoldSnapshot::default(),
            harness: HarnessHandoff {
                harness: HarnessId::Opencode,
                state_version: 1,
                pid: 4242,
                stdin_fd: HarnessHandoff::NO_PIPE,
                stdout_fd: HarnessHandoff::NO_PIPE,
                stderr_fd: Some(pipe[0]),
                extra_fds: Vec::new(),
                stdout_leftover: Vec::new(),
                stderr_tail: Vec::new(),
                state: serde_json::Value::Null,
                undrained_steers: Vec::new(),
            },
        };
        // Flagged inheritable, validated and closed: the real pipe only.
        assert_eq!(run.fds(), vec![pipe[0]]);
        let mut harness = run.harness.clone();
        let owned = dup_harness_fds(&mut harness).expect("a placeholder is not refused");
        assert_eq!(owned.len(), 1);
        assert_eq!(harness.stdin_fd, HarnessHandoff::NO_PIPE);
        assert_eq!(harness.stdout_fd, HarnessHandoff::NO_PIPE);
        assert_eq!(harness.stderr_fd, Some(owned[0].as_raw_fd()));
        drop(owned);
        unsafe {
            libc::close(pipe[0]);
            libc::close(pipe[1]);
        }
    }

    impl Rig {
        fn sessions(&self) -> &SessionsEngine {
            &self.core.sessions
        }

        async fn start(&self, chat: &str, prompt: &str) -> String {
            self.sessions()
                .dispatch(chat, HarnessId::Mock, request(prompt), None)
                .await
                .unwrap()
        }

        fn journal(&self, chat: &str) -> Vec<AgentEvent> {
            self.sessions()
                .inner
                .journal
                .replay(chat, 0)
                .unwrap()
                .into_iter()
                .map(|(_, event)| event)
                .collect()
        }

        fn said(&self, chat: &str, text: &str) -> bool {
            self.journal(chat)
                .iter()
                .any(|event| matches!(event, AgentEvent::TextDelta { text: t } if t == text))
        }

        async fn until_said(&self, chat: &str, text: &str) {
            eventually(&format!("{chat} says {text:?}"), || self.said(chat, text)).await;
        }

        fn ends_in_done(&self, chat: &str) -> bool {
            matches!(self.journal(chat).last(), Some(AgentEvent::Done { .. }))
        }

        fn entries(&self, chat: &str) -> Vec<SessionMessageEntry> {
            self.sessions()
                .doc_handle(chat)
                .unwrap()
                .doc()
                .read_entries()
                .unwrap()
        }

        fn aborted(&self, chat: &str) -> usize {
            self.entries(chat)
                .iter()
                .filter(|entry| entry.status == Some(MessageStatus::Aborted))
                .count()
        }

        fn status(&self, chat: &str) -> Option<SessionStatus> {
            self.sessions().session_status(chat).map(|s| s.status)
        }

        fn live_run(&self, chat: &str) -> Option<String> {
            lock(&self.sessions().inner.runs)
                .get(chat)
                .map(|handle| handle.run_id.clone())
        }

        fn input_request(&self, chat: &str) -> Option<String> {
            self.journal(chat)
                .into_iter()
                .find_map(|event| match event {
                    AgentEvent::InputRequested { request_id, .. } => Some(request_id),
                    _ => None,
                })
        }

        /// Freeze and commit every run, retire this image the way an exec
        /// would, and boot a successor on the same data dir that adopts them.
        async fn hand_over(self) -> (Rig, OldImage, Vec<RunHandoff>) {
            let before: Vec<_> = lock(&self.sessions().inner.runs)
                .keys()
                .map(|chat| (chat.clone(), self.status(chat), self.journal(chat).len()))
                .collect();
            let runs = self
                .sessions()
                .freeze_runs()
                .await
                .expect("freeze")
                .commit();
            // The committed run tasks end (their harness streams end) without
            // settling anything: still registered, same status, no Done.
            eventually("the old run tasks end", || {
                lock(&self.sessions().inner.runs)
                    .values()
                    .all(|handle| handle.engine_tx.is_closed())
            })
            .await;
            for (chat, status, journal) in before {
                assert!(self.live_run(&chat).is_some(), "{chat}: still registered");
                assert_eq!(self.status(&chat), status, "{chat}: status untouched");
                assert_eq!(
                    self.journal(&chat).len(),
                    journal,
                    "{chat}: journal untouched"
                );
            }
            let (dir, lock_fd, old) = retire(self).await;
            let profile = EngineProfile::local(dir.path()).unwrap();
            let lock = InstanceLock::adopt(lock_fd, profile.device_root()).unwrap();
            let core = EngineCore::assemble_with_profile_adopting(
                profile,
                registry(Arc::new(FreezableMock)),
                HarnessId::Mock,
                None,
                lock,
                AdoptedState {
                    terminals: Vec::new(),
                    runs: runs.clone(),
                },
            )
            .unwrap();
            (Rig { dir, core }, old.with(runs.clone()), runs)
        }
    }

    async fn eventually(what: &str, mut check: impl FnMut() -> bool) {
        let deadline = tokio::time::Instant::now() + WAIT;
        while !check() {
            assert!(tokio::time::Instant::now() < deadline, "timed out: {what}");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// Stand-in for the exec: the old image never touches its runs again.
    /// Its runs were committed (each harness gave its child and pipes up and
    /// ended; each run task ended without settling anything); its shutdown
    /// flushes the docs and — gated by the freeze — must not touch a run;
    /// and it is forgotten, never dropped, so it runs no destructor (its
    /// instance lock passes to the successor, as across an exec).
    async fn retire(old: Rig) -> (TempDir, RawFd, OldImage) {
        old.core.shutdown().await;
        let lock_fd = old.core._instance_lock.raw_fd();
        let Rig { dir, core } = old;
        std::mem::forget(core);
        (dir, lock_fd, OldImage(Vec::new()))
    }

    /// Closes the handed-over originals when the test ends, as the
    /// successor's `Adoption::commit` would.
    struct OldImage(Vec<RawFd>);

    impl OldImage {
        fn with(mut self, runs: Vec<RunHandoff>) -> Self {
            self.0 = runs.iter().flat_map(RunHandoff::fds).collect();
            self
        }
    }

    impl Drop for OldImage {
        fn drop(&mut self) {
            for &fd in &self.0 {
                unsafe { libc::close(fd) };
            }
        }
    }

    fn alive(pid: i32) -> bool {
        unsafe { libc::kill(pid, 0) == 0 }
    }

    fn cloexec(fd: RawFd) -> bool {
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
        flags >= 0 && flags & libc::FD_CLOEXEC != 0
    }

    fn yes(question_id: &str) -> Vec<UserInputAnswer> {
        vec![UserInputAnswer {
            question_id: question_id.into(),
            labels: vec!["yes".into()],
        }]
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_frozen_run_is_not_marked_done_aborted_or_redispatched_and_thaws() {
        let rig = rig();
        let run_id = rig.start("c1", "hold").await;
        rig.until_said("c1", "working").await;
        let journal = rig.journal("c1").len();

        let frozen = rig
            .sessions()
            .freeze_runs()
            .await
            .expect("a held turn freezes");
        let handoff = frozen.handoffs().remove(0);
        assert_eq!(handoff.run_id, run_id);
        assert!(alive(handoff.harness.pid));
        assert!(
            handoff
                .fold
                .folded
                .iter()
                .any(|part| matches!(part, MessagePart::Text { text, .. } if text == "working")),
            "the open segment is carried: {:?}",
            handoff.fold.folded
        );
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(rig.journal("c1").len(), journal, "nothing settled the run");
        assert!(!rig.ends_in_done("c1"), "no synthetic Done");
        assert_eq!(rig.aborted("c1"), 0, "no aborted stamp");
        assert_eq!(rig.status("c1"), Some(SessionStatus::Working));
        assert_eq!(rig.live_run("c1"), Some(run_id.clone()));

        frozen.thaw();
        assert_eq!(
            rig.sessions().steer("c1", "continue", None).await.unwrap(),
            SteerOutcome::Accepted
        );
        rig.until_said("c1", "continued").await;
        assert_eq!(rig.live_run("c1"), Some(run_id), "the same run carried on");
        rig.sessions().shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn nothing_starts_steers_interrupts_or_answers_while_frozen() {
        let rig = rig();
        rig.start("c1", "hold").await;
        rig.until_said("c1", "working").await;
        let frozen = rig.sessions().freeze_runs().await.unwrap();

        let refused = rig
            .sessions()
            .dispatch("c2", HarnessId::Mock, request("hi"), None)
            .await
            .unwrap_err();
        assert!(refused.to_string().contains("updating"), "{refused}");
        assert_eq!(rig.live_run("c2"), None);
        assert_eq!(
            rig.sessions().steer("c1", "late", None).await.unwrap(),
            SteerOutcome::DeferredByUpdate,
            "a steer is held, not lost behind the snapshot"
        );
        assert!(
            rig.sessions()
                .dispatch("c1", HarnessId::Mock, request("routed"), None)
                .await
                .is_err()
        );
        assert!(rig.sessions().interrupt("c1").await.is_err());
        assert!(
            rig.sessions()
                .respond_input("c1", "any", Vec::new())
                .is_err()
        );
        assert!(
            rig.sessions().freeze_runs().await.is_err(),
            "no second freeze"
        );

        drop(frozen); // dropping thaws
        rig.start("c2", "hi").await;
        rig.until_said("c2", "reply: hi").await;
        rig.sessions().shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn unsafe_states_are_busy_not_frozen() {
        for (prompt, reason) in [
            ("busy inline", "an inline request is in flight"),
            ("busy held", "a turn end is being held"),
            ("busy setup", "the agent is still starting up"),
        ] {
            let rig = rig();
            rig.start("c1", prompt).await;
            rig.until_said("c1", "busy").await;
            assert!(rig.sessions().unfreezable_runs().is_empty(), "{prompt}");
            let err = rig.sessions().freeze_runs().await.unwrap_err().to_string();
            assert!(
                err.contains("busy") && err.contains(reason),
                "{prompt}: {err}"
            );
            let vetoes = rig.sessions().unfreezable_runs();
            assert_eq!(vetoes.len(), 1, "{prompt}: {vetoes:?}");
            assert!(vetoes[0].contains(reason), "{vetoes:?}");
            // The refusal lasts until the run moves on.
            rig.sessions().steer("c1", "continue", None).await.unwrap();
            rig.until_said("c1", "continued").await;
            eventually("the veto clears", || {
                rig.sessions().unfreezable_runs().is_empty()
            })
            .await;
            rig.sessions().shutdown().await;
        }

        // Interrupting (the engine knows this one itself).
        let rig = rig();
        rig.start("c1", "slowstop").await;
        rig.until_said("c1", "working").await;
        let sessions = rig.sessions().clone();
        let stopping = tokio::spawn(async move { sessions.interrupt("c1").await });
        eventually("the interrupt is under way", || {
            !rig.sessions().unfreezable_runs().is_empty()
        })
        .await;
        assert!(rig.sessions().unfreezable_runs()[0].contains("interrupted"));
        let err = rig.sessions().freeze_runs().await.unwrap_err().to_string();
        assert!(err.contains("busy") && err.contains("interrupted"), "{err}");
        assert!(stopping.await.unwrap().unwrap());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_freeze_is_all_or_nothing() {
        let rig = rig();
        rig.start("fine", "hold").await;
        rig.start("stuck", "busy inline").await;
        rig.until_said("fine", "working").await;
        rig.until_said("stuck", "busy").await;
        let err = rig.sessions().freeze_runs().await.unwrap_err().to_string();
        assert!(err.contains("stuck") && !err.contains("fine"), "{err}");
        assert!(!rig.sessions().handoff_frozen());
        // The run that did freeze was thawed and carries on.
        rig.sessions()
            .steer("fine", "continue", None)
            .await
            .unwrap();
        rig.until_said("fine", "continued").await;
        rig.sessions().shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_adopted_run_is_not_recovered_as_stale_and_keeps_its_child() {
        let old = rig();
        let run_id = old.start("c1", "hold").await;
        old.until_said("c1", "working").await;
        let (new, _old_image, runs) = old.hand_over().await;
        let pid = runs[0].harness.pid;

        assert_eq!(
            new.live_run("c1"),
            Some(run_id.clone()),
            "registered before recovery"
        );
        assert!(!new.ends_in_done("c1"), "no stale-run recovery Done");
        assert_eq!(new.aborted("c1"), 0);
        assert_eq!(
            new.sessions().inner.journal.resume_attempts("c1"),
            0,
            "no auto-resume"
        );
        assert_eq!(new.status("c1"), Some(SessionStatus::Working));
        assert!(alive(pid));

        new.sessions().steer("c1", "continue", None).await.unwrap();
        new.until_said("c1", "continued").await;
        assert_eq!(new.live_run("c1"), Some(run_id));
        assert!(alive(pid), "the same child answered");
        // One assistant entry across both images: the adopted run kept
        // writing into the segment the predecessor opened.
        let assistant: Vec<_> = new
            .entries("c1")
            .into_iter()
            .filter(|entry| entry.role == MessageRole::Assistant)
            .collect();
        assert!(
            assistant[0].parts.iter().any(
                |part| matches!(part, MessagePart::Text { text, .. } if text.contains("working"))
            ),
            "{assistant:?}"
        );
        new.sessions().shutdown().await;
        eventually("the adopted child is reaped", || !alive(pid)).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_parked_question_answers_under_the_same_request_id_with_no_duplicate_prompt() {
        let old = rig();
        old.start("c1", "ask").await;
        eventually("the question is asked", || {
            old.input_request("c1").is_some()
        })
        .await;
        eventually("awaiting input", || {
            old.status("c1") == Some(SessionStatus::AwaitingInput)
        })
        .await;
        let request_id = old.input_request("c1").unwrap();
        let (new, _old_image, runs) = old.hand_over().await;
        assert_eq!(runs[0].pending_inputs.len(), 1);
        assert_eq!(runs[0].pending_inputs[0].request_id, request_id);
        assert_eq!(new.status("c1"), Some(SessionStatus::AwaitingInput));

        assert!(
            new.sessions()
                .respond_input("c1", &request_id, yes("q-1"))
                .unwrap(),
            "the adopted run knows the original request id"
        );
        new.until_said("c1", "you said yes").await;
        let asked = new
            .journal("c1")
            .iter()
            .filter(|event| matches!(event, AgentEvent::InputRequested { .. }))
            .count();
        assert_eq!(asked, 1, "no second InputRequested");
        let inputs = new
            .entries("c1")
            .iter()
            .flat_map(|entry| entry.parts.clone())
            .filter(|part| matches!(part, MessagePart::Input { .. }))
            .count();
        assert_eq!(inputs, 1, "no duplicate question chip");
        new.sessions().shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn queued_steers_are_not_dispatched_as_new_turns_after_adoption() {
        let old = rig();
        let run_id = old.start("c1", "ask").await;
        eventually("asked", || old.input_request("c1").is_some()).await;
        let request_id = old.input_request("c1").unwrap();
        // The harness does not read its mailbox while a question is parked:
        // this steer is still in it when the freeze comes.
        assert_eq!(
            old.sessions()
                .steer("c1", "later", Some("m-later".into()))
                .await
                .unwrap(),
            SteerOutcome::Accepted
        );
        let (new, _old_image, runs) = old.hand_over().await;
        assert_eq!(runs[0].undrained_steers.len(), 1);
        assert_eq!(runs[0].routed_steers.len(), 1);

        new.sessions()
            .respond_input("c1", &request_id, yes("q-1"))
            .unwrap();
        new.until_said("c1", "reply: later").await;
        assert_eq!(
            new.live_run("c1"),
            Some(run_id),
            "delivered to the same run"
        );
        let ledger = lock(&new.sessions().inner.runs)
            .get("c1")
            .map(|handle| lock(&handle.routed_steers).len());
        assert_eq!(ledger, Some(0), "confirmed by its Steered boundary");
        let copies = new
            .entries("c1")
            .iter()
            .filter(|entry| entry.id == "m-later")
            .count();
        assert_eq!(copies, 1, "the steer's user message exists once");
        new.sessions().shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn adopting_a_run_does_not_look_like_a_config_change() {
        let old = rig();
        let run_id = old.start("c1", "hold").await;
        old.until_said("c1", "working").await;
        let (new, _old_image, runs) = old.hand_over().await;
        // The same configuration as the run's: routed into the adopted run,
        // not an interrupt and a restart.
        let routed = new
            .sessions()
            .dispatch("c1", HarnessId::Mock, request("continue"), None)
            .await
            .unwrap();
        assert_eq!(routed, run_id);
        new.until_said("c1", "continued").await;
        assert!(alive(runs[0].harness.pid));
        assert!(
            !new.journal("c1").iter().any(|event| matches!(
                event,
                AgentEvent::Done {
                    status: DoneStatus::Interrupted,
                    ..
                }
            )),
            "never interrupted"
        );
        new.sessions().shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_run_that_cannot_be_adopted_takes_the_crash_path() {
        let old = rig();
        old.start("c1", "hold").await;
        old.until_said("c1", "working").await;
        let runs = old.sessions().freeze_runs().await.unwrap().commit();
        let pid = runs[0].harness.pid;
        let (dir, lock_fd, old_image) = retire(old).await;
        let _old_image = old_image.with(runs.clone());
        // The successor's mock cannot adopt.
        let profile = EngineProfile::local(dir.path()).unwrap();
        let lock = InstanceLock::adopt(lock_fd, profile.device_root()).unwrap();
        let core = EngineCore::assemble_with_profile_adopting(
            profile,
            registry(Arc::new(zeron_harness::mock::MockHarness {
                script: Vec::new(),
            })),
            HarnessId::Mock,
            None,
            lock,
            AdoptedState {
                terminals: Vec::new(),
                runs,
            },
        )
        .unwrap();
        let new = Rig { dir, core };
        assert!(new.ends_in_done("c1"), "stale-run recovery settled it");
        assert!(new.journal("c1").iter().any(|event| matches!(
            event,
            AgentEvent::Done { error: Some(error), .. } if error.contains("engine restart")
        )));
        eventually("the orphaned child is stopped", || !alive(pid)).await;
        new.sessions().shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_engine_handoff_carries_the_run_and_a_failed_exec_thaws_it() {
        use std::os::unix::fs::PermissionsExt;
        let rig = rig();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        rig.core.set_handoff_listener(listener.as_raw_fd());
        let exe = rig.dir.path().join("zeron-next");
        std::fs::write(
            &exe,
            format!(
                "#!/bin/sh\necho handoff-ok {}\n",
                crate::handoff::MANIFEST_VERSION
            ),
        )
        .unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        rig.start("c1", "hold").await;
        rig.until_said("c1", "working").await;

        let seen = std::cell::RefCell::new(None);
        let outcome = rig
            .core
            .handoff_with(&exe, |_, manifest_fd, _| {
                let manifest = crate::handoff::Manifest::read_fd(manifest_fd).unwrap();
                assert_eq!(manifest.runs.len(), 1);
                let run = manifest.runs[0].clone();
                assert_eq!(run.chat_id, "c1");
                for fd in run.fds() {
                    assert!(!cloexec(fd), "agent pipe {fd} is carried");
                }
                *seen.borrow_mut() = Some(run);
                std::io::Error::other("simulated exec failure")
            })
            .await;
        assert!(
            matches!(outcome, crate::handoff::HandoffError::Exec(_)),
            "{outcome}"
        );
        let run = seen.into_inner().expect("the exec closure ran");
        for fd in run.fds() {
            assert!(cloexec(fd), "agent pipe {fd} is close-on-exec again");
        }
        assert!(!rig.sessions().handoff_frozen());
        rig.sessions().steer("c1", "continue", None).await.unwrap();
        rig.until_said("c1", "continued").await;
        rig.sessions().shutdown().await;
    }

    /// A harness without adoption whose run stays open until interrupted.
    struct QuietHarness {
        feed: Mutex<Option<mpsc::UnboundedReceiver<AgentEvent>>>,
    }

    #[async_trait::async_trait]
    impl Harness for QuietHarness {
        fn id(&self) -> HarnessId {
            HarnessId::Mock
        }
        fn display_name(&self) -> &str {
            "Quiet"
        }
        fn supports_steering(&self) -> bool {
            true
        }
        fn steering_mode(&self) -> zeron_proto::SteeringMode {
            zeron_proto::SteeringMode::StepBoundary
        }
        fn reasoning_levels(&self) -> &[zeron_proto::ReasoningLevel] {
            &[zeron_proto::ReasoningLevel::Medium]
        }
        fn deterministic_turn_end(&self) -> bool {
            true
        }
        async fn models(&self) -> Result<Vec<zeron_proto::Model>, zeron_harness::HarnessError> {
            Ok(vec![])
        }
        async fn run(
            &self,
            _request: RunRequest,
            controls: RunControls,
        ) -> Result<
            futures::stream::BoxStream<'static, Result<AgentEvent, zeron_harness::HarnessError>>,
            zeron_harness::HarnessError,
        > {
            let feed = lock(&self.feed).take().expect("one run per test");
            let token = controls.interrupt.clone();
            Ok(futures::stream::unfold(
                (feed, token, controls),
                |(mut feed, token, controls)| async move {
                    tokio::select! {
                        event = feed.recv() => event.map(|event| (Ok(event), (feed, token, controls))),
                        _ = token.cancelled() => None,
                    }
                },
            )
            .boxed())
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_harness_without_adoption_is_busy_mid_turn_and_retired_when_parked() {
        let (feed, rx) = mpsc::unbounded_channel();
        let rig = rig_with(Arc::new(QuietHarness {
            feed: Mutex::new(Some(rx)),
        }));
        rig.start("c1", "go").await;
        eventually("the run starts", || {
            lock(&rig.sessions().inner.runs)
                .get("c1")
                .is_some_and(|handle| handle.started.load(std::sync::atomic::Ordering::SeqCst))
        })
        .await;
        let vetoes = rig.sessions().unfreezable_runs();
        assert_eq!(vetoes.len(), 1, "{vetoes:?}");
        assert!(vetoes[0].contains("cannot hand a run over"), "{vetoes:?}");
        assert!(rig.sessions().freeze_runs().await.is_err());
        assert!(
            rig.live_run("c1").is_some(),
            "a busy freeze touches nothing"
        );

        feed.send(AgentEvent::TextDelta {
            text: "done".into(),
        })
        .unwrap();
        feed.send(AgentEvent::Done {
            status: DoneStatus::Completed,
            result: None,
            error: None,
            session_id: Some("s1".into()),
        })
        .unwrap();
        eventually("parked", || rig.status("c1") == Some(SessionStatus::Idle)).await;
        assert!(rig.sessions().unfreezable_runs().is_empty());
        let frozen = rig
            .sessions()
            .freeze_runs()
            .await
            .expect("a parked run is retired");
        assert!(frozen.handoffs().is_empty(), "nothing to carry");
        assert_eq!(
            rig.live_run("c1"),
            None,
            "retired through the ordinary path"
        );
        assert_eq!(rig.aborted("c1"), 0, "the finished turn stays complete");
        frozen.thaw();
    }

    // Review fix 1: a steer accepted into a parked run of a harness without
    // adoption, in the window before `steer()` flips the status to Working,
    // must make the freeze busy — retiring the run would drop the ledger.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_steer_accepted_into_a_parked_run_without_adoption_makes_the_freeze_busy() {
        let (feed, rx) = mpsc::unbounded_channel();
        let rig = rig_with(Arc::new(QuietHarness {
            feed: Mutex::new(Some(rx)),
        }));
        rig.start("c1", "go").await;
        feed.send(AgentEvent::Done {
            status: DoneStatus::Completed,
            result: None,
            error: None,
            session_id: Some("s1".into()),
        })
        .unwrap();
        eventually("parked", || rig.status("c1") == Some(SessionStatus::Idle)).await;
        // The state `steer()` leaves between its ledger push and its status
        // write: accepted, still Idle.
        let ledger = lock(&rig.sessions().inner.runs)
            .get("c1")
            .map(|handle| handle.routed_steers.clone())
            .unwrap();
        lock(&ledger).push_back(RoutedSteer {
            prompt: "late".into(),
            message_id: "m-late".into(),
            fork_history: false,
        });
        assert_eq!(rig.status("c1"), Some(SessionStatus::Idle));
        assert!(
            !rig.sessions().unfreezable_runs().is_empty(),
            "an owed steer vetoes"
        );
        let err = rig.sessions().freeze_runs().await.unwrap_err().to_string();
        assert!(err.contains("busy"), "{err}");
        assert!(rig.live_run("c1").is_some(), "not retired");
        assert_eq!(lock(&ledger).len(), 1, "the steer is still owed");
    }

    // Review fix 2: both recovery paths (stale-run recovery at boot and a
    // failed adoption) may reach the same chat; it is recovered once.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_chat_is_recovered_once_however_many_paths_reach_it() {
        let rig = rig();
        rig.sessions().inner.publish(
            "c9",
            &AgentEvent::TextDelta {
                text: "cut off".into(),
            },
        );
        rig.sessions().recover_chat("c9".into()).unwrap();
        rig.sessions().recover_chat("c9".into()).unwrap();
        let dones = rig
            .journal("c9")
            .iter()
            .filter(|event| matches!(event, AgentEvent::Done { .. }))
            .count();
        assert_eq!(dones, 1);
    }

    /// Freeze, commit and retire `old`; hand back what a successor needs.
    async fn retire_frozen(old: Rig) -> (TempDir, RawFd, OldImage, Vec<RunHandoff>) {
        let runs = old.sessions().freeze_runs().await.unwrap().commit();
        let (dir, lock_fd, old_image) = retire(old).await;
        let old_image = old_image.with(runs.clone());
        (dir, lock_fd, old_image, runs)
    }

    fn successor(
        dir: TempDir,
        lock_fd: RawFd,
        registry: Arc<HarnessRegistry>,
        runs: Vec<RunHandoff>,
    ) -> Rig {
        let profile = EngineProfile::local(dir.path()).unwrap();
        let lock = InstanceLock::adopt(lock_fd, profile.device_root()).unwrap();
        let core = EngineCore::assemble_with_profile_adopting(
            profile,
            registry,
            HarnessId::Mock,
            None,
            lock,
            AdoptedState {
                terminals: Vec::new(),
                runs,
            },
        )
        .unwrap();
        Rig { dir, core }
    }

    fn restart_notes(rig: &Rig, chat: &str) -> usize {
        rig.journal(chat)
            .iter()
            .filter(|event| {
                matches!(
                    event,
                    AgentEvent::Done { error: Some(error), .. } if error.contains("engine restart")
                )
            })
            .count()
    }

    // Review fix 2: `Harness::adopt` failing in the run task races boot
    // recovery on a multi-thread runtime; the chat is settled and resumed
    // exactly once, and only after the orphaned agent is gone.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn an_adoption_that_fails_late_is_recovered_exactly_once() {
        for round in 0..5 {
            let old = rig();
            old.start("c1", "hold").await;
            old.until_said("c1", "working").await;
            let (dir, lock_fd, _old_image, mut runs) = retire_frozen(old).await;
            let pid = runs[0].harness.pid;
            // The mock refuses an unknown state version inside `adopt`.
            runs[0].harness.state_version = 99;
            let new = successor(dir, lock_fd, registry(Arc::new(FreezableMock)), runs);
            eventually("recovered", || restart_notes(&new, "c1") >= 1).await;
            eventually("auto-resumed", || {
                new.said("c1", "working") && new.live_run("c1").is_some()
            })
            .await;
            assert!(
                !alive(pid),
                "round {round}: the old agent was stopped before the resume"
            );
            tokio::time::sleep(Duration::from_millis(300)).await;
            assert_eq!(
                restart_notes(&new, "c1"),
                1,
                "round {round}: recovered once"
            );
            assert_eq!(
                new.sessions().inner.journal.resume_attempts("c1"),
                1,
                "round {round}: resumed once"
            );
            new.sessions().shutdown().await;
        }
    }

    // The adoption commit closes the inherited originals as soon as boot is
    // done, but a run whose execution lease is delayed (a harness update
    // pending) adopts later: the engine duplicated the descriptors when the run
    // was registered, so the harness still finds them open.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_delayed_adoption_still_finds_its_descriptors_after_the_originals_close() {
        let old = rig();
        old.start("c1", "hold").await;
        old.until_said("c1", "working").await;
        let (dir, lock_fd, old_image, runs) = retire_frozen(old).await;
        let pid = runs[0].harness.pid;
        let registry = registry(Arc::new(FreezableMock));
        registry.begin_update(HarnessId::Mock); // the lease is held: adoption waits
        let new = successor(dir, lock_fd, registry.clone(), runs);
        assert!(new.live_run("c1").is_some());
        // The adoption commit: the inherited originals are closed.
        drop(old_image);
        tokio::time::sleep(Duration::from_millis(200)).await;
        registry.end_update(HarnessId::Mock);
        // The run adopts now and carries on with the same child.
        new.sessions().steer("c1", "continue", None).await.unwrap();
        new.until_said("c1", "continued").await;
        assert!(alive(pid), "the same child answered");
        assert_eq!(
            new.sessions().inner.journal.resume_attempts("c1"),
            0,
            "not the recovery path"
        );
        new.sessions().shutdown().await;
        eventually("the adopted child is reaped", || !alive(pid)).await;
    }

    // Review fix 2: a Stop while the adopted run still waits for its
    // execution lease settles it as interrupted — no crash path, no resume.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_interrupt_before_adoption_completes_is_a_stop() {
        let old = rig();
        old.start("c1", "hold").await;
        old.until_said("c1", "working").await;
        let (dir, lock_fd, _old_image, runs) = retire_frozen(old).await;
        let pid = runs[0].harness.pid;
        let registry = registry(Arc::new(FreezableMock));
        // An accepted harness update holds the lease: adoption waits.
        registry.begin_update(HarnessId::Mock);
        let new = successor(dir, lock_fd, registry.clone(), runs);
        assert!(new.live_run("c1").is_some());
        assert!(new.sessions().interrupt("c1").await.unwrap());
        eventually("settled", || new.live_run("c1").is_none()).await;
        registry.end_update(HarnessId::Mock);
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(
            matches!(
                new.journal("c1").last(),
                Some(AgentEvent::Done {
                    status: DoneStatus::Interrupted,
                    error: None,
                    ..
                })
            ),
            "{:?}",
            new.journal("c1").last()
        );
        assert_eq!(restart_notes(&new, "c1"), 0, "not the crash path");
        assert_eq!(
            new.sessions().inner.journal.resume_attempts("c1"),
            0,
            "no auto-resume"
        );
        assert_eq!(new.live_run("c1"), None);
        assert_eq!(
            new.aborted("c1"),
            1,
            "the open segment is stamped like any interrupt"
        );
        assert!(!alive(pid), "the agent was stopped");
    }

    // Review minor (a): engine-initiated dispatches (orphaned steers, the
    // startup retry) wait out a freeze instead of failing.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_engine_initiated_dispatch_waits_out_the_freeze() {
        let rig = rig();
        rig.start("c1", "hold").await;
        rig.until_said("c1", "working").await;
        let frozen = rig.sessions().freeze_runs().await.unwrap();
        assert!(
            !rig.sessions()
                .wait_for_thaw(Duration::from_millis(100))
                .await
        );
        let sessions = rig.sessions().clone();
        let waiting = tokio::spawn(async move {
            sessions.wait_for_thaw(Duration::from_secs(10)).await
                && sessions
                    .dispatch("c2", HarnessId::Mock, request("hi"), None)
                    .await
                    .is_ok()
        });
        tokio::time::sleep(Duration::from_millis(200)).await;
        frozen.thaw();
        assert!(waiting.await.unwrap(), "dispatched once thawed");
        rig.until_said("c2", "reply: hi").await;
        rig.sessions().shutdown().await;
    }

    // Review minor (b): a question id shared by two parked sets never picks
    // one at random; the engine request id is exact.
    #[tokio::test]
    async fn rebinding_prefers_the_request_id_and_never_guesses() {
        use tokio::sync::oneshot::error::TryRecvError;
        let pending: PendingInputs = Arc::new(Mutex::new(HashMap::new()));
        let (engine_tx, _engine_rx) = mpsc::unbounded_channel();
        let question = |id: &str| UserInputQuestion {
            id: id.into(),
            header: String::new(),
            question: String::new(),
            options: Vec::new(),
            prefill: None,
            multiline: false,
            multi_select: false,
        };
        let mut slots = HashMap::new();
        let mut records = Vec::new();
        let mut resolvers = HashMap::new();
        for request_id in ["r1", "r2"] {
            let (tx, rx) = oneshot::channel();
            resolvers.insert(request_id, tx);
            slots.insert(request_id.to_string(), rx);
            records.push(PendingInputRecord {
                request_id: request_id.into(),
                questions: vec![question("shared")],
            });
        }
        let rebind = rebind_bridge(pending, engine_tx, slots, records);
        let mut ambiguous = rebind("shared".into());
        tokio::task::yield_now().await;
        assert_eq!(ambiguous.try_recv(), Err(TryRecvError::Closed), "no guess");
        let mut exact = rebind("r1".into());
        // r1 is taken: the shared id now names r2 alone.
        let mut unique = rebind("shared".into());
        resolvers.remove("r1").unwrap().send(yes("shared")).unwrap();
        resolvers.remove("r2").unwrap().send(yes("shared")).unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(exact.try_recv().is_ok());
        assert!(unique.try_recv().is_ok());
        assert_eq!(
            rebind("r1".into()).try_recv(),
            Err(TryRecvError::Closed),
            "a slot rebinds once"
        );
    }

    // Review minor (c): commands that arrive while frozen stay pending and
    // are applied after the thaw — nobody has to resend or re-answer.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn commands_that_arrive_while_frozen_are_applied_after_the_thaw() {
        let rig = rig();
        rig.start("c1", "ask").await;
        eventually("asked", || rig.input_request("c1").is_some()).await;
        let request_id = rig.input_request("c1").unwrap();
        let frozen = rig.sessions().freeze_runs().await.unwrap();
        let handle = rig.core.doc_host.open("c1").unwrap();
        handle
            .doc()
            .queue_command(&zeron_doc::SessionCommandEntry {
                id: "cmd-answer".into(),
                payload: zeron_doc::SessionCommandPayload::RespondInput {
                    request_id,
                    answers: yes("q-1"),
                },
                issued_by: "viewer".into(),
                issued_at: now_ms(),
                based_on: None,
                expires_at: None,
                status: zeron_doc::SessionCommandStatus::Pending,
                resolution: None,
            })
            .unwrap();
        let status = |handle: &Arc<ChatDocHandle>| {
            handle
                .doc()
                .read_commands()
                .unwrap()
                .into_iter()
                .find(|command| command.id == "cmd-answer")
                .map(|command| command.status)
        };
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert_eq!(
            status(&handle),
            Some(zeron_doc::SessionCommandStatus::Pending),
            "deferred, not rejected"
        );
        frozen.thaw();
        rig.until_said("c1", "you said yes").await;
        eventually("applied", || {
            status(&handle) == Some(zeron_doc::SessionCommandStatus::Applied)
        })
        .await;
        rig.sessions().shutdown().await;
    }

    /// An adoptable harness whose stream ends right after it froze — as when
    /// one of its tasks notices the child died while the run is frozen.
    struct EndsWhileFrozen;

    #[async_trait::async_trait]
    impl Harness for EndsWhileFrozen {
        fn id(&self) -> HarnessId {
            HarnessId::Mock
        }
        fn display_name(&self) -> &str {
            "Ends"
        }
        fn supports_steering(&self) -> bool {
            true
        }
        fn steering_mode(&self) -> zeron_proto::SteeringMode {
            zeron_proto::SteeringMode::StepBoundary
        }
        fn reasoning_levels(&self) -> &[zeron_proto::ReasoningLevel] {
            &[zeron_proto::ReasoningLevel::Medium]
        }
        fn deterministic_turn_end(&self) -> bool {
            true
        }
        fn supports_adoption(&self) -> bool {
            true
        }
        async fn models(&self) -> Result<Vec<zeron_proto::Model>, zeron_harness::HarnessError> {
            Ok(vec![])
        }
        async fn run(
            &self,
            _request: RunRequest,
            mut controls: RunControls,
        ) -> Result<
            futures::stream::BoxStream<'static, Result<AgentEvent, zeron_harness::HarnessError>>,
            zeron_harness::HarnessError,
        > {
            let (tx, rx) = mpsc::unbounded_channel();
            let _ = tx.send(AgentEvent::TextDelta {
                text: "working".into(),
            });
            tokio::spawn(async move {
                let Some(request) = controls.freeze.recv().await else {
                    return;
                };
                let (commit, _verdict) = oneshot::channel();
                let _ = request.reply.send(Ok(zeron_harness::FrozenRun {
                    handoff: HarnessHandoff {
                        harness: HarnessId::Mock,
                        state_version: 1,
                        pid: -1,
                        stdin_fd: -1,
                        stdout_fd: -1,
                        stderr_fd: None,
                        extra_fds: Vec::new(),
                        stdout_leftover: Vec::new(),
                        stderr_tail: Vec::new(),
                        state: serde_json::Value::Null,
                        undrained_steers: Vec::new(),
                    },
                    commit,
                }));
                tokio::time::sleep(Duration::from_millis(50)).await;
                drop(tx); // the stream ends while frozen
                drop(controls);
            });
            Ok(futures::stream::unfold(rx, |mut rx| async move {
                rx.recv().await.map(|event| (Ok(event), rx))
            })
            .boxed())
        }
    }

    // Review minor (e): a run that ends between the snapshot and the exec is
    // not settled by the old image (the manifest already hands it over): a
    // thaw settles it once, a commit leaves it to the successor.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_run_that_ends_while_frozen_is_settled_only_after_the_verdict() {
        let dones = |rig: &Rig| {
            rig.journal("c1")
                .iter()
                .filter(|event| matches!(event, AgentEvent::Done { .. }))
                .count()
        };
        for commit in [false, true] {
            let rig = rig_with(Arc::new(EndsWhileFrozen));
            rig.start("c1", "go").await;
            rig.until_said("c1", "working").await;
            let frozen = rig.sessions().freeze_runs().await.unwrap();
            tokio::time::sleep(Duration::from_millis(300)).await;
            assert_eq!(dones(&rig), 0, "commit={commit}: not settled while frozen");
            assert!(rig.live_run("c1").is_some());
            if commit {
                frozen.commit();
                tokio::time::sleep(Duration::from_millis(300)).await;
                assert_eq!(dones(&rig), 0, "handed over: the successor settles it");
                assert!(rig.live_run("c1").is_some());
            } else {
                frozen.thaw();
                eventually("settled after the thaw", || rig.live_run("c1").is_none()).await;
                assert_eq!(dones(&rig), 1, "settled once");
            }
        }
    }
}
