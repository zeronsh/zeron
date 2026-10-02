//! Goal mode on the doc host: command execution, the controller tick, and
//! verifier runs. The decisions themselves are pure (`crate::goal`); this
//! module gathers observations, performs the chosen action, and persists the
//! goal in the chat's session doc.
//!
//! Only the chat's host runs any of it — `goal_tick` and the command arm both
//! gate on `is_host`, exactly like the queue drain.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::Duration;

use zeron_proto::{
    Goal, GoalCommand, GoalError, GoalEventKind, GoalReasonKind, GoalStatus, MessageOrigin,
    SessionStatus, agent_goal_permission,
};

use super::*;
use crate::ask::{AskBackend, AskError, AskFailure, AskOutcome, AskSpec};
use crate::goal::{
    Action, Next, Observation, RoundFacts, RoundOutcome, VERIFIER_TIMEOUT, VerifierVerdict,
    apply_stop, apply_verdict, decide, restart_grace, round_prompt, tool_changes_state,
    turn_pending, verdict_schema, verifier_prompt, verify_pending,
};

/// A verification of one round of one goal, from the moment the controller
/// commits to it (reserved under `goal_lock` together with the `Verifying`
/// ledger write) until its verdict has been applied (released under the same
/// lock). The entry is the "in flight or verdict pending apply" marker that
/// makes starting a verification idempotent per `(goal, round)`.
pub(super) struct GoalRun {
    pub goal_id: String,
    pub round: u32,
    /// Tells this run apart from a later one for the same chat, so a stale
    /// finisher never releases its successor's entry.
    pub run_id: String,
    pub cancel: CancellationToken,
}

/// Device-local list of chats whose goal is running: how a restarted host
/// finds goals to resume without opening every chat doc. Not synced — only
/// the chat's host runs the controller.
pub(super) struct GoalIndex {
    path: PathBuf,
    ids: Mutex<BTreeSet<String>>,
}

impl GoalIndex {
    pub(super) fn open(path: PathBuf) -> Self {
        let ids = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<BTreeSet<String>>(&bytes).ok())
            .unwrap_or_default();
        Self {
            path,
            ids: Mutex::new(ids),
        }
    }

    fn set(&self, chat_id: &str, running: bool) {
        let mut ids = lock(&self.ids);
        let changed = if running {
            ids.insert(chat_id.to_owned())
        } else {
            ids.remove(chat_id)
        };
        if !changed {
            return;
        }
        let Ok(bytes) = serde_json::to_vec(&*ids) else {
            return;
        };
        let tmp = self.path.with_extension("tmp");
        if std::fs::write(&tmp, bytes)
            .and_then(|()| std::fs::rename(&tmp, &self.path))
            .is_err()
        {
            tracing::warn!(path = %self.path.display(), "goal index write failed");
        }
    }

    fn all(&self) -> Vec<String> {
        lock(&self.ids).iter().cloned().collect()
    }
}

type CommandOutcome = Result<(SessionCommandStatus, Option<String>), EngineError>;

impl DocHost {
    // ── wiring ─────────────────────────────────────────────────────────────

    /// Wire the child-ask runner (engine assembly).
    pub fn set_asks(&self, asks: crate::ask::AskService) {
        *lock(&self.inner.asks) = Some(asks);
    }

    pub fn asks(&self) -> Option<crate::ask::AskService> {
        lock(&self.inner.asks).clone()
    }

    /// Replace the ask backend: verifications and every other engine-internal
    /// ask go to `backend` instead of spawning child chats. For tests and for
    /// schedulers that script their agents.
    pub fn set_ask_backend(&self, backend: Arc<dyn AskBackend>) {
        *lock(&self.inner.ask_override) = Some(backend);
    }

    /// The backend engine-internal asks run on: the override, else the real
    /// child-chat runner.
    pub fn ask_backend(&self) -> Option<Arc<dyn AskBackend>> {
        if let Some(backend) = lock(&self.inner.ask_override).clone() {
            return Some(backend);
        }
        self.asks()
            .map(|asks| Arc::new(asks) as Arc<dyn AskBackend>)
    }

