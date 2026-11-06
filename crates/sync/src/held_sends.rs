//! The durable outbox for sends waiting on attachment bytes
//! (attachment_store.rs): a row exists from the moment the user sends until
//! the message is in the chat, across restarts. Rows hold metadata only; the
//! bytes stay in the sender's own staging area.

use rusqlite::{OptionalExtension, params};

use crate::attachment_store::{HeldPayload, HeldSend};
use crate::store::{DocsStore, StoreError, store_blocking};

fn encode(payload: &HeldPayload) -> Result<String, StoreError> {
    serde_json::to_string(payload)
        .map_err(|err| StoreError::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, err)))
}

type RawRow = (String, String, String, i64, Option<String>);

fn read_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RawRow> {
    Ok((
        row.get(0)?,
        row.get(1)?,
        row.get(2)?,
        row.get(3)?,
        row.get(4)?,
    ))
}

/// A row this build can't read (a downgrade) stays in the table for the build
/// that wrote it.
fn decode((id, chat_id, payload, created_at, failure): RawRow) -> Option<HeldSend> {
    match serde_json::from_str(&payload) {
        Ok(payload) => Some(HeldSend {
            id,
            chat_id,
            payload,
            created_at,
            failure,
        }),
        Err(err) => {
            tracing::warn!(send = %id, %err, "held send unreadable; leaving it");
            None
        }
    }
}

impl DocsStore {
    /// Hold a send. Idempotent on `id`.
    pub fn hold_send(
        &self,
        id: &str,
        chat_id: &str,
        payload: &HeldPayload,
        created_at: i64,
    ) -> Result<(), StoreError> {
        let payload = encode(payload)?;
        store_blocking(|| {
            self.conn().execute(
                "INSERT OR IGNORE INTO held_sends(id,chat_id,payload,created_at) VALUES (?1,?2,?3,?4)",
                params![id, chat_id, payload, created_at],
            )?;
            Ok(())
        })
    }

    /// Every held send, in send order.
    pub fn held_sends(&self) -> Result<Vec<HeldSend>, StoreError> {
        store_blocking(|| {
            let conn = self.conn();
            let mut stmt = conn.prepare(
                "SELECT id,chat_id,payload,created_at,failure FROM held_sends ORDER BY ordinal",
            )?;
            let rows = stmt
                .query_map([], read_row)?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows.into_iter().filter_map(decode).collect())
        })
    }

    /// One chat's held sends, in send order.
    pub fn held_sends_for(&self, chat_id: &str) -> Result<Vec<HeldSend>, StoreError> {
        store_blocking(|| {
            let conn = self.conn();
            let mut stmt = conn.prepare(
                "SELECT id,chat_id,payload,created_at,failure FROM held_sends
                 WHERE chat_id=?1 ORDER BY ordinal",
            )?;
            let rows = stmt
                .query_map(params![chat_id], read_row)?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows.into_iter().filter_map(decode).collect())
        })
    }

    /// The next send of a chat still trying: failed rows wait for the user and
    /// don't hold up what was sent after them.
    pub fn next_held_send(&self, chat_id: &str) -> Result<Option<HeldSend>, StoreError> {
        store_blocking(|| {
            let row = self
                .conn()
                .query_row(
                    "SELECT id,chat_id,payload,created_at,failure FROM held_sends
                     WHERE chat_id=?1 AND failure IS NULL ORDER BY ordinal LIMIT 1",
                    params![chat_id],
                    read_row,
                )
                .optional()?;
            Ok(row.and_then(decode))
        })
    }

    /// Whether a later send of this chat must queue behind a held one.
    pub fn has_active_held_send(&self, chat_id: &str) -> Result<bool, StoreError> {
        store_blocking(|| {
            Ok(self.conn().query_row(
                "SELECT EXISTS(SELECT 1 FROM held_sends WHERE chat_id=?1 AND failure IS NULL)",
                params![chat_id],
                |row| row.get(0),
            )?)
        })
    }

    /// Stop trying; the send stays, as a failed message, for the user to retry.
    pub fn fail_held_send(&self, id: &str, why: &str) -> Result<(), StoreError> {
        store_blocking(|| {
            self.conn().execute(
                "UPDATE held_sends SET failure=?2 WHERE id=?1",
                params![id, why],
            )?;
            Ok(())
        })
    }

    /// The user asked to try again: failed sends of the chat resume, with a
    /// fresh day of retries. Returns how many came back.
    pub fn retry_held_sends(&self, chat_id: &str, now: i64) -> Result<usize, StoreError> {
        store_blocking(|| {
            Ok(self.conn().execute(
                "UPDATE held_sends SET failure=NULL, created_at=?2
                 WHERE chat_id=?1 AND failure IS NOT NULL",
                params![chat_id, now],
            )?)
        })
    }

    /// The message is in the chat (or discarded).
    pub fn remove_held_send(&self, id: &str) -> Result<(), StoreError> {
        store_blocking(|| {
            self.conn()
                .execute("DELETE FROM held_sends WHERE id=?1", params![id])?;
            Ok(())
        })
    }

    /// Failed sends older than `failed_before`, removed and returned, so the
    /// caller can drop their local bytes too.
    pub fn purge_failed_held_sends(&self, failed_before: i64) -> Result<Vec<HeldSend>, StoreError> {
        store_blocking(|| {
            let conn = self.conn();
            let mut stmt = conn.prepare(
                "DELETE FROM held_sends WHERE failure IS NOT NULL AND created_at < ?1
                 RETURNING id,chat_id,payload,created_at,failure",
            )?;
            let rows = stmt
                .query_map(params![failed_before], read_row)?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows.into_iter().filter_map(decode).collect())
        })
    }
}

