//! `DocsStore` — local SQLite persistence for doc snapshots and the
//! processed-command ledger (ARCHITECTURE §2 command plane: entries are marked
//! processed BEFORE execution so a crash can never double-execute a command).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, params};

/// A doc's state on disk as a load found it, which a saver may adopt as
/// the base for its deltas ([`DocsStore::adopt_loaded_base`]).
#[derive(Debug, Clone)]
pub struct LoadedBase {
    pub token: u64,
    /// The row's lineage epoch: a saver only continues a chat2 (epoch 2) row.
    pub epoch: u32,
    pub version: loro::VersionVector,
    pub delta_rows: usize,
    pub delta_bytes: usize,
    pub base_bytes: usize,
}

/// Errors surfaced by [`DocsStore`].
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("sqlite: {0}")]
    Sqlite(rusqlite::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

impl From<rusqlite::Error> for StoreError {
    /// Every store error passes here: one that says the file is damaged is
    /// noted, so the engine can recover the store ([`crate::corruption_seen`]).
    fn from(err: rusqlite::Error) -> Self {
        crate::store_recovery::note_error(&err);
        Self::Sqlite(err)
    }
}

/// Synchronous command/doc callbacks still need commit-before-send semantics.
/// Relinquish a multithread runtime's core BEFORE either SQLite or its mutex
/// can block; moving only the snapshot writer leaves these contenders fatal.
pub(crate) fn store_blocking<T>(f: impl FnOnce() -> T) -> T {
    if tokio::runtime::Handle::try_current()
        .is_ok_and(|h| h.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread)
    {
        tokio::task::block_in_place(f)
    } else {
        f()
    }
}

/// Ordered, append-only migrations. Each entry runs once inside a transaction;
/// `schema_migrations` records what has been applied.
const MIGRATIONS: &[&str] = &[
    // v1 — snapshots + processed-command ledger
    "CREATE TABLE snapshots (
        doc_id   TEXT PRIMARY KEY,
        bytes    BLOB NOT NULL,
        saved_at INTEGER NOT NULL
     ) STRICT;
     CREATE TABLE processed_commands (
        command_id   TEXT PRIMARY KEY,
        processed_at INTEGER NOT NULL
     ) STRICT;",
    // v2 — chat2 room cursor + doc epoch (docs/chat2-sync.md C2). The cursor
    // is persisted in the SAME transaction as the snapshot bytes, so content
    // and cursor cannot diverge (restored backups / copied devices simply
    // redownload from their honest cursor). `epoch` marks rebuild lineage
    // (M1/M3): 2 = thin chat2 rebuild; NULL/0 = pre-migration s2 doc.
    "ALTER TABLE snapshots ADD COLUMN cursor INTEGER;
     ALTER TABLE snapshots ADD COLUMN epoch INTEGER;",
    // v3 — publication survives actor eviction and process restart.
    "CREATE TABLE chat_outbox (
        ordinal INTEGER PRIMARY KEY AUTOINCREMENT,
        doc_id TEXT NOT NULL,
        batch_id TEXT NOT NULL UNIQUE,
        bytes BLOB NOT NULL,
        needs_checkpoint INTEGER NOT NULL DEFAULT 0
     ) STRICT;
     CREATE INDEX chat_outbox_doc ON chat_outbox(doc_id,ordinal);
     CREATE TABLE chat_outbox_initialized (doc_id TEXT PRIMARY KEY) STRICT;",
    // Older cursors may have advanced over parked imports. Trust only writes
    // made by the causal-aware persister, not every legacy epoch-2 snapshot.
    "ALTER TABLE snapshots ADD COLUMN cursor_verified INTEGER NOT NULL DEFAULT 0;",
    // Durable wake/recovery receipts, independent of live document handles.
    "CREATE TABLE chat_sync_jobs (
        doc_id TEXT NOT NULL,
        kind TEXT NOT NULL,
        version INTEGER NOT NULL DEFAULT 1,
        PRIMARY KEY(doc_id, kind)
    ) STRICT;
    CREATE TABLE sync_job_clock (id INTEGER PRIMARY KEY CHECK(id=1), value INTEGER NOT NULL) STRICT;
    INSERT INTO sync_job_clock VALUES (1,0);",
    "ALTER TABLE chat_sync_jobs ADD COLUMN cursor TEXT NOT NULL DEFAULT '';",
    // Updates that came by relay, until a saved snapshot holds them. Nothing on
    // the edge can give these back to a device that crashes before saving.
    "CREATE TABLE IF NOT EXISTS chat_inbox (
        ordinal INTEGER PRIMARY KEY AUTOINCREMENT,
        doc_id TEXT NOT NULL,
        bytes BLOB NOT NULL
     ) STRICT;
     CREATE INDEX IF NOT EXISTS chat_inbox_doc ON chat_inbox(doc_id,ordinal);",
    // Sends waiting for their attachment bytes to reach the edge
    // (held_sends.rs). Ordinal order is send order, per chat.
    // The relay inbox again: a pre-release build took the previous slot for
    // this table, so a device that ran it skipped the inbox above.
    "CREATE TABLE IF NOT EXISTS held_sends (
        ordinal INTEGER PRIMARY KEY AUTOINCREMENT,
        id TEXT NOT NULL UNIQUE,
        chat_id TEXT NOT NULL,
        payload TEXT NOT NULL,
        created_at INTEGER NOT NULL,
        failure TEXT
    ) STRICT;
    CREATE INDEX IF NOT EXISTS held_sends_chat ON held_sends(chat_id,ordinal);
    CREATE TABLE IF NOT EXISTS chat_inbox (
        ordinal INTEGER PRIMARY KEY AUTOINCREMENT,
        doc_id TEXT NOT NULL,
        bytes BLOB NOT NULL
     ) STRICT;
     CREATE INDEX IF NOT EXISTS chat_inbox_doc ON chat_inbox(doc_id,ordinal);",
    // Loro updates saved on top of a doc's snapshot row, oldest first (see
    // `append_snapshot_delta`). Each row carries the cursor its state covers.
    // The snapshots row keeps its own bytes and cursor, so a build that
    // predates this table reads a consistent (older) snapshot.
    "CREATE TABLE IF NOT EXISTS snapshot_deltas (
        ordinal INTEGER PRIMARY KEY AUTOINCREMENT,
        doc_id TEXT NOT NULL,
        bytes BLOB NOT NULL,
        cursor INTEGER NOT NULL
     ) STRICT;
     CREATE INDEX IF NOT EXISTS snapshot_deltas_doc ON snapshot_deltas(doc_id,ordinal);",
    // The `saved_at` of the base a delta extends: a delta claims its cursor
    // only while that base is the row on disk (an older build may rewrite it).
    // Pre-release builds created the table with this column already; rows
    // that predate it (0) never match a base.
    "ADD COLUMN IF MISSING snapshot_deltas base_saved_at INTEGER NOT NULL DEFAULT 0",
    // A delta's place (1, 2, ...) in its base's chain: a missing number is a
    // lost row, even when the rows after it import cleanly. Rows that predate
    // it (0) end the chain, and go to the parked inbox.
    "ADD COLUMN IF MISSING snapshot_deltas seq INTEGER NOT NULL DEFAULT 0",
    // Opaque row-level deltas over a non-Loro snapshot (the registry's JSON
    // replica): each holds what changed since the save before it, chained
    // (seq 1, 2, ...) on the base whose saved_at it names. A whole save of
    // the doc clears them; a build without this table reads the base alone.
    "CREATE TABLE IF NOT EXISTS row_deltas (
        ordinal INTEGER PRIMARY KEY AUTOINCREMENT,
        doc_id TEXT NOT NULL,
        bytes BLOB NOT NULL,
        base_saved_at INTEGER NOT NULL,
        seq INTEGER NOT NULL
     ) STRICT;
     CREATE INDEX IF NOT EXISTS row_deltas_doc ON row_deltas(doc_id,ordinal);",
];

const STORE_BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// WAL size retained after a checkpoint resets the log. SQLite's default (-1)
/// keeps the file at its all-time high-water mark (observed: 400 MB). Steady
/// state never reaches this — auto-checkpoint folds the log every ~4 MB
/// (1000 pages) — so the cap only bites after a large burst (a multi-MB
/// snapshot, a VACUUM). 64 MiB bounds idle disk use while sparing ordinary
/// bursts a truncate/re-extend cycle on every write.
const JOURNAL_SIZE_LIMIT_BYTES: i64 = 64 * 1024 * 1024;

/// VACUUM ([`DocsStore::reclaim_free_space`]) only when free pages are BOTH a large share of the file
/// and a meaningful absolute size: a healthy database never pays the rewrite.
const VACUUM_MIN_FREE_BYTES: i64 = 32 * 1024 * 1024;
/// `freelist_count / page_count` above which reclaiming is worth a rewrite.
const VACUUM_MIN_FREE_DENOMINATOR: i64 = 4;

/// Maintenance is skipped (and logged) rather than waited on when another
/// connection holds a lock.
const MAINTENANCE_BUSY_TIMEOUT: Duration = Duration::from_millis(250);

/// SQLite-backed store under a data directory (`{data_dir}/docs.sqlite3`).
///
/// Holds warm-open doc snapshots (the DO room is authoritative; these make
/// cold starts instant and offline restarts possible) and the command ledger
/// that gives command execution mark-BEFORE-execute idempotence.
pub struct DocsStore {
    conn: Mutex<Connection>,
    /// Snapshot reads and writes: a whole-snapshot write no longer holds the
    /// connection every commit's outbox write needs. SQLite still runs one
    /// writer at a time, so those writes are kept small (deltas).
    snap_conn: Mutex<Connection>,
    failed_publications: Mutex<HashSet<String>>,
    /// Snapshot jobs wait here asynchronously before entering the blocking pool.
    pub snapshot_writer: std::sync::Arc<tokio::sync::Mutex<()>>,
    /// A token per doc, new with every full snapshot write. A delta is only
    /// accepted on top of the base it was taken against. Only one process
    /// opens a store (the engine's instance lock), so memory is enough.
    /// Token and `saved_at` of each doc's current base.
    bases: Mutex<std::collections::HashMap<String, (u64, i64)>>,
    next_base: std::sync::atomic::AtomicU64,
    /// Set before a restart into another version: every save from then on
    /// is a whole snapshot ([`Self::stop_snapshot_deltas`]).
    deltas_stopped: std::sync::atomic::AtomicBool,
    /// What a load found on disk, for a saver to adopt as its delta base.
    loaded: Mutex<std::collections::HashMap<String, LoadedBase>>,
    /// Loads that merged snapshot deltas (diagnostics: each costs a whole
    /// import and export of the doc).
    delta_merges: std::sync::atomic::AtomicU64,
    path: PathBuf,
    /// This open found the file damaged and rebuilt the store.
    recovery: Option<crate::store_recovery::Recovery>,
    /// Bumped whenever `chat_sync_jobs` gains, renews or loses a row. The
    /// engine's sync dispatchers wait on this instead of polling the table.
    sync_jobs_changed: tokio::sync::watch::Sender<u64>,
    /// Bumped whenever `chat_outbox` gains rows (a durable batch to send).
    outbox_changed: tokio::sync::watch::Sender<u64>,
}