    /// Wire the device-local goal index (engine assembly) and resume goals
    /// that were running when the engine last stopped.
    pub fn set_goal_index(&self, path: PathBuf) {
        let index = GoalIndex::open(path);
        let ids = index.all();
        if self.inner.goal_index.set(index).is_err() || ids.is_empty() {
            return;
        }
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let host = self.clone();
        self.spawn_worker_on(&runtime, async move {
            // Let crash recovery's auto-resume of interrupted turns land first.
            tokio::time::sleep(restart_grace()).await;
            for chat_id in ids {
                if !host.is_host(&chat_id) {
                    continue;
                }
                match host.open(&chat_id) {
                    Ok(handle) => host.goal_tick(&handle).await,
                    Err(err) => {
                        tracing::warn!(chat = %chat_id, error = %err, "goal recovery: open failed");
                    }
                }
            }
        });
    }

    /// The chat's goal, if it has one.
    pub fn goal(&self, chat_id: &str) -> Option<Goal> {
        self.open(chat_id).ok()?.doc.goal()
    }

    /// Tokens the agent's turn just reported (from the sessions engine).
    pub fn note_turn_usage(&self, chat_id: &str, tokens: u64) {
        let handle = lock(&self.inner.handles).get(chat_id).cloned();
        if let Some(handle) = handle
            && handle.goal_live.load(Ordering::Acquire)
        {
            handle.goal_tokens.fetch_add(tokens, Ordering::AcqRel);
        }
    }

    fn sync_goal_live(&self, handle: &ChatDocHandle, goal: Option<&Goal>) {
        let running = goal.is_some_and(|g| g.status.is_running());
        handle.goal_live.store(running, Ordering::Release);
        if let Some(index) = self.inner.goal_index.get() {
            index.set(&handle.chat_id, running);
        }
    }

    // ── markers ────────────────────────────────────────────────────────────

    /// Append a lifecycle marker (system-role entry) to the transcript.
    /// Idempotent by `id`.
    #[allow(clippy::too_many_arguments)]
    fn goal_marker(
        &self,
        handle: &ChatDocHandle,
        id: String,
        goal: &Goal,
        event: GoalEventKind,
        round: u32,
        title: &str,
        detail: &str,
        verifier_chat_id: Option<String>,
    ) {
        match handle.doc.read_entries() {
            Ok(entries) if entries.iter().any(|e| e.id == id) => return,
            Err(_) => return,
            Ok(_) => {}
        }
        let entry = SessionMessageEntry {
            origin: Some(MessageOrigin::GoalEvent {
                goal_id: goal.id.clone(),
                event,
                round,
                title: title.to_owned(),
                detail: detail.to_owned(),
                verifier_chat_id,
            }),
            id: id.clone(),
            role: MessageRole::System,
            parts: vec![MessagePart::Text {
                id: "t0".into(),
                text: MessageOrigin::goal_event_text(event, round, title, detail),
            }],
            created_at: now_ms(),
            device_id: handle.device_id.clone(),
            status: Some(MessageStatus::Complete),
            continuation_of: None,
            duration_ms: None,
        };
        if let Err(err) = handle.doc.push_message(&entry) {
            tracing::warn!(chat = %handle.chat_id, error = %err, "goal marker write failed");
        }
    }

    fn stop_marker(&self, handle: &ChatDocHandle, goal: &Goal) {
        let Some(reason) = &goal.reason else { return };
        let event = match (goal.status, reason.kind) {
            (GoalStatus::BudgetLimited, _) => GoalEventKind::BudgetLimited,
            (_, GoalReasonKind::VerifierFailed) => return, // its own marker already says so
            _ => GoalEventKind::Paused,
        };
        self.goal_marker(
            handle,
            format!("goal-{}-stop-{}", goal.id, now_ms()),
            goal,
            event,
            goal.iteration,
            &goal.summary_title,
            &reason.message,
            None,
        );
    }

    // ── commands ───────────────────────────────────────────────────────────

