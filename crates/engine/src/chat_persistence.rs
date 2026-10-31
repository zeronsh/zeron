//! Coalesced chat snapshots. Cursor is sampled BEFORE export; snapshot and
//! cursor commit atomically. Queue/lock waits never occupy a Tokio worker.
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;
use tokio::sync::mpsc;
use zeron_doc::SessionDoc;
use zeron_sync::DocsStore;

/// Local-only docs stay here. It is also the floor for an edge outbox:
/// local ops are already durable in `chat_outbox` until acked, so a slow
/// export may wait longer instead of rewriting the whole snapshot every second.
const SAVE_INTERVAL_MIN_MS: u64 = 1_000;
const SAVE_INTERVAL_MAX_MS: u64 = 10_000;
/// `clamp(last_export_ms * SCALE, MIN, MAX)` once a doc has an edge outbox.
const SAVE_INTERVAL_EXPORT_SCALE: u64 = 50;

fn scaled_save_interval(edge_outbox: bool, last_export_ms: u64) -> Duration {
    if !edge_outbox {
        return Duration::from_millis(SAVE_INTERVAL_MIN_MS);
    }
    let scaled = last_export_ms.saturating_mul(SAVE_INTERVAL_EXPORT_SCALE);
    Duration::from_millis(scaled.clamp(SAVE_INTERVAL_MIN_MS, SAVE_INTERVAL_MAX_MS))
}

pub(crate) struct ChatPersistence {
    doc: Weak<SessionDoc>,
    store: Arc<DocsStore>,
    chat_id: String,
    cursor: AtomicU64,
    generation: AtomicU64,
    saved: AtomicU64,
    snapshot_bytes: AtomicUsize,
    urgent: AtomicBool,
    /// Refreshed on the blocking flush. The async loop only reads the atomic:
    /// the marker query must not occupy a Tokio worker.
    edge_outbox: AtomicBool,
    /// Wall time of the last `export_snapshot`, in milliseconds.
    last_export_ms: AtomicU64,
    write: Mutex<()>,
    wake: Option<mpsc::Sender<()>>,
    /// Seeded from the store, then set by the first verified write: cursor
    /// repair is once per legacy snapshot, never once per rejoin. The thin
    /// client reads the same flag from the store (client/src/live/room.rs).
    cursor_verified: AtomicBool,
    #[cfg(test)]
    writes: std::sync::atomic::AtomicUsize,
    #[cfg(test)]
    before_export: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

impl ChatPersistence {
    pub(crate) fn new(
        doc: &Arc<SessionDoc>,
        store: Arc<DocsStore>,
        chat_id: String,
        cursor: u64,
    ) -> Arc<Self> {
        let verified = store.snapshot_cursor_verified(&chat_id).unwrap_or(false);
        let runtime = tokio::runtime::Handle::try_current().ok();
        let (tx, rx) = mpsc::channel(1);
        let this = Arc::new(Self {
            doc: Arc::downgrade(doc),
            store,
            chat_id,
            // Legacy cursors can have holes. Don't certify one before the
            // one-time repair has established a contiguous applied prefix.
            cursor: AtomicU64::new(if verified { cursor } else { 0 }),
            generation: AtomicU64::new(0),
            saved: AtomicU64::new(0),
            snapshot_bytes: AtomicUsize::new(0),
            urgent: AtomicBool::new(false),
            edge_outbox: AtomicBool::new(false),
            last_export_ms: AtomicU64::new(0),
            write: Mutex::new(()),
            wake: runtime.as_ref().map(|_| tx),
            cursor_verified: AtomicBool::new(verified),
            #[cfg(test)]
            writes: std::sync::atomic::AtomicUsize::new(0),
            #[cfg(test)]
            before_export: Mutex::new(None),
        });
        if let Some(runtime) = runtime {
            runtime.spawn(Self::run(Arc::downgrade(&this), rx));
        }
        this
    }

    pub(crate) fn cursor(&self) -> u64 {
        self.cursor.load(Ordering::Acquire)
    }

    pub(crate) fn cursor_verified(&self) -> bool {
        self.cursor_verified.load(Ordering::Acquire)
    }

    pub(crate) fn is_clean(&self) -> bool {
        self.saved.load(Ordering::Acquire) == self.generation.load(Ordering::Acquire)
    }

    pub(crate) fn snapshot_bytes(&self) -> usize {
        self.snapshot_bytes.load(Ordering::Relaxed)
    }

    /// Edge docs wait `clamp(last_export_ms * 50, 1s, 10s)`. Local-only docs
    /// stay at 1s: the snapshot is their only durability.
    fn save_interval(&self) -> Duration {
        scaled_save_interval(
            self.edge_outbox.load(Ordering::Relaxed),
            self.last_export_ms.load(Ordering::Relaxed),
        )
    }