impl DocsStore {
    /// Open (creating directory, database, and schema as needed).
    pub fn open(data_dir: impl AsRef<Path>) -> Result<Self, StoreError> {
        let data_dir = data_dir.as_ref();
        std::fs::create_dir_all(data_dir)?;
        let path = data_dir.join("docs.sqlite3");
        // A damaged file is moved aside before anything reads it; what can
        // still be read of it is copied into the fresh store below.
        let damaged = path
            .exists()
            .then(|| crate::store_recovery::damage(&path))
            .flatten()
            .map(|reason| -> Result<_, StoreError> {
                tracing::error!(%reason, path = %path.display(), "docs store: damaged; rebuilding it");
                Ok((reason, crate::store_recovery::quarantine(&path)?))
            })
            .transpose()?;
        let mut conn = Connection::open(&path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "journal_size_limit", JOURNAL_SIZE_LIMIT_BYTES)?;
        conn.busy_timeout(STORE_BUSY_TIMEOUT)?;
        migrate(&mut conn)?;
        // Best-effort hygiene must never stall boot behind another connection.
        conn.busy_timeout(MAINTENANCE_BUSY_TIMEOUT)?;
        // Only the cheap WAL truncate here: open sits on boot paths (engine
        // assembly, the iOS client), and a VACUUM's cost is linear in the
        // live size. Callers run `reclaim_free_space` off those paths.
        checkpoint_truncate(&conn, &path, "open");
        conn.busy_timeout(STORE_BUSY_TIMEOUT)?;
        let recovery = damaged.map(|(reason, kept_at)| {
            let (salvaged_rows, damaged_tables) = crate::store_recovery::salvage(&kept_at, &conn);
            tracing::warn!(
                salvaged_rows,
                ?damaged_tables,
                kept_at = %kept_at.display(),
                "docs store: rebuilt; what wasn't readable comes back from the rooms"
            );
            crate::store_recovery::Recovery { reason, kept_at, salvaged_rows, damaged_tables }
        });
        let snap_conn = Connection::open(&path)?;
        snap_conn.pragma_update(None, "synchronous", "NORMAL")?;
        snap_conn.pragma_update(None, "journal_size_limit", JOURNAL_SIZE_LIMIT_BYTES)?;
        snap_conn.busy_timeout(STORE_BUSY_TIMEOUT)?;
        Ok(Self {
            conn: Mutex::new(conn),
            snap_conn: Mutex::new(snap_conn),
            failed_publications: Mutex::new(HashSet::new()),
            snapshot_writer: std::sync::Arc::new(tokio::sync::Mutex::new(())),
            bases: Mutex::new(std::collections::HashMap::new()),
            next_base: std::sync::atomic::AtomicU64::new(1),
            deltas_stopped: std::sync::atomic::AtomicBool::new(false),
            loaded: Mutex::new(std::collections::HashMap::new()),
            delta_merges: std::sync::atomic::AtomicU64::new(0),
            path,
            recovery,
            sync_jobs_changed: tokio::sync::watch::Sender::new(0),
            outbox_changed: tokio::sync::watch::Sender::new(0),
        })
    }

    /// Changes whenever durable sync jobs (`schedule_sync_job`,
    /// `reconcile_page`, `complete_sync_job`) change. Wait on it rather than
    /// polling `pending_sync_jobs`.
    pub fn watch_sync_jobs(&self) -> tokio::sync::watch::Receiver<u64> {
        self.sync_jobs_changed.subscribe()
    }

    /// Changes whenever the chat outbox gains rows.
    pub fn watch_outbox(&self) -> tokio::sync::watch::Receiver<u64> {
        self.outbox_changed.subscribe()
    }

    pub(crate) fn note_sync_jobs_changed(&self) {
        self.sync_jobs_changed.send_modify(|v| *v = v.wrapping_add(1));
    }

    fn note_outbox_changed(&self) {
        self.outbox_changed.send_modify(|v| *v = v.wrapping_add(1));
    }

    /// What this open found and did, when the file was damaged.
    pub fn recovery(&self) -> Option<&crate::store_recovery::Recovery> {
        self.recovery.as_ref()
    }

