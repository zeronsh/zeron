//! Coalesced chat snapshots. Cursor is sampled BEFORE export; snapshot and
//! cursor commit atomically. Queue/lock waits never occupy a Tokio worker.
use crate::EngineError;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;
use tokio::sync::mpsc;
use zeron_doc::SessionDoc;
use zeron_sync::DocsStore;

/// Extra export passes a single flush_sync may take to catch a mid-flush
/// dirty before yielding to the async retry loop.
const MAX_FLUSH_PASSES: usize = 8;

const SAVE_INTERVAL: Duration = Duration::from_secs(1);

/// Unique-per-thread key for the `owner` marker — `ThreadId` cannot live
/// in an AtomicU64, so a monotonic counter hands each thread one.
fn thread_key() -> u64 {
    thread_local! {
        static KEY: u64 = THREAD_KEYS.fetch_add(1, Ordering::Relaxed) + 1;
    }
    KEY.with(|key| *key)
}
static THREAD_KEYS: AtomicU64 = AtomicU64::new(0);

/// Drops the flush-owner marker when `flush_sync` returns or unwinds.
struct FlushOwnerGuard<'a> {
    owner: &'a AtomicU64,
}
impl Drop for FlushOwnerGuard<'_> {
    fn drop(&mut self) {
        self.owner.store(0, Ordering::Release);
    }
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
    /// The thread currently inside `flush_sync` (a `thread_key`, 0 for
    /// none). Only a SAME-THREAD reentry — a commit subscriber calling
    /// `dirty` mid-flush — is deferred to the owning flush's generation
    /// recheck; every other thread takes the normal write-lock path and
    /// gets a real result. An RAII guard resets it so a panic can't
    /// leave it set.
    owner: AtomicU64,
    /// Test hook: every flush fails until `inject_flush_failure(false)`
    /// clears it.
    fail_next: AtomicBool,
    write: Mutex<()>,
    wake: Option<mpsc::Sender<()>>,
    pub(crate) initial_cursor_verified: bool,
    #[cfg(test)]
    writes: std::sync::atomic::AtomicUsize,
    #[cfg(test)]
    before_export: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    /// Test hook: fires after the export, before the save — the mid-pass
    /// parking point for the dirty-handoff test.
    #[cfg(test)]
    after_export: Mutex<Option<Box<dyn FnOnce() + Send>>>,
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
            owner: AtomicU64::new(0),
            fail_next: AtomicBool::new(false),
            write: Mutex::new(()),
            wake: runtime.as_ref().map(|_| tx),
            initial_cursor_verified: verified,
            #[cfg(test)]
            writes: std::sync::atomic::AtomicUsize::new(0),
            #[cfg(test)]
            before_export: Mutex::new(None),
            #[cfg(test)]
            after_export: Mutex::new(None),
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
        // SeqCst: the bump must be visible before the try_lock decision —
        // a failed try_lock then implies the holder's post-release
        // recheck already sees it.
        self.generation.fetch_add(1, Ordering::SeqCst);
        if immediate {
            self.urgent.store(true, Ordering::Release);
        }
        // Same-thread reentry (a commit subscriber mid-flush): mark and
        // return; the owning flush re-checks the generation and loops.
        // Another thread falls through to the normal path.
        if self.owner.load(Ordering::Acquire) == thread_key() {
            return;
        }
        if let Some(wake) = &self.wake {
            let _ = wake.try_send(());
        } else {
            // Offline synchronous tooling/tests have no executor — drain
            // inline without ever waiting on the write lock; a busy lock
            // belongs to a flush whose recheck covers this bump.
            if let Err(err) = self.drain(false) {
                tracing::debug!(chat = %self.chat_id, error = %err, "inline dirty drain");
            }
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
                let _ = job.flush_sync();
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

    /// Test hook: flushes fail until cleared — deliveries prove the retry
    /// path with a persistence failure that can't be consumed by an
    /// unrelated write.
    #[doc(hidden)]
    pub fn inject_flush_failure(&self, on: bool) {
        self.fail_next.store(on, Ordering::Release);
    }

    /// One export+save pass; the caller holds `self.write`.
    fn flush_pass(&self) -> Result<(), EngineError> {
        let flush = || -> Result<(), EngineError> {
            let generation = self.generation.load(Ordering::Acquire);
            if generation == self.saved.load(Ordering::Acquire) {
                return Ok(());
            }
            let Some(doc) = self.doc.upgrade() else {
                return Ok(());
            };
            // Never read the cursor AFTER exporting: a concurrent import
            // could then label an older snapshot with a newer cursor.
            let cursor = self.cursor.load(Ordering::Acquire);
            #[cfg(test)]
            if let Some(hook) = self
                .before_export
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take()
            {
                hook();
            }
            let result = if self.fail_next.load(Ordering::Acquire) {
                Err(EngineError::Other("injected snapshot failure".into()))
            } else {
                doc.export_snapshot()
                    .map_err(|e| EngineError::Other(e.to_string()))
                    .and_then(|bytes| {
                        #[cfg(test)]
                        if let Some(hook) = self
                            .after_export
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .take()
                        {
                            hook();
                        }
                        self.snapshot_bytes.store(bytes.len(), Ordering::Relaxed);
                        self.store
                            .save_verified_snapshot_with_cursor(
                                &self.chat_id,
                                &bytes,
                                cursor,
                                super::chat2_host::CHAT2_DOC_EPOCH,
                            )
                            .map_err(|e| EngineError::Other(e.to_string()))
                    })
            };
            match result {
                Ok(()) => {
                    #[cfg(test)]
                    self.writes.fetch_add(1, Ordering::Relaxed);
                    self.saved.store(generation, Ordering::Release);
                }
                Err(ref error) => {
                    tracing::warn!(chat = %self.chat_id, %error, "chat snapshot failed; retrying")
                }
            }
            result
        };
        flush()
    }

    /// Claim the flush-owner marker while holding `write` — the guard
    /// resets it only if this claim set it.
    fn claim_owner(&self, me: u64) -> Option<FlushOwnerGuard<'_>> {
        self.owner
            .compare_exchange(0, me, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| FlushOwnerGuard { owner: &self.owner })
    }

    /// The shared flush loop: acquire `write` (blocking for flush_sync,
    /// try_lock for inline dirty — a WouldBlock miss means the holder's
    /// post-release recheck covers this bump, while a Poisoned miss is
    /// recovered exactly like the blocking path), claim the owner marker,
    /// take one pass, release, and loop while the generation advanced.
    /// Bounded passes; an unsaved remainder is a retryable error — for a
    /// blocking caller Ok never means an unsaved generation. The inline
    /// (no-runtime, non-blocking) drain is best-effort: on a busy lock, a
    /// pass-bound exit, or a write error the remainder stays dirty for the
    /// next dirty()/flush_sync or the final shutdown flush — durability
    /// boundaries use blocking flush_sync, which reports it.
    fn drain(&self, blocking: bool) -> Result<(), EngineError> {
        let me = thread_key();
        let mut passes = 0usize;
        let result = loop {
            let pass = if blocking {
                // The lock acquisition rides block_in_place too: a queued
                // locker must not occupy a runtime worker while it waits.
                let run = || {
                    let _write = self.write.lock().unwrap_or_else(|e| e.into_inner());
                    let _owned = self.claim_owner(me);
                    self.flush_pass()
                };
                if tokio::runtime::Handle::try_current()
                    .is_ok_and(|h| h.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread)
                {
                    tokio::task::block_in_place(run)
                } else {
                    run()
                }
            } else {
                match self.write.try_lock() {
                    // A busy write lock belongs to an in-flight flush whose
                    // post-release recheck sees this bump — don't wait.
                    Err(std::sync::TryLockError::WouldBlock) => return Ok(()),
                    // A poisoned lock is recovered exactly like the
                    // blocking path's `into_inner` and proceeds as owner.
                    Err(std::sync::TryLockError::Poisoned(p)) => {
                        let _write = p.into_inner();
                        let _owned = self.claim_owner(me);
                        self.flush_pass()
                    }
                    Ok(_write) => {
                        let _owned = self.claim_owner(me);
                        self.flush_pass()
                    }
                }
            };
            passes += 1;
            if pass.is_err()
                || passes >= MAX_FLUSH_PASSES
                || self.saved.load(Ordering::Acquire) == self.generation.load(Ordering::Acquire)
            {
                break pass;
            }
        };
        result.and_then(|()| {
            if self.saved.load(Ordering::Acquire) != self.generation.load(Ordering::Acquire) {
                // Never Ok for an unsaved generation — the caller must be
                // able to retry (the async loop does).
                Err(EngineError::Other(
                    "chat snapshot flush did not catch up".into(),
                ))
            } else {
                Ok(())
            }
        })
    }

    pub(crate) fn flush_sync(&self) -> Result<(), EngineError> {
        let me = thread_key();
        if self.owner.load(Ordering::Acquire) == me {
            // Same-thread recursive call (e.g. a commit subscriber that
            // flushes): retryable — the owning pass catches the bump.
            return Err(EngineError::Other(
                "flush already in progress on this thread".into(),
            ));
        }
        // Compatibility for synchronous shutdown/eviction and command APIs.
        // Async persisters already execute this on the blocking pool.
        self.drain(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A commit subscriber that marks the persister dirty while a flush
    /// holds the write lock must not reenter it — the owning flush's
    /// generation recheck persists the change instead. No tokio runtime:
    /// `dirty` calls `flush_sync` inline.
    #[test]
    fn a_dirty_during_commit_never_recurses_into_flush() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(DocsStore::open(dir.path()).unwrap());
        let doc = Arc::new(SessionDoc::init("flush-reentry").unwrap());
        let persistence = ChatPersistence::new(&doc, store.clone(), "flush-reentry".into(), 0);
        let p = persistence.clone();
        let _sub = doc.doc().subscribe_local_update(Box::new(move |_| {
            p.dirty(true);
            true
        }));
        // A pending uncommitted edit: the flush's export commits it, which
        // fires the subscriber on this thread mid-flush.
        doc.doc().get_map("meta").insert("pending", "edit").unwrap();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let handle = std::thread::spawn(move || {
            persistence.dirty(true);
            persistence.flush_sync().expect("flush_sync");
            let _ = done_tx.send(persistence);
        });
        let persistence = done_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("dirty+flush_sync deadlocked on the commit subscriber");
        handle.join().unwrap();
        assert!(persistence.is_clean(), "the flush looped to persistence");
        // The pending edit is persisted.
        let reloaded = SessionDoc::from_doc({
            let fresh = loro::LoroDoc::new();
            fresh
                .import(&store.load_snapshot("flush-reentry").unwrap().unwrap())
                .unwrap();
            fresh
        });
        assert!(
            matches!(
                reloaded.doc().get_map("meta").get("pending"),
                Some(loro::ValueOrContainer::Value(loro::LoroValue::String(v)))
                    if v.as_str() == "edit"
            ),
            "the edit the subscriber's dirty marked is persisted"
        );
    }

    /// Inline dirty() drains like flush_sync: thread A's inline drain
    /// parks mid-pass; thread B's dirty() finds the write lock busy and
    /// returns — and A's post-release recheck persists B's edit with no
    /// further flush_sync call.
    #[test]
    fn a_busy_write_lock_hands_the_bump_to_the_holder() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(DocsStore::open(dir.path()).unwrap());
        let doc = Arc::new(SessionDoc::init("dirty-handoff").unwrap());
        let persistence = ChatPersistence::new(&doc, store.clone(), "dirty-handoff".into(), 0);
        // Park A's inline drain after its export, before its save.
        let (parked_tx, parked_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        *persistence.after_export.lock().unwrap() = Some(Box::new(move || {
            parked_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        }));
        let a = {
            let p = persistence.clone();
            std::thread::spawn(move || p.dirty(true))
        };
        parked_rx.recv().unwrap();
        // B: an edit + dirty — the write lock is busy, so B returns
        // without flushing; the bump survives for the holder's recheck.
        doc.doc().get_map("meta").insert("b-edit", "x").unwrap();
        doc.doc().commit();
        let b = {
            let p = persistence.clone();
            std::thread::spawn(move || p.dirty(true))
        };
        b.join()
            .expect("B's dirty returned without waiting on the write lock");
        release_tx.send(()).unwrap();
        a.join().expect("A's inline drain finished");
        assert!(persistence.is_clean(), "the holder drained B's bump");
        let reloaded = SessionDoc::from_doc({
            let fresh = loro::LoroDoc::new();
            fresh
                .import(&store.load_snapshot("dirty-handoff").unwrap().unwrap())
                .unwrap();
            fresh
        });
        assert!(
            matches!(
                reloaded.doc().get_map("meta").get("b-edit"),
                Some(loro::ValueOrContainer::Value(loro::LoroValue::String(v)))
                    if v.as_str() == "x"
            ),
            "B's edit persisted via the holder's recheck — no flush_sync"
        );
    }

    /// A queued blocking flush never occupies a runtime worker: with a
    /// snapshot parked holding `write`, two block_in_place waiters queue
    /// on a 2-worker runtime while an unrelated async task still runs —
    /// watched by an independent std thread so starvation can't hang the
    /// suite.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_blocked_flush_does_not_starve_the_runtime() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(DocsStore::open(dir.path()).unwrap());
        let doc = Arc::new(SessionDoc::init("flush-contention").unwrap());
        let persistence = ChatPersistence::new(&doc, store.clone(), "flush-contention".into(), 0);
        let (parked_tx, parked_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        *persistence.before_export.lock().unwrap() = Some(Box::new(move || {
            parked_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        }));
        // The parked holder flushes on the blocking pool.
        let holder = {
            let p = persistence.clone();
            tokio::task::spawn_blocking(move || {
                p.dirty(true);
                p.flush_sync()
            })
        };
        parked_rx.recv().unwrap();
        let mut waiters = Vec::new();
        for _ in 0..2 {
            let p = persistence.clone();
            waiters.push(tokio::spawn(async move {
                p.dirty(true);
                p.flush_sync()
            }));
        }
        // Unrelated async work reports to a std watchdog thread — a
        // starved runtime times out there instead of hanging the suite.
        let (proof_tx, proof_rx) = std::sync::mpsc::channel();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            let _ = proof_tx.send(());
        });
        let watchdog =
            std::thread::spawn(move || proof_rx.recv_timeout(Duration::from_secs(10)).is_ok());
        // Always release before asserting: the parked holder must never
        // pin the suite on failure.
        let survived = watchdog.join().unwrap_or(false);
        release_tx.send(()).unwrap();
        assert!(
            survived,
            "unrelated async work completed while flushers queued"
        );
        holder
            .await
            .expect("the holder joined")
            .expect("the holder flushed");
        for w in waiters {
            w.await.expect("a waiter joined").expect("a waiter flushed");
        }
        assert!(persistence.is_clean());
    }

    /// A poisoned write lock is recovered by the non-blocking path too:
    /// after a panicking export poisons `write`, an inline dirty() — no
    /// flush_sync — still persists its edit.
    #[test]
    fn a_poisoned_write_lock_is_recovered_by_inline_dirty() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(DocsStore::open(dir.path()).unwrap());
        let doc = Arc::new(SessionDoc::init("flush-poison").unwrap());
        let persistence = ChatPersistence::new(&doc, store.clone(), "flush-poison".into(), 0);
        *persistence.before_export.lock().unwrap() = Some(Box::new(|| panic!("boom")));
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            persistence.dirty(true);
            let _ = persistence.flush_sync();
        }));
        // The lock is now poisoned. An inline dirty() — the no-runtime
        // path — must recover the guard and persist, not silently defer.
        doc.doc()
            .get_map("meta")
            .insert("after-poison", "z")
            .unwrap();
        doc.doc().commit();
        persistence.dirty(true);
        let reloaded = SessionDoc::from_doc({
            let fresh = loro::LoroDoc::new();
            fresh
                .import(&store.load_snapshot("flush-poison").unwrap().unwrap())
                .unwrap();
            fresh
        });
        assert!(
            matches!(
                reloaded.doc().get_map("meta").get("after-poison"),
                Some(loro::ValueOrContainer::Value(loro::LoroValue::String(v)))
                    if v.as_str() == "z"
            ),
            "the inline dirty recovered the poisoned lock and persisted"
        );
    }

    /// Inline dirty() is best-effort: an injected save failure leaves the
    /// edit dirty, and the shutdown/close path's blocking flush_sync is
    /// what makes it durable.
    #[test]
    fn a_failed_inline_dirty_is_completed_by_the_shutdown_flush() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(DocsStore::open(dir.path()).unwrap());
        let doc = Arc::new(SessionDoc::init("flush-shutdown").unwrap());
        let persistence = ChatPersistence::new(&doc, store.clone(), "flush-shutdown".into(), 0);
        doc.doc()
            .get_map("meta")
            .insert("closing-edit", "q")
            .unwrap();
        doc.doc().commit();
        persistence.inject_flush_failure(true);
        persistence.dirty(true); // inline drain fails — best-effort
        persistence.inject_flush_failure(false);
        assert!(
            !persistence.is_clean(),
            "the failed inline drain left the edit dirty"
        );
        // The close path (doc_host's per-handle save_snapshot_result)
        // calls the blocking flush_sync, which must persist it.
        persistence.flush_sync().unwrap();
        assert!(persistence.is_clean());
        let reloaded = SessionDoc::from_doc({
            let fresh = loro::LoroDoc::new();
            fresh
                .import(&store.load_snapshot("flush-shutdown").unwrap().unwrap())
                .unwrap();
            fresh
        });
        assert!(
            matches!(
                reloaded.doc().get_map("meta").get("closing-edit"),
                Some(loro::ValueOrContainer::Value(loro::LoroValue::String(v)))
                    if v.as_str() == "q"
            ),
            "the shutdown flush persisted the edit the inline drain lost"
        );
    }

    /// A CONCURRENT flush on another thread is never deferred: it blocks
    /// behind the active flush and performs a real write — or reports
    /// the real error. An Ok from it must mean the snapshot is durable.
    #[test]
    fn a_concurrent_flush_gets_a_real_result() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(DocsStore::open(dir.path()).unwrap());
        let doc = Arc::new(SessionDoc::init("flush-concurrent").unwrap());
        let persistence = ChatPersistence::new(&doc, store.clone(), "flush-concurrent".into(), 0);
        // Park the first flush before its export.
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        *persistence.before_export.lock().unwrap() = Some(Box::new(move || {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        }));
        let first = {
            let p = persistence.clone();
            std::thread::spawn(move || {
                p.dirty(true);
                p.flush_sync()
            })
        };
        started_rx.recv().unwrap();
        // Another thread's flush waits for the parked one, then writes.
        let second = {
            let p = persistence.clone();
            std::thread::spawn(move || p.flush_sync())
        };
        // Prove it did NOT return early while the first flush is parked.
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert!(
            !second.is_finished(),
            "a concurrent flush_sync must block, not return Ok early"
        );
        doc.doc().get_map("meta").insert("notice", "text").unwrap();
        release_tx.send(()).unwrap();
        first.join().unwrap().unwrap();
        second.join().unwrap().unwrap();
        // The write the parked flush exported is durable; a clean check
        // confirms every generation flushed.
        assert!(persistence.is_clean());
        // With an injected write failure the result is Err, never Ok.
        persistence.inject_flush_failure(true);
        doc.doc().get_map("meta").insert("more", true).unwrap();
        persistence.dirty(true);
        assert!(
            persistence.flush_sync().is_err(),
            "a failed flush must report Err, not Ok"
        );
        persistence.inject_flush_failure(false);
        persistence.dirty(true);
        persistence.flush_sync().unwrap();
        assert!(persistence.is_clean());
    }

    /// A panic inside the flush must not pin the owner marker — the next
    /// flush proceeds.
    #[test]
    fn a_panicking_export_releases_the_owner_marker() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(DocsStore::open(dir.path()).unwrap());
        let doc = Arc::new(SessionDoc::init("flush-panic").unwrap());
        let persistence = ChatPersistence::new(&doc, store.clone(), "flush-panic".into(), 0);
        *persistence.before_export.lock().unwrap() = Some(Box::new(|| panic!("boom")));
        let p = persistence.clone();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            p.dirty(true);
            p.flush_sync()
        }));
        assert!(result.is_err(), "the panic surfaced");
        // The marker reset on unwind: a fresh flush proceeds and writes.
        persistence.dirty(true);
        persistence.flush_sync().unwrap();
        assert!(persistence.is_clean());
    }

    /// The first owner's panic frees the write lock; the WAITING caller
    /// acquires it, owns the export — ITS commit subscriber's dirty
    /// defers to its own pass — and completes. Deterministic: the waiter
    /// proves it was blocked on the lock before the owner unwound.
    #[test]
    fn a_waiting_flush_owns_its_subscriber_reentry_after_owner_panic() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(DocsStore::open(dir.path()).unwrap());
        let doc = Arc::new(SessionDoc::init("flush-panic-wait").unwrap());
        let persistence = ChatPersistence::new(&doc, store.clone(), "flush-panic-wait".into(), 0);
        let subscriber_ran = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let p = persistence.clone();
        let ran = subscriber_ran.clone();
        let _sub = doc.doc().subscribe_local_update(Box::new(move |_| {
            ran.fetch_add(1, Ordering::Relaxed);
            p.dirty(true);
            true
        }));
        // First owner: parks before its export while holding `write`.
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        *persistence.before_export.lock().unwrap() = Some(Box::new(move || {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            panic!("boom");
        }));
        let panicked = {
            let p = persistence.clone();
            std::thread::spawn(move || {
                p.dirty(true);
                p.flush_sync()
            })
        };
        started_rx.recv().unwrap();
        // The waiter blocks on `write` while the first owner is parked.
        let waited = {
            let p = persistence.clone();
            std::thread::spawn(move || {
                p.dirty(true);
                p.flush_sync()
            })
        };
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert!(
            !waited.is_finished(),
            "the waiter must be blocked on the write lock"
        );
        // Plant an uncommitted edit for the waiter's export to carry.
        doc.doc()
            .get_map("meta")
            .insert("waited-edit", "y")
            .unwrap();
        // Release the first owner into its panic — the waiter proceeds.
        release_tx.send(()).unwrap();
        assert!(panicked.join().is_err(), "the first flush panicked");
        waited
            .join()
            .unwrap()
            .expect("the waiting flush completed, no deadlock");
        assert!(
            subscriber_ran.load(Ordering::Relaxed) > 0,
            "the waiter's commit fired its subscriber"
        );
        assert!(persistence.is_clean());
    }

    /// A commit on another thread whose subscriber marks dirty must not
    /// wait on the write lock an in-flight flush holds — the holder's
    /// generation recheck persists the edit.
    #[test]
    fn a_subscriber_dirty_on_another_thread_never_waits_on_write() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(DocsStore::open(dir.path()).unwrap());
        let doc = Arc::new(SessionDoc::init("flush-nowait").unwrap());
        let persistence = ChatPersistence::new(&doc, store.clone(), "flush-nowait".into(), 0);
        let p = persistence.clone();
        let _sub = doc.doc().subscribe_local_update(Box::new(move |_| {
            p.dirty(true);
            true
        }));
        // A flushes, parked before its export while holding `write`.
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        *persistence.before_export.lock().unwrap() = Some(Box::new(move || {
            started_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        }));
        let flush_thread = {
            let p = persistence.clone();
            std::thread::spawn(move || {
                p.dirty(true);
                p.flush_sync()
            })
        };
        started_rx.recv().unwrap();
        // B commits an edit — its subscriber's dirty() must return
        // immediately instead of waiting on A's write lock.
        let committer = {
            let doc = doc.clone();
            std::thread::spawn(move || {
                doc.doc().get_map("meta").insert("b-edit", "x").unwrap();
                doc.doc().commit();
            })
        };
        committer
            .join()
            .expect("the commit returned while the flush held write");
        release_tx.send(()).unwrap();
        flush_thread.join().unwrap().expect("the flush finished");
        // The edit B committed is persisted by the holder's recheck.
        let reloaded = SessionDoc::from_doc({
            let fresh = loro::LoroDoc::new();
            fresh
                .import(&store.load_snapshot("flush-nowait").unwrap().unwrap())
                .unwrap();
            fresh
        });
        assert!(
            matches!(
                reloaded.doc().get_map("meta").get("b-edit"),
                Some(loro::ValueOrContainer::Value(loro::LoroValue::String(v)))
                    if v.as_str() == "x"
            ),
            "the cross-thread commit's edit is persisted"
        );
    }

    /// flush_sync called on the thread that already owns a flush returns
    /// a distinct retryable error — never an Ok pretending success.
    #[test]
    fn a_same_thread_recursive_flush_returns_err() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(DocsStore::open(dir.path()).unwrap());
        let doc = Arc::new(SessionDoc::init("flush-recursive").unwrap());
        let persistence = ChatPersistence::new(&doc, store.clone(), "flush-recursive".into(), 0);
        let p = persistence.clone();
        let (result_tx, result_rx) = std::sync::mpsc::channel();
        *persistence.before_export.lock().unwrap() = Some(Box::new(move || {
            let _ = result_tx.send(p.flush_sync());
        }));
        persistence.dirty(true);
        persistence.flush_sync().unwrap();
        let err = result_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("the recursive flush result")
            .expect_err("a recursive flush_sync must not report Ok");
        assert!(
            err.to_string().contains("already in progress"),
            "distinct retryable error: {err}"
        );
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
        let _ = persistence.flush_sync();
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
        let _ = persistence.flush_sync(); // shutdown/eviction must not await the timer
        assert_eq!(store.snapshot_cursor("whale").unwrap(), 1002);
        persistence.reset_cursor(0);
        let _ = persistence.flush_sync();
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
        let _ = job.await.unwrap();
        // The mid-export dirty advanced the generation, so the owning
        // flush loops and persists cursor 2 itself — each pass captures
        // its cursor BEFORE exporting, so the label matches the bytes.
        assert_eq!(store.snapshot_cursor("whale").unwrap(), 2);
        let reloaded = SessionDoc::from_doc({
            let fresh = loro::LoroDoc::new();
            fresh
                .import(&store.load_snapshot("whale").unwrap().unwrap())
                .unwrap();
            fresh
        });
        assert!(
            matches!(
                reloaded.doc().get_map("test").get("second"),
                Some(loro::ValueOrContainer::Value(loro::LoroValue::Bool(true)))
            ),
            "the second pass exported the edit its cursor labels"
        );
    }
}
