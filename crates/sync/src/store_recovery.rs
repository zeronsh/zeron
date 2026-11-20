//! A damaged `docs.sqlite3`: found at open, moved aside, rebuilt.
//!
//! The store holds warm copies of docs (the rooms are authoritative), the
//! command ledger, and outboxes. A Cloud machine's disk is snapshotted and
//! restored by its provider, and on staging one came back with pages from two
//! moments ("2nd reference to page"): every read of the chat failed, and the
//! message sent to it waited for good while the machine read "ready".
//!
//! So every open checks the file first (`PRAGMA quick_check`, cheap at the sizes
//! a store has). A file that fails is renamed aside, kept for diagnosis, a fresh
//! store is made, and every row that can still be read is copied over. What
//! isn't readable comes back from the rooms. A store that errored as corrupt
//! while running leaves a marker, so the next open looks harder
//! (`integrity_check`) even if the quick check passes.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use rusqlite::types::Value;
use rusqlite::{Connection, OpenFlags};

/// Corrupt copies kept beside the store, newest first; older ones go.
const KEEP_CORRUPT: usize = 3;
/// Past this size the open check is skipped unless a marker asks for it: a
/// quick check reads the whole file, and boot waits on it.
const CHECK_MAX_BYTES: u64 = 1024 * 1024 * 1024;

/// A store error said the database is corrupt, in this process.
static CORRUPTION_SEEN: AtomicBool = AtomicBool::new(false);

/// Whether a store operation has reported corruption since this process
/// started. The engine restarts (Cloud) or recovers on its next start.
pub fn corruption_seen() -> bool {
    CORRUPTION_SEEN.load(Ordering::SeqCst)
}

/// Note an error, flagging it when it says the file is damaged.
pub(crate) fn note_error(err: &rusqlite::Error) {
    if is_corruption(err) && !CORRUPTION_SEEN.swap(true, Ordering::SeqCst) {
        tracing::error!(%err, "docs store: the database reports corruption");
    }
}

pub(crate) fn is_corruption(err: &rusqlite::Error) -> bool {
    matches!(
        err.sqlite_error_code(),
        Some(rusqlite::ErrorCode::DatabaseCorrupt | rusqlite::ErrorCode::NotADatabase)
    )
}

/// The file beside the store that asks the next open for a full check.
pub fn marker_path(store: &Path) -> PathBuf {
    let mut name = store.as_os_str().to_owned();
    name.push(".check");
    name.into()
}

/// Ask the next open of the store at `store` for a full integrity check.
pub fn request_check(store: &Path) {
    let _ = std::fs::write(marker_path(store), b"corruption reported while running\n");
}

/// What an open found and did about a damaged store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recovery {
    /// What the check said.
    pub reason: String,
    /// Where the damaged file was kept.
    pub kept_at: PathBuf,
    /// Rows copied into the fresh store.
    pub salvaged_rows: usize,
    /// Tables some of whose rows couldn't be read.
    pub damaged_tables: Vec<String>,
}

/// `Some(reason)` when the store at `path` must not be used.
pub(crate) fn damage(path: &Path) -> Option<String> {
    let marker = marker_path(path);
    let asked = marker.exists();
    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    if !asked && size > CHECK_MAX_BYTES {
        tracing::info!(size, "docs store: too large to check at open; skipped");
        return None;
    }
    let verdict = (|| -> Result<Option<String>, rusqlite::Error> {
        let conn = Connection::open(path)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        let pragma = if asked { "PRAGMA integrity_check(20)" } else { "PRAGMA quick_check(20)" };
        let mut stmt = conn.prepare(pragma)?;
        let lines: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<_, _>>()?;
        Ok((lines != ["ok"]).then(|| lines.join("; ")))
    })();
    let found = match verdict {
        Ok(found) => found,
        Err(err) if is_corruption(&err) => Some(err.to_string()),
        // Busy, or a lock someone holds: not damage, and not ours to judge.
        Err(err) => {
            tracing::warn!(%err, "docs store: couldn't check the database at open");
            None
        }
    };
    if found.is_none() && asked {
        let _ = std::fs::remove_file(&marker);
    }
    found
}

