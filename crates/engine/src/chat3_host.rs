//! chat3 host wiring (docs/chat3-sync.md): the engine side of the transcript
//! plane. A gen-3 handle's transcript doc is local-only; this module moves it
//! to and from the room.
//!
//! - Host: the [`Publisher`] turns transcript commits into stored rows,
//!   queued in the durable outbox and uploaded in batches, and live frames
//!   sent on the chat's socket. Full tool outputs queue beside the rows.
//! - Reader: pages the newest history in, follows the change feed, applies
//!   relayed rows and live frames, and pages older history on request.
//!
//! Background tasks hold the doc WEAKLY and upgrade only around synchronous
//! applies: a strong ref across an await reads as a live writer to the LRU's
//! `pinned()` and would make every gen-3 chat immune to eviction.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};

use futures::future::BoxFuture;
use tokio::sync::Notify;
use zeron_doc::SessionDoc;
use zeron_doc::transcript_plane::{Item, ItemKey, Publication, Publisher, Reader};
pub use zeron_sync::chat3::PAGE_LIMIT;
use zeron_sync::chat3::{self, ReaderPlane, TranscriptTransport};
use zeron_sync::{DocsStore, OutboxItem, SyncError};

use crate::doc_host::EdgeConfig;
use crate::http_error::describe_http_error;

/// One `POST /items` body (the room accepts up to 8 MiB).
const UPLOAD_BATCH_BYTES: usize = 1024 * 1024;
/// Errors the room returns for a row that can never be accepted. Retrying
/// them would replay forever (the chat2 wedge class); they are dropped.
const PERMANENT_ROW_ERRORS: &[&str] = &["too_large", "empty", "bad_ord", "bad_idx", "bad_entry", "bad_frame"];

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub(crate) fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Per-handle chat3 state: the host's publisher and the reader plane.
pub struct Chat3 {
    chat_id: String,
    publisher: Mutex<Publisher>,
    plane: ReaderPlane,
    /// The transcript doc changed since the last publisher drain.
    pub(crate) changed: Notify,
    /// Rows or outputs were queued for upload.
    pub(crate) upload: Notify,
    host: AtomicBool,
    /// Rows this handle uploaded and the room acknowledged (diagnostics).
    uploaded: AtomicU64,
    /// A host is applying rows an earlier host wrote: the publisher waits
    /// until they are adopted, or it would re-upload them.
    adopting: AtomicBool,
}

