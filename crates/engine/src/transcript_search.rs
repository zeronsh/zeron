//! Full-text search over transcripts stored on this engine.
//!
//! The index is derived data. `transcript-search.sqlite3` holds one FTS5 row
//! per chat and the snapshot `saved_at` it was built from. Refresh diffs the
//! index against `DocsStore` snapshots of the workspace's live chats, so
//! anything that writes a snapshot gets indexed with no hook. That covers live
//! persistence, chat2 seeding, and profile import. Deleting the file only
//! costs a rebuild.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use rusqlite::{Connection, params};
use zeron_doc::{MessagePart, MessageRole, SessionDoc};
use zeron_proto::TranscriptSearchHit;
use zeron_sync::DocsStore;

use crate::EngineError;
use crate::workspace_host::WorkspaceHost;

const SCHEMA_VERSION: i64 = 1;
const REFRESH_BUDGET: Duration = Duration::from_secs(1);
const SNIPPET_TOKENS: i64 = 16;

#[derive(Clone)]
pub struct TranscriptSearch {
    inner: Arc<Inner>,
}

struct Inner {
    index: Mutex<Connection>,
    refreshing: Mutex<()>,
    store: Arc<DocsStore>,
    workspace: WorkspaceHost,
}

impl TranscriptSearch {
    pub fn open(
        dir: &Path,
        store: Arc<DocsStore>,
        workspace: WorkspaceHost,
    ) -> Result<Self, EngineError> {
        let conn = Connection::open(dir.join("transcript-search.sqlite3")).map_err(sql)?;
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(sql)?;
        conn.pragma_update(None, "synchronous", "NORMAL")
            .map_err(sql)?;
        let version: i64 = conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .map_err(sql)?;
        if version != SCHEMA_VERSION {
            conn.execute_batch(&format!(
                "DROP TABLE IF EXISTS chats;
                 DROP TABLE IF EXISTS transcripts;
                 CREATE TABLE chats (
                    id       INTEGER PRIMARY KEY,
                    chat_id  TEXT NOT NULL UNIQUE,
                    saved_at INTEGER NOT NULL
                 ) STRICT;
                 CREATE VIRTUAL TABLE transcripts USING fts5(
                    body, tokenize = 'unicode61 remove_diacritics 2'
                 );
                 PRAGMA user_version = {SCHEMA_VERSION};"
            ))
            .map_err(sql)?;
        }
        Ok(Self {
            inner: Arc::new(Inner {
                index: Mutex::new(conn),
                refreshing: Mutex::new(()),
                store,
                workspace,
            }),
        })
    }

    /// An empty query only refreshes, so callers can warm the index.
    pub async fn search(
        &self,
        query: String,
        limit: u16,
    ) -> Result<Vec<TranscriptSearchHit>, EngineError> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            if let Err(error) = inner.refresh() {
                tracing::warn!(%error, "transcript index refresh failed");
            }
            match match_expression(&query) {
                Some(expr) => inner.query(&expr, limit),
                None => Ok(Vec::new()),
            }
        })
        .await
        .map_err(|e| EngineError::Other(e.to_string()))?
    }
}

