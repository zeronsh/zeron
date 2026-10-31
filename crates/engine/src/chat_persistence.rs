//! Coalesced chat snapshot delta log. Cursor is sampled BEFORE export; snapshot and
//! cursor commit atomically. Queue/lock waits never occupy a Tokio worker.
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;
use tokio::sync::mpsc;
use zeron_doc::SessionDoc;
use zeron_sync::DocsStore;

const SAVE_INTERVAL: Duration = Duration::from_secs(1);
const QUIET_INTERVAL: Duration = Duration::from_secs(30);
const MAX_DELTAS: usize = 256;
const MIN_DELTA_BYTES: usize = 512 * 1024;

#[derive(Default)]
struct WriteState {
    // None forces a full first write. In particular, recovered outbox ops
    // must never be mistaken for operations already persisted in this log.
    saved_vv: Option<loro::VersionVector>,
    delta_count: usize,
    delta_bytes: usize,
    verifications: u64,
    needs_compaction: bool,
}

pub(crate) struct ChatPersistence {
    doc: Weak<SessionDoc>,
    store: Arc<DocsStore>,
    chat_id: String,
    cursor: AtomicU64,
    generation: AtomicU64,
    saved: AtomicU64,
    snapshot_bytes: AtomicUsize,
    delta_bytes: AtomicUsize,
    urgent: AtomicBool,
    write: Mutex<WriteState>,
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
            delta_bytes: AtomicUsize::new(0),
            urgent: AtomicBool::new(false),
            write: Mutex::new(WriteState::default()),
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