    /// Execute a `/goal` mutation. Rejections explain themselves in the
    /// command's resolution.
    pub(super) async fn apply_goal_command(
        &self,
        handle: &Arc<ChatDocHandle>,
        command: &GoalCommand,
        issuer: Option<&str>,
    ) -> CommandOutcome {
        let outcome = {
            let _guard = handle.goal_lock.lock().await;
            self.apply_goal_command_locked(handle, command, issuer)
        };
        self.goal_tick(handle).await;
        outcome
    }

    fn apply_goal_command_locked(
        &self,
        handle: &Arc<ChatDocHandle>,
        command: &GoalCommand,
        issuer: Option<&str>,
    ) -> CommandOutcome {
        let now = now_ms();
        let rejected = |message: String| Ok((SessionCommandStatus::Rejected, Some(message)));
        let current = handle.doc.goal();
        // An agent's tools get a narrower set of goal actions than a person.
        let agent = issuer.filter(|id| !id.trim().is_empty());
        if let Some(agent) = agent
            && let Err(message) =
                agent_goal_permission(command, current.as_ref(), agent, &handle.chat_id)
        {
            tracing::info!(chat = %handle.chat_id, agent, "agent goal command refused: {message}");
            return rejected(message);
        }
        match command {
            GoalCommand::Set {
                objective,
                limits,
                replace,
            } => {
                if let Some(cur) = &current
                    && cur.status != GoalStatus::Complete
                    && cur.status != GoalStatus::BudgetLimited
                    && !replace
                {
                    return rejected(GoalError::AlreadyExists.to_string());
                }
                let mut goal = match Goal::new(new_id(), objective, limits, now) {
                    Ok(goal) => goal,
                    Err(err) => return rejected(err.to_string()),
                };
                goal.set_by_agent = agent.map(str::to_owned);
                if let Some(old) = &current {
                    self.discard_goal_runtime(handle, old);
                }
                handle.doc.set_goal(&goal)?;
                self.sync_goal_live(handle, Some(&goal));
                self.goal_marker(
                    handle,
                    format!("goal-{}-set", goal.id),
                    &goal,
                    GoalEventKind::Set,
                    0,
                    &goal.summary_title,
                    "",
                    None,
                );
                Ok((SessionCommandStatus::Applied, Some("goal set".into())))
            }
            GoalCommand::Pause => {
                let Some(mut goal) = current else {
                    return rejected(GoalError::NoGoal.to_string());
                };
                match goal.status {
                    GoalStatus::Paused => {
                        Ok((SessionCommandStatus::Applied, Some("already paused".into())))
                    }
                    GoalStatus::Active | GoalStatus::Verifying => {
                        self.discard_goal_runtime(handle, &goal);
                        self.fold_turn(handle, &mut goal, now);
                        let by = if agent.is_some() {
                            "Paused by the agent that set it."
                        } else {
                            "Paused by you."
                        };
                        goal.stop(GoalStatus::Paused, GoalReasonKind::User, by);
                        goal.updated_at = now;
                        handle.doc.set_goal(&goal)?;
                        self.sync_goal_live(handle, Some(&goal));
                        self.stop_marker(handle, &goal);
                        Ok((SessionCommandStatus::Applied, Some("goal paused".into())))
                    }
                    other => rejected(GoalError::NotPausable(other).to_string()),
                }
            }
            GoalCommand::Resume => {
                let Some(mut goal) = current else {
                    return rejected(GoalError::NoGoal.to_string());
                };
                match goal.status {
                    GoalStatus::Active | GoalStatus::Verifying => Ok((
                        SessionCommandStatus::Applied,
                        Some("already running".into()),
                    )),
                    GoalStatus::Paused | GoalStatus::BudgetLimited => {
                        // Resuming is the deliberate retry for a queue frozen
                        // by a failed send (the goal paused on it): thaw it so
                        // the stale round prompt either goes out or fails and
                        // pauses the goal again, once.
                        if handle.queue_send_failed.swap(false, Ordering::AcqRel) {
                            handle.queue_paused.store(false, Ordering::Release);
                        }
                        if goal.status == GoalStatus::BudgetLimited {
                            // Resuming past a cap grants another allowance.
                            goal.extensions = goal.extensions.saturating_add(1);
                        }
                        goal.status = GoalStatus::Active;
                        goal.reason = None;
                        goal.pending = None;
                        goal.updated_at = now;
                        handle.doc.set_goal(&goal)?;
                        self.sync_goal_live(handle, Some(&goal));
                        self.goal_marker(
                            handle,
                            format!("goal-{}-resume-{now}", goal.id),
                            &goal,
                            GoalEventKind::Resumed,
                            goal.iteration,
                            &goal.summary_title,
                            "",
                            None,
                        );
                        Ok((SessionCommandStatus::Applied, Some("goal resumed".into())))
                    }
                    other => rejected(GoalError::NotResumable(other).to_string()),
                }
            }
            GoalCommand::Clear => {
                let Some(goal) = current else {
                    return Ok((SessionCommandStatus::Applied, Some("no goal".into())));
                };
                self.discard_goal_runtime(handle, &goal);
                handle.doc.clear_goal()?;
                self.sync_goal_live(handle, None);
                self.goal_marker(
                    handle,
                    format!("goal-{}-cleared", goal.id),
                    &goal,
                    GoalEventKind::Cleared,
                    goal.iteration,
                    &goal.summary_title,
                    "",
                    None,
                );
                Ok((SessionCommandStatus::Applied, Some("goal cleared".into())))
            }
        }
    }