    /// Before the machine sleeps (its disk snapshotted from outside, at any
    /// moment after) or the process stops: everything in the WAL goes into the
    /// database file and the WAL is emptied, so the file alone is the store.
    /// Returns whether the checkpoint completed.
    pub fn flush_for_sleep(&self) -> bool {
        // Both connections held (snapshot one first, the order every path
        // takes): no snapshot write can land between the checkpoint and the
        // sleep, so the WAL is empty when the disk is snapshotted.
        let _snapshots = self.snap_conn();
        let conn = self.conn();
        checkpoint_truncate(&conn, &self.path, "sleep");
        conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| row.get::<_, i64>(0))
            .is_ok_and(|busy| busy == 0)
    }

    /// The database file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Insert before sending. Stable IDs make crash-after-ACK replay idempotent.
    pub fn enqueue_chat_update(
        &self,
        doc_id: &str,
        batch_id: &str,
        bytes: &[u8],
    ) -> Result<(), StoreError> {
        store_blocking(|| self.enqueue_chat_update_blocking(doc_id, batch_id, bytes))
    }

    fn enqueue_chat_update_blocking(
        &self,
        doc_id: &str,
        batch_id: &str,
        bytes: &[u8],
    ) -> Result<(), StoreError> {
        let result = self.conn().execute(
            "INSERT OR IGNORE INTO chat_outbox(doc_id,batch_id,bytes) VALUES (?1,?2,?3)",
            params![doc_id, batch_id, bytes],
        );
        if matches!(result, Ok(n) if n > 0) {
            self.note_outbox_changed();
        }
        if result.is_err() {
            self.failed_publications
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(doc_id.to_string());
        }
        result?;
        Ok(())
    }

    pub fn pending_chat_updates(&self, doc_id: &str) -> Result<Vec<(String, Vec<u8>)>, StoreError> {
        store_blocking(|| self.pending_chat_updates_blocking(doc_id))
    }

    /// Admission/lifetime checks must not copy the pending payloads.
    pub fn has_pending_chat_updates(&self, doc_id: &str) -> Result<bool, StoreError> {
        store_blocking(|| {
            Ok(self.conn().query_row(
                "SELECT EXISTS(SELECT 1 FROM chat_outbox WHERE doc_id=?1)",
                params![doc_id],
                |row| row.get(0),
            )?)
        })
    }

    fn pending_chat_updates_blocking(
        &self,
        doc_id: &str,
    ) -> Result<Vec<(String, Vec<u8>)>, StoreError> {
        let conn = self.conn();
        let mut stmt = conn
            .prepare("SELECT batch_id,bytes FROM chat_outbox WHERE doc_id=?1 ORDER BY ordinal")?;
        Ok(stmt
            .query_map(params![doc_id], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<_, _>>()?)
    }

    pub fn reject_chat_update(&self, doc_id: &str, batch_id: &str) -> Result<(), StoreError> {
        store_blocking(|| self.reject_chat_update_blocking(doc_id, batch_id))
    }

    fn reject_chat_update_blocking(&self, doc_id: &str, batch_id: &str) -> Result<(), StoreError> {
        self.conn().execute(
            "UPDATE chat_outbox SET needs_checkpoint=1 WHERE doc_id=?1 AND batch_id=?2",
            params![doc_id, batch_id],
        )?;
        Ok(())
    }

    pub fn rejected_chat_updates(
        &self,
        doc_id: &str,
    ) -> Result<Vec<(String, Vec<u8>)>, StoreError> {
        store_blocking(|| self.rejected_chat_updates_blocking(doc_id))
    }

    fn rejected_chat_updates_blocking(
        &self,
        doc_id: &str,
    ) -> Result<Vec<(String, Vec<u8>)>, StoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT batch_id,bytes FROM chat_outbox WHERE doc_id=?1 AND needs_checkpoint=1 ORDER BY ordinal")?;
        Ok(stmt
            .query_map(params![doc_id], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<_, _>>()?)
    }

    /// Keep an update that arrived by relay until a saved snapshot covers it.
    /// Returns its position, for [`Self::retire_chat_inbox`].
    pub fn journal_chat_inbox(&self, doc_id: &str, bytes: &[u8]) -> Result<i64, StoreError> {
        store_blocking(|| {
            let conn = self.conn();
            conn.execute(
                "INSERT INTO chat_inbox(doc_id,bytes) VALUES (?1,?2)",
                params![doc_id, bytes],
            )?;
            Ok(conn.last_insert_rowid())
        })
    }

    /// What a crash left unsaved, oldest first, with the newest position.
    pub fn pending_chat_inbox(&self, doc_id: &str) -> Result<(Vec<Vec<u8>>, i64), StoreError> {
        store_blocking(|| {
            let conn = self.conn();
            let mut stmt =
                conn.prepare("SELECT ordinal,bytes FROM chat_inbox WHERE doc_id=?1 ORDER BY ordinal")?;
            let rows = stmt.query_map(params![doc_id], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?))
            })?;
            let mut updates = Vec::new();
            let mut newest = 0;
            for row in rows {
                let (ordinal, bytes) = row?;
                newest = ordinal;
                updates.push(bytes);
            }
            Ok((updates, newest))
        })
    }

    /// A saved snapshot holds everything up to `through`.
    pub fn retire_chat_inbox(&self, doc_id: &str, through: i64) -> Result<(), StoreError> {
        store_blocking(|| {
            self.conn().execute(
                "DELETE FROM chat_inbox WHERE doc_id=?1 AND ordinal<=?2",
                params![doc_id, through],
            )?;
            Ok(())
        })
    }

    pub fn acknowledge_chat_update(&self, doc_id: &str, batch_id: &str) -> Result<(), StoreError> {
        store_blocking(|| self.acknowledge_chat_update_blocking(doc_id, batch_id))
    }

    fn acknowledge_chat_update_blocking(
        &self,
        doc_id: &str,
        batch_id: &str,
    ) -> Result<(), StoreError> {
        self.conn().execute(
            "DELETE FROM chat_outbox WHERE doc_id=?1 AND batch_id=?2",
            params![doc_id, batch_id],
        )?;
        Ok(())
    }

    pub fn chat_outbox_initialized(&self, doc_id: &str) -> Result<bool, StoreError> {
        store_blocking(|| self.chat_outbox_initialized_blocking(doc_id))
    }

    fn chat_outbox_initialized_blocking(&self, doc_id: &str) -> Result<bool, StoreError> {
        Ok(self
            .conn()
            .query_row(
                "SELECT 1 FROM chat_outbox_initialized WHERE doc_id=?1",
                params![doc_id],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }

    /// Upgrade legacy snapshots (including orphaned local history) once. The
    /// marker and complete replay obligation commit together; never reset cursors.
    pub fn initialize_chat_outbox(
        &self,
        doc_id: &str,
        updates: &[Vec<u8>],
    ) -> Result<(), StoreError> {
        store_blocking(|| self.initialize_chat_outbox_blocking(doc_id, updates))
    }

    fn initialize_chat_outbox_blocking(
        &self,
        doc_id: &str,
        updates: &[Vec<u8>],
    ) -> Result<(), StoreError> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        if tx.execute(
            "INSERT OR IGNORE INTO chat_outbox_initialized(doc_id) VALUES (?1)",
            params![doc_id],
        )? != 0
        {
            for bytes in updates {
                tx.execute(
                    "INSERT INTO chat_outbox(doc_id,batch_id,bytes) VALUES (?1,?2,?3)",
                    params![doc_id, uuid::Uuid::new_v4().to_string(), bytes],
                )?;
            }
        }
        tx.commit()?;
        if !updates.is_empty() {
            self.note_outbox_changed();
        }
        Ok(())
    }

    /// Latest saved snapshot for `doc_id`, if any.
    pub fn load_snapshot(&self, doc_id: &str) -> Result<Option<Vec<u8>>, StoreError> {
        store_blocking(|| self.load_snapshot_blocking(doc_id))
    }

    fn load_snapshot_blocking(&self, doc_id: &str) -> Result<Option<Vec<u8>>, StoreError> {
        Ok(self.load_merged(doc_id)?.map(|(bytes, _, _)| bytes))
    }

    /// How many loads have merged snapshot deltas so far.
    pub fn delta_merges(&self) -> u64 {
        self.delta_merges.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// From now on, refuse deltas: savers write whole snapshots, which every
    /// version of Zeron reads. For the last moments before a restart into
    /// another version (which may be an older one).
    pub fn stop_snapshot_deltas(&self) {
        self.deltas_stopped
            .store(true, std::sync::atomic::Ordering::Release);
    }

    /// Whether [`Self::stop_snapshot_deltas`] has been called.
    pub fn snapshot_deltas_stopped(&self) -> bool {
        self.deltas_stopped.load(std::sync::atomic::Ordering::Acquire)
    }

    /// Save Loro updates on top of `doc_id`'s snapshot, with the cursor the
    /// doc's state covers once they are applied. `base` is the token the
    /// caller's last full save returned ([`Self::save_base_snapshot`]); any
    /// full write since then (another writer, a rebuild, a delete) replaced
    /// that base, and this returns `Ok(false)` without writing: the caller
    /// saves a full snapshot instead.
    pub fn append_snapshot_delta(
        &self,
        doc_id: &str,
        base: u64,
        bytes: &[u8],
        cursor: u64,
    ) -> Result<bool, StoreError> {
        store_blocking(|| {
            if self.deltas_stopped.load(std::sync::atomic::Ordering::Acquire) {
                return Ok(false);
            }
            let mut conn = self.snap_conn();
            // From memory, not the row: `saved_at` sits after the blob, and
            // reading it walks the whole snapshot's overflow pages.
            let saved_at = match self.bases.lock().unwrap_or_else(PoisonError::into_inner).get(doc_id) {
                Some(&(token, saved_at)) if token == base => saved_at,
                _ => return Ok(false),
            };
            // IMMEDIATE: take the write lock up front, so a busy store waits
            // out its busy timeout here instead of failing at the INSERT.
            let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            // The primary key alone: no read of the blob.
            let has_base: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM snapshots WHERE doc_id = ?1)",
                params![doc_id],
                |row| row.get(0),
            )?;
            if !has_base {
                return Ok(false);
            }
            tx.execute(
                "INSERT INTO snapshot_deltas (doc_id, bytes, cursor, base_saved_at, seq)
                 VALUES (?1, ?2, ?3, ?4, (SELECT COALESCE(MAX(seq), 0) + 1 FROM snapshot_deltas
                                          WHERE doc_id = ?1 AND base_saved_at = ?4))",
                params![doc_id, bytes, cursor as i64, saved_at],
            )?;
            self.invalidate_failed_publication(&tx, doc_id)?;
            tx.commit()?;
            Ok(true)
        })
    }

    /// Like [`Self::save_verified_snapshot_with_cursor`], and returns the base
    /// token deltas on top of this snapshot must name.
    pub fn save_base_snapshot(
        &self,
        doc_id: &str,
        bytes: &[u8],
        cursor: u64,
        epoch: u32,
    ) -> Result<u64, StoreError> {
        store_blocking(|| {
            self.save_verified_snapshot_with_cursor_inner(doc_id, bytes, cursor, epoch)
        })
    }

    /// (rows, bytes) of saved deltas, for diagnostics and tests.
    pub fn snapshot_delta_stats(&self, doc_id: &str) -> Result<(u64, u64), StoreError> {
        store_blocking(|| {
            Ok(self.snap_conn().query_row(
                "SELECT count(*), COALESCE(sum(length(bytes)), 0) FROM snapshot_deltas WHERE doc_id = ?1",
                params![doc_id],
                |r| Ok((r.get::<_, i64>(0)? as u64, r.get::<_, i64>(1)? as u64)),
            )?)
        })
    }

    /// A full write replaced `doc_id`'s base: its deltas go in the same
    /// transaction, and earlier base tokens stop matching.
    fn replace_base(&self, tx: &rusqlite::Transaction<'_>, doc_id: &str) -> Result<(), StoreError> {
        tx.execute("DELETE FROM snapshot_deltas WHERE doc_id = ?1", params![doc_id])?;
        tx.execute("DELETE FROM row_deltas WHERE doc_id = ?1", params![doc_id])?;
        Ok(())
    }

    /// Called after the full write commits, with the connection still held.
    fn new_base(&self, doc_id: &str, saved_at: i64) -> u64 {
        self.loaded
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(doc_id);
        let token = self
            .next_base
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.bases
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(doc_id.to_owned(), (token, saved_at));
        token
    }

    /// Save (upsert) the snapshot for `doc_id`.
    pub fn save_snapshot(&self, doc_id: &str, bytes: &[u8]) -> Result<(), StoreError> {
        store_blocking(|| self.save_snapshot_blocking(doc_id, bytes))
    }

    fn save_snapshot_blocking(&self, doc_id: &str, bytes: &[u8]) -> Result<(), StoreError> {
        self.save_snapshot_inner(doc_id, bytes).map(drop)
    }

    /// Like [`Self::save_snapshot`], and returns the base token row deltas
    /// on top of this snapshot must name ([`Self::append_row_delta`]).
    pub fn save_snapshot_base(&self, doc_id: &str, bytes: &[u8]) -> Result<u64, StoreError> {
        store_blocking(|| self.save_snapshot_inner(doc_id, bytes))
    }

    /// Store what changed since the last save of `doc_id`, after its whole
    /// snapshot. `base` is the token that snapshot's save returned; any full
    /// write since (another writer, a delete), or a coming version change
    /// ([`Self::stop_snapshot_deltas`]), refuses with `Ok(false)` and the
    /// caller saves whole.
    pub fn append_row_delta(&self, doc_id: &str, base: u64, bytes: &[u8]) -> Result<bool, StoreError> {
        store_blocking(|| {
            if self.deltas_stopped.load(std::sync::atomic::Ordering::Acquire) {
                return Ok(false);
            }
            let mut conn = self.snap_conn();
            let saved_at = match self.bases.lock().unwrap_or_else(PoisonError::into_inner).get(doc_id) {
                Some(&(token, saved_at)) if token == base => saved_at,
                _ => return Ok(false),
            };
            let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            // The base on disk must still be the one the token names: another
            // process writing the same file (an older build) may have replaced
            // it, and deltas on a stale base would never load. (Row-delta docs
            // are small JSON replicas: reading past their blob is cheap.)
            let current: Option<i64> = tx
                .query_row(
                    "SELECT saved_at FROM snapshots WHERE doc_id = ?1",
                    params![doc_id],
                    |row| row.get(0),
                )
                .optional()?;
            if current != Some(saved_at) {
                return Ok(false);
            }
            tx.execute(
                "INSERT INTO row_deltas (doc_id, bytes, base_saved_at, seq)
                 VALUES (?1, ?2, ?3, (SELECT COALESCE(MAX(seq), 0) + 1 FROM row_deltas
                                      WHERE doc_id = ?1 AND base_saved_at = ?3))",
                params![doc_id, bytes, saved_at],
            )?;
            tx.commit()?;
            Ok(true)
        })
    }

    /// `doc_id`'s whole snapshot and the row deltas saved on it: the chain
    /// (contiguous from seq 1, oldest first) and any rows after a break
    /// (a lost row). Deltas on a base something else rewrote (a build
    /// without this table) are left out. One read transaction.
    #[allow(clippy::type_complexity)]
    pub fn load_row_deltas(
        &self,
        doc_id: &str,
    ) -> Result<Option<(Vec<u8>, Vec<Vec<u8>>, Vec<Vec<u8>>)>, StoreError> {
        store_blocking(|| {
            let mut conn = self.snap_conn();
            let read = conn.transaction()?;
            let row: Option<(Vec<u8>, i64)> = read
                .query_row(
                    "SELECT bytes, saved_at FROM snapshots WHERE doc_id = ?1",
                    params![doc_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            let Some((base, saved_at)) = row else { return Ok(None) };
            let deltas: Vec<(i64, Vec<u8>)> = read
                .prepare(
                    "SELECT seq, bytes FROM row_deltas WHERE doc_id = ?1 AND base_saved_at = ?2 ORDER BY ordinal",
                )?
                .query_map(params![doc_id, saved_at], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect::<Result<_, _>>()?;
            read.commit()?;
            let mut chain = Vec::new();
            let mut tail = Vec::new();
            for (seq, bytes) in deltas {
                if tail.is_empty() && seq == chain.len() as i64 + 1 {
                    chain.push(bytes);
                } else {
                    tail.push(bytes);
                }
            }
            Ok(Some((base, chain, tail)))
        })
    }

    /// (rows, bytes) of row deltas saved for `doc_id` (diagnostics, tests).
    pub fn row_delta_stats(&self, doc_id: &str) -> Result<(u64, u64), StoreError> {
        store_blocking(|| {
            Ok(self.snap_conn().query_row(
                "SELECT count(*), COALESCE(sum(length(bytes)), 0) FROM row_deltas WHERE doc_id = ?1",
                params![doc_id],
                |r| Ok((r.get::<_, i64>(0)? as u64, r.get::<_, i64>(1)? as u64)),
            )?)
        })
    }

    fn save_snapshot_inner(&self, doc_id: &str, bytes: &[u8]) -> Result<u64, StoreError> {
        let mut conn = self.snap_conn();
        let saved_at = now_ms();
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO snapshots (doc_id, bytes, saved_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(doc_id) DO UPDATE SET bytes = excluded.bytes, saved_at = excluded.saved_at",
            params![doc_id, bytes, saved_at],
        )?;
        self.replace_base(&tx, doc_id)?;
        self.invalidate_failed_publication(&tx, doc_id)?;
        tx.commit()?;
        Ok(self.new_base(doc_id, saved_at))
    }

    /// Save the snapshot together with its chat2 room cursor and doc epoch —
    /// ONE transaction, so bytes and cursor can never disagree (the C2 rule;
    /// a divergent pair is exactly the restored-backup redownload bug).
    pub fn save_snapshot_with_cursor(
        &self,
        doc_id: &str,
        bytes: &[u8],
        cursor: u64,
        epoch: u32,
    ) -> Result<(), StoreError> {
        store_blocking(|| self.save_snapshot_with_cursor_blocking(doc_id, bytes, cursor, epoch))
    }

    fn save_snapshot_with_cursor_blocking(
        &self,
        doc_id: &str,
        bytes: &[u8],
        cursor: u64,
        epoch: u32,
    ) -> Result<(), StoreError> {
        let mut conn = self.snap_conn();
        let saved_at = now_ms();
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO snapshots (doc_id, bytes, saved_at, cursor, epoch) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(doc_id) DO UPDATE SET bytes = excluded.bytes, saved_at = excluded.saved_at,
                 cursor = excluded.cursor, epoch = excluded.epoch, cursor_verified = 0",
            params![doc_id, bytes, saved_at, cursor as i64, epoch as i64],
        )?;
        self.replace_base(&tx, doc_id)?;
        self.invalidate_failed_publication(&tx, doc_id)?;
        tx.commit()?;
        self.new_base(doc_id, saved_at);
        Ok(())
    }

    pub fn save_verified_snapshot_with_cursor(
        &self,
        doc_id: &str,
        bytes: &[u8],
        cursor: u64,
        epoch: u32,
    ) -> Result<(), StoreError> {
        store_blocking(|| {
            self.save_verified_snapshot_with_cursor_inner(doc_id, bytes, cursor, epoch)
                .map(drop)
        })
    }

    fn save_verified_snapshot_with_cursor_inner(
        &self,
        doc_id: &str,
        bytes: &[u8],
        cursor: u64,
        epoch: u32,
    ) -> Result<u64, StoreError> {
        let mut conn = self.snap_conn();
        let saved_at = now_ms();
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO snapshots (doc_id, bytes, saved_at, cursor, epoch, cursor_verified) VALUES (?1, ?2, ?3, ?4, ?5, 1)
             ON CONFLICT(doc_id) DO UPDATE SET bytes = excluded.bytes, saved_at = excluded.saved_at,
                 cursor = excluded.cursor, epoch = excluded.epoch, cursor_verified = 1",
            params![doc_id, bytes, saved_at, cursor as i64, epoch as i64],
        )?;
        self.replace_base(&tx, doc_id)?;
        self.invalidate_failed_publication(&tx, doc_id)?;
        tx.commit()?;
        Ok(self.new_base(doc_id, saved_at))
    }

    /// The cursor of the newest saved state: the last delta's when there are
    /// deltas (a reset may have lowered it), else the snapshot row's.
    /// The cursor a load of `doc_id` comes back with
    /// ([`Self::load_snapshot_with_cursor`]): with deltas on disk only a merge
    /// can tell which ones apply, so it runs one (a chat open's rare case).
    pub fn snapshot_cursor(&self, doc_id: &str) -> Result<u64, StoreError> {
        store_blocking(|| {
            let has_deltas = self
                .conn()
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM snapshot_deltas WHERE doc_id = ?1)",
                    params![doc_id],
                    |row| row.get::<_, bool>(0),
                )?;
            if has_deltas {
                return Ok(self
                    .load_snapshot_with_cursor_blocking(doc_id)?
                    .map_or(0, |(_, cursor, _)| cursor));
            }
            Ok(self
                .conn()
                .query_row(
                    "SELECT COALESCE(cursor, 0) FROM snapshots WHERE doc_id = ?1",
                    params![doc_id],
                    |row| row.get::<_, i64>(0),
                )
                .optional()?
                .unwrap_or(0) as u64)
        })
    }

    pub fn snapshot_cursor_verified(&self, doc_id: &str) -> Result<bool, StoreError> {
        store_blocking(|| self.snapshot_cursor_verified_blocking(doc_id))
    }

    fn snapshot_cursor_verified_blocking(&self, doc_id: &str) -> Result<bool, StoreError> {
        Ok(self
            .conn()
            .query_row(
                "SELECT cursor_verified FROM snapshots WHERE doc_id = ?1",
                params![doc_id],
                |row| row.get(0),
            )
            .optional()?
            .unwrap_or(false))
    }

    /// Snapshot + chat2 cursor + epoch. Pre-migration rows (or rows written
    /// by [`Self::save_snapshot`]) read back as `(bytes, 0, 0)`.
    pub fn load_snapshot_with_cursor(
        &self,
        doc_id: &str,
    ) -> Result<Option<(Vec<u8>, u64, u32)>, StoreError> {
        store_blocking(|| self.load_snapshot_with_cursor_blocking(doc_id))
    }

    /// Read admission metadata without fetching or decoding the snapshot blob.
    pub fn snapshot_epoch(&self, doc_id: &str) -> Result<u32, StoreError> {
        store_blocking(|| {
            Ok(self
                .conn()
                .query_row(
                    "SELECT COALESCE(epoch, 0) FROM snapshots WHERE doc_id = ?1",
                    params![doc_id],
                    |row| row.get::<_, u32>(0),
                )
                .optional()?
                .unwrap_or(0))
        })
    }

    fn load_snapshot_with_cursor_blocking(
        &self,
        doc_id: &str,
    ) -> Result<Option<(Vec<u8>, u64, u32)>, StoreError> {
        self.load_merged(doc_id)
    }

    /// The snapshot row with the clean run of its deltas applied, and the
    /// cursor that state covers. Bytes and cursor stay a pair: the merged
    /// state goes with the last APPLIED delta's cursor (and only if it was
    /// saved on this base: an older build may have rewritten the row), an
    /// unmergeable one with the base's own. Rows after a stop are moved to
    /// the doc's parked inbox, which an open replays until nothing waits:
    /// they may hold ops (relay rows, local ops) nothing else still has.
    fn load_merged(&self, doc_id: &str) -> Result<Option<(Vec<u8>, u64, u32)>, StoreError> {
        let mut conn = self.snap_conn();
        // The row and its deltas from ONE read transaction: a write on the
        // other connection between the two reads can't pair them wrongly.
        let read = conn.transaction()?;
        let row = read
            .query_row(
                "SELECT bytes, cursor, epoch, saved_at FROM snapshots WHERE doc_id = ?1",
                params![doc_id],
                |row| {
                    Ok((
                        row.get::<_, Vec<u8>>(0)?,
                        row.get::<_, Option<i64>>(1)?.unwrap_or(0) as u64,
                        row.get::<_, Option<i64>>(2)?.unwrap_or(0) as u32,
                        row.get::<_, i64>(3)?,
                    ))
                },
            )
            .optional()?;
        let Some((bytes, cursor, epoch, saved_at)) = row else { return Ok(None) };
        let deltas = load_deltas(&read, doc_id)?;
        read.commit()?;
        drop(conn);
        if deltas.is_empty() {
            if let Ok(meta) = loro::LoroDoc::decode_import_blob_meta(&bytes, false) {
                self.offer_base(doc_id, saved_at, epoch, meta.partial_end_vv, 0, 0, bytes.len());
            }
            return Ok(Some((bytes, cursor, epoch)));
        }
        self.delta_merges
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let merge = merge_deltas(doc_id, &bytes, &deltas);
        if let Some((at, kind)) = merge.stop {
            self.park_delta_tail(doc_id, &deltas[at..], kind)?;
        }
        // New deltas may follow these only if the whole chain merged and
        // all of it extends this base; else the next save is whole.
        if merge.stop.is_none()
            && let Some(version) = &merge.version
            && deltas.iter().all(|delta| delta.base_saved_at == saved_at)
        {
            let delta_bytes = deltas.iter().map(|delta| delta.bytes.len()).sum();
            self.offer_base(doc_id, saved_at, epoch, version.clone(), deltas.len(), delta_bytes, bytes.len());
        }
        Ok(Some(match merge.merged {
            Some(merged) => {
                let last = &deltas[merge.applied - 1];
                let cursor = if last.base_saved_at == saved_at { last.cursor } else { cursor };
                (merged, cursor, epoch)
            }
            None => (bytes, cursor, epoch),
        }))
    }

    /// Record a loaded state as a delta base, unless this process already
    /// tracks a base for the doc (a live saver's, or a newer full write):
    /// never displace one.
    fn offer_base(
        &self,
        doc_id: &str,
        saved_at: i64,
        epoch: u32,
        version: loro::VersionVector,
        delta_rows: usize,
        delta_bytes: usize,
        base_bytes: usize,
    ) {
        let mut bases = self.bases.lock().unwrap_or_else(PoisonError::into_inner);
        if bases.contains_key(doc_id) {
            return;
        }
        let token = self
            .next_base
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        bases.insert(doc_id.to_owned(), (token, saved_at));
        self.loaded
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(
                doc_id.to_owned(),
                LoadedBase { token, epoch, version, delta_rows, delta_bytes, base_bytes },
            );
    }

    /// The state a load of `doc_id` found on disk, if it is still the doc's
    /// current base: a saver that adopts it writes deltas from there instead
    /// of a whole first save. Taken once.
    pub fn adopt_loaded_base(&self, doc_id: &str) -> Option<LoadedBase> {
        let loaded = self
            .loaded
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(doc_id)?;
        let current = self
            .bases
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(doc_id)
            .map(|(token, _)| *token);
        (current == Some(loaded.token)).then_some(loaded)
    }

    /// Move the rows from a merge's stop onward out of the delta chain, into
    /// the doc's parked inbox (an undecodable row is dropped: nothing can
    /// read it). One transaction: the chain then ends at its clean run.
    fn park_delta_tail(&self, doc_id: &str, tail: &[Delta], kind: MergeStop) -> Result<(), StoreError> {
        let mut conn = self.snap_conn();
        let tx = conn.transaction()?;
        let parked = format!("{doc_id}#parked");
        for (i, delta) in tail.iter().enumerate() {
            if !(i == 0 && kind == MergeStop::Undecodable) {
                tx.execute(
                    "INSERT INTO chat_inbox (doc_id, bytes) VALUES (?1, ?2)",
                    params![parked, delta.bytes],
                )?;
            }
            tx.execute("DELETE FROM snapshot_deltas WHERE ordinal = ?1", params![delta.ordinal])?;
        }
        tx.commit()?;
        tracing::warn!(doc = doc_id, rows = tail.len(), ?kind, "snapshot deltas after a gap moved to the parked inbox");
        Ok(())
    }

    /// Delete the snapshot row for `doc_id` (destructive schema breaks: the
    /// legacy `workspace` row is dropped on open). Missing rows are a no-op.
    pub fn delete_snapshot(&self, doc_id: &str) -> Result<(), StoreError> {
        store_blocking(|| self.delete_snapshot_blocking(doc_id))
    }

    fn delete_snapshot_blocking(&self, doc_id: &str) -> Result<(), StoreError> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM snapshots WHERE doc_id = ?1", params![doc_id])?;
        self.replace_base(&tx, doc_id)?;
        tx.execute("DELETE FROM chat_outbox WHERE doc_id = ?1", params![doc_id])?;
        tx.execute(
            "DELETE FROM chat_inbox WHERE doc_id = ?1 OR doc_id = ?1 || '#parked'",
            params![doc_id],
        )?;
        tx.execute(
            "DELETE FROM chat_sync_jobs WHERE doc_id = ?1",
            params![doc_id],
        )?;
        tx.execute(
            "DELETE FROM chat_outbox_initialized WHERE doc_id = ?1",
            params![doc_id],
        )?;
        tx.commit()?;
        self.bases
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(doc_id);
        self.loaded
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(doc_id);
        Ok(())
    }

    /// Whether a snapshot row exists for `doc_id` — presence only, no blob read.
    pub fn has_snapshot(&self, doc_id: &str) -> Result<bool, StoreError> {
        store_blocking(|| self.has_snapshot_blocking(doc_id))
    }

    fn has_snapshot_blocking(&self, doc_id: &str) -> Result<bool, StoreError> {
        let hit = self
            .conn()
            .query_row(
                "SELECT 1 FROM snapshots WHERE doc_id = ?1",
                params![doc_id],
                |_| Ok(()),
            )
            .optional()?;
        Ok(hit.is_some())
    }

    /// The full command ledger — profile-import reads the source's claims so
    /// imported pending commands can never re-execute under the new profile.
    pub fn processed_commands(&self) -> Result<Vec<(String, i64)>, StoreError> {
        store_blocking(|| self.processed_commands_blocking())
    }

    fn processed_commands_blocking(&self) -> Result<Vec<(String, i64)>, StoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT command_id, processed_at FROM processed_commands")?;
        let rows = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Merge foreign ledger claims (profile import). Existing claims win;
    /// returns how many rows were newly inserted.
    pub fn import_processed_commands(&self, rows: &[(String, i64)]) -> Result<usize, StoreError> {
        store_blocking(|| self.import_processed_commands_blocking(rows))
    }

    fn import_processed_commands_blocking(
        &self,
        rows: &[(String, i64)],
    ) -> Result<usize, StoreError> {
        let mut inserted = 0;
        let conn = self.conn();
        for (command_id, processed_at) in rows {
            inserted += conn.execute(
                "INSERT OR IGNORE INTO processed_commands (command_id, processed_at) VALUES (?1, ?2)",
                params![command_id, processed_at],
            )?;
        }
        Ok(inserted)
    }

    /// Whether `command_id` has already been claimed for execution.
    pub fn is_processed(&self, command_id: &str) -> Result<bool, StoreError> {
        store_blocking(|| self.is_processed_blocking(command_id))
    }

    fn is_processed_blocking(&self, command_id: &str) -> Result<bool, StoreError> {
        let hit = self
            .conn()
            .query_row(
                "SELECT 1 FROM processed_commands WHERE command_id = ?1",
                params![command_id],
                |_| Ok(()),
            )
            .optional()?;
        Ok(hit.is_some())
    }

    /// Claim `command_id` for execution — call BEFORE executing (ledger rule:
    /// a crash mid-execution must never re-run the command). Returns `true`
    /// if this call claimed it, `false` if it was already processed.
    pub fn mark_processed(&self, command_id: &str) -> Result<bool, StoreError> {
        store_blocking(|| self.mark_processed_blocking(command_id))
    }

    fn mark_processed_blocking(&self, command_id: &str) -> Result<bool, StoreError> {
        let changed = self.conn().execute(
            "INSERT OR IGNORE INTO processed_commands (command_id, processed_at) VALUES (?1, ?2)",
            params![command_id, now_ms()],
        )?;
        Ok(changed > 0)
    }

    // Every snapshot path, including ACK/cursor persistence, must retain a
    // replay obligation for operations whose outbox write failed. Invalidate
    // the marker atomically with those operations becoming durable in a snapshot.
    fn invalidate_failed_publication(
        &self,
        tx: &rusqlite::Transaction<'_>,
        doc_id: &str,
    ) -> Result<(), StoreError> {
        if self
            .failed_publications
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(doc_id)
        {
            tx.execute(
                "DELETE FROM chat_outbox_initialized WHERE doc_id=?1",
                params![doc_id],
            )?;
        }
        Ok(())
    }

    /// Reclaim free space: WAL truncate, then a VACUUM only past the
    /// free-page thresholds, under a short busy timeout. Cheap when there is
    /// nothing to reclaim. Blocking and holds the store for the VACUUM (it
    /// rewrites the live database) — run it off any latency-sensitive path;
    /// best-effort — problems are logged, never returned.
    pub fn reclaim_free_space(&self) {
        store_blocking(|| {
            let conn = self.conn();
            let Some(path) = conn.path().map(std::path::PathBuf::from) else {
                return;
            };
            if let Err(err) = conn.busy_timeout(MAINTENANCE_BUSY_TIMEOUT) {
                tracing::warn!(%err, "docs store: reclaim skipped");
                return;
            }
            maintain(&conn, &path);
            if let Err(err) = conn.busy_timeout(STORE_BUSY_TIMEOUT) {
                tracing::warn!(%err, "docs store: busy timeout not restored after reclaim");
            }
        });
    }

    fn snap_conn(&self) -> MutexGuard<'_, Connection> {
        self.snap_conn.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn conn(&self) -> MutexGuard<'_, Connection> {
        // A poisoned lock only means another thread panicked mid-query; the
        // connection itself is still usable.
        self.conn.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// One saved delta row.