    /// Called with the imported disk state BEFORE replaying the independent
    /// outbox. Outbox-only operations must be included in the next delta.
    pub(crate) fn restore_base(
        &self,
        vv: loro::VersionVector,
        snapshot_bytes: usize,
        delta_count: usize,
        delta_bytes: usize,
        needs_compaction: bool,
    ) {
        let mut write = self.write.lock().unwrap_or_else(|e| e.into_inner());
        write.saved_vv = if needs_compaction || !self.cursor_verified() {
            None
        } else {
            Some(vv)
        };
        write.needs_compaction = needs_compaction;
        write.delta_count = delta_count;
        write.delta_bytes = delta_bytes;
        self.snapshot_bytes.store(snapshot_bytes, Ordering::Relaxed);
        self.delta_bytes.store(delta_bytes, Ordering::Relaxed);
        if (delta_count > 0 || needs_compaction)
            && let Some(wake) = &self.wake
        {
            let _ = wake.try_send(());
        }
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
        // Keep the host's warm-document budget aware of growth between
        // compactions, without taking the persistence lock on admission.
        self.snapshot_bytes
            .load(Ordering::Relaxed)
            .saturating_add(self.delta_bytes.load(Ordering::Relaxed))
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
            self.flush(false);
        }
    }

    async fn run(weak: Weak<Self>, mut rx: mpsc::Receiver<()>) {
        let mut compact_at = None;
        loop {
            let compact = tokio::select! {
                message = rx.recv() => if message.is_none() { return } else { false },
                _ = async { tokio::time::sleep_until(compact_at.unwrap()).await }, if compact_at.is_some() => true,
            };
            if !compact {
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
            }
            let Some(this) = weak.upgrade() else { return };
            // All chat persisters sharing this SQLite connection queue
            // asynchronously, BEFORE occupying a blocking-pool thread.
            let permit = this.store.snapshot_writer.clone().lock_owned().await;
            let job = this.clone();
            if let Err(error) = tokio::task::spawn_blocking(move || {
                let _permit = permit;
                job.flush(compact);
            })
            .await
            {
                tracing::error!(%error, "chat snapshot worker failed");
            }
            compact_at = {
                let write = this.write.lock().unwrap_or_else(|e| e.into_inner());
                (write.delta_count > 0 || write.needs_compaction)
                    .then(|| tokio::time::Instant::now() + QUIET_INTERVAL)
            };
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
        self.flush(true);
    }

    fn flush(&self, compact: bool) {
        let flush = || {
            let mut write = self.write.lock().unwrap_or_else(|e| e.into_inner());
            let generation = self.generation.load(Ordering::Acquire);
            if generation == self.saved.load(Ordering::Acquire)
                && !(compact && write.delta_count > 0)
                && !write.needs_compaction
            {
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
            let vv_now = doc.doc().oplog_vv();
            let unsaved = generation != self.saved.load(Ordering::Acquire);
            let result = (|| -> Result<(), String> {
                let delta = write
                    .saved_vv
                    .as_ref()
                    .map(|vv| doc.doc().export(loro::ExportMode::updates(vv)))
                    .transpose()
                    .map_err(|e| e.to_string())?;
                let roots = DocsStore::snapshot_roots(doc.doc()).map_err(|e| e.to_string())?;
                let want_full = compact
                    || delta.as_ref().is_none_or(|bytes| {
                        write.delta_count + 1 > MAX_DELTAS
                            || write.delta_bytes + bytes.len() + roots.len()
                                > MIN_DELTA_BYTES
                                    .max(self.snapshot_bytes.load(Ordering::Relaxed) / 2)
                    });
                let mut full_saved = false;
                if want_full {
                    if write.needs_compaction {
                        // The chain being replaced has rows that failed
                        // validation. Keep a copy first; the repair must
                        // not proceed even if the copy cannot be made (a
                        // full disk would fail the save too), but must say so.
                        match self
                            .store
                            .quarantine_delta_suffix(&self.chat_id, write.delta_count)
                        {
                            Ok(rows) => tracing::error!(
                                chat = %self.chat_id, rows,
                                "damaged snapshot delta rows quarantined; replacing the chain"
                            ),
                            Err(error) => tracing::error!(
                                chat = %self.chat_id, %error,
                                "could not quarantine damaged snapshot delta rows"
                            ),
                        }
                    }
                    match self.save_full(&doc, cursor) {
                        Ok(()) => {
                            write.delta_count = 0;
                            write.delta_bytes = 0;
                            write.needs_compaction = false;
                            full_saved = true;
                        }
                        // A failed compaction (a large write: disk full, a
                        // busy timeout) must not cost the unsaved edits when
                        // the small delta can still land. The chain is
                        // allowed to overshoot its limits only until the
                        // next successful compaction, and by at most 2x.
                        Err(error)
                            if unsaved
                                && !write.needs_compaction
                                && write.delta_count < 2 * MAX_DELTAS
                                && delta.is_some() =>
                        {
                            tracing::warn!(chat = %self.chat_id, %error,
                                "snapshot compaction failed; appending a delta instead");
                        }
                        Err(error) => return Err(error),
                    }
                }
                if !full_saved {
                    let bytes = delta.expect("a delta chain always has a saved version");
                    self.store
                        .append_snapshot_delta(&self.chat_id, &bytes, cursor, &roots)
                        .map_err(|e| e.to_string())?;
                    write.delta_count += 1;
                    write.delta_bytes += bytes.len() + roots.len();
                    write.verifications += 1;
                    // The oracle uses the full snapshot at exactly the
                    // persisted frontier, so a racing edit is not a mismatch.
                    if cfg!(debug_assertions)
                        || (std::env::var("ZERON_PERF_VERIFY").as_deref() == Ok("1")
                            && write.verifications % 32 == 0)
                    {
                        if let Err(error) = self.verify_full(&doc, &vv_now) {
                            tracing::error!(chat = %self.chat_id, %error, "snapshot delta mismatch; using full snapshot");
                            self.save_full(&doc, cursor)?;
                            write.delta_count = 0;
                            write.delta_bytes = 0;
                        }
                    }
                }
                // Sampled before export: later operations may be included and
                // are harmlessly re-exported next time. Never skip an op.
                write.saved_vv = Some(vv_now);
                Ok(())
            })();
            self.delta_bytes.store(write.delta_bytes, Ordering::Relaxed);
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

    fn save_full(&self, doc: &SessionDoc, cursor: u64) -> Result<(), String> {
        let bytes = doc.export_snapshot().map_err(|e| e.to_string())?;
        self.store
            .save_verified_snapshot_with_cursor(
                &self.chat_id,
                &bytes,
                cursor,
                super::chat2_host::CHAT2_DOC_EPOCH,
            )
            .map_err(|e| e.to_string())?;
        self.snapshot_bytes.store(bytes.len(), Ordering::Relaxed);
        Ok(())
    }

    fn verify_full(&self, doc: &SessionDoc, vv_now: &loro::VersionVector) -> Result<(), String> {
        let restored = self
            .store
            .load_chat_state(&self.chat_id)
            .map_err(|e| e.to_string())?
            .ok_or("missing persisted doc")?;
        if !restored.doc.oplog_vv().includes_vv(vv_now) {
            return Err("delta replay omitted sampled operations".into());
        }
        // SnapshotAt temporarily checks out the document. Only use it on an
        // isolated full copy, never on the live doc that may still stream.
        let bytes = doc.export_snapshot().map_err(|e| e.to_string())?;
        let mut full = loro::LoroDoc::new();
        full.import(&bytes).map_err(|e| e.to_string())?;
        if full.oplog_vv() != restored.doc.oplog_vv() {
            let bytes = full
                .export(loro::ExportMode::snapshot_at(
                    &restored.doc.state_frontiers(),
                ))
                .map_err(|e| e.to_string())?;
            full = loro::LoroDoc::new();
            full.import(&bytes).map_err(|e| e.to_string())?;
        }
        if full.oplog_vv() != restored.doc.oplog_vv()
            || full.get_deep_value() != restored.doc.get_deep_value()
        {
            return Err("full snapshot differs from delta replay".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn seeded_ascii(bytes: usize) -> String {
        let mut rng = 0x12_ab_cd_ef_u64;
        (0..bytes)
            .map(|_| {
                rng ^= rng << 13;
                rng ^= rng >> 7;
                rng ^= rng << 17;
                (32 + rng % 95) as u8 as char
            })
            .collect()
    }

    fn assert_state(store: &DocsStore, doc: &SessionDoc, cursor: u64) -> zeron_sync::ChatState {
        let state = store.load_chat_state("whale").unwrap().unwrap();
        let full = loro::LoroDoc::new();
        full.import(&doc.export_snapshot().unwrap()).unwrap();
        assert_eq!(state.doc.oplog_vv(), full.oplog_vv());
        assert_eq!(state.doc.get_deep_value(), full.get_deep_value());
        assert_eq!(state.cursor, cursor);
        state
    }

    #[test]
    fn delta_count_and_byte_thresholds_compact_without_losing_cursor() {
        let (_dir, doc, store, p) = fixture();
        p.applied(1, false); // first save establishes a base
        for i in 0..MAX_DELTAS {
            doc.doc().get_map("counter").insert("i", i as i64).unwrap();
            doc.doc().commit();
            p.applied(i as u64 + 2, false);
        }
        assert_eq!(assert_state(&store, &doc, 257).delta_count, MAX_DELTAS);
        p.applied(258, false);
        assert_eq!(assert_state(&store, &doc, 258).delta_count, 0);
        doc.doc()
            .get_text("big")
            .insert(0, &seeded_ascii(MIN_DELTA_BYTES + 1000))
            .unwrap();
        doc.doc().commit();
        p.applied(259, false);
        assert_eq!(assert_state(&store, &doc, 259).delta_count, 0);
        doc.doc().get_map("counter").insert("last", true).unwrap();
        doc.doc().commit();
        p.applied(260, false);
        assert_eq!(assert_state(&store, &doc, 260).delta_count, 1);
        assert!(p.is_clean());
        p.flush_sync(); // clean deltas still compact on shutdown/eviction
        assert_eq!(assert_state(&store, &doc, 260).delta_count, 0);
    }

    #[test]
    fn restored_base_exports_recovered_outbox_ops_and_repairs_torn_suffix() {
        let (dir, doc, store, p) = fixture();
        p.applied(1, false);
        doc.doc().get_text("text").insert(0, "saved").unwrap();
        doc.doc().commit();
        p.applied(2, false);
        let state = store.load_chat_state("whale").unwrap().unwrap();
        let vv = state.doc.oplog_vv();
        let reopened = Arc::new(SessionDoc::from_doc(state.doc));
        // This import happened before the host subscribed the new persister.
        reopened.doc().get_text("text").insert(0, "outbox").unwrap();
        reopened.doc().commit();
        let next = ChatPersistence::new(&reopened, store.clone(), "whale".into(), state.cursor);
        next.restore_base(
            vv,
            state.snapshot_bytes,
            state.delta_count,
            state.delta_bytes,
            state.needs_compaction,
        );
        next.applied(3, false);
        assert_eq!(assert_state(&store, &reopened, 3).delta_count, 2);
        // Corrupt the first delta; reopening must use the base and mark that
        // the suffix must be cleared before the next append (also in release).
        let db = rusqlite::Connection::open(dir.path().join("docs.sqlite3")).unwrap();
        db.execute("UPDATE snapshot_deltas SET bytes=X'00' WHERE seq=1", [])
            .unwrap();
        let state = store.load_chat_state("whale").unwrap().unwrap();
        assert!(state.needs_compaction);
        assert_eq!(state.delta_count, 0);
        let vv = state.doc.oplog_vv();
        let repaired = Arc::new(SessionDoc::from_doc(state.doc));
        let next = ChatPersistence::new(&repaired, store.clone(), "whale".into(), state.cursor);
        next.restore_base(
            vv,
            state.snapshot_bytes,
            state.delta_count,
            state.delta_bytes,
            state.needs_compaction,
        );
        repaired
            .doc()
            .get_map("after-crash")
            .insert("saved", true)
            .unwrap();
        repaired.doc().commit();
        next.applied(4, false);
        assert_eq!(assert_state(&store, &repaired, 4).delta_count, 0);
        // The damaged chain was copied aside before the repair replaced it.
        let quarantined: Vec<(i64, Vec<u8>)> = db
            .prepare(
                "SELECT seq,bytes FROM snapshot_delta_quarantine WHERE doc_id='whale' ORDER BY seq",
            )
            .unwrap()
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(quarantined.len(), 2);
        assert_eq!(quarantined[0], (1, vec![0]));
        assert_eq!(quarantined[1].0, 2);
    }

    #[test]
    fn equivalence_oracle_replaces_an_incorrect_delta_with_full_state() {
        let (_dir, doc, store, p) = fixture();
        p.applied(1, false);
        doc.doc()
            .get_text("text")
            .insert(0, "must survive")
            .unwrap();
        doc.doc().commit();
        // Simulate an incorrectly advanced incremental cache. The oracle
        // must contain the mistake by replacing the log with a full save.
        p.write.lock().unwrap().saved_vv = Some(doc.doc().oplog_vv());
        p.applied(2, false);
        assert!(p.is_clean());
        assert_eq!(assert_state(&store, &doc, 2).delta_count, 0);
    }

    #[test]
    fn failed_delta_retries_from_last_durable_vv() {
        let (dir, doc, store, p) = fixture();
        p.applied(1, false);
        let db = rusqlite::Connection::open(dir.path().join("docs.sqlite3")).unwrap();
        db.execute_batch("CREATE TRIGGER fail_delta BEFORE INSERT ON snapshot_deltas BEGIN SELECT RAISE(ABORT,'disk full'); END").unwrap();
        doc.doc().get_text("text").insert(0, "first").unwrap();
        doc.doc().commit();
        p.applied(2, false);
        assert!(!p.is_clean());
        doc.doc().get_text("text").insert(0, "second").unwrap();
        doc.doc().commit();
        db.execute_batch("DROP TRIGGER fail_delta").unwrap();
        p.applied(3, false);
        assert_eq!(assert_state(&store, &doc, 3).delta_count, 1);
        assert!(p.is_clean());
    }

    #[test]
    fn failed_compaction_still_lands_unsaved_edits_as_a_delta() {
        let (dir, doc, store, p) = fixture();
        let db = rusqlite::Connection::open(dir.path().join("docs.sqlite3")).unwrap();
        p.applied(1, false);
        doc.doc().get_text("text").insert(0, "first").unwrap();
        doc.doc().commit();
        p.applied(2, false);
        // The edit below cannot be appended, so it is unsaved at shutdown.
        db.execute_batch("CREATE TRIGGER fail_delta BEFORE INSERT ON snapshot_deltas BEGIN SELECT RAISE(ABORT,'busy'); END").unwrap();
        doc.doc().get_text("text").insert(0, "second").unwrap();
        doc.doc().commit();
        p.applied(3, false);
        assert!(!p.is_clean());
        // Shutdown compaction fails too (it must delete the existing row).
        db.execute_batch(
            "DROP TRIGGER fail_delta;
             CREATE TRIGGER fail_compact BEFORE DELETE ON snapshot_deltas BEGIN SELECT RAISE(ABORT,'disk full'); END",
        )
        .unwrap();
        p.flush_sync();
        assert!(p.is_clean(), "the delta fallback made the edit durable");
        assert_eq!(assert_state(&store, &doc, 3).delta_count, 2);
        // A clean chain whose compaction fails stays as it is.
        p.flush_sync();
        assert_eq!(assert_state(&store, &doc, 3).delta_count, 2);
        db.execute_batch("DROP TRIGGER fail_compact").unwrap();
        p.flush_sync();
        assert_eq!(assert_state(&store, &doc, 3).delta_count, 0);
    }

    #[tokio::test(start_paused = true)]
    async fn quiet_compacts_after_thirty_seconds_and_keeps_one_second_save() {
        let (_dir, doc, store, p) = fixture();
        p.applied(1, false);
        p.flush(false);
        doc.doc().get_text("text").insert(0, "stream").unwrap();
        doc.doc().commit();
        p.applied(2, false);
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_millis(999)).await;
        assert_eq!(store.load_chat_state("whale").unwrap().unwrap().cursor, 1);
        tokio::time::advance(Duration::from_millis(1)).await;
        let start = std::time::Instant::now();
        while p.writes.load(Ordering::Relaxed) < 2 {
            assert!(start.elapsed() < Duration::from_secs(10));
            tokio::task::yield_now().await;
        }
        assert_eq!(assert_state(&store, &doc, 2).delta_count, 1);
        // Let run() observe completion and arm its idle deadline.
        for _ in 0..20 {
            tokio::task::yield_now().await;
        }
        tokio::time::advance(Duration::from_secs(29)).await;
        assert_eq!(
            store.load_chat_state("whale").unwrap().unwrap().delta_count,
            1
        );
        tokio::time::advance(Duration::from_secs(1)).await;
        let start = std::time::Instant::now();
        while p.writes.load(Ordering::Relaxed) < 3 {
            assert!(start.elapsed() < Duration::from_secs(10));
            tokio::task::yield_now().await;
        }
        assert_eq!(assert_state(&store, &doc, 2).delta_count, 0);
    }

    #[tokio::test]
    async fn streaming_five_mb_store_write_volume() {
        let (_dir, doc, store, p) = fixture();
        let baseline_dir = tempfile::tempdir().unwrap();
        let baseline = DocsStore::open(baseline_dir.path()).unwrap();
        // Loro stores this text in state and operation history: 2.5 MB of
        // content makes a measured ~5 MB serialized snapshot.
        let text = seeded_ascii(2_500_000);
        doc.doc().get_text("history").insert(0, &text).unwrap();
        doc.doc().commit();
        p.applied(0, false);
        p.flush(false);
        let base_bytes = p.snapshot_bytes();
        let before = store.snapshot_payload_bytes_written();
        let seconds = 12u64;
        for second in 1..=seconds {
            // Eight stream commits per simulated second, one persistence tick.
            for _ in 0..8 {
                let text = doc.doc().get_text("stream");
                text.insert(text.len_unicode(), &"token🦀".repeat(12))
                    .unwrap();
                doc.doc().commit();
                p.applied(second, false);
            }
            p.flush(false);
            baseline
                .save_verified_snapshot_with_cursor(
                    "whale",
                    &doc.export_snapshot().unwrap(),
                    second,
                    2,
                )
                .unwrap();
        }
        let full = baseline.snapshot_payload_bytes_written();
        let delta = store.snapshot_payload_bytes_written() - before;
        eprintln!(
            "5MB_STREAM base_bytes={base_bytes} seconds={seconds} full_bytes={full} delta_bytes={delta} full_bytes_per_second={} delta_bytes_per_second={}",
            full / seconds,
            delta / seconds
        );
        assert!((5_000_000..5_010_000).contains(&base_bytes));
        assert!(delta / seconds < 50_000);
        assert!(full > delta * 100);
        assert_eq!(
            assert_state(&store, &doc, seconds).delta_count,
            seconds as usize
        );
        let before_shutdown = store.snapshot_payload_bytes_written();
        p.flush_sync();
        eprintln!(
            "5MB_STREAM shutdown_compaction_bytes={}",
            store.snapshot_payload_bytes_written() - before_shutdown
        );
        assert_eq!(assert_state(&store, &doc, seconds).delta_count, 0);
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
        persistence.applied(0, false);
        persistence.flush(false); // establish the base, then exercise a delta export
        persistence.applied(1, false);
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        *persistence.before_export.lock().unwrap() = Some(Box::new(move || {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        }));
        let p = persistence.clone();
        let job = tokio::task::spawn_blocking(move || p.flush(false));
        started_rx.recv().unwrap();
        doc.doc().get_map("test").insert("second", true).unwrap();
        doc.doc().commit();
        persistence.applied(2, false);
        release_tx.send(()).unwrap();
        job.await.unwrap();
        assert_eq!(
            store.load_chat_state("whale").unwrap().unwrap().cursor,
            1,
            "cursor must have been captured before export"
        );
        persistence.flush_sync();
        assert_eq!(store.snapshot_cursor("whale").unwrap(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn initialized_outbox_keeps_snapshot_deadline_and_shutdown_flush() {
        let (_dir, doc, store, persistence) = fixture();
        store.initialize_chat_outbox("whale", &[]).unwrap();
        doc.doc().get_map("test").insert("remote", 1).unwrap();
        doc.doc().commit();
        persistence.applied(1, false);
        tokio::task::yield_now().await;
        tokio::time::advance(Duration::from_secs(1)).await;
        let start = std::time::Instant::now();
        while persistence.writes.load(Ordering::Relaxed) == 0 {
            assert!(
                start.elapsed() < Duration::from_secs(5),
                "snapshot did not finish"
            );
            tokio::task::yield_now().await;
        }
        let (bytes, cursor, _) = store.load_snapshot_with_cursor("whale").unwrap().unwrap();
        assert_eq!(cursor, 1);
        let restored = loro::LoroDoc::new();
        restored.import(&bytes).unwrap();
        assert_eq!(
            restored.get_map("test").get_deep_value(),
            doc.doc().get_map("test").get_deep_value()
        );

        doc.doc().get_map("test").insert("shutdown", 2).unwrap();
        doc.doc().commit();
        persistence.applied(2, false);
        persistence.flush_sync();
        let (bytes, cursor, _) = store.load_snapshot_with_cursor("whale").unwrap().unwrap();
        assert_eq!(cursor, 2);
        let restored = loro::LoroDoc::new();
        restored.import(&bytes).unwrap();
        assert_eq!(
            restored.get_map("test").get_deep_value(),
            doc.doc().get_map("test").get_deep_value()
        );
    }
}
