//! Synced composer drafts (docs/draft-sync.md §3).
//!
//! A draft is a tiny Loro doc (`zeron_doc::DraftDoc`, one `LoroText`) per chat, edited live from
//! every device of one account and from every composer window on one device. This module owns the
//! engine half:
//!
//! - one [`DraftHandle`] per chat that has a watcher or unsynced state, holding the engine's
//!   replica, a broadcast of [`DraftFrame`]s (`reset` snapshot first, then incremental deltas of
//!   EVERY doc change: window edits, remote rows, checkpoints) and the room bookkeeping;
//! - a supervised join loop per handle that reuses [`zeron_sync::ChatClient`] against
//!   `/draft/{orgId}/{chatId}/…` (same binary frames as chat2), re-checking the room **epoch**
//!   before every join and after every disconnect so a device that slept through a discard can
//!   never push a sent draft back to life;
//! - local persistence as a `draft/{chatId}` row in the docs store (the snapshot plus the last
//!   known epoch and two flags), so a draft survives a restart while offline.
//!
//! Drafts deliberately never touch `chat_outbox` / `chat_sync_jobs` (the chat sync scheduler's
//! inputs): there is no per-edit durable outbox. Local commits are pushed as rows while connected;
//! after each (re)join catches up, a handle whose replica has changes the room never acknowledged
//! pushes ONE full-snapshot row (small, and Loro merge is idempotent).
//!
//! Without an edge (Local scope, signed out, or after `disconnect_edge`) drafts still work purely
//! locally: the persisted snapshot plus the watcher frames, so several windows on one engine share
//! the draft.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::{Duration, Instant};

use base64::Engine as _;
use futures::StreamExt;
use futures::stream::BoxStream;
use loro::VersionVector;
use serde::Deserialize;
use tokio::sync::{Notify, broadcast};
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;
use zeron_doc::DraftDoc;
use zeron_proto::DraftFrame;
use zeron_sync::chat_client::{ChatDocSink, ChatEvent, RowImportOutcome};
use zeron_sync::{ChatClient, DocsStore};

use crate::chat2_host::{EdgeChatTransport, EdgeCheckpointFetcher, RoomPath};
use crate::doc_host::EdgeConfig;
use crate::http_error::describe_http_error;

/// Store doc-id namespace for drafts. Chat ids match `[A-Za-z0-9_-]+`, so this can never collide
/// with (or be mistaken for) a chat's row.
pub const DRAFT_DOC_PREFIX: &str = "draft/";

/// The store doc id of a chat's draft.
pub fn draft_doc_id(chat_id: &str) -> String {
    format!("{DRAFT_DOC_PREFIX}{chat_id}")
}

/// Whether a store doc id names a draft (never a chat).
pub fn is_draft_doc_id(doc_id: &str) -> bool {
    doc_id.starts_with(DRAFT_DOC_PREFIX)
}

/// The room-id alphabet the edge enforces (`ID_RE`): `[A-Za-z0-9_-]{1,128}`.
pub fn is_valid_chat_id(chat_id: &str) -> bool {
    (1..=128).contains(&chat_id.len())
        && chat_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Edge cap for one pushed row (`64 KiB`) minus frame headroom.
pub const MAX_ROW_BYTES: usize = 60 * 1024;
/// Edge cap for a checkpoint (`256 KiB`) minus headroom.
pub const MAX_CHECKPOINT_BYTES: usize = 240 * 1024;
/// Largest `EditDraft` update the engine accepts from a window.
pub const MAX_EDIT_BYTES: usize = 256 * 1024;

/// Timing knobs; production values via [`Default`], tests shrink them.
#[derive(Debug, Clone)]
pub struct DraftTuning {
    /// Coalescing window for pushes and local persistence after a change (250 ms: continuous
    /// typing is ~240 pushes/minute against the edge's 300/minute quota).
    pub debounce: Duration,
    /// How long a handle with no watcher and nothing unacknowledged keeps its room joined.
    pub idle_leave: Duration,
    /// Unpinned handles kept warm.
    pub handle_cap: usize,
    /// Post a checkpoint once the room's row log reaches this many rows.
    pub checkpoint_rows: u64,
    /// Ack/reconcile/idle sweep cadence while joined.
    pub tick: Duration,
    /// Epoch re-check cadence while the socket is down.
    pub epoch_recheck: Duration,
    /// Join retry backoff (base, cap).
    pub join_backoff: (Duration, Duration),
    /// After a permanent push rejection, wait this long before re-pushing.
    pub reject_backoff: Duration,
}

impl Default for DraftTuning {
    fn default() -> Self {
        Self {
            debounce: Duration::from_millis(250),
            idle_leave: Duration::from_secs(5),
            handle_cap: 16,
            checkpoint_rows: 16,
            tick: Duration::from_millis(500),
            epoch_recheck: Duration::from_secs(10),
            join_backoff: (Duration::from_millis(500), Duration::from_secs(16)),
            reject_backoff: Duration::from_secs(30),
        }
    }
}

/// Wiring for one engine runtime's draft host.
#[derive(Clone)]
pub struct DraftHostConfig {
    pub device_id: String,
    /// Org segment of the room URL (`/draft/{orgId}/…`), same source as the registry room.
    pub org_id: String,
    /// `None` = purely local drafts (Local scope without an edge, signed out).
    pub edge: Option<EdgeConfig>,
    pub tuning: DraftTuning,
}

#[derive(Debug, thiserror::Error)]
pub enum DraftError {
    #[error("invalid chat id")]
    InvalidChatId,
    #[error("invalid draft update: {0}")]
    InvalidUpdate(String),
}

/// What `ClearDraft` did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClearOutcome {
    /// Discard generation the engine currently knows.
    pub epoch: u64,
    /// The room discard has not been confirmed yet (offline / in flight); it is retried on the
    /// next join and survives restarts.
    pub pending: bool,
}

