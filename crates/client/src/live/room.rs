//! One chat's chat2 room membership: the viewer sink (remote rows → the
//! session doc), durable local publication (own commands/queue rows → the
//! outbox → the room), debounced snapshot+cursor persistence, and the join
//! loop that NEVER gives up (the legacy one-shot join left a chat dark until
//! relaunch).
//!
//! Shape follows the engine's `EngineChatSink` / `ChatPersistence`, minus
//! host duties (checkpoint posting, command execution).

use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use futures::future::BoxFuture;
use loro::VersionVector;
use tokio::sync::{Notify, watch};
use tokio_util::sync::CancellationToken;
use zeron_doc::SessionDoc;
use zeron_proto::relay_gen::RELAY_ROOM_GEN;
use zeron_sync::chat_client::{ChatTransport, RowImportOutcome};
use zeron_sync::chat_relay::{RelayHandle, RelayImport, RelayPolicy};
use zeron_sync::{
    ChatClient, ChatDocSink, ChatEvent, CheckpointFetcher, DocsStore, SyncError, UrlProvider,
};

use super::chat_link::{self, LinkWanted};
use super::urls::PullReader;
use super::{Bearer, JOIN_RETRY_BASE, JOIN_RETRY_CAP, urls};
use crate::lock;

/// chat2 doc epoch stamped on every snapshot (pre-chat2 epochs are ignored).
pub(crate) const CHAT2_DOC_EPOCH: u32 = 2;
/// Snapshot flush cadence while rows stream in.
const SAVE_INTERVAL: Duration = Duration::from_secs(1);

/// Loaded local state for a chat (instant open).
pub(crate) struct LocalDoc {
    pub doc: SessionDoc,
    pub cursor: u64,
    /// A snapshot or outbox existed — content is on screen immediately.
    pub had_content: bool,
    /// Newest journal row replayed here: the next save covers it.
    pub journaled: i64,
}

/// Restore a chat doc from disk: the chat2 snapshot (epoch ≥ 2 only — a
/// retired s2 lineage is never imported), any unacknowledged own writes, and what
/// came by relay since the last save, which nothing on the edge can give back.
pub(crate) fn load_local(store: &DocsStore, chat_id: &str) -> LocalDoc {
    let raw = loro::LoroDoc::new();
    let mut cursor = 0;
    let mut had_content = false;
    match store.load_snapshot_with_cursor(chat_id) {
        Ok(Some((bytes, stored_cursor, epoch))) if epoch >= CHAT2_DOC_EPOCH => {
            match raw.import(&bytes) {
                Ok(_) => {
                    had_content = true;
                    if store.snapshot_cursor_verified(chat_id).unwrap_or(false) {
                        cursor = stored_cursor;
                    }
                }
                Err(err) => {
                    tracing::warn!(chat = %chat_id, error = %err, "chat snapshot unreadable; starting fresh")
                }
            }
        }
        Ok(_) => {}
        Err(err) => tracing::warn!(chat = %chat_id, error = %err, "chat snapshot load failed"),
    }
    if let Ok(pending) = store.pending_chat_updates(chat_id) {
        for (_, bytes) in pending {
            if raw.import(&bytes).is_ok() {
                had_content = true;
            }
        }
    }
    let mut journaled = 0;
    match store.pending_chat_inbox(chat_id) {
        Ok((received, newest)) => {
            journaled = newest;
            for bytes in received {
                match raw.import(&bytes) {
                    Ok(_) => had_content = true,
                    Err(err) => {
                        tracing::warn!(chat = %chat_id, error = %err, "couldn't replay a kept relay update")
                    }
                }
            }
        }
        Err(err) => tracing::warn!(chat = %chat_id, error = %err, "relay journal load failed"),
    }
    LocalDoc {
        doc: SessionDoc::from_doc(raw),
        cursor,
        had_content,
        journaled,
    }
}

/// Debounced snapshot + cursor writer (cursor sampled BEFORE export, both
/// committed in one transaction).
pub(crate) struct Persister {
    doc: Weak<SessionDoc>,
    store: Arc<DocsStore>,
    chat_id: String,
    cursor: AtomicU64,
    generation: AtomicU64,
    saved: AtomicU64,
    urgent: AtomicBool,
    write: Mutex<()>,
    wake: Arc<Notify>,
    /// Newest row in the relay journal, and how far a save has retired it. A row is
    /// journaled after its import, so an export begun once it's noted holds it.
    journaled: AtomicI64,
    retired: AtomicI64,
}

impl Persister {
    pub(crate) fn new(
        doc: &Arc<SessionDoc>,
        store: Arc<DocsStore>,
        chat_id: &str,
        cursor: u64,
    ) -> Arc<Self> {
        Arc::new(Self {
            doc: Arc::downgrade(doc),
            store,
            chat_id: chat_id.to_owned(),
            cursor: AtomicU64::new(cursor),
            generation: AtomicU64::new(0),
            saved: AtomicU64::new(0),
            urgent: AtomicBool::new(false),
            write: Mutex::new(()),
            wake: Arc::new(Notify::new()),
            journaled: AtomicI64::new(0),
            retired: AtomicI64::new(0),
        })
    }

    /// A journal row that the next save covers.
    pub(crate) fn note_journaled(&self, ordinal: i64) {
        self.journaled.fetch_max(ordinal, Ordering::AcqRel);
    }

