//! Sessions: one chat's doc mirror, its incrementally maintained transcript,
//! the composer state, and every command a viewer can issue.
//!
//! Writer discipline (what a viewer writes into the session doc):
//! - `commands`: append-only own entries (run / steer / interrupt /
//!   respondInput) with client-minted message ids for optimistic echo;
//! - `queue`: enqueue + reorder. Edits and removals go through the host
//!   (lease / acknowledgement RPCs) because it serializes them with delivery.
//!
//! The host writes all transcript entries and command outcomes.

mod snapshot;
pub(crate) mod transcript;

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock, Weak};

use tokio::sync::watch;
use zeron_doc::{
    CommandBasedOn, MessagePart, MessageRole, MessageStatus, QueueDeliveryGate, QueuedMessage,
    SessionCommandEntry, SessionCommandPayload, SessionCommandStatus, SessionDoc,
    SessionMessageEntry,
};
use zeron_proto::{ChatIndicator, ContextUsage, RunRequest, SandboxLevel, UserInputAnswer, WorktreeSpec};

pub use snapshot::{
    AppendHint, ComposerState, Entry, HostCapabilities, HostInfo, InputRequest, LiveStatus,
    LocalEcho, PendingKind, PendingSend, QueueGate, QueueItem, RoomState, SessionSnapshot,
    SnapshotDelta,
};
use transcript::{Dirty, Tracker};

use crate::attachments;
use crate::client::ClientInner;
use crate::connectivity::{SendState, send_state};
use crate::error::{ClientError, Result};
use crate::{lock, now_ms, read, write};

/// What to do with a message typed while the agent is working.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum BusyPolicy {
    /// Park it on the shared queue until the turn ends (the default).
    #[default]
    Queue,
    /// Deliver mid-turn: a queue row the host may steer into the live turn,
    /// or a steer command on hosts without the shared queue.
    Steer,
}

/// Image bytes picked in the composer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutgoingAttachment {
    pub name: String,
    pub mime_type: String,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SendRequest {
    pub text: String,
    pub attachments: Vec<OutgoingAttachment>,
    /// Host-side isolated worktree for this run (new-session "New worktree").
    pub worktree: Option<WorktreeSpec>,
    pub busy: BusyPolicy,
}