/// Point-in-time view for diagnostics and tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DraftStatus {
    pub text: String,
    pub epoch: u64,
    /// Local changes the room has not acknowledged.
    pub unacked: bool,
    pub pending_discard: bool,
    /// A room client is attached, caught up, and on the current epoch.
    pub joined: bool,
    pub watchers: usize,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

// ── persisted row ───────────────────────────────────────────────────────────

const ROW_MAGIC: &[u8; 4] = b"DRF1";
const FLAG_DIRTY: u8 = 1;
const FLAG_PENDING_DISCARD: u8 = 2;
const ROW_HEADER: usize = 4 + 8 + 1;

struct StoredDraft {
    epoch: u64,
    dirty: bool,
    pending_discard: bool,
    snapshot: Vec<u8>,
}

fn encode_row(epoch: u64, dirty: bool, pending_discard: bool, snapshot: &[u8]) -> Vec<u8> {
    let mut flags = 0;
    if dirty {
        flags |= FLAG_DIRTY;
    }
    if pending_discard {
        flags |= FLAG_PENDING_DISCARD;
    }
    let mut out = Vec::with_capacity(ROW_HEADER + snapshot.len());
    out.extend_from_slice(ROW_MAGIC);
    out.extend_from_slice(&epoch.to_le_bytes());
    out.push(flags);
    out.extend_from_slice(snapshot);
    out
}

fn decode_row(bytes: &[u8]) -> Option<StoredDraft> {
    if bytes.len() < ROW_HEADER || &bytes[..4] != ROW_MAGIC {
        return None;
    }
    let epoch = u64::from_le_bytes(bytes[4..12].try_into().ok()?);
    let flags = bytes[12];
    Some(StoredDraft {
        epoch,
        dirty: flags & FLAG_DIRTY != 0,
        pending_discard: flags & FLAG_PENDING_DISCARD != 0,
        snapshot: bytes[ROW_HEADER..].to_vec(),
    })
}

// ── shared host context ─────────────────────────────────────────────────────

struct Shared {
    store: Arc<DocsStore>,
    edge: Option<EdgeConfig>,
    org_id: String,
    device_id: String,
    tuning: DraftTuning,
    http: reqwest::Client,
    /// Cancels every worker (join loops); `DraftHost::shutdown`.
    shutdown: CancellationToken,
    /// Cancels the join loops only (`disconnect_edge`): drafts keep working locally.
    edge_off: CancellationToken,
    tasks: TaskTracker,
}

impl Shared {
    fn edge(&self) -> Option<&EdgeConfig> {
        self.edge.as_ref().filter(|_| !self.edge_off.is_cancelled())
    }

    fn room_url(&self, chat_id: &str, route: &str) -> Option<String> {
        let edge = self.edge()?;
        Some(format!(
            "{}/draft/{}/{chat_id}/{route}",
            edge.url.trim_end_matches('/'),
            self.org_id
        ))
    }
}

#[derive(Deserialize)]
struct EpochBody {
    epoch: u64,
}

#[derive(Deserialize)]
struct DiscardBody {
    epoch: u64,
}

impl Shared {
    /// `GET /epoch`.
    async fn fetch_epoch(&self, chat_id: &str) -> Result<u64, String> {
        let edge = self.edge().ok_or("edge disconnected")?;
        let url = self.room_url(chat_id, "epoch").ok_or("edge disconnected")?;
        let _permit = zeron_sync::budget::shared()
            .http(zeron_sync::budget::Priority::Interactive)
            .await
            .map_err(|e| e.to_string())?;
        let bearer = edge.bearer().await.map_err(|e| e.to_string())?;
        let res = self
            .http
            .get(url)
            .bearer_auth(&bearer)
            .timeout(Duration::from_secs(15))
            .send()
            .await
            .map_err(describe_http_error)?;
        if !res.status().is_success() {
            return Err(format!("draft epoch http {}", res.status()));
        }
        let body: EpochBody = res.json().await.map_err(|e| e.to_string())?;
        Ok(body.epoch)
    }

    /// `POST /discard?epoch=N` → the room's epoch after the call (idempotent: a stale `N` is a
    /// no-op that reports the current epoch).
    async fn post_discard(&self, chat_id: &str, epoch: u64) -> Result<u64, String> {
        let edge = self.edge().ok_or("edge disconnected")?;
        let url = self
            .room_url(chat_id, "discard")
            .ok_or("edge disconnected")?;
        let _permit = zeron_sync::budget::shared()
            .http(zeron_sync::budget::Priority::Interactive)
            .await
            .map_err(|e| e.to_string())?;
        let bearer = edge.bearer().await.map_err(|e| e.to_string())?;
        let res = self
            .http
            .post(url)
            .query(&[("epoch", epoch.to_string())])
            .bearer_auth(&bearer)
            .timeout(Duration::from_secs(15))
            .send()
            .await
            .map_err(describe_http_error)?;
        if !res.status().is_success() {
            return Err(format!("draft discard http {}", res.status()));
        }
        let body: DiscardBody = res.json().await.map_err(|e| e.to_string())?;
        Ok(body.epoch)
    }

    /// `POST /checkpoint?epoch=E&seqCovered=N`. `Ok(false)` = epoch mismatch (409).
    async fn post_checkpoint(
        &self,
        chat_id: &str,
        epoch: u64,
        seq_covered: u64,
        frontier: &[u8],
        snapshot: Vec<u8>,
    ) -> Result<bool, String> {
        let edge = self.edge().ok_or("edge disconnected")?;
        let url = self
            .room_url(chat_id, "checkpoint")
            .ok_or("edge disconnected")?;
        let _permit = zeron_sync::budget::shared()
            .http(zeron_sync::budget::Priority::Background)
            .await
            .map_err(|e| e.to_string())?;
        let bearer = edge.bearer().await.map_err(|e| e.to_string())?;
        let res = self
            .http
            .post(url)
            .query(&[
                ("epoch", epoch.to_string()),
                ("seqCovered", seq_covered.to_string()),
            ])
            .bearer_auth(&bearer)
            .header(
                "x-chat2-frontier",
                base64::engine::general_purpose::STANDARD.encode(frontier),
            )
            .timeout(Duration::from_secs(60))
            .body(snapshot)
            .send()
            .await
            .map_err(describe_http_error)?;
        match res.status().as_u16() {
            200..=299 => Ok(true),
            409 => Ok(false),
            code => Err(format!("draft checkpoint http {code}")),
        }
    }
}