    pub(crate) fn start(self: &Arc<Self>, cancel: CancellationToken) {
        let weak = Arc::downgrade(self);
        let wake = self.wake.clone();
        crate::runtime::shared().spawn(async move {
            loop {
                let notified = wake.notified();
                tokio::select! {
                    _ = cancel.cancelled() => {
                        if let Some(this) = weak.upgrade() {
                            let _ = tokio::task::spawn_blocking(move || this.flush()).await;
                        }
                        return;
                    }
                    _ = notified => {}
                }
                let Some(this) = weak.upgrade() else { return };
                if !this.urgent.swap(false, Ordering::AcqRel) {
                    drop(this);
                    tokio::time::sleep(SAVE_INTERVAL).await;
                    let Some(again) = weak.upgrade() else { return };
                    let _ = tokio::task::spawn_blocking(move || again.flush()).await;
                } else {
                    let _ = tokio::task::spawn_blocking(move || this.flush()).await;
                }
            }
        });
    }

    pub(crate) fn cursor(&self) -> u64 {
        self.cursor.load(Ordering::Acquire)
    }

    pub(crate) fn applied(&self, cursor: u64, urgent: bool) {
        self.cursor.fetch_max(cursor, Ordering::AcqRel);
        self.dirty(urgent);
    }

    pub(crate) fn reset_cursor(&self, cursor: u64) {
        self.cursor.store(cursor, Ordering::Release);
        self.dirty(true);
    }

    pub(crate) fn dirty(&self, urgent: bool) {
        self.generation.fetch_add(1, Ordering::AcqRel);
        if urgent {
            self.urgent.store(true, Ordering::Release);
        }
        self.wake.notify_one();
    }

    /// Write now if anything changed since the last save (blocking I/O).
    pub(crate) fn flush(&self) {
        let _serial = lock(&self.write);
        let generation = self.generation.load(Ordering::Acquire);
        if generation == self.saved.load(Ordering::Acquire) {
            return;
        }
        let Some(doc) = self.doc.upgrade() else {
            return;
        };
        let cursor = self.cursor();
        // Before the export: only what's noted by now is certainly in it.
        let journaled = self.journaled.load(Ordering::Acquire);
        let bytes = match doc.export_snapshot() {
            Ok(bytes) => bytes,
            Err(err) => {
                tracing::warn!(chat = %self.chat_id, error = %err, "chat snapshot export failed");
                return;
            }
        };
        match self.store.save_verified_snapshot_with_cursor(
            &self.chat_id,
            &bytes,
            cursor,
            CHAT2_DOC_EPOCH,
        ) {
            Ok(()) => {
                self.saved.store(generation, Ordering::Release);
                if journaled > self.retired.load(Ordering::Acquire) {
                    match self.store.retire_chat_inbox(&self.chat_id, journaled) {
                        Ok(()) => self.retired.store(journaled, Ordering::Release),
                        Err(err) => {
                            tracing::warn!(chat = %self.chat_id, error = %err, "relay journal retirement failed")
                        }
                    }
                }
            }
            Err(err) => {
                tracing::warn!(chat = %self.chat_id, error = %err, "chat snapshot save failed")
            }
        }
    }
}

/// What the room's relay task and direct link need to know, kept current as the
/// chat's row, its host and this screen change.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct RelayRoute {
    pub generation: u32,
    pub host: String,
    /// The host's device row says it can read the queue. A host back on an older
    /// build keeps the generation it wrote, but this goes false.
    pub host_reads_queue: bool,
    pub on_screen: bool,
    pub host_online: bool,
    pub link: LinkWanted,
}

/// How long a chat on screen waits on its host before reading a deferred checkpoint.
const DEFER_PATIENCE: Duration = Duration::from_secs(8);

/// Remote bytes → the session doc; outbox hooks → the docs store.
pub(crate) struct ViewerSink {
    pub(crate) doc: Weak<SessionDoc>,
    pub(crate) store: Arc<DocsStore>,
    pub(crate) chat_id: String,
    pub(crate) persister: Arc<Persister>,
    /// Coalesced "the doc changed remotely" wake (republish).
    pub(crate) on_applied: Arc<dyn Fn() + Send + Sync>,
    /// Relay layer on for this client: the relay hooks below answer for real.
    pub(crate) relay: bool,
    /// May a checkpoint wait for the host? Set from the [`RelayRoute`].
    pub(crate) defer_checkpoint: Arc<AtomicBool>,
    /// When a relayed update last landed or left (ms), for the link's idle clock.
    pub(crate) last_activity_ms: Arc<AtomicI64>,
}

impl ViewerSink {
    fn import(&self, bytes: &[u8], cursor: u64, urgent: bool) -> RowImportOutcome {
        let Some(doc) = self.doc.upgrade() else {
            return RowImportOutcome::Applied;
        };
        match doc.doc().import(bytes) {
            Ok(status) if status.pending.is_some() => RowImportOutcome::PendingDependencies,
            Ok(_) => {
                self.persister.applied(cursor, urgent);
                (self.on_applied)();
                RowImportOutcome::Applied
            }
            Err(err) => {
                // A poison row must not wedge the room: skip it, keep going.
                tracing::warn!(chat = %self.chat_id, error = %err, "chat row import failed; skipping");
                self.persister.applied(cursor, urgent);
                RowImportOutcome::Applied
            }
        }
    }
}

impl ChatDocSink for ViewerSink {
    fn cursor_is_verified(&self) -> bool {
        self.store
            .snapshot_cursor_verified(&self.chat_id)
            .unwrap_or(false)
    }

