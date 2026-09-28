//! `DocsStore` — local SQLite persistence for doc snapshots and the
//! processed-command ledger (ARCHITECTURE §2 command plane: entries are marked
//! processed BEFORE execution so a crash can never double-execute a command).

use std::collections::HashSet;
use std::path::Path;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, OptionalExtension, params};

/// Errors surfaced by [`DocsStore`].
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
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
];

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
}

impl DocsStore {
    /// Open (creating directory, database, and schema as needed).
    pub fn open(data_dir: impl AsRef<Path>) -> Result<Self, StoreError> {
        let data_dir = data_dir.as_ref();
        std::fs::create_dir_all(data_dir)?;
        let mut conn = Connection::open(data_dir.join("docs.sqlite3"))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        migrate(&mut conn)?;
        ensure_child_notifications(&conn)?;
        Ok(Self {
            conn: Mutex::new(conn),
            failed_publications: Mutex::new(HashSet::new()),
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
        tx.commit()?;
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
        tx.commit()?;
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
        tx.commit()?;
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
            "DELETE FROM chat_sync_jobs WHERE doc_id = ?1",
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

    /// One child-notifier ledger row.
    pub fn child_notification(
        &self,
        child_chat_id: &str,
        turn_key: &str,
    ) -> Result<Option<ChildNotification>, StoreError> {
        store_blocking(|| {
            Ok(self
                .conn()
                .query_row(
                    "SELECT child_chat_id, turn_key, parent_chat_id, outcome, state
                     FROM child_notifications WHERE child_chat_id=?1 AND turn_key=?2",
                    params![child_chat_id, turn_key],
                    |row| {
                        Ok(ChildNotification {
                            child_chat_id: row.get(0)?,
                            turn_key: row.get(1)?,
                            parent_chat_id: row.get(2)?,
                            outcome: row.get(3)?,
                            state: row.get(4)?,
                        })
                    },
                )
                .optional()?)
        })
    }

    /// Claim `(child_chat_id, turn_key)` for delivery — call BEFORE the card
    /// write or prompt send (same ledger rule as `mark_processed`: a crash
    /// mid-delivery must not double-notify the parent). `true` when this call
    /// claimed the key; `false` when a prior claim — of ANY state — already
    /// owns it (acked rows win: a `zeron chat wait` ack that beat the claim
    /// suppresses the notification entirely).
    pub fn claim_child_notification(
        &self,
        child_chat_id: &str,
        turn_key: &str,
        parent_chat_id: &str,
        outcome: &str,
    ) -> Result<bool, StoreError> {
        store_blocking(|| {
            Ok(self.conn().execute(
                "INSERT OR IGNORE INTO child_notifications
                     (child_chat_id, turn_key, parent_chat_id, outcome, state, created_at)
                 VALUES (?1, ?2, ?3, ?4, 'pending', ?5)",
                params![child_chat_id, turn_key, parent_chat_id, outcome, now_ms()],
            )? > 0)
        })
    }

    /// Batched claim for a detect pass: every `INSERT OR IGNORE` plus each
    /// claimed child's pending-key supersede lands in ONE transaction, so a
    /// burst of sibling settles costs one SQLite write round-trip. Returns
    /// per-claim "newly claimed" flags in input order.
    pub fn claim_child_notifications(
        &self,
        claims: &[ChildNotificationClaim<'_>],
    ) -> Result<Vec<bool>, StoreError> {
        store_blocking(|| {
            let mut conn = self.conn();
            let tx = conn.transaction()?;
            let mut claimed = Vec::with_capacity(claims.len());
            for claim in claims {
                let inserted = tx.execute(
                    "INSERT OR IGNORE INTO child_notifications
                         (child_chat_id, turn_key, parent_chat_id, outcome, state, created_at)
                     VALUES (?1, ?2, ?3, ?4, 'pending', ?5)",
                    params![
                        claim.child_chat_id,
                        claim.turn_key,
                        claim.parent_chat_id,
                        claim.outcome,
                        now_ms()
                    ],
                )? > 0;
                claimed.push(inserted);
                if inserted {
                    // Newest pending wins: older unclaimed-by-the-parent keys
                    // for this child are folded into this update.
                    tx.execute(
                        "UPDATE child_notifications SET state='superseded'
                         WHERE child_chat_id=?1 AND turn_key!=?2 AND state='pending'",
                        params![claim.child_chat_id, claim.turn_key],
                    )?;
                }
            }
            tx.commit()?;
            Ok(claimed)
        })
    }

    /// Batched terminal transition for a flush: one transaction settles every
    /// delivered (or dropped) row the prompt carried. `delivered` stamps
    /// `delivered_at`; rows already in another terminal state (an ack that
    /// beat the flush) are left alone.
    pub fn settle_child_notifications(
        &self,
        keys: &[(&str, &str)],
        state: &str,
    ) -> Result<(), StoreError> {
        store_blocking(|| {
            let mut conn = self.conn();
            let tx = conn.transaction()?;
            for (child_chat_id, turn_key) in keys {
                tx.execute(
                    "UPDATE child_notifications
                     SET state=?3, delivered_at = CASE WHEN ?3='delivered' THEN ?4 ELSE delivered_at END
                     WHERE child_chat_id=?1 AND turn_key=?2 AND state='pending'",
                    params![child_chat_id, turn_key, state, now_ms()],
                )?;
            }
            tx.commit()?;
            Ok(())
        })
    }

    /// Coalescing rule: per child only the NEWEST pending update is owed to
    /// the parent — every other still-pending key for the child supersedes.
    pub fn supersede_pending_child_notifications(
        &self,
        child_chat_id: &str,
        keep_turn_key: &str,
    ) -> Result<(), StoreError> {
        store_blocking(|| {
            self.conn().execute(
                "UPDATE child_notifications SET state='superseded'
                 WHERE child_chat_id=?1 AND turn_key!=?2 AND state='pending'",
                params![child_chat_id, keep_turn_key],
            )?;
            Ok(())
        })
    }

    /// Every ledger row for a child, any state (tests, diagnostics).
    pub fn child_notifications_for(
        &self,
        child_chat_id: &str,
    ) -> Result<Vec<ChildNotification>, StoreError> {
        store_blocking(|| {
            let conn = self.conn();
            let mut stmt = conn.prepare(
                "SELECT child_chat_id, turn_key, parent_chat_id, outcome, state
                 FROM child_notifications WHERE child_chat_id=?1 ORDER BY created_at, rowid",
            )?;
            Ok(stmt
                .query_map(params![child_chat_id], |row| {
                    Ok(ChildNotification {
                        child_chat_id: row.get(0)?,
                        turn_key: row.get(1)?,
                        parent_chat_id: row.get(2)?,
                        outcome: row.get(3)?,
                        state: row.get(4)?,
                    })
                })?
                .collect::<Result<_, _>>()?)
        })
    }

    /// Still-owed updates for one parent, oldest claim first so the combined
    /// prompt reads in settle order.
    pub fn pending_child_notifications(
        &self,
        parent_chat_id: &str,
    ) -> Result<Vec<ChildNotification>, StoreError> {
        store_blocking(|| {
            let conn = self.conn();
            let mut stmt = conn.prepare(
                "SELECT child_chat_id, turn_key, parent_chat_id, outcome, state
                 FROM child_notifications
                 WHERE parent_chat_id=?1 AND state='pending' ORDER BY created_at, rowid",
            )?;
            Ok(stmt
                .query_map(params![parent_chat_id], |row| {
                    Ok(ChildNotification {
                        child_chat_id: row.get(0)?,
                        turn_key: row.get(1)?,
                        parent_chat_id: row.get(2)?,
                        outcome: row.get(3)?,
                        state: row.get(4)?,
                    })
                })?
                .collect::<Result<_, _>>()?)
        })
    }

    /// Parents with any still-pending rows — the restart re-arm set.
    pub fn pending_child_notification_parents(&self) -> Result<Vec<String>, StoreError> {
        store_blocking(|| {
            let conn = self.conn();
            let mut stmt = conn.prepare(
                "SELECT DISTINCT parent_chat_id FROM child_notifications WHERE state='pending'",
            )?;
            Ok(stmt
                .query_map([], |row| row.get(0))?
                .collect::<Result<_, _>>()?)
        })
    }

    /// Terminal-state transition for one claimed row (`delivered`, `dropped`,
    /// `acked`, `superseded`). `delivered` also stamps `delivered_at`.
    /// No-op for rows already in another terminal state — an ack landing
    /// between claim and flush wins.
    pub fn settle_child_notification(
        &self,
        child_chat_id: &str,
        turn_key: &str,
        state: &str,
    ) -> Result<(), StoreError> {
        store_blocking(|| {
            self.conn().execute(
                "UPDATE child_notifications
                 SET state=?3, delivered_at = CASE WHEN ?3='delivered' THEN ?4 ELSE delivered_at END
                 WHERE child_chat_id=?1 AND turn_key=?2 AND state='pending'",
                params![child_chat_id, turn_key, state, now_ms()],
            )?;
            Ok(())
        })
    }

    /// A consumer ack (`zeron chat wait`/`output` inside the parent) for a key
    /// the notifier may not have claimed yet: insert the row ALREADY `acked`
    /// so the later claim's INSERT OR IGNORE no-ops. Returns `false` when the
    /// row already exists under a DIFFERENT parent — such acks are rejected.
    pub fn ack_child_notification(
        &self,
        child_chat_id: &str,
        turn_key: &str,
        parent_chat_id: &str,
    ) -> Result<bool, StoreError> {
        store_blocking(|| {
            // NB: bind before the update — `if let` keeps the `conn()` guard
            // alive for its whole block, and a nested `conn()` would
            // deadlock the non-reentrant mutex.
            let existing_parent: Option<String> = self
                .conn()
                .query_row(
                    "SELECT parent_chat_id FROM child_notifications
                     WHERE child_chat_id=?1 AND turn_key=?2",
                    params![child_chat_id, turn_key],
                    |row| row.get::<_, String>(0),
                )
                .optional()?;
            if let Some(existing_parent) = existing_parent {
                if existing_parent != parent_chat_id {
                    return Ok(false);
                }
                self.conn().execute(
                    "UPDATE child_notifications SET state='acked'
                     WHERE child_chat_id=?1 AND turn_key=?2 AND state='pending'",
                    params![child_chat_id, turn_key],
                )?;
                return Ok(true);
            }
            self.conn().execute(
                "INSERT INTO child_notifications
                     (child_chat_id, turn_key, parent_chat_id, state, created_at)
                 VALUES (?1, ?2, ?3, 'acked', ?4)",
                params![child_chat_id, turn_key, parent_chat_id, now_ms()],
            )?;
            Ok(true)
        })
    }

    /// Every still-pending row for an archived parent: `dropped`. Sending
    /// would unarchive the chat, which is exactly what archiving was meant
    /// to stop.
    pub fn drop_pending_child_notifications(&self, parent_chat_id: &str) -> Result<(), StoreError> {
        store_blocking(|| {
            self.conn().execute(
                "UPDATE child_notifications SET state='dropped'
                 WHERE parent_chat_id=?1 AND state='pending'",
                params![parent_chat_id],
            )?;
            Ok(())
        })
    }

    pub(crate) fn conn(&self) -> MutexGuard<'_, Connection> {
        // A poisoned lock only means another thread panicked mid-query; the
        // connection itself is still usable.
        self.conn.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// One claim in a batched [`DocsStore::claim_child_notifications`] pass.
pub struct ChildNotificationClaim<'a> {
    pub child_chat_id: &'a str,
    pub turn_key: &'a str,
    pub parent_chat_id: &'a str,
    pub outcome: &'a str,
}

/// One row of the child-notification ledger: a settle transition
/// detected on an agent-spawned child, owed to its parent chat.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildNotification {
    pub child_chat_id: String,
    /// The stable dedupe key from `child_update` (`done:<turn>`,
    /// `error:<ms>`, `input:<ms>`, `stopped:<ms>`).
    pub turn_key: String,
    pub parent_chat_id: String,
    /// `ChildOutcome`'s camelCase wire name, kept for rebuilds and debugging.
    pub outcome: Option<String>,
    /// `pending` | `delivered` | `acked` | `superseded` | `dropped`.
    pub state: String,
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

/// Agent-spawned child settle notifications: claim-before-send ledger so a
/// transition detected on the child's session row reaches the parent
/// exactly once, even across engine restarts. `state` flows
/// pending -> delivered|dropped|superseded|acked (acked outranks
/// everything: `zeron chat wait`/`output` saw the turn before we sent).
///
/// Deliberately outside `MIGRATIONS`: upstream owns the numbered list and
/// may add its own next entry, which a database that recorded this table
/// as a numbered migration would then skip. `IF NOT EXISTS` also keeps
/// databases that already hold the table (or a stale higher version row
/// from an earlier build) working unchanged.
fn ensure_child_notifications(conn: &Connection) -> Result<(), StoreError> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS child_notifications (
            child_chat_id  TEXT NOT NULL,
            turn_key       TEXT NOT NULL,
            parent_chat_id TEXT NOT NULL,
            outcome        TEXT,
            state          TEXT NOT NULL CHECK(state IN ('pending','delivered','acked','superseded','dropped')),
            created_at     INTEGER NOT NULL,
            delivered_at   INTEGER,
            PRIMARY KEY(child_chat_id, turn_key)
        ) STRICT;",
    )?;
    Ok(())
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
    fn child_notification_ledger_claims_coalesces_and_acks() {
        let dir = tempfile::tempdir().unwrap();
        let store = DocsStore::open(dir.path()).unwrap();

        // First claim wins; repeats are deduped by the (child, key) PK.
        assert!(
            store
                .claim_child_notification("c1", "done:t1", "p", "completed")
                .unwrap()
        );
        assert!(
            !store
                .claim_child_notification("c1", "done:t1", "p", "completed")
                .unwrap()
        );
        // A second pending key for the same child supersedes the first.
        assert!(
            store
                .claim_child_notification("c1", "done:t2", "p", "completed")
                .unwrap()
        );
        store
            .supersede_pending_child_notifications("c1", "done:t2")
            .unwrap();
        let pending = store.pending_child_notifications("p").unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].turn_key, "done:t2");
        assert_eq!(pending[0].outcome.as_deref(), Some("completed"));
        assert_eq!(
            store
                .child_notification("c1", "done:t1")
                .unwrap()
                .unwrap()
                .state,
            "superseded"
        );
        // Parents with pending rows are the restart re-arm set.
        assert_eq!(
            store.pending_child_notification_parents().unwrap(),
            vec!["p".to_string()]
        );
        // Delivery is a terminal transition; a second settle no-ops.
        store
            .settle_child_notification("c1", "done:t2", "delivered")
            .unwrap();
        store
            .settle_child_notification("c1", "done:t2", "acked")
            .unwrap();
        assert_eq!(
            store
                .child_notification("c1", "done:t2")
                .unwrap()
                .unwrap()
                .state,
            "delivered"
        );
        assert!(store.pending_child_notifications("p").unwrap().is_empty());
        assert!(
            store
                .pending_child_notification_parents()
                .unwrap()
                .is_empty()
        );
        // An ack for an unclaimed key inserts the row already acked so a
        // later claim is ignored; a mismatched parent is rejected.
        assert!(store.ack_child_notification("c2", "done:t9", "p").unwrap());
        assert!(
            !store
                .claim_child_notification("c2", "done:t9", "p", "completed")
                .unwrap()
        );
        assert!(
            !store
                .ack_child_notification("c2", "done:t9", "other")
                .unwrap()
        );
        assert_eq!(
            store
                .child_notification("c2", "done:t9")
                .unwrap()
                .unwrap()
                .state,
            "acked"
        );
        // Archived parents drop their pending rows.
        assert!(
            store
                .claim_child_notification("c3", "error:1", "p2", "errored")
                .unwrap()
        );
        store.drop_pending_child_notifications("p2").unwrap();
        assert_eq!(
            store
                .child_notification("c3", "error:1")
                .unwrap()
                .unwrap()
                .state,
            "dropped"
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

    #[test]
    fn fresh_store_has_child_ledger_and_exactly_upstream_migrations() {
        let dir = tempfile::tempdir().unwrap();
        let store = DocsStore::open(dir.path()).unwrap();
        {
            let conn = store.conn();
            // Upstream owns the numbered list: exactly its six entries.
            let applied: i64 = conn
                .query_row("SELECT COUNT(*) FROM schema_migrations", [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(applied, 6);
            let ledger: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master \
                     WHERE type='table' AND name='child_notifications'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(ledger, 1);
        }
        assert!(
            store
                .claim_child_notification("c1", "done:t1", "p", "completed")
                .unwrap()
        );
    }

    #[test]
    fn stray_version_seven_keeps_ledger_working() {
        let dir = tempfile::tempdir().unwrap();
        {
            // A database an earlier build migrated past upstream's list:
            // the stray version row must be left alone — upstream's next
            // release may claim it — while the ledger table keeps working.
            let store = DocsStore::open(dir.path()).unwrap();
            store
                .conn()
                .execute(
                    "INSERT INTO schema_migrations (version, applied_at) VALUES (7, 0)",
                    [],
                )
                .unwrap();
        }
        let store = DocsStore::open(dir.path()).unwrap();
        {
            let conn = store.conn();
            let ledger: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master \
                     WHERE type='table' AND name='child_notifications'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(ledger, 1);
            let version_seven: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM schema_migrations WHERE version = 7",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(version_seven, 1);
        }
        assert!(
            store
                .claim_child_notification("c1", "done:t1", "p", "completed")
                .unwrap()
        );
        assert!(
            !store
                .claim_child_notification("c1", "done:t1", "p", "completed")
                .unwrap()
        );
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
