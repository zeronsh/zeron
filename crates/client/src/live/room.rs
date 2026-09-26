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
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use zeron_doc::SessionDoc;
use zeron_sync::chat_client::{ChatTransport, RowImportOutcome};
use zeron_sync::{
    ChatClient, ChatDocSink, ChatEvent, CheckpointFetcher, DocsStore, SyncError, UrlProvider,
};

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
}

/// Restore a chat doc from disk: the chat2 snapshot (epoch ≥ 2 only — a
/// retired s2 lineage is never imported) plus any unacknowledged own writes.
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
    LocalDoc {
        doc: SessionDoc::from_doc(raw),
        cursor,
        had_content,
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
        })
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
            Ok(()) => self.saved.store(generation, Ordering::Release),
            Err(err) => {
                tracing::warn!(chat = %self.chat_id, error = %err, "chat snapshot save failed")
            }
        }
    }
}

/// Remote bytes → the session doc; outbox hooks → the docs store.
pub(crate) struct ViewerSink {
    pub(crate) doc: Weak<SessionDoc>,
    pub(crate) store: Arc<DocsStore>,
    pub(crate) chat_id: String,
    pub(crate) persister: Arc<Persister>,
    /// Coalesced "the doc changed remotely" wake (republish).
    pub(crate) on_applied: Arc<dyn Fn() + Send + Sync>,
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
}

/// Fresh `?token=` per dial.
pub(crate) struct ChatUrl {
    pub(crate) bearer: Bearer,
    pub(crate) edge: String,
    pub(crate) chat_id: String,
    pub(crate) device_id: String,
}

impl UrlProvider for ChatUrl {
    fn url(&self) -> BoxFuture<'static, Result<String, SyncError>> {
        let bearer = self.bearer.clone();
        let (edge, chat, device) = (
            self.edge.clone(),
            self.chat_id.clone(),
            self.device_id.clone(),
        );
        Box::pin(async move {
            let token = bearer.get().await?;
            Ok(urls::chat_ws(&edge, &chat, &token, &device))
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

impl ChatTransport for ChatHttp {
    fn fetch_rows(&self, after: u64) -> BoxFuture<'static, Result<Vec<u8>, SyncError>> {
        let bearer = self.bearer.clone();
        let url = urls::chat_rows(&self.edge, &self.chat_id, after, &self.device_id);
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

    fn push(
        &self,
        batch_id: String,
        bytes: Vec<u8>,
    ) -> BoxFuture<'static, Result<String, SyncError>> {
        let bearer = self.bearer.clone();
        let url = urls::chat_push(&self.edge, &self.chat_id, &batch_id, &self.device_id);
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

/// One live chat room. Dropping it stops the client and flushes the doc.
pub(crate) struct Room {
    pub(crate) chat_id: String,
    slot: Arc<Mutex<Slot>>,
    pub(crate) status: Arc<RoomStatus>,
    pub(crate) persister: Arc<Persister>,
    cancel: CancellationToken,
    _local_updates: loro::Subscription,
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
        let cancel = CancellationToken::new();
        persister.start(cancel.clone());
        let slot = Arc::new(Mutex::new(Slot::default()));

        // Own writes (commands, queue rows): durable outbox first, then the
        // live client (or the pre-join buffer).
        let publish_slot = slot.clone();
        let store = deps.store.clone();
        let publish_chat = chat_id.to_owned();
        let publish_persister = persister.clone();
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
        });
        room.spawn_join(doc, deps);
        room
    }

    fn spawn_join(self: &Arc<Self>, doc: &Arc<SessionDoc>, deps: RoomDeps) {
        let sink = Arc::new(ViewerSink {
            doc: Arc::downgrade(doc),
            store: deps.store.clone(),
            chat_id: self.chat_id.clone(),
            persister: self.persister.clone(),
            on_applied: deps.on_applied.clone(),
        });
        let url: Arc<dyn UrlProvider> = Arc::new(ChatUrl {
            bearer: deps.bearer.clone(),
            edge: deps.edge.clone(),
            chat_id: self.chat_id.clone(),
            device_id: deps.device_id.clone(),
        });
        let http = Arc::new(ChatHttp {
            bearer: deps.bearer.clone(),
            edge: deps.edge.clone(),
            chat_id: self.chat_id.clone(),
            device_id: deps.device_id.clone(),
        });
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
                let attempt = ChatClient::connect_via_transport(
                    url.clone(),
                    sink.clone(),
                    http.clone(),
                    &device_id,
                    cursor,
                    http.clone(),
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