    fn reset_cursor(&self, cursor: u64) {
        self.persister.reset_cursor(cursor);
    }

    fn pending_updates(&self) -> Result<Vec<(String, Vec<u8>)>, String> {
        let rejected: std::collections::HashSet<String> = self
            .store
            .rejected_chat_updates(&self.chat_id)
            .map_err(|e| e.to_string())?
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        Ok(self
            .store
            .pending_chat_updates(&self.chat_id)
            .map_err(|e| e.to_string())?
            .into_iter()
            .filter(|(id, bytes)| {
                !rejected.contains(id) && bytes.len() <= zeron_sync::chat_client::MAX_PUSH_BYTES
            })
            .collect())
    }

    fn persist_update(&self, batch_id: &str, bytes: &[u8]) -> Result<(), String> {
        self.store
            .enqueue_chat_update(&self.chat_id, batch_id, bytes)
            .map_err(|e| e.to_string())
    }

    fn acknowledge_update(&self, batch_id: &str) -> Result<(), String> {
        // The outbox row is this send's only local copy until a save holds it. A
        // kill right after the ack would otherwise lose it from the phone's own
        // transcript, and offline there's nowhere to read it back from.
        self.persister.flush();
        self.store
            .acknowledge_chat_update(&self.chat_id, batch_id)
            .map_err(|e| e.to_string())
    }

    fn reject_update(&self, batch_id: &str) -> Result<(), String> {
        self.store
            .reject_chat_update(&self.chat_id, batch_id)
            .map_err(|e| e.to_string())
    }

    fn apply_row(&self, bytes: &[u8], cursor: u64) -> RowImportOutcome {
        self.import(bytes, cursor, false)
    }

    fn apply_checkpoint(&self, bytes: &[u8], cursor: u64) -> Result<(), String> {
        match self.import(bytes, cursor, true) {
            RowImportOutcome::Applied => Ok(()),
            RowImportOutcome::PendingDependencies => {
                Err("checkpoint is missing causal dependencies".into())
            }
        }
    }

    fn contains_frontier(&self, frontier: &[u8]) -> bool {
        let Some(doc) = self.doc.upgrade() else {
            return false;
        };
        // An undecodable or EMPTY frontier is "not contained": fetch the
        // checkpoint (an empty-VV shortcut once left readers blank).
        match VersionVector::decode(frontier) {
            Ok(vv) if !vv.is_empty() => doc.doc().oplog_vv().includes_vv(&vv),
            _ => false,
        }
    }

    fn advance_cursor(&self, cursor: u64) {
        self.persister.applied(cursor, true);
    }

    fn relay_version(&self) -> Option<Vec<u8>> {
        if !self.relay {
            return None;
        }
        Some(self.doc.upgrade()?.doc().oplog_vv().encode())
    }

    fn relay_lacks(&self, version: &[u8]) -> bool {
        let (Some(doc), Ok(vv)) = (self.doc.upgrade(), VersionVector::decode(version)) else {
            return false;
        };
        !doc.doc().oplog_vv().includes_vv(&vv)
    }

    fn relay_behind(&self, version: &[u8]) -> bool {
        let (Some(doc), Ok(vv)) = (self.doc.upgrade(), VersionVector::decode(version)) else {
            return false;
        };
        !vv.includes_vv(&doc.doc().oplog_vv())
    }

    fn relay_export(&self, since: &[u8]) -> Option<Vec<u8>> {
        let doc = self.doc.upgrade()?;
        let vv = VersionVector::decode(since).ok()?;
        if vv.includes_vv(&doc.doc().oplog_vv()) {
            return Some(Vec::new());
        }
        doc.doc().export(loro::ExportMode::updates(&vv)).ok()
    }

    /// As with a stored row, except that nothing on the edge can give this back: it
    /// goes in the journal until a save covers it. Only what moved the doc, so a
    /// repeat delivery writes nothing.
    fn relay_import(&self, bytes: &[u8], history: bool) -> RelayImport {
        let Some(doc) = self.doc.upgrade() else {
            return RelayImport::Applied;
        };
        let before = doc.doc().oplog_vv();
        let status = match doc.doc().import(bytes) {
            Ok(status) => status,
            Err(err) => {
                tracing::warn!(chat = %self.chat_id, error = %err, "relay import failed");
                return RelayImport::Unreadable;
            }
        };
        if doc.doc().oplog_vv() != before {
            match self.store.journal_chat_inbox(&self.chat_id, bytes) {
                Ok(ordinal) => self.persister.note_journaled(ordinal),
                Err(err) => {
                    tracing::warn!(chat = %self.chat_id, error = %err, "relay journal write failed")
                }
            }
            // A catch-up can be the whole chat: save it now, not in a second.
            self.persister.dirty(history);
            self.last_activity_ms.store(crate::now_ms(), Ordering::Release);
            (self.on_applied)();
        }
        if status.pending.is_some() {
            RelayImport::Parked
        } else {
            RelayImport::Applied
        }
    }

    fn relay_durable(&self) -> bool {
        true
    }

    fn relay_defer_checkpoint(&self) -> bool {
        // A doc with nothing in it has nothing to show meanwhile, so it reads.
        self.defer_checkpoint.load(Ordering::Acquire)
            && self.doc.upgrade().is_some_and(|doc| !doc.doc().oplog_vv().is_empty())
    }
}

/// Fresh `?token=` per dial.
pub(crate) struct ChatUrl {
    pub(crate) bearer: Bearer,
    pub(crate) edge: String,
    pub(crate) chat_id: String,
    pub(crate) device_id: String,
    /// Read per dial: a chat can move to another host.
    pub(crate) host: Arc<Mutex<String>>,
}

