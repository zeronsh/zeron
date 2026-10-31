//! The chat outbox writer: one thread that owns every `chat_outbox` INSERT for
//! local commits.
//!
//! A streaming commit used to INSERT into the outbox from inside the
//! `subscribe_local_update` hook, then INSERT the same row again through
//! `ChatClient::enqueue_batch`, each on the stream task, each behind the single
//! SQLite connection mutex that multi-MB snapshot upserts also hold. While the
//! stream task waited there it did not read the harness's stdout. Now the hook
//! only hands the update to a queue; this thread batches whatever has
//! queued into ONE transaction and then, per item in FIFO order, hands the
//! update to its chat (`OutboxSink`), which gives it to the room client without
//! a second INSERT.
//!
//! Rows are inserted before client delivery. A separate delivery thread may
//! wait for a client lock without holding up persistence. Snapshot barriers
//! wait for write attempts only; explicit shutdown flushes also wait for
//! delivery. Failed attempts invalidate the outbox bootstrap marker when the
//! snapshot is saved, retaining the full-replay recovery obligation.
//!
//! Streaming commits queue in memory. Storage stalls can extend this crash
//! window and grow the queue; there is no hard one-millisecond bound. Raw
//! agent events remain in the run journal, but boot's journal-tail recovery
//! only classifies the interrupted turn; it does not restore lost transcript
//! content or regenerate its CRDT operations.

use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError, Weak};

use zeron_sync::DocsStore;
use zeron_sync::SnapshotBarrier;

/// Rows in one transaction.
pub(crate) const MAX_BATCH: usize = 64;

/// The chat an update belongs to, told once its outbox write has been attempted.
pub(crate) trait OutboxSink: Send + Sync {
    /// Called from the writer thread, in submission order. `durable` is false
    /// when the row could not be written: the sink must keep the update for
    /// retry (the writer has already recorded the failed publication).
    fn outbox_written(&self, batch_id: String, bytes: Vec<u8>, durable: bool);
}

#[derive(Clone)]
struct Item {
    chat_id: String,
    sink: Weak<dyn OutboxSink>,
    batch_id: String,
    bytes: Vec<u8>,
}

#[derive(Default)]
struct Counts {
    submitted: u64,
    completed: u64,
    written: u64,
    /// Items per chat between `submit` and the end of their delivery.
    in_flight: HashMap<String, usize>,
}

struct Shared {
    counts: Mutex<Counts>,
    done: Condvar,
    batches: AtomicU64,
    #[cfg(test)]
    gate: Gate,
}

#[cfg(test)]
#[derive(Default)]
struct Gate {
    closed: Mutex<bool>,
    open: Condvar,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Relinquish a multithread runtime's core before blocking, as the store does.
fn block_scope<T>(f: impl FnOnce() -> T) -> T {
    if tokio::runtime::Handle::try_current()
        .is_ok_and(|h| h.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread)
    {
        tokio::task::block_in_place(f)
    } else {
        f()
    }
}

pub(crate) struct OutboxWriter {
    tx: Sender<Item>,
    shared: Arc<Shared>,
    /// Registered with the store; dropping the writer unregisters it.
    _barrier: Arc<SnapshotBarrier>,
}

impl OutboxWriter {
    pub(crate) fn new(store: Arc<DocsStore>) -> Self {
        let (tx, rx) = channel();
        let (delivery_tx, delivery_rx) = channel();
        let shared = Arc::new(Shared {
            counts: Mutex::new(Counts::default()),
            done: Condvar::new(),
            batches: AtomicU64::new(0),
            #[cfg(test)]
            gate: Gate::default(),
        });
        let thread_store = store.clone();
        let thread_shared = shared.clone();
        // Delivery can acquire the client lock. Keep it off the persistence
        // thread: snapshots may wait for disk while holding a client lock.
        let delivery_shared = shared.clone();
        std::thread::Builder::new()
            .name("outbox-delivery".into())
            .spawn(move || deliver(&delivery_shared, delivery_rx))
            .expect("start outbox delivery thread");
        std::thread::Builder::new()
            .name("outbox-writer".into())
            .spawn(move || run(&thread_store, &thread_shared, rx, delivery_tx))
            .expect("start outbox writer thread");
        let barrier_shared = shared.clone();
        let barrier: Arc<SnapshotBarrier> = Arc::new(move |_, _| {
            flush_shared(&barrier_shared, false);
            Ok(())
        });
        store.add_snapshot_barrier(&barrier);
        Self {
            tx,
            shared,
            _barrier: barrier,
        }
    }