impl SendRequest {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SendOutcome {
    /// A new turn was requested (`run`); echo id = `message_id`.
    Started { message_id: String },
    /// Delivered into the live turn (`steer`).
    Steered { message_id: String },
    /// Parked on the shared queue.
    Queued { queue_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueEditLease {
    pub row_id: String,
    pub lease_id: String,
    /// Row text at lease time (edit starts from this).
    pub text: String,
    pub base_text_hash: String,
    pub expires_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueueEditStart {
    Acquired(QueueEditLease),
    /// Another editor holds the row.
    Locked,
    /// The row left the queue.
    Missing,
    /// Host unreachable / capability missing.
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueEditAction {
    /// Save the new text.
    Commit,
    /// Keep the old text, release the row.
    Cancel,
    /// Delete the row.
    Discard,
    /// Release without changing anything.
    Release,
}

impl QueueEditAction {
    fn wire(self) -> &'static str {
        match self {
            QueueEditAction::Commit => "commit",
            QueueEditAction::Cancel => "cancel",
            QueueEditAction::Discard => "discard",
            QueueEditAction::Release => "release",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueEditFinish {
    Finished,
    /// The row changed under the lease (hash mismatch).
    Conflict,
    Missing,
    /// The lease was superseded or expired.
    Lost,
    Unavailable,
}

/// Mutable per-session state behind one lock.
struct CoreState {
    tracker: Tracker,
    queue: Vec<QueuedMessage>,
    commands: Vec<SessionCommandEntry>,
    context_usage: Option<ContextUsage>,
    /// Stable echo entries by message id.
    echoes: HashMap<String, Arc<Entry>>,
    pending: Vec<PendingSend>,
    /// Delivery-grace clock per message id (retry restarts it).
    grace_started: HashMap<String, i64>,
    queue_actions_pending: HashSet<String>,
    queue_error: Option<String>,
    last_submitted: Option<String>,
    transfer_progress: Option<f64>,
    hydrated: bool,
    transcript_revision: u64,
    composer_revision: u64,
}

pub(crate) struct SessionCore {
    pub(crate) chat_id: String,
    client: Weak<ClientInner>,
    doc: Arc<SessionDoc>,
    write_gate: Mutex<()>,
    dirty: Arc<Mutex<Dirty>>,
    _subscription: loro::Subscription,
    state: Mutex<CoreState>,
    transcript_tx: watch::Sender<Arc<SessionSnapshot>>,
    composer: RwLock<Arc<ComposerState>>,
    view_attached: AtomicBool,
    /// Last open/attach/detach (warm-set eviction order).
    touched_ms: std::sync::atomic::AtomicI64,
}

impl SessionCore {
    pub(crate) fn new(chat_id: &str, client: &Arc<ClientInner>, doc: SessionDoc) -> Arc<Self> {
        let doc = Arc::new(doc);
        let dirty = Arc::new(Mutex::new(Dirty::all()));
        let sink = dirty.clone();
        let subscription = doc.doc().subscribe_root(Arc::new(move |event| {
            lock(&sink).observe(&event);
        }));
        let (transcript_tx, _) = watch::channel(Arc::new(SessionSnapshot {
            chat_id: chat_id.to_owned(),
            ..Default::default()
        }));
        let composer = Arc::new(empty_composer(chat_id));
        Arc::new(Self {
            chat_id: chat_id.to_owned(),
            client: Arc::downgrade(client),
            doc,
            write_gate: Mutex::new(()),
            dirty,
            _subscription: subscription,
            state: Mutex::new(CoreState {
                tracker: Tracker::new(),
                queue: Vec::new(),
                commands: Vec::new(),
                context_usage: None,
                echoes: HashMap::new(),
                pending: Vec::new(),
                grace_started: HashMap::new(),
                queue_actions_pending: HashSet::new(),
                queue_error: None,
                last_submitted: None,
                transfer_progress: None,
                hydrated: false,
                transcript_revision: 0,
                composer_revision: 0,
            }),
            transcript_tx,
            composer: RwLock::new(composer),
            view_attached: AtomicBool::new(false),
            touched_ms: std::sync::atomic::AtomicI64::new(now_ms()),
        })
    }

    pub(crate) fn doc(&self) -> &Arc<SessionDoc> {
        &self.doc
    }

    fn client(&self) -> Result<Arc<ClientInner>> {
        self.client.upgrade().ok_or(ClientError::Closed)
    }

    pub(crate) fn set_hydrated(&self) {
        lock(&self.state).hydrated = true;
    }

    /// Serialized doc write, then an incremental refresh.
    pub(crate) fn write<R>(
        &self,
        f: impl FnOnce(&SessionDoc) -> std::result::Result<R, zeron_doc::DocError>,
    ) -> Result<R> {
        let result = {
            let _gate = lock(&self.write_gate);
            f(&self.doc)?
        };
        self.refresh();
        Ok(result)
    }

    pub(crate) fn snapshot(&self) -> Arc<SessionSnapshot> {
        self.transcript_tx.borrow().clone()
    }

    pub(crate) fn subscribe(&self) -> watch::Receiver<Arc<SessionSnapshot>> {
        self.transcript_tx.subscribe()
    }

    pub(crate) fn composer(&self) -> Arc<ComposerState> {
        read(&self.composer).clone()
    }

    /// Oldest unadopted send (feeds the workspace row badge).
    pub(crate) fn send_state(&self) -> Option<SendState> {
        oldest_state(&lock(&self.state).pending)
    }

    pub(crate) fn has_pending_sends(&self) -> bool {
        !lock(&self.state).pending.is_empty()
    }

    /// Apply queued doc events + time-driven state; publish what changed.
    pub(crate) fn refresh(&self) {
        let Ok(client) = self.client() else { return };
        let dirty = std::mem::take(&mut *lock(&self.dirty));
        let now = now_ms();
        let degraded = client.chat_delivery_degraded(&self.chat_id);
        let (indicator_working, working_since_ms) = client
            .workspace
            .snapshot()
            .session(&self.chat_id)
            .map_or((false, None), |row| {
                (row.indicator == ChatIndicator::Working, row.working_since_ms)
            });
        let mut transcript_event = None;
        let send_before;
        let send_after;
        {
            let mut st = lock(&self.state);
            send_before = oldest_state(&st.pending);
            let change = st.tracker.refresh(self.doc.doc(), &dirty);
            if dirty.queue {
                st.queue = self.doc.read_queue().unwrap_or_default();
            }
            if dirty.commands {
                st.commands = self.doc.read_commands().unwrap_or_default();
            }
            if dirty.meta {
                st.context_usage = self.doc.context_usage();
            }
            let echoes_changed = derive_pending(&mut st, &client.config.device_id, degraded, now);
            send_after = oldest_state(&st.pending);
            let previous = self.snapshot();
            let streaming = st.tracker.entries().last().is_some_and(|e| e.is_streaming());
            let live = LiveFlags {
                working: indicator_working || streaming,
                working_since_ms,
            };
            let live_changed = previous.working != live.working
                || previous.working_since_ms != live.working_since_ms
                || previous.hydrated != st.hydrated;
            if change.is_some()
                || echoes_changed
                || dirty.meta
                || live_changed
                || st.transcript_revision == 0
            {
                st.transcript_revision += 1;
                let snapshot = build_snapshot(&self.chat_id, &st, change, live, &previous);
                transcript_event = Some(snapshot.revision);
                self.transcript_tx.send_replace(Arc::new(snapshot));
            }
        }
        if let Some(revision) = transcript_event {
            client.events.session(&self.chat_id, revision);
        }
        self.recompute_composer(&client);
        if send_before != send_after {
            client.recompute_workspace();
        }
    }

    /// Rebuild the composer view (also after workspace/connectivity changes).
    pub(crate) fn recompute_composer(&self, client: &Arc<ClientInner>) {
        let workspace = client.workspace.snapshot();
        let row = workspace.session(&self.chat_id).cloned();
        let snapshot = self.snapshot();
        let degraded = client.chat_delivery_degraded(&self.chat_id);
        let room = client.room_state(&self.chat_id);
        let mut st = lock(&self.state);
        let host_id = row.as_ref().map(|r| r.device_id.clone()).unwrap_or_default();
        let device = workspace.device(&host_id);
        let capabilities = client.host_capabilities(&host_id, row.as_ref().and_then(|r| r.harness.as_deref()));
        let indicator = row.as_ref().map_or(ChatIndicator::Idle, |r| r.indicator);
        let busy = matches!(indicator, ChatIndicator::Working | ChatIndicator::AwaitingInput);
        let mut next = ComposerState {
            chat_id: self.chat_id.clone(),
            revision: 0,
            title: row.as_ref().map_or_else(|| "New session".into(), |r| r.title.clone()),
            host: HostInfo {
                device_id: host_id.clone(),
                name: device.map(|d| d.name.clone()),
                online: device.is_some_and(|d| d.online),
                capabilities,
            },
            live: LiveStatus {
                indicator,
                working_since_ms: row.as_ref().and_then(|r| r.working_since_ms),
                streaming: snapshot.streaming,
                can_interrupt: busy || snapshot.streaming,
            },
            queue: st
                .queue
                .iter()
                .map(|item| queue_item(item, &client.config.device_id, &st.queue_actions_pending))
                .collect(),
            pending_sends: st.pending.clone(),
            send_state: oldest_state(&st.pending),
            delivery_degraded: degraded,
            room,
            transfer_progress: st.transfer_progress,
            open_input: open_input(st.tracker.entries()),
            queue_error: st.queue_error.clone(),
            last_submitted_message_id: st.last_submitted.clone(),
            context_usage: st.context_usage,
        };
        let current = self.composer();
        next.revision = current.revision;
        if *current == next {
            return;
        }
        st.composer_revision += 1;
        next.revision = st.composer_revision;
        drop(st);
        let revision = next.revision;
        *write(&self.composer) = Arc::new(next);
        client.events.composer(&self.chat_id, revision);
    }

    fn update_state(&self, f: impl FnOnce(&mut CoreState)) {
        f(&mut lock(&self.state));
        if let Ok(client) = self.client() {
            self.recompute_composer(&client);
        }
    }

    #[allow(dead_code)] // attachment escort progress (live uploads)
    pub(crate) fn set_transfer_progress(&self, progress: Option<f64>) {
        self.update_state(|st| st.transfer_progress = progress);
    }

    fn last_entry_id(&self) -> Option<String> {
        self.snapshot()
            .entries
            .get(..self.snapshot().transcript_len)
            .and_then(|e| e.last())
            .map(|e| e.id.clone())
    }

    fn queue_command(&self, device_id: &str, payload: SessionCommandPayload) -> Result<String> {
        let now = now_ms();
        let entry = SessionCommandEntry {
            id: crate::new_id(),
            payload,
            issued_by: device_id.to_owned(),
            issued_at: now,
            based_on: self.last_entry_id().map(|turn_id| CommandBasedOn {
                turn_id: Some(turn_id),
                frontier: None,
            }),
            expires_at: Some(now + zeron_doc::COMMAND_DEFAULT_TTL_MS),
            status: SessionCommandStatus::Pending,
            resolution: None,
        };
        let id = entry.id.clone();
        self.write(|doc| doc.queue_command(&entry))?;
        Ok(id)
    }

    pub(crate) fn view_attached(&self) -> bool {
        self.view_attached.load(Ordering::Acquire)
    }

    pub(crate) fn touch(&self) {
        self.touched_ms.store(now_ms(), Ordering::Release);
    }

    pub(crate) fn touched_ms(&self) -> i64 {
        self.touched_ms.load(Ordering::Acquire)
    }
}

fn empty_composer(chat_id: &str) -> ComposerState {
    ComposerState {
        chat_id: chat_id.to_owned(),
        revision: 0,
        title: "New session".into(),
        host: HostInfo {
            device_id: String::new(),
            name: None,
            online: false,
            capabilities: HostCapabilities::default(),
        },
        live: LiveStatus {
            indicator: ChatIndicator::Idle,
            working_since_ms: None,
            streaming: false,
            can_interrupt: false,
        },
        queue: Vec::new(),
        pending_sends: Vec::new(),
        send_state: None,
        delivery_degraded: false,
        room: RoomState::default(),
        transfer_progress: None,
        open_input: None,
        queue_error: None,
        last_submitted_message_id: None,
        context_usage: None,
    }
}

fn oldest_state(pending: &[PendingSend]) -> Option<SendState> {
    pending
        .iter()
        .min_by_key(|p| p.sent_at_ms)
        .map(|oldest| {
            if pending.iter().any(|p| p.state == SendState::Failed) {
                SendState::Failed
            } else {
                oldest.state
            }
        })
}

fn queue_item(item: &QueuedMessage, device_id: &str, pending: &HashSet<String>) -> QueueItem {
    QueueItem {
        id: item.id.clone(),
        text: item.text.clone(),
        visible_text: attachments::queue_visible_text(&item.text, &item.attachments),
        attachments: item.attachments.clone(),
        hold_for_turn_end: item.hold_for_turn_end,
        issued_by: item.issued_by.clone(),
        from_this_device: item.issued_by == device_id,
        issued_at_ms: item.issued_at,
        edited_at_ms: item.edited_at,
        gate: item.delivery_gate.as_ref().map(|gate| match gate {
            QueueDeliveryGate::Editing {
                owner_device_id,
                expires_at_ms,
                ..
            } => QueueGate::Editing {
                owner_device_id: owner_device_id.clone(),
                expires_at_ms: *expires_at_ms,
                mine: owner_device_id == device_id,
            },
            QueueDeliveryGate::ReviewRequired {
                owner_device_id, ..
            } => QueueGate::ReviewRequired {
                owner_device_id: owner_device_id.clone(),
            },
        }),
        action_pending: pending.contains(&item.id),
    }
}

/// The newest unresolved question with at least one question to answer.
fn open_input(entries: &[Arc<Entry>]) -> Option<InputRequest> {
    for entry in entries.iter().rev() {
        for part in entry.message.parts.iter().rev() {
            if let MessagePart::Input {
                request_id,
                questions,
                resolved: false,
                ..
            } = part
                && !questions.is_empty()
            {
                return Some(InputRequest {
                    entry_id: entry.id.clone(),
                    request_id: request_id.clone(),
                    questions: questions.clone(),
                });
            }
        }
    }
    None
}

/// Own run/steer commands whose message hasn't landed = pending echoes.
/// Returns whether the echo set/states changed.
fn derive_pending(st: &mut CoreState, device_id: &str, degraded: bool, now: i64) -> bool {
    struct Attempt {
        text: String,
        kind: PendingKind,
        first_issued: i64,
        live: bool,
        dead: bool,
    }
    let mut order: Vec<String> = Vec::new();
    let mut attempts: HashMap<String, Attempt> = HashMap::new();
    for command in &st.commands {
        if command.issued_by != device_id {
            continue;
        }
        let (message_id, text, kind) = match &command.payload {
            SessionCommandPayload::Run {
                request,
                message_id,
            } => (message_id, &request.prompt, PendingKind::Run),
            SessionCommandPayload::Steer {
                prompt,
                message_id: Some(message_id),
            } => (message_id, prompt, PendingKind::Steer),
            _ => continue,
        };
        if st.tracker.contains_id(message_id) {
            continue;
        }
        let expired = now >= command.effective_expiry();
        let (live, dead) = match command.status {
            SessionCommandStatus::Pending => (!expired, expired),
            SessionCommandStatus::Applied => (true, false),
            SessionCommandStatus::Rejected
            | SessionCommandStatus::Expired
            | SessionCommandStatus::Superseded => (false, true),
            SessionCommandStatus::Cancelled => continue,
        };
        let attempt = attempts.entry(message_id.clone()).or_insert_with(|| {
            order.push(message_id.clone());
            Attempt {
                text: text.clone(),
                kind,
                first_issued: command.issued_at,
                live: false,
                dead: false,
            }
        });
        attempt.live |= live;
        attempt.dead |= dead;
    }
    let mut next = Vec::with_capacity(order.len());
    for message_id in order {
        let attempt = &attempts[&message_id];
        let started = st
            .grace_started
            .get(&message_id)
            .copied()
            .unwrap_or(attempt.first_issued);
        let parsed = attachments::parse_user_message(&attempt.text);
        next.push(PendingSend {
            state: send_state(started, degraded, attempt.dead && !attempt.live, now),
            message_id,
            text: attempt.text.clone(),
            visible_text: parsed.text,
            images: parsed.images.into_iter().map(|i| i.path).collect(),
            kind: attempt.kind,
            sent_at_ms: attempt.first_issued,
        });
    }
    let live_ids: HashSet<&str> = next.iter().map(|p| p.message_id.as_str()).collect();
    st.grace_started.retain(|id, _| live_ids.contains(id.as_str()));
    let changed = next != st.pending;
    if changed {
        // Rebuild echo entries, reusing unchanged Arcs.
        let mut echoes = HashMap::with_capacity(next.len());
        for pending in &next {
            let reuse = st.echoes.get(&pending.message_id).filter(|e| {
                e.echo.as_ref().map(|x| x.state) == Some(pending.state)
                    && matches!(e.message.parts.first(), Some(MessagePart::Text { text, .. }) if *text == pending.text)
            });
            let entry = match reuse {
                Some(entry) => entry.clone(),
                None => {
                    let rev = st.tracker.next_rev();
                    Arc::new(Entry {
                        id: pending.message_id.clone(),
                        rev,
                        message: Arc::new(SessionMessageEntry {
                            id: pending.message_id.clone(),
                            role: MessageRole::User,
                            parts: vec![MessagePart::Text {
                                id: "t0".into(),
                                text: pending.text.clone(),
                            }],
                            created_at: pending.sent_at_ms,
                            device_id: device_id.to_owned(),
                            status: Some(MessageStatus::Complete),
                            continuation_of: None,
                            duration_ms: None,
                        }),
                        echo: Some(LocalEcho {
                            state: pending.state,
                            sent_at_ms: pending.sent_at_ms,
                        }),
                        append: None,
                    })
                }
            };
            echoes.insert(pending.message_id.clone(), entry);
        }
        st.echoes = echoes;
        st.pending = next;
    }
    changed
}

#[derive(Clone, Copy)]
struct LiveFlags {
    working: bool,
    working_since_ms: Option<i64>,
}

fn build_snapshot(
    chat_id: &str,
    st: &CoreState,
    change: Option<transcript::TranscriptChange>,
    live: LiveFlags,
    previous: &SessionSnapshot,
) -> SessionSnapshot {
    let transcript = st.tracker.entries();
    let mut entries: Vec<Arc<Entry>> = Vec::with_capacity(transcript.len() + st.pending.len());
    entries.extend(transcript.iter().cloned());
    let transcript_len = entries.len();
    for pending in &st.pending {
        if let Some(echo) = st.echoes.get(&pending.message_id) {
            entries.push(echo.clone());
        }
    }
    let index: HashMap<String, usize> = entries
        .iter()
        .enumerate()
        .map(|(i, e)| (e.id.clone(), i))
        .collect();
    let streaming = transcript.last().is_some_and(|e| e.is_streaming());
    let mut snapshot = SessionSnapshot {
        chat_id: chat_id.to_owned(),
        revision: st.transcript_revision,
        entries,
        transcript_len,
        streaming,
        working: live.working,
        working_since_ms: live.working_since_ms,
        pending: st.pending.clone(),
        context_usage: st.context_usage,
        hydrated: st.hydrated,
        delta: SnapshotDelta::default(),
        index: Arc::new(index),
    };
    snapshot.delta = match change {
        Some(change) if change.reset => SnapshotDelta {
            reset: true,
            changed: snapshot.entries.iter().map(|e| e.id.clone()).collect(),
            removed: Vec::new(),
        },
        _ => snapshot.changes_since(previous),
    };
    snapshot
}

/// Stops a [`SessionHandle::watch`] when dropped.
#[derive(Debug)]
pub struct SnapshotWatch {
    cancel: tokio_util::sync::CancellationToken,
}

impl Drop for SnapshotWatch {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

/// One open chat. Cheap to clone; all clones share the session.
#[derive(Clone)]
pub struct SessionHandle {
    pub(crate) core: Arc<SessionCore>,
}

impl std::fmt::Debug for SessionHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionHandle")
            .field("chat_id", &self.core.chat_id)
            .finish()
    }
}

impl SessionHandle {
    pub fn chat_id(&self) -> &str {
        &self.core.chat_id
    }

    /// The current transcript (O(1): an `Arc` clone).
    pub fn snapshot(&self) -> Arc<SessionSnapshot> {
        self.core.snapshot()
    }

    /// Transcript subscription for Rust consumers (the layout engine). The
    /// watch coalesces: after `changed()`, diff with
    /// [`SessionSnapshot::changes_since`] against the last snapshot you
    /// processed rather than trusting `delta` alone.
    pub fn subscribe(&self) -> watch::Receiver<Arc<SessionSnapshot>> {
        self.core.subscribe()
    }

    /// Callback-style subscription: `f` gets the current snapshot at once,
    /// then every newer one (coalesced — diff with
    /// [`SessionSnapshot::changes_since`]). Runs on a client runtime thread;
    /// dropping the returned guard stops it.
    pub fn watch(&self, f: impl Fn(Arc<SessionSnapshot>) + Send + Sync + 'static) -> SnapshotWatch {
        let mut rx = self.subscribe();
        let cancel = tokio_util::sync::CancellationToken::new();
        let token = cancel.clone();
        crate::runtime::shared().spawn(async move {
            let first = rx.borrow_and_update().clone();
            f(first);
            loop {
                tokio::select! {
                    _ = token.cancelled() => return,
                    changed = rx.changed() => if changed.is_err() { return },
                }
                let next = rx.borrow_and_update().clone();
                f(next);
            }
        });
        SnapshotWatch { cancel }
    }

    pub fn composer(&self) -> Arc<ComposerState> {
        self.core.composer()
    }

    /// Tell the client whether a transcript view is on screen (marks seen on
    /// attach; prioritizes this room's sync in live mode).
    pub fn set_view_attached(&self, attached: bool) {
        let was = self.core.view_attached.swap(attached, Ordering::AcqRel);
        self.core.touch();
        if attached && !was {
            self.mark_seen();
        }
    }

    pub fn mark_seen(&self) {
        if let Ok(client) = self.core.client() {
            client.mark_seen(&self.core.chat_id);
        }
    }

    /// Send a message: a new turn when idle; while the agent works, park it
    /// on the shared queue (or steer, per `busy`). Attachments ride
    /// `pending://` refs; their bytes are escorted to the host.
    pub fn send(&self, request: SendRequest) -> Result<SendOutcome> {
        let client = self.core.client()?;
        let chat = client
            .workspace
            .chat(&self.core.chat_id)
            .ok_or_else(|| ClientError::NotFound(self.core.chat_id.clone()))?;
        if request.text.trim().is_empty() && request.attachments.is_empty() {
            return Err(ClientError::InvalidArgument("empty message".into()));
        }
        let composer = self.composer();
        let busy = matches!(composer.live.indicator, ChatIndicator::Working) || composer.live.streaming;
        let caps = &composer.host.capabilities;
        let refs = client.stage_attachments(&chat.device_id, &request.attachments)?;
        let prompt_text = request.text.trim_end().to_owned();
        let device_id = client.config.device_id.clone();

        let outcome = if busy && caps.message_queue && (refs.is_empty() || caps.queue_attachments) {
            let text = if refs.is_empty() || caps.clean_attachment_text {
                if prompt_text.is_empty() {
                    attachments::ATTACHMENT_ONLY_TEXT.to_owned()
                } else {
                    prompt_text.clone()
                }
            } else {
                attachments::with_attachments(&prompt_text, &refs)
            };
            let id = self.enqueue_inner(&device_id, &text, refs.clone(), request.busy == BusyPolicy::Queue)?;
            SendOutcome::Queued { queue_id: id }
        } else {
            let content = attachments::with_attachments(&prompt_text, &refs);
            let message_id = crate::new_id();
            if busy {
                self.core.queue_command(
                    &device_id,
                    SessionCommandPayload::Steer {
                        prompt: content,
                        message_id: Some(message_id.clone()),
                    },
                )?;
                SendOutcome::Steered { message_id }
            } else {
                let config = chat.config.as_ref();
                let request = RunRequest {
                    prompt: content,
                    harness: config.map(|c| c.harness),
                    model: config.and_then(|c| c.model.clone()),
                    reasoning: config.and_then(|c| c.reasoning),
                    model_options: config.map(|c| c.model_options.clone()).unwrap_or_default(),
                    cwd: chat.cwd.clone().unwrap_or_else(|| "~".into()),
                    sandbox: config.map_or(SandboxLevel::WorkspaceWrite, |c| c.sandbox),
                    auto_approve: true,
                    resume: None,
                    attachments: refs.clone(),
                    worktree: request.worktree.clone(),
                    mcp: None,
                };
                self.core.queue_command(
                    &device_id,
                    SessionCommandPayload::Run {
                        request,
                        message_id: message_id.clone(),
                    },
                )?;
                SendOutcome::Started { message_id }
            }
        };
        if let SendOutcome::Started { message_id } | SendOutcome::Steered { message_id } = &outcome {
            let id = message_id.clone();
            self.core.update_state(|st| st.last_submitted = Some(id));
        }
        client.after_command(&self.core, !refs.is_empty());
        Ok(outcome)
    }

    /// Stop the live turn.
    pub fn interrupt(&self) -> Result<()> {
        let client = self.core.client()?;
        self.core
            .queue_command(&client.config.device_id, SessionCommandPayload::Interrupt {})?;
        client.after_command(&self.core, false);
        Ok(())
    }

    /// Answer the open question panel.
    pub fn respond_input(&self, request_id: &str, answers: Vec<UserInputAnswer>) -> Result<()> {
        let client = self.core.client()?;
        self.core.queue_command(
            &client.config.device_id,
            SessionCommandPayload::RespondInput {
                request_id: request_id.to_owned(),
                answers,
            },
        )?;
        client.after_command(&self.core, false);
        Ok(())
    }

    fn enqueue_inner(
        &self,
        device_id: &str,
        text: &str,
        attachments: Vec<String>,
        hold_for_turn_end: bool,
    ) -> Result<String> {
        let mut item = QueuedMessage::new(crate::new_id(), text, device_id);
        item.attachments = attachments;
        item.hold_for_turn_end = hold_for_turn_end;
        item.issued_at = now_ms();
        let id = item.id.clone();
        self.core.write(|doc| doc.push_queued(&item))?;
        Ok(id)
    }

    /// Park a message on the shared queue directly.
    pub fn enqueue(
        &self,
        text: &str,
        attachments: Vec<String>,
        hold_for_turn_end: bool,
    ) -> Result<String> {
        let client = self.core.client()?;
        if text.trim().is_empty() {
            return Err(ClientError::InvalidArgument("empty message".into()));
        }
        let id = self.enqueue_inner(&client.config.device_id, text, attachments, hold_for_turn_end)?;
        client.after_command(&self.core, false);
        Ok(id)
    }

    /// Move a queue row to `to` (clamped). Rows under a delivery gate or with
    /// an action in flight don't move.
    pub fn move_queued(&self, id: &str, to: usize) -> Result<bool> {
        {
            let st = lock(&self.core.state);
            let row = st.queue.iter().find(|q| q.id == id);
            if row.is_none_or(|r| r.delivery_gate.is_some()) || st.queue_actions_pending.contains(id) {
                return Ok(false);
            }
        }
        let moved = self.core.write(|doc| doc.move_queued(id, to))?;
        if moved && let Ok(client) = self.core.client() {
            client.after_command(&self.core, false);
        }
        Ok(moved)
    }

    /// Nudge a row one slot up (`-1`) or down (`+1`).
    pub fn move_queued_by(&self, id: &str, delta: i32) -> Result<bool> {
        let (from, count) = {
            let st = lock(&self.core.state);
            (st.queue.iter().position(|q| q.id == id), st.queue.len())
        };
        let Some(from) = from else { return Ok(false) };
        let to = from as i64 + delta as i64;
        if to < 0 || to >= count as i64 {
            return Ok(false);
        }
        self.move_queued(id, to as usize)
    }

    async fn queue_rpc(&self, method: &'static str, params: serde_json::Value) -> Result<serde_json::Value> {
        let client = self.core.client()?;
        let host = self.composer().host.device_id.clone();
        if host.is_empty() {
            return Err(ClientError::HostUnavailable("no host".into()));
        }
        client.host_rpc(&host, method, params).await
    }

    pub async fn begin_queued_edit(&self, id: &str, instance_id: &str) -> QueueEditStart {
        let composer = self.composer();
        if !composer.host.capabilities.queue_edit_lease
            || !composer.queue.iter().any(|q| q.id == id && !q.action_pending)
        {
            return QueueEditStart::Unavailable;
        }
        let Ok(client) = self.core.client() else {
            return QueueEditStart::Unavailable;
        };
        let reply = self
            .queue_rpc(
                zeron_rpc::methods::BEGIN_QUEUED_MESSAGE_EDIT,
                serde_json::json!({
                    "chatId": self.core.chat_id,
                    "id": id,
                    "editorDeviceId": client.config.device_id,
                    "editorInstanceId": instance_id,
                }),
            )
            .await;
        match reply {
            Ok(value) => match value.get("outcome").and_then(|o| o.as_str()) {
                Some("acquired") => {
                    let field = |k: &str| value.get(k).and_then(|v| v.as_str()).map(str::to_owned);
                    match (field("leaseId"), field("baseTextHash"), value.get("expiresAtMs").and_then(|v| v.as_i64())) {
                        (Some(lease_id), Some(base_text_hash), Some(expires_at_ms)) => {
                            QueueEditStart::Acquired(QueueEditLease {
                                row_id: id.to_owned(),
                                lease_id,
                                text: field("text").unwrap_or_default(),
                                base_text_hash,
                                expires_at_ms,
                            })
                        }
                        _ => QueueEditStart::Unavailable,
                    }
                }
                Some("locked") => QueueEditStart::Locked,
                _ => QueueEditStart::Missing,
            },
            Err(_) => QueueEditStart::Unavailable,
        }
    }

    pub async fn renew_queued_edit(&self, lease: &QueueEditLease) -> bool {
        self.queue_rpc(
            zeron_rpc::methods::RENEW_QUEUED_MESSAGE_EDIT,
            serde_json::json!({
                "chatId": self.core.chat_id,
                "id": lease.row_id,
                "leaseId": lease.lease_id,
            }),
        )
        .await
        .is_ok_and(|v| v.get("outcome").and_then(|o| o.as_str()) == Some("renewed"))
    }

    pub async fn finish_queued_edit(
        &self,
        lease: &QueueEditLease,
        action: QueueEditAction,
        text: Option<String>,
    ) -> QueueEditFinish {
        let mut params = serde_json::json!({
            "chatId": self.core.chat_id,
            "id": lease.row_id,
            "leaseId": lease.lease_id,
            "action": action.wire(),
            "expectedTextHash": lease.base_text_hash,
        });
        if let Some(text) = text {
            params["text"] = serde_json::Value::String(text);
        }
        match self
            .queue_rpc(zeron_rpc::methods::FINISH_QUEUED_MESSAGE_EDIT, params)
            .await
        {
            Ok(value) => match value.get("outcome").and_then(|o| o.as_str()) {
                Some("committed" | "cancelled" | "discarded" | "released") => QueueEditFinish::Finished,
                Some("conflict") => QueueEditFinish::Conflict,
                Some("missing") => QueueEditFinish::Missing,
                _ => QueueEditFinish::Lost,
            },
            Err(_) => QueueEditFinish::Unavailable,
        }
    }

    async fn queue_action(&self, id: &str, send_now: bool) -> Result<bool> {
        {
            let mut st = lock(&self.core.state);
            let Some(row) = st.queue.iter().find(|q| q.id == id) else {
                return Ok(false);
            };
            if st.queue_actions_pending.contains(id) || (send_now && row.delivery_gate.is_some()) {
                return Ok(false);
            }
            st.queue_actions_pending.insert(id.to_owned());
            st.queue_error = None;
        }
        if let Ok(client) = self.core.client() {
            self.core.recompute_composer(&client);
        }
        let method = if send_now {
            zeron_rpc::methods::SEND_QUEUED_MESSAGE_NOW
        } else {
            zeron_rpc::methods::REMOVE_QUEUED_MESSAGE
        };
        let result = self
            .queue_rpc(method, serde_json::json!({ "chatId": self.core.chat_id, "id": id }))
            .await;
        let label = if send_now { "send now" } else { "remove" };
        let key = if send_now { "sent" } else { "removed" };
        let outcome = match result {
            Ok(value) if value.get(key).and_then(|v| v.as_bool()) == Some(true) => {
                if !send_now {
                    // Apply only a confirmed removal; sends leave it to sync.
                    let _ = self.core.write(|doc| doc.remove_queued(id));
                }
                Ok(true)
            }
            Ok(_) => Err("The host did not confirm the action. The message may have already left the queue.".to_owned()),
            Err(err) => Err(format!(
                "Couldn't complete {label}. Check the connection to the chat host and the queue before retrying. ({err})"
            )),
        };
        let id = id.to_owned();
        match outcome {
            Ok(done) => {
                self.core.update_state(|st| {
                    st.queue_actions_pending.remove(&id);
                });
                Ok(done)
            }
            Err(message) => {
                self.core.update_state(|st| {
                    st.queue_actions_pending.remove(&id);
                    st.queue_error = Some(message.clone());
                });
                Err(ClientError::HostUnavailable(message))
            }
        }
    }

    /// Deliver a queued row now (interrupting the live turn if needed).
    pub async fn send_queued_now(&self, id: &str) -> Result<bool> {
        self.queue_action(id, true).await
    }

    /// Remove a queued row (applied locally only after the host acks).
    pub async fn remove_queued(&self, id: &str) -> Result<bool> {
        self.queue_action(id, false).await
    }

    pub fn clear_queue_error(&self) {
        self.core.update_state(|st| st.queue_error = None);
    }

    /// "Not delivered — tap to retry": restart grace clocks, re-issue dead
    /// attempts (fresh command id, same message id — the host dedupes the
    /// user entry by message id), kick the room, nudge the host.
    pub fn retry_delivery(&self) -> Result<()> {
        let client = self.core.client()?;
        let now = now_ms();
        let device_id = client.config.device_id.clone();
        let (commands, pending_ids) = {
            let mut st = lock(&self.core.state);
            let ids: Vec<String> = st.pending.iter().map(|p| p.message_id.clone()).collect();
            for id in &ids {
                st.grace_started.insert(id.clone(), now);
            }
            (st.commands.clone(), ids)
        };
        let landed = |id: &str| !pending_ids.iter().any(|p| p == id);
        let mut latest_dead: HashMap<String, SessionCommandEntry> = HashMap::new();
        let mut live: HashSet<String> = HashSet::new();
        for command in &commands {
            if command.issued_by != device_id {
                continue;
            }
            let message_id = match &command.payload {
                SessionCommandPayload::Run { message_id, .. } => message_id.clone(),
                SessionCommandPayload::Steer {
                    message_id: Some(message_id),
                    ..
                } => message_id.clone(),
                _ => continue,
            };
            if landed(&message_id) {
                continue;
            }
            let expired = now >= command.effective_expiry();
            match command.status {
                SessionCommandStatus::Pending if !expired => {
                    live.insert(message_id);
                }
                SessionCommandStatus::Applied => {
                    live.insert(message_id);
                }
                SessionCommandStatus::Rejected
                | SessionCommandStatus::Expired
                | SessionCommandStatus::Pending => {
                    let newer = latest_dead
                        .get(&message_id)
                        .is_none_or(|existing| existing.issued_at < command.issued_at);
                    if newer {
                        latest_dead.insert(message_id, command.clone());
                    }
                }
                _ => {}
            }
        }
        for (message_id, attempt) in latest_dead {
            if live.contains(&message_id) {
                continue;
            }
            tracing::info!(chat = %self.core.chat_id, old = %attempt.id, "retry re-issues a dead send attempt");
            self.core.queue_command(&device_id, attempt.payload.clone())?;
        }
        self.core.refresh();
        client.after_command(&self.core, true);
        client.kick_room(&self.core.chat_id);
        Ok(())
    }
}