impl UrlProvider for ChatUrl {
    fn url(&self) -> BoxFuture<'static, Result<String, SyncError>> {
        let bearer = self.bearer.clone();
        let host = lock(&self.host).clone();
        let (edge, chat, device) = (
            self.edge.clone(),
            self.chat_id.clone(),
            self.device_id.clone(),
        );
        Box::pin(async move {
            let token = bearer.get().await?;
            Ok(format!(
                "{}&hostDevice={}",
                urls::chat_ws(&edge, &chat, &token, &device),
                urls::encode(&host)
            ))
        })
    }
}

/// `GET /chat2/{id}/checkpoint` with Range resume (engine
/// `EdgeCheckpointFetcher`), plus the HTTPS rows pull/push transport.
pub(crate) struct ChatHttp {
    pub(crate) bearer: Bearer,
    pub(crate) edge: String,
    pub(crate) chat_id: String,
    pub(crate) device_id: String,
    pub(crate) host: Arc<Mutex<String>>,
    /// This client runs the relay layer: its pull isn't an old reader's.
    pub(crate) relay: bool,
}

fn http_err(err: reqwest::Error) -> SyncError {
    SyncError::WebSocket(err.to_string())
}

impl CheckpointFetcher for ChatHttp {
    fn fetch(&self) -> BoxFuture<'static, Result<Vec<u8>, SyncError>> {
        let bearer = self.bearer.clone();
        let url = urls::chat_checkpoint(&self.edge, &self.chat_id);
        Box::pin(async move {
            let mut got: Vec<u8> = Vec::new();
            let mut seen_seq: Option<String> = None;
            let mut last_failure = None;
            for _attempt in 0..4 {
                let _permit = zeron_sync::budget::shared()
                    .http(zeron_sync::budget::Priority::Interactive)
                    .await?;
                let token = bearer.get().await?;
                let mut request = crate::auth::http()
                    .get(&url)
                    .bearer_auth(&token)
                    .timeout(Duration::from_secs(300));
                if !got.is_empty() {
                    request = request.header("range", format!("bytes={}-", got.len()));
                }
                let response = match request.send().await {
                    Ok(response) => response,
                    Err(err) => {
                        last_failure = Some(err.to_string());
                        continue;
                    }
                };
                let seq = response
                    .headers()
                    .get("x-chat2-checkpoint-seq")
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_owned);
                if seq.is_some() && seen_seq.is_some() && seq != seen_seq {
                    // A new checkpoint committed between attempts: restart.
                    got.clear();
                    seen_seq = seq;
                    continue;
                }
                if seq.is_some() {
                    seen_seq = seq;
                }
                match response.status().as_u16() {
                    200 => got.clear(),
                    206 => {}
                    404 => return Err(SyncError::Protocol("no checkpoint".into())),
                    code => return Err(SyncError::Protocol(format!("checkpoint HTTP {code}"))),
                }
                let mut response = response;
                loop {
                    match response.chunk().await {
                        Ok(Some(chunk)) => got.extend_from_slice(&chunk),
                        Ok(None) => return Ok(got),
                        Err(err) => {
                            last_failure = Some(err.to_string());
                            break;
                        }
                    }
                }
            }
            Err(SyncError::Protocol(format!(
                "checkpoint fetch exhausted resume attempts: {}",
                last_failure.unwrap_or_default()
            )))
        })
    }
}

impl ChatHttp {
    fn pull(&self, after: u64, reader: PullReader) -> BoxFuture<'static, Result<Vec<u8>, SyncError>> {
        let bearer = self.bearer.clone();
        let url = urls::chat_rows(&self.edge, &self.chat_id, after, &self.device_id, reader);
        Box::pin(async move {
            let _permit = zeron_sync::budget::shared()
                .http(zeron_sync::budget::Priority::Interactive)
                .await?;
            let token = bearer.get().await?;
            let response = crate::auth::http()
                .get(url)
                .bearer_auth(token)
                .timeout(Duration::from_secs(120))
                .send()
                .await
                .map_err(http_err)?;
            if !response.status().is_success() {
                return Err(SyncError::Protocol(format!(
                    "chat pull http {}",
                    response.status()
                )));
            }
            Ok(response.bytes().await.map_err(http_err)?.to_vec())
        })
    }
}

impl ChatTransport for ChatHttp {
    fn fetch_rows(&self, after: u64) -> BoxFuture<'static, Result<Vec<u8>, SyncError>> {
        let reader = if self.relay { PullReader::Relay } else { PullReader::Stored };
        self.pull(after, reader)
    }

    fn fetch_rows_fallback(&self, after: u64) -> BoxFuture<'static, Result<Vec<u8>, SyncError>> {
        let reader = if self.relay { PullReader::Socketless } else { PullReader::Stored };
        self.pull(after, reader)
    }

    fn push(
        &self,
        batch_id: String,
        bytes: Vec<u8>,
    ) -> BoxFuture<'static, Result<String, SyncError>> {
        let bearer = self.bearer.clone();
        let url = format!(
            "{}&hostDevice={}",
            urls::chat_push(&self.edge, &self.chat_id, &batch_id, &self.device_id),
            urls::encode(&lock(&self.host))
        );
        Box::pin(async move {
            let _permit = zeron_sync::budget::shared()
                .http(zeron_sync::budget::Priority::Interactive)
                .await?;
            let token = bearer.get().await?;
            let response = crate::auth::http()
                .post(url)
                .bearer_auth(token)
                .body(bytes)
                .send()
                .await
                .map_err(http_err)?;
            if !response.status().is_success() {
                return Err(SyncError::Protocol(format!(
                    "chat push http {}",
                    response.status()
                )));
            }
            response.text().await.map_err(http_err)
        })
    }
}