    /// Queue without waiting for disk or client delivery. Pending memory grows
    /// during storage stalls; a bounded blocking channel would stall commits
    /// (and harness stdout reads) precisely when snapshots hold SQLite.
    pub(crate) fn submit(
        &self,
        chat_id: &str,
        sink: Weak<dyn OutboxSink>,
        batch_id: String,
        bytes: Vec<u8>,
    ) {
        let mut counts = lock(&self.shared.counts);
        // Sending under this lock makes the flush watermark match FIFO order.
        self.tx
            .send(Item {
                chat_id: chat_id.to_string(),
                sink,
                batch_id,
                bytes,
            })
            .expect("outbox writer stopped");
        counts.submitted += 1;
        *counts.in_flight.entry(chat_id.to_string()).or_default() += 1;
    }

    /// Block until every update submitted before this call has been written
    /// and handed to its chat. Must not be called with a chat's client lock
    /// held, or from `OutboxSink::outbox_written`.
    pub(crate) fn flush(&self) {
        flush_shared(&self.shared, true);
    }

    pub(crate) fn flush_writes(&self) {
        flush_shared(&self.shared, false);
    }

    /// Whether `chat_id` has updates the outbox has not taken yet.
    pub(crate) fn has_in_flight(&self, chat_id: &str) -> bool {
        lock(&self.shared.counts)
            .in_flight
            .get(chat_id)
            .is_some_and(|n| *n > 0)
    }

    /// Transactions the writer has committed (or attempted) so far.
    #[cfg(test)]
    pub(crate) fn batches(&self) -> u64 {
        self.shared.batches.load(Ordering::Relaxed)
    }

    /// Hold the writer before its next write until the guard drops.
    #[cfg(test)]
    pub(crate) fn pause(&self) -> PauseGuard<'_> {
        *lock(&self.shared.gate.closed) = true;
        PauseGuard(&self.shared)
    }
}

#[cfg(test)]
pub(crate) struct PauseGuard<'a>(&'a Shared);

#[cfg(test)]
impl Drop for PauseGuard<'_> {
    fn drop(&mut self) {
        *lock(&self.0.gate.closed) = false;
        self.0.gate.open.notify_all();
    }
}

fn flush_shared(shared: &Shared, delivery: bool) {
    block_scope(|| {
        let mut counts = lock(&shared.counts);
        let target = counts.submitted;
        while (if delivery {
            counts.completed
        } else {
            counts.written
        }) < target
        {
            counts = shared
                .done
                .wait(counts)
                .unwrap_or_else(PoisonError::into_inner);
        }
    });
}