    /// Stop whatever the controller has in flight for `goal`: cancel its
    /// verifier and withdraw a round prompt that has not been delivered.
    fn discard_goal_runtime(&self, handle: &ChatDocHandle, goal: &Goal) {
        if let Some(run) = lock(&self.inner.goal_runs).remove(&handle.chat_id) {
            run.cancel.cancel();
        }
        if let Some(pending) = &goal.pending
            && pending.kind == zeron_proto::GoalPendingKind::Turn
            && handle
                .doc
                .remove_queued(&pending.message_id)
                .unwrap_or(false)
        {
            handle.publish_queue();
        }
    }

    /// The user pressed Stop: the goal stands down (ZCode parity). A verifier
    /// in flight is cancelled; the turn itself is interrupted by the caller.
    pub(super) async fn goal_on_user_interrupt(&self, handle: &Arc<ChatDocHandle>) {
        if !handle.goal_live.load(Ordering::Acquire) {
            return;
        }
        let _guard = handle.goal_lock.lock().await;
        let Some(mut goal) = handle.doc.goal() else {
            return;
        };
        if !goal.status.is_running() {
            return;
        }
        let now = now_ms();
        self.discard_goal_runtime(handle, &goal);
        self.fold_turn(handle, &mut goal, now);
        goal.stop(
            GoalStatus::Paused,
            GoalReasonKind::Interrupted,
            "You stopped the turn. Resume the goal to continue.",
        );
        goal.updated_at = now;
        if let Err(err) = handle.doc.set_goal(&goal) {
            tracing::warn!(chat = %handle.chat_id, error = %err, "goal pause write failed");
            return;
        }
        self.sync_goal_live(handle, Some(&goal));
        self.stop_marker(handle, &goal);
    }

    /// Account the finished (or abandoned) agent turn into the goal.
    fn fold_turn(&self, handle: &ChatDocHandle, goal: &mut Goal, now: i64) {
        goal.tokens_used = goal
            .tokens_used
            .saturating_add(handle.goal_tokens.swap(0, Ordering::AcqRel));
        if let Some(pending) = &goal.pending
            && pending.kind == zeron_proto::GoalPendingKind::Turn
        {
            goal.time_used_seconds = goal
                .time_used_seconds
                .saturating_add((now.saturating_sub(pending.started_at).max(0) / 1000) as u64);
        }
    }

    // ── the controller ─────────────────────────────────────────────────────

