//! `DocHost` wiring for gen-3 chats (docs/chat3-sync.md): opening the split
//! doc, the publisher task, the host/reader role tasks that ride a chat's
//! room membership, history paging and tool outputs. The plane logic itself
//! lives in `chat3_host` and `zeron_doc::transcript_plane`.

use std::sync::Arc;
use std::time::Duration;

use super::*;
use crate::chat2_host::{CHAT3_DOC_EPOCH, transcript_doc_id};
use crate::chat3_host::EdgeTranscriptTransport;
use zeron_doc::transcript_plane::Reader;

/// Coalesce a burst of transcript commits into one publisher drain (the host
/// commits every `STREAM_COMMIT_MS`; this only merges same-tick writes).
const PUBLISH_COALESCE: Duration = Duration::from_millis(40);
const RETRY_CAP: Duration = Duration::from_secs(30);
/// A reader without a socket polls the change feed this often.
const HTTP_FEED_POLL: Duration = Duration::from_secs(30);

impl DocHost {
    /// The generation of a chat whose registry row hasn't synced yet. A
    /// subagent doc follows its parent chat (a Cloud runner can't see the
    /// labs flag, but it sees the chats it hosts); anything else follows the
    /// flag, which is what the minting device stamped.
    pub(super) fn assumed_room_gen(&self, chat_id: &str) -> u32 {
        if let Some((parent, _)) = chat_id.split_once("--sub--")
            && let Some(row) = self.workspace().and_then(|w| w.chat(parent).ok().flatten())
        {
            return if row.needs_newer_app() { 1 } else { row.room_generation().max(2) };
        }
        self.workspace().map_or(2, |w| w.new_chat_room_gen())
    }

    /// Whether this device publishes a gen-3 doc's transcript. A subagent doc
    /// has no registry row (where `is_host` defaults to yes): its host is its
    /// parent chat's, or every viewer would think it publishes and ignore the
    /// rows and live frames the real host relays.
    pub(super) fn plane_host(&self, chat_id: &str) -> bool {
        match chat_id.split_once("--sub--") {
            Some((parent, _)) => self.is_host(parent),
            None => self.is_host(chat_id),
        }
    }

    /// Materialize a gen-3 doc: the state snapshot under the chat id and the
    /// local transcript beside it. An older-lineage doc on disk (this device
    /// opened the chat before its row synced, on the generation it assumed)
    /// is set aside with a rollback copy; this device's own pending commands
    /// carry over, exactly like the chat2 adopt.
    pub(super) fn open_split_doc(
        &self,
        chat_id: &str,
        stored: Option<(Vec<u8>, u64, u32)>,
        snapshot_len: &mut usize,
        cursor: &mut u64,
        requeue_commands: &mut Vec<SessionCommandEntry>,
    ) -> Result<SessionDoc, EngineError> {
        let transcript_id = transcript_doc_id(chat_id);
        match stored {
            Some((bytes, stored_cursor, epoch)) if epoch >= CHAT3_DOC_EPOCH => {
                *snapshot_len = bytes.len();
                *cursor = stored_cursor;
                let state = loro::LoroDoc::new();
                state
                    .import(&bytes)
                    .map_err(|e| EngineError::Other(format!("snapshot import failed: {e}")))?;
                let transcript = loro::LoroDoc::new();
                if let Some((bytes, _, _)) = self.inner.store.load_snapshot_with_cursor(&transcript_id)? {
                    *snapshot_len += bytes.len();
                    if let Err(err) = transcript.import(&bytes) {
                        // The room holds every row; a reader re-pages and a
                        // host re-adopts. Never fail the chat over a cache.
                        tracing::warn!(chat = %chat_id, error = %err,
                            "chat3 transcript snapshot unreadable; refilling from the room");
                    }
                }
                Ok(SessionDoc::from_split(state, transcript))
            }
            Some((bytes, _, epoch)) => {
                tracing::info!(chat = %chat_id, old_epoch = epoch,
                    "chat3 adopt: setting aside an older local doc (rollback copy kept)");
                let rollback_id = format!("{chat_id}.pre-chat3");
                if matches!(self.inner.store.load_snapshot(&rollback_id), Ok(None)) {
                    let _ = self.inner.store.save_snapshot(&rollback_id, &bytes);
                }
                let old = loro::LoroDoc::new();
                if old.import(&bytes).is_ok()
                    && let Ok(commands) = SessionDoc::from_doc(old).read_commands()
                {
                    requeue_commands.extend(commands.into_iter().filter(|c| {
                        c.status == SessionCommandStatus::Pending && c.issued_by == self.inner.config.device_id
                    }));
                }
                self.init_split_doc(chat_id)
            }
            None => self.init_split_doc(chat_id),
        }
    }