/// Room connection posture (feeds connectivity + the composer strip).
#[derive(Default)]
pub(crate) struct RoomStatus {
    pub connected: AtomicBool,
    /// A server state has been applied at least once (hydration).
    pub caught_up: AtomicBool,
    /// Next scheduled join attempt while the first join keeps failing.
    pub retry_at_ms: AtomicI64,
    pub last_failure: Mutex<Option<String>>,
}

impl RoomRelay {
    fn apply_route(&self, route: &RelayRoute) {
        // This runs every second: an unchanged route mustn't wake the relay task.
        if lock(&self.applied).as_ref() == Some(route) {
            return;
        }
        *lock(&self.applied) = Some(route.clone());
        let relayed = route.generation >= RELAY_ROOM_GEN;
        self.relayed.store(relayed, Ordering::Release);
        *lock(&self.host) = route.host.clone();
        self.handle.set_host(Some(route.host.clone()));
        // Never the host here: a phone doesn't confirm the queue or relay its own.
        self.handle.set_hello(relayed, false);
        self.handle.set_queue_pushes(relayed && route.host_reads_queue);
        // Off screen it waits for as long as it likes: nobody's looking. On screen,
        // only while the host can answer.
        self.defer_checkpoint
            .store(relayed && (!route.on_screen || route.host_online), Ordering::Release);
        self.link_wanted.send_if_modified(|current| {
            let next = route.link.wanted();
            let changed = *current != next;
            *current = next;
            changed
        });
    }
}

/// One live chat room. Dropping it stops the client and flushes the doc.
pub(crate) struct Room {
    pub(crate) chat_id: String,
    slot: Arc<Mutex<Slot>>,
    pub(crate) status: Arc<RoomStatus>,
    pub(crate) persister: Arc<Persister>,
    cancel: CancellationToken,
    _local_updates: loro::Subscription,
    /// The relay layer's state. None: this client doesn't run it.
    relay: Option<RoomRelay>,
}

struct RoomRelay {
    handle: RelayHandle,
    host: Arc<Mutex<String>>,
    /// The chat is on the relay generation: a local commit also takes the direct
    /// link, and its follow-up announces go out. Stored chats pay for none of it.
    relayed: Arc<AtomicBool>,
    defer_checkpoint: Arc<AtomicBool>,
    last_activity_ms: Arc<AtomicI64>,
    link_wanted: watch::Sender<bool>,
    /// Since when a chat on screen has been waiting with its checkpoint deferred.
    deferred_since: Mutex<Option<std::time::Instant>>,
    applied: Mutex<Option<RelayRoute>>,
}

#[derive(Default)]
struct Slot {
    client: Option<ChatClient>,
    /// Own updates committed before the client existed (also durable in the
    /// store's outbox; handed to the client on join — duplicates are
    /// idempotent: batch-id dedupe + Loro re-import no-op).
    unsent: Vec<(String, Vec<u8>)>,
}

pub(crate) struct RoomDeps {
    pub bearer: Bearer,
    pub edge: String,
    pub device_id: String,
    pub store: Arc<DocsStore>,
    /// Remote change landed (schedule a coalesced session refresh).
    pub on_applied: Arc<dyn Fn() + Send + Sync>,
    /// Connection posture changed (connectivity recompute).
    pub on_status: Arc<dyn Fn() + Send + Sync>,
    /// The client's lifetime: sign-out stops the room even while a view
    /// still holds its session.
    pub cancel: CancellationToken,
    /// This client runs the relay layer.
    pub relay_layer: bool,
    /// The chat as this device knows it right now.
    pub route: RelayRoute,
    /// Newest relay journal row [`load_local`] replayed.
    pub journaled: i64,
    /// The direct peers, when they started.
    pub direct: Option<zeron_preview::viewer::ViewerPeers>,
}

