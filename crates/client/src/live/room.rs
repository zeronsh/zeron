//! One chat's chat2 room membership: the viewer sink (remote rows → the
//! session doc), durable local publication (own commands/queue rows → the
//! outbox → the room), debounced snapshot+cursor persistence, and the join
//! loop that NEVER gives up (the legacy one-shot join left a chat dark until
//! relaunch).
//!
//! Shape follows the engine's `EngineChatSink` / `ChatPersistence`, minus
//! host duties (checkpoint posting, command execution).

use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use futures::future::BoxFuture;
use loro::VersionVector;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use zeron_doc::SessionDoc;
use zeron_proto::relay_gen::RELAY_ROOM_GEN;
use zeron_sync::chat_client::{ChatTransport, RowImportOutcome};
use zeron_sync::chat_relay::{RelayHandle, RelayImport, RelayPolicy};
use zeron_sync::{
    ChatClient, ChatDocSink, ChatEvent, CheckpointFetcher, DocsStore, SyncError, UrlProvider,
};

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

/// The journal key for relayed updates that arrived ahead of their history (the
/// engine uses the same key).
pub(crate) fn parked_inbox(chat_id: &str) -> String {
    format!("{chat_id}#parked")
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
    // Updates that came ahead of their history. Once all are in, none is needed again.
    let parked_key = parked_inbox(chat_id);
    if let Ok((parked, newest)) = store.pending_chat_inbox(&parked_key)
        && !parked.is_empty()
    {
        let mut waiting = false;
        for bytes in &parked {
            match raw.import(bytes) {
                Ok(status) => waiting |= status.pending.is_some(),
                Err(err) => tracing::warn!(chat = %chat_id, error = %err, "couldn't replay a parked relay update"),
            }
        }
        if !waiting {
            let _ = store.retire_chat_inbox(&parked_key, newest);
        }
    }
    LocalDoc {
        doc: SessionDoc::from_doc(raw),
        cursor,
        had_content,
        journaled,
    }
}

/// Saved deltas stay on top of a snapshot until they pass this many bytes (a
/// quarter of the snapshot, at least 1 MiB) or [`DELTA_ROW_LIMIT`] rows; then the
/// chat is saved whole again. Same budgets as the engine's chat persistence: a
/// whole save of a 34 MB chat rewrote 34 MB every second while rows streamed in,
/// and held SQLite's one writer while it did, so a send's outbox row waited on it.
const DELTA_MIN_BUDGET: usize = 1024 * 1024;
const DELTA_ROW_LIMIT: usize = 512;
/// Past the budget, the worker keeps writing deltas and folds them in once the
/// chat has been quiet this long, not mid-stream...
const QUIET_FOR_COMPACTION: Duration = Duration::from_secs(2);
/// ...unless the deltas reach this many times the budget: then the save is whole
/// even mid-stream, so they can't grow without bound.
const DELTA_HARD_CAP_FACTOR: usize = 4;

fn delta_budget(snapshot_bytes: usize) -> usize {
    (snapshot_bytes / 4).max(DELTA_MIN_BUDGET)
}

/// What this persister last saved whole, and the deltas on top of it.
struct DeltaState {
    /// The store's token for that whole save ([`DocsStore::save_base_snapshot`]).
    base: u64,
    /// The doc's oplog version when the newest saved state was taken.
    saved_vv: VersionVector,
    bytes: usize,
    rows: usize,
}

/// Debounced snapshot + cursor writer (cursor sampled BEFORE export, both
/// committed in one transaction). After the first whole save it writes what
/// changed since the last save, as a delta on that base; the store reads the
/// base and its deltas back as one snapshot ([`DocsStore::load_snapshot_with_cursor`]).
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
    /// Taken under `write`; its own lock keeps that obvious.
    delta: Mutex<Option<DeltaState>>,
    snapshot_bytes: AtomicUsize,
    /// The deltas passed their budget: fold them in at the next quiet moment.
    compaction_due: AtomicBool,
    /// Deltas already on disk that this persister didn't adopt (left by a kill):
    /// a compaction folds them even if the chat never changes.
    inherited_deltas: AtomicBool,
    /// A worker ([`Self::start`]) folds deltas in when the chat goes quiet. Without
    /// one, a save past the budget is whole at once.
    worker: AtomicBool,
    #[cfg(test)]
    stats: test_hooks::Stats,
}

