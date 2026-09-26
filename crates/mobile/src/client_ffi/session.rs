//! The per-chat FFI object: composer-facing state and every command.
//! Transcript rows never cross here — the layout engine reads
//! [`zeron_client::SessionSnapshot`] in Rust.

use std::sync::Arc;

use zeron_client as zc;

use super::types::{CoreResult, SendState};

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum BusyPolicy {
    /// Park it on the shared queue until the turn ends (default).
    Queue,
    /// Deliver mid-turn (steer).
    Steer,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct OutgoingAttachment {
    pub name: String,
    pub mime_type: String,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct WorktreeSpec {
    /// The repo to branch (the project's folder on the host).
    pub repo_path: String,
    /// Base ref for the fresh `zeron/<name>` branch.
    pub base: String,
    pub space_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct SendRequest {
    pub text: String,
    pub attachments: Vec<OutgoingAttachment>,
    /// Isolated host worktree for this run (new-session "New worktree").
    pub worktree: Option<WorktreeSpec>,
    pub busy: BusyPolicy,
}

impl From<SendRequest> for zc::SendRequest {
    fn from(r: SendRequest) -> Self {
        zc::SendRequest {
            text: r.text,
            attachments: r
                .attachments
                .into_iter()
                .map(|a| zc::OutgoingAttachment {
                    name: a.name,
                    mime_type: a.mime_type,
                    data: a.data,
                })
                .collect(),
            worktree: r.worktree.map(|w| zc::WorktreeSpec {
                repo_path: w.repo_path,
                base: w.base,
                space_id: w.space_id,
            }),
            busy: match r.busy {
                BusyPolicy::Queue => zc::BusyPolicy::Queue,
                BusyPolicy::Steer => zc::BusyPolicy::Steer,
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum SendOutcome {
    /// A new turn was requested; the echo's id is `message_id`.
    Started { message_id: String },
    /// Delivered into the live turn.
    Steered { message_id: String },
    /// Parked on the shared queue.
    Queued { queue_id: String },
}

impl From<zc::SendOutcome> for SendOutcome {
    fn from(o: zc::SendOutcome) -> Self {
        match o {
            zc::SendOutcome::Started { message_id } => SendOutcome::Started { message_id },
            zc::SendOutcome::Steered { message_id } => SendOutcome::Steered { message_id },
            zc::SendOutcome::Queued { queue_id } => SendOutcome::Queued { queue_id },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct HostCapabilities {
    pub message_queue: bool,
    pub queue_actions: bool,
    pub queue_attachments: bool,
    pub clean_attachment_text: bool,
    pub queue_edit_lease: bool,
    /// Host ≥ 0.2.12: attachments ride `pending://` refs.
    pub queued_attachments: bool,
    /// The chat's harness steers mid-turn (unknown until a live catalog).
    pub mid_turn_steering: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct HostInfo {
    pub device_id: String,
    pub name: Option<String>,
    pub online: bool,
    pub capabilities: HostCapabilities,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct LiveStatus {
    pub indicator: super::types::ChatIndicator,
    pub working_since_ms: Option<i64>,
    /// The newest transcript entry is streaming.
    pub streaming: bool,
    /// Stop is meaningful.
    pub can_interrupt: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum QueueGate {
    /// Someone holds an edit lease; the row won't deliver until released.
    Editing {
        owner_device_id: String,
        expires_at_ms: i64,
        mine: bool,
    },
    /// A lease lapsed — a person must review before it sends.
    ReviewRequired { owner_device_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct QueueItem {
    pub id: String,
    /// Raw row text (what the host will send).
    pub text: String,
    /// User-visible text (trailer/Appshot context stripped).
    pub visible_text: String,
    pub attachments: Vec<String>,
    pub hold_for_turn_end: bool,
    pub issued_by: String,
    pub from_this_device: bool,
    pub issued_at_ms: i64,
    pub edited_at_ms: Option<i64>,
    pub gate: Option<QueueGate>,
    /// A Send-now / Remove is awaiting the host's acknowledgement.
    pub action_pending: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum PendingKind {
    Run,
    Steer,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct PendingSend {
    /// Client-minted id (the host's user entry reuses it).
    pub message_id: String,
    /// Raw content (attachment trailer included).
    pub text: String,
    pub visible_text: String,
    pub images: Vec<String>,
    pub kind: PendingKind,
    pub sent_at_ms: i64,
    pub state: SendState,
}

impl From<&zc::PendingSend> for PendingSend {
    fn from(p: &zc::PendingSend) -> Self {
        Self {
            message_id: p.message_id.clone(),
            text: p.text.clone(),
            visible_text: p.visible_text.clone(),
            images: p.images.clone(),
            kind: match p.kind {
                zc::PendingKind::Run => PendingKind::Run,
                zc::PendingKind::Steer => PendingKind::Steer,
            },
            sent_at_ms: p.sent_at_ms,
            state: p.state.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct UserInputQuestion {
    pub id: String,
    pub header: String,
    pub question: String,
    pub options: Vec<String>,
    pub multi_select: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct UserInputAnswer {
    pub question_id: String,
    /// Picked option labels (or free text as a single label).
    pub labels: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct InputRequest {
    pub entry_id: String,
    pub request_id: String,
    pub questions: Vec<UserInputQuestion>,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct RoomState {
    pub connected: bool,
    pub retry_at_ms: Option<i64>,
    /// Graced degradation for this chat.
    pub degraded: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct ContextUsage {
    pub tokens: Option<u64>,
    pub window: Option<u64>,
}

/// Everything the composer and its status strip render.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct ComposerState {
    pub chat_id: String,
    pub revision: u64,
    pub title: String,
    pub host: HostInfo,
    pub live: LiveStatus,
    /// The shared queue, delivery order.
    pub queue: Vec<QueueItem>,
    /// This device's unadopted sends, oldest first.
    pub pending_sends: Vec<PendingSend>,
    /// Headline state of the oldest pending send.
    pub send_state: Option<SendState>,
    /// A send now would queue rather than deliver promptly.
    pub delivery_degraded: bool,
    pub room: RoomState,
    /// Attachment escort progress (0..1).
    pub transfer_progress: Option<f64>,
    /// Answer this instead of typing.
    pub open_input: Option<InputRequest>,
    /// Last queue action failure.
    pub queue_error: Option<String>,
    /// Scroll-to-own-send target.
    pub last_submitted_message_id: Option<String>,
    pub context_usage: Option<ContextUsage>,
}

impl From<&zc::ComposerState> for ComposerState {
    fn from(c: &zc::ComposerState) -> Self {
        let caps = &c.host.capabilities;
        Self {
            chat_id: c.chat_id.clone(),
            revision: c.revision,
            title: c.title.clone(),
            host: HostInfo {
                device_id: c.host.device_id.clone(),
                name: c.host.name.clone(),
                online: c.host.online,
                capabilities: HostCapabilities {
                    message_queue: caps.message_queue,
                    queue_actions: caps.queue_actions,
                    queue_attachments: caps.queue_attachments,
                    clean_attachment_text: caps.clean_attachment_text,
                    queue_edit_lease: caps.queue_edit_lease,
                    queued_attachments: caps.queued_attachments,
                    mid_turn_steering: caps.mid_turn_steering,
                },
            },
            live: LiveStatus {
                indicator: c.live.indicator.into(),
                working_since_ms: c.live.working_since_ms,
                streaming: c.live.streaming,
                can_interrupt: c.live.can_interrupt,
            },
            queue: c
                .queue
                .iter()
                .map(|q| QueueItem {
                    id: q.id.clone(),
                    text: q.text.clone(),
                    visible_text: q.visible_text.clone(),
                    attachments: q.attachments.clone(),
                    hold_for_turn_end: q.hold_for_turn_end,
                    issued_by: q.issued_by.clone(),
                    from_this_device: q.from_this_device,
                    issued_at_ms: q.issued_at_ms,
                    edited_at_ms: q.edited_at_ms,
                    gate: q.gate.as_ref().map(|g| match g {
                        zc::QueueGate::Editing {
                            owner_device_id,
                            expires_at_ms,
                            mine,
                        } => QueueGate::Editing {
                            owner_device_id: owner_device_id.clone(),
                            expires_at_ms: *expires_at_ms,
                            mine: *mine,
                        },
                        zc::QueueGate::ReviewRequired { owner_device_id } => {
                            QueueGate::ReviewRequired {
                                owner_device_id: owner_device_id.clone(),
                            }
                        }
                    }),
                    action_pending: q.action_pending,
                })
                .collect(),
            pending_sends: c.pending_sends.iter().map(Into::into).collect(),
            send_state: c.send_state.map(Into::into),
            delivery_degraded: c.delivery_degraded,
            room: RoomState {
                connected: c.room.connected,
                retry_at_ms: c.room.retry_at_ms,
                degraded: c.room.degraded,
            },
            transfer_progress: c.transfer_progress,
            open_input: c.open_input.as_ref().map(|i| InputRequest {
                entry_id: i.entry_id.clone(),
                request_id: i.request_id.clone(),
                questions: i
                    .questions
                    .iter()
                    .map(|q| UserInputQuestion {
                        id: q.id.clone(),
                        header: q.header.clone(),
                        question: q.question.clone(),
                        options: q.options.clone(),
                        multi_select: q.multi_select,
                    })
                    .collect(),
            }),
            queue_error: c.queue_error.clone(),
            last_submitted_message_id: c.last_submitted_message_id.clone(),
            context_usage: c.context_usage.map(|u| ContextUsage {
                tokens: u.tokens,
                window: u.window,
            }),
        }
    }
}

/// Transcript-level facts the platform needs without the rows (loader vs
/// empty state, scroll-to-bottom affordances, the working pill).
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct TranscriptStatus {
    pub revision: u64,
    /// Content present (false = show a loader, not an empty chat).
    pub hydrated: bool,
    /// Host-written entries.
    pub entry_count: u32,
    pub pending_count: u32,
    pub streaming: bool,
    pub working: bool,
    pub working_since_ms: Option<i64>,
    /// Id of the newest entry (host or echo).
    pub last_entry_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct QueueEditLease {
    pub row_id: String,
    pub lease_id: String,
    /// Row text at lease time.
    pub text: String,
    pub base_text_hash: String,
    pub expires_at_ms: i64,
}

impl From<zc::QueueEditLease> for QueueEditLease {
    fn from(l: zc::QueueEditLease) -> Self {
        Self {
            row_id: l.row_id,
            lease_id: l.lease_id,
            text: l.text,
            base_text_hash: l.base_text_hash,
            expires_at_ms: l.expires_at_ms,
        }
    }
}

impl From<QueueEditLease> for zc::QueueEditLease {
    fn from(l: QueueEditLease) -> Self {
        Self {
            row_id: l.row_id,
            lease_id: l.lease_id,
            text: l.text,
            base_text_hash: l.base_text_hash,
            expires_at_ms: l.expires_at_ms,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum QueueEditStart {
    Acquired { lease: QueueEditLease },
    /// Another editor holds the row.
    Locked,
    /// The row left the queue.
    Missing,
    /// Host unreachable / capability missing.
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum QueueEditFinish {
    Finished,
    /// The row changed under the lease.
    Conflict,
    Missing,
    /// The lease was superseded or expired.
    Lost,
    Unavailable,
}

/// One open chat. Obtain via `CoreClient.open_session`.
#[derive(uniffi::Object)]
pub struct SessionHandle {
    pub(crate) inner: zc::SessionHandle,
}

impl SessionHandle {
    pub(crate) fn new(inner: zc::SessionHandle) -> Arc<Self> {
        Arc::new(Self { inner })
    }

    /// The Rust handle (transcript snapshots + subscription for the layout
    /// engine).
    #[allow(dead_code)]
    pub fn handle(&self) -> &zc::SessionHandle {
        &self.inner
    }
}

/// Run an async client call on the client runtime (executor-agnostic).
async fn on_runtime<T, F>(fut: F) -> CoreResult<T>
where
    F: std::future::Future<Output = Result<T, zc::ClientError>> + Send + 'static,
    T: Send + 'static,
{
    zc::runtime::run(fut).await.map_err(Into::into)
}

#[uniffi::export]
impl SessionHandle {
    pub fn chat_id(&self) -> String {
        self.inner.chat_id().to_owned()
    }

    /// Composer + status strip state (pull after `ComposerChanged`).
    pub fn composer(&self) -> ComposerState {
        (&*self.inner.composer()).into()
    }

    pub fn transcript_status(&self) -> TranscriptStatus {
        let snap = self.inner.snapshot();
        TranscriptStatus {
            revision: snap.revision,
            hydrated: snap.hydrated,
            entry_count: snap.transcript_len as u32,
            pending_count: snap.pending.len() as u32,
            streaming: snap.streaming,
            working: snap.working,
            working_since_ms: snap.working_since_ms,
            last_entry_id: snap.entries.last().map(|e| e.id.clone()),
        }
    }

    /// Plain text of one entry's text parts (copy / share).
    pub fn message_text(&self, entry_id: String) -> Option<String> {
        let snap = self.inner.snapshot();
        let entry = snap.entry(&entry_id)?;
        let texts: Vec<&str> = entry
            .message
            .parts
            .iter()
            .filter_map(|p| match p {
                zc::MessagePart::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        let joined = texts.join("\n\n");
        Some(if entry.message.role == zc::MessageRole::User {
            zc::attachments::parse_user_message(&joined).text
        } else {
            joined
        })
    }

    /// A transcript view is on/off screen (marks seen on attach).
    pub fn set_view_attached(&self, attached: bool) {
        self.inner.set_view_attached(attached);
    }

    pub fn mark_seen(&self) {
        self.inner.mark_seen();
    }

    /// Send: a new turn when idle; while working, queue (or steer per `busy`).
    pub fn send(&self, request: SendRequest) -> CoreResult<SendOutcome> {
        Ok(self.inner.send(request.into())?.into())
    }

    pub fn interrupt(&self) -> CoreResult<()> {
        Ok(self.inner.interrupt()?)
    }

    pub fn respond_input(&self, request_id: String, answers: Vec<UserInputAnswer>) -> CoreResult<()> {
        let answers = answers
            .into_iter()
            .map(|a| zc::UserInputAnswer {
                question_id: a.question_id,
                labels: a.labels,
            })
            .collect();
        Ok(self.inner.respond_input(&request_id, answers)?)
    }

    /// Park a message on the shared queue directly. Returns the row id.
    pub fn enqueue(&self, text: String, attachments: Vec<String>, hold_for_turn_end: bool) -> CoreResult<String> {
        Ok(self.inner.enqueue(&text, attachments, hold_for_turn_end)?)
    }

    /// Move a queue row to index `to` (clamped). False when it can't move.
    pub fn move_queued(&self, id: String, to: u32) -> CoreResult<bool> {
        Ok(self.inner.move_queued(&id, to as usize)?)
    }

    /// Nudge a row one slot up (`-1`) or down (`+1`).
    pub fn move_queued_by(&self, id: String, delta: i32) -> CoreResult<bool> {
        Ok(self.inner.move_queued_by(&id, delta)?)
    }

    pub async fn begin_queued_edit(&self, id: String, instance_id: String) -> QueueEditStart {
        let inner = self.inner.clone();
        let result = zc::runtime::run(async move {
            Ok(inner.begin_queued_edit(&id, &instance_id).await)
        })
        .await;
        match result {
            Ok(zc::QueueEditStart::Acquired(lease)) => QueueEditStart::Acquired { lease: lease.into() },
            Ok(zc::QueueEditStart::Locked) => QueueEditStart::Locked,
            Ok(zc::QueueEditStart::Missing) => QueueEditStart::Missing,
            Ok(zc::QueueEditStart::Unavailable) | Err(_) => QueueEditStart::Unavailable,
        }
    }

    pub async fn renew_queued_edit(&self, lease: QueueEditLease) -> bool {
        let inner = self.inner.clone();
        let lease: zc::QueueEditLease = lease.into();
        zc::runtime::run(async move { Ok(inner.renew_queued_edit(&lease).await) })
            .await
            .unwrap_or(false)
    }

    pub async fn finish_queued_edit(
        &self,
        lease: QueueEditLease,
        action: QueueEditAction,
        text: Option<String>,
    ) -> QueueEditFinish {
        let inner = self.inner.clone();
        let lease: zc::QueueEditLease = lease.into();
        let action = match action {
            QueueEditAction::Commit => zc::QueueEditAction::Commit,
            QueueEditAction::Cancel => zc::QueueEditAction::Cancel,
            QueueEditAction::Discard => zc::QueueEditAction::Discard,
            QueueEditAction::Release => zc::QueueEditAction::Release,
        };
        let result =
            zc::runtime::run(async move { Ok(inner.finish_queued_edit(&lease, action, text).await) }).await;
        match result {
            Ok(zc::QueueEditFinish::Finished) => QueueEditFinish::Finished,
            Ok(zc::QueueEditFinish::Conflict) => QueueEditFinish::Conflict,
            Ok(zc::QueueEditFinish::Missing) => QueueEditFinish::Missing,
            Ok(zc::QueueEditFinish::Lost) => QueueEditFinish::Lost,
            Ok(zc::QueueEditFinish::Unavailable) | Err(_) => QueueEditFinish::Unavailable,
        }
    }

    /// Deliver a queued row now (interrupting the live turn if needed).
    pub async fn send_queued_now(&self, id: String) -> CoreResult<bool> {
        let inner = self.inner.clone();
        on_runtime(async move { inner.send_queued_now(&id).await }).await
    }

    /// Remove a queued row (applied locally after the host acks).
    pub async fn remove_queued(&self, id: String) -> CoreResult<bool> {
        let inner = self.inner.clone();
        on_runtime(async move { inner.remove_queued(&id).await }).await
    }

    pub fn clear_queue_error(&self) {
        self.inner.clear_queue_error();
    }

    /// "Not delivered — tap to retry".
    pub fn retry_delivery(&self) -> CoreResult<()> {
        Ok(self.inner.retry_delivery()?)
    }
}
