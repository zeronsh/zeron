//! Immutable session views: the transcript ([`SessionSnapshot`]) consumed in
//! Rust by the layout engine, and the composer-facing [`ComposerState`]
//! exported to the platform.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use zeron_doc::{MessagePart, MessageRole, MessageStatus, SessionMessageEntry};
use zeron_proto::{ChatIndicator, ContextUsage, UserInputQuestion};

use crate::connectivity::SendState;

/// A pending optimistic echo: a send from THIS device the host has not yet
/// written into the transcript. It carries the client-minted message id the
/// host will reuse, so adoption swaps the echo for the real entry under the
/// same id — no flicker, no reorder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalEcho {
    pub state: SendState,
    /// Wall time of the send (display).
    pub sent_at_ms: i64,
}

/// "Only text grew" hint for streaming. Consumer rule: if you hold a version
/// of this entry with `rev >= base_rev`, every part is unchanged except
/// `parts[part_index]`, whose text (text or reasoning body) only grew — the
/// new bytes start at *your* cached text length for that part.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AppendHint {
    pub base_rev: u64,
    pub part_index: usize,
}

/// One transcript entry (continuations already joined onto their root).
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    /// Stable id (the root message id; for an echo, the client-minted id).
    pub id: String,
    /// Per-session monotonic content revision: a new `Arc<Entry>` always has a
    /// new `rev`; an unchanged entry keeps both its `Arc` and its `rev`.
    pub rev: u64,
    pub message: SessionMessageEntry,
    /// `Some` for a local echo that is not in the doc yet.
    pub echo: Option<LocalEcho>,
    pub append: Option<AppendHint>,
}

impl Entry {
    pub fn role(&self) -> MessageRole {
        self.message.role
    }

    pub fn parts(&self) -> &[MessagePart] {
        &self.message.parts
    }

    pub fn is_streaming(&self) -> bool {
        self.message.status == Some(MessageStatus::Streaming)
    }
}

/// What changed from `revision - 1` to `revision`. Coalescing consumers that
/// skipped revisions must use [`SessionSnapshot::changes_since`] instead.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SnapshotDelta {
    /// Everything should be treated as new (first load / resync).
    pub reset: bool,
    /// Ids whose `Arc<Entry>` is new (inserted or changed), in transcript order.
    pub changed: Vec<String>,
    /// Ids no longer present.
    pub removed: Vec<String>,
}

impl SnapshotDelta {
    pub fn is_empty(&self) -> bool {
        !self.reset && self.changed.is_empty() && self.removed.is_empty()
    }
}

/// The transcript at one revision. Cheap to clone (`Arc`s all the way down).
#[derive(Debug, Clone, Default)]
pub struct SessionSnapshot {
    pub chat_id: String,
    /// Monotonic per session; bumps on every transcript change.
    pub revision: u64,
    /// Host-written entries, then this device's pending echoes.
    pub entries: Vec<Arc<Entry>>,
    /// `entries[..transcript_len]` are host-written (the doc); the rest are
    /// local echoes.
    pub transcript_len: usize,
    /// The newest host entry is still streaming.
    pub streaming: bool,
    pub context_usage: Option<ContextUsage>,
    /// Content is present (local snapshot loaded or first sync landed). False
    /// = show a loader, not an empty chat.
    pub hydrated: bool,
    pub delta: SnapshotDelta,
    pub(crate) index: Arc<HashMap<String, usize>>,
}

impl SessionSnapshot {
    pub fn entry(&self, id: &str) -> Option<&Arc<Entry>> {
        self.index.get(id).and_then(|&ix| self.entries.get(ix))
    }

    pub fn position(&self, id: &str) -> Option<usize> {
        self.index.get(id).copied()
    }

