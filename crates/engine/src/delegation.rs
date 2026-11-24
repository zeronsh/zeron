//! Delegated work, on the parent's host: every turn a chat hands one of its
//! children is a [`DelegatedTask`] kept here (across restarts), its child is
//! watched on whichever device runs it, and the parent is woken once with
//! whatever settled.
//!
//! The parent's own turn ends like any other. While it has tasks whose result
//! it hasn't been told, its completion is held back
//! ([`crate::sessions`]: `Inner::set_status_with_completion`) and its chat
//! reads Waiting, whatever harness runs it: nothing here needs a live
//! runtime, so one that exits between turns waits as well as one that parks.
//!
//! A child is watched by its synced session row, the same for this device's
//! and any other's. A row nobody refreshes says nothing: a child whose device
//! is asleep or offline is still running until its host says otherwise.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use zeron_doc::{MessagePart, MessageRole, MessageStatus, SessionCommandPayload};
use zeron_proto::{
    Chat, CohortDisposition, CompletionWake, DelegatedInput, DelegatedStatus, DelegatedTask, DelegatedTaskView,
    DelegationBaseline, DelegationMode, DeliveryState, DoneStatus, HarnessId, Session, SessionStatus, WaitingKind,
    WaitingOn, WakeItem, WakeKind, WakeNotice, delegation,
};

use crate::doc_host::DocHost;
use crate::sessions::SessionsEngine;
use crate::workspace_host::WorkspaceHost;

/// How much of a child's reply its parent is sent.
const RESULT_LIMIT: usize = 12_000;
/// Tasks told or disposed are kept this long, for `task_status` and the list.
const KEEP_SETTLED: chrono::Duration = chrono::Duration::days(3);
/// And no more of them than this per parent.
const KEEP_PER_PARENT: usize = 100;
/// A Cloud machine stays up for a child heard from this recently.
const HOLDS_MACHINE: chrono::Duration = chrono::Duration::minutes(30);
/// A wake that can't be sent is given up on after this many tries.
const MAX_WAKE_TRIES: u32 = 8;

/// How long things wait (tests shorten it).
#[derive(Debug, Clone, Copy)]
pub struct Timing {
    /// What settles this close together goes in one wake.
    pub coalesce: Duration,
    /// A child's question waits this long for the person before its parent
    /// is asked.
    pub question_grace: Duration,
    /// Between looks while anything is held.
    pub tick: Duration,
    /// How long a settled child's reply is waited for, here and on another
    /// device.
    pub reply_local: Duration,
    pub reply_remote: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            coalesce: Duration::from_millis(800),
            question_grace: Duration::from_secs(5),
            tick: Duration::from_secs(5),
            reply_local: Duration::from_secs(5),
            reply_remote: Duration::from_secs(120),
        }
    }
}