impl Chat3 {
    pub fn new(
        chat_id: impl Into<String>,
        known: HashMap<(u64, i64), u64>,
        host: bool,
        import_lock: Arc<Mutex<()>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            chat_id: chat_id.into(),
            publisher: Mutex::new(Publisher::new(known)),
            plane: ReaderPlane::new(import_lock),
            changed: Notify::new(),
            upload: Notify::new(),
            host: AtomicBool::new(host),
            uploaded: AtomicU64::new(0),
            adopting: AtomicBool::new(false),
        })
    }

    /// A reader saw a rev gap on the socket: the role task runs the feed.
    pub fn catch_up_requested(&self) -> tokio::sync::futures::Notified<'_> {
        self.plane.catch_up.notified()
    }

    pub fn chat_id(&self) -> &str {
        &self.chat_id
    }

    pub fn is_host(&self) -> bool {
        self.host.load(Ordering::Acquire)
    }

    /// The chat moved to or from this device. A new host re-examines its
    /// whole doc so nothing it now owns goes unpublished.
    pub fn set_host(&self, host: bool) {
        if self.host.swap(host, Ordering::AcqRel) != host && host {
            lock(&self.publisher).mark_all();
            self.changed.notify_one();
        }
    }

    pub fn uploaded(&self) -> u64 {
        self.uploaded.load(Ordering::Relaxed)
    }

    /// Transcript-doc change event (the handle's subscription). Readers
    /// don't track it: their publisher never drains, and a device that
    /// becomes host rescans everything (`set_host`).
    pub fn observe(&self, event: &loro::event::DiffEvent<'_>) {
        if !self.is_host() {
            return;
        }
        lock(&self.publisher).observe(event);
        self.changed.notify_one();
    }

    /// Host: drain the publisher. Stored rows go to the outbox (one local
    /// transaction with their published hashes), live frames to `send_live`.
    pub fn publish(&self, doc: &SessionDoc, store: &DocsStore, send_live: &dyn Fn(Vec<u8>)) {
        if !self.is_host() || self.adopting.load(Ordering::Acquire) {
            return;
        }
        let publications = {
            let mut publisher = lock(&self.publisher);
            if !publisher.is_dirty() {
                return;
            }
            publisher.collect(doc, now_ms())
        };
        self.route(publications, store, send_live);
    }

    fn route(&self, publications: Vec<Publication>, store: &DocsStore, send_live: &dyn Fn(Vec<u8>)) {
        let mut rows = Vec::new();
        for publication in publications {
            match publication {
                Publication::Store(item) => rows.push(outbox_item(&item)),
                Publication::Live(live) => send_live(chat3::encode_live(&live)),
            }
        }
        if rows.is_empty() {
            return;
        }
        if let Err(err) = store.chat3_publish(&self.chat_id, &rows) {
            tracing::error!(chat = %self.chat_id, error = %err, "chat3: outbox write failed");
            // The rows stay unknown to the store; the next open's full scan
            // republishes them.
            return;
        }
        self.upload.notify_one();
    }

    /// Reader: a transcript frame relayed on the chat's socket. Returns true
    /// when the doc changed.
    pub fn apply_frame(&self, doc: &SessionDoc, kind: u8, header: &serde_json::Value, payload: &[u8]) -> bool {
        if self.is_host() {
            // The host's local doc is the source of truth; the room relays
            // nothing it needs (its own rows are never echoed back).
            return false;
        }
        self.plane.apply_frame(doc, kind, header, payload)
    }

    /// Reader catch-up: the newest page, then the change feed.
    pub async fn catch_up(&self, doc: &Weak<SessionDoc>, transport: &dyn TranscriptTransport) -> Result<(), SyncError> {
        self.plane.catch_up(doc, transport).await
    }

    /// Reader: the page before the oldest row held. Returns whether older
    /// rows remain.
    pub async fn load_earlier(&self, doc: &Weak<SessionDoc>, transport: &dyn TranscriptTransport) -> Result<bool, SyncError> {
        self.plane.load_earlier(doc, transport).await
    }

    /// Host: pull rows an earlier host wrote, adopt them as stored, and
    /// queue anything this host wrote that never reached the room.
    pub async fn host_catch_up(
        &self,
        doc: &Weak<SessionDoc>,
        store: &DocsStore,
        transport: &dyn TranscriptTransport,
    ) -> Result<(), SyncError> {
        struct Adopting<'a>(&'a Chat3);
        impl Drop for Adopting<'_> {
            fn drop(&mut self) {
                self.0.adopting.store(false, Ordering::Release);
                // Anything the publisher skipped meanwhile runs now.
                self.0.changed.notify_one();
            }
        }
        self.adopting.store(true, Ordering::Release);
        let _adopting = Adopting(self);
        let pulled = {
            let publisher = &self.publisher;
            self.plane.feed(doc, transport, |key| !lock(publisher).knows(key)).await?
        };
        let Some(doc) = doc.upgrade() else { return Ok(()) };
        let (upload, adopted) = lock(&self.publisher).adopt(&doc, &pulled, now_ms());
        let adopted: Vec<(f64, i64, u64)> = adopted
            .into_iter()
            .map(|((ord, idx), hash)| (f64::from_bits(ord), idx, hash))
            .collect();
        if let Err(err) = store.chat3_mark_published(&self.chat_id, &adopted) {
            tracing::warn!(chat = %self.chat_id, error = %err, "chat3: adopt record failed");
        }
        if !pulled.is_empty() {
            tracing::info!(chat = %self.chat_id, rows = pulled.len(), "chat3: host adopted rows from the room");
        }
        self.route(upload, store, &|_| {});
        Ok(())
    }

    /// Host: upload queued rows, then queued outputs, until both are empty or
    /// the network fails. Returns false on a transient failure (back off).
    pub async fn upload_once(
        &self,
        doc: &Weak<SessionDoc>,
        store: &DocsStore,
        transport: &dyn TranscriptTransport,
    ) -> bool {
        loop {
            let batch = match store.chat3_pending(&self.chat_id, UPLOAD_BATCH_BYTES) {
                Ok(batch) => batch,
                Err(err) => {
                    tracing::error!(chat = %self.chat_id, error = %err, "chat3: outbox read failed");
                    return false;
                }
            };
            if batch.is_empty() {
                break;
            }
            let items: Vec<Item> = batch
                .iter()
                .map(|row| Item {
                    key: ItemKey { ord: row.ord, idx: row.idx },
                    entry: row.entry.clone(),
                    json: row.bytes.clone(),
                })
                .collect();
            let ack = match transport.post_items(chat3::encode_item_puts(&items)).await {
                Ok(text) => match serde_json::from_str::<chat3::PostAck>(&text) {
                    Ok(ack) => ack,
                    Err(err) => {
                        tracing::warn!(chat = %self.chat_id, error = %err, "chat3: unreadable items ack");
                        return false;
                    }
                },
                Err(err) => {
                    tracing::debug!(chat = %self.chat_id, error = %err, "chat3: items upload failed");
                    return false;
                }
            };
            let mut acked = Vec::new();
            let mut changed = 0u64;
            for (row, result) in batch.iter().zip(ack.results.iter()) {
                match &result.error {
                    None => {
                        acked.push((row.ord, row.idx, row.hash));
                        if result.changed.unwrap_or(false) {
                            changed += 1;
                        }
                    }
                    Some(code) if PERMANENT_ROW_ERRORS.contains(&code.as_str()) => {
                        tracing::error!(chat = %self.chat_id, ord = row.ord, idx = row.idx, code,
                            "chat3: row permanently refused; dropped from the outbox");
                        acked.push((row.ord, row.idx, row.hash));
                    }
                    Some(_) => {}
                }
            }
            if let Err(err) = store.chat3_ack(&self.chat_id, &acked) {
                tracing::error!(chat = %self.chat_id, error = %err, "chat3: outbox ack failed");
                return false;
            }
            self.uploaded.fetch_add(acked.len() as u64, Ordering::Relaxed);
            // The host's feed cursor may skip its own rows only when nothing
            // else landed in between: then the ack's head is exactly ours.
            if let Some(doc) = doc.upgrade() {
                let head = Reader::cursor(&doc).head_rev;
                if changed > 0 && ack.head_rev == head + changed {
                    let _ = Reader::note_head_rev(&doc, ack.head_rev);
                }
            }
        }
        loop {
            let next = match store.chat3_next_output(&self.chat_id) {
                Ok(next) => next,
                Err(err) => {
                    tracing::error!(chat = %self.chat_id, error = %err, "chat3: output queue read failed");
                    return false;
                }
            };
            let Some((part, bytes)) = next else { break };
            let len = bytes.len();
            if let Err(err) = transport.put_output(part.clone(), bytes).await {
                tracing::debug!(chat = %self.chat_id, error = %err, "chat3: output upload failed");
                return false;
            }
            if let Err(err) = store.chat3_ack_output(&self.chat_id, &part, len) {
                tracing::error!(chat = %self.chat_id, error = %err, "chat3: output ack failed");
                return false;
            }
        }
        true
    }
}