impl Persister {
    pub(crate) fn new(
        doc: &Arc<SessionDoc>,
        store: Arc<DocsStore>,
        chat_id: &str,
        cursor: u64,
    ) -> Arc<Self> {
        // The state the load just found on disk, while it's still the chat's base
        // and this doc grew from it: deltas continue from there, so opening a chat
        // costs no whole save. Only a chat2 row (deltas can't stamp a lineage) whose
        // cursor is verified: the room client reads that flag from the snapshots
        // row, which only a whole save sets, so an unverified base must be saved
        // whole once or its legacy cursor repair would run on every open.
        let verified = store.snapshot_cursor_verified(chat_id).unwrap_or(false);
        let adopted = store.adopt_loaded_base(chat_id).filter(|loaded| {
            verified
                && loaded.epoch == CHAT2_DOC_EPOCH
                && doc.doc().oplog_vv().includes_vv(&loaded.version)
        });
        let inherited = adopted.is_none()
            && store
                .snapshot_delta_stats(chat_id)
                .is_ok_and(|(rows, _)| rows > 0);
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
            snapshot_bytes: AtomicUsize::new(adopted.as_ref().map_or(0, |loaded| loaded.base_bytes)),
            delta: Mutex::new(adopted.map(|loaded| DeltaState {
                base: loaded.token,
                saved_vv: loaded.version,
                bytes: loaded.delta_bytes,
                rows: loaded.delta_rows,
            })),
            compaction_due: AtomicBool::new(false),
            inherited_deltas: AtomicBool::new(inherited),
            worker: AtomicBool::new(false),
            #[cfg(test)]
            stats: Default::default(),
        })
    }

    /// A journal row that the next save covers.
    pub(crate) fn note_journaled(&self, ordinal: i64) {
        self.journaled.fetch_max(ordinal, Ordering::AcqRel);
    }

    pub(crate) fn start(self: &Arc<Self>, cancel: CancellationToken) {
        self.worker.store(true, Ordering::Release);
        let weak = Arc::downgrade(self);
        let wake = self.wake.clone();
        crate::runtime::shared().spawn(async move {
            loop {
                let notified = wake.notified();
                let Some(compaction_due) = weak
                    .upgrade()
                    .map(|this| this.compaction_due.load(Ordering::Acquire))
                else {
                    return;
                };
                let quiet = async {
                    if compaction_due {
                        tokio::time::sleep(QUIET_FOR_COMPACTION).await
                    } else {
                        std::future::pending().await
                    }
                };
                tokio::select! {
                    _ = cancel.cancelled() => {
                        if let Some(this) = weak.upgrade() {
                            let _ = tokio::task::spawn_blocking(move || this.compact()).await;
                        }
                        return;
                    }
                    _ = notified => {}
                    _ = quiet => {
                        // Quiet: fold the deltas in now, between bursts.
                        let Some(this) = weak.upgrade() else { return };
                        let _ = tokio::task::spawn_blocking(move || this.compact()).await;
                        continue;
                    }
                }
                let Some(this) = weak.upgrade() else { return };
                if !this.urgent.swap(false, Ordering::AcqRel) {
                    drop(this);
                    tokio::time::sleep(SAVE_INTERVAL).await;
                }
                let Some(this) = weak.upgrade() else { return };
                let job = this.clone();
                let _ = tokio::task::spawn_blocking(move || job.flush()).await;
                if !this.is_clean() && this.doc.strong_count() > 0 {
                    // A failed write: try again next interval, even if the chat
                    // has gone quiet.
                    wake.notify_one();
                }
            }
        });
    }

    pub(crate) fn cursor(&self) -> u64 {
        self.cursor.load(Ordering::Acquire)
    }

    pub(crate) fn is_clean(&self) -> bool {
        self.saved.load(Ordering::Acquire) == self.generation.load(Ordering::Acquire)
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

    /// Save what changed since the last save, as a delta when it can, else the
    /// whole snapshot (blocking I/O). True once everything changed before this
    /// call is on disk.
    pub(crate) fn flush(&self) -> bool {
        self.save(false)
    }

    /// Save the whole snapshot, folding any deltas into it (backgrounding,
    /// eviction, shutdown): a reopen then reads one blob, and a build from
    /// before deltas (0.2.138 and older) reads the snapshots row alone.
    pub(crate) fn compact(&self) -> bool {
        self.save(true)
    }

    fn save(&self, compact: bool) -> bool {
        let _serial = lock(&self.write);
        let mut delta = lock(&self.delta);
        let generation = self.generation.load(Ordering::Acquire);
        let folds_deltas = compact
            && match delta.as_ref() {
                Some(state) => state.rows > 0,
                None => self.inherited_deltas.load(Ordering::Acquire),
            };
        if generation == self.saved.load(Ordering::Acquire) && !folds_deltas {
            return true;
        }
        let Some(doc) = self.doc.upgrade() else {
            return false;
        };
        // Never read the cursor after exporting: a concurrent import could then
        // label an older snapshot with a newer cursor.
        let cursor = self.cursor();
        // Before the export: only what's noted by now is certainly in it.
        let journaled = self.journaled.load(Ordering::Acquire);
        #[cfg(test)]
        self.stats.run(&self.stats.before_export);
        // The version before the export too: an op committed in between is then
        // in this save AND counted unsaved, so the next delta repeats it (imports
        // are idempotent) rather than skipping it.
        let version = doc.doc().oplog_vv();
        #[cfg(test)]
        self.stats.run(&self.stats.after_version);
        let budget = delta_budget(self.snapshot_bytes.load(Ordering::Relaxed));
        let (byte_limit, row_limit) = if self.worker.load(Ordering::Acquire) {
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
                    self.stats.run(&self.stats.after_export);
                    self.store
                        .append_snapshot_delta(&self.chat_id, state.base, &bytes, cursor)
                        .map(|written| written.then_some(bytes.len()))
                        .map_err(|e| e.to_string())
                });
            match appended {
                Ok(Some(len)) => {
                    #[cfg(test)]
                    self.stats.wrote(false, len);
                    state.saved_vv = version;
                    state.bytes += len;
                    state.rows += 1;
                    if state.bytes >= budget || state.rows >= DELTA_ROW_LIMIT {
                        self.compaction_due.store(true, Ordering::Release);
                    }
                    self.saved.store(generation, Ordering::Release);
                    self.retire_journal(journaled);
                    return true;
                }
                // Another whole write replaced the base: save whole.
                Ok(None) => {}
                Err(err) => {
                    tracing::warn!(chat = %self.chat_id, error = %err, "chat snapshot delta failed");
                    return false;
                }
            }
        }
        let bytes = match doc.export_snapshot() {
            Ok(bytes) => bytes,
            Err(err) => {
                tracing::warn!(chat = %self.chat_id, error = %err, "chat snapshot export failed");
                return false;
            }
        };
        #[cfg(test)]
        self.stats.run(&self.stats.after_export);
        match self
            .store
            .save_base_snapshot(&self.chat_id, &bytes, cursor, CHAT2_DOC_EPOCH)
        {
            Ok(base) => {
                #[cfg(test)]
                self.stats.wrote(true, bytes.len());
                self.snapshot_bytes.store(bytes.len(), Ordering::Relaxed);
                // A whole save replaces every delta on disk.
                self.inherited_deltas.store(false, Ordering::Release);
                self.compaction_due.store(false, Ordering::Release);
                *delta = Some(DeltaState {
                    base,
                    saved_vv: version,
                    bytes: 0,
                    rows: 0,
                });
                self.saved.store(generation, Ordering::Release);
                self.retire_journal(journaled);
                true
            }
            Err(err) => {
                tracing::warn!(chat = %self.chat_id, error = %err, "chat snapshot save failed");
                false
            }
        }
    }

    /// After a save that holds every journal row up to `journaled`.
    fn retire_journal(&self, journaled: i64) {
        if journaled > self.retired.load(Ordering::Acquire) {
            match self.store.retire_chat_inbox(&self.chat_id, journaled) {
                Ok(()) => self.retired.store(journaled, Ordering::Release),
                Err(err) => {
                    tracing::warn!(chat = %self.chat_id, error = %err, "relay journal retirement failed")
                }
            }
        }
    }
}

#[cfg(test)]
mod test_hooks {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    pub(crate) type Hook = Mutex<Option<Box<dyn FnOnce() + Send>>>;

    /// Save counts, and code run at points inside a save.
    #[derive(Default)]
    pub(crate) struct Stats {
        pub(crate) whole: AtomicUsize,
        pub(crate) deltas: AtomicUsize,
        pub(crate) bytes: AtomicUsize,
        pub(crate) before_export: Hook,
        /// Between a save's version read and its export.
        pub(crate) after_version: Hook,
        /// Between a save's export and its store write.
        pub(crate) after_export: Hook,
    }

    impl Stats {
        pub(crate) fn run(&self, hook: &Hook) {
            let hook = hook.lock().unwrap().take();
            if let Some(hook) = hook {
                hook();
            }
        }

        pub(crate) fn wrote(&self, whole: bool, bytes: usize) {
            if whole { &self.whole } else { &self.deltas }.fetch_add(1, Ordering::Relaxed);
            self.bytes.fetch_add(bytes, Ordering::Relaxed);
        }

        pub(crate) fn whole(&self) -> usize {
            self.whole.load(Ordering::Relaxed)
        }