    /// Diff against any older snapshot of the same session by `Arc` identity —
    /// O(entries) pointer compares, no content comparison.
    pub fn changes_since(&self, older: &SessionSnapshot) -> SnapshotDelta {
        if older.chat_id != self.chat_id || older.revision == 0 {
            return SnapshotDelta {
                reset: true,
                changed: self.entries.iter().map(|e| e.id.clone()).collect(),
                removed: Vec::new(),
            };
        }
        let changed = self
            .entries
            .iter()
            .filter(|entry| {
                older
                    .entry(&entry.id)
                    .is_none_or(|old| !Arc::ptr_eq(old, entry))
            })
            .map(|e| e.id.clone())
            .collect();
        let current: HashSet<&str> = self.entries.iter().map(|e| e.id.as_str()).collect();
        let removed = older
            .entries
            .iter()
            .filter(|e| !current.contains(e.id.as_str()))
            .map(|e| e.id.clone())
            .collect();
        SnapshotDelta {
            reset: false,
            changed,
            removed,
        }
    }
}

/// Who owns a queue row's delivery barrier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueueGate {
    /// Someone holds an edit lease; the row won't deliver until released.
    Editing {
        owner_device_id: String,
        expires_at_ms: i64,
        mine: bool,
    },
    /// An edit lease lapsed — a person must review before it can send.
    ReviewRequired { owner_device_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueItem {
    pub id: String,
    /// Raw row text (what the host will send).
    pub text: String,
    /// User-visible text (transport trailer + Appshot context stripped).
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingKind {
    Run,
    Steer,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingSend {
    pub message_id: String,
    pub text: String,
    pub kind: PendingKind,
    pub sent_at_ms: i64,
    pub state: SendState,
}

/// The unresolved question the composer should answer instead of typing.
#[derive(Debug, Clone, PartialEq)]
pub struct InputRequest {
    pub entry_id: String,
    pub request_id: String,
    pub questions: Vec<UserInputQuestion>,
}

/// Host-advertised features that gate composer affordances.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HostCapabilities {
    pub message_queue: bool,
    pub queue_actions: bool,
    pub queue_attachments: bool,
    pub clean_attachment_text: bool,
    pub queue_edit_lease: bool,
    /// Host ≥ 0.2.12: attachments ride `pending://` refs + escorted bytes.
    pub queued_attachments: bool,
    /// The chat's harness steers mid-turn (unknown until a live catalog).
    pub mid_turn_steering: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostInfo {
    pub device_id: String,
    pub name: Option<String>,
    pub online: bool,
    pub capabilities: HostCapabilities,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveStatus {
    pub indicator: ChatIndicator,
    pub working_since_ms: Option<i64>,
    /// The newest transcript entry is streaming.
    pub streaming: bool,
    /// Stop is meaningful (working, awaiting input, or streaming).
    pub can_interrupt: bool,
}

/// The chat room's transport state (live mode; demo is always connected).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RoomState {
    pub connected: bool,
    pub retry_at_ms: Option<i64>,
    /// Graced degradation for this chat (feeds Queued).
    pub degraded: bool,
}

/// Everything the composer and its status strip render. Transcript rows are
/// NOT here — they live in [`SessionSnapshot`].
#[derive(Debug, Clone, PartialEq)]
pub struct ComposerState {
    pub chat_id: String,
    pub revision: u64,
    pub title: String,
    pub host: HostInfo,
    pub live: LiveStatus,
    pub queue: Vec<QueueItem>,
    pub pending_sends: Vec<PendingSend>,
    /// Oldest pending send's state (the strip's headline), if any.
    pub send_state: Option<SendState>,
    /// A send now would queue rather than deliver promptly.
    pub delivery_degraded: bool,
    pub room: RoomState,
    /// Attachment escort progress (0..1) while bytes are moving.
    pub transfer_progress: Option<f64>,
    pub open_input: Option<InputRequest>,
    /// Last queue action failure (cleared on the next action).
    pub queue_error: Option<String>,
    /// The last message this device submitted (scroll-to-own-send target).
    pub last_submitted_message_id: Option<String>,
    pub context_usage: Option<ContextUsage>,
}