#[cfg(test)]
mod tests {
    use zeron_doc::{QueuedMessage, SessionCommandPayload};

    use crate::attachment_store::{HeldBody, HeldUpload};

    use super::*;

    fn payload(text: &str) -> HeldPayload {
        HeldPayload {
            body: HeldBody::Row {
                item: QueuedMessage::new("row", text, "phone"),
            },
            uploads: vec![HeldUpload {
                upload_id: "u1".into(),
                file_name: "a.png".into(),
            }],
            also_peer: false,
        }
    }

    fn open() -> (tempfile::TempDir, DocsStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = DocsStore::open(dir.path()).unwrap();
        (dir, store)
    }

    #[test]
    fn sends_survive_a_restart_in_order() {
        let dir = tempfile::tempdir().unwrap();
        {
            let store = DocsStore::open(dir.path()).unwrap();
            store.hold_send("a", "c1", &payload("first"), 10).unwrap();
            store.hold_send("b", "c1", &payload("second"), 20).unwrap();
            store.hold_send("x", "c2", &payload("other"), 15).unwrap();
            // Idempotent: a resumed send doesn't add a row.
            store.hold_send("a", "c1", &payload("first"), 10).unwrap();
        }
        let store = DocsStore::open(dir.path()).unwrap();
        let ids: Vec<_> = store
            .held_sends()
            .unwrap()
            .into_iter()
            .map(|s| s.id)
            .collect();
        assert_eq!(ids, ["a", "b", "x"]);
        let chat: Vec<_> = store
            .held_sends_for("c1")
            .unwrap()
            .into_iter()
            .map(|s| s.id)
            .collect();
        assert_eq!(chat, ["a", "b"]);
        assert_eq!(store.next_held_send("c1").unwrap().unwrap().id, "a");
    }

    #[test]
    fn a_failed_send_stays_visible_without_holding_up_the_next() {
        let (_dir, store) = open();
        store.hold_send("a", "c1", &payload("first"), 10).unwrap();
        store.hold_send("b", "c1", &payload("second"), 20).unwrap();
        assert!(store.has_active_held_send("c1").unwrap());

        store.fail_held_send("a", "too large").unwrap();
        assert_eq!(store.next_held_send("c1").unwrap().unwrap().id, "b");
        let failed = store.held_sends_for("c1").unwrap();
        assert_eq!(failed[0].failure.as_deref(), Some("too large"));

        store.remove_held_send("b").unwrap();
        assert!(!store.has_active_held_send("c1").unwrap());
        assert!(store.next_held_send("c1").unwrap().is_none());

        // Retry brings it back with a fresh clock.
        assert_eq!(store.retry_held_sends("c1", 99).unwrap(), 1);
        let back = store.next_held_send("c1").unwrap().unwrap();
        assert_eq!((back.id.as_str(), back.created_at), ("a", 99));
        assert_eq!(back.failure, None);
    }

    #[test]
    fn only_old_failures_are_purged() {
        let (_dir, store) = open();
        store.hold_send("old", "c1", &payload("old"), 10).unwrap();
        store.hold_send("new", "c1", &payload("new"), 500).unwrap();
        store
            .hold_send("trying", "c1", &payload("trying"), 10)
            .unwrap();
        store.fail_held_send("old", "no").unwrap();
        store.fail_held_send("new", "no").unwrap();

        let purged = store.purge_failed_held_sends(100).unwrap();
        assert_eq!(purged.len(), 1);
        assert_eq!(purged[0].id, "old");
        let left: Vec<_> = store
            .held_sends()
            .unwrap()
            .into_iter()
            .map(|s| s.id)
            .collect();
        assert_eq!(left, ["new", "trying"]);
    }

    #[test]
    fn a_run_command_round_trips_through_the_table() {
        let (_dir, store) = open();
        let held = HeldPayload {
            body: HeldBody::Command {
                payload: SessionCommandPayload::Interrupt {},
            },
            uploads: Vec::new(),
            also_peer: true,
        };
        store.hold_send("c", "c1", &held, 1).unwrap();
        assert_eq!(store.held_sends().unwrap()[0].payload, held);
    }
}