/// Per-dial room socket URL: the current epoch is read at dial time, so a redial after an epoch
/// change never carries a stale one (and the server refuses a mismatch with 409 anyway).
struct DraftRoomUrl {
    base: String,
    edge: EdgeConfig,
    epoch: Arc<AtomicU64>,
}

impl zeron_sync::UrlProvider for DraftRoomUrl {
    fn url(&self) -> futures::future::BoxFuture<'static, Result<String, zeron_sync::SyncError>> {
        let token = self.edge.token.clone();
        let base = self.base.clone();
        let device = self.edge.device_id.clone();
        let epoch = self.epoch.load(Ordering::Acquire);
        Box::pin(async move {
            let token = token.token().await.map_err(zeron_sync::SyncError::from)?;
            let mut url = format!("{base}?token={token}&epoch={epoch}");
            if !device.is_empty() {
                url.push_str(&format!("&device={device}"));
            }
            Ok(url)
        })
    }
}

// ── handle ──────────────────────────────────────────────────────────────────

struct State {
    doc: DraftDoc,
    /// Last known room epoch; `0` = never joined (a fresh, never-synced draft keeps its text when
    /// it first learns the room's epoch).
    epoch: u64,
    /// Bumped on every change made on THIS device (window edits). Remote imports never bump it.
    local_seq: u64,
    /// `local_seq` covered by the last row enqueued to the current room client.
    flushed_seq: u64,
    /// `local_seq` the room has acknowledged.
    acked_seq: u64,
    /// Version already enqueued to the current client (empty = nothing; the next push is a full
    /// snapshot).
    flushed_vv: VersionVector,
    pending_discard: bool,
    flush_scheduled: bool,
    client: Option<Arc<ChatClient>>,
    joined_epoch: u64,
    loop_running: bool,
    rows_since_checkpoint: u64,
    checkpointing: bool,
    rejected_at: Option<Instant>,
    oversize_logged: bool,
}

impl State {
    fn unacked(&self) -> bool {
        self.acked_seq < self.local_seq
    }
}

/// One chat's draft on this engine.
pub struct DraftHandle {
    chat_id: String,
    shared: Arc<Shared>,
    state: Mutex<State>,
    frames: broadcast::Sender<DraftFrame>,
    watchers: AtomicUsize,
    /// Mirror of `State::epoch` for the URL provider / checkpoint fetcher / rows transport.
    epoch_cell: Arc<AtomicU64>,
    /// Nudges the join loop (discard requested, watcher gone, change to reconcile).
    wake: Notify,
    persist_lock: Mutex<()>,
    touched: AtomicU64,
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

impl DraftHandle {
    fn load(shared: &Arc<Shared>, chat_id: &str) -> Arc<Self> {
        let stored = match shared.store.load_snapshot(&draft_doc_id(chat_id)) {
            Ok(Some(bytes)) => match decode_row(&bytes) {
                Some(row) => Some(row),
                None => {
                    tracing::warn!(chat = %chat_id, "draft row unreadable; starting empty");
                    None
                }
            },
            Ok(None) => None,
            Err(err) => {
                tracing::warn!(chat = %chat_id, %err, "draft row load failed; starting empty");
                None
            }
        };
        let (doc, epoch, dirty, pending_discard) = match stored {
            Some(row) => match DraftDoc::from_snapshot(&row.snapshot) {
                Ok(doc) => (doc, row.epoch, row.dirty, row.pending_discard),
                Err(err) => {
                    tracing::warn!(chat = %chat_id, %err, "draft snapshot unreadable; starting empty");
                    (DraftDoc::new(), row.epoch, false, row.pending_discard)
                }
            },
            None => (DraftDoc::new(), 0, false, false),
        };
        let (frames, _) = broadcast::channel(256);
        Arc::new(Self {
            chat_id: chat_id.to_string(),
            shared: shared.clone(),
            state: Mutex::new(State {
                doc,
                epoch,
                local_seq: u64::from(dirty),
                flushed_seq: 0,
                acked_seq: 0,
                flushed_vv: VersionVector::default(),
                pending_discard,
                flush_scheduled: false,
                client: None,
                joined_epoch: 0,
                loop_running: false,
                rows_since_checkpoint: 0,
                checkpointing: false,
                rejected_at: None,
                oversize_logged: false,
            }),
            frames,
            watchers: AtomicUsize::new(0),
            epoch_cell: Arc::new(AtomicU64::new(epoch)),
            wake: Notify::new(),
            persist_lock: Mutex::new(()),
            touched: AtomicU64::new(0),
        })
    }

    fn wanted(&self, st: &State) -> bool {
        self.watchers.load(Ordering::Acquire) > 0 || st.unacked() || st.pending_discard
    }

    /// Kept in the host map: a watcher, unacknowledged state, a queued flush, a running join
    /// loop, or any in-flight caller holding the handle.
    fn pinned(self: &Arc<Self>) -> bool {
        let st = lock(&self.state);
        self.wanted(&st) || st.flush_scheduled || st.loop_running || Arc::strong_count(self) > 1
    }

    fn frame(&self, st: &State, reset: bool, update: &[u8]) -> DraftFrame {
        DraftFrame {
            epoch: st.epoch,
            reset,
            update: b64(update),
        }
    }

    fn emit(&self, st: &State, reset: bool, update: &[u8]) {
        // No receivers is fine: the frame is a broadcast, not a queue.
        let _ = self.frames.send(self.frame(st, reset, update));
    }

    fn emit_reset(&self, st: &State) {
        self.emit(st, true, &st.doc.snapshot());
    }