impl Room {
    /// Start publishing local writes and joining the room. `cursor` is the
    /// verified cursor from [`load_local`].
    pub(crate) fn start(
        doc: &Arc<SessionDoc>,
        chat_id: &str,
        cursor: u64,
        deps: RoomDeps,
    ) -> Arc<Self> {
        let persister = Persister::new(doc, deps.store.clone(), chat_id, cursor);
        let cancel = deps.cancel.child_token();
        persister.start(cancel.clone());
        if deps.journaled > 0 {
            // Replayed at load: the next save holds it, and only then does it go.
            persister.note_journaled(deps.journaled);
            persister.dirty(false);
        }
        let slot = Arc::new(Mutex::new(Slot::default()));
        let sink = Arc::new(ViewerSink {
            doc: Arc::downgrade(doc),
            store: deps.store.clone(),
            chat_id: chat_id.to_owned(),
            persister: persister.clone(),
            on_applied: deps.on_applied.clone(),
            relay: deps.relay_layer,
            defer_checkpoint: Arc::new(AtomicBool::new(false)),
            last_activity_ms: Arc::new(AtomicI64::new(0)),
        });
        let relay = deps.relay_layer.then(|| {
            // Calls arrive from platform threads, and this spawns its task.
            let _runtime = crate::runtime::handle().enter();
            let handle = RelayHandle::spawn(&deps.device_id, sink.clone(), RelayPolicy::default());
            handle.set_serve_host_only(true);
            let (link_wanted, wanted) = watch::channel(false);
            let host = Arc::new(Mutex::new(deps.route.host.clone()));
            if let Some(direct) = deps.direct.clone() {
                chat_link::spawn(
                    handle.clone(),
                    direct,
                    chat_id.to_owned(),
                    host.clone(),
                    wanted,
                    cancel.clone(),
                );
            }
            RoomRelay {
                handle,
                host,
                relayed: Arc::new(AtomicBool::new(false)),
                defer_checkpoint: sink.defer_checkpoint.clone(),
                last_activity_ms: sink.last_activity_ms.clone(),
                link_wanted,
                deferred_since: Mutex::new(None),
                applied: Mutex::new(None),
            }
        });
        if let Some(relay) = &relay {
            // Before the first dial and pull, which read all of this.
            relay.apply_route(&deps.route);
        }
        // Without the layer, the hint is still what the join and pushes carry.
        let host = relay
            .as_ref()
            .map_or_else(|| Arc::new(Mutex::new(deps.route.host.clone())), |r| r.host.clone());

        // Own writes (commands, queue rows): durable outbox first, then the
        // live client (or the pre-join buffer).
        let publish_slot = slot.clone();
        let store = deps.store.clone();
        let publish_chat = chat_id.to_owned();
        let publish_persister = persister.clone();
        // The durable copy above is what counts: this one only saves a host that's
        // on a direct link the trip through the room, and whichever lands second
        // imports as nothing.
        let shortcut = relay
            .as_ref()
            .map(|r| (r.handle.clone(), r.relayed.clone(), r.last_activity_ms.clone()));
        let local_updates = doc
            .doc()
            .subscribe_local_update(Box::new(move |bytes: &Vec<u8>| {
                let batch_id = crate::new_id();
                let mut slot = lock(&publish_slot);
                if let Err(err) = store.enqueue_chat_update(&publish_chat, &batch_id, bytes) {
                    tracing::warn!(chat = %publish_chat, error = %err, "outbox enqueue failed");
                }
                match &slot.client {
                    Some(client) => client.enqueue_batch(batch_id, bytes.clone()),
                    None => slot.unsent.push((batch_id, bytes.clone())),
                }
                if let Some((relay, relayed, activity)) = &shortcut
                    && relayed.load(Ordering::Acquire)
                {
                    activity.store(crate::now_ms(), Ordering::Release);
                    relay.local_update(bytes.clone(), false);
                }
                publish_persister.dirty(false);
                true
            }));

        let room = Arc::new(Self {
            chat_id: chat_id.to_owned(),
            slot,
            status: Arc::new(RoomStatus::default()),
            persister,
            cancel,
            _local_updates: local_updates,
            relay,
        });
        room.spawn_join(sink, host, deps);
        room
    }

    /// The chat, its host or this screen changed.
    pub(crate) fn set_route(&self, route: &RelayRoute) {
        let Some(relay) = &self.relay else { return };
        relay.apply_route(route);
        let guard = lock(&self.slot);
        let Some(client) = guard.client.as_ref() else { return };
        let mut since = lock(&relay.deferred_since);
        if route.on_screen && client.checkpoint_deferred() {
            let at = *since.get_or_insert_with(std::time::Instant::now);
            // The host isn't coming, or hasn't come through: only the room can help.
            if !route.host_online || at.elapsed() >= DEFER_PATIENCE {
                client.read_deferred_checkpoint();
                *since = None;
            }
        } else {
            *since = None;
        }
    }

    /// When a relayed update last landed here or left here (ms), if ever.
    pub(crate) fn last_activity_ms(&self) -> i64 {
        self.relay
            .as_ref()
            .map_or(0, |relay| relay.last_activity_ms.load(Ordering::Acquire))
    }