    /// A fresh split doc, stamped on disk at once (the same reason the
    /// chat2 born path stamps: an unstamped row reads as an older lineage on
    /// the next open, and the adopt would discard everything written since).
    fn init_split_doc(&self, chat_id: &str) -> Result<SessionDoc, EngineError> {
        let doc = SessionDoc::init_split(chat_id)?;
        if let Ok(Some(transcript)) = doc.export_transcript_snapshot() {
            let _ = self.inner.store.save_snapshot_with_cursor(
                &transcript_doc_id(chat_id),
                &transcript,
                0,
                CHAT3_DOC_EPOCH,
            );
        }
        if let Ok(snapshot) = doc.export_snapshot() {
            let _ = self
                .inner
                .store
                .save_snapshot_with_cursor(chat_id, &snapshot, 0, CHAT3_DOC_EPOCH);
        }
        Ok(doc)
    }

    /// The host's publisher drain, one task per gen-3 handle. Runs whether or
    /// not the room is reachable: stored rows wait in the durable outbox.
    pub(super) fn spawn_chat3_publisher(&self, handle: &Arc<ChatDocHandle>) {
        let Some(chat3) = handle.chat3.clone() else { return };
        let weak = Arc::downgrade(handle);
        let host = self.clone();
        let store = self.inner.store.clone();
        let chat = handle.chat_id.clone();
        self.spawn_worker(async move {
            loop {
                tokio::select! {
                    _ = chat3.changed.notified() => {}
                    // An evicted handle never notifies again: notice and end.
                    _ = tokio::time::sleep(Duration::from_secs(60)) => {
                        if weak.strong_count() == 0 { return; }
                        continue;
                    }
                }
                tokio::time::sleep(PUBLISH_COALESCE).await;
                let Some(handle) = weak.upgrade() else { return };
                chat3.set_host(host.plane_host(&chat));
                let (chat3, store) = (chat3.clone(), store.clone());
                let _ = tokio::task::spawn_blocking(move || {
                    chat3.publish(&handle.doc, &store, &|frame| {
                        if let Some(client) = &*lock(&handle.chat2) {
                            client.send_live(frame);
                        }
                    });
                })
                .await;
            }
        });
    }

    /// Room-side duties of a gen-3 handle, started when its chat2 client is
    /// admitted and cancelled with it: the host pulls an earlier host's rows
    /// then uploads its outbox; a reader catches up on join, on every
    /// reconnect and on a rev gap.
    pub(super) fn spawn_chat3_roles(
        &self,
        edge: EdgeConfig,
        handle: &Arc<ChatDocHandle>,
        mut events: tokio::sync::broadcast::Receiver<zeron_sync::chat_client::ChatEvent>,
        cancel: CancellationToken,
        priority: zeron_sync::budget::Priority,
    ) {
        let Some(chat3) = handle.chat3.clone() else { return };
        chat3.set_host(self.plane_host(&handle.chat_id));
        let transport = Arc::new(
            EdgeTranscriptTransport::new(
                self.inner.http.clone(),
                edge,
                handle.chat_id.clone(),
                self.inner.config.device_id.clone(),
            )
            .with_priority(priority),
        );
        let doc = Arc::downgrade(&handle.doc);
        let store = self.inner.store.clone();
        let weak_handle = Arc::downgrade(handle);
        {
            let (chat3, transport, doc, store) = (chat3.clone(), transport.clone(), doc.clone(), store.clone());
            self.spawn_sync_worker(cancel.clone(), async move {
                use zeron_sync::chat_client::ChatEvent;
                let mut backoff = Duration::from_secs(1);
                loop {
                    let caught_up = if chat3.is_host() {
                        chat3.host_catch_up(&doc, &store, &*transport).await
                    } else {
                        chat3.catch_up(&doc, &*transport).await
                    };
                    if let Err(err) = caught_up {
                        tracing::debug!(chat = %chat3.chat_id(), error = %err, "chat3 catch-up failed; retrying");
                        tokio::time::sleep(backoff + crate::workspace_host::join_retry_jitter()).await;
                        backoff = (backoff * 2).min(RETRY_CAP);
                        continue;
                    }
                    backoff = Duration::from_secs(1);
                    loop {
                        tokio::select! {
                            event = events.recv() => match event {
                                Ok(ChatEvent::Connected) => break,
                                Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                                Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                            },
                            _ = chat3.catch_up_requested() => break,
                            // Rows reach a reader by socket relay. On a
                            // network that never passes the upgrade (the
                            // HTTPS pull transport), poll the feed instead.
                            _ = tokio::time::sleep(HTTP_FEED_POLL) => {
                                let Some(handle) = weak_handle.upgrade() else { return };
                                let connected = lock(&handle.chat2).as_ref().is_some_and(|c| c.stats().connected);
                                if !connected && !chat3.is_host() {
                                    break;
                                }
                            }
                        }
                    }
                }
            });
        }
        self.spawn_sync_worker(cancel, async move {
            let mut backoff = Duration::from_secs(1);
            loop {
                if chat3.is_host() {
                    if chat3.upload_once(&doc, &store, &*transport).await {
                        backoff = Duration::from_secs(1);
                    } else {
                        tokio::time::sleep(backoff + crate::workspace_host::join_retry_jitter()).await;
                        backoff = (backoff * 2).min(RETRY_CAP);
                        continue;
                    }
                }
                tokio::select! {
                    _ = chat3.upload.notified() => {}
                    _ = tokio::time::sleep(RETRY_CAP) => {}
                }
            }
        });
    }