    /// The first frame for a new watcher plus a receiver for everything after it, taken under
    /// one lock so no delta falls between them.
    fn subscribe(&self) -> (DraftFrame, broadcast::Receiver<DraftFrame>) {
        let st = lock(&self.state);
        let rx = self.frames.subscribe();
        (self.frame(&st, true, &st.doc.snapshot()), rx)
    }

    /// Merge a window's update into the engine replica.
    fn apply_edit(self: &Arc<Self>, bytes: &[u8]) -> Result<bool, DraftError> {
        let changed = {
            let mut st = lock(&self.state);
            let before = st.doc.version();
            st.doc
                .import(bytes)
                .map_err(|e| DraftError::InvalidUpdate(e.to_string()))?;
            let changed = st.doc.version() != before;
            if changed {
                st.local_seq += 1;
                if let Ok(delta) = st.doc.export_since(&before) {
                    self.emit(&st, false, &delta);
                }
            }
            changed
        };
        if changed {
            self.schedule_flush();
        }
        Ok(changed)
    }

    /// Merge rows / checkpoints from the room. Dropped while a discard is pending: the old
    /// epoch's content must not be adopted.
    fn apply_remote(self: &Arc<Self>, bytes: &[u8]) -> Result<(), String> {
        let changed = {
            let st = lock(&self.state);
            if st.pending_discard {
                return Ok(());
            }
            let before = st.doc.version();
            st.doc.import(bytes).map_err(|e| e.to_string())?;
            let changed = st.doc.version() != before;
            if changed && let Ok(delta) = st.doc.export_since(&before) {
                self.emit(&st, false, &delta);
            }
            changed
        };
        if changed {
            self.schedule_flush();
        }
        Ok(())
    }

    /// Empty the draft and start (or record) the room discard.
    fn clear(self: &Arc<Self>) -> ClearOutcome {
        let outcome = {
            let mut st = lock(&self.state);
            // A fresh doc, not `set_text("")`: the point of a discard is that the typing history
            // dies with it.
            st.doc = DraftDoc::new();
            st.local_seq += 1;
            st.flushed_seq = st.local_seq;
            st.acked_seq = st.local_seq;
            st.flushed_vv = VersionVector::default();
            st.rejected_at = None;
            st.oversize_logged = false;
            st.pending_discard = self.shared.edge().is_some();
            self.emit_reset(&st);
            ClearOutcome {
                epoch: st.epoch,
                pending: st.pending_discard,
            }
        };
        // Durable before we return: a crash after a send must not resurrect the text.
        self.persist();
        self.wake.notify_one();
        outcome
    }

    /// Learn the room's epoch. A first contact keeps local text; a changed epoch means the draft
    /// was sent elsewhere, so the local replica is dropped and watchers are reset.
    fn adopt_epoch(&self, epoch: u64) {
        {
            let mut st = lock(&self.state);
            if st.epoch == epoch {
                return;
            }
            if st.epoch != 0 {
                tracing::info!(chat = %self.chat_id, from = st.epoch, to = epoch,
                    "draft epoch changed; dropping local replica");
                st.doc = DraftDoc::new();
                st.local_seq += 1;
                st.flushed_seq = st.local_seq;
                st.acked_seq = st.local_seq;
                st.flushed_vv = VersionVector::default();
                st.rows_since_checkpoint = 0;
                st.epoch = epoch;
                self.epoch_cell.store(epoch, Ordering::Release);
                self.emit_reset(&st);
            } else {
                st.epoch = epoch;
                self.epoch_cell.store(epoch, Ordering::Release);
            }
        }
        self.persist();
    }

    /// Write the local row (or drop it when there is nothing worth keeping).
    fn persist(&self) {
        let _serial = lock(&self.persist_lock);
        let row = {
            let st = lock(&self.state);
            let dirty = st.unacked();
            if st.doc.is_empty() && !dirty && !st.pending_discard {
                None
            } else {
                Some(encode_row(
                    st.epoch,
                    dirty,
                    st.pending_discard,
                    &st.doc.snapshot(),
                ))
            }
        };
        let id = draft_doc_id(&self.chat_id);
        let result = match row {
            Some(bytes) => self.shared.store.save_snapshot(&id, &bytes),
            None => self.shared.store.delete_snapshot(&id),
        };
        if let Err(err) = result {
            tracing::warn!(chat = %self.chat_id, %err, "draft persist failed");
        }
    }

    fn schedule_flush(self: &Arc<Self>) {
        {
            let mut st = lock(&self.state);
            if st.flush_scheduled {
                return;
            }
            st.flush_scheduled = true;
        }
        let handle = self.clone();
        let debounce = self.shared.tuning.debounce;
        self.shared.tasks.spawn(async move {
            tokio::time::sleep(debounce).await;
            handle.flush();
        });
    }

    /// Debounced worker: push unflushed local changes to the joined room client and persist.
    fn flush(self: &Arc<Self>) {
        {
            let mut st = lock(&self.state);
            st.flush_scheduled = false;
        }
        self.push_unflushed();
        self.persist();
        self.wake.notify_one();
    }