        pub(crate) fn deltas(&self) -> usize {
            self.deltas.load(Ordering::Relaxed)
        }
    }
}

/// What the room's relay task needs to know, kept current as the chat's row, its
/// host and this screen change. A phone has no direct link to the host: the room
/// carries everything.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct RelayRoute {
    pub generation: u32,
    pub host: String,
    /// The host's device row says it can read the queue. A host back on an older
    /// build keeps the generation it wrote, but this goes false.
    pub host_reads_queue: bool,
    pub on_screen: bool,
    pub host_online: bool,
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
}

impl ViewerSink {
    fn import(&self, bytes: &[u8], cursor: u64, urgent: bool) -> RowImportOutcome {
        let Some(doc) = self.doc.upgrade() else {
            return RowImportOutcome::Applied;
        };
        match doc.import(bytes) {
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
        // transcript, and offline there's nowhere to read it back from. So the row
        // goes only after a save, and both happen off this call: it runs under the
        // chat client's lock, and an export can commit a pending transaction whose
        // local-update hook queues a push, which takes that lock again. Until then
        // the row stays, and a restart sends it again, which the room dedupes.
        let persister = self.persister.clone();
        let store = self.store.clone();
        let chat_id = self.chat_id.clone();
        let batch_id = batch_id.to_owned();
        crate::runtime::shared().spawn_blocking(move || {
            // A delta save counts: the store reads it back with its base.
            if !persister.flush() {
                tracing::warn!(chat = %chat_id, "chat save failed; the sent row stays until one holds it");
                return;
            }
            if let Err(err) = store.acknowledge_chat_update(&chat_id, &batch_id) {
                tracing::warn!(chat = %chat_id, error = %err, "outbox retirement failed");
            }
        });
        Ok(())
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
        // (Through the doc's change lock: an export commits what's pending,
        // and that commit's events must be delivered under it.)
        doc.export(loro::ExportMode::updates(&vv)).ok()
    }

    /// As with a stored row, except that nothing on the edge can give this back: it
    /// goes in the journal until a save covers it. Only what moved the doc, so a
    /// repeat delivery writes nothing.
    fn relay_import(&self, bytes: &[u8], history: bool) -> RelayImport {
        // Gone: nothing took it in, so it mustn't count as read.
        let Some(doc) = self.doc.upgrade() else {
            return RelayImport::Unreadable;
        };
        let before = doc.doc().oplog_vv();
        let status = match doc.import(bytes) {
            Ok(status) => status,
            Err(err) => {
                tracing::warn!(chat = %self.chat_id, error = %err, "relay import failed");
                return RelayImport::Unreadable;
            }
        };
        if status.pending.is_some() {
            // Held in memory until its history arrives, and a save doesn't hold it:
            // kept apart from the journal a save retires, and replayed on load.
            if let Err(err) = self.store.journal_chat_inbox(&parked_inbox(&self.chat_id), bytes) {
                tracing::warn!(chat = %self.chat_id, error = %err, "parked relay journal write failed")
            }
        } else if doc.doc().oplog_vv() != before {
            match self.store.journal_chat_inbox(&self.chat_id, bytes) {
                Ok(ordinal) => self.persister.note_journaled(ordinal),
                Err(err) => {
                    tracing::warn!(chat = %self.chat_id, error = %err, "relay journal write failed")
                }
            }
        }
        // A catch-up can be the whole chat: save it now, not in a second.
        if doc.doc().oplog_vv() != before {
            self.persister.dirty(history);
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
    /// The chat is on the relay generation: a local commit's follow-up announces go
    /// out. Stored chats pay for none of it.
    relayed: Arc<AtomicBool>,
    defer_checkpoint: Arc<AtomicBool>,
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
    /// Holds the first join until the platform has said whether there's a network.
    pub path: super::path_gate::PathGate,
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
        });
        let relay = deps.relay_layer.then(|| {
            // Calls arrive from platform threads, and this spawns its task.
            let _runtime = crate::runtime::handle().enter();
            let handle = RelayHandle::spawn(&deps.device_id, sink.clone(), RelayPolicy::default());
            handle.set_serve_host_only(true);
            let host = Arc::new(Mutex::new(deps.route.host.clone()));
            RoomRelay {
                handle,
                host,
                relayed: Arc::new(AtomicBool::new(false)),
                defer_checkpoint: sink.defer_checkpoint.clone(),
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
        // The durable copy above is what counts: this one only schedules the
        // follow-up announce that lets the host check it has everything.
        let shortcut = relay
            .as_ref()
            .map(|r| (r.handle.clone(), r.relayed.clone()));
        let local_updates = doc
            .doc()
            .subscribe_local_update(Box::new(move |bytes: &Vec<u8>| {
                let batch_id = crate::new_id();
                let mut slot = lock(&publish_slot);
                if let Err(err) = store.enqueue_chat_update(&publish_chat, &batch_id, bytes) {
                    tracing::warn!(chat = %publish_chat, error = %err, "outbox enqueue failed");
                }
                // Before the push can go out: its ack retires the outbox row once a
                // save holds the update, and a save that finds nothing marked
                // changed writes nothing.
                publish_persister.dirty(false);
                match &slot.client {
                    Some(client) => client.enqueue_batch(batch_id, bytes.clone()),
                    None => slot.unsent.push((batch_id, bytes.clone())),
                }
                if let Some((relay, relayed)) = &shortcut
                    && relayed.load(Ordering::Acquire)
                {
                    relay.local_update(bytes.clone(), false);
                }
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
        let path = deps.path.clone();
        crate::runtime::shared().spawn(async move {
            let mut backoff = JOIN_RETRY_BASE;
            let client = loop {
                tokio::select! {
                    _ = cancel.cancelled() => return,
                    _ = path.ready() => {}
                }
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

    /// Save now, whole (backgrounding): see [`Persister::compact`].
    pub(crate) fn flush(&self) {
        self.persister.compact();
    }
}

impl Drop for Room {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(client) = lock(&self.slot).client.take() {
            crate::runtime::shared().spawn(client.shutdown());
        }
        self.persister.compact();
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
        // Its history hasn't come: it says so, and it's kept apart from what a save
        // retires, since a save doesn't hold it.
        assert_eq!(sink.relay_import(&later, false), RelayImport::Parked);
        assert!(store.pending_chat_inbox("chat").unwrap().0.is_empty());
        assert_eq!(store.pending_chat_inbox(&parked_inbox("chat")).unwrap().0, vec![later.clone()]);
        // A restart now still has it, waiting.
        let reloaded = load_local(&store, "chat");
        assert_eq!(reloaded.doc.doc().get_text("t").to_string(), "");
        assert_eq!(store.pending_chat_inbox(&parked_inbox("chat")).unwrap().0.len(), 1, "still waiting");
        // It lands with its history.
        assert_eq!(sink.relay_import(&first, false), RelayImport::Applied);
        assert_eq!(store.pending_chat_inbox("chat").unwrap().0.len(), 1);
        assert_eq!(doc.doc().get_text("t").to_string(), "second first");
        // Once a load finds it complete, the parked copy goes.
        let complete = load_local(&store, "chat");
        assert_eq!(complete.doc.doc().get_text("t").to_string(), "second first");
        assert!(store.pending_chat_inbox(&parked_inbox("chat")).unwrap().0.is_empty());
    }

    #[test]
    fn a_relayed_update_for_a_doc_thats_gone_isnt_taken_as_read() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(DocsStore::open(dir.path()).unwrap());
        let doc = Arc::new(SessionDoc::from_doc(loro::LoroDoc::new()));
        let sink = sink(store, &doc, "chat");
        drop(doc);
        let (_, bytes) = update("x");
        assert_eq!(sink.relay_import(&bytes, false), RelayImport::Unreadable);
    }

    #[test]
    fn an_ack_never_exports_the_doc_on_the_callers_thread() {
        // The client calls this under its lock. An export commits a pending
        // transaction, and its local-update hook queues a push, which takes that lock.
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(DocsStore::open(dir.path()).unwrap());
        let doc = Arc::new(SessionDoc::from_doc(loro::LoroDoc::new()));
        let sink = sink(store.clone(), &doc, "chat");
        store.enqueue_chat_update("chat", "sent", &[1]).unwrap();
        let caller = std::thread::current().id();
        let hooked_here = Arc::new(AtomicBool::new(false));
        let flag = hooked_here.clone();
        let _hook = doc.doc().subscribe_local_update(Box::new(move |_| {
            if std::thread::current().id() == caller {
                flag.store(true, Ordering::SeqCst);
            }
            true
        }));
        doc.doc().get_text("t").insert(0, "typed, not committed").unwrap();
        sink.persister.dirty(false);
        sink.acknowledge_update("sent").unwrap();
        assert!(!hooked_here.load(Ordering::SeqCst), "exported under the caller's lock");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !store.pending_chat_updates("chat").unwrap().is_empty() {
            assert!(std::time::Instant::now() < deadline, "the outbox row was never retired");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
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
        let handle = RelayHandle::spawn("ios", sink.clone(), RelayPolicy::default());
        let relay = RoomRelay {
            handle: handle.clone(),
            host: Arc::new(Mutex::new(String::new())),
            relayed: Arc::new(AtomicBool::new(false)),
            defer_checkpoint: sink.defer_checkpoint.clone(),
            deferred_since: Mutex::new(None),
            applied: Mutex::new(None),
        };
        let route = RelayRoute {
            generation: 3,
            host: "mac".into(),
            host_reads_queue: true,
            on_screen: true,
            host_online: true,
        };
        relay.apply_route(&route);
        assert!(relay.relayed.load(Ordering::Acquire));
        assert_eq!(*lock(&relay.host), "mac");
        assert!(relay.defer_checkpoint.load(Ordering::Acquire), "the host can answer");

        // On screen with the host gone, only the room can help.
        relay.apply_route(&RelayRoute { host_online: false, ..route.clone() });
        assert!(!relay.defer_checkpoint.load(Ordering::Acquire));

        // Off screen, nobody's waiting for it.
        relay.apply_route(&RelayRoute { host_online: false, on_screen: false, ..route.clone() });
        assert!(relay.defer_checkpoint.load(Ordering::Acquire));

        // Stored: nothing relay-shaped is left.
        relay.apply_route(&RelayRoute { generation: 2, ..route });
        assert!(!relay.relayed.load(Ordering::Acquire));
        assert!(!relay.defer_checkpoint.load(Ordering::Acquire));
    }

    // ── snapshot deltas (the engine's chat_persistence tests, on this persister) ──

    fn fixture() -> (tempfile::TempDir, Arc<SessionDoc>, Arc<DocsStore>, Arc<Persister>) {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(DocsStore::open(dir.path()).unwrap());
        let doc = Arc::new(SessionDoc::from_doc(loro::LoroDoc::new()));
        let persister = Persister::new(&doc, store.clone(), "whale", 0);
        (dir, doc, store, persister)
    }

    /// About `kb` kilobytes of incompressible text (Loro compresses repeats away).
    fn noise(kb: usize, seed: &mut u64) -> String {
        (0..kb * 64)
            .map(|_| {
                *seed ^= *seed << 13;
                *seed ^= *seed >> 7;
                *seed ^= *seed << 17;
                format!("{seed:016x}")
            })
            .collect()
    }

    /// A doc of about `kb` kilobytes, saved once, whole.
    fn saved_whale(kb: usize) -> (tempfile::TempDir, Arc<SessionDoc>, Arc<DocsStore>, Arc<Persister>) {
        let (dir, doc, store, persister) = fixture();
        let text = doc.doc().get_text("transcript");
        text.insert(0, &noise(kb, &mut 0x9E37_79B9_7F4A_7C15)).unwrap();
        doc.doc().commit();
        persister.applied(1, false);
        assert!(persister.flush());
        assert_eq!(persister.stats.whole(), 1, "the first save is whole");
        (dir, doc, store, persister)
    }

    /// A row streaming in, saved at once (the worker's job, done inline).
    fn stream(doc: &SessionDoc, persister: &Persister, cursor: u64, token: &str) {
        let text = doc.doc().get_text("transcript");
        text.insert(text.len_unicode(), token).unwrap();
        doc.doc().commit();
        persister.applied(cursor, false);
        assert!(persister.flush());
    }

    fn reopened(store: &DocsStore, chat: &str) -> (loro::LoroDoc, u64) {
        let (bytes, cursor, epoch) = store.load_snapshot_with_cursor(chat).unwrap().unwrap();
        assert_eq!(epoch, CHAT2_DOC_EPOCH);
        let doc = loro::LoroDoc::new();
        doc.import(&bytes).unwrap();
        (doc, cursor)
    }

    /// The snapshots row as a build that never reads deltas sees it.
    fn snapshots_row(dir: &std::path::Path, chat: &str) -> (loro::LoroDoc, u64, bool) {
        let db = rusqlite::Connection::open(dir.join("docs.sqlite3")).unwrap();
        let (bytes, cursor, verified): (Vec<u8>, i64, bool) = db
            .query_row(
                "SELECT bytes, cursor, cursor_verified FROM snapshots WHERE doc_id = ?1",
                [chat],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        let doc = loro::LoroDoc::new();
        doc.import(&bytes).unwrap();
        (doc, cursor as u64, verified)
    }

    #[test]
    fn a_save_after_the_first_writes_what_changed_not_the_whole_doc() {
        let (_dir, doc, store, persister) = saved_whale(2048);
        assert!(persister.snapshot_bytes.load(Ordering::Relaxed) > 1024 * 1024);
        for cursor in 2..=11 {
            stream(&doc, &persister, cursor, "streamed tokens ");
        }
        assert_eq!(persister.stats.whole(), 1, "no whole-doc rewrite");
        assert_eq!(persister.stats.deltas(), 10);
        let (rows, bytes) = store.snapshot_delta_stats("whale").unwrap();
        assert_eq!(rows, 10);
        assert!(bytes < 10 * 1024, "{bytes} B of deltas for 10 short edits");
        let (loaded, cursor) = reopened(&store, "whale");
        assert_eq!(loaded.get_deep_value(), doc.doc().get_deep_value());
        assert_eq!(cursor, 11);
        assert!(store.snapshot_cursor_verified("whale").unwrap());
        assert!(persister.is_clean());
    }

    /// Backgrounding and eviction compact: afterwards the snapshots row alone (what
    /// a build from before deltas reads) is the whole doc, with its newest cursor.
    #[test]
    fn compaction_folds_deltas_into_the_snapshots_row_alone() {
        let (dir, doc, store, persister) = saved_whale(64);
        for cursor in 2..=4 {
            stream(&doc, &persister, cursor, "more ");
        }
        assert_eq!(store.snapshot_delta_stats("whale").unwrap().0, 3);
        // Until then, that row is the older honest pair.
        let (row, row_cursor, _) = snapshots_row(dir.path(), "whale");
        assert_ne!(row.get_deep_value(), doc.doc().get_deep_value());
        assert_eq!(row_cursor, 1);
        assert!(persister.compact());
        assert_eq!(persister.stats.whole(), 2);
        assert_eq!(store.snapshot_delta_stats("whale").unwrap(), (0, 0));
        let (row, row_cursor, verified) = snapshots_row(dir.path(), "whale");
        assert_eq!(row.get_deep_value(), doc.doc().get_deep_value());
        assert_eq!(row_cursor, 4);
        assert!(verified);
        // Clean with no deltas: nothing to fold, nothing written.
        assert!(persister.compact());
        assert_eq!(persister.stats.whole(), 2);
    }

    #[test]
    fn deltas_past_their_budget_save_the_whole_doc_again() {
        let (_dir, doc, store, persister) = saved_whale(16);
        for cursor in 2..(2 + DELTA_ROW_LIMIT as u64) {
            stream(&doc, &persister, cursor, "t");
        }
        assert_eq!(persister.stats.whole(), 1);
        assert_eq!(store.snapshot_delta_stats("whale").unwrap().0, DELTA_ROW_LIMIT as u64);
        stream(&doc, &persister, 9_999, "t");
        assert_eq!(persister.stats.whole(), 2, "row limit reached: whole save");
        assert_eq!(store.snapshot_delta_stats("whale").unwrap(), (0, 0));
        // Byte budget: one delta past max(1 MiB, snapshot / 4).
        stream(&doc, &persister, 10_000, &"y".repeat(DELTA_MIN_BUDGET + 1));
        assert_eq!(persister.stats.whole(), 2);
        stream(&doc, &persister, 10_001, "t");
        assert_eq!(persister.stats.whole(), 3, "byte budget reached: whole save");
        let (loaded, cursor) = reopened(&store, "whale");
        assert_eq!(loaded.get_deep_value(), doc.doc().get_deep_value());
        assert_eq!(cursor, 10_001);
    }

    #[test]
    fn another_full_write_makes_the_next_save_whole() {
        let (_dir, doc, store, persister) = saved_whale(16);
        stream(&doc, &persister, 2, "a");
        assert_eq!(store.snapshot_delta_stats("whale").unwrap().0, 1);
        // Another writer replaces the base.
        store
            .save_verified_snapshot_with_cursor("whale", &doc.export_snapshot().unwrap(), 2, CHAT2_DOC_EPOCH)
            .unwrap();
        assert_eq!(store.snapshot_delta_stats("whale").unwrap(), (0, 0), "a full write clears deltas");
        stream(&doc, &persister, 3, "b");
        assert_eq!(persister.stats.whole(), 2, "no delta onto a base it didn't write");
        let (loaded, cursor) = reopened(&store, "whale");
        assert_eq!(loaded.get_deep_value(), doc.doc().get_deep_value());
        assert_eq!(cursor, 3);
        // A deleted chat forgets its base too.
        store.delete_snapshot("whale").unwrap();
        stream(&doc, &persister, 4, "c");
        assert_eq!(persister.stats.whole(), 3);
        assert_eq!(reopened(&store, "whale").1, 4);
    }

    #[test]
    fn a_lowered_cursor_is_the_one_read_back() {
        let (_dir, doc, store, persister) = saved_whale(16);
        stream(&doc, &persister, 50, "a");
        persister.reset_cursor(7);
        assert!(persister.flush());
        assert_eq!(persister.stats.deltas(), 2);
        assert_eq!(reopened(&store, "whale").1, 7);
        assert_eq!(load_local(&store, "whale").cursor, 7);
    }

    /// A delta cut short never yields a state that claims the delta's cursor: the
    /// base and its own cursor load instead.
    #[test]
    fn a_torn_delta_falls_back_to_the_base_pair() {
        let (_dir, doc, store, persister) = saved_whale(16);
        let (base_bytes, base_cursor, _) = store.load_snapshot_with_cursor("whale").unwrap().unwrap();
        let vv = doc.doc().oplog_vv();
        stream(&doc, &persister, 2, "a token");
        let full = doc.doc().export(loro::ExportMode::updates(&vv)).unwrap();
        let base = store.save_base_snapshot("whale", &base_bytes, base_cursor, CHAT2_DOC_EPOCH).unwrap();
        assert!(store.append_snapshot_delta("whale", base, &full[..full.len() / 2], 2).unwrap());
        let (bytes, cursor, _) = store.load_snapshot_with_cursor("whale").unwrap().unwrap();
        assert_eq!(cursor, base_cursor);
        assert_eq!(bytes, base_bytes);
        let local = load_local(&store, "whale");
        assert_eq!(local.cursor, base_cursor);
    }

    /// An op that lands between a delta save's version read and its export is in
    /// that delta and still counted unsaved; one after the export is carried by the
    /// next. Neither is lost nor doubled.
    #[test]
    fn an_op_racing_a_delta_save_is_neither_lost_nor_doubled() {
        let (_dir, doc, store, persister) = saved_whale(16);
        stream(&doc, &persister, 2, "before ");
        let racer = doc.clone();
        *persister.stats.after_version.lock().unwrap() = Some(Box::new(move || {
            let text = racer.doc().get_text("transcript");
            text.insert(text.len_unicode(), "RACER ").unwrap();
            racer.doc().commit();
        }));
        stream(&doc, &persister, 3, "during ");
        stream(&doc, &persister, 4, "after ");
        let late = doc.clone();
        *persister.stats.after_export.lock().unwrap() = Some(Box::new(move || {
            let text = late.doc().get_text("transcript");
            text.insert(text.len_unicode(), "LATE ").unwrap();
            late.doc().commit();
        }));
        stream(&doc, &persister, 5, "then ");
        persister.dirty(false);
        assert!(persister.flush());
        let (loaded, cursor) = reopened(&store, "whale");
        assert_eq!(loaded.get_deep_value(), doc.doc().get_deep_value());
        let text = loaded.get_text("transcript").to_string();
        assert_eq!(text.matches("RACER").count(), 1);
        assert_eq!(text.matches("LATE").count(), 1);
        assert_eq!(cursor, 5);
        assert_eq!(persister.stats.whole(), 1, "all on the delta path");
    }

    /// A delta that the store refuses leaves the save unclean and the disk as it
    /// was; the retry writes it.
    #[test]
    fn a_failed_delta_write_stays_unsaved_until_a_retry_lands() {
        let (dir, doc, store, persister) = saved_whale(16);
        stream(&doc, &persister, 2, "first ");
        let before = reopened(&store, "whale");
        let db = rusqlite::Connection::open(dir.path().join("docs.sqlite3")).unwrap();
        db.execute_batch("CREATE TRIGGER no_delta BEFORE INSERT ON snapshot_deltas BEGIN SELECT RAISE(FAIL, 'disk full'); END;").unwrap();
        let text = doc.doc().get_text("transcript");
        text.insert(text.len_unicode(), "refused ").unwrap();
        doc.doc().commit();
        persister.applied(3, false);
        assert!(!persister.flush());
        assert!(!persister.is_clean(), "a failed write counted as saved");
        let after = reopened(&store, "whale");
        assert_eq!(after.0.get_deep_value(), before.0.get_deep_value());
        assert_eq!(after.1, 2);
        db.execute_batch("DROP TRIGGER no_delta;").unwrap();
        assert!(persister.flush());
        let (loaded, cursor) = reopened(&store, "whale");
        assert_eq!(loaded.get_deep_value(), doc.doc().get_deep_value());
        assert_eq!(cursor, 3);
    }

    /// The relay journal goes only once a save holds it, and a delta save does.
    #[test]
    fn a_delta_save_retires_the_relay_journal_rows_it_holds() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(DocsStore::open(dir.path()).unwrap());
        let doc = Arc::new(SessionDoc::from_doc(loro::LoroDoc::new()));
        let sink = sink(store.clone(), &doc, "chat");
        let (remote, first) = update("first");
        sink.relay_import(&first, false);
        assert!(sink.persister.flush());
        assert_eq!(sink.persister.stats.whole(), 1);
        let at = remote.oplog_vv();
        remote.get_text("t").insert(0, "second ").unwrap();
        remote.commit();
        sink.relay_import(&remote.export(loro::ExportMode::updates(&at)).unwrap(), false);
        assert_eq!(store.pending_chat_inbox("chat").unwrap().0.len(), 1);
        assert!(sink.persister.flush());
        assert_eq!(sink.persister.stats.deltas(), 1);
        assert!(store.pending_chat_inbox("chat").unwrap().0.is_empty(), "the delta holds it");
        let local = load_local(&store, "chat");
        assert_eq!(local.doc.doc().get_text("t").to_string(), "second first");
    }

    /// An ack retires its outbox row only after a save that holds the update has
    /// landed, delta or whole: until then the row is the send's only local copy.
    #[test]
    fn an_ack_retires_the_outbox_row_only_after_a_durable_delta() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(DocsStore::open(dir.path()).unwrap());
        let doc = Arc::new(SessionDoc::from_doc(loro::LoroDoc::new()));
        let sink = sink(store.clone(), &doc, "chat");
        doc.doc().get_text("t").insert(0, "history ").unwrap();
        doc.doc().commit();
        sink.persister.dirty(false);
        assert!(sink.persister.flush());
        // A send: committed, its outbox row written, the doc marked changed.
        let send = |text: &str, batch: &str| {
            let at = doc.doc().oplog_vv();
            let t = doc.doc().get_text("t");
            t.insert(t.len_unicode(), text).unwrap();
            doc.doc().commit();
            let bytes = doc.doc().export(loro::ExportMode::updates(&at)).unwrap();
            store.enqueue_chat_update("chat", batch, &bytes).unwrap();
            sink.persister.dirty(false);
        };
        let pending = |batch: &str| {
            store.pending_chat_updates("chat").unwrap().iter().any(|(id, _)| id == batch)
        };
        let settle = || std::thread::sleep(std::time::Duration::from_millis(300));

        let db = rusqlite::Connection::open(dir.path().join("docs.sqlite3")).unwrap();
        db.execute_batch(
            "CREATE TRIGGER no_delta BEFORE INSERT ON snapshot_deltas BEGIN SELECT RAISE(FAIL, 'disk full'); END;
             CREATE TRIGGER no_whole BEFORE UPDATE ON snapshots BEGIN SELECT RAISE(FAIL, 'disk full'); END;",
        )
        .unwrap();
        send("refused", "b1");
        sink.acknowledge_update("b1").unwrap();
        settle();
        assert!(pending("b1"), "retired without a save that holds it");
        db.execute_batch("DROP TRIGGER no_delta; DROP TRIGGER no_whole;").unwrap();

        send("kept", "b2");
        sink.acknowledge_update("b2").unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while pending("b2") {
            assert!(std::time::Instant::now() < deadline, "the outbox row was never retired");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(sink.persister.stats.whole(), 1, "the ack's save was a delta");
        // With the outbox row gone, the save alone holds the send.
        store.acknowledge_chat_update("chat", "b1").unwrap();
        let (loaded, _) = reopened(&store, "chat");
        assert_eq!(loaded.get_text("t").to_string(), "history refusedkept");
    }

    /// The app updated over a store the current release wrote (whole snapshots,
    /// cursor verified, no deltas): it opens unchanged, and saves deltas on it
    /// without a whole first save.
    #[test]
    fn app_updated_over_existing_data_reads_it_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let old = loro::LoroDoc::new();
        old.get_text("t").insert(0, &noise(64, &mut 7)).unwrap();
        old.commit();
        let bytes = old.export(loro::ExportMode::Snapshot).unwrap();
        {
            // What the release's persister wrote, and its relay journal.
            let store = DocsStore::open(dir.path()).unwrap();
            store.save_verified_snapshot_with_cursor("chat", &bytes, 42, CHAT2_DOC_EPOCH).unwrap();
        }
        let store = Arc::new(DocsStore::open(dir.path()).unwrap());
        assert_eq!(store.load_snapshot_with_cursor("chat").unwrap().unwrap(), (bytes.clone(), 42, 2));
        let local = load_local(&store, "chat");
        assert!(local.had_content);
        assert_eq!(local.cursor, 42);
        assert_eq!(local.doc.doc().get_deep_value(), old.get_deep_value());
        let doc = Arc::new(local.doc);
        let persister = Persister::new(&doc, store.clone(), "chat", local.cursor);
        for cursor in 43..=45 {
            let t = doc.doc().get_text("t");
            t.insert(0, "new ").unwrap();
            doc.doc().commit();
            persister.applied(cursor, false);
            assert!(persister.flush());
        }
        assert_eq!(persister.stats.whole(), 0, "a whole save after the update");
        assert_eq!(store.snapshot_delta_stats("chat").unwrap().0, 3);
        let again = load_local(&store, "chat");
        assert_eq!(again.cursor, 45);
        assert_eq!(again.doc.doc().get_deep_value(), doc.doc().get_deep_value());
        assert!(store.snapshot_cursor_verified("chat").unwrap());
    }

    /// A base whose cursor isn't verified (a legacy row) is saved whole once: the
    /// room client reads that flag from the snapshots row, and a delta can't set it.
    #[test]
    fn an_unverified_base_is_saved_whole_once() {
        let dir = tempfile::tempdir().unwrap();
        let old = loro::LoroDoc::new();
        old.get_text("t").insert(0, "legacy").unwrap();
        old.commit();
        let store = Arc::new(DocsStore::open(dir.path()).unwrap());
        store
            .save_snapshot_with_cursor("chat", &old.export(loro::ExportMode::Snapshot).unwrap(), 9, CHAT2_DOC_EPOCH)
            .unwrap();
        let local = load_local(&store, "chat");
        assert_eq!(local.cursor, 0, "an unverified cursor isn't trusted");
        let doc = Arc::new(local.doc);
        let persister = Persister::new(&doc, store.clone(), "chat", 0);
        persister.applied(12, false);
        assert!(persister.flush());
        assert_eq!(persister.stats.whole(), 1);
        assert!(store.snapshot_cursor_verified("chat").unwrap());
        persister.applied(13, false);
        assert!(persister.flush());
        assert_eq!(persister.stats.deltas(), 1);
        assert_eq!(load_local(&store, "chat").cursor, 13);
    }

    /// Deltas a kill left behind (not adopted: here the next open's doc didn't grow
    /// from them) fold in at the next compaction even if the chat never changes.
    #[test]
    fn deltas_left_by_a_kill_fold_at_the_next_compaction() {
        let dir = tempfile::tempdir().unwrap();
        {
            let store = Arc::new(DocsStore::open(dir.path()).unwrap());
            let doc = Arc::new(SessionDoc::from_doc(loro::LoroDoc::new()));
            let persister = Persister::new(&doc, store.clone(), "left", 0);
            for cursor in 1..=4u64 {
                doc.doc().get_text("t").insert(0, "x").unwrap();
                doc.doc().commit();
                persister.applied(cursor, false);
                assert!(persister.flush());
            }
            assert_eq!(store.snapshot_delta_stats("left").unwrap().0, 3);
        }
        let store = Arc::new(DocsStore::open(dir.path()).unwrap());
        // Adopted on a normal open: the next save is a delta, and a compaction folds.
        let local = load_local(&store, "left");
        let doc = Arc::new(local.doc);
        let persister = Persister::new(&doc, store.clone(), "left", local.cursor);
        assert!(persister.compact());
        assert_eq!(persister.stats.whole(), 1);
        assert_eq!(store.snapshot_delta_stats("left").unwrap(), (0, 0));
        let (row, cursor, _) = snapshots_row(dir.path(), "left");
        assert_eq!(row.get_deep_value(), doc.doc().get_deep_value());
        assert_eq!(cursor, 4);
    }

    /// Child of [`a_kill_mid_save_loses_and_doubles_nothing`]: streams numbered ops,
    /// saving after each (deltas, and whole saves at each budget), until killed.
    #[test]
    fn delta_crash_child() {
        let Some(dir) = std::env::var_os("ZERON_CLIENT_DELTA_CRASH_DIR") else {
            return;
        };
        let store = Arc::new(DocsStore::open(&dir).unwrap());
        let doc = Arc::new(SessionDoc::from_doc(loro::LoroDoc::new()));
        let persister = Persister::new(&doc, store, "crash", 0);
        let text = doc.doc().get_text("ops");
        for op in 1u64..=u64::MAX {
            text.insert(text.len_unicode(), &format!("{op},")).unwrap();
            doc.doc().commit();
            persister.applied(op, false);
            persister.flush();
            if op.is_multiple_of(97) {
                persister.compact();
            }
            if op == 1 {
                // The parent times its kill from here, not from a cold start.
                std::fs::write(std::path::Path::new(&dir).join("started"), b"").unwrap();
            }
        }
    }

    /// Kill the writer (SIGKILL) at arbitrary points, mid-append or mid-compaction.
    /// What reopens is exactly ops 1..=cursor.
    #[cfg(unix)]
    #[test]
    fn a_kill_mid_save_loses_and_doubles_nothing() {
        for (round, kill_after_ms) in [0u64, 15, 60, 140, 300].into_iter().enumerate() {
            let dir = tempfile::tempdir().unwrap();
            let mut child = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "live::room::tests::delta_crash_child", "--nocapture"])
                .env("ZERON_CLIENT_DELTA_CRASH_DIR", dir.path())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .unwrap();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
            while !dir.path().join("started").exists() {
                assert!(std::time::Instant::now() < deadline, "round {round}: the child never started");
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            std::thread::sleep(std::time::Duration::from_millis(kill_after_ms));
            child.kill().unwrap();
            child.wait().unwrap();
            let store = DocsStore::open(dir.path()).unwrap();
            let Some((bytes, cursor, _)) = store.load_snapshot_with_cursor("crash").unwrap() else {
                panic!("round {round}: nothing saved in {kill_after_ms} ms");
            };
            let doc = loro::LoroDoc::new();
            doc.import(&bytes).unwrap();
            let expected: String = (1..=cursor).map(|op| format!("{op},")).collect();
            assert_eq!(doc.get_text("ops").to_string(), expected, "round {round}: not ops 1..={cursor}");
            let local = load_local(&store, "crash");
            assert_eq!(local.cursor, cursor, "round {round}");
            let (rows, _) = store.snapshot_delta_stats("crash").unwrap();
            eprintln!("round {round}: killed at {kill_after_ms} ms, cursor {cursor}, {rows} deltas");
        }
    }

    /// Child of [`dying_between_export_and_commit_keeps_the_last_commit`]: streams
    /// ops 1.. and aborts inside save number ZERON_CLIENT_DELTA_ABORT_AT, after its
    /// export and before its store write.
    #[test]
    fn delta_abort_child() {
        let (Some(dir), Some(at)) = (
            std::env::var_os("ZERON_CLIENT_DELTA_CRASH_DIR"),
            std::env::var("ZERON_CLIENT_DELTA_ABORT_AT").ok().and_then(|v| v.parse::<u64>().ok()),
        ) else {
            return;
        };
        let store = Arc::new(DocsStore::open(&dir).unwrap());
        let doc = Arc::new(SessionDoc::from_doc(loro::LoroDoc::new()));
        let persister = Persister::new(&doc, store, "crash", 0);
        let text = doc.doc().get_text("ops");
        for op in 1u64..=u64::MAX {
            text.insert(text.len_unicode(), &format!("{op},")).unwrap();
            doc.doc().commit();
            if op == at {
                *persister.stats.after_export.lock().unwrap() = Some(Box::new(|| std::process::abort()));
            }
            persister.applied(op, false);
            persister.flush();
        }
    }

    /// Dying after the export and before the commit leaves exactly the last
    /// committed state: in the first whole save, in a delta, and in the whole save
    /// that folds 512 deltas.
    #[cfg(unix)]
    #[test]
    fn dying_between_export_and_commit_keeps_the_last_commit() {
        // Op 1 is the first (whole) save; 2..=513 are deltas; 514 is whole.
        for at in [1u64, 2, 37, 513, 514, 515] {
            let dir = tempfile::tempdir().unwrap();
            let status = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "live::room::tests::delta_abort_child", "--nocapture"])
                .env("ZERON_CLIENT_DELTA_CRASH_DIR", dir.path())
                .env("ZERON_CLIENT_DELTA_ABORT_AT", at.to_string())
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

    /// Ops stream on one thread, the worker saves, and another thread forces saves
    /// and compactions (backgrounding, acks) meanwhile. Every state on disk holds
    /// ops 1..=k with no gap or repeat, and never fewer than its cursor claims.
    #[test]
    fn deltas_stay_whole_while_the_cursor_advances_concurrently() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(DocsStore::open(dir.path()).unwrap());
        let doc = Arc::new(SessionDoc::from_doc(loro::LoroDoc::new()));
        let persister = Persister::new(&doc, store.clone(), "race", 0);
        let cancel = CancellationToken::new();
        persister.start(cancel.clone());
        let stop = Arc::new(AtomicBool::new(false));
        let writer = {
            let (doc, persister, stop) = (doc.clone(), persister.clone(), stop.clone());
            std::thread::spawn(move || {
                let text = doc.doc().get_text("ops");
                let mut op = 0u64;
                while !stop.load(Ordering::Relaxed) {
                    op += 1;
                    text.insert(text.len_unicode(), &format!("{op},")).unwrap();
                    doc.doc().commit();
                    persister.applied(op, op.is_multiple_of(7));
                }
                op
            })
        };
        let forcer = {
            let (persister, stop) = (persister.clone(), stop.clone());
            std::thread::spawn(move || {
                let mut n = 0u64;
                while !stop.load(Ordering::Relaxed) {
                    n += 1;
                    if n.is_multiple_of(50) {
                        persister.compact();
                    } else {
                        persister.flush();
                    }
                    std::thread::sleep(std::time::Duration::from_millis(3));
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
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(2_500);
        while std::time::Instant::now() < deadline {
            check("mid-stream");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        stop.store(true, Ordering::Relaxed);
        let last = writer.join().unwrap();
        forcer.join().unwrap();
        assert!(persister.flush());
        check("after the last save");
        let (loaded, cursor) = reopened(&store, "race");
        assert_eq!(loaded.get_deep_value(), doc.doc().get_deep_value());
        assert_eq!(cursor, last);
        assert!(persister.stats.deltas() > 0, "no deltas exercised");
        cancel.cancel();
    }

    /// Past the budget, a streaming chat isn't saved whole while rows keep coming;
    /// the worker folds the deltas in once it goes quiet.
    #[test]
    fn a_streaming_chat_is_saved_whole_once_it_goes_quiet() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(DocsStore::open(dir.path()).unwrap());
        let doc = Arc::new(SessionDoc::from_doc(loro::LoroDoc::new()));
        let persister = Persister::new(&doc, store.clone(), "stream", 0);
        let cancel = CancellationToken::new();
        persister.start(cancel.clone());
        let mut seed = 0x517c_c1b7_2722_0a95_u64;
        let mut cursor = 0;
        let mut tick = |kb: usize| {
            let text = doc.doc().get_text("t");
            text.insert(text.len_unicode(), &noise(kb, &mut seed)).unwrap();
            doc.doc().commit();
            cursor += 1;
            persister.applied(cursor, true);
            std::thread::sleep(std::time::Duration::from_millis(60));
        };
        let wait = |what: &str, f: &dyn Fn() -> bool| {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
            while !f() {
                assert!(std::time::Instant::now() < deadline, "{what}");
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        };
        tick(1);
        wait("the first save", &|| persister.stats.whole() == 1);
        // ~1.5 MiB past a 1 MiB budget, well under the 4 MiB cap.
        for _ in 0..48 {
            tick(32);
        }
        wait("the last delta", &|| persister.is_clean());
        assert!(persister.compaction_due.load(Ordering::Acquire), "never passed the budget");
        assert_eq!(persister.stats.whole(), 1, "a whole save landed mid-stream");
        wait("folding once quiet", &|| persister.stats.whole() == 2);
        assert_eq!(store.snapshot_delta_stats("stream").unwrap(), (0, 0));
        let (row, row_cursor, _) = snapshots_row(dir.path(), "stream");
        assert_eq!(row.get_deep_value(), doc.doc().get_deep_value());
        assert_eq!(row_cursor, cursor);
        cancel.cancel();
    }

    /// The real thing, when supplied: a big chat's snapshot (never written back)
    /// through deltas and a compaction; after every step a reload equals the doc.
    #[test]
    #[ignore = "requires ZERON_PHONE_BENCH_CHAT: a local chat snapshot, read into a temporary store"]
    fn real_chat_reloads_equal_through_deltas() {
        let path = std::env::var("ZERON_PHONE_BENCH_CHAT").unwrap();
        let raw = loro::LoroDoc::new();
        raw.import(&std::fs::read(path).unwrap()).unwrap();
        let doc = Arc::new(SessionDoc::from_doc(raw));
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(DocsStore::open(dir.path()).unwrap());
        let persister = Persister::new(&doc, store.clone(), "real", 0);
        let check = |label: &str| {
            let local = load_local(&store, "real");
            assert_eq!(local.doc.doc().oplog_vv(), doc.doc().oplog_vv(), "{label}: ops differ");
            assert_eq!(local.doc.doc().get_deep_value(), doc.doc().get_deep_value(), "{label}: state differs");
        };
        persister.applied(1, false);
        assert!(persister.flush());
        check("after the whole save");
        let text = doc.doc().get_text("bench-stream");
        for cursor in 2..=201u64 {
            text.insert(text.len_unicode(), &format!("streamed token {cursor} ")).unwrap();
            doc.doc().commit();
            persister.applied(cursor, false);
            assert!(persister.flush());
        }
        check("after 200 deltas");
        assert_eq!(persister.stats.whole(), 1);
        assert!(persister.compact());
        assert_eq!(store.snapshot_delta_stats("real").unwrap(), (0, 0));
        check("after compaction");
        assert_eq!(load_local(&store, "real").cursor, 201);
    }
}