/// What a parent hands a child.
#[derive(Debug, Clone)]
pub struct NewTask {
    pub parent_chat_id: String,
    pub parent_turn_id: String,
    pub child_chat_id: String,
    pub target_device_id: String,
    pub message_id: String,
    pub harness: HarnessId,
    pub model: Option<String>,
    pub title: String,
    pub mode: DelegationMode,
    pub completion_wake: CompletionWake,
    pub baseline: DelegationBaseline,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct State {
    tasks: Vec<DelegatedTask>,
    /// Parent turns the person stopped (`parent chat`, `turn`): what they
    /// started, and anything they still start, wakes nobody.
    stopped: Vec<(String, String)>,
}

#[derive(Clone)]
pub struct Delegations {
    inner: Arc<Store>,
}

struct Store {
    path: PathBuf,
    state: Mutex<State>,
    /// Rung on every change.
    changed: tokio::sync::watch::Sender<u64>,
    timing: Mutex<Timing>,
    /// Tasks whose reply is being read.
    finalizing: Mutex<HashSet<String>>,
    /// Parents being woken.
    waking: Mutex<HashSet<String>>,
    /// Wakes that failed: tries so far, and when to try again.
    backoff: Mutex<HashMap<String, (u32, tokio::time::Instant)>>,
    /// When each child was first seen asking its current question.
    asked: Mutex<HashMap<String, tokio::time::Instant>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Delegations {
    pub fn open(store_root: &Path) -> Self {
        let path = store_root.join("delegated-tasks.json");
        let mut state: State = std::fs::read(&path)
            .ok()
            .and_then(|bytes| match serde_json::from_slice(&bytes) {
                Ok(state) => Some(state),
                Err(error) => {
                    tracing::error!(%error, "delegated tasks couldn't be read; starting over");
                    None
                }
            })
            .unwrap_or_default();
        for task in &mut state.tasks {
            // The call that blocked on it died with the engine.
            if task.is_open() {
                task.mode = DelegationMode::Async;
            }
        }
        Self {
            inner: Arc::new(Store {
                path,
                state: Mutex::new(state),
                changed: tokio::sync::watch::channel(0).0,
                timing: Mutex::new(Timing::default()),
                finalizing: Mutex::default(),
                waking: Mutex::default(),
                backoff: Mutex::default(),
                asked: Mutex::default(),
            }),
        }
    }

    #[doc(hidden)]
    pub fn set_timing(&self, timing: Timing) {
        *lock(&self.inner.timing) = timing;
        self.inner.changed.send_modify(|n| *n += 1);
    }

    fn timing(&self) -> Timing {
        *lock(&self.inner.timing)
    }

    /// Apply `change`, keeping what it left on disk before anyone acts on it.
    fn update<T>(&self, change: impl FnOnce(&mut State) -> T) -> T {
        let out = {
            let mut state = lock(&self.inner.state);
            let out = change(&mut state);
            prune(&mut state);
            let written = serde_json::to_vec(&*state)
                .map_err(std::io::Error::other)
                .and_then(|bytes| {
                    let tmp = self.inner.path.with_extension("json.tmp");
                    std::fs::write(&tmp, bytes)?;
                    std::fs::rename(&tmp, &self.inner.path)
                });
            if let Err(error) = written {
                tracing::error!(%error, "delegated tasks weren't saved");
            }
            out
        };
        self.inner.changed.send_modify(|n| *n += 1);
        out
    }

    fn update_task<T>(&self, task_id: &str, change: impl FnOnce(&mut DelegatedTask) -> T) -> Option<T> {
        self.update(|state| {
            let task = state.tasks.iter_mut().find(|t| t.task_id == task_id)?;
            task.updated_at = Utc::now();
            Some(change(task))
        })
    }

    // ---- what the parent's tools do ----------------------------------------

    /// Record a turn handed to a child, before it's sent. An open task of the
    /// same child is superseded: its turn is the one this message joins or
    /// follows, and one result answers both.
    pub fn register(&self, new: NewTask) -> Result<DelegatedTask, String> {
        self.update(|state| {
            let turn = (new.parent_chat_id.clone(), new.parent_turn_id.clone());
            if state.stopped.contains(&turn) {
                return Err("This turn was stopped: it starts no more work".to_owned());
            }
            let now = Utc::now();
            for old in state.tasks.iter_mut().filter(|t| {
                t.parent_chat_id == new.parent_chat_id && t.child_chat_id == new.child_chat_id && t.holds_parent()
            }) {
                old.delivery_state = DeliveryState::Disposed;
                old.updated_at = now;
                if !old.status.is_settled() {
                    old.status = DelegatedStatus::Cancelled;
                    old.settled_at = Some(now);
                }
            }
            let task = DelegatedTask {
                task_id: uuid::Uuid::new_v4().to_string(),
                parent_chat_id: new.parent_chat_id,
                parent_turn_id: new.parent_turn_id,
                child_chat_id: new.child_chat_id,
                target_device_id: new.target_device_id,
                message_id: new.message_id,
                harness: new.harness,
                model: new.model,
                title: new.title,
                mode: new.mode,
                status: DelegatedStatus::Queued,
                completion_wake: new.completion_wake,
                delivery_state: DeliveryState::Pending,
                delivery_message_id: None,
                baseline: new.baseline,
                saw_active: false,
                created_at: now,
                updated_at: now,
                settled_at: None,
                result: None,
                input: None,
            };
            state.tasks.push(task.clone());
            Ok(task)
        })
    }

    /// The parent's own tasks, oldest first.
    pub fn of_parent(&self, parent: &str) -> Vec<DelegatedTask> {
        lock(&self.inner.state)
            .tasks
            .iter()
            .filter(|t| t.parent_chat_id == parent)
            .cloned()
            .collect()
    }

    pub fn views(&self, parent: &str, sessions: &[Session]) -> Vec<DelegatedTaskView> {
        let now = Utc::now();
        let stopped = lock(&self.inner.state).stopped.clone();
        self.of_parent(parent)
            .into_iter()
            .map(|task| DelegatedTaskView {
                cohort: if stopped.contains(&(task.parent_chat_id.clone(), task.parent_turn_id.clone())) {
                    CohortDisposition::Stopped
                } else {
                    CohortDisposition::Open
                },
                unreachable: task.is_open()
                    && task.status != DelegatedStatus::Queued
                    && row_of(sessions, &task)
                        .is_none_or(|row| zeron_proto::view::session_stale(row, now)),
                task,
            })
            .collect()
    }

    fn owned(&self, parent: &str, task_id: &str) -> Option<DelegatedTask> {
        self.of_parent(parent).into_iter().find(|t| t.task_id == task_id)
    }

    /// The parent took a task back: it's told nothing more of it. The caller
    /// stops the child.
    pub fn cancel(&self, parent: &str, task_id: &str) -> Option<DelegatedTask> {
        self.owned(parent, task_id)?;
        self.update_task(task_id, |task| {
            let before = task.clone();
            if !task.status.is_settled() {
                task.status = DelegatedStatus::Cancelled;
                task.settled_at = Some(Utc::now());
            }
            task.delivery_state = DeliveryState::Acknowledged;
            before
        })
    }

    /// A task whose message couldn't be sent: as if it never was.
    pub fn discard(&self, parent: &str, task_id: &str) -> bool {
        self.owned(parent, task_id).is_some()
            && self.update(|state| {
                let before = state.tasks.len();
                state.tasks.retain(|t| t.task_id != task_id);
                state.tasks.len() != before
            })
    }

    /// A wait that gave up: its result is pushed when there is one.
    pub fn make_async(&self, parent: &str, task_id: &str) -> bool {
        self.owned(parent, task_id).is_some()
            && self
                .update_task(task_id, |task| task.mode = DelegationMode::Async)
                .is_some()
    }

    /// A tool call returned the child's result to the parent itself.
    pub fn acknowledge(&self, parent: &str, task_id: &str, status: Option<DelegatedStatus>) -> bool {
        self.owned(parent, task_id).is_some()
            && self
                .update_task(task_id, |task| {
                    if !task.status.is_settled() {
                        task.status = status.filter(|s| s.is_settled()).unwrap_or(DelegatedStatus::Completed);
                        task.settled_at = Some(Utc::now());
                    }
                    if task.delivery_state == DeliveryState::Pending {
                        task.delivery_state = DeliveryState::Acknowledged;
                    }
                })
                .is_some()
    }

    /// The person stopped (or put away) the parent: nothing it started wakes
    /// it again. Returns what still ran, for the caller to stop.
    pub fn stop_parent(&self, parent: &str, current_turn: Option<&str>) -> Vec<DelegatedTask> {
        let held = current_turn.is_some()
            || lock(&self.inner.state)
                .tasks
                .iter()
                .any(|t| t.parent_chat_id == parent && t.holds_parent());
        if !held {
            return Vec::new();
        }
        self.update(|state| {
            let now = Utc::now();
            let mut turns: HashSet<String> = current_turn.map(str::to_owned).into_iter().collect();
            let mut open = Vec::new();
            for task in state
                .tasks
                .iter_mut()
                .filter(|t| t.parent_chat_id == parent && t.holds_parent())
            {
                turns.insert(task.parent_turn_id.clone());
                if task.is_open() {
                    open.push(task.clone());
                    task.status = DelegatedStatus::Cancelled;
                    task.settled_at = Some(now);
                }
                task.delivery_state = DeliveryState::Disposed;
                task.updated_at = now;
            }
            for turn in turns {
                let key = (parent.to_owned(), turn);
                if !state.stopped.contains(&key) {
                    state.stopped.push(key);
                }
            }
            open
        })
    }

    // ---- what the parent's chat reads ---------------------------------------

    /// What holds `parent`'s completion back: `None` when nothing does, else
    /// the children still at it (none, while the last result is on its way).
    pub fn waiting_on(&self, parent: &str) -> Option<Vec<WaitingOn>> {
        let state = lock(&self.inner.state);
        let mut held = state
            .tasks
            .iter()
            .filter(|t| t.parent_chat_id == parent && t.holds_parent())
            .peekable();
        held.peek()?;
        Some(
            held.filter(|t| t.is_open())
                .map(|t| WaitingOn {
                    id: t.task_id.clone(),
                    kind: WaitingKind::SubagentChat,
                    label: t.title.clone(),
                    child_chat_id: Some(t.child_chat_id.clone()),
                    device_id: Some(t.target_device_id.clone()),
                    harness: Some(t.harness),
                    started_at: Some(t.created_at),
                })
                .collect(),
        )
    }

    fn parents(&self) -> HashSet<String> {
        lock(&self.inner.state)
            .tasks
            .iter()
            .filter(|t| t.holds_parent())
            .map(|t| t.parent_chat_id.clone())
            .collect()
    }

    fn held(&self) -> Vec<DelegatedTask> {
        lock(&self.inner.state)
            .tasks
            .iter()
            .filter(|t| t.holds_parent())
            .cloned()
            .collect()
    }

    /// Parents a Cloud machine stays up for: their children were heard from
    /// lately, so a wake may come.
    pub fn parents_expecting(&self, now: DateTime<Utc>) -> usize {
        lock(&self.inner.state)
            .tasks
            .iter()
            .filter(|t| t.holds_parent() && now - t.updated_at < HOLDS_MACHINE)
            .map(|t| t.parent_chat_id.as_str())
            .collect::<HashSet<_>>()
            .len()
    }
}

/// Forget what's long told, and stopped turns nothing refers to.
fn prune(state: &mut State) {
    let cutoff = Utc::now() - KEEP_SETTLED;
    state.tasks.retain(|t| t.holds_parent() || t.updated_at > cutoff);
    let mut kept: HashMap<String, usize> = HashMap::new();
    let mut keep: Vec<bool> = state
        .tasks
        .iter()
        .rev()
        .map(|t| {
            let n = kept.entry(t.parent_chat_id.clone()).or_default();
            *n += 1;
            t.holds_parent() || *n <= KEEP_PER_PARENT
        })
        .collect();
    keep.reverse();
    let mut keep = keep.into_iter();
    state.tasks.retain(|_| keep.next().unwrap_or(true));
    let len = state.stopped.len();
    if len > 500 {
        state.stopped.drain(..len - 500);
    }
}

fn row_of<'a>(sessions: &'a [Session], task: &DelegatedTask) -> Option<&'a Session> {
    sessions
        .iter()
        .find(|s| s.chat_id == task.child_chat_id && s.device_id == task.target_device_id)
}