    pub(crate) fn applied(&self, cursor: u64, immediate: bool) {
        self.cursor.fetch_max(cursor, Ordering::AcqRel);
        self.dirty(immediate);
    }

    pub(crate) fn reset_cursor(&self, cursor: u64) {
        if self.cursor.swap(cursor, Ordering::AcqRel) != cursor {
            self.dirty(true);
        }
    }

    pub(crate) fn dirty(&self, immediate: bool) {
        self.generation.fetch_add(1, Ordering::Release);
        if immediate {
            self.urgent.store(true, Ordering::Release);
        }
        if let Some(wake) = &self.wake {
            let _ = wake.try_send(());
        } else {
            // Offline synchronous tooling/tests have no executor to schedule.
            self.flush_sync();
        }
    }

    async fn run(weak: Weak<Self>, mut rx: mpsc::Receiver<()>) {
        while rx.recv().await.is_some() {
            let Some(this) = weak.upgrade() else { return };
            let deadline = tokio::time::Instant::now() + this.save_interval();
            drop(this);
            loop {
                let Some(this) = weak.upgrade() else { return };
                let urgent = this.urgent.swap(false, Ordering::AcqRel);
                drop(this);
                if urgent {
                    break;
                }
                tokio::select! {
                    _ = tokio::time::sleep_until(deadline) => break,
                    message = rx.recv() => if message.is_none() { return; },
                }
            }
            let Some(this) = weak.upgrade() else { return };
            // All chat persisters sharing this SQLite connection queue
            // asynchronously, BEFORE occupying a blocking-pool thread.
            let permit = this.store.snapshot_writer.clone().lock_owned().await;
            let job = this.clone();
            if let Err(error) = tokio::task::spawn_blocking(move || {
                let _permit = permit;
                job.flush_sync();
            })
            .await
            {
                tracing::error!(%error, "chat snapshot worker failed");
            }
            if this.doc.strong_count() == 0 {
                return;
            }
            if this.saved.load(Ordering::Acquire) != this.generation.load(Ordering::Acquire) {
                // Includes failed writes and changes during export. Retry on
                // the next interval even if the document has gone quiet.
                if let Some(wake) = &this.wake {
                    let _ = wake.try_send(());
                }
            }
        }
    }

