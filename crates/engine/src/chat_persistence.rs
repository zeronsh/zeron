//! Coalesced chat snapshots. Cursor is sampled BEFORE export; snapshot and
//! cursor commit atomically. Queue/lock waits never occupy a Tokio worker.
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;
use tokio::sync::mpsc;
use zeron_doc::SessionDoc;
use zeron_sync::DocsStore;

const SAVE_INTERVAL: Duration = Duration::from_secs(1);

/// Saved deltas stay on top of a snapshot until they pass this many bytes
/// (a quarter of the snapshot, at least 1 MiB) or [`DELTA_ROW_LIMIT`] rows;
/// then the next save writes the whole snapshot again. Bounds what a reopen
/// after a crash replays, and keeps each save at the size of what changed:
/// a 47 MB chat took 30–125 ms and 47 MB of writes per save as a snapshot,
/// and 0.1 ms and a few hundred bytes as a delta.
const DELTA_MIN_BUDGET: usize = 1024 * 1024;
const DELTA_ROW_LIMIT: usize = 512;

fn delta_budget(snapshot_bytes: usize) -> usize {
    (snapshot_bytes / 4).max(DELTA_MIN_BUDGET)
}

/// Past the budget above, a persister with a worker keeps writing deltas
/// and folds them in once the chat has been quiet this long: a whole save
/// of a big chat (47 MB) stalls every chat's commits while it writes, so it
/// waits for a pause instead of landing mid-stream.
const QUIET_FOR_COMPACTION: Duration = Duration::from_secs(2);
/// ...unless the deltas reach this many times the budget: then the save is
/// whole even mid-stream, so they can't grow without bound.
const DELTA_HARD_CAP_FACTOR: usize = 4;

/// What this process last saved in full, and the deltas on top of it.
struct DeltaState {
    /// The store's token for that full save ([`DocsStore::save_base_snapshot`]).
    base: u64,
    /// The doc's oplog version when the newest saved state was taken.
    saved_vv: loro::VersionVector,
    bytes: usize,
    rows: usize,
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
    /// Guarded by `write` in practice; its own lock keeps that obvious.
    delta: Mutex<Option<DeltaState>>,
    /// The deltas passed their budget: fold them in at the next quiet moment.
    compaction_due: AtomicBool,
    /// Deltas already on disk when this persister started (left by a crash):
    /// a compaction folds them even if the chat never changes.
    inherited_deltas: AtomicBool,
    write: Mutex<()>,
    wake: Option<mpsc::Sender<()>>,
    /// Seeded from the store, then set by the first verified write: cursor
    /// repair is once per legacy snapshot, never once per rejoin. The thin
    /// client reads the same flag from the store (client/src/live/room.rs).
    cursor_verified: AtomicBool,
    #[cfg(test)]
    writes: std::sync::atomic::AtomicUsize,
    #[cfg(test)]
    delta_writes: std::sync::atomic::AtomicUsize,
    /// Bytes handed to the store, whole saves and deltas.
    #[cfg(test)]
    bytes_written: std::sync::atomic::AtomicUsize,
    #[cfg(test)]
    before_export: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    /// Runs between a save's export and its store write.
    #[cfg(test)]
    after_export: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    /// Runs between a save's version read and its export.
    #[cfg(test)]
    after_version: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

impl ChatPersistence {
    pub(crate) fn new(
        doc: &Arc<SessionDoc>,
        store: Arc<DocsStore>,
        chat_id: String,
        cursor: u64,
    ) -> Arc<Self> {
        let verified = store.snapshot_cursor_verified(&chat_id).unwrap_or(false);
        // The state a load just found on disk, when it still is the doc's
        // base: deltas continue from it, so reopening a chat costs no whole
        // save (47 MB for the largest; one per hot chat per engine start).
        // Only a chat2 row (epoch 2: deltas can't stamp a lineage) this doc
        // grew from: the M3 path loads a pre-chat2 row, then starts a FRESH
        // doc; deltas on that row would keep its old epoch, and a crash
        // before compaction would make the next open discard it all.
        let adopted = store.adopt_loaded_base(&chat_id).filter(|loaded| {
            loaded.epoch == super::chat2_host::CHAT2_DOC_EPOCH
                && doc.doc().oplog_vv().includes_vv(&loaded.version)
        });
        let inherited = adopted.is_none()
            && store
                .snapshot_delta_stats(&chat_id)
                .is_ok_and(|(rows, _)| rows > 0);
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
            snapshot_bytes: AtomicUsize::new(adopted.as_ref().map_or(0, |loaded| loaded.base_bytes)),
            urgent: AtomicBool::new(false),
            delta: Mutex::new(adopted.as_ref().map(|loaded| DeltaState {
                base: loaded.token,
                saved_vv: loaded.version.clone(),
                bytes: loaded.delta_bytes,
                rows: loaded.delta_rows,
            })),
            inherited_deltas: AtomicBool::new(inherited),
            compaction_due: AtomicBool::new(false),
            write: Mutex::new(()),
            wake: runtime.as_ref().map(|_| tx),
            cursor_verified: AtomicBool::new(verified),
            #[cfg(test)]
            writes: std::sync::atomic::AtomicUsize::new(0),
            #[cfg(test)]
            delta_writes: std::sync::atomic::AtomicUsize::new(0),
            #[cfg(test)]
            bytes_written: std::sync::atomic::AtomicUsize::new(0),
            #[cfg(test)]
            before_export: Mutex::new(None),
            #[cfg(test)]
            after_export: Mutex::new(None),
            #[cfg(test)]
            after_version: Mutex::new(None),
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

    /// Counts changes; a save records the count it started from.
    pub(crate) fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    /// The change count the newest successful save covers.
    pub(crate) fn saved_generation(&self) -> u64 {
        self.saved.load(Ordering::Acquire)
    }

    /// Save now rather than at the end of the debounce, on the persister's
    /// own blocking-pool worker: the caller never exports or writes.
    pub(crate) fn request_flush(&self) {
        if self.is_clean() {
            return;
        }
        self.urgent.store(true, Ordering::Release);
        if let Some(wake) = &self.wake {
            let _ = wake.try_send(());
        } else {
            self.flush_unless_saving();
        }
    }

    #[cfg(test)]
    pub(crate) fn saves(&self) -> usize {
        self.writes.load(Ordering::Relaxed) + self.delta_writes.load(Ordering::Relaxed)
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
            self.flush_unless_saving();
        }
    }

    /// Without a runtime: save now, unless a save is under way. That one
    /// takes this change in (its export waits for the change to finish), and
    /// the change stays marked for the next save. Never waits for it: this
    /// runs in a change's hooks, under the doc's change lock, which that
    /// save's export is waiting for.
    fn flush_unless_saving(&self) {
        self.flush_with(false, false);
    }