    /// Enqueue whatever this device changed since the last enqueue on the current client.
    fn push_unflushed(&self) {
        let mut st = lock(&self.state);
        if st.pending_discard || st.epoch == 0 || st.joined_epoch != st.epoch {
            return;
        }
        let Some(client) = st.client.clone() else {
            return;
        };
        // A queued debounce owns the next push; the flush clears this flag before it calls in.
        if st.flushed_seq >= st.local_seq || st.flush_scheduled {
            return;
        }
        if st
            .rejected_at
            .is_some_and(|at| at.elapsed() < self.shared.tuning.reject_backoff)
        {
            return;
        }
        // Nothing enqueued on this client yet: ONE full snapshot row (merge is idempotent), so a
        // reconnect never depends on which deltas an earlier client instance managed to send.
        let payload = if st.flushed_vv.is_empty() {
            st.doc.snapshot()
        } else {
            match st.doc.export_since(&st.flushed_vv) {
                Ok(delta) => delta,
                Err(err) => {
                    tracing::warn!(chat = %self.chat_id, %err, "draft delta export failed");
                    return;
                }
            }
        };
        if payload.len() > MAX_ROW_BYTES {
            // The room can never accept this row. Keep the text locally (it is persisted) and do
            // not hammer the server; a smaller draft goes out on a later change.
            if !st.oversize_logged {
                tracing::warn!(chat = %self.chat_id, bytes = payload.len(),
                    "draft exceeds the room row cap; keeping it local only");
                st.oversize_logged = true;
            }
            return;
        }
        st.oversize_logged = false;
        client.enqueue_update(payload);
        st.flushed_vv = st.doc.version();
        st.flushed_seq = st.local_seq;
        st.rows_since_checkpoint += 1;
    }

    /// Retire acknowledged pushes. The client's `pending_pushes` reaches zero once every enqueued
    /// row was acked; `flushed_seq` is read BEFORE the stats so a later enqueue is never
    /// mistaken for acknowledged.
    fn note_acks(&self, client: &ChatClient) {
        let flushed = lock(&self.state).flushed_seq;
        if client.stats().pending_pushes != 0 {
            return;
        }
        let advanced = {
            let mut st = lock(&self.state);
            if st.acked_seq < flushed {
                st.acked_seq = flushed;
                true
            } else {
                false
            }
        };
        if advanced {
            self.persist();
        }
    }

    /// A row was permanently refused: it left the client's queue but not the doc. Re-push as a
    /// fresh snapshot after a backoff.
    fn note_push_rejected(&self) {
        let mut st = lock(&self.state);
        tracing::warn!(chat = %self.chat_id, "draft push rejected; will retry as a snapshot");
        st.flushed_seq = st.acked_seq;
        st.flushed_vv = VersionVector::default();
        st.rejected_at = Some(Instant::now());
    }

    fn touch(&self, clock: &AtomicU64) {
        self.touched
            .store(clock.fetch_add(1, Ordering::Relaxed) + 1, Ordering::Relaxed);
    }

    fn status(&self) -> DraftStatus {
        let st = lock(&self.state);
        DraftStatus {
            text: st.doc.text(),
            epoch: st.epoch,
            unacked: st.unacked(),
            pending_discard: st.pending_discard,
            joined: st.joined_epoch == st.epoch
                && st.epoch != 0
                && st.client.as_ref().is_some_and(|c| c.caught_up()),
            watchers: self.watchers.load(Ordering::Acquire),
        }
    }
}

/// Decrements the watcher count when the `WatchDraft` stream is dropped.
struct WatchGuard {
    handle: Arc<DraftHandle>,
}

impl Drop for WatchGuard {
    fn drop(&mut self) {
        self.handle.watchers.fetch_sub(1, Ordering::AcqRel);
        self.handle.wake.notify_one();
    }
}

// ── room sink ───────────────────────────────────────────────────────────────

/// Imports room rows/checkpoints into the handle's replica. Weak: the client outlives nothing.
struct DraftSink {
    handle: Weak<DraftHandle>,
}

impl ChatDocSink for DraftSink {
    fn apply_row(&self, bytes: &[u8], _cursor: u64) -> RowImportOutcome {
        if let Some(handle) = self.handle.upgrade()
            && let Err(err) = handle.apply_remote(bytes)
        {
            // Malformed remote bytes cost the row, never the draft.
            tracing::warn!(chat = %handle.chat_id, %err, "draft sink: row import failed; skipping");
        }
        RowImportOutcome::Applied
    }

    fn apply_checkpoint(&self, bytes: &[u8], _cursor: u64) -> Result<(), String> {
        match self.handle.upgrade() {
            Some(handle) => handle.apply_remote(bytes),
            None => Ok(()),
        }
    }

    fn contains_frontier(&self, frontier: &[u8]) -> bool {
        let Some(handle) = self.handle.upgrade() else {
            return true;
        };
        let Ok(vv) = VersionVector::decode(frontier) else {
            return false;
        };
        // An empty frontier is the vacuous claim every doc satisfies: fetch (always safe).
        if vv.is_empty() {
            return false;
        }
        lock(&handle.state).doc.version().includes_vv(&vv)
    }