/// A child's row as a task is sent.
pub fn baseline_of(sessions: &[Session], chat: &Chat) -> DelegationBaseline {
    sessions
        .iter()
        .find(|s| s.chat_id == chat.id && s.device_id == chat.device_id)
        .map(|s| DelegationBaseline {
            last_completed_turn: s.last_completed_turn.clone(),
            last_outcome_at: s.last_turn_outcome.map(|o| o.finished_at),
        })
        .unwrap_or_default()
}

/// What a child's row says of a task. `None`: nothing to go by (no row, or
/// one whose device went quiet), so it stands as it was.
fn evidence(task: &DelegatedTask, row: Option<&Session>, now: DateTime<Utc>) -> Option<DelegatedStatus> {
    let row = row?;
    if zeron_proto::view::session_stale(row, now) {
        return None;
    }
    let outcome = row
        .last_turn_outcome
        .filter(|o| task.baseline.last_outcome_at != Some(o.finished_at));
    match row.status {
        SessionStatus::Working => Some(DelegatedStatus::Running),
        SessionStatus::AwaitingInput => Some(DelegatedStatus::AwaitingInput),
        SessionStatus::Errored => (outcome.is_some() || task.saw_active).then_some(DelegatedStatus::Failed),
        // Its turn ended into a wait of its own: not done, and its completion
        // is held back. (A wait whose device went quiet says nothing.)
        SessionStatus::Idle if row.waiting.is_some() => zeron_proto::view::waiting(Some(row), now)
            .is_some()
            .then_some(DelegatedStatus::Running),
        SessionStatus::Idle => {
            if row.last_completed_turn.is_some() && row.last_completed_turn != task.baseline.last_completed_turn {
                return Some(DelegatedStatus::Completed);
            }
            match outcome?.status {
                DoneStatus::Interrupted => Some(DelegatedStatus::Cancelled),
                DoneStatus::Errored => Some(DelegatedStatus::Failed),
                // (A turn whose completion goes on into the next one.)
                DoneStatus::Completed => None,
            }
        }
    }
}