#[derive(Debug, Clone)]
struct Delta {
    ordinal: i64,
    bytes: Vec<u8>,
    cursor: u64,
    base_saved_at: i64,
    seq: i64,
}

/// `doc_id`'s saved deltas, oldest first.
fn load_deltas(conn: &Connection, doc_id: &str) -> Result<Vec<Delta>, StoreError> {
    let mut q = conn.prepare_cached(
        "SELECT ordinal, bytes, cursor, base_saved_at, seq FROM snapshot_deltas WHERE doc_id = ?1 ORDER BY ordinal",
    )?;
    let rows = q
        .query_map(params![doc_id], |r| {
            Ok(Delta {
                ordinal: r.get(0)?,
                bytes: r.get(1)?,
                cursor: r.get::<_, i64>(2)? as u64,
                base_saved_at: r.get(3)?,
                seq: r.get(4)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Why a merge stopped before the end of the chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MergeStop {
    /// A number missing from the chain: a lost row.
    Gap,
    /// Ops that build on ops the store doesn't hold.
    Pending,
    /// Bytes that don't decode.
    Undecodable,
}

struct Merge {
    merged: Option<Vec<u8>>,
    /// The merged state's version.
    version: Option<loro::VersionVector>,
    /// How many deltas, from the first, applied cleanly.
    applied: usize,
    /// Where the run stopped, and why.
    stop: Option<(usize, MergeStop)>,
}

/// The base with the longest run of its deltas that applies cleanly, in
/// order. The run ends at a missing chain number (a lost row: rows after it
/// may import cleanly, being concurrent ops, yet the cursor would then mark
/// the lost ops received), at a delta whose ops build on ops the store
/// doesn't hold (Loro parks them and an export drops them), or at one that
/// doesn't decode. `merged` is `None` when not one delta applies.
fn merge_deltas(doc_id: &str, base: &[u8], deltas: &[Delta]) -> Merge {
    let none = |stop| Merge { merged: None, version: None, applied: 0, stop };
    let doc = loro::LoroDoc::new();
    if let Err(error) = doc.import(base) {
        tracing::warn!(doc = doc_id, %error, "snapshot base did not decode for its deltas; loading it alone");
        return none(None);
    }
    let mut next_seq: std::collections::HashMap<i64, i64> = std::collections::HashMap::new();
    let mut applied = 0;
    let mut stop = None;
    for (i, delta) in deltas.iter().enumerate() {
        let expected = next_seq.entry(delta.base_saved_at).or_insert(1);
        if delta.seq != *expected {
            stop = Some((i, MergeStop::Gap));
            break;
        }
        match doc.import(&delta.bytes) {
            Ok(status) if status.pending.is_none() => {
                *expected += 1;
                applied += 1;
            }
            Ok(_) => {
                stop = Some((i, MergeStop::Pending));
                break;
            }
            Err(error) => {
                tracing::warn!(doc = doc_id, %error, "a snapshot delta did not decode");
                stop = Some((i, MergeStop::Undecodable));
                break;
            }
        }
    }
    if let Some((at, kind)) = stop {
        tracing::warn!(doc = doc_id, applied, at, ?kind, deltas = deltas.len(), "snapshot deltas stop early; loading the clean run");
    }
    if applied == 0 {
        return none(stop);
    }
    // A stop may have left parked ops inside `doc`: rebuild from the clean run.
    let doc = if stop.is_some() {
        let clean = loro::LoroDoc::new();
        if clean.import(base).is_err() {
            return none(stop);
        }
        for delta in &deltas[..applied] {
            if clean.import(&delta.bytes).is_err() {
                return none(stop);
            }
        }
        clean
    } else {
        doc
    };
    match doc.export(loro::ExportMode::Snapshot) {
        Ok(bytes) => Merge { merged: Some(bytes), version: Some(doc.oplog_vv()), applied, stop },
        Err(error) => {
            tracing::warn!(doc = doc_id, %error, "merged snapshot did not export; loading the base");
            none(stop)
        }
    }
}

fn migrate(conn: &mut Connection) -> Result<(), StoreError> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
            version    INTEGER PRIMARY KEY,
            applied_at INTEGER NOT NULL
         ) STRICT",
    )?;
    let current: i64 = conn.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
        [],
        |row| row.get(0),
    )?;
    for (index, sql) in MIGRATIONS.iter().enumerate() {
        let version = index as i64 + 1;
        if version <= current {
            continue;
        }
        let tx = conn.transaction()?;
        if let Some(column) = sql.strip_prefix("ADD COLUMN IF MISSING ") {
            // `table column declaration`: an earlier build may have the
            // column already (pre-release schemas were edited in place).
            let mut parts = column.splitn(3, ' ');
            let (Some(table), Some(name), Some(declaration)) =
                (parts.next(), parts.next(), parts.next())
            else {
                unreachable!("malformed migration: {sql}");
            };
            let present = tx
                .prepare(&format!("PRAGMA table_info(\"{table}\")"))?
                .query_map([], |row| row.get::<_, String>(1))?
                .collect::<Result<Vec<_>, _>>()?
                .iter()
                .any(|column| column == name);
            if !present {
                tx.execute_batch(&format!("ALTER TABLE \"{table}\" ADD COLUMN {name} {declaration}"))?;
            }
        } else {
            tx.execute_batch(sql)?;
        }
        tx.execute(
            "INSERT INTO schema_migrations (version, applied_at) VALUES (?1, ?2)",
            params![version, now_ms()],
        )?;
        tx.commit()?;
    }
    Ok(())
}

/// Fold and truncate a bloated WAL, then reclaim free pages if they dominate
/// the file. Every step is best-effort — failures (including SQLITE_BUSY from
/// another connection) are logged. Steady-state WAL growth is handled by
/// SQLite's auto-checkpoint plus `journal_size_limit`, so there is no
/// periodic pass.
fn maintain(conn: &Connection, path: &Path) {
    checkpoint_truncate(conn, path, "open");
    match reclaim_free_pages(conn) {
        // VACUUM in WAL mode writes the whole live database into the WAL.
        Ok(true) => checkpoint_truncate(conn, path, "vacuum"),
        Ok(false) => {}
        Err(err) => tracing::warn!(%err, "docs store: VACUUM skipped"),
    }
}

/// `PRAGMA wal_checkpoint(TRUNCATE)`; logs instead of failing.
fn checkpoint_truncate(conn: &Connection, path: &Path, reason: &str) {
    let wal_path = path.with_extension("sqlite3-wal");
    let wal_bytes = || std::fs::metadata(&wal_path).map_or(0, |m| m.len());
    let before = wal_bytes();
    let result = conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
    });
    match result {
        Ok((0, frames)) => {
            if before > 0 {
                tracing::info!(
                    reason,
                    frames,
                    wal_bytes_before = before,
                    wal_bytes_after = wal_bytes(),
                    "docs store: WAL checkpointed and truncated"
                );
            }
        }
        Ok((_, frames)) => tracing::warn!(
            reason,
            frames,
            wal_bytes = before,
            "docs store: WAL checkpoint busy (another connection is active); left for later"
        ),
        Err(err) => tracing::warn!(reason, %err, "docs store: WAL checkpoint failed"),
    }
}