fn outbox_item(item: &Item) -> OutboxItem {
    OutboxItem {
        ord: item.key.ord,
        idx: item.key.idx,
        entry: item.entry.clone(),
        bytes: item.json.clone(),
        hash: item.hash(),
    }
}

/// Full tool output, capped like the harness cap: head and tail kept.
pub const TOOL_OUTPUT_CAP: usize = 256 * 1024;
/// The live preview a running tool's part carries (local doc only).
const LIVE_TAIL_LINES: usize = 12;
const LIVE_TAIL_BYTES: usize = 2048;

/// Accumulates one tool's streamed output (bounded) for its live tail and
/// the full upload.
#[derive(Default)]
pub struct OutputBuffer {
    head: String,
    tail: String,
    dropped: usize,
}

impl OutputBuffer {
    pub fn push(&mut self, text: &str) {
        let half = TOOL_OUTPUT_CAP / 2;
        if self.head.len() < half {
            let room = half - self.head.len();
            let cut = floor_char_boundary(text, room.min(text.len()));
            self.head.push_str(&text[..cut]);
            self.push_tail(&text[cut..]);
        } else {
            self.push_tail(text);
        }
    }

    fn push_tail(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.tail.push_str(text);
        let half = TOOL_OUTPUT_CAP / 2;
        if self.tail.len() > half {
            let excess = self.tail.len() - half;
            let cut = ceil_char_boundary(&self.tail, excess);
            self.dropped += cut;
            self.tail.drain(..cut);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.head.is_empty() && self.tail.is_empty()
    }

    /// The whole retained output, with a marker where the middle was cut.
    pub fn text(&self) -> String {
        if self.dropped == 0 {
            return format!("{}{}", self.head, self.tail);
        }
        format!("{}\n… [{} bytes truncated] …\n{}", self.head, self.dropped, self.tail)
    }

    /// The last few lines, for a running tool's live part.
    pub fn live_tail(&self) -> String {
        live_tail(&self.text())
    }
}

/// gen 3: fold a tool's output into its part — the live tail while it runs,
/// then the one-line summary, byte count and `chat3:` ref once it resolves —
/// and return `(part id, full output)` to store in the room. Runs after the
/// ordinary fold, which keeps outputs out of every doc.
pub fn fold_tool_output(
    parts: &mut [zeron_doc::MessagePart],
    event: &zeron_proto::AgentEvent,
    buffers: &mut HashMap<String, OutputBuffer>,
    chat_id: &str,
) -> Option<(String, String)> {
    use zeron_doc::MessagePart;
    use zeron_proto::AgentEvent;
    match event {
        AgentEvent::ToolOutputDelta { id, text } => {
            let buffer = buffers.entry(id.clone()).or_default();
            buffer.push(text);
            let tail = buffer.live_tail();
            for part in parts.iter_mut() {
                if let MessagePart::Tool { id: pid, resolved: false, output, .. } = part
                    && pid == id
                {
                    *output = (!tail.is_empty()).then_some(tail.clone());
                }
            }
            None
        }
        AgentEvent::ToolResult { id, output, .. } => {
            let streamed = buffers.remove(id);
            let full = match output.as_deref().filter(|o| !o.trim().is_empty()) {
                Some(text) => cap_output(text),
                None => streamed.filter(|b| !b.is_empty()).map(|b| b.text())?,
            };
            let reference = chat3::valid_part_id(id).then(|| chat3::output_ref(chat_id, id));
            for part in parts.iter_mut() {
                if let MessagePart::Tool {
                    id: pid,
                    output: summary,
                    output_bytes,
                    output_ref,
                    ..
                } = part
                    && pid == id
                {
                    *summary = zeron_doc::summarize_tool_output(&full);
                    *output_bytes = Some(full.len() as u64);
                    *output_ref = reference.clone();
                }
            }
            reference.map(|_| (id.clone(), full))
        }
        _ => None,
    }
}

/// Cap a complete output (claude/ACP results arrive whole).
pub fn cap_output(text: &str) -> String {
    let mut buffer = OutputBuffer::default();
    buffer.push(text);
    buffer.text()
}

/// The last lines of `text`, bounded in bytes.
pub fn live_tail(text: &str) -> String {
    let trimmed = text.trim_end_matches('\n');
    let mut start = trimmed.len();
    for (lines, (ix, _)) in trimmed.rmatch_indices('\n').enumerate() {
        if lines + 1 >= LIVE_TAIL_LINES || trimmed.len() - ix > LIVE_TAIL_BYTES {
            break;
        }
        start = ix + 1;
    }
    if !trimmed.contains('\n') || start == trimmed.len() {
        start = 0;
    }
    let tail = &trimmed[start..];
    if tail.len() > LIVE_TAIL_BYTES {
        let cut = ceil_char_boundary(tail, tail.len() - LIVE_TAIL_BYTES);
        return tail[cut..].to_owned();
    }
    tail.to_owned()
}

fn floor_char_boundary(s: &str, mut i: usize) -> usize {
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn ceil_char_boundary(s: &str, mut i: usize) -> usize {
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

// ── edge transport ──────────────────────────────────────────────────────────

/// `GET/POST /chat3/{id}/items` and `GET/PUT /chat3/{id}/outputs/{part}`
/// on the same bearer auth as the chat2 transport.
pub struct EdgeTranscriptTransport {
    priority: zeron_sync::budget::Priority,
    http: reqwest::Client,
    edge: EdgeConfig,
    chat_id: String,
    device_id: String,
}

impl EdgeTranscriptTransport {
    pub fn new(
        http: reqwest::Client,
        edge: EdgeConfig,
        chat_id: impl Into<String>,
        device_id: impl Into<String>,
    ) -> Self {
        Self {
            priority: zeron_sync::budget::Priority::Interactive,
            http,
            edge,
            chat_id: chat_id.into(),
            device_id: device_id.into(),
        }
    }

    pub fn with_priority(mut self, priority: zeron_sync::budget::Priority) -> Self {
        self.priority = priority;
        self
    }

    fn url(&self, tail: &str) -> String {
        format!("{}/chat3/{}/{tail}", self.edge.url.trim_end_matches('/'), self.chat_id)
    }
}

fn transport_error(err: reqwest::Error) -> SyncError {
    SyncError::WebSocket(describe_http_error(err))
}

impl TranscriptTransport for EdgeTranscriptTransport {
    fn post_items(&self, body: Vec<u8>) -> BoxFuture<'static, Result<String, SyncError>> {
        let (priority, http, edge) = (self.priority, self.http.clone(), self.edge.clone());
        let url = self.url("items");
        let device = self.device_id.clone();
        Box::pin(async move {
            let _permit = zeron_sync::budget::shared().http(priority).await?;
            let bearer = edge.bearer().await.map_err(SyncError::from)?;
            let res = http
                .post(&url)
                .query(&[("device", device)])
                .bearer_auth(&bearer)
                .body(body)
                .send()
                .await
                .map_err(transport_error)?;
            if !res.status().is_success() {
                return Err(SyncError::Protocol(format!("chat3 items http {}", res.status())));
            }
            res.text().await.map_err(transport_error)
        })
    }

    fn fetch_items(&self, query: String) -> BoxFuture<'static, Result<Vec<u8>, SyncError>> {
        let (priority, http, edge) = (self.priority, self.http.clone(), self.edge.clone());
        let url = format!("{}?{query}", self.url("items"));
        Box::pin(async move {
            let _permit = zeron_sync::budget::shared().http(priority).await?;
            let bearer = edge.bearer().await.map_err(SyncError::from)?;
            let res = http.get(&url).bearer_auth(&bearer).send().await.map_err(transport_error)?;
            if !res.status().is_success() {
                return Err(SyncError::Protocol(format!("chat3 items http {}", res.status())));
            }
            Ok(res.bytes().await.map_err(transport_error)?.to_vec())
        })
    }

    fn put_output(&self, part: String, bytes: Vec<u8>) -> BoxFuture<'static, Result<(), SyncError>> {
        let (http, edge) = (self.http.clone(), self.edge.clone());
        let url = self.url(&format!("outputs/{part}"));
        Box::pin(async move {
            let _permit = zeron_sync::budget::shared()
                .http(zeron_sync::budget::Priority::Background)
                .await?;
            let bearer = edge.bearer().await.map_err(SyncError::from)?;
            let res = http
                .put(&url)
                .bearer_auth(&bearer)
                .header("content-type", "text/plain; charset=utf-8")
                .body(bytes)
                .send()
                .await
                .map_err(transport_error)?;
            match res.status().as_u16() {
                200..=299 => Ok(()),
                // Can never be accepted: dropped rather than retried forever.
                400 | 413 => {
                    tracing::warn!(status = %res.status(), "chat3: output refused; dropped");
                    Ok(())
                }
                _ => Err(SyncError::Protocol(format!("chat3 output http {}", res.status()))),
            }
        })
    }