    fn advance_cursor(&self, _cursor: u64) {}
}

// ── host ────────────────────────────────────────────────────────────────────

/// The engine's draft registry. Cheap to clone.
#[derive(Clone)]
pub struct DraftHost {
    shared: Arc<Shared>,
    handles: Arc<Mutex<HashMap<String, Arc<DraftHandle>>>>,
    clock: Arc<AtomicU64>,
    resumed: Arc<std::sync::atomic::AtomicBool>,
}

enum JoinEnd {
    /// State changed (epoch moved, discard requested): start over immediately.
    Rejoin,
    /// Transient failure: back off, then start over.
    Retry,
    /// Nothing left to sync (or shutting down).
    Leave,
}

impl DraftHost {
    pub fn new(store: Arc<DocsStore>, config: DraftHostConfig) -> Self {
        let shared = Arc::new(Shared {
            store,
            edge: config.edge,
            org_id: config.org_id,
            device_id: config.device_id,
            tuning: config.tuning,
            http: reqwest::Client::new(),
            shutdown: CancellationToken::new(),
            edge_off: CancellationToken::new(),
            tasks: TaskTracker::new(),
        });
        Self {
            shared,
            handles: Arc::new(Mutex::new(HashMap::new())),
            clock: Arc::new(AtomicU64::new(0)),
            resumed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    /// Re-attach drafts that still owe the room something after a restart: unacknowledged edits
    /// and unconfirmed discards. Without this a draft typed offline (or a send whose discard
    /// never landed) would sit on disk until its chat's composer next opened. Needs a runtime;
    /// runs at most once, and every draft RPC calls it as a fallback.
    pub fn resume_pending(&self) {
        if self.shared.edge().is_none() || tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        if self.resumed.swap(true, Ordering::AcqRel) {
            return;
        }
        let ids = match self.shared.store.doc_ids_with_prefix(DRAFT_DOC_PREFIX) {
            Ok(ids) => ids,
            Err(err) => {
                tracing::warn!(%err, "draft resume: listing rows failed");
                return;
            }
        };
        for id in ids {
            let Some(chat_id) = id.strip_prefix(DRAFT_DOC_PREFIX) else {
                continue;
            };
            if let Ok(handle) = self.open_handle(chat_id) {
                self.ensure_sync(&handle);
            }
        }
    }

    /// Whether this host has a room to sync with (false = purely local drafts).
    pub fn is_synced(&self) -> bool {
        self.shared.edge().is_some()
    }

    // ── RPC surface ─────────────────────────────────────────────────────────

    /// `WatchDraft`: a `reset` frame with the engine's snapshot first, then a frame per doc
    /// change. A later `reset` frame means the draft was discarded (from any device). A watcher
    /// that falls behind the broadcast gets a fresh `reset` instead of a gap.
    pub fn watch(&self, chat_id: &str) -> Result<BoxStream<'static, DraftFrame>, DraftError> {
        self.resume_pending();
        let handle = self.open_handle(chat_id)?;
        handle.watchers.fetch_add(1, Ordering::AcqRel);
        let guard = WatchGuard {
            handle: handle.clone(),
        };
        let (first, rx) = handle.subscribe();
        self.ensure_sync(&handle);
        let stream = futures::stream::unfold(
            (Some(first), rx, guard),
            |(first, mut rx, guard)| async move {
                if let Some(frame) = first {
                    return Some((frame, (None, rx, guard)));
                }
                match rx.recv().await {
                    Ok(frame) => Some((frame, (None, rx, guard))),
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        let (frame, fresh) = guard.handle.subscribe();
                        Some((frame, (None, fresh, guard)))
                    }
                    Err(broadcast::error::RecvError::Closed) => None,
                }
            },
        );
        Ok(stream.boxed())
    }

    /// `EditDraft`: merge a window's Loro update. Returns the resulting epoch and whether the
    /// update changed the replica.
    pub fn edit(&self, chat_id: &str, update: &[u8]) -> Result<(u64, bool), DraftError> {
        if update.len() > MAX_EDIT_BYTES {
            return Err(DraftError::InvalidUpdate("update too large".into()));
        }
        self.resume_pending();
        let handle = self.open_handle(chat_id)?;
        let changed = handle.apply_edit(update)?;
        if changed {
            self.ensure_sync(&handle);
        }
        let epoch = lock(&handle.state).epoch;
        Ok((epoch, changed))
    }

    /// `ClearDraft`: the draft was sent — empty it here and discard the room everywhere.
    pub fn clear(&self, chat_id: &str) -> Result<ClearOutcome, DraftError> {
        self.resume_pending();
        let handle = self.open_handle(chat_id)?;
        let outcome = handle.clear();
        self.ensure_sync(&handle);
        Ok(outcome)
    }

    // ── introspection ───────────────────────────────────────────────────────

    pub fn status(&self, chat_id: &str) -> Result<DraftStatus, DraftError> {
        Ok(self.open_handle(chat_id)?.status())
    }

    /// Handles currently held (bounded by the LRU except for pinned ones).
    pub fn handle_count(&self) -> usize {
        lock(&self.handles).len()
    }

    // ── lifecycle ───────────────────────────────────────────────────────────

    /// Leave every room; drafts keep working locally (sign-out / identity switch).
    pub fn disconnect_edge(&self) {
        self.shared.edge_off.cancel();
    }

    /// Persist every open draft now.
    pub fn flush_all(&self) {
        let handles: Vec<_> = lock(&self.handles).values().cloned().collect();
        for handle in handles {
            handle.persist();
        }
    }

    /// Stop every worker, wait for them, and persist. Idempotent.
    pub async fn shutdown(&self) {
        self.shared.shutdown.cancel();
        self.shared.edge_off.cancel();
        self.shared.tasks.close();
        self.shared.tasks.wait().await;
        self.flush_all();
    }

    // ── internals ───────────────────────────────────────────────────────────

    fn open_handle(&self, chat_id: &str) -> Result<Arc<DraftHandle>, DraftError> {
        if !is_valid_chat_id(chat_id) {
            return Err(DraftError::InvalidChatId);
        }
        let mut map = lock(&self.handles);
        if let Some(handle) = map.get(chat_id) {
            handle.touch(&self.clock);
            return Ok(handle.clone());
        }
        let handle = DraftHandle::load(&self.shared, chat_id);
        handle.touch(&self.clock);
        map.insert(chat_id.to_string(), handle.clone());
        self.evict_locked(&mut map);
        Ok(handle)
    }

    /// Drop least-recently-used handles beyond the cap that hold nothing worth keeping. Anything
    /// unacknowledged is pinned (and also durable in the store, so it would survive anyway).
    fn evict_locked(&self, map: &mut HashMap<String, Arc<DraftHandle>>) {
        while map.len() > self.shared.tuning.handle_cap {
            let victim = map
                .iter()
                .filter(|(_, handle)| !handle.pinned())
                .min_by_key(|(_, handle)| handle.touched.load(Ordering::Relaxed))
                .map(|(id, _)| id.clone());
            let Some(id) = victim else { return };
            if let Some(handle) = map.remove(&id) {
                handle.persist();
                tracing::debug!(chat = %id, "draft handle evicted (LRU)");
            }
        }
    }

    /// Start the join loop for a handle that has something to sync.
    fn ensure_sync(&self, handle: &Arc<DraftHandle>) {
        if self.shared.edge().is_none() || self.shared.shutdown.is_cancelled() {
            return;
        }
        {
            let mut st = lock(&handle.state);
            if st.loop_running || !handle.wanted(&st) {
                return;
            }
            st.loop_running = true;
        }
        let host = self.clone();
        let handle = handle.clone();
        self.shared.tasks.spawn(async move {
            host.sync_loop(&handle).await;
            lock(&handle.state).loop_running = false;
            // A watcher or edit that arrived while we were exiting must not be stranded.
            host.ensure_sync(&handle);
        });
    }