/// VACUUM when free pages are both a large fraction of the file and above an
/// absolute floor. Returns whether a VACUUM ran. Rewrite cost is linear in the
/// LIVE size (~0.6 s for 150 MB live / 70 MB free on an M-series SSD, warm
/// cache); healthy databases skip it entirely.
fn reclaim_free_pages(conn: &Connection) -> rusqlite::Result<bool> {
    let pragma = |name: &str| conn.pragma_query_value(None, name, |row| row.get::<_, i64>(0));
    let page_size = pragma("page_size")?;
    let page_count = pragma("page_count")?;
    let freelist = pragma("freelist_count")?;
    if !worth_vacuuming(page_size, page_count, freelist) {
        return Ok(false);
    }
    let started = Instant::now();
    conn.execute_batch("VACUUM")?;
    let after = pragma("page_count")? * page_size;
    tracing::info!(
        bytes_before = page_count * page_size,
        bytes_after = after,
        free_bytes = freelist * page_size,
        elapsed_ms = started.elapsed().as_millis() as u64,
        "docs store: reclaimed free pages with VACUUM"
    );
    Ok(true)
}

fn worth_vacuuming(page_size: i64, page_count: i64, freelist: i64) -> bool {
    freelist * page_size >= VACUUM_MIN_FREE_BYTES
        && freelist * VACUUM_MIN_FREE_DENOMINATOR >= page_count
}