    /// One controller step for `handle`'s chat. Idempotent and cheap when
    /// there is nothing to do; called after every doc change, status change,
    /// command and verdict.
    pub async fn goal_tick(&self, handle: &Arc<ChatDocHandle>) {
        if !handle.goal_live.load(Ordering::Acquire) {
            return;
        }
        let Some(sessions) = self.sessions() else {
            return;
        };
        // A streaming turn commits every ~120ms; nothing here can act on it.
        if sessions.turn_in_flight(&handle.chat_id) || !self.is_host(&handle.chat_id) {
            return;
        }
        let recheck = {
            let _guard = handle.goal_lock.lock().await;
            self.goal_step(handle, &sessions)
        };
        match recheck {
            GoalStep::Done => {}
            GoalStep::Drain => self.drain_queue(handle).await,
            GoalStep::Recheck(after) => self.schedule_goal_tick(handle, after),
            GoalStep::Verify(run) => self.spawn_verification(handle, *run),
        }
    }

    fn schedule_goal_tick(&self, handle: &Arc<ChatDocHandle>, after: Duration) {
        let weak = Arc::downgrade(handle);
        let host = self.clone();
        self.spawn_worker(async move {
            tokio::time::sleep(after).await;
            if let Some(handle) = weak.upgrade() {
                host.goal_tick(&handle).await;
            }
        });
    }

    /// Decide and act, under `goal_lock`.
    fn goal_step(&self, handle: &Arc<ChatDocHandle>, sessions: &SessionsEngine) -> GoalStep {
        let Some(mut goal) = handle.doc.goal() else {
            self.sync_goal_live(handle, None);
            return GoalStep::Done;
        };
        if !goal.status.is_running() {
            self.sync_goal_live(handle, Some(&goal));
            return GoalStep::Done;
        }
        let now = now_ms();
        let obs = self.goal_observation(handle, sessions, &goal, now);
        let grace = restart_grace();
        let uptime_grace = self.inner.started.elapsed() < grace;
        let mut action = decide(&goal, &obs);
        // An aborted turn this soon after boot may be about to be revived by
        // crash recovery; look again after the grace instead of pausing.
        if uptime_grace
            && matches!(
                action,
                Action::Stop {
                    kind: GoalReasonKind::Restarted,
                    ..
                }
            )
        {
            action = Action::Recheck(grace);
        }
        match action {
            Action::Wait => GoalStep::Done,
            Action::Recheck(after) => GoalStep::Recheck(after),
            Action::EnqueueRound { round } => {
                goal.iteration = goal.iteration.max(round);
                goal.status = GoalStatus::Active;
                goal.pending = Some(turn_pending(&goal, round, now));
                goal.updated_at = now;
                if let Err(err) = handle.doc.set_goal(&goal) {
                    tracing::warn!(chat = %handle.chat_id, error = %err, "goal ledger write failed");
                    return GoalStep::Done;
                }
                self.enqueue_round_row(handle, &goal, round, now);
                GoalStep::Drain
            }
            Action::BeginVerification { round } | Action::RestartVerification { round } => {
                // One verification per (goal, round): a verdict that has not
                // been applied yet still counts as running.
                if self.goal_run_exists(&handle.chat_id, &goal.id, round) {
                    return GoalStep::Done;
                }
                self.fold_turn(handle, &mut goal, now);
                if let Some(kind) = goal.exhausted_budget() {
                    let stop = crate::goal::decide_budget_stop(&goal, kind);
                    apply_stop(&mut goal, stop);
                    self.finish_stop(handle, &mut goal, now);
                    return GoalStep::Done;
                }
                goal.status = GoalStatus::Verifying;
                goal.pending = Some(verify_pending(&goal, round, now));
                goal.updated_at = now;
                if let Err(err) = handle.doc.set_goal(&goal) {
                    tracing::warn!(chat = %handle.chat_id, error = %err, "goal ledger write failed");
                    return GoalStep::Done;
                }
                // Reserved before `goal_lock` is released and kept until the
                // verdict is applied, so no tick can see the ledger saying
                // "verifying" with nothing running.
                let cancel = CancellationToken::new();
                let run_id = new_id();
                let replaced = lock(&self.inner.goal_runs).insert(
                    handle.chat_id.clone(),
                    GoalRun {
                        goal_id: goal.id.clone(),
                        round,
                        run_id: run_id.clone(),
                        cancel: cancel.clone(),
                    },
                );
                if let Some(old) = replaced {
                    old.cancel.cancel();
                }
                GoalStep::Verify(Box::new(Verification {
                    goal,
                    round,
                    cancel,
                    run_id,
                }))
            }
            stop @ Action::Stop { .. } => {
                self.fold_turn(handle, &mut goal, now);
                apply_stop(&mut goal, stop);
                self.finish_stop(handle, &mut goal, now);
                GoalStep::Done
            }
        }
    }