fn run(
    store: &Arc<DocsStore>,
    shared: &Arc<Shared>,
    rx: Receiver<Item>,
    delivery: Sender<(Item, bool)>,
) {
    let mut batch = Vec::with_capacity(MAX_BATCH);
    while let Ok(first) = rx.recv() {
        #[cfg(test)]
        {
            let mut closed = lock(&shared.gate.closed);
            while *closed {
                closed = shared
                    .gate
                    .open
                    .wait(closed)
                    .unwrap_or_else(PoisonError::into_inner);
            }
        }
        batch.push(first);
        while batch.len() < MAX_BATCH {
            match rx.try_recv() {
                Ok(item) => batch.push(item),
                Err(_) => break,
            }
        }
        let items = std::mem::replace(&mut batch, Vec::with_capacity(MAX_BATCH));
        let durable = loop {
            match catch_unwind(AssertUnwindSafe(|| write_rows(store, shared, &items))) {
                Ok(durable) => break durable,
                Err(_) => {
                    // Keep the batch and its watermark outstanding. Retrying
                    // stable IDs is safe even if the transaction committed.
                    tracing::error!("outbox writer panicked; retaining and retrying batch");
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
            }
        };
        {
            let mut counts = lock(&shared.counts);
            counts.written += items.len() as u64;
        }
        shared.done.notify_all();
        for (item, durable) in items.into_iter().zip(durable) {
            delivery
                .send((item, durable))
                .expect("outbox delivery stopped");
        }
    }
}

/// One transaction for the batch; if that fails, one per row so a single
/// unwritable row (and its doc's failed-publication record) does not take the
/// rest down with it. Then, per item in order, the hand-off to its chat.
fn write_rows(store: &DocsStore, shared: &Shared, items: &[Item]) -> Vec<bool> {
    let rows: Vec<_> = items
        .iter()
        .map(|item| {
            (
                item.chat_id.as_str(),
                item.batch_id.as_str(),
                item.bytes.as_slice(),
            )
        })
        .collect();
    let durable: Vec<bool> = match store.enqueue_chat_updates(&rows) {
        Ok(()) => vec![true; items.len()],
        Err(err) => {
            tracing::warn!(%err, rows = items.len(), "outbox batch failed; retrying row by row");
            rows.iter()
                .map(|(doc, batch, bytes)| store.enqueue_chat_update(doc, batch, bytes).is_ok())
                .collect()
        }
    };
    drop(rows);
    shared.batches.fetch_add(1, Ordering::Relaxed);
    durable
}

fn deliver(shared: &Shared, rx: Receiver<(Item, bool)>) {
    while let Ok((item, durable)) = rx.recv() {
        let Item {
            chat_id,
            sink,
            batch_id,
            bytes,
        } = item;
        match sink.upgrade() {
            Some(sink) => loop {
                let delivered = catch_unwind(AssertUnwindSafe(|| {
                    sink.outbox_written(batch_id.clone(), bytes.clone(), durable);
                }));
                if delivered.is_ok() {
                    break;
                }
                tracing::error!(chat = %chat_id, "outbox hand-off panicked; retaining and retrying update");
                std::thread::sleep(std::time::Duration::from_millis(10));
            },
            // The chat was evicted. A written row is picked up by its next
            // join; an unwritten one has nowhere to be retried from.
            None if !durable => {
                tracing::error!(chat = %chat_id, "outbox write failed for an evicted chat");
            }
            None => {}
        }
        let mut counts = lock(&shared.counts);
        counts.completed += 1;
        if let Some(n) = counts.in_flight.get_mut(&chat_id) {
            *n -= 1;
            if *n == 0 {
                counts.in_flight.remove(&chat_id);
            }
        }
        shared.done.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    /// Records what the writer hands over, and what the store held at that moment.
    struct Recorder {
        store: Arc<DocsStore>,
        chat: String,
        seen: Mutex<Vec<(String, bool)>>,
        not_in_store: AtomicUsize,
    }

    impl Recorder {
        fn new(store: &Arc<DocsStore>, chat: &str) -> Arc<Self> {
            Arc::new(Self {
                store: store.clone(),
                chat: chat.into(),
                seen: Mutex::new(Vec::new()),
                not_in_store: AtomicUsize::new(0),
            })
        }
        fn weak(self: &Arc<Self>) -> Weak<dyn OutboxSink> {
            let sink: Arc<dyn OutboxSink> = self.clone();
            Arc::downgrade(&sink)
        }
    }

    impl OutboxSink for Recorder {
        fn outbox_written(&self, batch_id: String, _bytes: Vec<u8>, durable: bool) {
            // Insert before send: the row must already be readable here.
            let stored = self
                .store
                .pending_chat_updates(&self.chat)
                .unwrap()
                .iter()
                .any(|(id, _)| *id == batch_id);
            if durable && !stored {
                self.not_in_store.fetch_add(1, Ordering::Relaxed);
            }
            lock(&self.seen).push((batch_id, durable));
        }
    }

    fn store() -> (tempfile::TempDir, Arc<DocsStore>) {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(DocsStore::open(dir.path()).unwrap());
        (dir, store)
    }

    fn ids(store: &DocsStore, chat: &str) -> Vec<String> {
        store
            .pending_chat_updates(chat)
            .unwrap()
            .into_iter()
            .map(|(id, _)| id)
            .collect()
    }

    #[test]
    fn rows_follow_submission_order_and_arrive_after_their_insert() {
        let (_dir, store) = store();
        let writer = OutboxWriter::new(store.clone());
        let sink = Recorder::new(&store, "chat");
        let mut expected = Vec::new();
        for i in 0..300 {
            let id = format!("b{i:04}");
            writer.submit("chat", sink.weak(), id.clone(), vec![i as u8]);
            expected.push(id);
        }
        writer.flush();
        assert_eq!(ids(&store, "chat"), expected);
        let seen: Vec<_> = lock(&sink.seen).iter().map(|(id, _)| id.clone()).collect();
        assert_eq!(seen, expected, "hand-off follows submission order");
        assert_eq!(sink.not_in_store.load(Ordering::Relaxed), 0);
        assert!(!writer.has_in_flight("chat"));
    }

    #[test]
    fn concurrent_commits_across_chats_keep_each_chats_order_exactly_once() {
        let (_dir, store) = store();
        let writer = Arc::new(OutboxWriter::new(store.clone()));
        let chats = ["a", "b", "c"];
        let sinks: Vec<_> = chats.iter().map(|c| Recorder::new(&store, c)).collect();
        const PER_CHAT: usize = 400;
        // One producer per chat, like one commit stream per chat; the three
        // interleave freely on the shared queue.
        let threads: Vec<_> = chats
            .iter()
            .zip(&sinks)
            .map(|(chat, sink)| {
                let (writer, sink, chat) = (writer.clone(), sink.clone(), chat.to_string());
                std::thread::spawn(move || {
                    for i in 0..PER_CHAT {
                        writer.submit(&chat, sink.weak(), format!("{chat}-{i:04}"), vec![0; 8]);
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        writer.flush();
        for (chat, sink) in chats.iter().zip(&sinks) {
            let expected: Vec<String> = (0..PER_CHAT).map(|i| format!("{chat}-{i:04}")).collect();
            assert_eq!(ids(&store, chat), expected, "{chat}: ordinal order");
            let seen: Vec<_> = lock(&sink.seen).iter().map(|(id, _)| id.clone()).collect();
            assert_eq!(seen, expected, "{chat}: hand-off order, exactly once");
            assert_eq!(sink.not_in_store.load(Ordering::Relaxed), 0);
        }
    }

    #[test]
    fn a_resubmitted_batch_id_is_stored_once() {
        let (_dir, store) = store();
        let writer = OutboxWriter::new(store.clone());
        let sink = Recorder::new(&store, "chat");
        for _ in 0..3 {
            writer.submit("chat", sink.weak(), "same".into(), vec![1]);
        }
        writer.flush();
        assert_eq!(ids(&store, "chat"), vec!["same"]);
    }

    #[test]
    fn one_write_transaction_per_batch() {
        let (_dir, store) = store();
        let writer = OutboxWriter::new(store.clone());
        let sink = Recorder::new(&store, "chat");
        let before = store.outbox_write_transactions();
        {
            // Everything queues behind the paused writer, then drains as
            // full batches: 150 rows is 64 + 64 + 22.
            let _pause = writer.pause();
            for i in 0..150 {
                writer.submit("chat", sink.weak(), format!("b{i:04}"), vec![0]);
            }
        }
        writer.flush();
        assert_eq!(ids(&store, "chat").len(), 150);
        assert_eq!(writer.batches(), 3);
        assert_eq!(
            store.outbox_write_transactions() - before,
            3,
            "one transaction per batch, not one per update (and no second INSERT)"
        );
    }

    #[test]
    fn submit_does_not_touch_sqlite() {
        let (_dir, store) = store();
        let writer = OutboxWriter::new(store.clone());
        let sink = Recorder::new(&store, "chat");
        let before = store.outbox_write_transactions();
        let _pause = writer.pause();
        for i in 0..200 {
            writer.submit("chat", sink.weak(), format!("b{i}"), vec![0]);
        }
        // Submitting returned with the writer held: nothing was written on
        // this thread, and the rows are in flight, not in the outbox.
        assert_eq!(store.outbox_write_transactions(), before);
        assert!(ids(&store, "chat").is_empty());
        assert!(writer.has_in_flight("chat"));
    }

    #[test]
    fn flush_waits_for_the_queue_and_shutdown_loses_nothing() {
        let (_dir, store) = store();
        let writer = Arc::new(OutboxWriter::new(store.clone()));
        let sink = Recorder::new(&store, "chat");
        let pause = writer.pause();
        for i in 0..50 {
            writer.submit("chat", sink.weak(), format!("b{i:02}"), vec![0]);
        }
        let flushed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let waiter = {
            let (writer, flushed) = (writer.clone(), flushed.clone());
            std::thread::spawn(move || {
                writer.flush();
                flushed.store(true, Ordering::SeqCst);
            })
        };
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert!(
            !flushed.load(Ordering::SeqCst),
            "flush returned while rows were unwritten"
        );
        drop(pause);
        waiter.join().unwrap();
        assert_eq!(ids(&store, "chat").len(), 50);
    }

    #[test]
    fn dropping_the_writer_drains_what_is_queued() {
        let (_dir, store) = store();
        let sink = Recorder::new(&store, "chat");
        {
            let writer = OutboxWriter::new(store.clone());
            for i in 0..40 {
                writer.submit("chat", sink.weak(), format!("b{i:02}"), vec![0]);
            }
        }
        // The thread outlives the handle just long enough to drain.
        let started = std::time::Instant::now();
        while ids(&store, "chat").len() < 40 {
            assert!(started.elapsed().as_secs() < 5, "queued rows were dropped");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    #[test]
    fn snapshots_do_not_wait_for_client_delivery() {
        struct BlockedSink(Mutex<()>, std::sync::mpsc::Sender<()>);
        impl OutboxSink for BlockedSink {
            fn outbox_written(&self, _: String, _: Vec<u8>, _: bool) {
                self.1.send(()).unwrap();
                let _guard = lock(&self.0);
            }
        }
        let (_dir, store) = store();
        let writer = OutboxWriter::new(store.clone());
        let (entered_tx, entered_rx) = channel();
        let sink = Arc::new(BlockedSink(Mutex::new(()), entered_tx));
        let held = lock(&sink.0);
        let weak = Arc::downgrade(&(sink.clone() as Arc<dyn OutboxSink>));
        writer.submit("chat", weak.clone(), "first".into(), vec![1]);
        entered_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        // Also require a second transaction while delivery of the first is blocked.
        writer.submit("chat", weak, "second".into(), vec![2]);
        let (done_tx, done_rx) = channel();
        let snapshot_store = store.clone();
        let saver = std::thread::spawn(move || {
            snapshot_store.save_snapshot("chat", b"snapshot").unwrap();
            done_tx.send(()).unwrap();
        });
        let result = done_rx.recv_timeout(std::time::Duration::from_secs(5));
        drop(held);
        saver.join().unwrap();
        result.expect("snapshot waited for the client lock");
        writer.flush();
        assert_eq!(ids(&store, "chat"), ["first", "second"]);
    }

    #[test]
    fn panicking_delivery_is_retried_before_flush_completes() {
        struct PanicOnce(std::sync::atomic::AtomicBool, AtomicUsize);
        impl OutboxSink for PanicOnce {
            fn outbox_written(&self, _: String, _: Vec<u8>, _: bool) {
                assert!(!self.0.swap(false, Ordering::SeqCst), "injected panic");
                self.1.fetch_add(1, Ordering::SeqCst);
            }
        }
        let (_dir, store) = store();
        let writer = OutboxWriter::new(store.clone());
        let sink = Arc::new(PanicOnce(
            std::sync::atomic::AtomicBool::new(true),
            AtomicUsize::new(0),
        ));
        let weak = Arc::downgrade(&(sink.clone() as Arc<dyn OutboxSink>));
        writer.submit("chat", weak, "kept".into(), vec![1]);
        writer.flush();
        assert_eq!(sink.1.load(Ordering::SeqCst), 1);
        assert!(!writer.has_in_flight("chat"));
    }

    #[test]
    fn a_stalled_writer_does_not_block_more_than_1024_submits() {
        let (_dir, store) = store();
        let writer = OutboxWriter::new(store.clone());
        let sink = Recorder::new(&store, "chat");
        let pause = writer.pause();
        for i in 0..4096 {
            writer.submit("chat", sink.weak(), format!("b{i:04}"), vec![0]);
        }
        assert!(ids(&store, "chat").is_empty());
        drop(pause);
        writer.flush();
        assert_eq!(ids(&store, "chat").len(), 4096);
    }

    #[test]
    fn a_row_that_cannot_be_written_is_retried_alone_and_reported() {
        let (_dir, store) = store();
        let db = rusqlite::Connection::open(_dir.path().join("docs.sqlite3")).unwrap();
        db.execute_batch("CREATE TRIGGER injected BEFORE INSERT ON chat_outbox WHEN NEW.doc_id='bad' BEGIN SELECT RAISE(FAIL,'injected disk failure'); END;").unwrap();
        let writer = OutboxWriter::new(store.clone());
        let good = Recorder::new(&store, "good");
        let bad = Recorder::new(&store, "bad");
        {
            let _pause = writer.pause();
            writer.submit("good", good.weak(), "g1".into(), vec![1]);
            writer.submit("bad", bad.weak(), "x1".into(), vec![1]);
            writer.submit("good", good.weak(), "g2".into(), vec![1]);
        }
        writer.flush();
        assert_eq!(
            ids(&store, "good"),
            vec!["g1", "g2"],
            "one bad row spares the rest"
        );
        assert!(ids(&store, "bad").is_empty());
        assert_eq!(
            *lock(&good.seen),
            vec![("g1".into(), true), ("g2".into(), true)]
        );
        assert_eq!(*lock(&bad.seen), vec![("x1".to_string(), false)]);
    }

    #[test]
    fn an_evicted_chat_still_gets_its_row() {
        let (_dir, store) = store();
        let writer = OutboxWriter::new(store.clone());
        let sink = Recorder::new(&store, "gone");
        let weak = sink.weak();
        drop(sink);
        writer.submit("gone", weak, "kept".into(), vec![1]);
        writer.flush();
        assert_eq!(ids(&store, "gone"), vec!["kept"]);
    }

    #[test]
    fn snapshots_wait_for_the_outbox() {
        let (_dir, store) = store();
        let writer = Arc::new(OutboxWriter::new(store.clone()));
        let sink = Recorder::new(&store, "chat");
        let pause = writer.pause();
        writer.submit("chat", sink.weak(), "op".into(), vec![1]);
        let saver = {
            let store = store.clone();
            std::thread::spawn(move || store.save_snapshot("chat", b"contains op").unwrap())
        };
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert!(
            store.load_snapshot("chat").unwrap().is_none(),
            "a snapshot landed before the row for an operation it contains"
        );
        drop(pause);
        saver.join().unwrap();
        assert_eq!(ids(&store, "chat"), vec!["op"]);
        assert!(store.load_snapshot("chat").unwrap().is_some());
    }

    #[test]
    fn a_failed_row_is_recorded_before_the_snapshot_that_follows_it() {
        let (dir, store) = store();
        store.initialize_chat_outbox("bad", &[]).unwrap();
        let db = rusqlite::Connection::open(dir.path().join("docs.sqlite3")).unwrap();
        db.execute_batch("CREATE TRIGGER injected BEFORE INSERT ON chat_outbox WHEN NEW.doc_id='bad' BEGIN SELECT RAISE(FAIL,'injected disk failure'); END;").unwrap();
        let writer = OutboxWriter::new(store.clone());
        let sink = Recorder::new(&store, "bad");
        let pause = writer.pause();
        writer.submit("bad", sink.weak(), "x1".into(), vec![1]);
        let saver = {
            let store = store.clone();
            std::thread::spawn(move || store.save_snapshot("bad", b"contains x1").unwrap())
        };
        std::thread::sleep(std::time::Duration::from_millis(50));
        drop(pause);
        saver.join().unwrap();
        // The snapshot write retired the initialized marker, so the next open
        // replays the whole doc into the outbox: the lost row is not lost.
        assert!(!store.chat_outbox_initialized("bad").unwrap());
    }
}