    fn spawn_join(self: &Arc<Self>, sink: Arc<ViewerSink>, host: Arc<Mutex<String>>, deps: RoomDeps) {
        let url: Arc<dyn UrlProvider> = Arc::new(ChatUrl {
            bearer: deps.bearer.clone(),
            edge: deps.edge.clone(),
            chat_id: self.chat_id.clone(),
            device_id: deps.device_id.clone(),
            host: host.clone(),
        });
        let http = Arc::new(ChatHttp {
            bearer: deps.bearer.clone(),
            edge: deps.edge.clone(),
            chat_id: self.chat_id.clone(),
            device_id: deps.device_id.clone(),
            host,
            relay: deps.relay_layer,
        });
        let relay = self.relay.as_ref().map(|relay| relay.handle.clone());
        let weak = Arc::downgrade(self);
        let cancel = self.cancel.clone();
        let device_id = deps.device_id.clone();
        let on_status = deps.on_status.clone();
        let on_applied = deps.on_applied.clone();
        crate::runtime::shared().spawn(async move {
            let mut backoff = JOIN_RETRY_BASE;
            let client = loop {
                let Some(room) = weak.upgrade() else { return };
                let cursor = room.persister.cursor();
                let status = room.status.clone();
                drop(room);
                let attempt = ChatClient::connect_via_relay(
                    url.clone(),
                    sink.clone(),
                    http.clone(),
                    &device_id,
                    cursor,
                    http.clone(),
                    relay.clone(),
                );
                let result = tokio::select! {
                    _ = cancel.cancelled() => return,
                    result = tokio::time::timeout(Duration::from_secs(60), attempt) => result,
                };
                match result {
                    Ok(Ok(client)) => {
                        status.retry_at_ms.store(0, Ordering::Release);
                        break client;
                    }
                    Ok(Err(err)) => *lock(&status.last_failure) = Some(err.to_string()),
                    Err(_) => *lock(&status.last_failure) = Some("join timed out".into()),
                }
                status.retry_at_ms.store(
                    crate::now_ms() + backoff.as_millis() as i64,
                    Ordering::Release,
                );
                on_status();
                if !super::wait_backoff(&cancel, backoff).await {
                    return;
                }
                backoff = (backoff * 2).min(JOIN_RETRY_CAP);
            };
            let mut events = client.events();
            {
                let Some(room) = weak.upgrade() else { return };
                let mut slot = lock(&room.slot);
                for (batch_id, bytes) in std::mem::take(&mut slot.unsent) {
                    client.enqueue_batch(batch_id, bytes);
                }
                slot.client = Some(client);
            }
            on_status();
            loop {
                let event = tokio::select! {
                    _ = cancel.cancelled() => return,
                    event = events.recv() => event,
                };
                let Some(room) = weak.upgrade() else { return };
                match event {
                    Ok(ChatEvent::Connected) => {
                        room.status.connected.store(true, Ordering::Release);
                        *lock(&room.status.last_failure) = None;
                        on_status();
                    }
                    Ok(ChatEvent::CaughtUp { .. }) => {
                        room.status.connected.store(true, Ordering::Release);
                        room.status.caught_up.store(true, Ordering::Release);
                        on_applied();
                        on_status();
                    }
                    Ok(ChatEvent::Disconnected) => {
                        room.status.connected.store(false, Ordering::Release);
                        on_status();
                    }
                    Ok(ChatEvent::Applied) => on_applied(),
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => on_applied(),
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                }
            }
        });
    }

    /// Liveness hint (view attached / foreground): probe or redial now.
    pub(crate) fn kick(&self) {
        if let Some(client) = &lock(&self.slot).client {
            if client.stats().connected {
                client.probe();
            } else {
                client.redial();
            }
        }
    }

    /// Fresh socket (retry delivery).
    pub(crate) fn redial(&self) {
        if let Some(client) = &lock(&self.slot).client {
            client.redial();
        }
    }

    /// The room has answered this device: its rows and any checkpoint are in, or are
    /// deferred on purpose. An HTTPS pull counts, for a network with no sockets.
    pub(crate) fn answered(&self) -> bool {
        self.status.caught_up.load(Ordering::Acquire)
            || lock(&self.slot).client.as_ref().is_some_and(ChatClient::caught_up)
    }

    pub(crate) fn connected(&self) -> bool {
        lock(&self.slot)
            .client
            .as_ref()
            .is_some_and(|c| c.stats().connected)
    }

    /// Has unacknowledged own writes (never evict / keep the room).
    pub(crate) fn has_pending_pushes(&self) -> bool {
        let slot = lock(&self.slot);
        !slot.unsent.is_empty()
            || slot
                .client
                .as_ref()
                .is_some_and(|c| c.stats().pending_pushes > 0)
    }

    pub(crate) fn retry_at_ms(&self) -> Option<i64> {
        let at = self.status.retry_at_ms.load(Ordering::Acquire);
        (at > 0).then_some(at)
    }

    pub(crate) fn flush(&self) {
        self.persister.flush();
    }
}

impl Drop for Room {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(client) = lock(&self.slot).client.take() {
            crate::runtime::shared().spawn(client.shutdown());
        }
        self.persister.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sink(store: Arc<DocsStore>, doc: &Arc<SessionDoc>, chat: &str) -> ViewerSink {
        ViewerSink {
            doc: Arc::downgrade(doc),
            store: store.clone(),
            chat_id: chat.into(),
            persister: Persister::new(doc, store, chat, 0),
            on_applied: Arc::new(|| {}),
            relay: true,
            defer_checkpoint: Arc::new(AtomicBool::new(false)),
            last_activity_ms: Arc::new(AtomicI64::new(0)),
        }
    }

    fn update(text: &str) -> (loro::LoroDoc, Vec<u8>) {
        let doc = loro::LoroDoc::new();
        doc.get_text("t").insert(0, text).unwrap();
        doc.commit();
        let bytes = doc.export(loro::ExportMode::all_updates()).unwrap();
        (doc, bytes)
    }

    #[test]
    fn a_repeat_delivery_writes_nothing_and_a_save_retires_only_what_it_holds() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(DocsStore::open(dir.path()).unwrap());
        let doc = Arc::new(SessionDoc::from_doc(loro::LoroDoc::new()));
        let sink = sink(store.clone(), &doc, "chat");
        let (_, first) = update("first");
        assert_eq!(sink.relay_import(&first, false), RelayImport::Applied);
        assert_eq!(sink.relay_import(&first, false), RelayImport::Applied, "idempotent");
        assert_eq!(store.pending_chat_inbox("chat").unwrap().0.len(), 1, "one row, however often it comes");
        assert!(sink.relay_durable());

        sink.persister.flush();
        assert!(store.pending_chat_inbox("chat").unwrap().0.is_empty(), "the save holds it");