pub(crate) fn now_millis() -> i64 {
    now_ms()
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A build that predates `snapshot_deltas` reads only the snapshots row.
    /// Its bytes and cursor must stay a pair: the base's own cursor, never
    /// one only the deltas reach. New readers see base + deltas.
    #[test]
    fn deltas_leave_the_snapshot_row_an_honest_older_pair() {
        let dir = tempfile::tempdir().unwrap();
        let store = DocsStore::open(dir.path()).unwrap();
        let doc = loro::LoroDoc::new();
        doc.get_text("t").insert(0, "base").unwrap();
        doc.commit();
        let base_vv = doc.oplog_vv();
        let base_bytes = doc.export(loro::ExportMode::Snapshot).unwrap();
        let base = store.save_base_snapshot("chat", &base_bytes, 10, 2).unwrap();
        doc.get_text("t").insert(4, " + delta").unwrap();
        doc.commit();
        let update = doc.export(loro::ExportMode::updates(&base_vv)).unwrap();
        assert!(store.append_snapshot_delta("chat", base, &update, 12).unwrap());

        let (old_bytes, old_cursor): (Vec<u8>, i64) = store
            .conn()
            .query_row("SELECT bytes, cursor FROM snapshots WHERE doc_id='chat'", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!((old_bytes, old_cursor), (base_bytes, 10));

        let (bytes, cursor, epoch) = store.load_snapshot_with_cursor("chat").unwrap().unwrap();
        assert_eq!((cursor, epoch), (12, 2));
        let loaded = loro::LoroDoc::new();
        loaded.import(&bytes).unwrap();
        assert_eq!(loaded.get_text("t").to_string(), "base + delta");
        let plain = loro::LoroDoc::new();
        plain.import(&store.load_snapshot("chat").unwrap().unwrap()).unwrap();
        assert_eq!(plain.get_text("t").to_string(), "base + delta");
        assert_eq!(store.snapshot_cursor("chat").unwrap(), 12);

        // A stale base token writes nothing; a delete forgets the base.
        store.save_snapshot("chat", &bytes).unwrap();
        assert!(!store.append_snapshot_delta("chat", base, &update, 13).unwrap());
        assert_eq!(store.snapshot_delta_stats("chat").unwrap(), (0, 0));
        store.delete_snapshot("chat").unwrap();
        assert!(store.load_snapshot("chat").unwrap().is_none());
    }

    /// beta → stable → beta: a build without `snapshot_deltas` rewrites the
    /// snapshots row (here after a room reset, with a LOWER cursor) and
    /// leaves the deltas behind. Back on this build, their ops still merge
    /// (same doc), but their older-room cursor must not be believed.
    #[test]
    fn a_downgrade_rewrite_strips_stale_deltas_of_their_cursor() {
        let dir = tempfile::tempdir().unwrap();
        let store = DocsStore::open(dir.path()).unwrap();
        let doc = loro::LoroDoc::new();
        doc.get_text("t").insert(0, "base").unwrap();
        doc.commit();
        let base_vv = doc.oplog_vv();
        let base = store
            .save_base_snapshot("chat", &doc.export(loro::ExportMode::Snapshot).unwrap(), 900, 2)
            .unwrap();
        doc.get_text("t").insert(4, " +delta").unwrap();
        doc.commit();
        let update = doc.export(loro::ExportMode::updates(&base_vv)).unwrap();
        assert!(store.append_snapshot_delta("chat", base, &update, 950).unwrap());

        // The older build: plain upsert of the row, nothing else.
        std::thread::sleep(std::time::Duration::from_millis(2));
        let stable = loro::LoroDoc::new();
        stable.get_text("other").insert(0, "stable's own edit").unwrap();
        stable.commit();
        store
            .conn()
            .execute(
                "UPDATE snapshots SET bytes = ?1, cursor = 12, saved_at = ?2 WHERE doc_id = 'chat'",
                params![stable.export(loro::ExportMode::Snapshot).unwrap(), now_ms()],
            )
            .unwrap();

        let (bytes, cursor, _) = store.load_snapshot_with_cursor("chat").unwrap().unwrap();
        assert_eq!(cursor, 12, "a stale delta claimed a cursor from another base");
        assert_eq!(store.snapshot_cursor("chat").unwrap(), 12);
        let loaded = loro::LoroDoc::new();
        loaded.import(&bytes).unwrap();
        assert_eq!(loaded.get_text("other").to_string(), "stable's own edit");
    }

    /// base (cursor 10) with three chained deltas (cursors 11, 12, 13), each
    /// exported from the version the one before it left.
    fn chained_deltas(store: &DocsStore) -> (u64, Vec<Vec<u8>>) {
        let doc = loro::LoroDoc::new();
        let text = doc.get_text("t");
        text.insert(0, "base").unwrap();
        doc.commit();
        let base = store
            .save_base_snapshot("chat", &doc.export(loro::ExportMode::Snapshot).unwrap(), 10, 2)
            .unwrap();
        let mut deltas = Vec::new();
        for word in [" one", " two", " three"] {
            let vv = doc.oplog_vv();
            text.insert(text.len_unicode(), word).unwrap();
            doc.commit();
            deltas.push(doc.export(loro::ExportMode::updates(&vv)).unwrap());
        }
        (base, deltas)
    }

    fn loaded_text(store: &DocsStore) -> (String, u64) {
        let (bytes, cursor, _) = store.load_snapshot_with_cursor("chat").unwrap().unwrap();
        let doc = loro::LoroDoc::new();
        doc.import(&bytes).unwrap();
        (doc.get_text("t").to_string(), cursor)
    }

    /// A lost middle row (salvage after damage, a second writer) leaves a
    /// later delta whose ops build on ops nobody holds. Its cursor must not
    /// count: the load ends at the last delta that applied, and the room
    /// resends everything after it.
    #[test]
    fn a_lost_middle_delta_ends_the_load_at_the_last_whole_one() {
        let dir = tempfile::tempdir().unwrap();
        let store = DocsStore::open(dir.path()).unwrap();
        let (base, deltas) = chained_deltas(&store);
        for (delta, cursor) in deltas.iter().zip([11, 12, 13]) {
            assert!(store.append_snapshot_delta("chat", base, delta, cursor).unwrap());
        }
        assert_eq!(loaded_text(&store), ("base one two three".into(), 13));
        store
            .conn()
            .execute("DELETE FROM snapshot_deltas WHERE cursor = 12", [])
            .unwrap();
        assert_eq!(loaded_text(&store), ("base one".into(), 11));
        assert_eq!(store.snapshot_cursor("chat").unwrap(), 11);
        // The intact row after the gap waits in the parked inbox.
        assert_eq!(store.pending_chat_inbox("chat#parked").unwrap().0, vec![deltas[2].clone()]);
        assert_eq!(store.snapshot_delta_stats("chat").unwrap().0, 1);
        let plain = loro::LoroDoc::new();
        plain.import(&store.load_snapshot("chat").unwrap().unwrap()).unwrap();
        assert_eq!(plain.get_text("t").to_string(), "base one");
    }

    /// Pre-release builds created snapshot_deltas without base_saved_at, or
    /// without seq, or with both under one migration. Each store opens, its
    /// old rows can't claim a cursor (they go to the parked inbox), and new
    /// deltas work.
    #[test]
    fn stores_from_earlier_delta_schemas_open_and_work() {
        let shapes = [
            "ordinal INTEGER PRIMARY KEY AUTOINCREMENT, doc_id TEXT NOT NULL, bytes BLOB NOT NULL, cursor INTEGER NOT NULL",
            "ordinal INTEGER PRIMARY KEY AUTOINCREMENT, doc_id TEXT NOT NULL, bytes BLOB NOT NULL, cursor INTEGER NOT NULL, base_saved_at INTEGER NOT NULL",
            "ordinal INTEGER PRIMARY KEY AUTOINCREMENT, doc_id TEXT NOT NULL, bytes BLOB NOT NULL, cursor INTEGER NOT NULL, base_saved_at INTEGER NOT NULL, seq INTEGER NOT NULL",
        ];
        for (shape, columns) in shapes.into_iter().enumerate() {
            let dir = tempfile::tempdir().unwrap();
            let (base_bytes, old_delta, new_delta) = {
                let doc = loro::LoroDoc::new();
                doc.get_text("t").insert(0, "base").unwrap();
                doc.commit();
                let base = doc.export(loro::ExportMode::Snapshot).unwrap();
                let vv = doc.oplog_vv();
                doc.get_text("t").insert(4, " old").unwrap();
                doc.commit();
                let old = doc.export(loro::ExportMode::updates(&vv)).unwrap();
                let vv = doc.oplog_vv();
                doc.get_text("t").insert(8, " new").unwrap();
                doc.commit();
                (base, old, doc.export(loro::ExportMode::updates(&vv)).unwrap())
            };
            {
                // The earlier build: its table, its rows, its migration count.
                let store = DocsStore::open(dir.path()).unwrap();
                store.save_verified_snapshot_with_cursor("chat", &base_bytes, 10, 2).unwrap();
                let conn = store.conn();
                conn.execute_batch(&format!(
                    "DROP TABLE snapshot_deltas;
                     CREATE TABLE snapshot_deltas ({columns}) STRICT;
                     DELETE FROM schema_migrations WHERE version > (SELECT MAX(version) - 3 FROM schema_migrations);"
                ))
                .unwrap();
                let fields = match shape {
                    0 => "doc_id, bytes, cursor",
                    1 => "doc_id, bytes, cursor, base_saved_at",
                    _ => "doc_id, bytes, cursor, base_saved_at, seq",
                };
                let values = match shape {
                    0 => "'chat', ?1, 11",
                    1 => "'chat', ?1, 11, 0",
                    _ => "'chat', ?1, 11, 0, 1",
                };
                conn.execute(
                    &format!("INSERT INTO snapshot_deltas ({fields}) VALUES ({values})"),
                    params![old_delta],
                )
                .unwrap();
            }
            let store = DocsStore::open(dir.path()).unwrap();
            let (bytes, cursor, _) = store.load_snapshot_with_cursor("chat").unwrap().unwrap();
            assert_eq!(cursor, 10, "shape {shape}: an old row claimed a cursor");
            let loaded = loro::LoroDoc::new();
            loaded.import(&bytes).unwrap();
            let text = loaded.get_text("t").to_string();
            // Shape 2's old row is a clean chain start on a stale base: its
            // ops merge, its cursor doesn't count. The others park it.
            assert!(text == "base" || text == "base old", "shape {shape}: {text}");
            // New deltas on a fresh base work.
            let reloaded = loro::LoroDoc::new();
            reloaded.import(&bytes).unwrap();
            for parked in store.pending_chat_inbox("chat#parked").unwrap().0 {
                reloaded.import(&parked).unwrap();
            }
            let vv = reloaded.oplog_vv();
            let base = store
                .save_base_snapshot("chat", &reloaded.export(loro::ExportMode::Snapshot).unwrap(), 11, 2)
                .unwrap();
            let doc = loro::LoroDoc::new();
            doc.import(&reloaded.export(loro::ExportMode::Snapshot).unwrap()).unwrap();
            doc.import(&new_delta).unwrap();
            let update = doc.export(loro::ExportMode::updates(&vv)).unwrap();
            assert!(store.append_snapshot_delta("chat", base, &update, 12).unwrap());
            let (bytes, cursor, _) = store.load_snapshot_with_cursor("chat").unwrap().unwrap();
            let loaded = loro::LoroDoc::new();
            loaded.import(&bytes).unwrap();
            assert_eq!((loaded.get_text("t").to_string(), cursor), ("base old new".into(), 12), "shape {shape}");
        }
    }

    /// A lost row is not always a causal gap. Row 12 (peer P) is lost; row
    /// 13 (peer Q, concurrent, not built on P) imports cleanly. The chain
    /// number shows the loss: the cursor stays at 11, so the room resends
    /// P's ops, and Q's row waits in the parked inbox.
    #[test]
    fn a_lost_row_before_a_concurrent_row_keeps_the_cursor_before_it() {
        let dir = tempfile::tempdir().unwrap();
        let store = DocsStore::open(dir.path()).unwrap();
        let doc = loro::LoroDoc::new();
        doc.set_peer_id(1).unwrap();
        doc.get_text("t").insert(0, "base").unwrap();
        doc.commit();
        let base = store
            .save_base_snapshot("chat", &doc.export(loro::ExportMode::Snapshot).unwrap(), 10, 2)
            .unwrap();
        let vv = doc.oplog_vv();
        doc.get_text("t").insert(4, " one").unwrap();
        doc.commit();
        let one = doc.export(loro::ExportMode::updates(&vv)).unwrap();
        let after_one = doc.oplog_vv();
        let concurrent = |peer: u64, key: &str| {
            let fork = loro::LoroDoc::new();
            fork.import(&doc.export(loro::ExportMode::Snapshot).unwrap()).unwrap();
            fork.set_peer_id(peer).unwrap();
            fork.get_map("peers").insert(key, "op").unwrap();
            fork.commit();
            fork.export(loro::ExportMode::updates(&after_one)).unwrap()
        };
        let p = concurrent(2, "p");
        let q = concurrent(3, "q");
        // Q doesn't depend on P: on its own it imports with nothing pending.
        let check = loro::LoroDoc::new();
        check.import(&doc.export(loro::ExportMode::Snapshot).unwrap()).unwrap();
        assert!(check.import(&q).unwrap().pending.is_none());

        assert!(store.append_snapshot_delta("chat", base, &one, 11).unwrap());
        assert!(store.append_snapshot_delta("chat", base, &p, 12).unwrap());
        assert!(store.append_snapshot_delta("chat", base, &q, 13).unwrap());
        store.conn().execute("DELETE FROM snapshot_deltas WHERE cursor = 12", []).unwrap();

        let (bytes, cursor, _) = store.load_snapshot_with_cursor("chat").unwrap().unwrap();
        assert_eq!(cursor, 11, "the cursor passed a lost row");
        assert_eq!(store.snapshot_cursor("chat").unwrap(), 11);
        let loaded = loro::LoroDoc::new();
        loaded.import(&bytes).unwrap();
        assert_eq!(loaded.get_text("t").to_string(), "base one");
        assert_eq!(store.pending_chat_inbox("chat#parked").unwrap().0, vec![q]);
    }

    /// A stop on missing dependencies (with the chain intact: a second
    /// writer) moves the intact rows to the parked inbox, so the next whole
    /// save, which clears the delta chain, can't take them with it.
    #[test]
    fn rows_after_a_pending_stop_survive_the_next_whole_save() {
        let dir = tempfile::tempdir().unwrap();
        let store = DocsStore::open(dir.path()).unwrap();
        let (base, deltas) = chained_deltas(&store);
        assert!(store.append_snapshot_delta("chat", base, &deltas[0], 11).unwrap());
        // Skips " two": its ops build on ops no row holds.
        assert!(store.append_snapshot_delta("chat", base, &deltas[2], 13).unwrap());
        assert_eq!(loaded_text(&store), ("base one".into(), 11));
        assert_eq!(store.pending_chat_inbox("chat#parked").unwrap().0, vec![deltas[2].clone()]);
        let (bytes, _, _) = store.load_snapshot_with_cursor("chat").unwrap().unwrap();
        store.save_base_snapshot("chat", &bytes, 11, 2).unwrap();
        assert_eq!(store.snapshot_delta_stats("chat").unwrap(), (0, 0));
        assert_eq!(
            store.pending_chat_inbox("chat#parked").unwrap().0,
            vec![deltas[2].clone()],
            "the whole save took the parked rows"
        );
    }

    /// One undecodable delta between good ones: the load keeps the good one
    /// before it (not just the base) and stops there.
    #[test]
    fn good_then_garbage_then_good_stops_at_the_garbage() {
        let dir = tempfile::tempdir().unwrap();
        let store = DocsStore::open(dir.path()).unwrap();
        let (base, deltas) = chained_deltas(&store);
        assert!(store.append_snapshot_delta("chat", base, &deltas[0], 11).unwrap());
        assert!(store.append_snapshot_delta("chat", base, b"not a loro blob", 12).unwrap());
        assert!(store.append_snapshot_delta("chat", base, &deltas[2], 13).unwrap());
        assert_eq!(loaded_text(&store), ("base one".into(), 11));
        assert_eq!(store.snapshot_cursor("chat").unwrap(), 11);
    }

    /// Deltas that don't apply leave the base and ITS cursor, not the
    /// deltas' newer cursor with older bytes.
    #[test]
    fn unappliable_deltas_fall_back_to_the_base_pair() {
        let dir = tempfile::tempdir().unwrap();
        let store = DocsStore::open(dir.path()).unwrap();
        let doc = loro::LoroDoc::new();
        doc.get_text("t").insert(0, "base").unwrap();
        doc.commit();
        let base_bytes = doc.export(loro::ExportMode::Snapshot).unwrap();
        let base = store.save_base_snapshot("chat", &base_bytes, 10, 2).unwrap();
        assert!(store.append_snapshot_delta("chat", base, b"not a loro blob", 99).unwrap());
        let (bytes, cursor, _) = store.load_snapshot_with_cursor("chat").unwrap().unwrap();
        assert_eq!((bytes, cursor), (base_bytes, 10));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn contended_connection_does_not_starve_a_two_worker_runtime() {
        use std::sync::Arc;
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(DocsStore::open(dir.path()).unwrap());
        let holder = store.clone();
        let (locked_tx, locked_rx) = std::sync::mpsc::channel();
        let holding = std::thread::spawn(move || {
            let _connection = holder.conn();
            locked_tx.send(()).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(600));
        });
        locked_rx.recv().unwrap();
        let started = std::time::Instant::now();
        let write = store.clone();
        let a = tokio::spawn(async move {
            write.save_snapshot("whale", &[0; 1024]).unwrap();
        });
        let read = store.clone();
        let b = tokio::spawn(async move {
            read.load_snapshot("registry1").unwrap();
        });
        // Both contenders have a chance to enter the shared connection wait.
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        assert!(
            started.elapsed() < std::time::Duration::from_millis(300),
            "SQLite contenders monopolized both runtime workers"
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let socket = tokio::net::TcpStream::connect(listener.local_addr().unwrap())
            .await
            .unwrap();
        let _accepted = listener.accept().await.unwrap();
        assert!(
            started.elapsed() < std::time::Duration::from_millis(300),
            "network progress waited for SQLite"
        );
        drop(socket);
        a.await.unwrap();
        b.await.unwrap();
        holding.join().unwrap();
    }

    #[test]
    fn verified_cursor_marker_tracks_atomic_snapshot_writes() {
        let dir = tempfile::tempdir().unwrap();
        let store = DocsStore::open(dir.path()).unwrap();
        store
            .save_snapshot_with_cursor("chat", b"old", 69000, 2)
            .unwrap();
        assert!(!store.snapshot_cursor_verified("chat").unwrap());
        store
            .save_verified_snapshot_with_cursor("chat", b"safe", 69001, 2)
            .unwrap();
        assert!(store.snapshot_cursor_verified("chat").unwrap());
        drop(store);
        let store = DocsStore::open(dir.path()).unwrap();
        assert!(store.snapshot_cursor_verified("chat").unwrap());
        assert_eq!(
            store.load_snapshot_with_cursor("chat").unwrap(),
            Some((b"safe".to_vec(), 69001, 2))
        );
        store
            .save_snapshot_with_cursor("chat", b"replacement", 0, 2)
            .unwrap();
        assert!(
            !store.snapshot_cursor_verified("chat").unwrap(),
            "unverified replacement invalidates trust"
        );
    }

    #[test]
    fn snapshot_roundtrip_and_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let store = DocsStore::open(dir.path()).unwrap();

        assert_eq!(store.load_snapshot("chat-1").unwrap(), None);
        store.save_snapshot("chat-1", b"v1").unwrap();
        assert_eq!(
            store.load_snapshot("chat-1").unwrap().as_deref(),
            Some(&b"v1"[..])
        );
        store.save_snapshot("chat-1", b"v2-longer-bytes").unwrap();
        assert_eq!(
            store.load_snapshot("chat-1").unwrap().as_deref(),
            Some(&b"v2-longer-bytes"[..])
        );
        // Distinct docs do not collide.
        store.save_snapshot("chat-2", b"other").unwrap();
        assert_eq!(
            store.load_snapshot("chat-1").unwrap().as_deref(),
            Some(&b"v2-longer-bytes"[..])
        );
    }

    #[test]
    fn cursor_rides_the_snapshot_row() {
        let dir = tempfile::tempdir().unwrap();
        let store = DocsStore::open(dir.path()).unwrap();

        // Plain saves (pre-chat2 path) read back with cursor/epoch 0.
        store.save_snapshot("chat-1", b"v1").unwrap();
        assert_eq!(
            store.load_snapshot_with_cursor("chat-1").unwrap(),
            Some((b"v1".to_vec(), 0, 0))
        );
        // Cursor and bytes land together; a re-save moves both.
        store
            .save_snapshot_with_cursor("chat-1", b"v2", 41, 2)
            .unwrap();
        assert_eq!(
            store.load_snapshot_with_cursor("chat-1").unwrap(),
            Some((b"v2".to_vec(), 41, 2))
        );
        // Plain load still works for cursor-written rows.
        assert_eq!(
            store.load_snapshot("chat-1").unwrap().as_deref(),
            Some(&b"v2"[..])
        );
        // A plain save (legacy caller) clears nothing — cursor persists…
        store.save_snapshot("chat-1", b"v3").unwrap();
        let (bytes, cursor, epoch) = store.load_snapshot_with_cursor("chat-1").unwrap().unwrap();
        assert_eq!((bytes.as_slice(), cursor, epoch), (&b"v3"[..], 41, 2));
    }

    #[test]
    fn processed_ledger_claims_exactly_once() {
        let dir = tempfile::tempdir().unwrap();
        let store = DocsStore::open(dir.path()).unwrap();

        assert!(!store.is_processed("cmd-1").unwrap());
        assert!(store.mark_processed("cmd-1").unwrap(), "first mark claims");
        assert!(store.is_processed("cmd-1").unwrap());
        assert!(
            !store.mark_processed("cmd-1").unwrap(),
            "second mark must not re-claim"
        );
    }

    #[test]
    fn reopen_preserves_data_and_migrations_are_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        {
            let store = DocsStore::open(dir.path()).unwrap();
            store.save_snapshot("chat-1", b"persisted").unwrap();
            store.mark_processed("cmd-1").unwrap();
        }
        let store = DocsStore::open(dir.path()).unwrap(); // re-runs migrate()
        assert_eq!(
            store.load_snapshot("chat-1").unwrap().as_deref(),
            Some(&b"persisted"[..])
        );
        assert!(store.is_processed("cmd-1").unwrap());
        assert!(!store.mark_processed("cmd-1").unwrap());
    }
}

