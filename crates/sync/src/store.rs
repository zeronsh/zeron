//! `DocsStore` — local SQLite persistence for doc snapshots and the
//! processed-command ledger (ARCHITECTURE §2 command plane: entries are marked
//! processed BEFORE execution so a crash can never double-execute a command).

use std::collections::HashSet;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OpenFlags, OptionalExtension, params};

/// Errors surfaced by [`DocsStore`].
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("loro: {0}")]
    Loro(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
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
    // Boot salvage skips a chat once its transcript has been seen non-empty.
    // The run journal is never deleted or rewritten from this marker.
    "CREATE TABLE salvage_checked (
        doc_id TEXT PRIMARY KEY
    ) STRICT;",
    // Invalidate old unversioned receipts and tie future receipts to every
    // change of the persisted snapshot or its pending updates.
    "DELETE FROM salvage_checked;
     CREATE TRIGGER salvage_snapshot_insert AFTER INSERT ON snapshots BEGIN
       DELETE FROM salvage_checked WHERE doc_id = NEW.doc_id;
     END;
     CREATE TRIGGER salvage_snapshot_update AFTER UPDATE ON snapshots BEGIN
       DELETE FROM salvage_checked WHERE doc_id = NEW.doc_id;
     END;
     CREATE TRIGGER salvage_snapshot_delete AFTER DELETE ON snapshots BEGIN
       DELETE FROM salvage_checked WHERE doc_id = OLD.doc_id;
     END;
     CREATE TRIGGER salvage_outbox_insert AFTER INSERT ON chat_outbox BEGIN
       DELETE FROM salvage_checked WHERE doc_id = NEW.doc_id;
     END;
     CREATE TRIGGER salvage_outbox_delete AFTER DELETE ON chat_outbox BEGIN
       DELETE FROM salvage_checked WHERE doc_id = OLD.doc_id;
     END;",
    // Delta rows preserve the legacy base's bytes/cursor. These AFTER triggers
    // are a backstop: migration 10's guard rejects a base write while deltas
    // exist, so only a database that lost that guard reaches them.
    "CREATE TABLE snapshot_deltas (
        doc_id TEXT NOT NULL,
        seq INTEGER NOT NULL,
        bytes BLOB NOT NULL,
        cursor INTEGER NOT NULL,
        roots BLOB NOT NULL,
        PRIMARY KEY(doc_id, seq)
    ) STRICT;
    CREATE TRIGGER snapshot_deltas_replace AFTER UPDATE ON snapshots BEGIN
        DELETE FROM snapshot_deltas WHERE doc_id=OLD.doc_id;
    END;
    CREATE TRIGGER snapshot_deltas_delete AFTER DELETE ON snapshots BEGIN
        DELETE FROM snapshot_deltas WHERE doc_id=OLD.doc_id;
    END;
    CREATE TRIGGER salvage_delta_insert AFTER INSERT ON snapshot_deltas BEGIN
        DELETE FROM salvage_checked WHERE doc_id=NEW.doc_id;
    END;
    CREATE TRIGGER salvage_delta_update AFTER UPDATE ON snapshot_deltas BEGIN
        DELETE FROM salvage_checked WHERE doc_id=OLD.doc_id OR doc_id=NEW.doc_id;
    END;
    CREATE TRIGGER salvage_delta_delete AFTER DELETE ON snapshot_deltas BEGIN
        DELETE FROM salvage_checked WHERE doc_id=OLD.doc_id;
    END;",
    // Downgrade guard. An engine that predates the delta log reads only the
    // base and would overwrite it, discarding the delta-only state. These
    // triggers make its save/delete fail loudly while deltas exist; the delta
    // engine always removes the deltas first, in the same transaction.
    "CREATE TRIGGER snapshot_deltas_guard_replace BEFORE UPDATE ON snapshots
     WHEN EXISTS (SELECT 1 FROM snapshot_deltas WHERE doc_id=OLD.doc_id) BEGIN
        SELECT RAISE(ABORT, 'snapshot has an unmerged delta log: run the newer engine to compact it');
     END;
     CREATE TRIGGER snapshot_deltas_guard_delete BEFORE DELETE ON snapshots
     WHEN EXISTS (SELECT 1 FROM snapshot_deltas WHERE doc_id=OLD.doc_id) BEGIN
        SELECT RAISE(ABORT, 'snapshot has an unmerged delta log: run the newer engine to compact it');
     END;",
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

/// Negative `cache_size` is kibibytes: 8 MiB on each connection.
const CACHE_SIZE_KIB: i64 = -8192;
/// 256 MiB. Large snapshot reads stay off the page cache.
const MMAP_SIZE_BYTES: i64 = 268_435_456;

/// SQLite-backed store under a data directory (`{data_dir}/docs.sqlite3`).
///
/// Holds warm-open doc snapshots (the DO room is authoritative; these make
/// cold starts instant and offline restarts possible) and the command ledger
/// that gives command execution mark-BEFORE-execute idempotence.
pub struct DocsStore {
    conn: Mutex<Connection>,
    /// WAL reader. Each call is its own statement or read transaction, so a
    /// committed write is visible on the next call without the writer mutex.
    reader: Mutex<Connection>,
    failed_publications: Mutex<HashSet<String>>,
    /// Write transactions the outbox has taken: one per [`Self::enqueue_chat_updates`]
    /// batch, one per [`Self::enqueue_chat_update`] row.
    outbox_write_txs: AtomicU64,
    /// Snapshot/chat-state load calls. Salvage tests
    /// use this to prove a checked chat is not read again.
    snapshot_load_count: AtomicU64,
    snapshot_payload_bytes: AtomicU64,
    /// Run before snapshot or delta writes; see [`Self::add_snapshot_barrier`].
    snapshot_barriers: Mutex<Vec<Weak<SnapshotBarrier>>>,
    /// Snapshot jobs wait here asynchronously before entering the blocking pool.
    pub snapshot_writer: std::sync::Arc<tokio::sync::Mutex<()>>,
}

/// A consistent base + contiguous, checksum-checked delta prefix. The cursor
/// belongs to the last imported row, including cursor resets (not MAX(cursor)).
pub struct ChatState {
    /// Exact durable base bytes, for conditional salvage receipts/copies.
    pub base_snapshot: Vec<u8>,
    pub doc: loro::LoroDoc,
    pub cursor: u64,
    pub epoch: u32,
    pub verified: bool,
    pub snapshot_bytes: usize,
    pub delta_count: usize,
    pub delta_bytes: usize,
    /// A damaged suffix was ignored; replace the base before appending again.
    pub needs_compaction: bool,
}

/// A callback that must return before a snapshot is written.
pub type SnapshotBarrier = dyn Fn() + Send + Sync;