    /// Whether the loop has nothing left to do. Not atomic with clearing `loop_running`, which is
    /// fine: the spawner re-runs `ensure_sync` after the loop exits, so an edit or watcher that
    /// lands in between starts a fresh loop instead of being stranded.
    fn try_leave(&self, handle: &DraftHandle) -> bool {
        let st = lock(&handle.state);
        !handle.wanted(&st)
    }

    async fn sync_loop(&self, handle: &Arc<DraftHandle>) {
        self.sync_loop_inner(handle).await;
        // Whatever ended the loop (idle, sign-out, shutdown, a cancelled join), the room client
        // must not outlive it: dropping the last reference aborts its actor and socket.
        let mut st = lock(&handle.state);
        st.client = None;
        st.joined_epoch = 0;
    }

    async fn sync_loop_inner(&self, handle: &Arc<DraftHandle>) {
        let (base, cap) = self.shared.tuning.join_backoff;
        let mut backoff = base;
        let mut wake = zeron_sync::wake::subscribe();
        let mut online = zeron_sync::wake::subscribe_online();
        loop {
            if self.shared.edge().is_none() {
                return;
            }
            if self.try_leave(handle) {
                return;
            }
            let end = tokio::select! {
                end = self.join_once(handle) => end,
                _ = self.shared.edge_off.cancelled() => return,
            };
            match end {
                JoinEnd::Leave => return,
                JoinEnd::Rejoin => {
                    backoff = base;
                }
                JoinEnd::Retry => {
                    while online.try_recv().is_ok() {}
                    tokio::select! {
                        _ = self.shared.edge_off.cancelled() => return,
                        _ = tokio::time::sleep(backoff) => {
                            backoff = (backoff * 2).min(cap);
                        }
                        _ = wake.recv() => backoff = base,
                        _ = online.recv() => backoff = base,
                        _ = handle.wake.notified() => backoff = base,
                    }
                }
            }
        }
    }

    /// One membership: settle a pending discard, learn the epoch, run a `ChatClient` on it until
    /// something changes.
    async fn join_once(&self, handle: &Arc<DraftHandle>) -> JoinEnd {
        let Some(edge) = self.shared.edge().cloned() else {
            return JoinEnd::Leave;
        };
        let chat = handle.chat_id.clone();

        // A. A pending discard settles BEFORE anything is pulled: the old epoch's content must
        //    never be adopted, and the local doc counts as empty until the room confirms.
        if lock(&handle.state).pending_discard {
            let known = lock(&handle.state).epoch;
            let epoch = if known != 0 {
                known
            } else {
                match self.shared.fetch_epoch(&chat).await {
                    Ok(epoch) => epoch,
                    Err(err) => {
                        tracing::debug!(chat = %chat, %err, "draft epoch fetch failed (pending discard)");
                        return JoinEnd::Retry;
                    }
                }
            };
            match self.shared.post_discard(&chat, epoch).await {
                Ok(new_epoch) => {
                    {
                        let mut st = lock(&handle.state);
                        st.epoch = new_epoch;
                        handle.epoch_cell.store(new_epoch, Ordering::Release);
                        st.pending_discard = false;
                        st.rows_since_checkpoint = 0;
                    }
                    handle.persist();
                    tracing::info!(chat = %chat, epoch = new_epoch, "draft discarded");
                }
                Err(err) => {
                    tracing::warn!(chat = %chat, %err, "draft discard failed; will retry");
                    return JoinEnd::Retry;
                }
            }
            if self.try_leave(handle) {
                return JoinEnd::Leave;
            }
        }

        // B. Epoch. A changed epoch means the draft was sent elsewhere: drop the local replica.
        let epoch = match self.shared.fetch_epoch(&chat).await {
            Ok(epoch) => epoch,
            Err(err) => {
                tracing::debug!(chat = %chat, %err, "draft epoch fetch failed");
                return JoinEnd::Retry;
            }
        };
        handle.adopt_epoch(epoch);
        if lock(&handle.state).pending_discard {
            return JoinEnd::Rejoin; // cleared while we were fetching
        }

        // C. Join the room on that epoch.
        let sink = Arc::new(DraftSink {
            handle: Arc::downgrade(handle),
        });
        let room = RoomPath::draft(&self.shared.org_id, &chat, handle.epoch_cell.clone());
        let fetcher = Arc::new(EdgeCheckpointFetcher::for_room(
            self.shared.http.clone(),
            edge.clone(),
            room.clone(),
        ));
        let transport = Arc::new(EdgeChatTransport::for_room(
            self.shared.http.clone(),
            edge.clone(),
            room,
            self.shared.device_id.clone(),
        ));
        let url = Arc::new(DraftRoomUrl {
            base: format!(
                "{}/draft/{}/{chat}/ws",
                edge.url.replacen("http", "ws", 1).trim_end_matches('/'),
                self.shared.org_id
            ),
            edge: edge.clone(),
            epoch: handle.epoch_cell.clone(),
        });
        let joined = tokio::time::timeout(
            Duration::from_secs(60),
            ChatClient::connect_via_transport(
                url,
                sink,
                fetcher,
                &self.shared.device_id,
                0,
                transport,
            ),
        )
        .await;
        let client = match joined {
            Ok(Ok(client)) => Arc::new(client),
            Ok(Err(err)) => {
                tracing::warn!(chat = %chat, %err, "draft room join failed");
                return JoinEnd::Retry;
            }
            Err(_) => {
                tracing::warn!(chat = %chat, "draft room join timed out");
                return JoinEnd::Retry;
            }
        };
        {
            let mut st = lock(&handle.state);
            st.client = Some(client.clone());
            st.joined_epoch = epoch;
            // A new client has an empty queue: whatever was unacknowledged goes out again.
            st.flushed_seq = st.acked_seq;
            st.flushed_vv = VersionVector::default();
        }
        let end = self.run_membership(handle, &client, epoch).await;
        {
            let mut st = lock(&handle.state);
            st.client = None;
            st.joined_epoch = 0;
        }
        drop(client);
        end
    }

