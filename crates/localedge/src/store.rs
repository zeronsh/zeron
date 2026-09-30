//! The edge's durable state: one SQLite file standing in for every Durable
//! Object's storage. Tables mirror the DOs' own schemas (chat-log.ts,
//! registry-room.ts, device-nudges.ts) with the room name folded into the
//! key, since one database holds every room.
//!
//! SQLite's value cap here is ~1 GB, not the DO's ~2 MB, so blobs are stored
//! whole instead of chunked (edge/src/blobs.ts).

use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};

pub(crate) const DB_FILE: &str = "edge.db";

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS chat_rows (
    chat_id TEXT NOT NULL,
    seq INTEGER NOT NULL,
    device TEXT NOT NULL,
    batch_id TEXT NOT NULL,
    bytes BLOB NOT NULL,
    received_at INTEGER NOT NULL,
    PRIMARY KEY (chat_id, seq),
    UNIQUE (chat_id, batch_id)
);
CREATE TABLE IF NOT EXISTS chat_meta (
    chat_id TEXT NOT NULL,
    key TEXT NOT NULL,
    value TEXT NOT NULL,
    PRIMARY KEY (chat_id, key)
);
CREATE TABLE IF NOT EXISTS chat_blobs (
    chat_id TEXT NOT NULL,
    name TEXT NOT NULL,
    bytes BLOB NOT NULL,
    PRIMARY KEY (chat_id, name)
);
CREATE TABLE IF NOT EXISTS registry_rows (
    kind TEXT NOT NULL,
    id TEXT NOT NULL,
    seq INTEGER NOT NULL,
    deleted INTEGER NOT NULL,
    del_hlc TEXT,
    fields TEXT NOT NULL,
    clocks TEXT NOT NULL,
    PRIMARY KEY (kind, id)
);
CREATE INDEX IF NOT EXISTS registry_rows_seq ON registry_rows (seq);
CREATE TABLE IF NOT EXISTS registry_meta (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS device_meta (
    device_id TEXT NOT NULL,
    key TEXT NOT NULL,
    value TEXT NOT NULL,
    PRIMARY KEY (device_id, key)
);
CREATE TABLE IF NOT EXISTS device_sidecars (
    device_id TEXT NOT NULL,
    name TEXT NOT NULL,
    json TEXT NOT NULL,
    PRIMARY KEY (device_id, name)
);
CREATE TABLE IF NOT EXISTS pending_nudges (
    device_id TEXT NOT NULL,
    chat_id TEXT NOT NULL,
    queued_at INTEGER NOT NULL,
    token TEXT NOT NULL,
    PRIMARY KEY (device_id, chat_id)
);
CREATE TABLE IF NOT EXISTS nudge_reconcile (
    device_id TEXT PRIMARY KEY,
    token TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS blobs (
    key TEXT PRIMARY KEY,
    content_type TEXT NOT NULL,
    bytes BLOB NOT NULL
);
";

pub(crate) fn open(dir: &Path) -> rusqlite::Result<Connection> {
    let db = Connection::open(dir.join(DB_FILE))?;
    // WAL + NORMAL: a crash can lose the last commits, never corrupt the
    // file. Loro re-import and registry LWW make every client re-push safe,
    // which is the same bargain the DO's write coalescing makes.
    db.pragma_update(None, "journal_mode", "WAL")?;
    db.pragma_update(None, "synchronous", "NORMAL")?;
    db.busy_timeout(std::time::Duration::from_secs(5))?;
    db.execute_batch(SCHEMA)?;
    Ok(db)
}

/// Named-blob storage for the `/blob` sidecar keys and the legacy `/diff`
/// slot — the R2 bucket's role, keyed exactly like the Worker's keys.
pub(crate) fn put_blob(
    db: &Connection,
    key: &str,
    content_type: &str,
    bytes: &[u8],
) -> rusqlite::Result<()> {
    db.execute(
        "INSERT INTO blobs (key, content_type, bytes) VALUES (?1, ?2, ?3)
         ON CONFLICT(key) DO UPDATE SET content_type = excluded.content_type, bytes = excluded.bytes",
        params![key, content_type, bytes],
    )?;
    Ok(())
}

pub(crate) fn get_blob(db: &Connection, key: &str) -> rusqlite::Result<Option<(String, Vec<u8>)>> {
    db.query_row(
        "SELECT content_type, bytes FROM blobs WHERE key = ?1",
        params![key],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .optional()
}
