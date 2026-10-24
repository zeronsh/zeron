//! chat3 transcript plane, host side of the local store (docs/chat3-sync.md):
//! which rows this host already stored, the outbox of rows not yet acked, and
//! full tool outputs awaiting upload.

use std::collections::HashMap;

use rusqlite::{OptionalExtension, params};

use crate::store::{DocsStore, StoreError};

/// One outbox row: `(ord, idx)` key, entry id, item JSON, content hash.
#[derive(Debug, Clone, PartialEq)]
pub struct OutboxItem {
    pub ord: f64,
    pub idx: i64,
    pub entry: String,
    pub bytes: Vec<u8>,
    pub hash: u64,
}

impl DocsStore {
    /// Hashes of the rows this host has stored for `doc_id`, keyed like
    /// `transcript_plane::ItemKey::bits`.
    pub fn chat3_published(&self, doc_id: &str) -> Result<HashMap<(u64, i64), u64>, StoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare("SELECT ord, idx, hash FROM chat3_items WHERE doc_id = ?1")?;
        let rows = stmt.query_map(params![doc_id], |row| {
            let ord: f64 = row.get(0)?;
            let idx: i64 = row.get(1)?;
            let hash: i64 = row.get(2)?;
            Ok(((ord.to_bits(), idx), hash as u64))
        })?;
        let mut out = HashMap::new();
        for row in rows {
            let (key, hash) = row?;
            out.insert(key, hash);
        }
        Ok(out)
    }

    /// Record rows as published and queue them for upload, in one
    /// transaction: a crash can't leave a row marked stored but never sent.
    pub fn chat3_publish(&self, doc_id: &str, items: &[OutboxItem]) -> Result<(), StoreError> {
        if items.is_empty() {
            return Ok(());
        }
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        {
            let mut mark = tx.prepare(
                "INSERT INTO chat3_items (doc_id, ord, idx, hash) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(doc_id, ord, idx) DO UPDATE SET hash = excluded.hash",
            )?;
            let mut queue = tx.prepare(
                "INSERT INTO chat3_outbox (doc_id, ord, idx, entry, bytes, hash) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(doc_id, ord, idx) DO UPDATE SET entry = excluded.entry, bytes = excluded.bytes, hash = excluded.hash",
            )?;
            for item in items {
                mark.execute(params![doc_id, item.ord, item.idx, item.hash as i64])?;
                queue.execute(params![doc_id, item.ord, item.idx, item.entry, item.bytes, item.hash as i64])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Record rows as stored without queueing them (a host adopting rows an
    /// earlier host already put in the room).
    pub fn chat3_mark_published(&self, doc_id: &str, rows: &[(f64, i64, u64)]) -> Result<(), StoreError> {
        if rows.is_empty() {
            return Ok(());
        }
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        {
            let mut mark = tx.prepare(
                "INSERT INTO chat3_items (doc_id, ord, idx, hash) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(doc_id, ord, idx) DO UPDATE SET hash = excluded.hash",
            )?;
            for (ord, idx, hash) in rows {
                mark.execute(params![doc_id, ord, idx, *hash as i64])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// The oldest queued rows (key order: headers before their parts), up to
    /// `max_bytes` of payload but always at least one.
    pub fn chat3_pending(&self, doc_id: &str, max_bytes: usize) -> Result<Vec<OutboxItem>, StoreError> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT ord, idx, entry, bytes, hash FROM chat3_outbox WHERE doc_id = ?1 ORDER BY ord, idx",
        )?;
        let rows = stmt.query_map(params![doc_id], |row| {
            Ok(OutboxItem {
                ord: row.get(0)?,
                idx: row.get(1)?,
                entry: row.get(2)?,
                bytes: row.get(3)?,
                hash: row.get::<_, i64>(4)? as u64,
            })
        })?;
        let mut out = Vec::new();
        let mut total = 0usize;
        for row in rows {
            let item = row?;
            total += item.bytes.len();
            if !out.is_empty() && total > max_bytes {
                break;
            }
            out.push(item);
        }
        Ok(out)
    }

    /// Drop acked rows — only if the queued bytes are still the ones sent
    /// (a row rewritten while its upload was in flight stays queued).
    pub fn chat3_ack(&self, doc_id: &str, acked: &[(f64, i64, u64)]) -> Result<(), StoreError> {
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        {
            let mut stmt =
                tx.prepare("DELETE FROM chat3_outbox WHERE doc_id = ?1 AND ord = ?2 AND idx = ?3 AND hash = ?4")?;
            for (ord, idx, hash) in acked {
                stmt.execute(params![doc_id, ord, idx, *hash as i64])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn chat3_outbox_len(&self, doc_id: &str) -> Result<usize, StoreError> {
        let n: i64 = self.conn().query_row(
            "SELECT COUNT(*) FROM chat3_outbox WHERE doc_id = ?1",
            params![doc_id],
            |row| row.get(0),
        )?;
        Ok(n as usize)
    }

    /// Queue a full tool output (replacing any earlier one for the part).
    pub fn chat3_enqueue_output(&self, doc_id: &str, part: &str, bytes: &[u8]) -> Result<(), StoreError> {
        self.conn().execute(
            "INSERT INTO chat3_outputs (doc_id, part, bytes) VALUES (?1, ?2, ?3)
             ON CONFLICT(doc_id, part) DO UPDATE SET bytes = excluded.bytes",
            params![doc_id, part, bytes],
        )?;
        Ok(())
    }

    /// One queued output, oldest part id first.
    pub fn chat3_next_output(&self, doc_id: &str) -> Result<Option<(String, Vec<u8>)>, StoreError> {
        Ok(self
            .conn()
            .query_row(
                "SELECT part, bytes FROM chat3_outputs WHERE doc_id = ?1 ORDER BY part LIMIT 1",
                params![doc_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?)
    }

    /// A queued (not yet uploaded) output for one part.
    pub fn chat3_next_output_for(&self, doc_id: &str, part: &str) -> Result<Option<Vec<u8>>, StoreError> {
        Ok(self
            .conn()
            .query_row(
                "SELECT bytes FROM chat3_outputs WHERE doc_id = ?1 AND part = ?2",
                params![doc_id, part],
                |row| row.get(0),
            )
            .optional()?)
    }

    /// Drop an uploaded output if it is still the bytes that were sent.
    pub fn chat3_ack_output(&self, doc_id: &str, part: &str, len: usize) -> Result<(), StoreError> {
        self.conn().execute(
            "DELETE FROM chat3_outputs WHERE doc_id = ?1 AND part = ?2 AND LENGTH(bytes) = ?3",
            params![doc_id, part, len as i64],
        )?;
        Ok(())
    }

    /// Every chat3 table row for a doc (purge on chat delete).
    pub fn chat3_purge(&self, doc_id: &str) -> Result<(), StoreError> {
        let conn = self.conn();
        conn.execute("DELETE FROM chat3_items WHERE doc_id = ?1", params![doc_id])?;
        conn.execute("DELETE FROM chat3_outbox WHERE doc_id = ?1", params![doc_id])?;
        conn.execute("DELETE FROM chat3_outputs WHERE doc_id = ?1", params![doc_id])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(ord: f64, idx: i64, body: &str) -> OutboxItem {
        OutboxItem {
            ord,
            idx,
            entry: "e".into(),
            bytes: body.as_bytes().to_vec(),
            hash: (body.len() as u64 * 31).wrapping_add(idx as u64),
        }
    }

    #[test]
    fn outbox_coalesces_per_key_and_acks_only_what_was_sent() {
        let dir = tempfile::tempdir().unwrap();
        let store = DocsStore::open(dir.path()).unwrap();
        store.chat3_publish("c", &[item(1.0, -1, "h"), item(1.0, 0, "a")]).unwrap();
        store.chat3_publish("c", &[item(1.0, 0, "ab")]).unwrap();
        let pending = store.chat3_pending("c", 1 << 20).unwrap();
        assert_eq!(pending.iter().map(|i| (i.idx, i.bytes.clone())).collect::<Vec<_>>(), [
            (-1, b"h".to_vec()),
            (0, b"ab".to_vec())
        ]);
        // The part is rewritten while its upload is in flight: the stale ack
        // must not drop the newer bytes.
        let sent = pending[1].clone();
        store.chat3_publish("c", &[item(1.0, 0, "abc")]).unwrap();
        store
            .chat3_ack("c", &[(1.0, -1, pending[0].hash), (sent.ord, sent.idx, sent.hash)])
            .unwrap();
        let left = store.chat3_pending("c", 1 << 20).unwrap();
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].bytes, b"abc".to_vec());
        // Published hashes survive acks (they are what a reopened host skips).
        assert_eq!(store.chat3_published("c").unwrap().len(), 2);
    }

    #[test]
    fn pending_batches_respect_the_byte_budget_but_always_move() {
        let dir = tempfile::tempdir().unwrap();
        let store = DocsStore::open(dir.path()).unwrap();
        let big = "x".repeat(1000);
        store.chat3_publish("c", &[item(1.0, 0, &big), item(1.0, 1, &big), item(1.0, 2, &big)]).unwrap();
        assert_eq!(store.chat3_pending("c", 1500).unwrap().len(), 1);
        assert_eq!(store.chat3_pending("c", 10).unwrap().len(), 1);
        assert_eq!(store.chat3_pending("c", 5000).unwrap().len(), 3);
    }

    #[test]
    fn outputs_queue_and_ack_by_length() {
        let dir = tempfile::tempdir().unwrap();
        let store = DocsStore::open(dir.path()).unwrap();
        store.chat3_enqueue_output("c", "toolu_1", b"hello").unwrap();
        let (part, bytes) = store.chat3_next_output("c").unwrap().unwrap();
        store.chat3_enqueue_output("c", "toolu_1", b"hello world").unwrap();
        store.chat3_ack_output("c", &part, bytes.len()).unwrap();
        assert!(store.chat3_next_output("c").unwrap().is_some(), "newer bytes stay queued");
        store.chat3_ack_output("c", "toolu_1", 11).unwrap();
        assert!(store.chat3_next_output("c").unwrap().is_none());
    }
}