    fn finish_stop(&self, handle: &ChatDocHandle, goal: &mut Goal, now: i64) {
        goal.updated_at = now;
        if let Err(err) = handle.doc.set_goal(goal) {
            tracing::warn!(chat = %handle.chat_id, error = %err, "goal stop write failed");
            return;
        }
        self.sync_goal_live(handle, Some(goal));
        self.stop_marker(handle, goal);
    }

    /// Queue round `round`'s prompt unless it is already queued or delivered.
    fn enqueue_round_row(&self, handle: &ChatDocHandle, goal: &Goal, round: u32, now: i64) {
        let id = goal.round_message_id(round);
        let queue = handle.doc.read_queue().unwrap_or_default();
        let delivered = handle
            .doc
            .read_entries()
            .map(|entries| entries.iter().any(|e| e.id == id))
            .unwrap_or(false);
        if delivered || queue.iter().any(|row| row.id == id) {
            return;
        }
        let mut row = QueuedMessage::new(
            id,
            round_prompt(goal, round),
            self.inner.config.device_id.clone(),
        );
        row.issued_at = now;
        row.origin = Some(MessageOrigin::Goal {
            goal_id: goal.id.clone(),
            round,
            title: goal.round_title(round),
        });
        match handle.doc.push_queued(&row) {
            Ok(()) => {
                // An empty queue frozen by an earlier Stop is thawed by this
                // deliberate controller action: a goal that is still running
                // was resumed after that Stop. Rows a person queued are never
                // reached — the controller waits for those.
                if queue.is_empty() {
                    handle.queue_paused.store(false, Ordering::Release);
                }
                handle.publish_queue();
            }
            Err(err) => {
                tracing::warn!(chat = %handle.chat_id, error = %err, "goal round enqueue failed");
            }
        }
    }

    fn goal_observation(
        &self,
        handle: &ChatDocHandle,
        sessions: &SessionsEngine,
        goal: &Goal,
        now: i64,
    ) -> Observation {
        let queue = handle.doc.read_queue().unwrap_or_default();
        let pending_turn = goal
            .pending
            .as_ref()
            .filter(|p| p.kind == zeron_proto::GoalPendingKind::Turn);
        let own_row_queued =
            pending_turn.is_some_and(|p| queue.iter().any(|r| r.id == p.message_id));
        // A recovered or Stop-frozen queue holding only the controller's own
        // round prompt would never drain by itself; it is safe to thaw. Not a
        // queue frozen by a failed send: thawing it would retry the same
        // failing send on every tick (each try writes the doc, which ticks
        // again). The controller pauses the goal instead (`round_send_failed`).
        let send_failed = handle.queue_send_failed.load(Ordering::Acquire);
        let paused = handle.queue_paused.load(Ordering::Acquire);
        if own_row_queued && queue.len() == 1 && paused && !send_failed {
            handle.queue_paused.store(false, Ordering::Release);
        }
        let status = sessions.session_status(&handle.chat_id).map(|s| s.status);
        let session_errored = status == Some(SessionStatus::Errored);
        let (round_outcome, subagents_running) = match pending_turn {
            Some(p) if !own_row_queued => self.round_view(handle, &p.message_id, session_errored),
            _ => (RoundOutcome::NotStarted, false),
        };
        Observation {
            now_ms: now,
            turn_in_flight: sessions.turn_in_flight(&handle.chat_id),
            session_errored,
            queue_has_rows: !queue.is_empty(),
            own_row_queued,
            round_send_failed: own_row_queued && paused && send_failed,
            round_outcome,
            subagents_running,
            read_only_chat: self
                .workspace()
                .and_then(|ws| ws.chat_config(&handle.chat_id))
                .is_some_and(|c| c.sandbox == zeron_proto::SandboxLevel::ReadOnly),
            verifier_live: goal
                .pending
                .as_ref()
                .filter(|p| p.kind == zeron_proto::GoalPendingKind::Verify)
                .is_some_and(|p| self.goal_run_exists(&handle.chat_id, &goal.id, p.round)),
        }
    }