// ---- the supervisor ---------------------------------------------------------

/// Watches every open task until the engine goes, picking up where the last
/// one left off.
#[derive(Clone)]
pub(crate) struct Supervisor {
    pub(crate) tasks: Delegations,
    pub(crate) sessions: SessionsEngine,
    pub(crate) doc_host: DocHost,
    pub(crate) workspace: WorkspaceHost,
}

impl Supervisor {
    pub(crate) fn spawn(self) {
        let host = self.doc_host.clone();
        host.spawn_worker(self.run());
    }

    async fn run(self) {
        let mut rows = self.workspace.merged_sessions_watch(self.sessions.watch_sessions());
        let mut chats = self.workspace.watch_chats();
        let mut changed = self.tasks.inner.changed.subscribe();
        let mut known: HashSet<String> = HashSet::new();
        self.redeliver().await;
        loop {
            let sessions = rows.borrow_and_update().clone();
            let chat_rows = chats.borrow_and_update().clone();
            changed.borrow_and_update();
            let next = self.look(&sessions, &chat_rows).await;
            // Every parent held now or a moment ago reads what it waits on.
            let parents = self.tasks.parents();
            for parent in known.union(&parents) {
                self.sessions.refresh_delegated(parent);
            }
            known = parents;
            let tick = async {
                match (known.is_empty(), next) {
                    (true, _) => std::future::pending().await,
                    (false, next) => {
                        tokio::time::sleep(next.unwrap_or(self.tasks.timing().tick).min(self.tasks.timing().tick)).await
                    }
                }
            };
            tokio::select! {
                changed = rows.changed() => if changed.is_err() { return },
                changed = chats.changed() => if changed.is_err() { return },
                changed = changed.changed() => if changed.is_err() { return },
                () = tick => for parent in &known { self.sessions.heartbeat(parent) },
            }
        }
    }

    /// Wakes a stop cut off between being recorded and reaching the chat.
    async fn redeliver(&self) {
        for task in self.tasks.held() {
            let Some(message) = task
                .delivery_message_id
                .clone()
                .filter(|_| task.delivery_state == DeliveryState::Delivered)
            else {
                continue;
            };
            let there = self
                .doc_host
                .open(&task.parent_chat_id)
                .ok()
                .and_then(|handle| handle.doc().read_entries_shared().ok())
                .is_some_and(|entries| entries.iter().any(|e| e.id == message));
            self.tasks.update_task(&task.task_id, |task| {
                task.delivery_state = if there {
                    DeliveryState::Acknowledged
                } else {
                    DeliveryState::Pending
                };
            });
        }
    }

    /// One look at everything held. Returns how soon the next is due, when
    /// that's sooner than a tick.
    async fn look(&self, sessions: &[Session], chats: &[Chat]) -> Option<Duration> {
        let now = Utc::now();
        for parent in self.tasks.parents() {
            let archived = chats.iter().find(|c| c.id == parent).is_some_and(|c| c.archived);
            if archived || self.workspace.chat_deleted(&parent) {
                self.stop(&parent, None);
            }
        }
        for task in self.tasks.held().into_iter().filter(DelegatedTask::is_open) {
            let child = chats.iter().find(|c| c.id == task.child_chat_id);
            let gone = match child {
                Some(child) if child.archived => Some("The person archived this chat."),
                None if self.workspace.chat_deleted(&task.child_chat_id) => Some("This chat was deleted."),
                _ => None,
            };
            if let Some(why) = gone {
                self.settle(&task.task_id, DelegatedStatus::Cancelled, Some(why.to_owned()));
                continue;
            }
            // (Re-homed: watched where it runs now.)
            let device = child
                .map(|c| c.device_id.clone())
                .filter(|device| *device != task.target_device_id);
            if let Some(device) = device {
                self.tasks.update_task(&task.task_id, |task| task.target_device_id = device);
                continue;
            }
            let Some(status) = evidence(&task, row_of(sessions, &task), now) else {
                continue;
            };
            if status.is_settled() {
                self.finalize(task, status);
                continue;
            }
            if status != task.status || (!task.saw_active && status == DelegatedStatus::Running) {
                self.tasks.update_task(&task.task_id, |task| {
                    task.status = status;
                    task.saw_active = true;
                    if status != DelegatedStatus::AwaitingInput {
                        // Answered (by the person, or the parent): a wake
                        // that would have told the parent goes with it.
                        task.input = None;
                    }
                });
                if status != DelegatedStatus::AwaitingInput {
                    lock(&self.tasks.inner.asked).remove(&task.task_id);
                }
            }
            if status == DelegatedStatus::AwaitingInput {
                self.note_question(&task);
            }
        }
        let mut next: Option<Duration> = None;
        for parent in self.tasks.parents() {
            if let Some(due) = self.wake(&parent) {
                next = Some(next.map_or(due, |n| n.min(due)));
            }
        }
        next
    }

