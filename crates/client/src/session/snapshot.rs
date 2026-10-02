//! Immutable session views: the transcript ([`SessionSnapshot`]) consumed in
//! Rust by the layout engine, and the composer-facing [`ComposerState`]
//! exported to the platform.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use zeron_doc::{MessagePart, MessageRole, MessageStatus, SessionMessageEntry};
use zeron_proto::{
    ChatIndicator, ContextUsage, Goal, TodoItem, ToolCall, UserInputQuestion, WorkflowRunsState,
};

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
    /// The chat's goal (`meta.goal`, written by its host), when it has one.
    pub goal: Option<Arc<Goal>>,
    /// The chat's workflow runs (`meta.workflowRuns`), oldest first.
    pub workflows: Arc<WorkflowRunsState>,
    /// The checklist the agent most recently wrote (`None` when it never
    /// wrote one, or the last write cleared it).
    pub todo: Option<Arc<Vec<TodoItem>>>,
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
    /// The host runs goal mode (`goal-mode-v1`): it executes `goal` commands.
    pub goal_mode: bool,
    /// The host runs dynamic workflows (`workflows-v1`).
    pub workflows: bool,
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

/// The checklist the agent wrote last. Every harness that plans writes the
/// whole list on each update, so only the newest `Todo` part counts (ACP plans
/// reuse one tool id: "latest part", not "first part with this id"). An empty
/// write clears it. Walks back from the newest entry and stops at the first
/// hit; a chat that never planned scans its parts once per change.
pub fn latest_todo(entries: &[Arc<Entry>]) -> Option<Arc<Vec<TodoItem>>> {
    let items = entries
        .iter()
        .rev()
        .filter(|entry| entry.role() == MessageRole::Assistant)
        .flat_map(|entry| entry.parts().iter().rev())
        .find_map(|part| match part {
            MessagePart::Tool {
                call: ToolCall::Todo { items },
                ..
            } => Some(items),
            _ => None,
        })?;
    (!items.is_empty()).then(|| Arc::new(items.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeron_doc::SessionMessageEntry;

    fn items(spec: &str) -> Vec<TodoItem> {
        // x = completed, > = in progress, . = pending
        spec.chars()
            .enumerate()
            .map(|(i, c)| {
                TodoItem::new(
                    format!("item {i}"),
                    match c {
                        'x' => zeron_proto::TodoStatus::Completed,
                        '>' => zeron_proto::TodoStatus::InProgress,
                        _ => zeron_proto::TodoStatus::Pending,
                    },
                )
            })
            .collect()
    }

    fn todo_entry(id: &str, role: MessageRole, lists: &[(&str, Vec<TodoItem>)]) -> Arc<Entry> {
        let parts = lists
            .iter()
            .map(|(part, list)| MessagePart::Tool {
                id: (*part).into(),
                call: ToolCall::Todo { items: list.clone() },
                is_error: false,
                resolved: true,
                output: None,
                diff: None,
                output_ref: None,
                output_bytes: None,
                diff_ref: None,
                diff_stats: None,
                subagent_ref: None,
                subagent_status: None,
                subagent_tail: None,
            })
            .collect();
        Arc::new(Entry {
            id: id.into(),
            rev: 1,
            message: Arc::new(SessionMessageEntry {
                origin: None,
                id: id.into(),
                role,
                parts,
                created_at: 0,
                device_id: "d".into(),
                status: None,
                continuation_of: None,
                duration_ms: None,
            }),
            echo: None,
            append: None,
        })
    }

    #[test]
    fn the_newest_list_wins_across_and_within_entries() {
        let first = todo_entry("a", MessageRole::Assistant, &[("p", items("x.."))]);
        let second = todo_entry(
            "b",
            MessageRole::Assistant,
            &[("p1", items("xx.")), ("p2", items("xx>"))],
        );
        let latest = latest_todo(&[first, second]).unwrap();
        assert_eq!(*latest, items("xx>"));
        // An ACP plan reuses one tool id; the newer segment still wins.
        let old = todo_entry("c", MessageRole::Assistant, &[("plan", items(">.."))]);
        let new = todo_entry("d", MessageRole::Assistant, &[("plan", items("x>."))]);
        assert_eq!(*latest_todo(&[old, new]).unwrap(), items("x>."));
    }

    #[test]
    fn an_empty_write_clears_and_user_entries_are_ignored() {
        let list = todo_entry("a", MessageRole::Assistant, &[("p", items("x."))]);
        let cleared = todo_entry("b", MessageRole::Assistant, &[("q", Vec::new())]);
        assert!(latest_todo(&[list.clone(), cleared]).is_none());
        let user = todo_entry("u", MessageRole::User, &[("p", items("."))]);
        assert!(latest_todo(&[user]).is_none());
        assert!(latest_todo(&[]).is_none());
        assert_eq!(*latest_todo(&[list]).unwrap(), items("x."));
    }

    #[test]
    fn a_reply_becomes_a_page_and_binary_has_no_text() {
        let page = super::super::ArtifactPage::from_reply(&serde_json::json!({
            "version": {"version": 2, "contentType": "text/markdown", "title": "Doc"},
            "offset": 0, "total": 5, "encoding": "utf8", "data": "# Doc",
        }))
        .unwrap();
        assert_eq!((page.version, page.total), (2, 5));
        assert_eq!(page.text.as_deref(), Some("# Doc"));
        let bin = super::super::ArtifactPage::from_reply(&serde_json::json!({
            "version": {"version": 1, "contentType": "application/octet-stream"},
            "total": 9, "encoding": "base64", "data": "AAAA",
        }))
        .unwrap();
        assert!(bin.text.is_none());
        assert!(super::super::ArtifactPage::from_reply(&serde_json::json!({})).is_none());
    }
}