    /// A verification of `round` of `goal_id` is in flight or its verdict is
    /// waiting to be applied.
    fn goal_run_exists(&self, chat_id: &str, goal_id: &str, round: u32) -> bool {
        lock(&self.inner.goal_runs)
            .get(chat_id)
            .is_some_and(|run| run.goal_id == goal_id && run.round == round)
    }

    /// How the round that began with user message `message_id` ended, and
    /// whether a subagent it spawned still runs.
    fn round_view(
        &self,
        handle: &ChatDocHandle,
        message_id: &str,
        session_errored: bool,
    ) -> (RoundOutcome, bool) {
        let Ok(entries) = handle.doc.read_entries() else {
            return (RoundOutcome::NotStarted, false);
        };
        let Some(at) = entries.iter().position(|e| e.id == message_id) else {
            return (RoundOutcome::NotStarted, false);
        };
        let after = &entries[at + 1..];
        let assistants = || after.iter().filter(|e| e.role == MessageRole::Assistant);
        let subagents = assistants().any(|e| {
            e.parts.iter().any(|p| {
                matches!(
                    p,
                    MessagePart::Tool {
                        subagent_status: Some(SubagentStatus::Running),
                        ..
                    }
                )
            })
        });
        let outcome = match assistants().next_back() {
            Some(last) if last.status == Some(MessageStatus::Complete) => RoundOutcome::Completed,
            Some(_) => RoundOutcome::Aborted,
            // The prompt landed but no reply: the run died before writing one.
            None if session_errored => RoundOutcome::Aborted,
            None => RoundOutcome::Completed,
        };
        (outcome, subagents)
    }

    fn round_facts(&self, handle: &ChatDocHandle, message_id: &str) -> RoundFacts {
        let Ok(entries) = handle.doc.read_entries() else {
            return RoundFacts::default();
        };
        let Some(at) = entries.iter().position(|e| e.id == message_id) else {
            return RoundFacts::default();
        };
        let changed_state = entries[at + 1..]
            .iter()
            .filter(|e| e.role == MessageRole::Assistant)
            .flat_map(|e| e.parts.iter())
            .any(|p| matches!(p, MessagePart::Tool { call, .. } if tool_changes_state(call)));
        RoundFacts { changed_state }
    }

    // ── verification ───────────────────────────────────────────────────────

    fn spawn_verification(&self, handle: &Arc<ChatDocHandle>, run: Verification) {
        let Verification {
            goal,
            round,
            cancel,
            run_id,
        } = run;
        let host = self.clone();
        let handle = handle.clone();
        self.spawn_worker(async move {
            let result = match host.ask_backend() {
                Some(backend) => {
                    let mut spec = AskSpec::new(
                        "Verifier",
                        verifier_prompt(&goal, &handle.chat_id, round),
                        verdict_schema(),
                    )
                    .read_only()
                    .with_timeout(VERIFIER_TIMEOUT);
                    spec.title = Some(format!("Verifier · round {round} · {}", goal.summary_title));
                    spec.result_description =
                        "Your verdict on whether the goal is complete.".into();
                    backend.ask(&handle.chat_id, spec, cancel.clone()).await
                }
                None => Err(AskFailure::new(AskError::Setup(
                    "child asks are not available in this engine".into(),
                ))),
            };
            host.finish_verification(&handle, &goal.id, round, &run_id, &cancel, result)
                .await;
        });
    }