    /// Stop what `parent` started, for good: the person's Stop, or its chat
    /// put away.
    fn stop(&self, parent: &str, current_turn: Option<&str>) {
        for task in self.tasks.stop_parent(parent, current_turn) {
            interrupt(&self.doc_host, &task.child_chat_id);
        }
    }

    fn settle(&self, task_id: &str, status: DelegatedStatus, result: Option<String>) {
        lock(&self.tasks.inner.asked).remove(task_id);
        self.tasks.update_task(task_id, |task| {
            if task.status.is_settled() {
                return;
            }
            task.status = status;
            task.settled_at = Some(Utc::now());
            task.result = result;
            task.input = None;
        });
    }

    /// Read a settled child's reply, then settle its task. Its row and its
    /// transcript sync separately, so a reply on another device may be a
    /// while behind; what can't be read in time is said to be in the chat.
    fn finalize(&self, task: DelegatedTask, status: DelegatedStatus) {
        if !lock(&self.tasks.inner.finalizing).insert(task.task_id.clone()) {
            return;
        }
        let this = self.clone();
        self.doc_host.spawn_worker(async move {
            let local = task.target_device_id == this.doc_host.device_id();
            let timing = this.tasks.timing();
            let limit = if local { timing.reply_local } else { timing.reply_remote };
            let deadline = tokio::time::Instant::now() + limit;
            let mut pause = Duration::from_millis(50);
            let reply = loop {
                let read = read_reply(&this.doc_host, &task);
                match read {
                    Reply::Said(text) => break Some(text),
                    // An interrupted or failed turn may well have said nothing.
                    Reply::Nothing if status != DelegatedStatus::Completed => break None,
                    _ if tokio::time::Instant::now() + pause >= deadline => {
                        // The row ended another turn (a message before this
                        // one's): this task's own is still to come.
                        if matches!(read, Reply::NotSent) && local {
                            this.rebase(&task);
                            lock(&this.tasks.inner.finalizing).remove(&task.task_id);
                            return;
                        }
                        break None;
                    }
                    _ => {}
                }
                tokio::time::sleep(pause).await;
                pause = (pause * 2).min(Duration::from_secs(15));
            };
            this.settle(&task.task_id, status, reply);
            lock(&this.tasks.inner.finalizing).remove(&task.task_id);
        });
    }

    fn rebase(&self, task: &DelegatedTask) {
        let sessions = self.workspace.merged_sessions(&self.sessions.watch_sessions());
        let baseline = row_of(&sessions, task).map(|s| DelegationBaseline {
            last_completed_turn: s.last_completed_turn.clone(),
            last_outcome_at: s.last_turn_outcome.map(|o| o.finished_at),
        });
        if let Some(baseline) = baseline {
            self.tasks.update_task(&task.task_id, |task| {
                task.baseline = baseline;
                task.saw_active = false;
            });
        }
    }

    /// A child asks something: noted for its parent when it's the kind an
    /// agent may answer. An approval is the person's, always: it shows on the
    /// parent's chat instead.
    fn note_question(&self, task: &DelegatedTask) {
        let Some((request_id, questions)) = open_question(&self.doc_host, &task.child_chat_id) else {
            return;
        };
        if zeron_proto::is_approval(&questions) {
            return;
        }
        if task.input.as_ref().is_some_and(|input| input.request_id == request_id) {
            return;
        }
        lock(&self.tasks.inner.asked).insert(task.task_id.clone(), tokio::time::Instant::now());
        self.tasks.update_task(&task.task_id, |task| {
            task.input = Some(DelegatedInput {
                request_id,
                message_id: None,
            });
        });
    }

    /// Send `parent` what it hasn't been told, if it can be told now. Returns
    /// when to look again, when something is nearly due.
    fn wake(&self, parent: &str) -> Option<Duration> {
        if self.sessions.turn_in_flight(parent) || lock(&self.tasks.inner.waking).contains(parent) {
            return None;
        }
        let now = tokio::time::Instant::now();
        if let Some((_, at)) = lock(&self.tasks.inner.backoff).get(parent)
            && *at > now
        {
            return Some(*at - now);
        }
        let timing = self.tasks.timing();
        let held: Vec<DelegatedTask> = self
            .tasks
            .held()
            .into_iter()
            .filter(|t| t.parent_chat_id == parent)
            .collect();
        let open_turns: HashSet<&str> = held
            .iter()
            .filter(|t| t.is_open())
            .map(|t| t.parent_turn_id.as_str())
            .collect();
        let results: Vec<DelegatedTask> = held
            .iter()
            .filter(|t| t.status.is_settled() && t.delivery_state == DeliveryState::Pending)
            .filter(|t| {
                t.completion_wake == CompletionWake::Always || !open_turns.contains(t.parent_turn_id.as_str())
            })
            .cloned()
            .collect();
        if !results.is_empty() {
            let latest = results.iter().map(|t| t.settled_at.unwrap_or(t.updated_at)).max();
            let since = latest.map_or(timing.coalesce, |at| (Utc::now() - at).to_std().unwrap_or_default());
            if since < timing.coalesce {
                return Some(timing.coalesce - since);
            }
            self.deliver(parent, WakeKind::Results, results, held);
            return None;
        }
        let mut due: Option<Duration> = None;
        let asked = lock(&self.tasks.inner.asked).clone();
        let questions: Vec<DelegatedTask> = held
            .iter()
            .filter(|t| t.is_open() && t.input.as_ref().is_some_and(|i| i.message_id.is_none()))
            .filter(|t| match asked.get(&t.task_id) {
                Some(at) if at.elapsed() < timing.question_grace => {
                    let left = timing.question_grace - at.elapsed();
                    due = Some(due.map_or(left, |d| d.min(left)));
                    false
                }
                _ => true,
            })
            .cloned()
            .collect();
        if !questions.is_empty() {
            self.deliver(parent, WakeKind::Question, questions, held);
            return None;
        }
        due
    }

