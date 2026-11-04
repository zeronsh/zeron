//! `DocsStore` — local SQLite persistence for doc snapshots and the
//! processed-command ledger (ARCHITECTURE §2 command plane: entries are marked
//! processed BEFORE execution so a crash can never double-execute a command).

use std::collections::HashSet;
use std::path::Path;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, params};

/// Errors surfaced by [`DocsStore`].
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("command conflict: {0}")]
    CommandConflict(String),
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
    "CREATE TABLE crew_commands (
        command_id TEXT PRIMARY KEY,
        request BLOB NOT NULL,
        outcome BLOB
    ) STRICT;",
    "CREATE TABLE legacy_crew_records (
        doc_id TEXT NOT NULL,
        container TEXT NOT NULL,
        record_id TEXT NOT NULL,
        value BLOB NOT NULL,
        PRIMARY KEY(doc_id,container,record_id)
    ) STRICT;",
    // Observed directory evidence survives deleting a transcript. Work itself
    // uses chat_sync_jobs, including its conditional-completion generation.
    "CREATE TABLE crew_directory_state (
        doc_id TEXT PRIMARY KEY,
        state TEXT NOT NULL
    ) STRICT;",
    // Relay execution may settle before the original command row syncs.
    // NULL preserves the crash-recovery meaning of older processed claims.
    "ALTER TABLE processed_commands ADD COLUMN outcome BLOB;",
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
    failed_publications: Mutex<HashSet<String>>,
    /// Snapshot jobs wait here asynchronously before entering the blocking pool.
    pub snapshot_writer: std::sync::Arc<tokio::sync::Mutex<()>>,
    directory_changed: tokio::sync::watch::Sender<u64>,
}

impl DocsStore {
    /// Open (creating directory, database, and schema as needed).
    pub fn open(data_dir: impl AsRef<Path>) -> Result<Self, StoreError> {
        Self::open_inner(data_dir.as_ref(), false)
    }

    /// Only the copy-only importer may initialize its marked, unpublished store.
    pub(crate) fn open_import_destination(data_dir: &Path) -> Result<Self, StoreError> {
        Self::open_inner(data_dir, true)
    }