    /// Page older transcript history into a gen-3 chat a reader holds only
    /// the tail of. Returns whether still older history exists.
    pub async fn load_earlier_messages(&self, chat_id: &str) -> Result<bool, EngineError> {
        let handle = self.open(chat_id)?;
        let Some(chat3) = handle.chat3.clone() else { return Ok(false) };
        let Some(edge) = self.inner.config.edge.clone() else {
            return Ok(false);
        };
        let transport = EdgeTranscriptTransport::new(
            self.inner.http.clone(),
            edge,
            chat_id.to_string(),
            self.inner.config.device_id.clone(),
        );
        let doc = Arc::downgrade(&handle.doc);
        drop(handle);
        chat3
            .load_earlier(&doc, &transport)
            .await
            .map_err(|e| EngineError::Other(e.to_string()))
    }

    /// Whether a gen-3 chat's local transcript starts after the room's first
    /// row (the UI offers "Load earlier messages").
    pub fn has_earlier_messages(&self, chat_id: &str) -> bool {
        lock(&self.inner.handles)
            .get(chat_id)
            .is_some_and(|handle| handle.chat3.is_some() && Reader::cursor(&handle.doc).more_before)
    }

    /// Queue a tool's full output for its gen-3 chat's room. The part's
    /// `outputRef` points at it; the UI fetches it when the chip expands.
    pub fn queue_tool_output(&self, chat_id: &str, part_id: &str, text: &str) {
        if !zeron_sync::chat3::valid_part_id(part_id) {
            tracing::warn!(chat = %chat_id, part = %part_id, "chat3: tool id unusable as an output key; not stored");
            return;
        }
        if let Err(err) = self.inner.store.chat3_enqueue_output(chat_id, part_id, text.as_bytes()) {
            tracing::warn!(chat = %chat_id, error = %err, "chat3: output queue write failed");
            return;
        }
        if let Some(handle) = lock(&self.inner.handles).get(chat_id)
            && let Some(chat3) = &handle.chat3
        {
            chat3.upload.notify_one();
        }
    }

    /// `chat3:{chatId}/{partId}` refs: the full output from the chat's room.
    pub(super) async fn fetch_chat3_output(&self, chat_id: &str, part_id: &str) -> Result<String, EngineError> {
        use zeron_sync::chat3::TranscriptTransport;
        let Some(edge) = self.inner.config.edge.clone() else {
            return Err(EngineError::Other("sync is off".into()));
        };
        let transport = EdgeTranscriptTransport::new(
            self.inner.http.clone(),
            edge,
            chat_id.to_string(),
            self.inner.config.device_id.clone(),
        );
        match transport.get_output(part_id.to_string()).await {
            Ok(Some(bytes)) => Ok(String::from_utf8_lossy(&bytes).into_owned()),
            Ok(None) => {
                // Not uploaded yet: this device may be the host with it still
                // queued (offline, or just finished).
                match self.inner.store.chat3_next_output_for(chat_id, part_id) {
                    Ok(Some(bytes)) => Ok(String::from_utf8_lossy(&bytes).into_owned()),
                    _ => Err(EngineError::Other("full output not available".into())),
                }
            }
            Err(err) => Err(EngineError::Other(err.to_string())),
        }
    }