    fn deliver(&self, parent: &str, kind: WakeKind, tasks: Vec<DelegatedTask>, held: Vec<DelegatedTask>) {
        lock(&self.tasks.inner.waking).insert(parent.to_owned());
        let message_id = format!("delegated-{}", uuid::Uuid::new_v4());
        let told: HashSet<&str> = tasks.iter().map(|t| t.task_id.as_str()).collect();
        let open: Vec<&DelegatedTask> = held
            .iter()
            .filter(|t| t.is_open() && (kind == WakeKind::Question || !told.contains(t.task_id.as_str())))
            .collect();
        let notice = WakeNotice {
            kind,
            tasks: tasks.iter().map(wake_item).collect(),
            open: open.len(),
        };
        let body = match kind {
            WakeKind::Results => results_body(&tasks, &open),
            WakeKind::Question => questions_body(&self.doc_host, &tasks),
        };
        let text = delegation::wake_message(&notice, &body);
        // Recorded first: a stop between the two finds it and looks.
        for task in &tasks {
            self.tasks.update_task(&task.task_id, |task| match kind {
                WakeKind::Results => {
                    task.delivery_state = DeliveryState::Delivered;
                    task.delivery_message_id = Some(message_id.clone());
                }
                WakeKind::Question => {
                    if let Some(input) = task.input.as_mut() {
                        input.message_id = Some(message_id.clone());
                    }
                }
            });
        }
        let this = self.clone();
        let parent = parent.to_owned();
        self.doc_host.spawn_worker(async move {
            let sent = this.doc_host.deliver_wake(&parent, &message_id, &text).await;
            let sent = match sent {
                Ok(sent) => sent,
                Err(error) => {
                    tracing::warn!(chat = %parent, %error, "delegated results weren't delivered");
                    let mut backoff = lock(&this.tasks.inner.backoff);
                    let tries = backoff.get(&parent).map_or(1, |(tries, _)| tries + 1);
                    let wait = Duration::from_secs(2u64 << tries.min(7));
                    backoff.insert(parent.clone(), (tries, tokio::time::Instant::now() + wait));
                    // Told nothing further: the chat isn't held for a wake
                    // that isn't coming. (The results are in their chats.)
                    tries >= MAX_WAKE_TRIES
                }
            };
            if sent {
                lock(&this.tasks.inner.backoff).remove(&parent);
            }
            for task in &tasks {
                this.tasks.update_task(&task.task_id, |task| match kind {
                    WakeKind::Results if task.delivery_state != DeliveryState::Delivered => {}
                    WakeKind::Results if sent => task.delivery_state = DeliveryState::Acknowledged,
                    WakeKind::Results => {
                        task.delivery_state = DeliveryState::Pending;
                        task.delivery_message_id = None;
                    }
                    WakeKind::Question => {
                        if !sent && let Some(input) = task.input.as_mut() {
                            input.message_id = None;
                        }
                    }
                });
            }
            lock(&this.tasks.inner.waking).remove(&parent);
            this.tasks.inner.changed.send_modify(|n| *n += 1);
        });
    }
}

/// Stop a child's turn, wherever it runs.
pub(crate) fn interrupt(doc_host: &DocHost, child: &str) {
    if let Err(error) = doc_host.queue_command(child, SessionCommandPayload::Interrupt {}) {
        tracing::warn!(chat = %child, %error, "a delegated chat couldn't be stopped");
    }
}

