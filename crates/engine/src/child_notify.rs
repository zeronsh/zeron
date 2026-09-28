//! Durable parent notifications: when an agent-spawned child chat
//! settles — completes, errors, is interrupted, or parks on a question — the
//! engine hosting its PARENT writes a `role: System` ChildUpdate card into
//! the parent's doc and delivers one combined agent-only prompt, exactly
//! once, without ever interrupting the parent's turn.
//!
//! Exactly-once is built in two layers, both keyed on the stable turn key
//! from [`zeron_proto::orchestration::child_update`]:
//!
//! 1. the `child_notifications` ledger in [`DocsStore`] — claim-before-send
//!    (`INSERT OR IGNORE`), so a restart between detection and delivery
//!    re-arms from `pending` rows instead of double-sending, and an
//!    `AckChildUpdates` row that beat the claim suppresses it entirely;
//! 2. the card's fixed entry id `child:<childId>:<turnKey>` — the doc append
//!    is check-before-write, so a crash between card write and `delivered`
//!    stamp can never duplicate the visible card either.
//!
//! The notifier watches the MERGED session view ([`WorkspaceHost::
//! merged_sessions_watch`]): registry session rows of every device overlaid
//! with this engine's in-memory truth. Children can live anywhere — the
//! registry rows are the cross-device signal — while a local child still
//! reports its settle before its registry row round-trips. Chat rows come
//! from `watch_chats`, which is also what wakes the loop when a parent is
//! archived, unarchived, or re-homed here.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use tokio::time::Instant;
use zeron_doc::{MessagePart, MessageRole, MessageStatus, SessionMessageEntry};
use zeron_proto::entities::{Chat, Session, SessionStatus};
use zeron_proto::orchestration::{ChildOutcome, ChildUpdate, child_update};
use zeron_sync::DocsStore;

use crate::doc_host::DocHost;
use crate::sessions::SessionsEngine;
use crate::workspace_host::WorkspaceHost;

/// Post-claim quiet window: settles that land within it coalesce into the
/// ONE prompt a flush delivers (two children finishing a second apart read
/// as a single "Child chat updates" message).
const FLUSH_DEBOUNCE: Duration = Duration::from_secs(2);

/// Delivery failure backoff: 1 s, 5 s, 30 s, then every 5 minutes — a dead
/// doc-host path must never loop hot.
const RETRY_BACKOFF: [Duration; 3] = [
    Duration::from_secs(1),
    Duration::from_secs(5),
    Duration::from_secs(30),
];
const RETRY_CAP: Duration = Duration::from_secs(300);

/// Belt re-check cadences for parked flushes: the watch wake is primary
/// (the row the flush waits on changes), the timer only covers a missed or
/// never-emitted event. Both fire solely while notifications are pending.
const BUSY_RECHECK: Duration = Duration::from_secs(30);
const NOT_LOCAL_RECHECK: Duration = Duration::from_secs(60);

/// The prompt budget for a completed child's last reply.
const EXCERPT_PROMPT_LIMIT: usize = 4000;
/// The card's excerpt budget (the part, not the prompt).
const EXCERPT_CARD_LIMIT: usize = 600;
/// Needs-help detail block: up to 4 lines and 700 chars.
const BLOCKER_MAX_LINES: usize = 4;
const BLOCKER_MAX_CHARS: usize = 700;
const TRUNCATED_SUFFIX: &str = "\n\n[... output truncated ...]";