    async fn run_membership(
        &self,
        handle: &Arc<DraftHandle>,
        client: &Arc<ChatClient>,
        epoch: u64,
    ) -> JoinEnd {
        let chat = handle.chat_id.clone();
        let tuning = &self.shared.tuning;
        let mut events = client.events();
        let mut ticker = tokio::time::interval(tuning.tick);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut token_changes = self.shared.edge().and_then(EdgeConfig::token_changes);
        let mut idle_since: Option<Instant> = None;
        let mut last_epoch_check = Instant::now();
        let mut seeded_rows = false;
        loop {
            tokio::select! {
                _ = self.shared.edge_off.cancelled() => return JoinEnd::Leave,
                event = events.recv() => match event {
                    Ok(ChatEvent::Disconnected | ChatEvent::ServerReset) => {
                        // ChatClient does not surface close codes (4411 = discarded), so any
                        // drop re-checks the epoch before the client redials into it.
                        if self.epoch_moved(&chat, epoch).await {
                            return JoinEnd::Rejoin;
                        }
                        last_epoch_check = Instant::now();
                    }
                    Ok(ChatEvent::PushRejected) => handle.note_push_rejected(),
                    Ok(ChatEvent::Applied | ChatEvent::CaughtUp { .. }) => {
                        handle.note_acks(client);
                        if client.caught_up() {
                            handle.push_unflushed();
                        }
                    }
                    Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => return JoinEnd::Retry,
                },
                _ = handle.wake.notified() => {
                    if lock(&handle.state).pending_discard {
                        return JoinEnd::Rejoin;
                    }
                    handle.note_acks(client);
                }
                _ = crate::workspace_host::token_changed(&mut token_changes) => {
                    if let Some(edge) = self.shared.edge()
                        && matches!(edge.bearer().await, Err(zeron_rpc::TokenError::SignedOut))
                    {
                        return JoinEnd::Leave;
                    }
                }
                _ = ticker.tick() => {
                    let caught_up = client.caught_up();
                    if caught_up && !seeded_rows {
                        // The room's row log length decides when a checkpoint is due.
                        let rows = client.stats().row_count;
                        lock(&handle.state).rows_since_checkpoint += rows;
                        seeded_rows = true;
                    }
                    handle.note_acks(client);
                    if caught_up {
                        // Post-join reconcile and retry of rejected/oversize rows.
                        handle.push_unflushed();
                        self.maybe_checkpoint(handle, client, epoch);
                    }
                    let connected = client.stats().connected;
                    if !connected && last_epoch_check.elapsed() >= tuning.epoch_recheck {
                        last_epoch_check = Instant::now();
                        if self.epoch_moved(&chat, epoch).await {
                            return JoinEnd::Rejoin;
                        }
                    }
                    let wanted = {
                        let st = lock(&handle.state);
                        handle.wanted(&st)
                    };
                    if wanted {
                        idle_since = None;
                    } else {
                        let since = *idle_since.get_or_insert_with(Instant::now);
                        if since.elapsed() >= tuning.idle_leave && self.try_leave(handle) {
                            return JoinEnd::Leave;
                        }
                    }
                    if lock(&handle.state).pending_discard {
                        return JoinEnd::Rejoin;
                    }
                }
            }
        }
    }

    async fn epoch_moved(&self, chat: &str, joined: u64) -> bool {
        match self.shared.fetch_epoch(chat).await {
            Ok(epoch) => epoch != joined,
            Err(err) => {
                tracing::debug!(chat = %chat, %err, "draft epoch re-check failed");
                false
            }
        }
    }

    /// Post a checkpoint once the row log has grown, so a cold device loads fast. Small and
    /// bounded: one in flight, only from a fully acknowledged state, oversize drafts skipped.
    fn maybe_checkpoint(&self, handle: &Arc<DraftHandle>, client: &Arc<ChatClient>, epoch: u64) {
        // Read BEFORE the snapshot: rows are imported into the doc before the cursor advances, so
        // a cursor read first is always covered by the snapshot taken after it. The other order
        // could claim a row the snapshot lacks, and the room prunes every row up to the cursor.
        let seq_covered = client.stats().cursor;
        let (snapshot, frontier) = {
            let mut st = lock(&handle.state);
            if st.checkpointing
                || st.pending_discard
                || st.unacked()
                || st.rows_since_checkpoint < self.shared.tuning.checkpoint_rows
                || st.doc.is_empty()
            {
                return;
            }
            let snapshot = st.doc.snapshot();
            if snapshot.len() > MAX_CHECKPOINT_BYTES {
                return;
            }
            st.checkpointing = true;
            (snapshot, st.doc.version().encode())
        };
        let host = self.clone();
        let handle = handle.clone();
        let client = client.clone();
        self.shared.tasks.spawn(async move {
            let size = snapshot.len() as u64;
            let outcome = host
                .shared
                .post_checkpoint(&handle.chat_id, epoch, seq_covered, &frontier, snapshot)
                .await;
            {
                let mut st = lock(&handle.state);
                st.checkpointing = false;
                if matches!(outcome, Ok(true)) {
                    st.rows_since_checkpoint = 0;
                }
            }
            match outcome {
                Ok(true) => client.note_checkpoint(seq_covered, size),
                Ok(false) => handle.wake.notify_one(),
                Err(err) => {
                    tracing::debug!(chat = %handle.chat_id, %err, "draft checkpoint failed");
                    // Back off: try again after another batch of rows.
                    lock(&handle.state).rows_since_checkpoint = 0;
                }
            }
        });
    }
}

#[cfg(test)]
mod tests;