    fn open_inner(data_dir: &Path, importing: bool) -> Result<Self, StoreError> {
        if !importing && data_dir.join("crew-import-incomplete").exists() {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData,
                "incomplete Crew import: retain it for diagnosis and import into a fresh profile").into());
        }
        // Inspect before opening read/write or applying any schema migration.
        // Crew 0.1 and upstream both number migrations from one; numbers alone
        // are not a schema identity and must never authorize an in-place upgrade.
        let path = data_dir.join("docs.sqlite3");
        if path.exists() {
            let existing = Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
            let legacy: bool = existing.query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name IN ('document_scopes','semantic_intents','trusted_local_commands','session_startups','crew_directory_jobs'))",
                [], |row| row.get(0))?;
            let snapshots: bool = existing.query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='snapshots')",
                [], |row| row.get(0))?;
            let workspace = snapshots && existing.query_row(
                "SELECT EXISTS(SELECT 1 FROM snapshots WHERE doc_id='workspace3')", [], |row| row.get::<_, bool>(0))?;
            if legacy || workspace {
                return Err(std::io::Error::new(std::io::ErrorKind::InvalidData,
                    "legacy Crew database: use import-backup with a consistent copied backup and a new separate destination; in-place opening is forbidden").into());
            }
        }
        std::fs::create_dir_all(data_dir)?;
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
        Ok(Self {
            conn: Mutex::new(conn),
            failed_publications: Mutex::new(HashSet::new()),
            snapshot_writer: std::sync::Arc::new(tokio::sync::Mutex::new(())),
            directory_changed: tokio::sync::watch::channel(0).0,
        })
    }

    /// The actual opened SQLite parent, for fixed-profile read-only migration
    /// proof. RPC callers never supply or select this filesystem path.
    pub fn profile_root(&self) -> Result<std::path::PathBuf, StoreError> {
        let conn = self.conn();
        let parent = conn.path().and_then(|path| Path::new(path).parent()).ok_or_else(||
            std::io::Error::new(std::io::ErrorKind::InvalidData, "profile database has no filesystem parent"))?;
        Ok(parent.canonicalize()?)
    }

    pub fn watch_directory(&self) -> tokio::sync::watch::Receiver<u64> {
        self.directory_changed.subscribe()
    }

    pub fn queue_directory(&self, id: &str) -> Result<(), StoreError> {
        if !zeron_proto::is_cloud_session_id(id) { return Ok(()); }
        self.schedule_sync_job(id, "crew-directory")?;
        self.directory_changed.send_modify(|v| *v = v.wrapping_add(1));
        Ok(())
    }

    pub fn directory_state(&self, id: &str) -> Result<Option<String>, StoreError> {
        store_blocking(|| Ok(self.conn().query_row(
            "SELECT state FROM crew_directory_state WHERE doc_id=?1", params![id], |r| r.get(0),
        ).optional()?))
    }

    pub fn save_directory_state(&self, id: &str, state: &str) -> Result<(), StoreError> {
        store_blocking(|| {
            self.conn().execute(
                "INSERT INTO crew_directory_state(doc_id,state) VALUES (?1,?2)
                 ON CONFLICT(doc_id) DO UPDATE SET state=excluded.state", params![id, state],
            )?;
            Ok(())
        })
    }

    /// Reject oversized compressed snapshots before SQLite allocates their blob.
    pub fn load_directory_snapshot(&self, id: &str) -> Result<Option<Vec<u8>>, StoreError> {
        store_blocking(|| {
            let conn = self.conn();
            let size: Option<i64> = conn.query_row(
                "SELECT length(bytes) FROM snapshots WHERE doc_id=?1", params![id], |r| r.get(0),
            ).optional()?;
            if size.is_some_and(|size| size > 32 * 1024 * 1024) {
                return Err(std::io::Error::new(std::io::ErrorKind::InvalidData,
                    "directory snapshot exceeds 32 MiB; reconciliation required").into());
            }
            Ok(conn.query_row("SELECT bytes FROM snapshots WHERE doc_id=?1",
                params![id], |r| r.get(0)).optional()?)
        })
    }

    fn queue_directory_in(&self, tx: &rusqlite::Transaction<'_>, id: &str) -> Result<(), StoreError> {
        if zeron_proto::is_cloud_session_id(id) {
            tx.execute("UPDATE sync_job_clock SET value=value+1 WHERE id=1", [])?;
            tx.execute(
                "INSERT INTO chat_sync_jobs(doc_id,kind,version)
                 SELECT ?1,'crew-directory',value FROM sync_job_clock WHERE id=1
                 ON CONFLICT(doc_id,kind) DO UPDATE SET version=excluded.version",
                params![id],
            )?;
        }
        Ok(())
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
        Ok(())
    }

    /// Latest saved snapshot for `doc_id`, if any.
    pub fn load_snapshot(&self, doc_id: &str) -> Result<Option<Vec<u8>>, StoreError> {
        store_blocking(|| self.load_snapshot_blocking(doc_id))
    }

    fn load_snapshot_blocking(&self, doc_id: &str) -> Result<Option<Vec<u8>>, StoreError> {
        let bytes = self
            .conn()
            .query_row(
                "SELECT bytes FROM snapshots WHERE doc_id = ?1",
                params![doc_id],
                |row| row.get(0),
            )
            .optional()?;
        Ok(bytes)
    }

    /// Save (upsert) the snapshot for `doc_id`.
    pub fn save_snapshot(&self, doc_id: &str, bytes: &[u8]) -> Result<(), StoreError> {
        store_blocking(|| self.save_snapshot_blocking(doc_id, bytes))
    }

    fn save_snapshot_blocking(&self, doc_id: &str, bytes: &[u8]) -> Result<(), StoreError> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO snapshots (doc_id, bytes, saved_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(doc_id) DO UPDATE SET bytes = excluded.bytes, saved_at = excluded.saved_at",
            params![doc_id, bytes, now_ms()],
        )?;
        self.invalidate_failed_publication(&tx, doc_id)?;
        self.queue_directory_in(&tx, doc_id)?;
        tx.commit()?;
        self.directory_changed.send_modify(|v| *v = v.wrapping_add(1));
        Ok(())
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
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO snapshots (doc_id, bytes, saved_at, cursor, epoch) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(doc_id) DO UPDATE SET bytes = excluded.bytes, saved_at = excluded.saved_at,
                 cursor = excluded.cursor, epoch = excluded.epoch, cursor_verified = 0",
            params![doc_id, bytes, now_ms(), cursor as i64, epoch as i64],
        )?;
        self.invalidate_failed_publication(&tx, doc_id)?;
        self.queue_directory_in(&tx, doc_id)?;
        tx.commit()?;
        self.directory_changed.send_modify(|v| *v = v.wrapping_add(1));
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
            self.save_verified_snapshot_with_cursor_blocking(doc_id, bytes, cursor, epoch)
        })
    }

    fn save_verified_snapshot_with_cursor_blocking(
        &self,
        doc_id: &str,
        bytes: &[u8],
        cursor: u64,
        epoch: u32,
    ) -> Result<(), StoreError> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO snapshots (doc_id, bytes, saved_at, cursor, epoch, cursor_verified) VALUES (?1, ?2, ?3, ?4, ?5, 1)
             ON CONFLICT(doc_id) DO UPDATE SET bytes = excluded.bytes, saved_at = excluded.saved_at,
                 cursor = excluded.cursor, epoch = excluded.epoch, cursor_verified = 1",
            params![doc_id, bytes, now_ms(), cursor as i64, epoch as i64],
        )?;
        self.invalidate_failed_publication(&tx, doc_id)?;
        self.queue_directory_in(&tx, doc_id)?;
        tx.commit()?;
        self.directory_changed.send_modify(|v| *v = v.wrapping_add(1));
        Ok(())
    }

    pub fn snapshot_cursor(&self, doc_id: &str) -> Result<u64, StoreError> {
        store_blocking(|| {
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
        let row = self
            .conn()
            .query_row(
                "SELECT bytes, cursor, epoch FROM snapshots WHERE doc_id = ?1",
                params![doc_id],
                |row| {
                    Ok((
                        row.get::<_, Vec<u8>>(0)?,
                        row.get::<_, Option<i64>>(1)?.unwrap_or(0) as u64,
                        row.get::<_, Option<i64>>(2)?.unwrap_or(0) as u32,
                    ))
                },
            )
            .optional()?;
        Ok(row)
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
        tx.execute("DELETE FROM chat_outbox WHERE doc_id = ?1", params![doc_id])?;
        tx.execute(
            "DELETE FROM chat_sync_jobs WHERE doc_id = ?1 AND kind != 'crew-directory'",
            params![doc_id],
        )?;
        tx.execute(
            "DELETE FROM chat_outbox_initialized WHERE doc_id = ?1",
            params![doc_id],
        )?;
        tx.commit()?;
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

    /// Native subagent document ids (`{chat_id}--sub--…`) stored for one chat.
    /// Profile import carries them with their parent; they have no registry row.
    pub fn subagent_snapshot_ids(&self, chat_id: &str) -> Result<Vec<String>, StoreError> {
        store_blocking(|| {
            let prefix = format!("{chat_id}--sub--");
            let conn = self.conn();
            let mut stmt = conn.prepare(
                "SELECT doc_id FROM snapshots WHERE substr(doc_id, 1, length(?1)) = ?1 ORDER BY doc_id",
            )?;
            let ids = stmt
                .query_map(params![prefix], |row| row.get(0))?
                .collect::<Result<Vec<String>, _>>()?;
            Ok(ids)
        })
    }

    /// The full command ledger — profile-import preserves both at-most-once
    /// claims and known outcomes for pending command rows that arrive later.
    pub fn processed_commands(&self) -> Result<Vec<(String, i64, Option<Vec<u8>>)>, StoreError> {
        store_blocking(|| self.processed_commands_blocking())
    }

    fn processed_commands_blocking(&self) -> Result<Vec<(String, i64, Option<Vec<u8>>)>, StoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT command_id, processed_at, outcome FROM processed_commands")?;
        let rows = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Merge foreign ledger claims and outcomes (profile import). Existing
    /// claims and known outcomes win; fill unknown outcomes from the source.
    /// Returns only the number of newly inserted claims.
    pub fn import_processed_commands(&self, rows: &[(String, i64, Option<Vec<u8>>)]) -> Result<usize, StoreError> {
        store_blocking(|| self.import_processed_commands_blocking(rows))
    }

    fn import_processed_commands_blocking(
        &self,
        rows: &[(String, i64, Option<Vec<u8>>)],
    ) -> Result<usize, StoreError> {
        let mut inserted = 0;
        let conn = self.conn();
        for (command_id, processed_at, outcome) in rows {
            let added = conn.execute(
                "INSERT OR IGNORE INTO processed_commands (command_id, processed_at, outcome) VALUES (?1, ?2, ?3)",
                params![command_id, processed_at, outcome],
            )?;
            inserted += added;
            if added == 0 && outcome.is_some() {
                conn.execute(
                    "UPDATE processed_commands SET outcome=?2 WHERE command_id=?1 AND outcome IS NULL",
                    params![command_id, outcome],
                )?;
            }
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

    /// Record a claimed command's terminal status/resolution before acknowledging
    /// execution. The outcome shares the processed claim's lifetime and is immutable.
    pub fn finish_processed_command(&self, id: &str, outcome: &[u8]) -> Result<(), StoreError> {
        store_blocking(|| {
            let changed = self.conn().execute(
                "UPDATE processed_commands SET outcome=?2 WHERE command_id=?1 AND (outcome IS NULL OR outcome=?2)",
                params![id, outcome],
            )?;
            if changed != 1 {
                return Err(StoreError::CommandConflict(format!("{id}: missing claim or conflicting outcome")));
            }
            Ok(())
        })
    }

    /// A known terminal outcome, or None for an unclaimed/interrupted legacy id.
    pub fn processed_command_outcome(&self, id: &str) -> Result<Option<Vec<u8>>, StoreError> {
        store_blocking(|| Ok(self.conn().query_row(
            "SELECT outcome FROM processed_commands WHERE command_id=?1", [id],
            |row| row.get::<_, Option<Vec<u8>>>(0),
        ).optional()?.flatten()))
    }

    /// Claim immutable Crew control intent before network delivery. An existing
    /// claim without an outcome is ambiguous and must be reconciled, never replayed.
    pub fn claim_crew_command(&self, id: &str, request: &[u8]) -> Result<bool, StoreError> {
        store_blocking(|| {
            let mut conn = self.conn();
            let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            let existing: Option<Vec<u8>> = tx.query_row(
                "SELECT request FROM crew_commands WHERE command_id=?1", [id],
                |row| row.get(0),
            ).optional()?;
            if let Some(existing) = existing {
                if existing != request {
                    return Err(StoreError::CommandConflict(id.into()));
                }
                return Ok(false);
            }
            let claimed = tx.execute(
                "INSERT OR IGNORE INTO processed_commands(command_id,processed_at) VALUES (?1,?2)",
                params![id, now_ms()],
            )?;
            if claimed == 0 {
                return Err(StoreError::CommandConflict(format!("{id}: legacy claim has no intent")));
            }
            tx.execute("INSERT INTO crew_commands(command_id,request) VALUES (?1,?2)", params![id, request])?;
            tx.commit()?;
            Ok(true)
        })
    }

    pub fn crew_command(&self, id: &str) -> Result<Option<(Vec<u8>, Option<Vec<u8>>)>, StoreError> {
        store_blocking(|| Ok(self.conn().query_row(
            "SELECT request,outcome FROM crew_commands WHERE command_id=?1", [id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        ).optional()?))
    }

    /// Terminal outcomes are immutable. Identical completion is idempotent.
    pub fn finish_crew_command(&self, id: &str, outcome: &[u8]) -> Result<(), StoreError> {
        store_blocking(|| {
            let conn = self.conn();
            let changed = conn.execute(
                "UPDATE crew_commands SET outcome=?2 WHERE command_id=?1 AND (outcome IS NULL OR outcome=?2)",
                params![id, outcome],
            )?;
            if changed != 1 {
                return Err(StoreError::CommandConflict(format!("{id}: missing claim or conflicting outcome")));
            }
            Ok(())
        })
    }

    pub fn crew_commands(&self) -> Result<Vec<(String, Vec<u8>, Option<Vec<u8>>)>, StoreError> {
        store_blocking(|| {
            let conn = self.conn();
            let mut stmt = conn.prepare("SELECT command_id,request,outcome FROM crew_commands ORDER BY command_id")?;
            Ok(stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
                .collect::<Result<_, _>>()?)
        })
    }

    /// Discover pending and completed handoff roots and children without loading
    /// their payloads. Pages are key-ordered and exclusive of `after`, with at
    /// most 64 keys of at most 1 KiB each; malformed keys fail the entire page.
    pub fn crew_handoff_command_ids(&self, after: &str, limit: usize) -> Result<Vec<String>, StoreError> {
        store_blocking(|| {
            let conn = self.conn();
            let mut stmt = conn.prepare("SELECT command_id FROM crew_commands WHERE command_id GLOB 'crew-handoff:*' AND command_id>?1 ORDER BY command_id LIMIT ?2")?;
            let mut rows = stmt.query(params![after, limit.clamp(1, 64) as i64])?;
            let mut ids = Vec::new();
            while let Some(row) = rows.next()? {
                let rusqlite::types::ValueRef::Text(id) = row.get_ref(0)? else {
                    return Err(StoreError::CommandConflict("invalid handoff command ID".into()));
                };
                if id.len() > 1024 {
                    return Err(StoreError::CommandConflict("handoff command ID exceeds 1024 bytes".into()));
                }
                let id = std::str::from_utf8(id)
                    .map_err(|_| StoreError::CommandConflict("invalid UTF-8 handoff command ID".into()))?;
                ids.push(id.to_owned());
            }
            Ok(ids)
        })
    }

    pub fn pending_crew_commands(&self, after: &str, limit: usize) -> Result<Vec<(String, Vec<u8>)>, StoreError> {
        store_blocking(|| {
            let conn = self.conn();
            let mut stmt = conn.prepare("SELECT command_id,request FROM crew_commands WHERE outcome IS NULL AND command_id>?1 ORDER BY command_id LIMIT ?2")?;
            Ok(stmt.query_map(params![after, limit.clamp(1, 64) as i64], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect::<Result<_, _>>()?)
        })
    }

    /// Lossless migration evidence/outcomes, never an execution queue.
    pub fn legacy_crew_records(&self, doc_id: &str) -> Result<Vec<(String, String, Vec<u8>)>, StoreError> {
        store_blocking(|| {
            let conn = self.conn();
            let mut stmt = conn.prepare("SELECT container,record_id,value FROM legacy_crew_records WHERE doc_id=?1 ORDER BY container,record_id")?;
            Ok(stmt.query_map([doc_id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
                .collect::<Result<_, _>>()?)
        })
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

    pub(crate) fn conn(&self) -> MutexGuard<'_, Connection> {
        // A poisoned lock only means another thread panicked mid-query; the
        // connection itself is still usable.
        self.conn.lock().unwrap_or_else(PoisonError::into_inner)
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
        tx.execute_batch(sql)?;
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

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn private_auth_baseline_and_existing_boat_store_preserve_claims_and_outcomes() {
        for existing_boat in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let outcome = br#"["applied",null]"#;
            {
                let conn = Connection::open(dir.path().join("docs.sqlite3")).unwrap();
                conn.execute_batch("CREATE TABLE schema_migrations (version INTEGER PRIMARY KEY, applied_at INTEGER NOT NULL) STRICT;").unwrap();
                // Private auth 0.2.115 retains the prototype's first nine migrations.
                for (index, sql) in MIGRATIONS[..9].iter().enumerate() {
                    conn.execute_batch(sql).unwrap();
                    conn.execute("INSERT INTO schema_migrations VALUES (?1,0)", [index + 1]).unwrap();
                }
                conn.execute("INSERT INTO processed_commands VALUES ('legacy',123)", []).unwrap();
                conn.execute("INSERT INTO snapshots (doc_id,bytes,saved_at) VALUES ('chat',?1,456)", [b"retained".as_slice()]).unwrap();
                if existing_boat {
                    // Freeze the already-deployed prototype's version and SQL,
                    // rather than constructing it from the migration under test.
                    conn.execute_batch("ALTER TABLE processed_commands ADD COLUMN outcome BLOB;
                        INSERT INTO schema_migrations VALUES (10,0);").unwrap();
                    conn.execute("INSERT INTO processed_commands (command_id,processed_at,outcome) VALUES ('settled',789,?1)", [outcome.as_slice()]).unwrap();
                }
            }
            for _ in 0..2 {
                let store = DocsStore::open(dir.path()).unwrap();
                assert_eq!(store.load_snapshot("chat").unwrap().as_deref(), Some(b"retained".as_slice()));
                assert!(!store.mark_processed("legacy").unwrap());
                assert_eq!(store.processed_command_outcome("legacy").unwrap(), None);
                if existing_boat {
                    assert!(!store.mark_processed("settled").unwrap());
                    assert_eq!(store.processed_command_outcome("settled").unwrap().as_deref(), Some(outcome.as_slice()));
                }
                let conn = store.conn();
                assert_eq!(conn.query_row("SELECT COUNT(*) FROM schema_migrations", [], |row| row.get::<_, usize>(0)).unwrap(), 10);
                assert_eq!(conn.query_row("SELECT processed_at FROM processed_commands WHERE command_id='legacy'", [], |row| row.get::<_, i64>(0)).unwrap(), 123);
            }
        }
    }

    #[test]
    fn processed_outcomes_migrate_legacy_claims_and_survive_reopen() {
        let dir = tempfile::tempdir().unwrap();
        {
            let conn = Connection::open(dir.path().join("docs.sqlite3")).unwrap();
            conn.execute_batch(MIGRATIONS[0]).unwrap();
            conn.execute_batch("CREATE TABLE schema_migrations (version INTEGER PRIMARY KEY, applied_at INTEGER NOT NULL) STRICT;
                INSERT INTO schema_migrations VALUES (1, 0);
                INSERT INTO processed_commands VALUES ('legacy-interrupted', 0);").unwrap();
        }
        let outcome = br#"["applied",null]"#;
        {
            let store = DocsStore::open(dir.path()).unwrap();
            assert!(store.is_processed("legacy-interrupted").unwrap());
            assert_eq!(store.processed_command_outcome("legacy-interrupted").unwrap(), None);
            assert_eq!(store.processed_command_outcome("missing").unwrap(), None);
            assert!(store.finish_processed_command("missing", outcome).is_err());
            assert!(store.mark_processed("relayed").unwrap());
            store.finish_processed_command("relayed", outcome).unwrap();
            store.finish_processed_command("relayed", outcome).unwrap();
            assert!(store.finish_processed_command("relayed", br#"["rejected","interrupted"]"#).is_err());
        }
        let store = DocsStore::open(dir.path()).unwrap();
        assert_eq!(store.processed_command_outcome("relayed").unwrap().as_deref(), Some(outcome.as_slice()));
        assert!(!store.mark_processed("relayed").unwrap());
        assert_eq!(store.processed_command_outcome("legacy-interrupted").unwrap(), None);
        let target_dir = tempfile::tempdir().unwrap();
        let target = DocsStore::open(target_dir.path()).unwrap();
        assert_eq!(target.import_processed_commands(&store.processed_commands().unwrap()).unwrap(), 2);
        assert_eq!(target.processed_command_outcome("relayed").unwrap().as_deref(), Some(outcome.as_slice()));
        assert!(target.is_processed("legacy-interrupted").unwrap());
        assert_eq!(target.processed_command_outcome("legacy-interrupted").unwrap(), None);
    }

    #[test]
    fn profile_import_fills_unknown_outcomes_without_replacing_known_results() {
        let source_dir = tempfile::tempdir().unwrap();
        let source = DocsStore::open(source_dir.path()).unwrap();
        let target_dir = tempfile::tempdir().unwrap();
        let target = DocsStore::open(target_dir.path()).unwrap();
        let applied = br#"["applied",null]"#;
        let rejected = br#"["rejected","known failure"]"#;
        for id in ["legacy-claim", "known-result", "new-claim"] {
            source.mark_processed(id).unwrap();
            source.finish_processed_command(id, applied).unwrap();
        }
        target.mark_processed("legacy-claim").unwrap();
        target.mark_processed("known-result").unwrap();
        target.finish_processed_command("known-result", rejected).unwrap();
        let rows = source.processed_commands().unwrap();
        assert_eq!(target.import_processed_commands(&rows).unwrap(), 1, "only new claims count as inserted");
        assert_eq!(target.processed_command_outcome("legacy-claim").unwrap().as_deref(), Some(applied.as_slice()));
        assert_eq!(target.processed_command_outcome("known-result").unwrap().as_deref(), Some(rejected.as_slice()));
        assert_eq!(target.processed_command_outcome("new-claim").unwrap().as_deref(), Some(applied.as_slice()));
        assert_eq!(target.import_processed_commands(&rows).unwrap(), 0);
        assert!(!target.mark_processed("legacy-claim").unwrap());
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

            let store = DocsStore::open(dir.path()).expect("maintenance never fails open");
            store.reclaim_free_space();
            // Returning while the other transaction is still held proves
            // maintenance defers contention. Total open/reclaim wall time also
            // includes filesystem work and host scheduling, not just busy waits.
            assert!(
                !other.is_autocommit(),
                "{lock}: the competing lock is still held"
            );
            assert_eq!(
                pragma(&store.conn(), "busy_timeout"),
                STORE_BUSY_TIMEOUT.as_millis() as i64,
                "{lock}: maintenance restores the normal write timeout"
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