/// Rename the store and its WAL and shared-memory files aside, as a pair SQLite
/// can still open: `docs-corrupt-<millis>.sqlite3` (`-wal`, `-shm`).
pub(crate) fn quarantine(path: &Path) -> std::io::Result<PathBuf> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let stamp = crate::store::now_millis();
    let kept = dir.join(format!("docs-corrupt-{stamp}.sqlite3"));
    for suffix in ["-wal", "-shm", ""] {
        let from = with_suffix(path, suffix);
        if from.exists() {
            std::fs::rename(&from, with_suffix(&kept, suffix))?;
        }
    }
    let _ = std::fs::remove_file(marker_path(path));
    prune_corrupt(dir);
    Ok(kept)
}

fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    name.into()
}

/// Keep the newest few corrupt copies: one is for diagnosis, a pile is a disk.
fn prune_corrupt(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut kept: Vec<(u64, String)> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let stamp = name.strip_prefix("docs-corrupt-")?.strip_suffix(".sqlite3")?;
            Some((stamp.parse().ok()?, name))
        })
        .collect();
    kept.sort_by(|a, b| b.0.cmp(&a.0));
    for (_, name) in kept.into_iter().skip(KEEP_CORRUPT) {
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(dir.join(format!("{name}{suffix}")));
        }
    }
}

/// Copy every readable row of every table the fresh store also has. A table is
/// read forwards until a damaged page stops it, then backwards from the end, so
/// rows on both sides of the damage are kept. Rows already copied are skipped.
pub(crate) fn salvage(from: &Path, into: &Connection) -> (usize, Vec<String>) {
    let Ok(old) = Connection::open_with_flags(from, OpenFlags::SQLITE_OPEN_READ_ONLY) else {
        return (0, vec!["(unreadable)".into()]);
    };
    let tables: Vec<String> = match old
        .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'")
        .and_then(|mut s| s.query_map([], |r| r.get::<_, String>(0))?.collect())
    {
        Ok(tables) => tables,
        Err(_) => return (0, vec!["(schema)".into()]),
    };
    let mut copied = 0;
    let mut damaged = Vec::new();
    for table in tables {
        // Bookkeeping the fresh store already has, freshly made.
        if table == "schema_migrations" {
            continue;
        }
        let columns = shared_columns(&old, into, &table);
        if columns.is_empty() {
            continue;
        }
        let list = columns.iter().map(|c| format!("\"{c}\"")).collect::<Vec<_>>().join(",");
        let marks = vec!["?"; columns.len()].join(",");
        let insert = format!("INSERT OR IGNORE INTO main.\"{table}\" ({list}) VALUES ({marks})");
        let mut whole = true;
        for order in ["ASC", "DESC"] {
            let select = format!("SELECT {list} FROM \"{table}\" ORDER BY rowid {order}");
            let (n, complete) = copy_rows(&old, into, &select, &insert, columns.len());
            copied += n;
            if complete {
                break;
            }
            whole = false;
        }
        if !whole {
            damaged.push(table);
        }
    }
    (copied, damaged)
}

fn shared_columns(old: &Connection, new: &Connection, table: &str) -> Vec<String> {
    let names = |conn: &Connection, schema: &str| -> Vec<String> {
        conn.prepare(&format!("PRAGMA {schema}.table_info(\"{table}\")"))
            .and_then(|mut s| s.query_map([], |r| r.get::<_, String>(1))?.collect())
            .unwrap_or_default()
    };
    let fresh = names(new, "main");
    names(old, "main").into_iter().filter(|c| fresh.contains(c)).collect()
}

/// Rows copied, and whether the read reached the end.
fn copy_rows(old: &Connection, new: &Connection, select: &str, insert: &str, width: usize) -> (usize, bool) {
    let Ok(mut read) = old.prepare(select) else {
        return (0, false);
    };
    let Ok(mut write) = new.prepare(insert) else {
        return (0, false);
    };
    let Ok(mut rows) = read.query([]) else {
        return (0, false);
    };
    let mut copied = 0;
    loop {
        match rows.next() {
            Ok(Some(row)) => {
                let values: Result<Vec<Value>, _> = (0..width).map(|i| row.get::<_, Value>(i)).collect();
                let Ok(values) = values else { return (copied, false) };
                if write.execute(rusqlite::params_from_iter(values)).is_ok_and(|n| n > 0) {
                    copied += 1;
                }
            }
            Ok(None) => return (copied, true),
            Err(_) => return (copied, false),
        }
    }
}
