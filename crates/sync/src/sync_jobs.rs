//! Durable work discovery uses metadata only; no document/network is opened
//! until the engine admits a bounded page. Conditional completion cannot erase
//! a newer wakeup that arrived during a catch-up.
use crate::store::{DocsStore, StoreError, store_blocking};
use rusqlite::{OptionalExtension, params};

impl DocsStore {
    /// Counts only; diagnostics must not materialize queued payloads.
    pub fn sync_work_counts(&self) -> Result<(u64, u64), StoreError> {
        store_blocking(|| {
            Ok(self.conn().query_row(
                "SELECT (SELECT count(*) FROM chat_outbox), (SELECT count(*) FROM chat_sync_jobs)",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?)
        })
    }

    pub fn schedule_sync_job(&self, doc: &str, kind: &str) -> Result<(), StoreError> {
        store_blocking(|| {
            let mut conn = self.conn();
            let tx = conn.transaction()?;
            tx.execute("UPDATE sync_job_clock SET value=value+1 WHERE id=1", [])?;
            tx.execute(
                "INSERT INTO chat_sync_jobs(doc_id,kind,version) SELECT ?1,?2,value FROM sync_job_clock WHERE id=1
                 ON CONFLICT(doc_id,kind) DO UPDATE SET version=excluded.version,cursor=''",
                params![doc, kind],
            )?;
            tx.commit()?;
            Ok(())
        })
    }

    pub fn reconciliation_progress(&self) -> Result<Option<(i64, String)>, StoreError> {
        store_blocking(|| {
            Ok(self.conn().query_row(
            "SELECT version,cursor FROM chat_sync_jobs WHERE doc_id='*' AND kind='reconcile'", [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        ).optional()?)
        })
    }

    /// Atomically accept one bounded page and advance the durable scan cursor.
    pub fn reconcile_page(&self, version: i64, ids: &[String]) -> Result<(), StoreError> {
        store_blocking(|| {
            let mut conn = self.conn();
            let tx = conn.transaction()?;
            let current: Option<i64> = tx
                .query_row(
                    "SELECT version FROM chat_sync_jobs WHERE doc_id='*' AND kind='reconcile'",
                    [],
                    |r| r.get(0),
                )
                .optional()?;
            if current != Some(version) {
                return Ok(());
            }
            if let Some(last) = ids.last() {
                tx.execute("UPDATE sync_job_clock SET value=value+1 WHERE id=1", [])?;
                for id in ids {
                    tx.execute("INSERT INTO chat_sync_jobs(doc_id,kind,version) SELECT ?1,'wake',value FROM sync_job_clock WHERE id=1 ON CONFLICT(doc_id,kind) DO UPDATE SET version=excluded.version", params![id])?;
                }
                tx.execute(
                    "UPDATE chat_sync_jobs SET cursor=?1 WHERE doc_id='*' AND kind='reconcile'",
                    params![last],
                )?;
            } else {
                tx.execute(
                    "DELETE FROM chat_sync_jobs WHERE doc_id='*' AND kind='reconcile'",
                    [],
                )?;
            }
            tx.commit()?;
            Ok(())
        })
    }

    pub fn sync_job_version(&self, doc: &str, kind: &str) -> Result<Option<i64>, StoreError> {
        store_blocking(|| {
            Ok(self
                .conn()
                .query_row(
                    "SELECT version FROM chat_sync_jobs WHERE doc_id=?1 AND kind=?2",
                    params![doc, kind],
                    |r| r.get(0),
                )
                .optional()?)
        })
    }

    pub fn complete_sync_job(&self, doc: &str, kind: &str, version: i64) -> Result<(), StoreError> {
        store_blocking(|| {
            self.conn().execute(
                "DELETE FROM chat_sync_jobs WHERE doc_id=?1 AND kind=?2 AND version=?3",
                params![doc, kind, version],
            )?;
            Ok(())
        })
    }

    pub fn pending_sync_jobs(
        &self,
        kind: &str,
        after: &str,
        limit: usize,
    ) -> Result<Vec<String>, StoreError> {
        store_blocking(|| {
            let conn = self.conn();
            let mut q = conn.prepare("SELECT doc_id FROM chat_sync_jobs WHERE kind=?1 AND doc_id>?2 ORDER BY doc_id LIMIT ?3")?;
            Ok(
                q.query_map(params![kind, after, limit.min(64) as i64], |r| r.get(0))?
                    .collect::<Result<_, _>>()?,
            )
        })
    }

    pub fn pending_sync_docs(&self, after: &str, limit: usize) -> Result<Vec<String>, StoreError> {
        store_blocking(|| {
            let conn = self.conn();
            let mut q = conn.prepare(
                "SELECT doc_id FROM (SELECT doc_id FROM chat_outbox
                  UNION SELECT doc_id FROM chat_sync_jobs WHERE kind='wake')
                 WHERE doc_id>?1 ORDER BY doc_id LIMIT ?2",
            )?;
            Ok(
                q.query_map(params![after, limit.min(64) as i64], |r| r.get(0))?
                    .collect::<Result<_, _>>()?,
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reconciliation_resumes_and_new_requests_fence_old_completion() {
        let dir = tempfile::tempdir().unwrap();
        let store = DocsStore::open(dir.path()).unwrap();
        store.schedule_sync_job("*", "reconcile").unwrap();
        let (version, _) = store.reconciliation_progress().unwrap().unwrap();
        store
            .reconcile_page(version, &["a".into(), "b".into()])
            .unwrap();
        drop(store);
        let store = DocsStore::open(dir.path()).unwrap();
        assert_eq!(
            store.reconciliation_progress().unwrap(),
            Some((version, "b".into()))
        );
        assert_eq!(store.pending_sync_docs("", 10).unwrap(), vec!["a", "b"]);
        store.schedule_sync_job("*", "reconcile").unwrap();
        store.reconcile_page(version, &[]).unwrap();
        let (newer, cursor) = store.reconciliation_progress().unwrap().unwrap();
        assert!(newer > version);
        assert!(cursor.is_empty());
        store.reconcile_page(newer, &[]).unwrap();
        assert!(store.reconciliation_progress().unwrap().is_none());
    }
    #[test]
    fn work_survives_restart_pages_without_payloads_and_preserves_new_wakes() {
        let dir = tempfile::tempdir().unwrap();
        {
            let store = DocsStore::open(dir.path()).unwrap();
            for n in 0..300 {
                store
                    .schedule_sync_job(&format!("chat-{n:04}"), "wake")
                    .unwrap();
            }
            store
                .enqueue_chat_update("outgoing", "batch", b"durable")
                .unwrap();
            store.reject_chat_update("outgoing", "batch").unwrap();
        }
        let store = DocsStore::open(dir.path()).unwrap();
        let mut after = String::new();
        let mut total = 0;
        loop {
            let page = store.pending_sync_docs(&after, 17).unwrap();
            if page.is_empty() {
                break;
            }
            total += page.len();
            after = page.last().unwrap().clone();
        }
        assert_eq!(total, 301);
        let old = store
            .sync_job_version("chat-0000", "wake")
            .unwrap()
            .unwrap();
        store.schedule_sync_job("chat-0000", "wake").unwrap();
        store.complete_sync_job("chat-0000", "wake", old).unwrap();
        assert_eq!(
            store.sync_job_version("chat-0000", "wake").unwrap(),
            Some(301)
        );
        store.complete_sync_job("chat-0000", "wake", 301).unwrap();
        assert_eq!(store.sync_job_version("chat-0000", "wake").unwrap(), None);
        store.schedule_sync_job("chat-0000", "wake").unwrap();
        store.complete_sync_job("chat-0000", "wake", 301).unwrap();
        assert_eq!(
            store.sync_job_version("chat-0000", "wake").unwrap(),
            Some(302)
        );
    }
}