    pub(crate) fn flush_sync(&self) {
        let flush = || {
            let _write = self.write.lock().unwrap_or_else(|e| e.into_inner());
            let generation = self.generation.load(Ordering::Acquire);
            if generation == self.saved.load(Ordering::Acquire) {
                return;
            }
            let Some(doc) = self.doc.upgrade() else {
                return;
            };
            // Never read the cursor AFTER exporting: a concurrent import
            // could then label an older snapshot with a newer cursor.
            let cursor = self.cursor.load(Ordering::Acquire);
            let edge = self
                .store
                .chat_outbox_initialized(&self.chat_id)
                .unwrap_or(false);
            self.edge_outbox.store(edge, Ordering::Relaxed);
            #[cfg(test)]
            if let Some(hook) = self.before_export.lock().unwrap().take() {
                hook();
            }
            let export_started = std::time::Instant::now();
            let exported = doc.export_snapshot().map_err(|e| e.to_string());
            self.last_export_ms.store(
                u64::try_from(export_started.elapsed().as_millis()).unwrap_or(u64::MAX),
                Ordering::Relaxed,
            );
            let result = exported.and_then(|bytes| {
                self.snapshot_bytes.store(bytes.len(), Ordering::Relaxed);
                self.store
                    .save_verified_snapshot_with_cursor(
                        &self.chat_id,
                        &bytes,
                        cursor,
                        super::chat2_host::CHAT2_DOC_EPOCH,
                    )
                    .map_err(|e| e.to_string())
            });
            match result {
                Ok(()) => {
                    #[cfg(test)]
                    self.writes.fetch_add(1, Ordering::Relaxed);
                    self.cursor_verified.store(true, Ordering::Release);
                    self.saved.store(generation, Ordering::Release);
                }
                Err(error) => {
                    tracing::warn!(chat = %self.chat_id, %error, "chat snapshot failed; retrying")
                }
            }
        };
        // Compatibility for synchronous shutdown/eviction and command APIs.
        // Async persisters already execute this on the blocking pool.
        if tokio::runtime::Handle::try_current()
            .is_ok_and(|h| h.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread)
        {
            tokio::task::block_in_place(flush);
        } else {
            flush();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    /// Runs only on an explicitly supplied local snapshot; never writes back
    /// to it or connects to an edge/production room.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore = "requires ZERON_WHALE_SNAPSHOT; reads privately supplied snapshot into a temporary store"]
    async fn real_whale_replay_keeps_146_heartbeats_running_on_two_workers() {
        let bytes = std::fs::read(std::env::var("ZERON_WHALE_SNAPSHOT").unwrap()).unwrap();
        let raw = loro::LoroDoc::new();
        raw.import(&bytes).unwrap();
        let doc = Arc::new(SessionDoc::from_doc(raw));
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(DocsStore::open(dir.path()).unwrap());
        let persistence = ChatPersistence::new(&doc, store.clone(), "whale".into(), 0);
        let worst = Arc::new(AtomicU64::new(0));
        let beats = Arc::new(AtomicUsize::new(0));
        let mut heartbeat_tasks = Vec::new();
        for _ in 0..146 {
            let worst = worst.clone();
            let beats = beats.clone();
            heartbeat_tasks.push(tokio::spawn(async move {
                loop {
                    let start = std::time::Instant::now();
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    worst.fetch_max(start.elapsed().as_millis() as u64, Ordering::Relaxed);
                    beats.fetch_add(1, Ordering::Relaxed);
                }
            }));
        }
        let other_store = store.clone();
        let contender = tokio::spawn(async move {
            for _ in 0..32 {
                other_store
                    .save_snapshot("registry", b"other room")
                    .unwrap();
                let _ = other_store.load_snapshot("whale").unwrap();
                tokio::time::sleep(Duration::from_millis(125)).await;
            }
        });
        let mut cadence = tokio::time::interval(Duration::from_millis(125));
        for cursor in 1..=32 {
            cadence.tick().await;
            doc.doc()
                .get_map("persistence-test")
                .insert("applied", cursor as i64)
                .unwrap();
            doc.doc().commit();
            persistence.applied(cursor, false);
        }
        contender.await.unwrap();
        persistence.flush_sync();
        for task in heartbeat_tasks {
            task.abort();
        }
        let writes = persistence.writes.load(Ordering::Relaxed);
        eprintln!(
            "snapshot_bytes={} rows=32 writes={} heartbeat_count={} worst_50ms_heartbeat_ms={}",
            bytes.len(),
            writes,
            beats.load(Ordering::Relaxed),
            worst.load(Ordering::Relaxed)
        );
        assert!(writes <= 6, "replay must not export/write per row");
        assert!(beats.load(Ordering::Relaxed) > 146 * 50);
        assert!(
            worst.load(Ordering::Relaxed) < 300,
            "network timers starved"
        );
        assert_eq!(store.snapshot_cursor("whale").unwrap(), 32);
    }

    fn fixture() -> (
        tempfile::TempDir,
        Arc<SessionDoc>,
        Arc<DocsStore>,
        Arc<ChatPersistence>,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(DocsStore::open(dir.path()).unwrap());
        let doc = Arc::new(SessionDoc::init("whale").unwrap());
        let persistence = ChatPersistence::new(&doc, store.clone(), "whale".into(), 0);
        (dir, doc, store, persistence)
    }

    #[test]
    fn an_unverified_cursor_opens_at_zero() {
        let (_dir, doc, store, _) = fixture();
        let bytes = doc.export_snapshot().unwrap();
        store
            .save_snapshot_with_cursor("legacy", &bytes, 500, 2)
            .unwrap();
        let persistence = ChatPersistence::new(&doc, store, "legacy".into(), 500);
        assert_eq!(persistence.cursor(), 0);
    }

    #[tokio::test]
    async fn replay_burst_coalesces_and_flush_bypasses_debounce() {
        let (_dir, doc, store, persistence) = fixture();
        for cursor in 1..=1000 {
            doc.doc()
                .get_map("test")
                .insert("applied", cursor as i64)
                .unwrap();
            doc.doc().commit();
            persistence.applied(cursor, false);
        }
        assert_eq!(
            persistence.writes.load(Ordering::Relaxed),
            0,
            "no per-row writes"
        );
        // The debounce schedules a blocking SQLite write; a loaded CI runner
        // may not finish that write within 200 ms of the debounce deadline.
        // Wait for completion, then still require the entire burst to coalesce.
        tokio::time::timeout(Duration::from_secs(10), async {
            while persistence.writes.load(Ordering::Relaxed) == 0 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("debounced snapshot should finish");
        assert_eq!(persistence.writes.load(Ordering::Relaxed), 1);
        let (bytes, cursor, _) = store.load_snapshot_with_cursor("whale").unwrap().unwrap();
        assert_eq!(cursor, 1000);
        assert!(store.snapshot_cursor_verified("whale").unwrap());
        let restored = loro::LoroDoc::new();
        restored.import(&bytes).unwrap();
        assert_eq!(
            restored.get_map("test").get_deep_value(),
            doc.doc().get_map("test").get_deep_value()
        );
        persistence.applied(1001, true);
        tokio::time::timeout(Duration::from_millis(500), async {
            while persistence.writes.load(Ordering::Relaxed) != 2 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        persistence.applied(1002, false);
        persistence.flush_sync(); // shutdown/eviction must not await the timer
        assert_eq!(store.snapshot_cursor("whale").unwrap(), 1002);
        persistence.reset_cursor(0);
        persistence.flush_sync();
        assert_eq!(
            store.snapshot_cursor("whale").unwrap(),
            0,
            "server-reset cursor must be allowed to decrease"
        );
    }

    #[test]
    fn first_verified_write_ends_legacy_cursor_repair() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(DocsStore::open(dir.path()).unwrap());
        let doc = Arc::new(SessionDoc::init("born").unwrap());
        // A cold reader's first open stamps an unverified lineage.
        store
            .save_snapshot_with_cursor(
                "born",
                &doc.export_snapshot().unwrap(),
                0,
                crate::chat2_host::CHAT2_DOC_EPOCH,
            )
            .unwrap();
        let persistence = ChatPersistence::new(&doc, store.clone(), "born".into(), 0);
        assert!(!persistence.cursor_verified());
        persistence.applied(7, true); // no runtime: flushes inline
        assert!(persistence.cursor_verified(), "the next join would rewind");
        assert!(ChatPersistence::new(&doc, store, "born".into(), 7).cursor_verified());
    }

    /// Only a write that landed certifies the cursor: a failed one leaves
    /// the legacy repair in place until a retry succeeds.
    #[test]
    fn failed_write_keeps_legacy_cursor_repair() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(DocsStore::open(dir.path()).unwrap());
        let doc = Arc::new(SessionDoc::init("born").unwrap());
        store
            .save_snapshot_with_cursor(
                "born",
                &doc.export_snapshot().unwrap(),
                0,
                crate::chat2_host::CHAT2_DOC_EPOCH,
            )
            .unwrap();
        let persistence = ChatPersistence::new(&doc, store.clone(), "born".into(), 0);
        let db = rusqlite::Connection::open(dir.path().join("docs.sqlite3")).unwrap();
        db.execute_batch(
            "CREATE TRIGGER fail_update BEFORE UPDATE ON snapshots BEGIN SELECT RAISE(FAIL, 'injected disk failure'); END;
             CREATE TRIGGER fail_insert BEFORE INSERT ON snapshots BEGIN SELECT RAISE(FAIL, 'injected disk failure'); END;",
        )
        .unwrap();
        persistence.applied(7, true); // no runtime: flushes inline, and fails
        assert!(!persistence.is_clean(), "the write failed");
        assert!(!persistence.cursor_verified());
        assert!(!store.snapshot_cursor_verified("born").unwrap());
        db.execute_batch("DROP TRIGGER fail_update; DROP TRIGGER fail_insert;")
            .unwrap();
        persistence.flush_sync();
        assert!(persistence.is_clean());
        assert!(persistence.cursor_verified());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn concurrent_import_cannot_label_an_older_export_with_a_newer_cursor() {
        let (_dir, doc, store, persistence) = fixture();
        persistence.applied(1, false);
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        *persistence.before_export.lock().unwrap() = Some(Box::new(move || {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        }));
        let p = persistence.clone();
        let job = tokio::task::spawn_blocking(move || p.flush_sync());
        started_rx.recv().unwrap();
        doc.doc().get_map("test").insert("second", true).unwrap();
        doc.doc().commit();
        persistence.applied(2, false);
        release_tx.send(()).unwrap();
        job.await.unwrap();
        assert_eq!(
            store.snapshot_cursor("whale").unwrap(),
            1,
            "cursor must have been captured before export"
        );
        persistence.flush_sync();
        assert_eq!(store.snapshot_cursor("whale").unwrap(), 2);
    }

    #[test]
    fn adaptive_interval_scales_with_doc_size() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(DocsStore::open(dir.path()).unwrap());
        for id in ["small", "large"] {
            store.initialize_chat_outbox(id, &[]).unwrap();
        }
        let small_doc = Arc::new(SessionDoc::init("small").unwrap());
        small_doc.doc().get_map("t").insert("k", "v").unwrap();
        small_doc.doc().commit();
        let small = ChatPersistence::new(&small_doc, store.clone(), "small".into(), 0);
        small.dirty(true);

        let large_doc = Arc::new(SessionDoc::init("large").unwrap());
        large_doc
            .doc()
            .get_map("t")
            .insert("blob", "x".repeat(2 * 1024 * 1024).as_str())
            .unwrap();
        large_doc.doc().commit();
        let large = ChatPersistence::new(&large_doc, store.clone(), "large".into(), 0);
        large.dirty(true);

        let small_ms = small.last_export_ms.load(Ordering::Relaxed);
        let large_ms = large.last_export_ms.load(Ordering::Relaxed);
        assert!(
            large.snapshot_bytes() > small.snapshot_bytes(),
            "large {} bytes, small {} bytes",
            large.snapshot_bytes(),
            small.snapshot_bytes()
        );
        assert!(
            large_ms >= small_ms,
            "larger doc exported in {large_ms} ms, small in {small_ms} ms"
        );
        assert!(small.edge_outbox.load(Ordering::Relaxed));
        assert!(large.edge_outbox.load(Ordering::Relaxed));
        assert_eq!(
            small.save_interval(),
            super::scaled_save_interval(true, small_ms)
        );
        assert_eq!(
            large.save_interval(),
            super::scaled_save_interval(true, large_ms)
        );
        assert!(large.save_interval() >= small.save_interval());
        assert!(large.save_interval() >= Duration::from_secs(1));
        assert!(large.save_interval() <= Duration::from_secs(10));
        // The scale, independent of how fast this machine exports.
        assert_eq!(super::scaled_save_interval(true, 0), Duration::from_secs(1));
        assert_eq!(
            super::scaled_save_interval(true, 20),
            Duration::from_secs(1)
        );
        assert_eq!(
            super::scaled_save_interval(true, 100),
            Duration::from_secs(5)
        );
        assert_eq!(
            super::scaled_save_interval(true, 200),
            Duration::from_secs(10)
        );
        assert_eq!(
            super::scaled_save_interval(true, 10_000),
            Duration::from_secs(10)
        );
        assert_eq!(
            super::scaled_save_interval(false, 10_000),
            Duration::from_secs(1),
            "a local-only doc keeps the 1s floor"
        );

        let local_doc = Arc::new(SessionDoc::init("local").unwrap());
        local_doc
            .doc()
            .get_map("t")
            .insert("blob", "x".repeat(2 * 1024 * 1024).as_str())
            .unwrap();
        local_doc.doc().commit();
        let local = ChatPersistence::new(&local_doc, store, "local".into(), 0);
        local.dirty(true);
        assert!(!local.edge_outbox.load(Ordering::Relaxed));
        assert_eq!(local.save_interval(), Duration::from_secs(1));
    }

    /// Virtual clock: an edge doc whose last export took 100 ms waits 5 s.
    /// A local-only doc with the same export cost still waits 1 s.
    #[tokio::test(start_paused = true)]
    async fn edge_outbox_debounce_uses_the_scaled_interval() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(DocsStore::open(dir.path()).unwrap());
        store.initialize_chat_outbox("edge", &[]).unwrap();
        let edge_doc = Arc::new(SessionDoc::init("edge").unwrap());
        let edge = ChatPersistence::new(&edge_doc, store.clone(), "edge".into(), 0);
        // A prior export that took 100 ms → 5 s. The marker is already set.
        edge.edge_outbox.store(true, Ordering::Relaxed);
        edge.last_export_ms.store(100, Ordering::Relaxed);

        let local_doc = Arc::new(SessionDoc::init("local").unwrap());
        let local = ChatPersistence::new(&local_doc, store, "local".into(), 0);
        local.last_export_ms.store(100, Ordering::Relaxed);

        local.dirty(false);
        edge.dirty(false);
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }
        tokio::time::advance(Duration::from_secs(1)).await;
        wait_for_writes(&local, 1).await;
        assert_eq!(
            edge.writes.load(Ordering::Relaxed),
            0,
            "edge interval is 5s"
        );
        tokio::time::advance(Duration::from_millis(3_990)).await;
        assert_eq!(edge.writes.load(Ordering::Relaxed), 0);
        tokio::time::advance(Duration::from_millis(20)).await;
        wait_for_writes(&edge, 1).await;
    }

    async fn wait_for_writes(persistence: &ChatPersistence, n: usize) {
        let started = std::time::Instant::now();
        while persistence.writes.load(Ordering::Relaxed) < n {
            if started.elapsed() > Duration::from_secs(5) {
                panic!("snapshot flush did not finish");
            }
            tokio::task::yield_now().await;
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