        // Saved already, and a later frame arrives: its row outlives the old save.
        let (_, second) = update("second");
        assert_eq!(sink.relay_import(&second, false), RelayImport::Applied);
        assert_eq!(store.pending_chat_inbox("chat").unwrap().0.len(), 1);
        let local = load_local(&store, "chat");
        assert!(local.had_content);
        assert!(local.journaled > 0, "the load knows what it replayed");
        let text = local.doc.doc().get_text("t").to_string();
        assert!(text.contains("first") && text.contains("second"), "{text}");
    }

    #[test]
    fn a_frame_that_parks_keeps_what_it_could_apply_and_says_it_parked() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(DocsStore::open(dir.path()).unwrap());
        let doc = Arc::new(SessionDoc::from_doc(loro::LoroDoc::new()));
        let sink = sink(store.clone(), &doc, "chat");
        let (remote, first) = update("first");
        let at = remote.oplog_vv();
        remote.get_text("t").insert(0, "second ").unwrap();
        remote.commit();
        let later = remote.export(loro::ExportMode::updates(&at)).unwrap();
        // Its history hasn't come: nothing moved, so nothing's kept, and it says so.
        assert_eq!(sink.relay_import(&later, false), RelayImport::Parked);
        assert!(store.pending_chat_inbox("chat").unwrap().0.is_empty());
        // It lands with its history, and both are kept.
        assert_eq!(sink.relay_import(&first, false), RelayImport::Applied);
        assert_eq!(store.pending_chat_inbox("chat").unwrap().0.len(), 1);
        assert_eq!(doc.doc().get_text("t").to_string(), "second first");
    }

    #[test]
    fn a_save_between_journal_rows_never_retires_a_row_it_does_not_hold() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(DocsStore::open(dir.path()).unwrap());
        let doc = Arc::new(SessionDoc::from_doc(loro::LoroDoc::new()));
        let sink = sink(store.clone(), &doc, "chat");
        let (_, first) = update("first");
        sink.relay_import(&first, false);
        // A row journaled after the save sampled what it covers survives it: this is
        // that order, spelled out. It's noted only after the flush below.
        let (_, second) = update("second");
        let row = store.journal_chat_inbox("chat", &second).unwrap();
        sink.persister.flush();
        let kept = store.pending_chat_inbox("chat").unwrap();
        assert_eq!(kept.0.len(), 1, "the later row stays");
        assert_eq!(kept.1, row);
        // And the next load reads it back.
        let text = load_local(&store, "chat").doc.doc().get_text("t").to_string();
        assert!(text.contains("first") && text.contains("second"), "{text}");
    }

    #[test]
    fn a_doc_with_nothing_in_it_never_defers_its_checkpoint() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(DocsStore::open(dir.path()).unwrap());
        let doc = Arc::new(SessionDoc::from_doc(loro::LoroDoc::new()));
        let sink = sink(store, &doc, "chat");
        sink.defer_checkpoint.store(true, Ordering::Release);
        assert!(!sink.relay_defer_checkpoint(), "with nothing to show, it reads");
        let (_, first) = update("first");
        sink.relay_import(&first, false);
        assert!(sink.relay_defer_checkpoint());
        sink.defer_checkpoint.store(false, Ordering::Release);
        assert!(!sink.relay_defer_checkpoint());
    }

    #[test]
    fn a_route_turns_every_relay_behaviour_on_or_off_together() {
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let _guard = runtime.enter();
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(DocsStore::open(dir.path()).unwrap());
        let doc = Arc::new(SessionDoc::from_doc(loro::LoroDoc::new()));
        let sink = Arc::new(sink(store, &doc, "chat"));
        let (link_wanted, wanted) = watch::channel(false);
        let handle = RelayHandle::spawn("ios", sink.clone(), RelayPolicy::default());
        let relay = RoomRelay {
            handle: handle.clone(),
            host: Arc::new(Mutex::new(String::new())),
            relayed: Arc::new(AtomicBool::new(false)),
            defer_checkpoint: sink.defer_checkpoint.clone(),
            last_activity_ms: sink.last_activity_ms.clone(),
            link_wanted,
            deferred_since: Mutex::new(None),
            applied: Mutex::new(None),
        };
        let everything = LinkWanted {
            relayed: true,
            on_screen: true,
            foreground: true,
            network: true,
            host_online: true,
            active: true,
        };
        let route = RelayRoute {
            generation: 3,
            host: "mac".into(),
            host_reads_queue: true,
            on_screen: true,
            host_online: true,
            link: everything,
        };
        relay.apply_route(&route);
        assert!(relay.relayed.load(Ordering::Acquire));
        assert_eq!(*lock(&relay.host), "mac");
        assert!(*wanted.borrow());
        assert!(relay.defer_checkpoint.load(Ordering::Acquire), "the host can answer");

        // On screen with the host gone, only the room can help.
        relay.apply_route(&RelayRoute { host_online: false, link: LinkWanted { host_online: false, ..everything }, ..route.clone() });
        assert!(!relay.defer_checkpoint.load(Ordering::Acquire));
        assert!(!*wanted.borrow(), "no link to a host that isn't there");

        // Off screen, nobody's waiting for it.
        relay.apply_route(&RelayRoute { host_online: false, on_screen: false, ..route.clone() });
        assert!(relay.defer_checkpoint.load(Ordering::Acquire));

        // Stored: nothing relay-shaped is left.
        relay.apply_route(&RelayRoute { generation: 2, link: LinkWanted { relayed: false, ..everything }, ..route });
        assert!(!relay.relayed.load(Ordering::Acquire));
        assert!(!relay.defer_checkpoint.load(Ordering::Acquire));
        assert!(!*wanted.borrow());
    }
}