impl DocsStore {
    /// Open (creating directory, database, and schema as needed).
    pub fn open(data_dir: impl AsRef<Path>) -> Result<Self, StoreError> {
        let data_dir = data_dir.as_ref();
        std::fs::create_dir_all(data_dir)?;
        let path = data_dir.join("docs.sqlite3");
        let mut conn = Connection::open(&path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "journal_size_limit", JOURNAL_SIZE_LIMIT_BYTES)?;
        tune_connection(&conn)?;
        conn.busy_timeout(STORE_BUSY_TIMEOUT)?;
        migrate(&mut conn)?;
        // Best-effort hygiene must never stall boot behind another connection.
        conn.busy_timeout(MAINTENANCE_BUSY_TIMEOUT)?;
        // Only the cheap WAL truncate here: open sits on boot paths (engine
        // assembly, the iOS client), and a VACUUM's cost is linear in the
        // live size. Callers run `reclaim_free_space` off those paths.
        // The reader opens afterwards: a live reader would make this
        // truncate report busy.
        checkpoint_truncate(&conn, &path, "open");
        conn.busy_timeout(STORE_BUSY_TIMEOUT)?;
        let reader = open_reader(&path)?;
        Ok(Self {
            conn: Mutex::new(conn),
            reader: Mutex::new(reader),
            failed_publications: Mutex::new(HashSet::new()),
            outbox_write_txs: AtomicU64::new(0),
            snapshot_load_count: AtomicU64::new(0),
            snapshot_payload_bytes: AtomicU64::new(0),
            snapshot_barriers: Mutex::new(Vec::new()),
            snapshot_writer: std::sync::Arc::new(tokio::sync::Mutex::new(())),
        })
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
        self.outbox_write_txs.fetch_add(1, Ordering::Relaxed);
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

    /// Insert several `(doc_id, batch_id, bytes)` rows in ONE transaction, in
    /// slice order (so `ordinal` follows it). All or nothing. A failure is NOT
    /// recorded as a failed publication: the caller retries the rows one by
    /// one with [`Self::enqueue_chat_update`], which records exactly the docs
    /// whose own row cannot be written.
    pub fn enqueue_chat_updates(&self, items: &[(&str, &str, &[u8])]) -> Result<(), StoreError> {
        if items.is_empty() {
            return Ok(());
        }
        store_blocking(|| self.enqueue_chat_updates_blocking(items))
    }

    fn enqueue_chat_updates_blocking(
        &self,
        items: &[(&str, &str, &[u8])],
    ) -> Result<(), StoreError> {
        self.outbox_write_txs.fetch_add(1, Ordering::Relaxed);
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        {
            let mut insert = tx.prepare_cached(
                "INSERT OR IGNORE INTO chat_outbox(doc_id,batch_id,bytes) VALUES (?1,?2,?3)",
            )?;
            for (doc_id, batch_id, bytes) in items {
                insert.execute(params![doc_id, batch_id, bytes])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Outbox write transactions taken so far (a batch counts once).
    pub fn outbox_write_transactions(&self) -> u64 {
        self.outbox_write_txs.load(Ordering::Relaxed)
    }

    /// Register a callback that runs, outside the connection lock, before every
    /// snapshot/delta write. The engine uses it to flush its outbox writer: a
    /// snapshot may only land once the outbox rows for the operations it
    /// contains (or the record that such a row failed) are already on disk.
    /// The store keeps only a weak reference; dropping the `Arc` unregisters.
    pub fn add_snapshot_barrier(&self, barrier: &Arc<SnapshotBarrier>) {
        let mut barriers = self
            .snapshot_barriers
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        barriers.retain(|b| b.strong_count() > 0);
        barriers.push(Arc::downgrade(barrier));
    }

    fn run_snapshot_barriers(&self) {
        let live: Vec<_> = self
            .snapshot_barriers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .filter_map(Weak::upgrade)
            .collect();
        for barrier in live {
            barrier();
        }
    }

    pub fn pending_chat_updates(&self, doc_id: &str) -> Result<Vec<(String, Vec<u8>)>, StoreError> {
        store_blocking(|| self.pending_chat_updates_blocking(doc_id))
    }

    /// Admission/lifetime checks must not copy the pending payloads.
    pub fn has_pending_chat_updates(&self, doc_id: &str) -> Result<bool, StoreError> {
        store_blocking(|| {
            Ok(self.reader().query_row(
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
        let conn = self.reader();
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
        let conn = self.reader();
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
            .reader()
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

    /// Restore a chat using a single SQLite read transaction. A torn/corrupt
    /// row (or a gap/pending causal dependency) ends the usable prefix: never
    /// advance the cursor over missing content. SQLite itself commits rows
    /// atomically; checksum validation also contains damaged copied databases.
    pub fn load_chat_state(&self, doc_id: &str) -> Result<Option<ChatState>, StoreError> {
        store_blocking(|| {
            self.snapshot_load_count.fetch_add(1, Ordering::Relaxed);
            let (base, rows) = {
                let mut conn = self.reader();
                let tx = conn.transaction()?;
                let base = tx.query_row(
                    "SELECT bytes,COALESCE(cursor,0),COALESCE(epoch,0),cursor_verified FROM snapshots WHERE doc_id=?1",
                    [doc_id],
                    |r| Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, u64>(1)?, r.get::<_, u32>(2)?, r.get::<_, bool>(3)?)),
                ).optional()?;
                let rows = tx
                    .prepare(
                        "SELECT seq,bytes,roots,cursor FROM snapshot_deltas WHERE doc_id=?1 ORDER BY seq",
                    )?
                    .query_map([doc_id], |r| {
                        Ok((
                            r.get::<_, usize>(0)?,
                            r.get::<_, Vec<u8>>(1)?,
                            r.get::<_, Vec<u8>>(2)?,
                            r.get::<_, u64>(3)?,
                        ))
                    })?
                    .collect::<Result<Vec<_>, _>>()?;
                tx.commit()?;
                (base, rows)
            };
            let Some((base, cursor, epoch, verified)) = base else {
                return Ok(None);
            };
            let import_base = || {
                let doc = loro::LoroDoc::new();
                doc.import(&base)
                    .map_err(|e| StoreError::Loro(e.to_string()))?;
                Ok::<_, StoreError>(doc)
            };
            let mut state = ChatState {
                base_snapshot: Vec::new(),
                doc: import_base()?,
                cursor,
                epoch,
                verified,
                snapshot_bytes: base.len(),
                delta_count: 0,
                delta_bytes: 0,
                needs_compaction: false,
            };
            for (seq, bytes, roots, cursor) in &rows {
                let valid = *seq == state.delta_count + 1
                    && import_snapshot_delta(&state.doc, bytes, roots).is_ok();
                if !valid {
                    tracing::warn!(
                        doc = doc_id,
                        seq,
                        "ignoring damaged snapshot delta and suffix"
                    );
                    // An import error/pending update may have partially mutated
                    // Loro. Reconstruct only the known-good prefix.
                    state.needs_compaction = true;
                    state.doc = import_base()?;
                    for (_, bytes, roots, _) in rows.iter().take(state.delta_count) {
                        import_snapshot_delta(&state.doc, bytes, roots)?;
                    }
                    break;
                }
                state.cursor = *cursor;
                state.verified = true;
                state.delta_count += 1;
                state.delta_bytes += bytes.len() + roots.len();
            }
            state.base_snapshot = base;
            Ok(Some(state))
        })
    }

    /// Shallow root registry: empty roots have no Loro operations, but a full
    /// snapshot preserves them. Encoding only IDs avoids materializing data.
    /// Layout: `fnv1a64(payload) LE | payload`, payload = `ROOT_REGISTRY_V1`
    /// then, per root, `u32 LE length | ContainerID::to_bytes()`. Loro's own
    /// binary ID encoding is used rather than serde, whose representation of
    /// container IDs has changed between Loro releases and would turn every
    /// row written before an upgrade into a "damaged" one.
    pub fn snapshot_roots(doc: &loro::LoroDoc) -> Result<Vec<u8>, StoreError> {
        let value = doc.get_value();
        let loro::LoroValue::Map(map) = value else {
            return Err(StoreError::Loro("expected document root map".into()));
        };
        let mut payload = vec![ROOT_REGISTRY_V1];
        for v in map.values() {
            if let loro::LoroValue::Container(id) = v {
                let id = id.to_bytes();
                payload.extend((id.len() as u32).to_le_bytes());
                payload.extend(id);
            }
        }
        let mut bytes = root_registry_checksum(&payload).to_le_bytes().to_vec();
        bytes.extend(payload);
        Ok(bytes)
    }

    /// Append one durable update + cursor and the registry from
    /// [`Self::snapshot_roots`]. Publication barriers are identical to full
    /// saves, but this log neither acknowledges nor drains the outbox.
    pub fn append_snapshot_delta(
        &self,
        doc_id: &str,
        bytes: &[u8],
        cursor: u64,
        roots: &[u8],
    ) -> Result<(), StoreError> {
        self.run_snapshot_barriers();
        store_blocking(|| {
            let mut conn = self.conn();
            let tx = conn.transaction()?;
            tx.execute(
                "INSERT INTO snapshot_deltas(doc_id,seq,bytes,cursor,roots)
                 SELECT ?1,COALESCE((SELECT MAX(seq) FROM snapshot_deltas WHERE doc_id=?1),0)+1,?2,?3,?4
                 WHERE EXISTS(SELECT 1 FROM snapshots WHERE doc_id=?1)",
                params![doc_id, bytes, cursor, roots],
            ).and_then(|n| if n == 1 { Ok(n) } else { Err(rusqlite::Error::QueryReturnedNoRows) })?;
            self.invalidate_failed_publication(&tx, doc_id)?;
            tx.commit()?;
            self.snapshot_payload_bytes
                .fetch_add((bytes.len() + roots.len()) as u64, Ordering::Relaxed);
            Ok(())
        })
    }

    /// Committed snapshot/delta payload bytes (including root registries)
    /// submitted to SQLite. Excludes
    /// indexes, pages, WAL amplification and the independent publication outbox.
    pub fn snapshot_payload_bytes_written(&self) -> u64 {
        self.snapshot_payload_bytes.load(Ordering::Relaxed)
    }

    /// Latest full base snapshot for `doc_id`, if any. Chat readers must use
    /// [`Self::load_chat_state`] to include the durable delta suffix.
    pub fn load_snapshot(&self, doc_id: &str) -> Result<Option<Vec<u8>>, StoreError> {
        store_blocking(|| self.load_snapshot_blocking(doc_id))
    }

    fn load_snapshot_blocking(&self, doc_id: &str) -> Result<Option<Vec<u8>>, StoreError> {
        self.snapshot_load_count.fetch_add(1, Ordering::Relaxed);
        let bytes = self
            .reader()
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
        self.run_snapshot_barriers();
        store_blocking(|| self.save_snapshot_blocking(doc_id, bytes))
    }

    fn save_snapshot_blocking(&self, doc_id: &str, bytes: &[u8]) -> Result<(), StoreError> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        // Deltas first: the downgrade guard rejects a base write while any exist.
        tx.execute("DELETE FROM snapshot_deltas WHERE doc_id=?1", [doc_id])?;
        tx.execute(
            "INSERT INTO snapshots (doc_id, bytes, saved_at) VALUES (?1, ?2, ?3)
             ON CONFLICT(doc_id) DO UPDATE SET bytes = excluded.bytes, saved_at = excluded.saved_at",
            params![doc_id, bytes, now_ms()],
        )?;
        self.invalidate_failed_publication(&tx, doc_id)?;
        tx.commit()?;
        self.snapshot_payload_bytes
            .fetch_add(bytes.len() as u64, Ordering::Relaxed);
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
        self.run_snapshot_barriers();
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
        tx.execute("DELETE FROM snapshot_deltas WHERE doc_id=?1", [doc_id])?;
        tx.execute(
            "INSERT INTO snapshots (doc_id, bytes, saved_at, cursor, epoch) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(doc_id) DO UPDATE SET bytes = excluded.bytes, saved_at = excluded.saved_at,
                 cursor = excluded.cursor, epoch = excluded.epoch, cursor_verified = 0",
            params![doc_id, bytes, now_ms(), cursor as i64, epoch as i64],
        )?;
        self.invalidate_failed_publication(&tx, doc_id)?;
        tx.commit()?;
        self.snapshot_payload_bytes
            .fetch_add(bytes.len() as u64, Ordering::Relaxed);
        Ok(())
    }

    pub fn save_verified_snapshot_with_cursor(
        &self,
        doc_id: &str,
        bytes: &[u8],
        cursor: u64,
        epoch: u32,
    ) -> Result<(), StoreError> {
        self.run_snapshot_barriers();
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
        tx.execute("DELETE FROM snapshot_deltas WHERE doc_id=?1", [doc_id])?;
        tx.execute(
            "INSERT INTO snapshots (doc_id, bytes, saved_at, cursor, epoch, cursor_verified) VALUES (?1, ?2, ?3, ?4, ?5, 1)
             ON CONFLICT(doc_id) DO UPDATE SET bytes = excluded.bytes, saved_at = excluded.saved_at,
                 cursor = excluded.cursor, epoch = excluded.epoch, cursor_verified = 1",
            params![doc_id, bytes, now_ms(), cursor as i64, epoch as i64],
        )?;
        self.invalidate_failed_publication(&tx, doc_id)?;
        tx.commit()?;
        self.snapshot_payload_bytes
            .fetch_add(bytes.len() as u64, Ordering::Relaxed);
        Ok(())
    }

    pub fn snapshot_cursor(&self, doc_id: &str) -> Result<u64, StoreError> {
        store_blocking(|| {
            Ok(self
                .reader()
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
            .reader()
            .query_row(
                "SELECT cursor_verified FROM snapshots WHERE doc_id = ?1",
                params![doc_id],
                |row| row.get(0),
            )
            .optional()?
            .unwrap_or(false))
    }

    /// Full base + its chat2 cursor + epoch (does not include deltas).
    /// Pre-migration rows (or rows written
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
                .reader()
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
        self.snapshot_load_count.fetch_add(1, Ordering::Relaxed);
        let row = self
            .reader()
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
        tx.execute(
            "DELETE FROM snapshot_deltas WHERE doc_id = ?1",
            params![doc_id],
        )?;
        tx.execute("DELETE FROM snapshots WHERE doc_id = ?1", params![doc_id])?;
        tx.execute("DELETE FROM chat_outbox WHERE doc_id = ?1", params![doc_id])?;
        tx.execute(
            "DELETE FROM chat_sync_jobs WHERE doc_id = ?1",
            params![doc_id],
        )?;
        tx.execute(
            "DELETE FROM chat_outbox_initialized WHERE doc_id = ?1",
            params![doc_id],
        )?;
        tx.execute(
            "DELETE FROM salvage_checked WHERE doc_id = ?1",
            params![doc_id],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// How many times a snapshot blob was loaded. Diagnostic; salvage tests
    /// assert a checked chat does not move it.
    #[doc(hidden)]
    pub fn snapshot_loads(&self) -> u64 {
        self.snapshot_load_count.load(Ordering::Relaxed)
    }

    /// Whether boot salvage has already seen a non-empty transcript for `doc_id`.
    pub fn salvage_checked(&self, doc_id: &str) -> Result<bool, StoreError> {
        store_blocking(|| {
            let hit = self
                .reader()
                .query_row(
                    "SELECT 1 FROM salvage_checked WHERE doc_id = ?1
                     AND NOT EXISTS (SELECT 1 FROM snapshot_deltas WHERE doc_id = ?1)",
                    params![doc_id],
                    |_| Ok(()),
                )
                .optional()?;
            Ok(hit.is_some())
        })
    }

    /// Mark only the exact inspected snapshot, while no pending updates can
    /// change its decoded transcript. Deltas must first compact into the base;
    /// the comparison and insert are atomic with respect to both write logs.
    pub fn mark_salvage_checked(
        &self,
        doc_id: &str,
        bytes: &[u8],
        epoch: u32,
    ) -> Result<(), StoreError> {
        store_blocking(|| {
            self.conn().execute(
                "INSERT OR IGNORE INTO salvage_checked (doc_id)
                 SELECT doc_id FROM snapshots WHERE doc_id = ?1 AND bytes = ?2
                 AND COALESCE(epoch, 0) = ?3
                 AND NOT EXISTS (SELECT 1 FROM chat_outbox WHERE doc_id = ?1)
                 AND NOT EXISTS (SELECT 1 FROM snapshot_deltas WHERE doc_id = ?1)",
                params![doc_id, bytes, epoch],
            )?;
            Ok(())
        })
    }

    /// Whether a snapshot row exists for `doc_id` — presence only, no blob read.
    pub fn has_snapshot(&self, doc_id: &str) -> Result<bool, StoreError> {
        store_blocking(|| self.has_snapshot_blocking(doc_id))
    }

    fn has_snapshot_blocking(&self, doc_id: &str) -> Result<bool, StoreError> {
        let hit = self
            .reader()
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
        let conn = self.reader();
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
            .reader()
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

    pub(crate) fn conn(&self) -> MutexGuard<'_, Connection> {
        // A poisoned lock only means another thread panicked mid-query; the
        // connection itself is still usable.
        self.conn.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Read connection. Mutations and maintenance (`reclaim_free_space`,
    /// checkpoint) stay on [`Self::conn`].
    pub(crate) fn reader(&self) -> MutexGuard<'_, Connection> {
        self.reader.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

// Fixed FNV-1a checksum for the small JSON registry. Loro checksums cover
// the update blob; this covers the other payload, including valid-but-torn JSON.
fn root_registry_checksum(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    })
}

const ROOT_REGISTRY_V1: u8 = 1;

fn decode_root_registry(payload: &[u8]) -> Result<Vec<loro::ContainerID>, StoreError> {
    let bad = |what: &str| StoreError::Loro(format!("delta root registry: {what}"));
    let (version, mut rest) = payload.split_first().ok_or_else(|| bad("empty"))?;
    if *version != ROOT_REGISTRY_V1 {
        return Err(bad("unknown version"));
    }
    let mut roots = Vec::new();
    while !rest.is_empty() {
        let (len, tail) = rest
            .split_at_checked(4)
            .ok_or_else(|| bad("truncated length"))?;
        let len = u32::from_le_bytes(len.try_into().unwrap()) as usize;
        let (id, tail) = tail
            .split_at_checked(len)
            .ok_or_else(|| bad("truncated id"))?;
        let id = loro::ContainerID::try_from_bytes(id).map_err(|e| bad(&e.to_string()))?;
        if !id.is_root() {
            return Err(bad("contains a nested container"));
        }
        roots.push(id);
        rest = tail;
    }
    Ok(roots)
}

fn import_snapshot_delta(
    doc: &loro::LoroDoc,
    bytes: &[u8],
    roots: &[u8],
) -> Result<(), StoreError> {
    let (checksum, payload) = roots
        .split_at_checked(8)
        .ok_or_else(|| StoreError::Loro("truncated delta root registry".into()))?;
    if checksum != root_registry_checksum(payload).to_le_bytes() {
        return Err(StoreError::Loro(
            "delta root registry checksum mismatch".into(),
        ));
    }
    let roots = decode_root_registry(payload)?;
    loro::LoroDoc::decode_import_blob_meta(bytes, true)
        .map_err(|e| StoreError::Loro(e.to_string()))?;
    let status = doc
        .import(bytes)
        .map_err(|e| StoreError::Loro(e.to_string()))?;
    if status.pending.is_some() {
        return Err(StoreError::Loro(
            "delta has missing causal dependencies".into(),
        ));
    }
    for id in roots {
        if doc.get_container(id).is_none() {
            return Err(StoreError::Loro("invalid delta root container".into()));
        }
    }
    Ok(())
}

/// `temp_store`, page cache, and mmap. Per connection; not a schema change.
/// `synchronous` is left as the caller set it.
fn tune_connection(conn: &Connection) -> Result<(), StoreError> {
    conn.pragma_update(None, "temp_store", "MEMORY")?;
    conn.pragma_update(None, "cache_size", CACHE_SIZE_KIB)?;
    conn.pragma_update(None, "mmap_size", MMAP_SIZE_BYTES)?;
    Ok(())
}

fn open_reader(path: &Path) -> Result<Connection, StoreError> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    tune_connection(&conn)?;
    conn.busy_timeout(STORE_BUSY_TIMEOUT)?;
    Ok(conn)
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

    fn assert_same_doc(a: &loro::LoroDoc, b: &loro::LoroDoc) {
        assert_eq!(a.oplog_vv(), b.oplog_vv());
        assert_eq!(a.get_deep_value(), b.get_deep_value());
    }

    fn random_next(state: &mut u64) -> u64 {
        *state ^= *state << 13;
        *state ^= *state >> 7;
        *state ^= *state << 17;
        *state
    }

    #[test]
    fn delta_log_seeded_crash_reopen_equals_full_snapshot() {
        let seeds = std::env::var("PERF_SEEDS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(300);
        let dir = tempfile::tempdir().unwrap();
        let full_dir = tempfile::tempdir().unwrap();
        let mut store = DocsStore::open(dir.path()).unwrap();
        let full = DocsStore::open(full_dir.path()).unwrap();
        for seed in 1..=seeds {
            let mut rng = seed as u64;
            let mut doc = loro::LoroDoc::new();
            doc.set_peer_id(seed as u64).unwrap();
            let peer = loro::LoroDoc::new();
            peer.set_peer_id(seed as u64 + 100_000).unwrap();
            let base = doc.export(loro::ExportMode::Snapshot).unwrap();
            store
                .save_verified_snapshot_with_cursor("chat", &base, 0, 2)
                .unwrap();
            full.save_verified_snapshot_with_cursor("chat", &base, 0, 2)
                .unwrap();
            let mut saved_vv = doc.oplog_vv();
            let mut cursor = 0;
            for op in 0..150 {
                let n = random_next(&mut rng);
                match n % 6 {
                    0 => {
                        doc.get_map("meta")
                            .insert(&format!("k{}", n % 9), op as i64)
                            .unwrap();
                    }
                    1 => {
                        let text = doc.get_text("stream");
                        text.insert(text.len_unicode(), "delta🦀").unwrap();
                    }
                    2 => {
                        let text = doc.get_text("stream");
                        if text.len_unicode() > 0 {
                            text.delete(0, 1).unwrap();
                        }
                    }
                    3 => {
                        doc.get_map("meta").delete(&format!("k{}", n % 9)).unwrap();
                    }
                    4 => {
                        let list = doc.get_list("items");
                        list.push(op as i64).unwrap();
                        if list.len() > 4 {
                            list.delete(0, 1).unwrap();
                        }
                    }
                    _ => {
                        peer.get_map("remote")
                            .insert(&format!("k{}", n % 7), op as i64)
                            .unwrap();
                        peer.commit();
                        doc.import(&peer.export(loro::ExportMode::all_updates()).unwrap())
                            .unwrap();
                    }
                }
                doc.commit();
                if n % 4 != 0 {
                    cursor += 1;
                    let delta = doc.export(loro::ExportMode::updates(&saved_vv)).unwrap();
                    store
                        .append_snapshot_delta(
                            "chat",
                            &delta,
                            cursor,
                            &DocsStore::snapshot_roots(&doc).unwrap(),
                        )
                        .unwrap();
                    saved_vv = doc.oplog_vv();
                    let snapshot = doc.export(loro::ExportMode::Snapshot).unwrap();
                    full.save_verified_snapshot_with_cursor("chat", &snapshot, cursor, 2)
                        .unwrap();
                    if n % 11 == 0 {
                        store
                            .save_verified_snapshot_with_cursor("chat", &snapshot, cursor, 2)
                            .unwrap();
                        assert_eq!(
                            store.load_chat_state("chat").unwrap().unwrap().delta_count,
                            0
                        );
                    }
                }
                // A crash discards edits since the last scheduled 1 s save.
                // Both databases must restore the same VV as well as value.
                if n % 5 == 0 || op == 149 {
                    drop(store);
                    store = DocsStore::open(dir.path()).unwrap();
                    let delta = store.load_chat_state("chat").unwrap().unwrap();
                    let reference = full.load_chat_state("chat").unwrap().unwrap();
                    assert_same_doc(&delta.doc, &reference.doc);
                    assert_eq!(delta.cursor, reference.cursor, "seed={seed} op={op}");
                    assert!(delta.verified);
                    doc = delta.doc;
                    doc.set_peer_id(seed as u64).unwrap();
                    saved_vv = doc.oplog_vv();
                }
            }
        }
    }

    #[test]
    fn root_registry_encoding_is_pinned_and_rejects_foreign_ids() {
        let doc = loro::LoroDoc::new();
        doc.get_text("empty");
        doc.get_map("meta").insert("k", 1).unwrap();
        doc.commit();
        let bytes = DocsStore::snapshot_roots(&doc).unwrap();
        let (checksum, payload) = bytes.split_at(8);
        assert_eq!(checksum, root_registry_checksum(payload).to_le_bytes());
        // Loro's binary ID layout, not serde: tag byte then LEB128-length name.
        let empty = loro::ContainerID::new_root("empty", loro::ContainerType::Text);
        let mut expected = (empty.to_bytes().len() as u32).to_le_bytes().to_vec();
        expected.extend(empty.to_bytes());
        assert!(payload.windows(expected.len()).any(|w| w == expected));
        let mut roots = decode_root_registry(payload).unwrap();
        roots.sort_by_key(|id| id.to_string());
        assert_eq!(roots.len(), 2);
        // A nested ID, a future version and a short length are all rejected.
        let nested = loro::ContainerID::new_normal(loro::ID::new(1, 0), loro::ContainerType::Map);
        let mut foreign = vec![ROOT_REGISTRY_V1];
        foreign.extend((nested.to_bytes().len() as u32).to_le_bytes());
        foreign.extend(nested.to_bytes());
        assert!(decode_root_registry(&foreign).is_err());
        assert!(decode_root_registry(&[ROOT_REGISTRY_V1 + 1]).is_err());
        assert!(decode_root_registry(&[ROOT_REGISTRY_V1, 9, 0]).is_err());
        assert!(decode_root_registry(&[]).is_err());
    }

    #[test]
    fn delta_log_preserves_unversioned_empty_roots_and_rejects_torn_registry() {
        let dir = tempfile::tempdir().unwrap();
        let store = DocsStore::open(dir.path()).unwrap();
        let doc = loro::LoroDoc::new();
        store
            .save_verified_snapshot_with_cursor(
                "chat",
                &doc.export(loro::ExportMode::Snapshot).unwrap(),
                0,
                2,
            )
            .unwrap();
        let vv = doc.oplog_vv();
        doc.get_text("empty");
        assert_eq!(doc.oplog_vv(), vv, "accessing a root records no operation");
        store
            .append_snapshot_delta(
                "chat",
                &doc.export(loro::ExportMode::updates(&vv)).unwrap(),
                1,
                &DocsStore::snapshot_roots(&doc).unwrap(),
            )
            .unwrap();
        let full = loro::LoroDoc::new();
        full.import(&doc.export(loro::ExportMode::Snapshot).unwrap())
            .unwrap();
        assert_same_doc(&store.load_chat_state("chat").unwrap().unwrap().doc, &full);
        store
            .conn()
            .execute(
                "UPDATE snapshot_deltas SET roots=substr(roots,1,length(roots)-1)",
                [],
            )
            .unwrap();
        let state = store.load_chat_state("chat").unwrap().unwrap();
        assert_eq!(state.cursor, 0);
        assert_eq!(state.delta_count, 0);
        assert!(state.needs_compaction);
        assert_same_doc(&state.doc, &loro::LoroDoc::new());
    }

    #[test]
    fn delta_log_torn_row_and_dependent_suffix_are_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let store = DocsStore::open(dir.path()).unwrap();
        let doc = loro::LoroDoc::new();
        store
            .save_verified_snapshot_with_cursor(
                "chat",
                &doc.export(loro::ExportMode::Snapshot).unwrap(),
                0,
                2,
            )
            .unwrap();
        let mut vv = doc.oplog_vv();
        for cursor in 1..=3 {
            doc.get_text("text").insert(0, "abc").unwrap();
            doc.commit();
            store
                .append_snapshot_delta(
                    "chat",
                    &doc.export(loro::ExportMode::updates(&vv)).unwrap(),
                    cursor,
                    &DocsStore::snapshot_roots(&doc).unwrap(),
                )
                .unwrap();
            vv = doc.oplog_vv();
        }
        let first = store
            .conn()
            .query_row("SELECT bytes FROM snapshot_deltas WHERE seq=1", [], |r| {
                r.get::<_, Vec<u8>>(0)
            })
            .unwrap();
        let expected = loro::LoroDoc::new();
        expected.import(&first).unwrap();
        // Simulate a copied/truncated payload with a committed cursor. A
        // real interrupted SQLite INSERT instead rolls back the entire row.
        store
            .conn()
            .execute(
                "UPDATE snapshot_deltas SET bytes=substr(bytes,1,length(bytes)-7) WHERE seq=2",
                [],
            )
            .unwrap();
        drop(store);
        let store = DocsStore::open(dir.path()).unwrap();
        let state = store.load_chat_state("chat").unwrap().unwrap();
        assert_same_doc(&state.doc, &expected);
        assert_eq!(state.cursor, 1);
        assert_eq!(state.delta_count, 1);
    }

    #[test]
    fn delta_log_compaction_and_insert_rollback_preserve_prefix_and_outbox() {
        let dir = tempfile::tempdir().unwrap();
        let store = DocsStore::open(dir.path()).unwrap();
        let doc = loro::LoroDoc::new();
        store
            .save_verified_snapshot_with_cursor(
                "chat",
                &doc.export(loro::ExportMode::Snapshot).unwrap(),
                7,
                2,
            )
            .unwrap();
        let vv = doc.oplog_vv();
        doc.get_text("text").insert(0, "durable").unwrap();
        doc.commit();
        let delta = doc.export(loro::ExportMode::updates(&vv)).unwrap();
        store
            .append_snapshot_delta("chat", &delta, 8, &DocsStore::snapshot_roots(&doc).unwrap())
            .unwrap();
        store
            .enqueue_chat_update("chat", "unacked", &delta)
            .unwrap();
        // A crash before COMMIT publishes neither content nor its cursor.
        {
            let mut conn = store.conn();
            let tx = conn.transaction().unwrap();
            tx.execute(
                "INSERT INTO snapshot_deltas VALUES('chat',2,?1,999,X'5b5d')",
                [&delta],
            )
            .unwrap();
        }
        // Failure during DELETE rolls back the base upsert AND trigger deletes.
        store.conn().execute_batch("CREATE TRIGGER fail_compact BEFORE DELETE ON snapshot_deltas BEGIN SELECT RAISE(ABORT,'crash'); END;").unwrap();
        let snapshot = doc.export(loro::ExportMode::Snapshot).unwrap();
        assert!(
            store
                .save_verified_snapshot_with_cursor("chat", &snapshot, 8, 2)
                .is_err()
        );
        let state = store.load_chat_state("chat").unwrap().unwrap();
        assert_eq!(state.delta_count, 1);
        assert_eq!(state.cursor, 8);
        assert_same_doc(&state.doc, &doc);
        store
            .conn()
            .execute_batch("DROP TRIGGER fail_compact")
            .unwrap();
        store
            .save_verified_snapshot_with_cursor("chat", &snapshot, 8, 2)
            .unwrap();
        drop(store);
        let store = DocsStore::open(dir.path()).unwrap();
        let state = store.load_chat_state("chat").unwrap().unwrap();
        assert_eq!(state.delta_count, 0);
        assert_eq!(state.cursor, 8);
        assert_same_doc(&state.doc, &doc);
        assert_eq!(store.pending_chat_updates("chat").unwrap().len(), 1);
    }

    #[test]
    fn delta_log_invalidates_salvage_receipts_and_prevents_stale_certification() {
        let dir = tempfile::tempdir().unwrap();
        let store = DocsStore::open(dir.path()).unwrap();
        let doc = loro::LoroDoc::new();
        doc.get_text("text").insert(0, "healthy base").unwrap();
        doc.commit();
        let base = doc.export(loro::ExportMode::Snapshot).unwrap();
        store
            .save_verified_snapshot_with_cursor("chat", &base, 1, 2)
            .unwrap();
        store.mark_salvage_checked("chat", &base, 2).unwrap();
        assert!(store.salvage_checked("chat").unwrap());
        let vv = doc.oplog_vv();
        doc.get_text("text").delete(0, 12).unwrap();
        doc.commit();
        let delta = doc.export(loro::ExportMode::updates(&vv)).unwrap();
        // Failed appends cannot invalidate the receipt of unchanged content.
        store.conn().execute_batch("CREATE TRIGGER fail_delta BEFORE INSERT ON snapshot_deltas BEGIN SELECT RAISE(ABORT,'crash'); END;").unwrap();
        assert!(
            store
                .append_snapshot_delta("chat", &delta, 2, &DocsStore::snapshot_roots(&doc).unwrap())
                .is_err()
        );
        assert!(store.salvage_checked("chat").unwrap());
        store
            .conn()
            .execute_batch("DROP TRIGGER fail_delta")
            .unwrap();
        store
            .append_snapshot_delta("chat", &delta, 2, &DocsStore::snapshot_roots(&doc).unwrap())
            .unwrap();
        assert!(!store.salvage_checked("chat").unwrap());
        // A scan of the old healthy base raced the append that emptied it.
        store.mark_salvage_checked("chat", &base, 2).unwrap();
        assert!(!store.salvage_checked("chat").unwrap());
        for sql in [
            "UPDATE snapshot_deltas SET cursor=3 WHERE doc_id='chat'",
            "DELETE FROM snapshot_deltas WHERE doc_id='chat'",
        ] {
            store
                .conn()
                .execute("INSERT INTO salvage_checked VALUES('chat')", [])
                .unwrap(); // an old binary may certify only the legacy base
            assert!(!store.salvage_checked("chat").unwrap());
            store.conn().execute(sql, []).unwrap();
            assert!(!store.salvage_checked("chat").unwrap());
        }
        // Compaction's replacement base may be certified once both logs clear.
        let snapshot = doc.export(loro::ExportMode::Snapshot).unwrap();
        store
            .save_verified_snapshot_with_cursor("chat", &snapshot, 3, 2)
            .unwrap();
        store.mark_salvage_checked("chat", &snapshot, 2).unwrap();
        assert!(store.salvage_checked("chat").unwrap());
    }

    #[test]
    fn delta_log_legacy_migration_and_old_engine_write() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("docs.sqlite3");
        let doc = loro::LoroDoc::new();
        doc.get_map("legacy").insert("key", 1).unwrap();
        doc.commit();
        let base = doc.export(loro::ExportMode::Snapshot).unwrap();
        // Build the exact pre-delta schema, then apply only the new migrations.
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch("CREATE TABLE schema_migrations(version INTEGER PRIMARY KEY,applied_at INTEGER NOT NULL) STRICT").unwrap();
            for (index, sql) in MIGRATIONS.iter().take(MIGRATIONS.len() - 2).enumerate() {
                conn.execute_batch(sql).unwrap();
                conn.execute("INSERT INTO schema_migrations VALUES(?1,0)", [index + 1])
                    .unwrap();
            }
            conn.execute("INSERT INTO snapshots(doc_id,bytes,saved_at,cursor,epoch) VALUES('chat',?1,0,10,2)", [&base]).unwrap();
        }
        let store = DocsStore::open(dir.path()).unwrap();
        let state = store.load_chat_state("chat").unwrap().unwrap();
        assert_same_doc(&state.doc, &doc);
        assert!(!state.verified);
        let vv = doc.oplog_vv();
        doc.get_map("new").insert("delta", true).unwrap();
        doc.commit();
        store
            .append_snapshot_delta(
                "chat",
                &doc.export(loro::ExportMode::updates(&vv)).unwrap(),
                11,
                &DocsStore::snapshot_roots(&doc).unwrap(),
            )
            .unwrap();
        assert_eq!(
            store.load_snapshot_with_cursor("chat").unwrap().unwrap(),
            (base.clone(), 10, 2)
        );
        // Execute the old binary's actual SQL. It must not be able to replace
        // or delete a base while delta-only state exists: that state is the
        // only copy of the newest edits, and the old engine cannot read it.
        let old = Connection::open(path).unwrap();
        for sql in [
            "UPDATE snapshots SET bytes=x'00',cursor=12 WHERE doc_id='chat'",
            "INSERT INTO snapshots (doc_id, bytes, saved_at, cursor, epoch, cursor_verified) VALUES ('chat', x'00', 1, 12, 2, 1)
             ON CONFLICT(doc_id) DO UPDATE SET bytes = excluded.bytes, saved_at = excluded.saved_at,
                 cursor = excluded.cursor, epoch = excluded.epoch, cursor_verified = 1",
            "DELETE FROM snapshots WHERE doc_id='chat'",
        ] {
            let error = old.execute(sql, []).unwrap_err().to_string();
            assert!(error.contains("unmerged delta log"), "{error}");
        }
        let state = store.load_chat_state("chat").unwrap().unwrap();
        assert_eq!(state.delta_count, 1);
        assert_eq!(state.cursor, 11);
        assert_same_doc(&state.doc, &doc);
        // Once the delta engine compacts, the old binary may save again.
        store
            .save_verified_snapshot_with_cursor(
                "chat",
                &doc.export(loro::ExportMode::Snapshot).unwrap(),
                11,
                2,
            )
            .unwrap();
        old.execute("UPDATE snapshots SET cursor=12 WHERE doc_id='chat'", [])
            .unwrap();
        assert_eq!(store.load_chat_state("chat").unwrap().unwrap().cursor, 12);
        // Deleting a chat removes its deltas before the guarded base row.
        let vv = doc.oplog_vv();
        doc.get_map("more").insert("delta", 2).unwrap();
        doc.commit();
        store
            .append_snapshot_delta(
                "chat",
                &doc.export(loro::ExportMode::updates(&vv)).unwrap(),
                13,
                &DocsStore::snapshot_roots(&doc).unwrap(),
            )
            .unwrap();
        store.delete_snapshot("chat").unwrap();
        assert!(store.load_chat_state("chat").unwrap().is_none());
        let rows: i64 = old
            .query_row("SELECT count(*) FROM snapshot_deltas", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 0);
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

    fn pragma_i64(conn: &rusqlite::Connection, name: &str) -> i64 {
        conn.pragma_query_value(None, name, |row| row.get(0))
            .unwrap()
    }

    #[test]
    fn writer_and_reader_apply_cache_pragmas_without_changing_synchronous() {
        let dir = tempfile::tempdir().unwrap();
        let store = DocsStore::open(dir.path()).unwrap();
        let writer = store.conn();
        let reader = store.reader();
        for conn in [&*writer, &*reader] {
            assert_eq!(pragma_i64(conn, "temp_store"), 2, "MEMORY");
            assert_eq!(pragma_i64(conn, "cache_size"), CACHE_SIZE_KIB);
            assert_eq!(pragma_i64(conn, "mmap_size"), MMAP_SIZE_BYTES);
        }
        assert_eq!(pragma_i64(&writer, "synchronous"), 1, "NORMAL stays");
        let journal: String = writer
            .pragma_query_value(None, "journal_mode", |row| row.get(0))
            .unwrap();
        assert!(journal.eq_ignore_ascii_case("wal"));
        assert!(reader.is_readonly(rusqlite::DatabaseName::Main).unwrap());
        assert!(!writer.is_readonly(rusqlite::DatabaseName::Main).unwrap());
    }

    #[test]
    fn batched_outbox_insert_is_one_ordered_idempotent_transaction() {
        let dir = tempfile::tempdir().unwrap();
        let store = DocsStore::open(dir.path()).unwrap();
        let before = store.outbox_write_transactions();
        store
            .enqueue_chat_updates(&[
                ("a", "a1", b"1"),
                ("b", "b1", b"2"),
                ("a", "a2", b"3"),
                ("a", "a1", b"1"),
            ])
            .unwrap();
        assert_eq!(store.outbox_write_transactions() - before, 1);
        let ids = |doc| -> Vec<String> {
            store
                .pending_chat_updates(doc)
                .unwrap()
                .into_iter()
                .map(|(id, _)| id)
                .collect()
        };
        assert_eq!(ids("a"), vec!["a1", "a2"], "slice order, duplicate ignored");
        assert_eq!(ids("b"), vec!["b1"]);
        store.enqueue_chat_updates(&[]).unwrap();
        assert_eq!(store.outbox_write_transactions() - before, 1);
    }

    #[test]
    fn a_failed_batch_leaves_no_rows_and_no_failed_publication_mark() {
        let dir = tempfile::tempdir().unwrap();
        let store = DocsStore::open(dir.path()).unwrap();
        store.initialize_chat_outbox("good", &[]).unwrap();
        store.conn().execute_batch("CREATE TRIGGER fail_one BEFORE INSERT ON chat_outbox WHEN NEW.batch_id='poison' BEGIN SELECT RAISE(FAIL, 'injected'); END;").unwrap();
        assert!(
            store
                .enqueue_chat_updates(&[("good", "g1", b"1"), ("bad", "poison", b"2")])
                .is_err()
        );
        assert!(store.pending_chat_updates("good").unwrap().is_empty());
        // Not marked failed: a later snapshot of "good" keeps its marker.
        store.save_snapshot("good", b"snapshot").unwrap();
        assert!(store.chat_outbox_initialized("good").unwrap());
        // Row by row, only the poisoned doc is marked.
        store.enqueue_chat_update("good", "g1", b"1").unwrap();
        assert!(store.enqueue_chat_update("bad", "poison", b"2").is_err());
        assert_eq!(store.pending_chat_updates("good").unwrap().len(), 1);
    }

    #[test]
    fn snapshot_barriers_run_before_each_snapshot_write_and_unregister_on_drop() {
        let dir = tempfile::tempdir().unwrap();
        let store = DocsStore::open(dir.path()).unwrap();
        let calls = Arc::new(AtomicU64::new(0));
        let counter = calls.clone();
        let barrier: Arc<SnapshotBarrier> = Arc::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        });
        store.add_snapshot_barrier(&barrier);
        store.save_snapshot("a", b"1").unwrap();
        store.save_snapshot_with_cursor("a", b"2", 1, 2).unwrap();
        store
            .save_verified_snapshot_with_cursor("a", b"3", 1, 2)
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        drop(barrier);
        store.save_snapshot("a", b"4").unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn reader_sees_committed_writes_while_a_writer_transaction_is_open() {
        let dir = tempfile::tempdir().unwrap();
        let store = std::sync::Arc::new(DocsStore::open(dir.path()).unwrap());
        store
            .save_snapshot_with_cursor("chat", b"committed", 7, 2)
            .unwrap();
        store.mark_processed("cmd-1").unwrap();
        store.initialize_chat_outbox("chat", &[]).unwrap();
        store
            .enqueue_chat_update("chat", "batch", b"bytes")
            .unwrap();
        store.reject_chat_update("chat", "batch").unwrap();
        store.schedule_sync_job("chat", "wake").unwrap();

        let holder = store.clone();
        let (locked_tx, locked_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let (commit_tx, commit_rx) = std::sync::mpsc::channel::<()>();
        let holding = std::thread::spawn(move || {
            let mut conn = holder.conn();
            let tx = conn.transaction().unwrap();
            tx.execute(
                "INSERT INTO snapshots (doc_id, bytes, saved_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT(doc_id) DO UPDATE SET bytes = excluded.bytes, saved_at = excluded.saved_at",
                params!["chat", vec![9u8; 20 * 1024 * 1024], 1i64],
            )
            .unwrap();
            locked_tx.send(()).unwrap();
            release_rx.recv().unwrap();
            tx.commit().unwrap();
            commit_tx.send(()).unwrap();
        });
        locked_rx.recv().unwrap();

        let reader_store = store.clone();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let reading = std::thread::spawn(move || {
            let started = std::time::Instant::now();
            let snapshot = reader_store.load_snapshot("chat").unwrap();
            let with_cursor = reader_store.load_snapshot_with_cursor("chat").unwrap();
            let cursor = reader_store.snapshot_cursor("chat").unwrap();
            let epoch = reader_store.snapshot_epoch("chat").unwrap();
            let verified = reader_store.snapshot_cursor_verified("chat").unwrap();
            let has = reader_store.has_snapshot("chat").unwrap();
            let processed = reader_store.is_processed("cmd-1").unwrap();
            let missing = reader_store.is_processed("other").unwrap();
            let ledger = reader_store.processed_commands().unwrap();
            let outbox = reader_store.chat_outbox_initialized("chat").unwrap();
            let pending = reader_store.pending_chat_updates("chat").unwrap();
            let has_pending = reader_store.has_pending_chat_updates("chat").unwrap();
            let rejected = reader_store.rejected_chat_updates("chat").unwrap();
            let jobs = reader_store.pending_sync_jobs("wake", "", 10).unwrap();
            done_tx
                .send((
                    started.elapsed(),
                    snapshot,
                    with_cursor,
                    cursor,
                    epoch,
                    verified,
                    has,
                    processed,
                    missing,
                    ledger,
                    outbox,
                    pending,
                    has_pending,
                    rejected,
                    jobs,
                ))
                .unwrap();
        });
        let read = done_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .expect("reads blocked on the writer transaction");
        release_tx.send(()).unwrap();
        holding.join().unwrap();
        reading.join().unwrap();
        commit_rx.recv().unwrap();

        let (
            elapsed,
            snapshot,
            with_cursor,
            cursor,
            epoch,
            verified,
            has,
            processed,
            missing,
            ledger,
            outbox,
            pending,
            has_pending,
            rejected,
            jobs,
        ) = read;
        assert!(
            elapsed < std::time::Duration::from_secs(1),
            "read took {elapsed:?} behind a 20 MB open upsert"
        );
        assert_eq!(snapshot.as_deref(), Some(&b"committed"[..]));
        assert_eq!(with_cursor, Some((b"committed".to_vec(), 7, 2)));
        assert_eq!(cursor, 7);
        assert_eq!(epoch, 2);
        assert!(!verified);
        assert!(has);
        assert!(processed);
        assert!(!missing);
        assert!(ledger.iter().any(|(id, _)| id == "cmd-1"));
        assert!(outbox);
        assert_eq!(pending, vec![("batch".into(), b"bytes".to_vec())]);
        assert!(has_pending);
        assert_eq!(rejected, pending);
        assert_eq!(jobs, vec!["chat".to_string()]);

        let committed = store.load_snapshot("chat").unwrap().unwrap();
        assert_eq!(committed.len(), 20 * 1024 * 1024);
        assert!(committed.iter().all(|byte| *byte == 9));
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