    /// Apply a verifier's outcome. The run's marker stays registered until
    /// the verdict is written, both under `goal_lock`: a tick that lands
    /// while the verdict is waiting for the lock sees the verification as
    /// still running instead of starting a second one for the round.
    async fn finish_verification(
        &self,
        handle: &Arc<ChatDocHandle>,
        goal_id: &str,
        round: u32,
        run_id: &str,
        cancel: &CancellationToken,
        result: Result<AskOutcome, AskFailure>,
    ) {
        let applied = {
            let _guard = handle.goal_lock.lock().await;
            let applied = self.apply_verification(handle, goal_id, round, cancel, result);
            let mut runs = lock(&self.inner.goal_runs);
            if runs
                .get(&handle.chat_id)
                .is_some_and(|run| run.run_id == run_id)
            {
                runs.remove(&handle.chat_id);
            }
            applied
        };
        if applied {
            self.goal_tick(handle).await;
        }
    }

    /// Under `goal_lock`. True when a verdict was written.
    fn apply_verification(
        &self,
        handle: &Arc<ChatDocHandle>,
        goal_id: &str,
        round: u32,
        cancel: &CancellationToken,
        result: Result<AskOutcome, AskFailure>,
    ) -> bool {
        let Some(mut goal) = handle.doc.goal() else {
            return false;
        };
        let current = goal.status == GoalStatus::Verifying
            && goal.id == goal_id
            && goal.pending.as_ref().is_some_and(|p| {
                p.round == round && p.kind == zeron_proto::GoalPendingKind::Verify
            });
        if !current {
            return false; // paused, cleared or replaced while the verifier ran
        }
        let (outcome, child, usage) = match result {
            Ok(done) => {
                let verdict = serde_json::from_value::<VerifierVerdict>(done.result.clone())
                    .map_err(|e| {
                        AskError::InvalidResult(vec![zeron_proto::SchemaViolation {
                            path: String::new(),
                            message: e.to_string(),
                        }])
                    });
                (verdict, Some(done.child_chat_id), done.usage)
            }
            Err(failure) => {
                // Cancelled by shutdown (not by a user action, which
                // changed the goal): leave it Verifying so recovery
                // judges the round again.
                if failure.error == AskError::Cancelled || cancel.is_cancelled() {
                    return false;
                }
                (Err(failure.error), failure.child_chat_id, failure.usage)
            }
        };
        let now = now_ms();
        let message_id = goal.round_message_id(round);
        let facts = self.round_facts(handle, &message_id);
        let was_failure = outcome.is_err();
        let next = apply_verdict(&mut goal, round, outcome, child.clone(), usage, facts, now);
        if let Err(err) = handle.doc.set_goal(&goal) {
            tracing::warn!(chat = %handle.chat_id, error = %err, "goal verdict write failed");
            return false;
        }
        self.sync_goal_live(handle, Some(&goal));
        if let Some(verdict) = goal.last_verdict() {
            let (event, title) = match verdict.outcome {
                zeron_proto::VerdictOutcome::Pass => {
                    (GoalEventKind::Complete, goal.summary_title.clone())
                }
                zeron_proto::VerdictOutcome::NotSatisfied => (
                    GoalEventKind::NotSatisfied,
                    verdict.next_action.clone().unwrap_or_default(),
                ),
                zeron_proto::VerdictOutcome::Failed => {
                    (GoalEventKind::VerifierFailed, String::new())
                }
                // Only this host's own verdicts reach here; kept total.
                zeron_proto::VerdictOutcome::Unknown => (GoalEventKind::Unknown, String::new()),
            };
            self.goal_marker(
                handle,
                format!("goal-{}-v{round}", goal.id),
                &goal,
                event,
                round,
                &title,
                &verdict.reason,
                verdict.verifier_chat_id.clone(),
            );
        }
        if next == Next::Stopped && !was_failure {
            self.stop_marker(handle, &goal);
        }
        true
    }
}

/// What a controller step asks its caller to do after releasing the lock.
enum GoalStep {
    Done,
    /// Run the queue drain (a round prompt was queued).
    Drain,
    Recheck(Duration),
    /// Run this verification (already registered in `goal_runs`).
    Verify(Box<Verification>),
}

/// A committed verification, handed to the worker that runs it.
struct Verification {
    goal: Goal,
    round: u32,
    cancel: CancellationToken,
    run_id: String,
}