    /// Chat deleted: its transcript snapshot and plane bookkeeping go too.
    pub(super) fn purge_chat3(&self, chat_id: &str) {
        let _ = self.inner.store.delete_snapshot(&transcript_doc_id(chat_id));
        if let Err(err) = self.inner.store.chat3_purge(chat_id) {
            tracing::warn!(chat = %chat_id, error = %err, "chat3 purge failed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeron_doc::{MessagePart, MessageRole, MessageStatus, SessionMessageEntry};

    fn host_over(store: Arc<DocsStore>) -> DocHost {
        DocHost::new(
            store,
            DocHostConfig {
                device_id: "host".into(),
                default_harness: HarnessId::Mock,
                edge: None,
            },
        )
    }

    fn user(id: &str) -> SessionMessageEntry {
        SessionMessageEntry {
            id: id.into(),
            role: MessageRole::User,
            parts: vec![MessagePart::Text { id: "t0".into(), text: format!("hello from {id}") }],
            created_at: 1,
            device_id: "host".into(),
            status: Some(MessageStatus::Complete),
            continuation_of: None,
            duration_ms: None,
        }
    }

    async fn until(mut ok: impl FnMut() -> bool) {
        for _ in 0..200 {
            if ok() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("condition never held");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_gen3_chat_opens_split_publishes_and_survives_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(DocsStore::open(dir.path()).unwrap());
        // A split lineage on disk is what makes a chat gen 3 with no registry.
        let seed = SessionDoc::init_split("c3").unwrap();
        store
            .save_snapshot_with_cursor("c3", &seed.export_snapshot().unwrap(), 0, CHAT3_DOC_EPOCH)
            .unwrap();

        let host = host_over(store.clone());
        let handle = host.open_local("c3").unwrap();
        assert!(handle.doc.is_split());
        assert_eq!(handle.room_gen, 3);
        let chat3 = handle.chat3().expect("gen-3 handles carry the plane").clone();
        assert!(chat3.is_host(), "no workspace: this device hosts");

        handle.doc.push_message(&user("u0")).unwrap();
        // The publisher task drains into the durable outbox on its own.
        until(|| store.chat3_outbox_len("c3").unwrap() == 2).await;
        assert_eq!(store.chat3_published("c3").unwrap().len(), 2);

        host.flush_all();
        drop(handle);
        drop(host);

        // Restart: the transcript comes back from its own snapshot, and the
        // reopened publisher has nothing new to queue.
        let host = host_over(store.clone());
        let handle = host.open_local("c3").unwrap();
        let entries = handle.doc.read_entries().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, "u0");
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert_eq!(store.chat3_outbox_len("c3").unwrap(), 2, "no duplicate rows queued");
        // The main snapshot never holds messages (boot salvage must skip it).
        let (main, _, epoch) = store.load_snapshot_with_cursor("c3").unwrap().unwrap();
        assert_eq!(epoch, CHAT3_DOC_EPOCH);
        let raw = loro::LoroDoc::new();
        raw.import(&main).unwrap();
        assert!(SessionDoc::from_doc(raw).read_entries().unwrap().is_empty());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_chat2_chat_is_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(DocsStore::open(dir.path()).unwrap());
        let host = host_over(store.clone());
        let handle = host.open_local("c2").unwrap();
        assert!(!handle.doc.is_split());
        assert!(handle.chat3().is_none());
        handle.doc.push_message(&user("u0")).unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(store.chat3_outbox_len("c2").unwrap(), 0);
        assert!(store.load_snapshot(&transcript_doc_id("c2")).unwrap().is_none());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn purge_removes_the_transcript_and_plane_bookkeeping() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(DocsStore::open(dir.path()).unwrap());
        let seed = SessionDoc::init_split("gone").unwrap();
        store
            .save_snapshot_with_cursor("gone", &seed.export_snapshot().unwrap(), 0, CHAT3_DOC_EPOCH)
            .unwrap();
        let host = host_over(store.clone());
        let handle = host.open_local("gone").unwrap();
        handle.doc.push_message(&user("u0")).unwrap();
        until(|| store.chat3_outbox_len("gone").unwrap() == 2).await;
        host.flush_all();
        drop(handle);
        host.purge_chat("gone");
        assert!(store.load_snapshot("gone").unwrap().is_none());
        assert!(store.load_snapshot(&transcript_doc_id("gone")).unwrap().is_none());
        assert_eq!(store.chat3_outbox_len("gone").unwrap(), 0);
        assert!(store.chat3_published("gone").unwrap().is_empty());
    }
}