/// Spawn the notifier loop on the doc host's worker tracker (engine
/// assembly). All state lives inside the task: `last_seen` session rows for
/// edge detection, `due` per-parent flush times, `attempts` per-parent
/// delivery backoff, and `awaiting` — updates detected before the child's
/// (or parent's) chat row was visible, re-evaluated on every wake so a
/// registry-row ordering race can't lose a notification.
pub(crate) fn start(
    doc_host: &DocHost,
    workspace: &WorkspaceHost,
    sessions: &SessionsEngine,
    store: Arc<DocsStore>,
    device_id: String,
) {
    // Bare sync contexts (unit-test assembly without a runtime) get no
    // notifier rather than a spawn panic — same guard as DocHost::new.
    if tokio::runtime::Handle::try_current().is_err() {
        return;
    }
    let host = doc_host.clone();
    let workspace = workspace.clone();
    let sessions = sessions.clone();
    doc_host.spawn_worker(async move {
        let doc_host = host;
        let mut sessions_rx = workspace.merged_sessions_watch(sessions.watch_sessions());
        let mut chats_rx = workspace.watch_chats();
        let mut state = NotifyState::default();
        let mut attempts: HashMap<String, u32> = HashMap::new();
        // Restart re-arm: rows still pending from before this boot flush on
        // the same debounce as a fresh claim.
        for parent in store
            .pending_child_notification_parents()
            .unwrap_or_default()
        {
            state.due.insert(parent, Instant::now() + FLUSH_DEBOUNCE);
        }
        loop {
            // Read guards live only for the sync `detect` pass — holding a
            // watch borrow across `flush_due`'s awaits would stall publishers.
            {
                let session_rows = sessions_rx.borrow_and_update();
                let chat_rows = chats_rx.borrow_and_update();
                detect(&session_rows, &chat_rows, &store, &device_id, &mut state);
            }
            flush_due(
                &FlushCtx {
                    doc_host: &doc_host,
                    workspace: &workspace,
                    sessions: &sessions,
                    store: &store,
                    device_id: &device_id,
                    sessions_rx: &sessions_rx,
                },
                &mut state,
                &mut attempts,
            )
            .await;
            let next = state.due.values().min().copied();
            let wait = async move {
                match next {
                    Some(at) => tokio::time::sleep_until(at).await,
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                changed = sessions_rx.changed() => if changed.is_err() { break },
                changed = chats_rx.changed() => if changed.is_err() { break },
                _ = wait => {}
            }
        }
    });
}

/// The fields `child_update` reads from the PREVIOUS row, kept as a
/// fingerprint: an unchanged session costs a struct compare — no clone, no
/// allocation — and classification (`child_update`) runs only for rows that
/// actually changed.
#[derive(Clone, PartialEq)]
struct Seen {
    status: SessionStatus,
    last_completed_turn: Option<String>,
    started_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl Seen {
    fn of(session: &Session) -> Self {
        Self {
            status: session.status,
            last_completed_turn: session.last_completed_turn.clone(),
            started_at: session.started_at,
        }
    }

    /// Field-wise compare without building a fingerprint — unchanged rows
    /// (the common case) cost zero allocations.
    fn matches(&self, session: &Session) -> bool {
        self.status == session.status
            && self.last_completed_turn == session.last_completed_turn
            && self.started_at == session.started_at
    }

    /// `child_update` reads only status/marker/started_at from `prev`, so the
    /// row can be rebuilt losslessly for the diff call. Runs only on rows
    /// that already changed.
    fn as_session(&self, chat_id: &str) -> Session {
        Session {
            last_completed_turn: self.last_completed_turn.clone(),
            chat_id: chat_id.to_string(),
            device_id: String::new(),
            status: self.status,
            started_at: self.started_at,
            updated_at: chrono::Utc::now(),
        }
    }
}

/// The fields the notifier reads from each chat row, kept as a fingerprint:
/// unchanged rows cost a compare; eligibility and deferral re-checks run
/// only for rows that actually changed.
#[derive(PartialEq)]
struct ChatSeen {
    spawned_by_agent: bool,
    parent_chat_id: Option<String>,
    device_id: String,
    /// Not consulted for eligibility — tracked so archiving a deferred
    /// parent still wakes the flush to drop its pending rows.
    archived: bool,
}

impl ChatSeen {
    fn of(chat: &Chat) -> Self {
        Self {
            spawned_by_agent: chat.spawned_by_agent,
            parent_chat_id: chat.parent_chat_id.clone(),
            device_id: chat.device_id.clone(),
            archived: chat.archived,
        }
    }

    fn matches(&self, chat: &Chat) -> bool {
        self.spawned_by_agent == chat.spawned_by_agent
            && self.parent_chat_id == chat.parent_chat_id
            && self.device_id == chat.device_id
            && self.archived == chat.archived
    }
}

/// Why a parent's flush is parked. Re-armed only by a change to the row the
/// flush was waiting on — no timers tick for deferred parents.
enum Defer {
    /// Parent turn in flight; wake when its session row changes.
    InFlight,
    /// Parent chat row missing or hosted elsewhere; wake when it changes.
    ParentRow,
}

/// Per-wake fingerprints and parked work. Everything here is O(known rows)
/// memory and O(changed rows) work.
#[derive(Default)]
struct NotifyState {
    sessions: HashMap<String, Seen>,
    chats: HashMap<String, ChatSeen>,
    /// Settle updates that fired before the child's chat row was visible.
    awaiting: HashMap<String, ChildUpdate>,
    /// Parents whose flush is parked on a specific row change.
    deferred: HashMap<String, Defer>,
    /// Parents with a scheduled flush (debounce window or retry backoff).
    due: HashMap<String, Instant>,
    /// Ledger write passes executed — a heartbeat storm must leave this at
    /// the number of real settle transitions, not the number of wakes.
    claim_passes: u32,
}

impl NotifyState {
    fn arm(&mut self, parent: &str, at: Instant) {
        self.deferred.remove(parent);
        self.due.insert(parent.to_string(), at);
    }
}

/// Diff the merged session/chat rows against the fingerprint maps; claim
/// every detected settle for eligible children and schedule the parent's
/// flush. Per event: O(rows) cheap fingerprint compares, transition
/// classification and eligibility checks only for rows that actually
/// changed, then ONE batched ledger write.
fn detect(
    sessions: &[Session],
    chats: &[Chat],
    store: &DocsStore,
    device_id: &str,
    state: &mut NotifyState,
) {
    // Chat-row fingerprint diff first: awaiting children and deferred
    // parents re-check only when the row they wait on changed.
    let mut changed_chats: HashSet<&str> = HashSet::new();
    {
        let present: HashSet<&str> = chats.iter().map(|c| c.id.as_str()).collect();
        state.chats.retain(|id, _| present.contains(id.as_str()));
        for chat in chats {
            if state.chats.get(&chat.id).is_some_and(|s| s.matches(chat)) {
                continue;
            }
            changed_chats.insert(chat.id.as_str());
            state.chats.insert(chat.id.clone(), ChatSeen::of(chat));
        }
    }

    let eligible = |state: &NotifyState, child_id: &str| -> Option<String> {
        let child = state.chats.get(child_id)?;
        if !child.spawned_by_agent {
            return None;
        }
        let parent_id = child.parent_chat_id.as_deref()?;
        let parent = state.chats.get(parent_id)?;
        (parent.device_id == device_id).then(|| parent_id.to_string())
    };

    let mut wake: Vec<String> = Vec::new();
    for (parent, why) in &state.deferred {
        match why {
            Defer::ParentRow if changed_chats.contains(parent.as_str()) => {
                wake.push(parent.clone());
            }
            _ => {}
        }
    }
    for parent in wake {
        state.deferred.remove(&parent);
        state.due.insert(parent, Instant::now());
    }

    // (child, update, parent) triples to claim in one transaction.
    let mut fresh: Vec<(String, ChildUpdate, String)> = Vec::new();

    // Retry updates parked on children whose chat row just landed.
    if !state.awaiting.is_empty() {
        let parked: Vec<(String, ChildUpdate)> = state
            .awaiting
            .iter()
            .filter(|(id, _)| changed_chats.contains(id.as_str()))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        for (child_id, update) in parked {
            match eligible(state, &child_id) {
                Some(parent_id) => {
                    state.awaiting.remove(&child_id);
                    fresh.push((child_id, update, parent_id));
                }
                // Once the child exists and is plainly not ours to report
                // (non-agent child, remote parent), stop re-checking it.
                None if state.chats.contains_key(&child_id) => {
                    state.awaiting.remove(&child_id);
                }
                None => {}
            }
        }
    }

    let mut changed_sessions: HashSet<&str> = HashSet::new();
    {
        let present: HashSet<&str> = sessions.iter().map(|s| s.chat_id.as_str()).collect();
        state.sessions.retain(|id, _| present.contains(id.as_str()));
        for next in sessions {
            if state
                .sessions
                .get(&next.chat_id)
                .is_some_and(|s| s.matches(next))
            {
                continue; // unchanged row — nothing could transition
            }
            changed_sessions.insert(next.chat_id.as_str());
            let fingerprint = Seen::of(next);
            let prev = state
                .sessions
                .insert(next.chat_id.clone(), fingerprint)
                .map(|s| s.as_session(&next.chat_id));
            let Some(update) = child_update(prev.as_ref(), next) else {
                continue;
            };
            match eligible(state, &next.chat_id) {
                Some(parent_id) => fresh.push((next.chat_id.clone(), update, parent_id)),
                // Park it when the chat row isn't here yet: spawn and settle
                // routinely race the registry row landing.
                None if !state.chats.contains_key(&next.chat_id) => {
                    state.awaiting.insert(next.chat_id.clone(), update);
                }
                None => {}
            }
        }
    }

    // Parents parked on a live turn wake when its session row changed — or
    // vanished entirely (a removed row means the turn is over either way).
    let inflight_wake: Vec<String> = state
        .deferred
        .iter()
        .filter(|(parent, why)| {
            matches!(why, Defer::InFlight)
                && (changed_sessions.contains(parent.as_str())
                    || !sessions.iter().any(|s| s.chat_id == **parent))
        })
        .map(|(parent, _)| parent.clone())
        .collect();
    for parent in inflight_wake {
        state.deferred.remove(&parent);
        state.due.insert(parent, Instant::now());
    }

    if fresh.is_empty() {
        return;
    }
    // Claim-before-send, one transaction for the whole pass: the ledger
    // decides whether each transition still owes the parent a notification
    // (prior pending claim, delivered, or an ack that beat us all read as
    // "not owed"), and each fresh claim supersedes the child's older pending
    // keys — only the newest settle per child is delivered.
    let specs: Vec<zeron_sync::ChildNotificationClaim<'_>> = fresh
        .iter()
        .map(
            |(child_id, update, parent_id)| zeron_sync::ChildNotificationClaim {
                child_chat_id: child_id,
                turn_key: &update.key,
                parent_chat_id: parent_id,
                outcome: outcome_name(update.outcome),
            },
        )
        .collect();
    state.claim_passes += 1;
    match store.claim_child_notifications(&specs) {
        Ok(claimed) => {
            for (claimed, (_, _, parent_id)) in claimed.iter().zip(fresh.iter()) {
                if *claimed {
                    state.arm(parent_id, Instant::now() + FLUSH_DEBOUNCE);
                }
            }
        }
        Err(err) => {
            tracing::warn!(error = %err, "child-notification claim failed");
        }
    }
}

/// The shared context a flush needs — bundled so `flush_due` stays readable.
struct FlushCtx<'a> {
    doc_host: &'a DocHost,
    workspace: &'a WorkspaceHost,
    sessions: &'a SessionsEngine,
    store: &'a DocsStore,
    device_id: &'a str,
    sessions_rx: &'a tokio::sync::watch::Receiver<Vec<Session>>,
}

/// Deliver every parent whose flush time arrived. Holds: parent missing or
/// hosted elsewhere (parked until its chat row changes), archived (pending
/// rows drop — sending would unarchive it), or mid-turn (parked until its
/// session row changes; the merged watch wakes us when it does).
async fn flush_due(
    ctx: &FlushCtx<'_>,
    state: &mut NotifyState,
    attempts: &mut HashMap<String, u32>,
) {
    let doc_host = ctx.doc_host;
    let workspace = ctx.workspace;
    let sessions = ctx.sessions;
    let store = ctx.store;
    let device_id = ctx.device_id;
    let now = Instant::now();
    let ready: Vec<String> = state
        .due
        .iter()
        .filter(|(_, at)| **at <= now)
        .map(|(p, _)| p.clone())
        .collect();
    for parent_id in ready {
        state.due.remove(&parent_id);
        let parent = match workspace.chat(&parent_id) {
            Ok(parent) => parent,
            Err(err) => {
                tracing::warn!(chat = %parent_id, error = %err, "child-notification parent read failed");
                // Transient store error: timed retry while work is pending.
                state.due.insert(parent_id, Instant::now() + RETRY_CAP);
                continue;
            }
        };
        let Some(parent) = parent else {
            // Row not visible yet; the chats watch wakes us when it lands,
            // the belt re-checks in case no event ever comes.
            state.deferred.insert(parent_id.clone(), Defer::ParentRow);
            state
                .due
                .insert(parent_id, Instant::now() + NOT_LOCAL_RECHECK);
            continue;
        };
        if parent.device_id != device_id {
            // Not ours to notify; rows stay pending for whoever hosts it.
            // If the chat migrates here its row changes and the watch wakes.
            state.deferred.insert(parent_id.clone(), Defer::ParentRow);
            state
                .due
                .insert(parent_id, Instant::now() + NOT_LOCAL_RECHECK);
            continue;
        }
        if parent.archived {
            if let Err(err) = store.drop_pending_child_notifications(&parent_id) {
                tracing::warn!(chat = %parent_id, error = %err, "child-notification drop failed");
                state.due.insert(parent_id, Instant::now() + RETRY_CAP);
            }
            continue;
        }
        // AwaitingInput is still a turn — the parent is owed its answer, not
        // a steer. Idle/Errored/no session are deliverable. Borrow the watch
        // fresh at flush time: the snapshot read at wake is already stale.
        let in_flight = sessions.turn_in_flight(&parent_id)
            || ctx
                .sessions_rx
                .borrow()
                .iter()
                .find(|s| s.chat_id == parent_id)
                .is_some_and(|s| {
                    matches!(
                        s.status,
                        SessionStatus::Working | SessionStatus::AwaitingInput
                    )
                });
        if in_flight {
            // Parked on the parent's turn: its session row change wakes us
            // immediately, and the 30s belt caps the cost at one re-check
            // per interval while it stays busy.
            state.deferred.insert(parent_id.clone(), Defer::InFlight);
            state.due.insert(parent_id, Instant::now() + BUSY_RECHECK);
            continue;
        }
        let pending = match store.pending_child_notifications(&parent_id) {
            Ok(pending) => pending,
            Err(err) => {
                tracing::warn!(chat = %parent_id, error = %err, "child-notification read failed");
                state.due.insert(parent_id, Instant::now() + RETRY_CAP);
                continue;
            }
        };
        if pending.is_empty() {
            attempts.remove(&parent_id);
            continue;
        }
        let mut cards = Vec::with_capacity(pending.len());
        let mut lines = Vec::with_capacity(pending.len());
        for row in &pending {
            cards.push(card_entry(doc_host, workspace, device_id, row));
            lines.push(prompt_line(doc_host, workspace, row));
        }
        let prompt = combined_prompt(&lines);
        match doc_host
            .deliver_child_updates(&parent_id, &cards, &prompt)
            .await
        {
            Ok(()) => {
                attempts.remove(&parent_id);
                // One transaction marks every delivered row the prompt
                // carried; an ack that raced the flush wins (it already left
                // 'pending').
                let keys: Vec<(&str, &str)> = pending
                    .iter()
                    .map(|row| (row.child_chat_id.as_str(), row.turn_key.as_str()))
                    .collect();
                if let Err(err) = store.settle_child_notifications(&keys, "delivered") {
                    tracing::warn!(chat = %parent_id, error = %err, "child-notification settle failed");
                }
            }
            Err(err) => {
                let attempt = attempts.entry(parent_id.clone()).or_insert(0);
                let wait = RETRY_BACKOFF
                    .get(*attempt as usize)
                    .copied()
                    .unwrap_or(RETRY_CAP);
                *attempt += 1;
                tracing::warn!(chat = %parent_id, error = %err,
                    backoff_ms = wait.as_millis() as u64,
                    "child-notification delivery failed; retrying");
                state.due.insert(parent_id, Instant::now() + wait);
            }
        }
    }
}

fn outcome_name(outcome: ChildOutcome) -> &'static str {
    match outcome {
        ChildOutcome::Completed => "completed",
        ChildOutcome::Errored => "errored",
        ChildOutcome::Interrupted => "interrupted",
        ChildOutcome::NeedsInput => "needsInput",
    }
}