    async fn run(weak: Weak<Self>, mut rx: mpsc::Receiver<()>) {
        loop {
            let compaction_due = match weak.upgrade() {
                Some(this) => this.compaction_due.load(Ordering::Acquire),
                None => return,
            };
            if compaction_due {
                tokio::select! {
                    message = rx.recv() => if message.is_none() { return; },
                    _ = tokio::time::sleep(QUIET_FOR_COMPACTION) => {
                        // Quiet: fold the deltas in now, off the commit path's
                        // busiest moments.
                        let Some(this) = weak.upgrade() else { return };
                        let permit = this.store.snapshot_writer.clone().lock_owned().await;
                        let job = this.clone();
                        if let Err(error) = tokio::task::spawn_blocking(move || {
                            let _permit = permit;
                            job.compact_sync();
                        })
                        .await
                        {
                            tracing::error!(%error, "chat snapshot compaction failed");
                        }
                        continue;
                    }
                }
            } else if rx.recv().await.is_none() {
                return;
            }
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

    /// Save what changed since the last save: a delta on top of this
    /// process's last full snapshot when it can, else the whole snapshot.
    pub(crate) fn flush_sync(&self) {
        self.flush(false);
    }

    /// Save the whole snapshot, folding any deltas into it (eviction and
    /// shutdown: a reopen then reads one blob, and a build without deltas
    /// sees everything).
    pub(crate) fn compact_sync(&self) {
        self.flush(true);
    }

    fn flush(&self, compact: bool) {
        self.flush_with(compact, true);
    }

    /// `wait`: for a save already under way (else, with one, do nothing).
    fn flush_with(&self, compact: bool, wait: bool) {
        let flush = || {
            let _write = if wait {
                self.write.lock().unwrap_or_else(|e| e.into_inner())
            } else {
                match self.write.try_lock() {
                    Ok(write) => write,
                    Err(std::sync::TryLockError::Poisoned(e)) => e.into_inner(),
                    Err(std::sync::TryLockError::WouldBlock) => return,
                }
            };
            let mut delta = self.delta.lock().unwrap_or_else(|e| e.into_inner());
            let generation = self.generation.load(Ordering::Acquire);
            let folds_deltas = compact
                && match delta.as_ref() {
                    Some(state) => state.rows > 0,
                    None => self.inherited_deltas.load(Ordering::Acquire),
                };
            if generation == self.saved.load(Ordering::Acquire) && !folds_deltas {
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
            // Read the version BEFORE exporting, for the same reason: an op
            // committed in between is then in this save AND counted as
            // unsaved, so the next delta repeats it (imports are idempotent)
            // rather than skipping it.
            let version = doc.doc().oplog_vv();
            #[cfg(test)]
            if let Some(hook) = self.after_version.lock().unwrap().take() {
                hook();
            }
            let budget = delta_budget(self.snapshot_bytes.load(Ordering::Relaxed));
            // With a worker, past the budget the save stays a delta and the
            // worker folds them in at a quiet moment; the hard cap forces it.
            // Without one (offline tools), the budget saves whole at once.
            let (byte_limit, row_limit) = if self.wake.is_some() {
                (budget * DELTA_HARD_CAP_FACTOR, DELTA_ROW_LIMIT * DELTA_HARD_CAP_FACTOR)
            } else {
                (budget, DELTA_ROW_LIMIT)
            };
            if !compact
                && let Some(state) = delta.as_mut()
                && state.bytes < byte_limit
                && state.rows < row_limit
            {
                let appended = doc
                    .export(loro::ExportMode::updates(&state.saved_vv))
                    .map_err(|e| e.to_string())
                    .and_then(|bytes| {
                        #[cfg(test)]
                        if let Some(hook) = self.after_export.lock().unwrap().take() {
                            hook();
                        }
                        self.store
                            .append_snapshot_delta(&self.chat_id, state.base, &bytes, cursor)
                            .map(|written| written.then_some(bytes.len()))
                            .map_err(|e| e.to_string())
                    });
                match appended {
                    Ok(Some(len)) => {
                        #[cfg(test)]
                        self.delta_writes.fetch_add(1, Ordering::Relaxed);
                        #[cfg(test)]
                        self.bytes_written.fetch_add(len, Ordering::Relaxed);
                        state.saved_vv = version;
                        state.bytes += len;
                        state.rows += 1;
                        if state.bytes >= budget || state.rows >= DELTA_ROW_LIMIT {
                            self.compaction_due.store(true, Ordering::Release);
                        }
                        self.cursor_verified.store(true, Ordering::Release);
                        self.saved.store(generation, Ordering::Release);
                        return;
                    }
                    // Another full write replaced our base: save in full.
                    Ok(None) => {}
                    Err(error) => {
                        tracing::warn!(chat = %self.chat_id, %error, "chat snapshot delta failed; retrying");
                        return;
                    }
                }
            }
            let result = doc
                .export_snapshot()
                .map_err(|e| e.to_string())
                .and_then(|bytes| {
                    #[cfg(test)]
                    if let Some(hook) = self.after_export.lock().unwrap().take() {
                        hook();
                    }
                    self.snapshot_bytes.store(bytes.len(), Ordering::Relaxed);
                    self.store
                        .save_base_snapshot(
                            &self.chat_id,
                            &bytes,
                            cursor,
                            super::chat2_host::CHAT2_DOC_EPOCH,
                        )
                        .map_err(|e| e.to_string())
                });
            match result {
                Ok(base) => {
                    #[cfg(test)]
                    self.writes.fetch_add(1, Ordering::Relaxed);
                    #[cfg(test)]
                    self.bytes_written
                        .fetch_add(self.snapshot_bytes.load(Ordering::Relaxed), Ordering::Relaxed);
                    // A whole save replaces every delta on disk.
                    self.inherited_deltas.store(false, Ordering::Release);
                    self.compaction_due.store(false, Ordering::Release);
                    *delta = Some(DeltaState {
                        base,
                        saved_vv: version,
                        bytes: 0,
                        rows: 0,
                    });
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

    /// Off a runtime a change's hooks save inline. A save already under
    /// way (holding the persister's write lock) waits in its export for the
    /// doc's change lock, which the change holds while its hooks run: the
    /// hook must not wait for that save. It finishes, and the save that was
    /// under way takes the change in.
    #[test]
    fn an_inline_save_from_a_changes_hooks_never_waits_for_a_save_under_way() {
        use std::sync::OnceLock;
        let raw = loro::LoroDoc::new();
        let persister: Arc<OnceLock<Arc<ChatPersistence>>> = Arc::default();
        let (started, saving) = std::sync::mpsc::channel::<()>();
        let saving = Mutex::new(saving);
        let saver: Arc<Mutex<Option<std::thread::JoinHandle<()>>>> = Arc::default();
        // Ahead of the hook: while the change's events are delivered (the
        // change lock held), a save starts on another thread and gets as
        // far as its export.
        let _pause = {
            let (persister, saver) = (persister.clone(), saver.clone());
            raw.subscribe_root(Arc::new(move |_| {
                let Some(p) = persister.get() else { return };
                if saver.lock().unwrap().is_some() {
                    return;
                }
                let p = p.clone();
                let started = started.clone();
                *saver.lock().unwrap() = Some(std::thread::spawn(move || {
                    p.generation.fetch_add(1, Ordering::Release);
                    let _ = started.send(());
                    p.flush_sync();
                }));
                saving.lock().unwrap().recv().unwrap();
                std::thread::sleep(Duration::from_millis(30));
            }))
        };
        // The hook, as the engine's: a change marks the chat dirty.
        let _hook = {
            let persister = persister.clone();
            raw.subscribe_root(Arc::new(move |_| {
                if let Some(p) = persister.get() {
                    p.dirty(false);
                }
            }))
        };
        let doc = Arc::new(SessionDoc::from_doc(raw));
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(DocsStore::open(dir.path()).unwrap());
        assert!(tokio::runtime::Handle::try_current().is_err(), "off a runtime");
        let p = ChatPersistence::new(&doc, store.clone(), "inline".into(), 0);
        let _ = persister.set(p.clone());
        let (done, finished) = std::sync::mpsc::channel();
        let writer = {
            let doc = doc.clone();
            std::thread::spawn(move || {
                doc.doc().get_map("meta").insert("note", "a change").unwrap();
                doc.commit();
                let _ = done.send(());
            })
        };
        finished
            .recv_timeout(Duration::from_secs(10))
            .expect("the change finished: its hook didn't wait for the save under way");
        writer.join().unwrap();
        saver.lock().unwrap().take().unwrap().join().unwrap();
        // What was saved has the change.
        let saved = store.load_snapshot("inline").unwrap().expect("saved");
        let reopened = loro::LoroDoc::new();
        reopened.import(&saved).unwrap();
        use loro::ToJson;
        assert_eq!(reopened.get_map("meta").get_value().to_json_value()["note"], "a change");
    }
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
        // The urgent save lands at once, as a delta on the first save.
        tokio::time::timeout(Duration::from_millis(500), async {
            while persistence.writes.load(Ordering::Relaxed)
                + persistence.delta_writes.load(Ordering::Relaxed)
                != 2
            {
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

    /// A doc of about `kb` kilobytes of text, saved once in full.
    fn saved_whale(kb: usize) -> (tempfile::TempDir, Arc<SessionDoc>, Arc<DocsStore>, Arc<ChatPersistence>) {
        let (dir, doc, store, persistence) = fixture();
        let text = doc.doc().get_text("transcript");
        // Incompressible lines: Loro compresses repeats to nothing.
        let mut seed = 0x9E37_79B9_7F4A_7C15u64;
        for _ in 0..kb {
            let line: String = (0..64)
                .map(|_| {
                    seed ^= seed << 13;
                    seed ^= seed >> 7;
                    seed ^= seed << 17;
                    format!("{seed:016x}")
                })
                .collect();
            text.insert(text.len_unicode(), &line).unwrap();
        }
        doc.doc().commit();
        persistence.applied(1, false);
        assert_eq!(persistence.writes.load(Ordering::Relaxed), 1, "first save is whole");
        (dir, doc, store, persistence)
    }

    fn stream(doc: &SessionDoc, persistence: &ChatPersistence, cursor: u64, token: &str) {
        let text = doc.doc().get_text("transcript");
        text.insert(text.len_unicode(), token).unwrap();
        doc.doc().commit();
        persistence.applied(cursor, false);
    }

    fn reopened(store: &DocsStore) -> (loro::LoroDoc, u64) {
        let (bytes, cursor, epoch) = store.load_snapshot_with_cursor("whale").unwrap().unwrap();
        assert_eq!(epoch, super::super::chat2_host::CHAT2_DOC_EPOCH);
        let doc = loro::LoroDoc::new();
        doc.import(&bytes).unwrap();
        (doc, cursor)
    }

    #[test]
    fn a_save_after_the_first_writes_what_changed_not_the_whole_doc() {
        let (_dir, doc, store, persistence) = saved_whale(2048);
        let whole = persistence.snapshot_bytes();
        assert!(whole > 1024 * 1024, "fixture too small: {whole}");
        for cursor in 2..=11 {
            stream(&doc, &persistence, cursor, "streamed tokens ");
        }
        assert_eq!(persistence.writes.load(Ordering::Relaxed), 1, "no whole-doc rewrite");
        assert_eq!(persistence.delta_writes.load(Ordering::Relaxed), 10);
        let (rows, bytes) = store.snapshot_delta_stats("whale").unwrap();
        assert_eq!(rows, 10);
        assert!(bytes < 10 * 1024, "{bytes} B of deltas for 10 short edits");
        // What a reopen reads is the doc as it is now, with its newest cursor.
        let (loaded, cursor) = reopened(&store);
        assert_eq!(loaded.get_deep_value(), doc.doc().get_deep_value());
        assert_eq!(cursor, 11);
        assert_eq!(store.snapshot_cursor("whale").unwrap(), 11);
        assert!(persistence.is_clean());
    }

    #[test]
    fn compaction_folds_deltas_into_one_snapshot() {
        let (_dir, doc, store, persistence) = saved_whale(64);
        for cursor in 2..=4 {
            stream(&doc, &persistence, cursor, "more ");
        }
        assert_eq!(store.snapshot_delta_stats("whale").unwrap().0, 3);
        persistence.compact_sync();
        assert_eq!(persistence.writes.load(Ordering::Relaxed), 2);
        assert_eq!(store.snapshot_delta_stats("whale").unwrap(), (0, 0));
        let (loaded, cursor) = reopened(&store);
        assert_eq!(loaded.get_deep_value(), doc.doc().get_deep_value());
        assert_eq!(cursor, 4);
        // Clean with no deltas: nothing to fold, nothing written.
        persistence.compact_sync();
        assert_eq!(persistence.writes.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn deltas_past_their_budget_save_the_whole_doc_again() {
        let (_dir, doc, store, persistence) = saved_whale(16);
        for cursor in 2..(2 + DELTA_ROW_LIMIT as u64) {
            stream(&doc, &persistence, cursor, "t");
        }
        assert_eq!(persistence.writes.load(Ordering::Relaxed), 1);
        assert_eq!(store.snapshot_delta_stats("whale").unwrap().0, DELTA_ROW_LIMIT as u64);
        stream(&doc, &persistence, 9_999, "t");
        assert_eq!(persistence.writes.load(Ordering::Relaxed), 2, "row limit reached: whole save");
        assert_eq!(store.snapshot_delta_stats("whale").unwrap(), (0, 0));
        // Byte budget: one delta past max(1 MiB, snapshot / 4).
        stream(&doc, &persistence, 10_000, &"y".repeat(DELTA_MIN_BUDGET + 1));
        assert_eq!(persistence.writes.load(Ordering::Relaxed), 2);
        stream(&doc, &persistence, 10_001, "t");
        assert_eq!(persistence.writes.load(Ordering::Relaxed), 3, "byte budget reached: whole save");
        let (loaded, cursor) = reopened(&store);
        assert_eq!(loaded.get_deep_value(), doc.doc().get_deep_value());
        assert_eq!(cursor, 10_001);
    }

    #[test]
    fn another_full_write_makes_the_next_save_whole() {
        let (_dir, doc, store, persistence) = saved_whale(16);
        stream(&doc, &persistence, 2, "a");
        assert_eq!(store.snapshot_delta_stats("whale").unwrap().0, 1);
        // Another writer (a rebuild, a relay heal) replaces the base.
        let other = loro::LoroDoc::new();
        other.import(&doc.export_snapshot().unwrap()).unwrap();
        store.save_snapshot_with_cursor("whale", &other.export(loro::ExportMode::Snapshot).unwrap(), 2, 2).unwrap();
        assert_eq!(store.snapshot_delta_stats("whale").unwrap(), (0, 0), "a full write clears deltas");
        stream(&doc, &persistence, 3, "b");
        assert_eq!(persistence.writes.load(Ordering::Relaxed), 2, "no delta onto a base it didn't write");
        let (loaded, cursor) = reopened(&store);
        assert_eq!(loaded.get_deep_value(), doc.doc().get_deep_value());
        assert_eq!(cursor, 3);
    }

    #[test]
    fn a_lowered_cursor_is_the_one_read_back() {
        let (_dir, doc, store, persistence) = saved_whale(16);
        stream(&doc, &persistence, 50, "a");
        persistence.reset_cursor(7);
        assert_eq!(store.snapshot_cursor("whale").unwrap(), 7);
        assert_eq!(reopened(&store).1, 7);
    }

    /// Child of [`a_kill_mid_save_loses_and_doubles_nothing`]: streams
    /// numbered ops into a doc, saving after each (deltas, and whole saves
    /// whenever a budget is reached), until it is killed.
    #[test]
    fn delta_crash_child() {
        let Some(dir) = std::env::var_os("ZERON_DELTA_CRASH_DIR") else {
            return;
        };
        let store = Arc::new(DocsStore::open(&dir).unwrap());
        let doc = Arc::new(SessionDoc::init("crash").unwrap());
        // Offline (no runtime): every change saves at once.
        let persistence = ChatPersistence::new(&doc, store, "crash".into(), 0);
        let text = doc.doc().get_text("ops");
        for op in 1u64.. {
            text.insert(text.len_unicode(), &format!("{op},")).unwrap();
            doc.doc().commit();
            persistence.applied(op, false);
        }
    }

    /// Kill the writer at arbitrary points, mid-append or mid-compaction.
    /// What reopens is exactly ops 1..=cursor: bytes and cursor commit
    /// together, and no op is lost or doubled.
    #[cfg(unix)]
    #[test]
    fn a_kill_mid_save_loses_and_doubles_nothing() {
        for (round, kill_after_ms) in [40u64, 90, 170, 260, 410].into_iter().enumerate() {
            let dir = tempfile::tempdir().unwrap();
            let mut child = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "chat_persistence::tests::delta_crash_child", "--nocapture"])
                .env("ZERON_DELTA_CRASH_DIR", dir.path())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .unwrap();
            std::thread::sleep(Duration::from_millis(kill_after_ms));
            unsafe { libc::kill(child.id() as i32, libc::SIGKILL) };
            child.wait().unwrap();
            let store = DocsStore::open(dir.path()).unwrap();
            let Some((bytes, cursor, _)) = store.load_snapshot_with_cursor("crash").unwrap() else {
                panic!("round {round}: nothing saved in {kill_after_ms} ms");
            };
            let doc = loro::LoroDoc::new();
            doc.import(&bytes).unwrap();
            let expected: String = (1..=cursor).map(|op| format!("{op},")).collect();
            assert_eq!(
                doc.get_text("ops").to_string(),
                expected,
                "round {round}: reopened state isn't ops 1..={cursor}"
            );
            let (rows, _) = store.snapshot_delta_stats("crash").unwrap();
            eprintln!("round {round}: killed at {kill_after_ms} ms, cursor {cursor}, {rows} deltas");
        }
    }

    /// A delta cut short (a torn blob) never yields a state that claims the
    /// delta's cursor: the base and its own cursor load instead.
    #[test]
    fn a_torn_delta_falls_back_to_the_base_pair() {
        let (_dir, doc, store, persistence) = saved_whale(16);
        let (base_bytes, base_cursor, _) = store.load_snapshot_with_cursor("whale").unwrap().unwrap();
        let vv = doc.doc().oplog_vv();
        stream(&doc, &persistence, 2, "a token");
        let full = doc.doc().export(loro::ExportMode::updates(&vv)).unwrap();
        // Put the base back, with a torn copy of that delta on top.
        let base = store.save_base_snapshot("whale", &base_bytes, base_cursor, 2).unwrap();
        assert!(store.append_snapshot_delta("whale", base, &full[..full.len() / 2], 2).unwrap());
        let (bytes, cursor, _) = store.load_snapshot_with_cursor("whale").unwrap().unwrap();
        assert_eq!(cursor, base_cursor);
        assert_eq!(bytes, base_bytes);
    }

    /// A store written before `snapshot_deltas` existed opens, migrates, and
    /// reads its snapshot unchanged; deltas work on it after.
    #[test]
    fn a_store_from_before_deltas_migrates_and_reads_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let doc = Arc::new(SessionDoc::init("old").unwrap());
        doc.doc().get_text("t").insert(0, "written by an older build").unwrap();
        doc.doc().commit();
        let bytes = doc.export_snapshot().unwrap();
        {
            // Make the store, then roll it back to the previous schema.
            let store = DocsStore::open(dir.path()).unwrap();
            store.save_verified_snapshot_with_cursor("old", &bytes, 42, 2).unwrap();
        }
        {
            let db = rusqlite::Connection::open(dir.path().join("docs.sqlite3")).unwrap();
            db.execute_batch(
                "DROP TABLE snapshot_deltas;
                 DELETE FROM schema_migrations WHERE version > (SELECT MAX(version) - 4 FROM schema_migrations);",
            )
            .unwrap();
        }
        let store = Arc::new(DocsStore::open(dir.path()).unwrap());
        assert_eq!(
            store.load_snapshot_with_cursor("old").unwrap().unwrap(),
            (bytes.clone(), 42, 2)
        );
        assert!(store.snapshot_cursor_verified("old").unwrap());
        let persistence = ChatPersistence::new(&doc, store.clone(), "old".into(), 42);
        doc.doc().get_text("t").insert(0, "and a newer one: ").unwrap();
        doc.doc().commit();
        persistence.applied(43, false);
        doc.doc().get_text("t").insert(0, "! ").unwrap();
        doc.doc().commit();
        persistence.applied(44, false);
        // The migrated snapshot was loaded, so both saves continue from it.
        assert_eq!(store.snapshot_delta_stats("old").unwrap().0, 2);
        let (bytes, cursor, _) = store.load_snapshot_with_cursor("old").unwrap().unwrap();
        let loaded = loro::LoroDoc::new();
        loaded.import(&bytes).unwrap();
        assert_eq!(loaded.get_deep_value(), doc.doc().get_deep_value());
        assert_eq!(cursor, 44);
    }

    /// The real thing: a privately supplied snapshot of a big chat (never
    /// written back). Streams realistic edits through deltas and a
    /// compaction; after every step, a reload equals the doc in memory.
    #[test]
    #[ignore = "requires ZERON_DELTA_WHALE: a local chat snapshot, read into a temporary store"]
    fn real_whale_reloads_equal_to_before_through_deltas() {
        let path = std::env::var("ZERON_DELTA_WHALE").unwrap();
        let raw = loro::LoroDoc::new();
        raw.import(&std::fs::read(&path).unwrap()).unwrap();
        let doc = Arc::new(SessionDoc::from_doc(raw));
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(DocsStore::open(dir.path()).unwrap());
        let persistence = ChatPersistence::new(&doc, store.clone(), "whale".into(), 0);
        let check = |label: &str| {
            let t = std::time::Instant::now();
            let (bytes, _, _) = store.load_snapshot_with_cursor("whale").unwrap().unwrap();
            let loaded = loro::LoroDoc::new();
            loaded.import(&bytes).unwrap();
            assert_eq!(loaded.oplog_vv(), doc.doc().oplog_vv(), "{label}: ops differ");
            assert_eq!(loaded.get_deep_value(), doc.doc().get_deep_value(), "{label}: state differs");
            eprintln!("{label}: reload+compare {:?}", t.elapsed());
        };
        let t = std::time::Instant::now();
        doc.doc().get_map("delta-bench").insert("opened", true).unwrap();
        doc.doc().commit();
        persistence.applied(1, false);
        eprintln!("whole save: {:?}, {} B", t.elapsed(), persistence.snapshot_bytes());
        check("after whole save");
        let text = doc.doc().get_text("delta-bench-stream");
        let mut delta_time = Duration::ZERO;
        for cursor in 2..=301u64 {
            text.insert(text.len_unicode(), &format!("streamed token {cursor} ")).unwrap();
            doc.doc().commit();
            let t = std::time::Instant::now();
            persistence.applied(cursor, false);
            delta_time += t.elapsed();
            if cursor % 100 == 0 {
                check(&format!("after {} deltas", cursor - 1));
            }
        }
        let (rows, bytes) = store.snapshot_delta_stats("whale").unwrap();
        eprintln!("300 delta saves: {delta_time:?} total, {rows} rows, {bytes} B on disk");
        assert_eq!(persistence.writes.load(Ordering::Relaxed), 1, "one whole save, the rest deltas");
        persistence.compact_sync();
        assert_eq!(store.snapshot_delta_stats("whale").unwrap(), (0, 0));
        check("after compaction");
        assert_eq!(store.snapshot_cursor("whale").unwrap(), 301);
    }

    /// Child of [`dying_between_export_and_commit_keeps_the_last_commit`]:
    /// streams ops 1.. and aborts the process inside save number
    /// ZERON_DELTA_ABORT_AT, after its export and before its store write.
    #[test]
    fn delta_abort_child() {
        let (Some(dir), Some(at)) = (
            std::env::var_os("ZERON_DELTA_CRASH_DIR"),
            std::env::var("ZERON_DELTA_ABORT_AT").ok().and_then(|v| v.parse::<u64>().ok()),
        ) else {
            return;
        };
        let store = Arc::new(DocsStore::open(&dir).unwrap());
        let doc = Arc::new(SessionDoc::init("crash").unwrap());
        let persistence = ChatPersistence::new(&doc, store, "crash".into(), 0);
        let text = doc.doc().get_text("ops");
        for op in 1u64.. {
            text.insert(text.len_unicode(), &format!("{op},")).unwrap();
            doc.doc().commit();
            if op == at {
                *persistence.after_export.lock().unwrap() = Some(Box::new(|| std::process::abort()));
            }
            persistence.applied(op, false);
        }
    }

    /// A process that dies after exporting and before committing leaves
    /// exactly the last committed state: in the first whole save, in a
    /// delta, and in the whole save that folds 512 deltas.
    #[cfg(unix)]
    #[test]
    fn dying_between_export_and_commit_keeps_the_last_commit() {
        // Op 1 is the first (whole) save; 2..=513 are deltas; 514 compacts.
        for at in [1u64, 2, 37, 513, 514, 515] {
            let dir = tempfile::tempdir().unwrap();
            let status = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "chat_persistence::tests::delta_abort_child", "--nocapture"])
                .env("ZERON_DELTA_CRASH_DIR", dir.path())
                .env("ZERON_DELTA_ABORT_AT", at.to_string())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .unwrap();
            assert!(!status.success(), "op {at}: the child should have died");
            let store = DocsStore::open(dir.path()).unwrap();
            let Some((bytes, cursor, _)) = store.load_snapshot_with_cursor("crash").unwrap() else {
                assert_eq!(at, 1, "op {at}: nothing on disk");
                continue;
            };
            assert_eq!(cursor, at - 1, "op {at}: the cursor isn't the last committed save's");
            let doc = loro::LoroDoc::new();
            doc.import(&bytes).unwrap();
            let expected: String = (1..at).map(|op| format!("{op},")).collect();
            assert_eq!(doc.get_text("ops").to_string(), expected, "op {at}");
        }
    }

    /// Ops stream on one thread, the persister saves on its own worker, and
    /// another thread forces saves (eviction, urgent paths) meanwhile. Every
    /// state on disk holds ops 1..=k with no gap or repeat, and never fewer
    /// than its cursor claims.
    #[test]
    fn deltas_stay_whole_while_the_cursor_advances_concurrently() {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(DocsStore::open(dir.path()).unwrap());
        let doc = Arc::new(SessionDoc::init("race").unwrap());
        let persistence = {
            let _guard = runtime.enter();
            ChatPersistence::new(&doc, store.clone(), "race".into(), 0)
        };
        let stop = Arc::new(AtomicBool::new(false));
        let writer = {
            let (doc, persistence, stop) = (doc.clone(), persistence.clone(), stop.clone());
            std::thread::spawn(move || {
                let text = doc.doc().get_text("ops");
                let mut op = 0u64;
                while !stop.load(Ordering::Relaxed) {
                    op += 1;
                    text.insert(text.len_unicode(), &format!("{op},")).unwrap();
                    doc.doc().commit();
                    persistence.applied(op, op % 7 == 0);
                }
                op
            })
        };
        let forcer = {
            let (persistence, stop) = (persistence.clone(), stop.clone());
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    persistence.flush_sync();
                    std::thread::sleep(Duration::from_millis(3));
                }
            })
        };
        let check = |label: &str| {
            let Some((bytes, cursor, _)) = store.load_snapshot_with_cursor("race").unwrap() else {
                return;
            };
            let loaded = loro::LoroDoc::new();
            loaded.import(&bytes).unwrap();
            let text = loaded.get_text("ops").to_string();
            let ops: Vec<u64> = text.split_terminator(',').map(|op| op.parse().unwrap()).collect();
            assert!(ops.iter().copied().eq(1..=ops.len() as u64), "{label}: gap or repeat");
            assert!(ops.len() as u64 >= cursor, "{label}: cursor {cursor} past {} ops", ops.len());
        };
        let deadline = std::time::Instant::now() + Duration::from_millis(2_500);
        let mut checks = 0;
        while std::time::Instant::now() < deadline {
            check("mid-stream");
            checks += 1;
            std::thread::sleep(Duration::from_millis(20));
        }
        stop.store(true, Ordering::Relaxed);
        let last = writer.join().unwrap();
        forcer.join().unwrap();
        persistence.flush_sync();
        check("after the last save");
        let (bytes, cursor, _) = store.load_snapshot_with_cursor("race").unwrap().unwrap();
        let loaded = loro::LoroDoc::new();
        loaded.import(&bytes).unwrap();
        assert_eq!(loaded.get_deep_value(), doc.doc().get_deep_value());
        assert_eq!(cursor, last);
        assert!(persistence.delta_writes.load(Ordering::Relaxed) > 0, "no deltas exercised");
        eprintln!(
            "{last} ops, {checks} reloads checked, {} whole saves, {} deltas",
            persistence.writes.load(Ordering::Relaxed),
            persistence.delta_writes.load(Ordering::Relaxed)
        );
        drop(runtime);
    }

    /// Live-like load, before and after: the real 49 MB chat streaming a
    /// token every 120 ms (an agent writing) beside 14 small chats edited
    /// every 2 s, through the real 1 s persister, 60 s per mode. "Before"
    /// refuses deltas, so every save is whole, as it was.
    #[test]
    #[ignore = "requires ZERON_DELTA_WHALE; 2 minutes; prints bytes/min and CPU"]
    fn live_like_bytes_and_cpu_before_and_after() {
        let path = std::env::var("ZERON_DELTA_WHALE").unwrap();
        let whale = std::fs::read(&path).unwrap();
        let cpu = || {
            let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
            unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) };
            let t = |tv: libc::timeval| tv.tv_sec as f64 + tv.tv_usec as f64 / 1e6;
            t(usage.ru_utime) + t(usage.ru_stime)
        };
        for deltas in [false, true] {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(4)
                .enable_all()
                .build()
                .unwrap();
            let dir = tempfile::tempdir().unwrap();
            let store = Arc::new(DocsStore::open(dir.path()).unwrap());
            if !deltas {
                store.stop_snapshot_deltas();
            }
            let raw = loro::LoroDoc::new();
            raw.import(&whale).unwrap();
            let mut docs = vec![Arc::new(SessionDoc::from_doc(raw))];
            for i in 0..14 {
                let doc = SessionDoc::init(&format!("small-{i}")).unwrap();
                doc.doc().get_text("body").insert(0, &"seed text ".repeat(2_000)).unwrap();
                doc.doc().commit();
                docs.push(Arc::new(doc));
            }
            let persisters: Vec<_> = {
                let _guard = runtime.enter();
                docs.iter()
                    .enumerate()
                    .map(|(i, doc)| ChatPersistence::new(doc, store.clone(), format!("doc-{i}"), 0))
                    .collect()
            };
            // Everyone's first (whole) save, outside the measured minute.
            for (doc, persistence) in docs.iter().zip(&persisters) {
                doc.doc().get_map("load").insert("opened", true).unwrap();
                doc.doc().commit();
                persistence.applied(1, true);
                persistence.flush_sync();
            }
            let bytes = |ps: &[Arc<ChatPersistence>]| ps.iter().map(|p| p.bytes_written.load(Ordering::Relaxed)).sum::<usize>();
            let (bytes0, cpu0, t0) = (bytes(&persisters), cpu(), std::time::Instant::now());
            let mut cursor = 1;
            let mut tick = 0u64;
            while t0.elapsed() < Duration::from_secs(60) {
                tick += 1;
                cursor += 1;
                let text = docs[0].doc().get_text("live-stream");
                text.insert(text.len_unicode(), &format!("{tick:06} {}", "streamed agent output ".repeat(9))).unwrap();
                docs[0].doc().commit();
                persisters[0].applied(cursor, false);
                if tick % 16 == 0 {
                    let i = 1 + (tick / 16) as usize % 14;
                    let text = docs[i].doc().get_text("body");
                    text.insert(text.len_unicode(), "a status edit ").unwrap();
                    docs[i].doc().commit();
                    persisters[i].applied(cursor, false);
                }
                std::thread::sleep(Duration::from_millis(120));
            }
            let minutes = t0.elapsed().as_secs_f64() / 60.0;
            let written = (bytes(&persisters) - bytes0) as f64 / minutes;
            let cpu_share = (cpu() - cpu0) / t0.elapsed().as_secs_f64() * 100.0;
            let whole: usize = persisters.iter().map(|p| p.writes.load(Ordering::Relaxed)).sum();
            let delta: usize = persisters.iter().map(|p| p.delta_writes.load(Ordering::Relaxed)).sum();
            eprintln!(
                "{}: {:.1} MB/min to the store, {cpu_share:.1}% of a core (incl. the streaming itself), {whole} whole saves, {delta} deltas",
                if deltas { "AFTER (deltas)" } else { "BEFORE (whole saves)" },
                written / 1048576.0
            );
            drop(persisters);
            drop(runtime);
        }
    }

    /// Deltas a crash left behind fold at the next compaction (eviction,
    /// shutdown) even if the chat never changes again in this process.
    #[test]
    fn deltas_left_by_a_crash_fold_at_the_next_compaction() {
        let dir = tempfile::tempdir().unwrap();
        {
            let store = Arc::new(DocsStore::open(dir.path()).unwrap());
            let doc = Arc::new(SessionDoc::init("left").unwrap());
            let persistence = ChatPersistence::new(&doc, store.clone(), "left".into(), 0);
            for cursor in 1..=4u64 {
                doc.doc().get_text("t").insert(0, "x").unwrap();
                doc.doc().commit();
                persistence.applied(cursor, false);
            }
            assert_eq!(store.snapshot_delta_stats("left").unwrap().0, 3);
            // Crash: no compaction, no shutdown.
        }
        let store = Arc::new(DocsStore::open(dir.path()).unwrap());
        let raw = loro::LoroDoc::new();
        raw.import(&store.load_snapshot("left").unwrap().unwrap()).unwrap();
        let doc = Arc::new(SessionDoc::from_doc(raw));
        let persistence = ChatPersistence::new(&doc, store.clone(), "left".into(), 4);
        persistence.compact_sync();
        assert_eq!(store.snapshot_delta_stats("left").unwrap(), (0, 0));
        assert_eq!(persistence.writes.load(Ordering::Relaxed), 1);
        let (bytes, cursor, _) = store.load_snapshot_with_cursor("left").unwrap().unwrap();
        let loaded = loro::LoroDoc::new();
        loaded.import(&bytes).unwrap();
        assert_eq!(loaded.get_deep_value(), doc.doc().get_deep_value());
        assert_eq!(cursor, 4);
    }

    /// On a DELTA save (not the first whole one), an op that lands between
    /// the version read and the export is in this delta and still counted
    /// unsaved: the next delta repeats it, and the reload holds it once.
    #[test]
    fn an_op_racing_a_delta_save_is_neither_lost_nor_doubled() {
        let (_dir, doc, store, persistence) = saved_whale(16);
        stream(&doc, &persistence, 2, "before ");
        assert_eq!(persistence.delta_writes.load(Ordering::Relaxed), 1);
        let racer = doc.clone();
        *persistence.after_version.lock().unwrap() = Some(Box::new(move || {
            let text = racer.doc().get_text("transcript");
            text.insert(text.len_unicode(), "RACER ").unwrap();
            racer.doc().commit();
        }));
        stream(&doc, &persistence, 3, "during ");
        assert_eq!(persistence.writes.load(Ordering::Relaxed), 1, "still on the delta path");
        let (loaded, cursor) = reopened(&store);
        assert_eq!(loaded.get_deep_value(), doc.doc().get_deep_value(), "the racing op wasn't saved");
        assert_eq!(cursor, 3);
        // The next delta repeats it (counted unsaved); the reload holds it once.
        stream(&doc, &persistence, 4, "after ");
        let (loaded, _) = reopened(&store);
        assert_eq!(loaded.get_deep_value(), doc.doc().get_deep_value());
        assert_eq!(loaded.get_text("transcript").to_string().matches("RACER").count(), 1);

        // Later still: an op after the export, before the write. Not in this
        // delta, and NOT counted saved (the version was read first), so the
        // next delta carries it. Reading the version after the export would
        // lose it.
        let late = doc.clone();
        *persistence.after_export.lock().unwrap() = Some(Box::new(move || {
            let text = late.doc().get_text("transcript");
            text.insert(text.len_unicode(), "LATE ").unwrap();
            late.doc().commit();
        }));
        stream(&doc, &persistence, 5, "then ");
        stream(&doc, &persistence, 6, "next ");
        let (loaded, cursor) = reopened(&store);
        assert_eq!(loaded.get_deep_value(), doc.doc().get_deep_value(), "an op after the export was lost");
        assert_eq!(loaded.get_text("transcript").to_string().matches("LATE").count(), 1);
        assert_eq!(cursor, 6);
        assert_eq!(persistence.writes.load(Ordering::Relaxed), 1, "all on the delta path");
    }

    /// A delta write that fails (INSERT INTO snapshot_deltas refused) leaves
    /// the save unclean and the disk as it was; the retry writes it.
    #[test]
    fn a_failed_delta_write_stays_unsaved_until_a_retry_lands() {
        let (dir, doc, store, persistence) = saved_whale(16);
        stream(&doc, &persistence, 2, "first ");
        let before = reopened(&store);
        let db = rusqlite::Connection::open(dir.path().join("docs.sqlite3")).unwrap();
        db.execute_batch("CREATE TRIGGER no_delta BEFORE INSERT ON snapshot_deltas BEGIN SELECT RAISE(FAIL, 'disk full'); END;").unwrap();
        stream(&doc, &persistence, 3, "refused ");
        assert!(!persistence.is_clean(), "a failed write counted as saved");
        let after_failure = reopened(&store);
        assert_eq!(after_failure.0.get_deep_value(), before.0.get_deep_value());
        assert_eq!(after_failure.1, 2);
        db.execute_batch("DROP TRIGGER no_delta;").unwrap();
        persistence.flush_sync();
        assert!(persistence.is_clean());
        let (loaded, cursor) = reopened(&store);
        assert_eq!(loaded.get_deep_value(), doc.doc().get_deep_value());
        assert_eq!(cursor, 3);
    }

    /// A first run saves a chat whole and then in deltas; returns its dir.
    fn saved_in_an_earlier_run(deltas: u64) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(DocsStore::open(dir.path()).unwrap());
        let doc = Arc::new(SessionDoc::init("reopen").unwrap());
        let persistence = ChatPersistence::new(&doc, store, "reopen".into(), 0);
        for cursor in 1..=(1 + deltas) {
            doc.doc().get_text("t").insert(0, &format!("{cursor} ")).unwrap();
            doc.doc().commit();
            persistence.applied(cursor, false);
        }
        dir
    }

    /// This run: open the store, load the chat as an open does.
    fn reopened_run(dir: &std::path::Path) -> (Arc<DocsStore>, Arc<SessionDoc>, u64) {
        let store = Arc::new(DocsStore::open(dir).unwrap());
        let (bytes, cursor, _) = store.load_snapshot_with_cursor("reopen").unwrap().unwrap();
        let raw = loro::LoroDoc::new();
        raw.import(&bytes).unwrap();
        (store, Arc::new(SessionDoc::from_doc(raw)), cursor)
    }

    /// Reopening a chat after a restart costs no whole save: the persister
    /// adopts what the load found on disk and writes deltas from there.
    #[test]
    fn a_reopened_chat_saves_a_delta_not_the_whole_doc() {
        for earlier_deltas in [0, 3] {
            let dir = saved_in_an_earlier_run(earlier_deltas);
            let (store, doc, cursor) = reopened_run(dir.path());
            assert_eq!(cursor, 1 + earlier_deltas);
            let persistence = ChatPersistence::new(&doc, store.clone(), "reopen".into(), cursor);
            doc.doc().get_text("t").insert(0, "after the restart ").unwrap();
            doc.doc().commit();
            persistence.applied(cursor + 1, false);
            assert_eq!(persistence.writes.load(Ordering::Relaxed), 0, "a whole save after reopening");
            assert_eq!(persistence.delta_writes.load(Ordering::Relaxed), 1);
            let (_, again, again_cursor) = reopened_run(dir.path());
            assert_eq!(again.doc().get_deep_value(), doc.doc().get_deep_value());
            assert_eq!(again_cursor, cursor + 1);
            // Inherited deltas still count toward compaction.
            persistence.compact_sync();
            assert_eq!(store.snapshot_delta_stats("reopen").unwrap(), (0, 0));
        }
    }

    /// No adopting when the load stopped at a bad delta (new deltas after it
    /// would never be read back), nor when a full write came in between.
    #[test]
    fn a_reopened_chat_saves_whole_when_its_disk_state_is_not_a_clean_base() {
        let dir = saved_in_an_earlier_run(2);
        {
            let db = rusqlite::Connection::open(dir.path().join("docs.sqlite3")).unwrap();
            db.execute(
                "INSERT INTO snapshot_deltas (doc_id, bytes, cursor, base_saved_at, seq)
                 SELECT doc_id, x'00ff', 99, saved_at,
                        (SELECT COALESCE(MAX(seq), 0) + 1 FROM snapshot_deltas WHERE doc_id = 'reopen')
                 FROM snapshots WHERE doc_id = 'reopen'",
                [],
            )
            .unwrap();
        }
        let (store, doc, cursor) = reopened_run(dir.path());
        assert_eq!(cursor, 3, "the load stops before the bad delta");
        let persistence = ChatPersistence::new(&doc, store.clone(), "reopen".into(), cursor);
        doc.doc().get_text("t").insert(0, "x").unwrap();
        doc.doc().commit();
        persistence.applied(4, false);
        assert_eq!(persistence.writes.load(Ordering::Relaxed), 1, "must save whole past a bad delta");
        assert_eq!(store.snapshot_delta_stats("reopen").unwrap(), (0, 0));

        let dir = saved_in_an_earlier_run(1);
        let (store, doc, cursor) = reopened_run(dir.path());
        store.save_snapshot_with_cursor("reopen", &doc.export_snapshot().unwrap(), cursor, 2).unwrap();
        let persistence = ChatPersistence::new(&doc, store.clone(), "reopen".into(), cursor);
        doc.doc().get_text("t").insert(0, "y").unwrap();
        doc.doc().commit();
        persistence.applied(cursor + 1, false);
        assert_eq!(persistence.writes.load(Ordering::Relaxed), 1, "a full write replaced the loaded base");
    }

    /// A streaming chat past its delta budget is not saved whole while
    /// commits keep arriving (each whole save of a big chat stalls every
    /// chat's commits); it is folded in once the chat pauses, and the hard
    /// cap still forces it mid-stream so deltas can't grow without bound.
    #[test]
    fn a_streaming_chat_is_saved_whole_only_when_quiet_or_at_the_hard_cap() {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(DocsStore::open(dir.path()).unwrap());
        let doc = Arc::new(SessionDoc::init("stream").unwrap());
        let persistence = {
            let _guard = runtime.enter();
            ChatPersistence::new(&doc, store.clone(), "stream".into(), 0)
        };
        let mut cursor = 0u64;
        let mut seed = 0x51_7cc1_b727_220a_95u64;
        let mut tick = |cursor: &mut u64| {
            // ~32 KB of incompressible text per 100 ms tick: ~320 KB/s.
            let token: String = (0..2_000)
                .map(|_| {
                    seed ^= seed << 13;
                    seed ^= seed >> 7;
                    seed ^= seed << 17;
                    format!("{seed:016x}")
                })
                .collect();
            let text = doc.doc().get_text("t");
            text.insert(text.len_unicode(), &token).unwrap();
            doc.doc().commit();
            *cursor += 1;
            persistence.applied(*cursor, false);
            std::thread::sleep(Duration::from_millis(100));
        };
        let whole = || persistence.writes.load(Ordering::Relaxed);
        // The first save is whole.
        tick(&mut cursor);
        std::thread::sleep(Duration::from_millis(1_300));
        let first = whole();
        assert_eq!(first, 1);
        // 1. Streaming past the 1 MiB budget, under the 4 MiB cap: no whole save.
        for _ in 0..80 {
            tick(&mut cursor);
        }
        assert!(persistence.compaction_due.load(Ordering::Acquire), "the fixture never passed the budget");
        assert_eq!(whole(), first, "a whole save landed while commits kept arriving");
        // 2. A pause: folded in within the quiet window.
        std::thread::sleep(QUIET_FOR_COMPACTION + Duration::from_millis(1_500));
        assert_eq!(whole(), first + 1, "the deltas weren't folded in when the chat went quiet");
        assert_eq!(store.snapshot_delta_stats("stream").unwrap(), (0, 0));
        // 3. Streaming far past the cap: it forces a whole save mid-stream.
        for _ in 0..320 {
            tick(&mut cursor);
        }
        assert!(whole() >= first + 2, "the hard cap didn't force a whole save");
        persistence.compact_sync();
        let (bytes, last, _) = store.load_snapshot_with_cursor("stream").unwrap().unwrap();
        let loaded = loro::LoroDoc::new();
        loaded.import(&bytes).unwrap();
        assert_eq!(loaded.get_deep_value(), doc.doc().get_deep_value());
        assert_eq!(last, cursor);
        drop(runtime);
    }

    /// The M3 path: an open loads a pre-chat2 row (epoch < 2), discards it,
    /// and starts a FRESH doc. Its persister must not continue that row with
    /// deltas: they would keep the old epoch, and after a crash before
    /// compaction the next open would discard everything again. Nor may it
    /// continue any row whose version the doc doesn't hold.
    #[test]
    fn a_fresh_doc_never_continues_a_row_it_did_not_grow_from() {
        let dir = tempfile::tempdir().unwrap();
        {
            let store = Arc::new(DocsStore::open(dir.path()).unwrap());
            let old = SessionDoc::init("m3").unwrap();
            old.doc().get_text("t").insert(0, "pre-chat2 history").unwrap();
            old.doc().commit();
            store.save_snapshot_with_cursor("m3", &old.export_snapshot().unwrap(), 0, 0).unwrap();
            // The open reads it (and so offers it as a base), then discards it.
            let (_, _, epoch) = store.load_snapshot_with_cursor("m3").unwrap().unwrap();
            assert_eq!(epoch, 0);
            let fresh = Arc::new(SessionDoc::init("m3").unwrap());
            let persistence = ChatPersistence::new(&fresh, store.clone(), "m3".into(), 0);
            fresh.doc().get_text("t").insert(0, "the new lineage").unwrap();
            fresh.doc().commit();
            persistence.applied(1, false);
            assert_eq!(persistence.writes.load(Ordering::Relaxed), 1, "the fresh doc wrote deltas on a pre-chat2 row");
            assert_eq!(store.snapshot_epoch("m3").unwrap(), super::super::chat2_host::CHAT2_DOC_EPOCH);
            // Crash: no compaction, no shutdown.
        }
        let store = DocsStore::open(dir.path()).unwrap();
        let (bytes, _, epoch) = store.load_snapshot_with_cursor("m3").unwrap().unwrap();
        assert_eq!(epoch, super::super::chat2_host::CHAT2_DOC_EPOCH, "the reopen would discard it (M3)");
        let loaded = loro::LoroDoc::new();
        loaded.import(&bytes).unwrap();
        assert_eq!(loaded.get_text("t").to_string(), "the new lineage");

        // An epoch-2 row the doc didn't grow from: a whole save too.
        let (store, _, _) = reopened_run(saved_in_an_earlier_run(1).path());
        let unrelated = Arc::new(SessionDoc::init("reopen").unwrap());
        let persistence = ChatPersistence::new(&unrelated, store, "reopen".into(), 0);
        unrelated.doc().get_text("t").insert(0, "x").unwrap();
        unrelated.doc().commit();
        persistence.applied(1, false);
        assert_eq!(persistence.writes.load(Ordering::Relaxed), 1);
    }
}