#[cfg(test)]
mod publication_tests {
    use super::*;
    #[test]
    fn outbox_survives_restart_initializes_once_and_preserves_cursor() {
        let dir = tempfile::tempdir().unwrap();
        let original;
        {
            let store = DocsStore::open(dir.path()).unwrap();
            store
                .save_snapshot_with_cursor("chat", b"snapshot", 40229, 2)
                .unwrap();
            store
                .initialize_chat_outbox("chat", &[b"legacy-tail".to_vec()])
                .unwrap();
            original = store.pending_chat_updates("chat").unwrap();
            store
                .enqueue_chat_update("chat", "stable-id", b"new-turn")
                .unwrap();
            store
                .enqueue_chat_update("chat", "stable-id", b"new-turn")
                .unwrap();
        }
        let store = DocsStore::open(dir.path()).unwrap();
        store
            .initialize_chat_outbox("chat", &[b"must-not-reseed".to_vec()])
            .unwrap();
        let pending = store.pending_chat_updates("chat").unwrap();
        assert_eq!(pending.len(), 2);
        assert_eq!(pending[0], original[0]);
        assert_eq!(pending[1], ("stable-id".into(), b"new-turn".to_vec()));
        assert_eq!(
            store.load_snapshot_with_cursor("chat").unwrap().unwrap(),
            (b"snapshot".to_vec(), 40229, 2)
        );
        store
            .acknowledge_chat_update("other-chat", "stable-id")
            .unwrap();
        assert_eq!(store.pending_chat_updates("chat").unwrap().len(), 2);
        store.acknowledge_chat_update("chat", "stable-id").unwrap();
        assert_eq!(store.pending_chat_updates("chat").unwrap(), original);
        store.delete_snapshot("chat").unwrap();
        assert!(store.pending_chat_updates("chat").unwrap().is_empty());
        assert!(!store.chat_outbox_initialized("chat").unwrap());
    }
}

#[cfg(test)]
mod publication_failure_tests {
    use super::*;

    /// Outbox writes (every commit's, inside the commit callback) while a
    /// 50 MB whole snapshot is saved on another thread: their latency.
    /// Returns (max, p99) in ms over the save's duration.
    fn outbox_latency_during_a_whole_save(store: std::sync::Arc<DocsStore>) -> (f64, f64, f64) {
        outbox_latency_with_saver(store.clone(), store)
    }

    fn outbox_latency_with_saver(store: std::sync::Arc<DocsStore>, saver_store: std::sync::Arc<DocsStore>) -> (f64, f64, f64) {
        let mut seed = 0x2545_F491_4F6C_DD1Du64;
        let blob: Vec<u8> = (0..50 * 1024 * 1024 / 8)
            .flat_map(|_| {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                seed.to_le_bytes()
            })
            .collect();
        // Warm: the row exists, as for a chat saved before.
        store.save_snapshot_with_cursor("whale", &blob, 1, 2).unwrap();
        store.initialize_chat_outbox("busy", &[]).unwrap();
        let saving = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let saver = {
            let (store, saving) = (saver_store, saving.clone());
            std::thread::spawn(move || {
                let started = Instant::now();
                for cursor in 2..5 {
                    store.save_snapshot_with_cursor("whale", &blob, cursor, 2).unwrap();
                }
                saving.store(false, std::sync::atomic::Ordering::Release);
                started.elapsed().as_secs_f64() * 1000.0 / 3.0
            })
        };
        let mut latencies = Vec::new();
        let mut batch = 0;
        while saving.load(std::sync::atomic::Ordering::Acquire) {
            batch += 1;
            let started = Instant::now();
            store
                .enqueue_chat_update("busy", &format!("batch-{batch}"), b"a streamed token")
                .unwrap();
            latencies.push(started.elapsed().as_secs_f64() * 1000.0);
            std::thread::sleep(Duration::from_millis(5));
        }
        let save_ms = saver.join().unwrap();
        latencies.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let max = *latencies.last().unwrap();
        let p99 = latencies[(latencies.len() * 99 / 100).min(latencies.len() - 1)];
        (max, p99, save_ms)
    }

