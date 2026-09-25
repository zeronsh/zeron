//! Coalesced chat snapshots. Cursor is sampled BEFORE export; snapshot and
//! cursor commit atomically. Queue/lock waits never occupy a Tokio worker.
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;
use tokio::sync::mpsc;
use zeron_doc::SessionDoc;
use zeron_sync::DocsStore;

const SAVE_INTERVAL: Duration = Duration::from_secs(1);

pub(crate) struct ChatPersistence {
    doc: Weak<SessionDoc>,
    store: Arc<DocsStore>,
    chat_id: String,
    cursor: AtomicU64,
    generation: AtomicU64,
    saved: AtomicU64,
    snapshot_bytes: AtomicUsize,
    urgent: AtomicBool,
    write: Mutex<()>,
    wake: Option<mpsc::Sender<()>>,
    pub(crate) initial_cursor_verified: bool,
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
            write: Mutex::new(()),
            wake: runtime.as_ref().map(|_| tx),
            initial_cursor_verified: verified,
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

    pub(crate) fn is_clean(&self) -> bool {
        self.saved.load(Ordering::Acquire) == self.generation.load(Ordering::Acquire)
    }

    pub(crate) fn snapshot_bytes(&self) -> usize {
        self.snapshot_bytes.load(Ordering::Relaxed)
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
            let deadline = tokio::time::Instant::now() + SAVE_INTERVAL;
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
            #[cfg(test)]
            if let Some(hook) = self.before_export.lock().unwrap().take() {
                hook();
            }
            let result = doc
                .export_snapshot()
                .map_err(|e| e.to_string())
                .and_then(|bytes| {
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
        tokio::time::sleep(Duration::from_millis(1200)).await;
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
}
