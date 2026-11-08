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
    /// The (joined) doc message. Shared: cloning it is an `Arc` bump, and it
    /// stays pointer-equal across snapshots while the entry is unchanged —
    /// the layout engine's row-reuse key.
    pub message: Arc<SessionMessageEntry>,
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
    /// A turn is running (host status Working or a streaming tail) or one of
    /// this device's sends is on its way to a reachable host. A send parked
    /// for an offline host is NOT working (its echo shows Queued). Drives
    /// the transcript's tail "Working…" row.
    pub working: bool,
    /// Run start of the live turn, when the host reported one.
    pub working_since_ms: Option<i64>,
    /// This device's unadopted sends, oldest first. Their echo entries are
    /// `entries[transcript_len..]` (same order, same ids).
    pub pending: Vec<PendingSend>,
    pub context_usage: Option<ContextUsage>,
    /// Content is present (local snapshot loaded or first sync landed). False
    /// = show a loader, not an empty chat.
    pub hydrated: bool,
    pub delta: SnapshotDelta,
    pub(crate) index: Arc<HashMap<String, usize>>,
}

impl SessionSnapshot {
    /// Host-written entries (no echoes).
    pub fn transcript(&self) -> &[Arc<Entry>] {
        &self.entries[..self.transcript_len]
    }

    /// The host-written messages as shared `Arc`s — pointer-equal across
    /// snapshots while unchanged (O(entries) `Arc` bumps, no content copy).
    pub fn transcript_messages(&self) -> Vec<Arc<SessionMessageEntry>> {
        self.transcript()
            .iter()
            .map(|e| e.message.clone())
            .collect()
    }

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
    /// Client-minted id; the host's user entry reuses it (adoption).
    pub message_id: String,
    /// Raw prompt content exactly as the host will write it (attachment
    /// trailer included — row builders split it themselves).
    pub text: String,
    /// The user-visible part of `text` (trailer + Appshot context stripped).
    pub visible_text: String,
    /// Attachment refs/paths from the trailer (`pending://…` until adopted).
    pub images: Vec<String>,
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
    /// The host moves the chat to another provider between turns; older
    /// hosts would resume one provider's session under another.
    pub provider_switch: bool,
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
    /// The row's display status (an in-flight send reads Working).
    pub indicator: ChatIndicator,
    /// A turn is actually running on the host (host status Working or a
    /// streaming tail) — new sends queue/steer instead of starting a turn.
    pub turn_running: bool,
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

/// Why a transcript with nothing in it has nothing in it. Anything stored on this
/// device always shows, so this is only ever about a chat it has never loaded.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TranscriptLoad {
    /// Show the transcript: it has content, or the chat really is empty.
    #[default]
    Ready,
    /// The room or the host is still answering.
    Syncing,
    /// The room can't be reached, though the network looks fine.
    Reconnecting,
    /// No network: "Offline — this chat loads when you're back online".
    Offline,
    /// Its room stores nothing and its host is away: "This chat lives on another
    /// device — it loads when that device is back online". Nothing is lost.
    HostAway,
}

/// What [`transcript_load`] decides from.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct LoadInputs {
    pub has_content: bool,
    pub has_messages: bool,
    pub network: bool,
    pub generation: u32,
    /// The room has answered this device: stored rows and any checkpoint are in.
    pub room_answered: bool,
    pub host_online: bool,
    /// Long enough that "still syncing" would be an endless spinner.
    pub waited_too_long: bool,
}

pub(crate) fn transcript_load(i: LoadInputs) -> TranscriptLoad {
    // A retired legacy room has nothing to dial: what's here is all there is.
    if i.has_content || !i.has_messages || i.generation < 2 {
        return TranscriptLoad::Ready;
    }
    if !i.network {
        return TranscriptLoad::Offline;
    }
    if !i.room_answered {
        return if i.waited_too_long { TranscriptLoad::Reconnecting } else { TranscriptLoad::Syncing };
    }
    // A stored chat's room has told it everything.
    if i.generation < zeron_proto::relay_gen::RELAY_ROOM_GEN {
        return TranscriptLoad::Ready;
    }
    // A relayed chat's room stores nothing, so only its host can fill this copy.
    if i.host_online && !i.waited_too_long {
        TranscriptLoad::Syncing
    } else {
        TranscriptLoad::HostAway
    }
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
    /// What to show in place of an empty transcript, if anything.
    pub transcript_load: TranscriptLoad,
}

#[cfg(test)]
mod load_tests {
    use super::*;

    fn relayed() -> LoadInputs {
        LoadInputs {
            has_messages: true,
            network: true,
            generation: 3,
            room_answered: true,
            host_online: true,
            ..Default::default()
        }
    }

    #[test]
    fn what_is_stored_here_always_shows() {
        for has_content in [LoadInputs { has_content: true, network: false, ..relayed() },
            LoadInputs { has_content: true, host_online: false, waited_too_long: true, ..relayed() }] {
            assert_eq!(transcript_load(has_content), TranscriptLoad::Ready);
        }
    }

    #[test]
    fn a_chat_with_no_messages_is_really_empty() {
        let empty = LoadInputs { has_messages: false, host_online: false, ..relayed() };
        assert_eq!(transcript_load(empty), TranscriptLoad::Ready);
    }

    #[test]
    fn no_network_says_offline_before_anything_else() {
        let i = LoadInputs { network: false, room_answered: false, ..relayed() };
        assert_eq!(transcript_load(i), TranscriptLoad::Offline);
        let stored = LoadInputs { network: false, generation: 2, room_answered: false, ..relayed() };
        assert_eq!(transcript_load(stored), TranscriptLoad::Offline);
    }

    #[test]
    fn a_relayed_chat_waits_on_its_host_and_never_forever() {
        assert_eq!(transcript_load(relayed()), TranscriptLoad::Syncing);
        let away = LoadInputs { host_online: false, ..relayed() };
        assert_eq!(transcript_load(away), TranscriptLoad::HostAway);
        // Online by presence but not answering is the same to someone waiting.
        let silent = LoadInputs { waited_too_long: true, ..relayed() };
        assert_eq!(transcript_load(silent), TranscriptLoad::HostAway);
    }

    #[test]
    fn a_stored_chat_believes_its_room() {
        let stored = LoadInputs { generation: 2, host_online: false, ..relayed() };
        assert_eq!(transcript_load(stored), TranscriptLoad::Ready);
    }

    #[test]
    fn a_room_that_doesnt_answer_stops_spinning() {
        let i = LoadInputs { room_answered: false, ..relayed() };
        assert_eq!(transcript_load(i), TranscriptLoad::Syncing);
        let i = LoadInputs { waited_too_long: true, ..i };
        assert_eq!(transcript_load(i), TranscriptLoad::Reconnecting);
    }

    #[test]
    fn a_retired_room_has_nothing_to_wait_for() {
        let i = LoadInputs { generation: 1, network: false, room_answered: false, ..relayed() };
        assert_eq!(transcript_load(i), TranscriptLoad::Ready);
    }
}