    #[test]
    fn outbox_writes_stay_fast_while_a_whole_snapshot_saves() {
        let dir = tempfile::tempdir().unwrap();
        let store = std::sync::Arc::new(DocsStore::open(dir.path()).unwrap());
        let (max, p99, save_ms) = outbox_latency_during_a_whole_save(store);
        eprintln!("outbox write during a 50 MB whole save: max {max:.1} ms, p99 {p99:.1} ms (save {save_ms:.0} ms)");
        let dir = tempfile::tempdir().unwrap();
        let store = std::sync::Arc::new(DocsStore::open(dir.path()).unwrap());
        let other = std::sync::Arc::new(DocsStore::open(dir.path()).unwrap());
        let (max, p99, save_ms) = outbox_latency_with_saver(store, other);
        eprintln!("same, saver on its own connection: max {max:.1} ms, p99 {p99:.1} ms (save {save_ms:.0} ms)");
    }

    /// A relay chat whose outbox write failed must replay its operations
    /// unless a save made them durable. A delta is such a save: it clears the
    /// replay obligation in its own transaction, and only if it commits.
    #[test]
    fn a_delta_save_retires_a_failed_publication_only_when_it_commits() {
        let dir = tempfile::tempdir().unwrap();
        let store = DocsStore::open(dir.path()).unwrap();
        let doc = loro::LoroDoc::new();
        doc.get_text("t").insert(0, "base").unwrap();
        doc.commit();
        let vv = doc.oplog_vv();
        let base = store
            .save_base_snapshot("chat", &doc.export(loro::ExportMode::Snapshot).unwrap(), 42, 2)
            .unwrap();
        store.initialize_chat_outbox("chat", &[]).unwrap();
        store.conn().execute_batch("CREATE TRIGGER fail_publication BEFORE INSERT ON chat_outbox BEGIN SELECT RAISE(FAIL, 'injected disk failure'); END;").unwrap();
        assert!(store.enqueue_chat_update("chat", "batch", b"op").is_err());
        doc.get_text("t").insert(4, " + op").unwrap();
        doc.commit();
        let update = doc.export(loro::ExportMode::updates(&vv)).unwrap();

        // The delta's own write fails: the obligation stays.
        store.conn().execute_batch("CREATE TRIGGER fail_delta BEFORE INSERT ON snapshot_deltas BEGIN SELECT RAISE(FAIL, 'injected disk failure'); END;").unwrap();
        assert!(store.append_snapshot_delta("chat", base, &update, 43).is_err());
        assert!(store.chat_outbox_initialized("chat").unwrap());
        assert_eq!(store.snapshot_cursor("chat").unwrap(), 42);

        // It commits: obligation and delta go together.
        store.conn().execute_batch("DROP TRIGGER fail_delta;").unwrap();
        assert!(store.append_snapshot_delta("chat", base, &update, 43).unwrap());
        drop(store);
        let reopened = DocsStore::open(dir.path()).unwrap();
        assert!(!reopened.chat_outbox_initialized("chat").unwrap());
        let (bytes, cursor, _) = reopened.load_snapshot_with_cursor("chat").unwrap().unwrap();
        let loaded = loro::LoroDoc::new();
        loaded.import(&bytes).unwrap();
        assert_eq!((loaded.get_text("t").to_string(), cursor), ("base + op".into(), 43));
    }
    #[test]
    fn cursor_save_after_failed_outbox_write_keeps_a_durable_replay_obligation() {
        for cursor_save in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let store = DocsStore::open(dir.path()).unwrap();
            store
                .save_snapshot_with_cursor("chat", b"before", 42, 2)
                .unwrap();
            store.initialize_chat_outbox("chat", &[]).unwrap();
            store.conn().execute_batch("CREATE TRIGGER fail_publication BEFORE INSERT ON chat_outbox BEGIN SELECT RAISE(FAIL, 'injected disk failure'); END;").unwrap();
            assert!(
                store
                    .enqueue_chat_update("chat", "batch", b"cleanup")
                    .is_err()
            );
            if cursor_save {
                store
                    .save_snapshot_with_cursor("chat", b"after", 43, 2)
                    .unwrap();
            } else {
                store.save_snapshot("chat", b"after").unwrap();
            }
            drop(store);
            let reopened = DocsStore::open(dir.path()).unwrap();
            assert!(!reopened.chat_outbox_initialized("chat").unwrap());
            assert_eq!(
                reopened.load_snapshot_with_cursor("chat").unwrap().unwrap(),
                (b"after".to_vec(), if cursor_save { 43 } else { 42 }, 2)
            );
        }
    }
}

#[cfg(test)]
mod maintenance_tests {
    use super::*;

    const MIB: usize = 1024 * 1024;

    fn pragma(conn: &Connection, name: &str) -> i64 {
        conn.pragma_query_value(None, name, |row| row.get(0))
            .unwrap()
    }

    fn file_len(path: &Path) -> u64 {
        std::fs::metadata(path).map_or(0, |m| m.len())
    }

    /// 48 MiB of snapshots, 40 MiB of them deleted: well past both thresholds.
    fn fragmented_store(dir: &Path) {
        let store = DocsStore::open(dir).unwrap();
        for i in 0..48 {
            store
                .save_snapshot(&format!("d{i}"), &vec![7; MIB])
                .unwrap();
        }
        for i in 0..40 {
            store.delete_snapshot(&format!("d{i}")).unwrap();
        }
        let conn = store.conn();
        assert!(worth_vacuuming(
            pragma(&conn, "page_size"),
            pragma(&conn, "page_count"),
            pragma(&conn, "freelist_count")
        ));
    }

    #[test]
    fn open_folds_and_truncates_a_bloated_wal() {
        let dir = tempfile::tempdir().unwrap();
        drop(DocsStore::open(dir.path()).unwrap());
        let wal = dir.path().join("docs.sqlite3-wal");
        // A live connection keeps the WAL from being cleaned up on close.
        let other = Connection::open(dir.path().join("docs.sqlite3")).unwrap();
        other.pragma_update(None, "wal_autocheckpoint", 0).unwrap();
        for i in 0..8 {
            other
                .execute(
                    "INSERT INTO snapshots(doc_id,bytes,saved_at) VALUES (?1,zeroblob(?2),0)",
                    params![format!("d{i}"), MIB as i64],
                )
                .unwrap();
        }
        assert!(file_len(&wal) > 8 * MIB as u64);

        let store = DocsStore::open(dir.path()).unwrap();
        assert_eq!(file_len(&wal), 0, "boot checkpoint truncates the WAL");
        assert!(store.has_snapshot("d7").unwrap(), "frames folded, not lost");
        assert_eq!(
            pragma(&store.conn(), "journal_size_limit"),
            JOURNAL_SIZE_LIMIT_BYTES
        );
    }

    #[test]
    fn reclaim_vacuums_a_fragmented_database_once() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("docs.sqlite3");
        fragmented_store(dir.path());
        let before = file_len(&db);

        let store = DocsStore::open(dir.path()).unwrap();
        // Open never pays for the rewrite; the reclaim pass does.
        assert!(pragma(&store.conn(), "freelist_count") > 0);
        store.reclaim_free_space();
        assert_eq!(pragma(&store.conn(), "freelist_count"), 0);
        assert!(file_len(&db) < before / 2, "file shrank");
        assert_eq!(file_len(&dir.path().join("docs.sqlite3-wal")), 0);
        assert_eq!(store.load_snapshot("d47").unwrap().unwrap().len(), MIB);
        assert!(
            !reclaim_free_pages(&store.conn()).unwrap(),
            "a compacted database is not rewritten again"
        );
    }

    #[test]
    fn reclaim_after_a_post_open_shrink_vacuums_without_a_relaunch() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("docs.sqlite3");
        // Open-time maintenance finds nothing to do; the shrink happens after.
        let store = DocsStore::open(dir.path()).unwrap();
        store
            .save_snapshot("registry1", &vec![7; 48 * MIB])
            .unwrap();
        store.save_snapshot("registry1", &vec![7; MIB]).unwrap();
        assert!(pragma(&store.conn(), "freelist_count") > 0);
        let before = file_len(&db);

        store.reclaim_free_space();
        assert_eq!(pragma(&store.conn(), "freelist_count"), 0);
        assert!(file_len(&db) < before / 2, "file shrank");
        assert_eq!(file_len(&dir.path().join("docs.sqlite3-wal")), 0);
        assert_eq!(
            store.load_snapshot("registry1").unwrap().unwrap().len(),
            MIB
        );
        // The busy timeout is back at the store's normal wait.
        let busy: i64 = pragma(&store.conn(), "busy_timeout");
        assert_eq!(busy, STORE_BUSY_TIMEOUT.as_millis() as i64);
        // Idempotent and cheap once compact.
        store.reclaim_free_space();
        assert_eq!(pragma(&store.conn(), "freelist_count"), 0);
    }

    #[test]
    fn small_or_proportionally_minor_freelists_are_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let store = DocsStore::open(dir.path()).unwrap();
        for i in 0..4 {
            store
                .save_snapshot(&format!("d{i}"), &vec![7; MIB])
                .unwrap();
        }
        for i in 0..3 {
            store.delete_snapshot(&format!("d{i}")).unwrap();
        }
        let freelist = pragma(&store.conn(), "freelist_count");
        assert!(freelist > 0);
        assert!(!reclaim_free_pages(&store.conn()).unwrap(), "below 32 MiB");
        assert_eq!(pragma(&store.conn(), "freelist_count"), freelist);

        let page = 4096;
        let floor = VACUUM_MIN_FREE_BYTES / page;
        assert!(worth_vacuuming(page, floor * 4, floor));
        assert!(!worth_vacuuming(page, floor * 4 + 1, floor), "under 25%");
        assert!(!worth_vacuuming(page, floor, floor - 1), "under the floor");
    }

    #[test]
    fn maintenance_yields_to_a_connection_holding_the_database() {
        // WAL lets VACUUM proceed beside a reader; a held write lock defers it.
        for (lock, vacuumed) in [
            ("BEGIN; SELECT count(*) FROM snapshots;", true),
            ("BEGIN IMMEDIATE;", false),
        ] {
            let dir = tempfile::tempdir().unwrap();
            fragmented_store(dir.path());
            let other = Connection::open(dir.path().join("docs.sqlite3")).unwrap();
            other.execute_batch(lock).unwrap();

            let started = Instant::now();
            let store = DocsStore::open(dir.path()).expect("maintenance never fails open");
            store.reclaim_free_space();
            assert!(
                started.elapsed() < Duration::from_secs(3),
                "{lock}: maintenance waited on the store busy timeout"
            );
            assert_eq!(store.load_snapshot("d47").unwrap().unwrap().len(), MIB);
            assert_eq!(
                pragma(&store.conn(), "freelist_count") == 0,
                vacuumed,
                "{lock}"
            );

            other.execute_batch("COMMIT").unwrap();
            store.save_snapshot("after", b"ok").unwrap();
            assert!(store.has_snapshot("after").unwrap());
            drop(store);
            // A deferred VACUUM simply runs on the next uncontended pass.
            let store = DocsStore::open(dir.path()).unwrap();
            store.reclaim_free_space();
            assert_eq!(pragma(&store.conn(), "freelist_count"), 0, "{lock}");
        }
    }
}