fn outcome_from_name(name: &str) -> ChildOutcome {
    match name {
        "errored" => ChildOutcome::Errored,
        "interrupted" => ChildOutcome::Interrupted,
        "needsInput" => ChildOutcome::NeedsInput,
        _ => ChildOutcome::Completed,
    }
}

fn chat_title(chat: Option<&Chat>, chat_id: &str) -> String {
    chat.and_then(|c| c.title.clone())
        .filter(|t| !t.trim().is_empty())
        .unwrap_or_else(|| {
            // Untitled chats label by their id prefix, like agent_message.
            chat_id.chars().take(8).collect()
        })
}

/// Truncate at a char boundary to at most `max` chars; adds the truncation
/// suffix when anything was cut.
fn cap_chars(text: &str, max: usize) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= max {
        return trimmed.to_string();
    }
    let cut = trimmed
        .char_indices()
        .nth(max)
        .map_or(trimmed.len(), |(i, _)| i);
    format!("{}{}", trimmed[..cut].trim_end(), TRUNCATED_SUFFIX)
}

/// Bounded tail window for excerpt reads — a flush reads only the end of
/// the child's doc, never the whole transcript.
const EXCERPT_TAIL_PARTS: usize = 256;

/// The child's last assistant reply: text parts of the latest settled
/// (Complete, non-empty) assistant entry in the child's local doc.
fn last_assistant_reply(doc_host: &DocHost, child_chat_id: &str) -> Option<String> {
    let handle = doc_host.open_local(child_chat_id).ok()?;
    let entries = handle.doc().read_opening_tail(EXCERPT_TAIL_PARTS).ok()?;
    let entry = entries.iter().rev().find(|e| {
        e.role == MessageRole::Assistant
            && e.status == Some(MessageStatus::Complete)
            && e.parts.iter().any(|p| match p {
                MessagePart::Text { text, .. } => !text.trim().is_empty(),
                _ => false,
            })
    })?;
    let text = entry
        .parts
        .iter()
        .filter_map(|p| match p {
            MessagePart::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    (!text.trim().is_empty()).then(|| text.trim().to_string())
}

/// The child's still-open input request: labels + detail lines for the
/// "needs help" prompt, read from the child's local doc.
fn pending_input_details(doc_host: &DocHost, child_chat_id: &str) -> Option<(String, String)> {
    let handle = doc_host.open_local(child_chat_id).ok()?;
    let entries = handle.doc().read_opening_tail(EXCERPT_TAIL_PARTS).ok()?;
    let questions = entries.iter().rev().find_map(|e| {
        e.parts.iter().find_map(|p| match p {
            MessagePart::Input {
                questions,
                resolved: false,
                ..
            } => Some(questions.clone()),
            _ => None,
        })
    })?;
    let first = questions.first()?;
    let label = if !first.header.trim().is_empty() {
        first.header.trim().to_string()
    } else {
        first.question.trim().to_string()
    };
    let mut lines: Vec<String> = Vec::new();
    for q in &questions {
        if !q.question.trim().is_empty() {
            lines.push(q.question.trim().to_string());
        }
        if !q.options.is_empty() {
            lines.push(format!("options: {}", q.options.join(", ")));
        }
        if lines.len() >= BLOCKER_MAX_LINES {
            break;
        }
    }
    lines.truncate(BLOCKER_MAX_LINES);
    Some((label, cap_chars(&lines.join("\n"), BLOCKER_MAX_CHARS)))
}

/// The excerpt stored on the card AND expanded in the prompt body for a
/// completed child: the doc's last reply when the doc is readable here, else
/// the registry preview with a pointer at `zeron chat output`.
fn completion_excerpt(
    doc_host: &DocHost,
    workspace: &WorkspaceHost,
    child_chat_id: &str,
) -> Option<String> {
    if let Some(reply) = last_assistant_reply(doc_host, child_chat_id) {
        return Some(cap_chars(&reply, EXCERPT_PROMPT_LIMIT));
    }
    let preview = workspace
        .chat(child_chat_id)
        .ok()
        .flatten()
        .and_then(|c| c.last_message_preview)
        .filter(|p| !p.trim().is_empty());
    preview.map(|preview| {
        let id8: String = child_chat_id.chars().take(8).collect();
        format!(
            "{}\n\nRead it with `zeron chat output {}`.",
            preview.trim(),
            id8
        )
    })
}

/// What the ChildUpdate card carries: the prompt's own excerpt, capped for
/// display — the completed child's reply, or the blocker a needs-help child
/// parked on. Failed/interrupted updates have no excerpt.
fn card_excerpt(
    doc_host: &DocHost,
    workspace: &WorkspaceHost,
    row: &zeron_sync::ChildNotification,
) -> Option<String> {
    match outcome_from_name(row.outcome.as_deref().unwrap_or("")) {
        ChildOutcome::Completed => completion_excerpt(doc_host, workspace, &row.child_chat_id)
            .map(|e| cap_chars(&e, EXCERPT_CARD_LIMIT)),
        ChildOutcome::NeedsInput => {
            pending_input_details(doc_host, &row.child_chat_id).map(|(label, detail)| {
                cap_chars(
                    &format!("Blocked on {label}:\n{detail}"),
                    EXCERPT_CARD_LIMIT,
                )
            })
        }
        ChildOutcome::Errored | ChildOutcome::Interrupted => None,
    }
}

/// The `role: System` entry for one delivered update — fixed id so the
/// append is idempotent across delivery retries.
fn card_entry(
    doc_host: &DocHost,
    workspace: &WorkspaceHost,
    device_id: &str,
    row: &zeron_sync::ChildNotification,
) -> SessionMessageEntry {
    let child = workspace.chat(&row.child_chat_id).ok().flatten();
    SessionMessageEntry {
        duration_ms: None,
        id: format!("child:{}:{}", row.child_chat_id, row.turn_key),
        role: MessageRole::System,
        parts: vec![MessagePart::ChildUpdate {
            id: "cu0".into(),
            child_chat_id: row.child_chat_id.clone(),
            child_title: chat_title(child.as_ref(), &row.child_chat_id),
            outcome: outcome_from_name(row.outcome.as_deref().unwrap_or("")),
            excerpt: card_excerpt(doc_host, workspace, row),
        }],
        created_at: crate::now_ms(),
        device_id: device_id.to_string(),
        status: Some(MessageStatus::Complete),
        continuation_of: None,
    }
}

enum PromptLine {
    Completed(String, Option<String>),
    Failed(String),
    Interrupted(String),
    NeedsHelp(String, Option<(String, String)>),
}

/// Build the per-update prompt content (kept as an enum so the single-update
/// and batch templates share the same classification).
fn prompt_line(
    doc_host: &DocHost,
    workspace: &WorkspaceHost,
    row: &zeron_sync::ChildNotification,
) -> PromptLine {
    let mention = format!("@chat:{}", row.child_chat_id);
    match outcome_from_name(row.outcome.as_deref().unwrap_or("")) {
        ChildOutcome::Completed => PromptLine::Completed(
            mention,
            completion_excerpt(doc_host, workspace, &row.child_chat_id),
        ),
        ChildOutcome::Errored => PromptLine::Failed(mention),
        ChildOutcome::Interrupted => PromptLine::Interrupted(mention),
        ChildOutcome::NeedsInput => {
            PromptLine::NeedsHelp(mention, pending_input_details(doc_host, &row.child_chat_id))
        }
    }
}

/// The full agent-only prompt: `[Zeron system]` + the D9 template. A single
/// update expands inline; several collapse to the batch bullet list.
fn combined_prompt(lines: &[PromptLine]) -> String {
    let body = match lines {
        [PromptLine::Completed(mention, excerpt)] => format!(
            "{mention} completed:\n\n{}",
            excerpt
                .as_deref()
                .unwrap_or("No final output was recorded.")
        ),
        [PromptLine::Failed(mention)] => {
            format!("{mention} failed.\n\nReview the chat before deciding next steps.")
        }
        [PromptLine::Interrupted(mention)] => format!(
            "{mention} was interrupted.\n\nReview the chat before deciding next steps.\n\nIf the user stopped it manually, do not resume, restart, retry, replace, or continue the work unless the user explicitly asks."
        ),
        [PromptLine::NeedsHelp(mention, details)] => {
            let blocked = details
                .as_ref()
                .map(|(label, detail)| format!("Blocked on {label}:\n{detail}"))
                .unwrap_or_else(|| "Blocked on input.".to_string());
            format!(
                "{mention} needs help.\n{blocked}\n\nReview the blocker. If you can resolve it from existing context, reply to the chat with guidance. Otherwise, ask the user for the missing decision."
            )
        }
        _ => {
            let mut bullets = Vec::with_capacity(lines.len());
            let mut any_interrupted = false;
            for line in lines {
                bullets.push(match line {
                    PromptLine::Completed(m, _) => format!("- {m} completed."),
                    PromptLine::Failed(m) => format!("- {m} failed."),
                    PromptLine::Interrupted(m) => {
                        any_interrupted = true;
                        format!("- {m} was interrupted.")
                    }
                    PromptLine::NeedsHelp(m, _) => format!("- {m} needs help."),
                });
            }
            let mut body = format!("Child chat updates:\n\n{}", bullets.join("\n"));
            if any_interrupted {
                body.push_str("\n\nIf the user stopped a chat manually, do not resume, restart, retry, replace, or continue its work unless the user explicitly asks.");
            }
            body
        }
    };
    format!("[Zeron system]\n\n{body}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chat(id: &str, device_id: &str) -> Chat {
        Chat {
            id: id.to_string(),
            device_id: device_id.to_string(),
            title: None,
            archived: false,
            cwd: None,
            branch: None,
            checkout_id: None,
            source_context: None,
            config: None,
            last_message_preview: None,
            last_message_at: None,
            created_at: chrono::Utc::now(),
            harness_session_id: None,
            harness_session_cwd: None,
            space_id: None,
            last_seen_at: None,
            room_gen: None,
            parent_chat_id: None,
            spawned_by_agent: false,
        }
    }

    /// A heartbeat bumps `updated_at` alone; the fingerprint skip must keep
    /// an already-claimed settle from ever reaching the ledger again.
    #[test]
    fn heartbeat_repeats_of_a_settled_row_write_once() {
        let dir = tempfile::tempdir().unwrap();
        let store = DocsStore::open(dir.path()).unwrap();
        let mut state = NotifyState::default();
        let mut child = chat("child", "dev");
        child.parent_chat_id = Some("parent".into());
        child.spawned_by_agent = true;
        let chats = vec![chat("parent", "dev"), child];

        let started_at = chrono::DateTime::from_timestamp_millis(1_000);
        for ms in 0..100 {
            let row = Session {
                last_completed_turn: None,
                chat_id: "child".to_string(),
                device_id: "dev".to_string(),
                status: SessionStatus::Errored,
                started_at,
                updated_at: chrono::DateTime::from_timestamp_millis(ms).unwrap(),
            };
            detect(
                std::slice::from_ref(&row),
                &chats,
                &store,
                "dev",
                &mut state,
            );
        }

        assert_eq!(state.claim_passes, 1);
        let rows = store.child_notifications_for("child").unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].parent_chat_id, "parent");
        assert_eq!(rows[0].outcome.as_deref(), Some("errored"));
        assert!(state.due.contains_key("parent"));
    }
}