    fn get_output(&self, part: String) -> BoxFuture<'static, Result<Option<Vec<u8>>, SyncError>> {
        let (priority, http, edge) = (self.priority, self.http.clone(), self.edge.clone());
        let url = self.url(&format!("outputs/{part}"));
        Box::pin(async move {
            let _permit = zeron_sync::budget::shared().http(priority).await?;
            let bearer = edge.bearer().await.map_err(SyncError::from)?;
            let res = http.get(&url).bearer_auth(&bearer).send().await.map_err(transport_error)?;
            match res.status().as_u16() {
                404 => Ok(None),
                200..=299 => Ok(Some(res.bytes().await.map_err(transport_error)?.to_vec())),
                _ => Err(SyncError::Protocol(format!("chat3 output http {}", res.status()))),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeron_doc::transcript_plane::ReceivedItem;
    use zeron_doc::{MessagePart, MessageRole, MessageStatus, SegmentWriter, SessionMessageEntry};
    use zeron_sync::chat_frames::{self as wire, frame_type};

    /// The room's transcript plane in memory, with the edge's rules: upsert
    /// by key with a rev (identical rewrites skipped), newest-first pages
    /// that prepend a cut entry's header, the rev feed, outputs.
    #[derive(Default)]
    struct MemRoom {
        rows: Mutex<Vec<ReceivedItem>>,
        outputs: Mutex<HashMap<String, Vec<u8>>>,
        posts: AtomicU64,
        offline: AtomicBool,
    }

    impl MemRoom {
        fn head(&self) -> u64 {
            lock(&self.rows).iter().map(|r| r.rev).max().unwrap_or(0)
        }

        fn body(&self, items: Vec<ReceivedItem>, more: bool, head: u64) -> Vec<u8> {
            let mut frames: Vec<Vec<u8>> = items
                .iter()
                .map(|i| {
                    wire::encode(
                        frame_type::ITEM,
                        &serde_json::json!({"ord": i.key.ord, "idx": i.key.idx, "entry": i.entry, "rev": i.rev}),
                        &i.json,
                    )
                })
                .collect();
            frames.push(wire::encode(frame_type::ITEMS_DONE, &serde_json::json!({"headRev": head, "more": more}), &[]));
            chat3::length_prefixed(&frames)
        }
    }

    fn cmp(a: &ItemKey, b: &ItemKey) -> std::cmp::Ordering {
        a.ord.total_cmp(&b.ord).then(a.idx.cmp(&b.idx))
    }

    /// Shared handle (the orphan rule forbids implementing the trait on Arc).
    #[derive(Clone, Default)]
    struct Mem(Arc<MemRoom>);

    impl std::ops::Deref for Mem {
        type Target = MemRoom;
        fn deref(&self) -> &MemRoom {
            &self.0
        }
    }

    impl TranscriptTransport for Mem {
        fn post_items(&self, body: Vec<u8>) -> BoxFuture<'static, Result<String, SyncError>> {
            let room = self.clone();
            Box::pin(async move {
                if room.offline.load(Ordering::Relaxed) {
                    return Err(SyncError::Protocol("offline".into()));
                }
                room.posts.fetch_add(1, Ordering::Relaxed);
                let mut rows = lock(&room.rows);
                let mut results = Vec::new();
                for raw in chat3::split_length_prefixed(&body) {
                    let frame = wire::decode(raw).unwrap();
                    let key = ItemKey {
                        ord: frame.header["ord"].as_f64().unwrap(),
                        idx: frame.header["idx"].as_i64().unwrap(),
                    };
                    let entry = frame.header["entry"].as_str().unwrap().to_owned();
                    if let Some(row) = rows.iter().find(|r| r.key.bits() == key.bits() && r.json == frame.payload) {
                        results.push(serde_json::json!({"rev": row.rev, "changed": false}));
                        continue;
                    }
                    let rev = rows.iter().map(|r| r.rev).max().unwrap_or(0) + 1;
                    rows.retain(|r| r.key.bits() != key.bits());
                    rows.push(ReceivedItem { key, entry, rev, json: frame.payload });
                    results.push(serde_json::json!({"rev": rev, "changed": true}));
                }
                let head = rows.iter().map(|r| r.rev).max().unwrap_or(0);
                Ok(serde_json::json!({"headRev": head, "results": results}).to_string())
            })
        }

        fn fetch_items(&self, query: String) -> BoxFuture<'static, Result<Vec<u8>, SyncError>> {
            let room = self.clone();
            Box::pin(async move {
                if room.offline.load(Ordering::Relaxed) {
                    return Err(SyncError::Protocol("offline".into()));
                }
                let head = room.head();
                let params: HashMap<&str, &str> = query.split('&').filter_map(|kv| kv.split_once('=')).collect();
                let rows = lock(&room.rows).clone();
                if let Some(since) = params.get("since") {
                    let since: u64 = since.parse().unwrap();
                    let mut feed: Vec<_> = rows.into_iter().filter(|r| r.rev > since).collect();
                    feed.sort_by_key(|r| r.rev);
                    return Ok(room.body(feed, false, head));
                }
                let limit: usize = params.get("limit").unwrap().parse().unwrap();
                let before = params.get("before").map(|b| {
                    let (ord, idx) = b.split_once(':').unwrap();
                    ItemKey { ord: ord.parse().unwrap(), idx: idx.parse().unwrap() }
                });
                let mut older: Vec<_> = rows
                    .iter()
                    .filter(|r| before.is_none_or(|b| cmp(&r.key, &b).is_lt()))
                    .cloned()
                    .collect();
                older.sort_by(|a, b| cmp(&a.key, &b.key));
                let start = older.len().saturating_sub(limit);
                let mut page: Vec<_> = older[start..].to_vec();
                if let Some(first) = page.first().cloned()
                    && first.key.idx >= 0
                    && let Some(header) = rows.iter().find(|r| r.key.ord == first.key.ord && r.key.idx == -1)
                {
                    page.insert(0, header.clone());
                }
                Ok(room.body(page, start > 0, head))
            })
        }

        fn put_output(&self, part: String, bytes: Vec<u8>) -> BoxFuture<'static, Result<(), SyncError>> {
            let room = self.clone();
            Box::pin(async move {
                if room.offline.load(Ordering::Relaxed) {
                    return Err(SyncError::Protocol("offline".into()));
                }
                lock(&room.outputs).insert(part, bytes);
                Ok(())
            })
        }

        fn get_output(&self, part: String) -> BoxFuture<'static, Result<Option<Vec<u8>>, SyncError>> {
            let room = self.clone();
            Box::pin(async move { Ok(lock(&room.outputs).get(&part).cloned()) })
        }
    }

    /// A gen-3 handle in miniature: split doc, its Chat3, the transcript
    /// subscription feeding the publisher.
    struct Side {
        doc: Arc<SessionDoc>,
        chat3: Arc<Chat3>,
        store: DocsStore,
        _dir: tempfile::TempDir,
        _sub: loro::Subscription,
    }

    impl Side {
        fn new(host: bool) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let store = DocsStore::open(dir.path()).unwrap();
            Self::with_store(host, store, dir)
        }

        fn with_store(host: bool, store: DocsStore, dir: tempfile::TempDir) -> Self {
            let doc = Arc::new(SessionDoc::init_split("chat").unwrap());
            let known = store.chat3_published("chat").unwrap();
            let chat3 = Chat3::new("chat", known, host, Arc::new(Mutex::new(())));
            let feed = chat3.clone();
            let sub = doc
                .transcript_doc()
                .subscribe_root(Arc::new(move |event| feed.observe(&event)));
            Self { doc, chat3, store, _dir: dir, _sub: sub }
        }

        fn publish(&self) -> Vec<Vec<u8>> {
            let lives = Mutex::new(Vec::new());
            self.chat3.publish(&self.doc, &self.store, &|frame| lock(&lives).push(frame));
            lives.into_inner().unwrap()
        }

        fn weak(&self) -> Weak<SessionDoc> {
            Arc::downgrade(&self.doc)
        }
    }

    fn user(id: &str, text: &str) -> SessionMessageEntry {
        SessionMessageEntry {
            id: id.into(),
            role: MessageRole::User,
            parts: vec![MessagePart::Text { id: "t0".into(), text: text.into() }],
            created_at: 1,
            device_id: "host".into(),
            status: Some(MessageStatus::Complete),
            continuation_of: None,
            duration_ms: None,
        }
    }

    fn text(id: &str, body: &str) -> MessagePart {
        MessagePart::Text { id: id.into(), text: body.into() }
    }

    #[tokio::test]
    async fn host_uploads_reader_pages_in_and_follows_live() {
        let room = Mem::default();
        let host = Side::new(true);
        let reader = Side::new(false);

        // History: 30 complete turns.
        for i in 0..30 {
            host.doc.push_message(&user(&format!("u{i}"), &format!("question {i}"))).unwrap();
            let writer = SegmentWriter::begin(&host.doc, &format!("a{i}"), "host", 1).unwrap();
            let parts: Vec<MessagePart> = (0..8).map(|p| text(&format!("t{p}"), &format!("answer {i}.{p}"))).collect();
            writer.finish(&parts, MessageStatus::Complete).unwrap();
        }
        host.publish();
        assert!(host.chat3.upload_once(&host.weak(), &host.store, &room).await);
        assert_eq!(host.store.chat3_outbox_len("chat").unwrap(), 0);
        let stored = lock(&room.rows).len();
        assert_eq!(stored, 30 * (2 + 1 + 8));

        // A fresh reader paints from one page, not the whole chat.
        reader.chat3.catch_up(&reader.weak(), &room).await.unwrap();
        let shown = reader.doc.read_entries().unwrap();
        assert_eq!(shown.last().unwrap().id, "a29");
        assert!(Reader::cursor(&reader.doc).more_before);
        let shown_parts: usize = shown.iter().map(|e| e.parts.len()).sum();
        assert!(shown_parts < PAGE_LIMIT, "{shown_parts} parts on first paint");

        // A streaming turn reaches the reader through live frames only.
        host.doc.push_message(&user("u30", "one more")).unwrap();
        let mut writer = SegmentWriter::begin(&host.doc, "a30", "host", 1).unwrap();
        let mut body = String::new();
        let posts_before = room.posts.load(Ordering::Relaxed);
        for i in 0..50 {
            body.push_str(&format!("tok{i} "));
            writer.sync(&[text("t0", &body)]).unwrap();
            for frame in host.publish() {
                let frame = wire::decode(&frame).unwrap();
                reader.chat3.apply_frame(&reader.doc, frame.kind, &frame.header, &frame.payload);
            }
            // Stored rows (the new entries' headers and the user message)
            // relay to the reader as `item` frames, like the room does.
            let before = room.head();
            assert!(host.chat3.upload_once(&host.weak(), &host.store, &room).await);
            for row in lock(&room.rows).iter().filter(|r| r.rev > before) {
                let header = serde_json::json!({"ord": row.key.ord, "idx": row.key.idx, "entry": row.entry, "rev": row.rev});
                reader.chat3.apply_frame(&reader.doc, frame_type::ITEM, &header, &row.json);
            }
        }
        let live_entries = reader.doc.read_entries().unwrap();
        let streamed = live_entries.iter().find(|e| e.id == "a30").expect("streaming entry reached the reader");
        assert_eq!(streamed.parts, vec![text("t0", &body)]);
        // 50 ticks of streaming cost a handful of POSTs (the new entries'
        // rows), not 50.
        assert!(room.posts.load(Ordering::Relaxed) - posts_before <= 3);

        // Older history on demand, until the reader holds all of it.
        while reader.chat3.load_earlier(&reader.weak(), &room).await.unwrap() {}
        writer.finish(&[text("t0", &body)], MessageStatus::Complete).unwrap();
        host.publish();
        assert!(host.chat3.upload_once(&host.weak(), &host.store, &room).await);
        reader.chat3.catch_up(&reader.weak(), &room).await.unwrap();
        assert_eq!(reader.doc.read_entries().unwrap(), host.doc.read_entries().unwrap());
    }

    #[tokio::test]
    async fn an_offline_host_queues_and_a_moved_host_adopts() {
        let room = Mem::default();
        // Host A writes while offline: the outbox holds it, nothing is lost.
        let a = Side::new(true);
        room.offline.store(true, Ordering::Relaxed);
        a.doc.push_message(&user("u0", "written offline")).unwrap();
        a.publish();
        assert!(!a.chat3.upload_once(&a.weak(), &a.store, &room).await);
        assert_eq!(a.store.chat3_outbox_len("chat").unwrap(), 2);
        room.offline.store(false, Ordering::Relaxed);
        assert!(a.chat3.upload_once(&a.weak(), &a.store, &room).await);
        assert_eq!(lock(&room.rows).len(), 2);

        // The chat moves to host B (empty local transcript). B adopts A's
        // rows without re-uploading them, then publishes only its own.
        std::thread::sleep(std::time::Duration::from_millis(5));
        let b = Side::new(true);
        b.chat3.host_catch_up(&b.weak(), &b.store, &room).await.unwrap();
        assert_eq!(b.doc.read_entries().unwrap(), a.doc.read_entries().unwrap());
        assert_eq!(b.store.chat3_outbox_len("chat").unwrap(), 0, "adopted rows are not re-sent");
        b.doc.push_message(&user("u1", "on B")).unwrap();
        b.publish();
        assert_eq!(b.store.chat3_outbox_len("chat").unwrap(), 2);
        assert!(b.chat3.upload_once(&b.weak(), &b.store, &room).await);
        let ids: Vec<String> = {
            let reader = Side::new(false);
            reader.chat3.catch_up(&reader.weak(), &room).await.unwrap();
            reader.doc.read_entries().unwrap().into_iter().map(|e| e.id).collect()
        };
        assert_eq!(ids, ["u0", "u1"]);
    }

    #[tokio::test]
    async fn a_reopened_host_publishes_only_what_changed() {
        let room = Mem::default();
        let dir = tempfile::tempdir().unwrap();
        let store = DocsStore::open(dir.path()).unwrap();
        let host = Side::with_store(true, store, dir);
        host.doc.push_message(&user("u0", "hello")).unwrap();
        host.publish();
        assert!(host.chat3.upload_once(&host.weak(), &host.store, &room).await);
        let snapshot = (
            host.doc.export_snapshot().unwrap(),
            host.doc.export_transcript_snapshot().unwrap().unwrap(),
        );
        // Reopen over the same store: the known hashes come back from disk.
        let Side { store, _dir, .. } = host;
        let known = store.chat3_published("chat").unwrap();
        let state = loro::LoroDoc::new();
        state.import(&snapshot.0).unwrap();
        let transcript = loro::LoroDoc::new();
        transcript.import(&snapshot.1).unwrap();
        let doc = Arc::new(SessionDoc::from_split(state, transcript));
        let chat3 = Chat3::new("chat", known, true, Arc::new(Mutex::new(())));
        let posts = room.posts.load(Ordering::Relaxed);
        chat3.publish(&doc, &store, &|_| {});
        assert!(chat3.upload_once(&Arc::downgrade(&doc), &store, &room).await);
        assert_eq!(room.posts.load(Ordering::Relaxed), posts, "nothing to send after a reopen");
    }

    #[tokio::test]
    async fn a_rev_gap_on_the_socket_asks_for_the_feed() {
        let room = Mem::default();
        let host = Side::new(true);
        let reader = Side::new(false);
        reader.chat3.catch_up(&reader.weak(), &room).await.unwrap();
        host.doc.push_message(&user("u0", "a")).unwrap();
        host.doc.push_message(&user("u1", "b")).unwrap();
        host.publish();
        assert!(host.chat3.upload_once(&host.weak(), &host.store, &room).await);
        // Only the last row is relayed: revs 1..3 were missed.
        let last = lock(&room.rows).iter().max_by_key(|r| r.rev).cloned().unwrap();
        let header = serde_json::json!({"ord": last.key.ord, "idx": last.key.idx, "entry": last.entry, "rev": last.rev});
        assert!(!reader.chat3.apply_frame(&reader.doc, frame_type::ITEM, &header, &last.json));
        tokio::time::timeout(std::time::Duration::from_millis(100), reader.chat3.catch_up_requested())
            .await
            .expect("gap requests a catch-up");
        reader.chat3.catch_up(&reader.weak(), &room).await.unwrap();
        assert_eq!(reader.doc.read_entries().unwrap(), host.doc.read_entries().unwrap());
    }

    #[tokio::test]
    async fn tool_output_streams_as_a_tail_and_settles_with_a_ref() {
        use zeron_proto::{AgentEvent, ToolCall};
        let mut parts = vec![MessagePart::Tool {
            id: "call_1".into(),
            call: ToolCall::Exec { command: "cargo build".into() },
            is_error: false,
            resolved: false,
            output: None,
            diff: None,
            output_ref: None,
            output_bytes: None,
            diff_ref: None,
            diff_stats: None,
            subagent_ref: None,
            subagent_status: None,
            subagent_tail: None,
        }];
        let mut buffers = HashMap::new();
        for i in 0..40 {
            let delta = AgentEvent::ToolOutputDelta { id: "call_1".into(), text: format!("Compiling crate{i}\n") };
            assert!(fold_tool_output(&mut parts, &delta, &mut buffers, "chat").is_none());
        }
        let MessagePart::Tool { output: Some(tail), .. } = &parts[0] else { panic!("no live tail") };
        assert!(tail.ends_with("Compiling crate39"));
        assert!(tail.lines().count() <= LIVE_TAIL_LINES);

        // The ordinary fold resolves the part and clears its output; then
        // ours stamps the summary, size and ref and hands back the full text.
        let result = AgentEvent::ToolResult { id: "call_1".into(), is_error: false, output: None, diff: None };
        zeron_doc::fold_event_into_parts(&mut parts, &result);
        let (part, full) = fold_tool_output(&mut parts, &result, &mut buffers, "chat").expect("full output");
        assert_eq!(part, "call_1");
        assert_eq!(full.lines().count(), 40);
        let MessagePart::Tool { resolved: true, output, output_bytes, output_ref, .. } = &parts[0] else {
            panic!("unexpected {:?}", parts[0])
        };
        assert_eq!(output.as_deref(), Some("Compiling crate0…"));
        assert_eq!(*output_bytes, Some(full.len() as u64));
        assert_eq!(output_ref.as_deref(), Some("chat3:chat/call_1"));
    }

    #[test]
    fn output_buffer_keeps_head_and_tail_under_the_cap() {
        let mut buffer = OutputBuffer::default();
        let line = "0123456789abcdef\n";
        for _ in 0..40_000 {
            buffer.push(line); // ~680 KB
        }
        let text = buffer.text();
        assert!(text.len() <= TOOL_OUTPUT_CAP + 64, "{}", text.len());
        assert!(text.starts_with(line));
        assert!(text.ends_with(line));
        assert!(text.contains("bytes truncated"));
    }

    #[test]
    fn small_outputs_pass_through_untouched() {
        assert_eq!(cap_output("ok\n"), "ok\n");
        let mut buffer = OutputBuffer::default();
        buffer.push("héllo ");
        buffer.push("wörld");
        assert_eq!(buffer.text(), "héllo wörld");
    }

    #[test]
    fn live_tail_is_the_last_lines() {
        let text: String = (0..100).map(|i| format!("line {i}\n")).collect();
        let tail = live_tail(&text);
        assert!(tail.ends_with("line 99"));
        assert!(tail.lines().count() <= LIVE_TAIL_LINES);
        assert_eq!(live_tail("single"), "single");
        let long = "x".repeat(10_000);
        assert_eq!(live_tail(&long).len(), LIVE_TAIL_BYTES);
    }
}