impl Inner {
    /// Reindex chats whose snapshot changed, newest first, within
    /// [`REFRESH_BUDGET`]. A refresh already in flight makes this a no-op.
    fn refresh(&self) -> Result<(), EngineError> {
        let Ok(_refreshing) = self.refreshing.try_lock() else {
            return Ok(());
        };
        let started = Instant::now();
        let live: HashSet<String> = self
            .workspace
            .read_chats()?
            .into_iter()
            .map(|chat| chat.id)
            .collect();
        let mut wanted: Vec<(String, i64)> = self
            .store
            .snapshot_saved_at()?
            .into_iter()
            .filter(|(id, _)| live.contains(id))
            .collect();
        let indexed: HashMap<String, (i64, i64)> = {
            let conn = self.conn();
            let mut stmt = conn
                .prepare("SELECT chat_id, id, saved_at FROM chats")
                .map_err(sql)?;
            stmt.query_map([], |row| Ok((row.get(0)?, (row.get(1)?, row.get(2)?))))
                .map_err(sql)?
                .collect::<Result<_, _>>()
                .map_err(sql)?
        };
        let wanted_ids: HashSet<&str> = wanted.iter().map(|(id, _)| id.as_str()).collect();
        let gone: Vec<i64> = indexed
            .iter()
            .filter(|(chat_id, _)| !wanted_ids.contains(chat_id.as_str()))
            .map(|(_, (row, _))| *row)
            .collect();
        if !gone.is_empty() {
            let mut conn = self.conn();
            let tx = conn.transaction().map_err(sql)?;
            for row in gone {
                tx.execute("DELETE FROM transcripts WHERE rowid = ?1", params![row])
                    .map_err(sql)?;
                tx.execute("DELETE FROM chats WHERE id = ?1", params![row])
                    .map_err(sql)?;
            }
            tx.commit().map_err(sql)?;
        }
        wanted.retain(|(id, saved_at)| indexed.get(id).map(|(_, at)| at) != Some(saved_at));
        wanted.sort_by_key(|(_, saved_at)| std::cmp::Reverse(*saved_at));
        for (chat_id, saved_at) in wanted {
            if started.elapsed() > REFRESH_BUDGET {
                break;
            }
            let body = match self.store.load_snapshot(&chat_id) {
                Ok(Some(bytes)) => transcript_text(&bytes).unwrap_or_else(|error| {
                    tracing::warn!(chat = %chat_id, %error, "transcript unreadable; indexed empty");
                    String::new()
                }),
                Ok(None) => continue,
                Err(error) => return Err(error.into()),
            };
            let mut conn = self.conn();
            let tx = conn.transaction().map_err(sql)?;
            let row: i64 = tx
                .query_row(
                    "INSERT INTO chats (chat_id, saved_at) VALUES (?1, ?2)
                     ON CONFLICT(chat_id) DO UPDATE SET saved_at = excluded.saved_at
                     RETURNING id",
                    params![chat_id, saved_at],
                    |row| row.get(0),
                )
                .map_err(sql)?;
            tx.execute("DELETE FROM transcripts WHERE rowid = ?1", params![row])
                .map_err(sql)?;
            tx.execute(
                "INSERT INTO transcripts (rowid, body) VALUES (?1, ?2)",
                params![row, body],
            )
            .map_err(sql)?;
            tx.commit().map_err(sql)?;
        }
        Ok(())
    }

    fn query(&self, expr: &str, limit: u16) -> Result<Vec<TranscriptSearchHit>, EngineError> {
        let conn = self.conn();
        let mut stmt = conn
            .prepare_cached(
                "SELECT chats.chat_id, snippet(transcripts, 0, '', '', '…', ?3)
                 FROM transcripts JOIN chats ON chats.id = transcripts.rowid
                 WHERE transcripts MATCH ?1
                 ORDER BY rank
                 LIMIT ?2",
            )
            .map_err(sql)?;
        stmt.query_map(params![expr, limit, SNIPPET_TOKENS], |row| {
            Ok(TranscriptSearchHit {
                chat_id: row.get(0)?,
                snippet: row.get(1)?,
            })
        })
        .map_err(sql)?
        .collect::<Result<_, _>>()
        .map_err(sql)
    }

    fn conn(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.index.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// User and assistant text only. Reasoning is hidden in the UI, and tool
/// output would drown real matches.
fn transcript_text(snapshot: &[u8]) -> Result<String, EngineError> {
    let raw = loro::LoroDoc::new();
    raw.import(snapshot)
        .map_err(|e| EngineError::Other(e.to_string()))?;
    let entries = SessionDoc::from_doc(raw).read_entries()?;
    let mut body = String::new();
    for entry in entries {
        if !matches!(entry.role, MessageRole::User | MessageRole::Assistant) {
            continue;
        }
        for part in entry.parts {
            if let MessagePart::Text { text, .. } = part {
                body.push_str(&text);
                body.push('\n');
            }
        }
    }
    Ok(body)
}

/// Every word must appear, each as a quoted prefix so FTS5 operators in user
/// text stay literal. Words without a letter or digit have no tokens to match.
fn match_expression(query: &str) -> Option<String> {
    let terms: Vec<String> = query
        .split_whitespace()
        .filter(|word| word.chars().any(char::is_alphanumeric))
        .map(|word| format!("\"{}\"*", word.replace('"', "\"\"")))
        .collect();
    (!terms.is_empty()).then(|| terms.join(" "))
}

fn sql(error: rusqlite::Error) -> EngineError {
    EngineError::Other(format!("transcript index: {error}"))
}