fn wake_item(task: &DelegatedTask) -> WakeItem {
    WakeItem {
        task_id: task.task_id.clone(),
        chat_id: task.child_chat_id.clone(),
        device_id: task.target_device_id.clone(),
        title: task.title.clone(),
        harness: task.harness,
        status: task.status,
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

fn results_body(tasks: &[DelegatedTask], open: &[&DelegatedTask]) -> String {
    let mut body = format!(
        "{} you delegated settled. This is Zeron, not the person: nobody wrote this message. \
         Each result below is the child chat's own final reply, quoted.\n",
        plural(tasks.len(), "task", "tasks")
    );
    for task in tasks {
        let status = match task.status {
            DelegatedStatus::Failed => "failed",
            DelegatedStatus::Cancelled => "stopped before it finished",
            _ => "completed",
        };
        body.push_str(&format!(
            "\n<result task=\"{}\" chat=\"{}\" status=\"{status}\" title=\"{}\">\n",
            task.task_id,
            task.child_chat_id,
            delegation::quoted_result(&task.title).replace('"', "'"),
        ));
        match task.result.as_deref().map(str::trim).filter(|r| !r.is_empty()) {
            Some(result) => body.push_str(&delegation::quoted_result(&clip(result))),
            None => body.push_str("(No reply could be read. Use read_chat on that chat.)"),
        }
        body.push_str("\n</result>\n");
    }
    if open.is_empty() {
        body.push_str("\nNothing you delegated is still running. Carry on, and finish with your reply to the person.");
    } else {
        body.push_str(&format!(
            "\nStill running: {}. Carry on with what you have, or end your turn: you're woken when they settle.",
            open.iter()
                .map(|t| format!("{} ({})", delegation::quoted_result(&t.title), t.task_id))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    body
}

fn questions_body(doc_host: &DocHost, tasks: &[DelegatedTask]) -> String {
    let mut body = "A chat you delegated to asks something before it can go on. This is Zeron, not the person. \
                    Answer with respond_to_input if you can from what you know; if only the person can, ask them \
                    in your reply and end your turn (they may answer it in that chat themselves).\n"
        .to_owned();
    for task in tasks {
        let Some((request_id, questions)) = open_question(doc_host, &task.child_chat_id) else {
            continue;
        };
        body.push_str(&format!(
            "\n<result task=\"{}\" chat=\"{}\" request=\"{request_id}\" title=\"{}\">\n",
            task.task_id,
            task.child_chat_id,
            delegation::quoted_result(&task.title).replace('"', "'"),
        ));
        for q in questions {
            body.push_str(&format!("- [{}] {}", q.id, delegation::quoted_result(&q.question)));
            if !q.options.is_empty() {
                body.push_str(&format!(
                    " (options: {})",
                    delegation::quoted_result(&q.options.join(" | "))
                ));
            }
            body.push('\n');
        }
        body.push_str("</result>\n");
    }
    body
}

fn clip(text: &str) -> String {
    if text.chars().count() <= RESULT_LIMIT {
        return text.to_owned();
    }
    let head: String = text.chars().take(RESULT_LIMIT).collect();
    format!("{head}\n… (cut short: read_chat has the rest)")
}

/// A child's reply to a task's message, as its transcript stands here.
enum Reply {
    /// The message hasn't reached the transcript.
    NotSent,
    /// Its turn has written nothing final yet.
    Writing,
    /// Its turn wrote no words.
    Nothing,
    Said(String),
}

fn read_reply(doc_host: &DocHost, task: &DelegatedTask) -> Reply {
    let Some(entries) = doc_host
        .open(&task.child_chat_id)
        .ok()
        .and_then(|handle| handle.doc().read_entries_shared().ok())
    else {
        return Reply::NotSent;
    };
    let Some(at) = entries.iter().position(|e| e.id == task.message_id) else {
        return Reply::NotSent;
    };
    let mut said = None;
    let mut wrote = false;
    for entry in entries[at + 1..].iter().filter(|e| e.role == MessageRole::Assistant) {
        if entry.status == Some(MessageStatus::Streaming) {
            return Reply::Writing;
        }
        wrote = true;
        let mut text: Vec<&str> = Vec::new();
        for part in &entry.parts {
            match part {
                MessagePart::Text { text: t, .. } => text.push(t),
                MessagePart::Error { message, .. } => text.push(message),
                // (The words after its last tool call are the reply.)
                MessagePart::Tool { .. } => text.clear(),
                _ => {}
            }
        }
        let text = text.join("\n\n").trim().to_owned();
        if !text.is_empty() {
            said = Some(text);
        }
    }
    match said {
        Some(text) => Reply::Said(text),
        None if wrote => Reply::Nothing,
        None => Reply::Writing,
    }
}

/// What `chat` asks now, from its latest open question.
fn open_question(doc_host: &DocHost, chat: &str) -> Option<(String, Vec<zeron_proto::UserInputQuestion>)> {
    let entries = doc_host.open(chat).ok()?.doc().read_entries_shared().ok()?;
    entries.iter().rev().flat_map(|e| e.parts.iter().rev()).find_map(|part| match part {
        MessagePart::Input {
            request_id,
            questions,
            resolved: false,
            ..
        } => Some((request_id.clone(), questions.clone())),
        _ => None,
    })
}

/// The parent turn a task belongs to: the message that opened it.
pub(crate) fn current_turn(doc_host: &DocHost, chat: &str) -> String {
    doc_host
        .open(chat)
        .ok()
        .and_then(|handle| handle.doc().read_entries_shared().ok())
        .and_then(|entries| {
            entries
                .iter()
                .rev()
                .find(|e| e.role == MessageRole::User)
                .map(|e| e.id.clone())
        })
        .unwrap_or_else(|| "start".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, Delegations) {
        let dir = tempfile::tempdir().unwrap();
        let store = Delegations::open(dir.path());
        (dir, store)
    }

    fn new_task(child: &str, turn: &str) -> NewTask {
        NewTask {
            parent_chat_id: "parent".into(),
            parent_turn_id: turn.into(),
            child_chat_id: child.into(),
            target_device_id: "mac".into(),
            message_id: format!("m-{child}"),
            harness: HarnessId::Codex,
            model: None,
            title: child.into(),
            mode: DelegationMode::Async,
            completion_wake: CompletionWake::Always,
            baseline: DelegationBaseline::default(),
        }
    }

    fn row(status: SessionStatus) -> Session {
        Session {
            last_turn_outcome: None,
            terminal_activity: None,
            last_completed_turn: None,
            chat_id: "a".into(),
            device_id: "mac".into(),
            status,
            started_at: None,
            updated_at: Utc::now(),
            running_subagents: 0,
            background_tasks: Vec::new(),
            waiting: None,
            blocked_on_wait: false,
        }
    }

    #[test]
    fn tasks_survive_a_restart_and_a_dead_wait_is_pushed() {
        let (dir, store) = store();
        let mut new = new_task("a", "t1");
        new.mode = DelegationMode::Wait;
        let task = store.register(new).unwrap();
        let reopened = Delegations::open(dir.path());
        let kept = reopened.of_parent("parent");
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].task_id, task.task_id);
        assert_eq!(kept[0].mode, DelegationMode::Async);
        assert_eq!(reopened.waiting_on("parent").unwrap()[0].child_chat_id.as_deref(), Some("a"));
        assert!(reopened.waiting_on("other").is_none());
    }

    #[test]
    fn a_follow_up_supersedes_the_childs_open_task() {
        let (_dir, store) = store();
        let first = store.register(new_task("a", "t1")).unwrap();
        let second = store.register(new_task("a", "t2")).unwrap();
        let tasks = store.of_parent("parent");
        assert_eq!(tasks[0].task_id, first.task_id);
        assert_eq!(tasks[0].delivery_state, DeliveryState::Disposed);
        assert!(tasks[1].is_open() && tasks[1].task_id == second.task_id);
        assert_eq!(store.waiting_on("parent").unwrap().len(), 1);
    }

    #[test]
    fn a_stop_disposes_what_ran_and_whatever_that_turn_starts_next() {
        let (_dir, store) = store();
        store.register(new_task("a", "t1")).unwrap();
        let done = store.register(new_task("b", "t1")).unwrap();
        store.update_task(&done.task_id, |t| t.status = DelegatedStatus::Completed);
        let stopped = store.stop_parent("parent", Some("t1"));
        assert_eq!(stopped.len(), 1, "only what still ran is stopped");
        assert_eq!(stopped[0].child_chat_id, "a");
        assert!(store.waiting_on("parent").is_none(), "the finished one's result is disposed too");
        assert!(store.register(new_task("c", "t1")).is_err());
        store.register(new_task("c", "t2")).expect("the person's next turn starts work again");
        assert_eq!(store.views("parent", &[])[0].cohort, CohortDisposition::Stopped);
        assert_eq!(store.views("parent", &[])[2].cohort, CohortDisposition::Open);
    }

    #[test]
    fn only_the_parent_acts_on_its_tasks() {
        let (_dir, store) = store();
        let task = store.register(new_task("a", "t1")).unwrap();
        assert!(store.cancel("someone", &task.task_id).is_none());
        assert!(!store.acknowledge("someone", &task.task_id, None));
        assert!(!store.make_async("someone", &task.task_id));
        assert!(!store.discard("someone", &task.task_id));
        assert!(store.waiting_on("parent").is_some());
        assert!(store.cancel("parent", &task.task_id).is_some());
        assert!(store.waiting_on("parent").is_none());
    }

    #[test]
    fn a_quiet_device_is_never_a_failure() {
        let (_dir, store) = store();
        let mut task = store.register(new_task("a", "t1")).unwrap();
        task.saw_active = true;
        let now = Utc::now();
        assert_eq!(evidence(&task, None, now), None);
        let mut quiet = row(SessionStatus::Working);
        quiet.updated_at = now - chrono::Duration::hours(5);
        assert_eq!(evidence(&task, Some(&quiet), now), None);
        quiet.updated_at = now;
        assert_eq!(evidence(&task, Some(&quiet), now), Some(DelegatedStatus::Running));
    }

    #[test]
    fn a_task_settles_on_an_end_after_it_was_sent() {
        let (_dir, store) = store();
        let mut task = store.register(new_task("a", "t1")).unwrap();
        let now = Utc::now();
        task.baseline.last_completed_turn = Some("before".into());
        let mut idle = row(SessionStatus::Idle);
        idle.last_completed_turn = Some("before".into());
        assert_eq!(evidence(&task, Some(&idle), now), None, "the turn before isn't this one");
        idle.last_completed_turn = Some("after".into());
        assert_eq!(evidence(&task, Some(&idle), now), Some(DelegatedStatus::Completed));
        // Ended into a wait of its own: not done.
        idle.last_completed_turn = Some("before".into());
        idle.waiting = Some(zeron_proto::SessionWaiting { since: now, on: vec![] });
        assert_eq!(evidence(&task, Some(&idle), now), Some(DelegatedStatus::Running));
        idle.waiting = None;
        idle.last_turn_outcome = Some(zeron_proto::NativeTurnOutcome {
            status: DoneStatus::Interrupted,
            finished_at: now,
        });
        assert_eq!(evidence(&task, Some(&idle), now), Some(DelegatedStatus::Cancelled));
        task.baseline.last_outcome_at = Some(now);
        assert_eq!(evidence(&task, Some(&idle), now), None);
        let mut errored = row(SessionStatus::Errored);
        errored.last_turn_outcome = Some(zeron_proto::NativeTurnOutcome {
            status: DoneStatus::Errored,
            finished_at: now + chrono::Duration::seconds(1),
        });
        assert_eq!(evidence(&task, Some(&errored), now), Some(DelegatedStatus::Failed));
    }

    #[test]
    fn what_was_told_is_forgotten_in_time_and_what_holds_never() {
        let (_dir, store) = store();
        let told = store.register(new_task("a", "t1")).unwrap();
        store.register(new_task("b", "t1")).unwrap();
        store.update(|state| {
            for task in &mut state.tasks {
                task.updated_at = Utc::now() - chrono::Duration::days(30);
            }
            let task = state.tasks.iter_mut().find(|t| t.task_id == told.task_id).unwrap();
            task.delivery_state = DeliveryState::Acknowledged;
        });
        let kept = store.of_parent("parent");
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].child_chat_id, "b");
    }
}
