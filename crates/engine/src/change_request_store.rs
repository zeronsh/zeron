//! On-disk copy of the pull request board, so it opens instantly — across
//! launches too — while a fresh page loads behind it.
//!
//! Pull requests are stored once each, per GitHub account, and a board query
//! (account, repository, filter) stores only which of them it returned, in
//! order, beside its total and cursor. A pull request seen through any query
//! therefore reads the same everywhere: the newest copy wins, so refreshing
//! one filter updates the others. Keyed by the `gh` account so switching
//! accounts never shows another account's results. Local data only; the
//! board's own refresh always reaches GitHub and writes back here.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use chrono::{TimeZone, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use zeron_proto::{ChangeRequestFilter, ChangeRequestListItem, ChangeRequestPage};

const FILE_NAME: &str = "pull-requests.sqlite3";
/// The schema below; older files are rebuilt (it's a cache).
const SCHEMA_VERSION: i64 = 3;
/// Queries older than this are dropped on write.
const MAX_AGE_DAYS: i64 = 30;
/// At most this many queries are kept, most recently fetched first. Pull
/// requests no remaining query returned go with them.
const MAX_QUERIES: i64 = 200;

#[derive(Clone)]
pub(crate) struct ChangeRequestStore {
    inner: Arc<Inner>,
}

struct Inner {
    path: PathBuf,
    /// Opened on first use: the board is optional, and boot shouldn't pay
    /// for it.
    conn: Mutex<Option<Connection>>,
}

impl ChangeRequestStore {
    pub(crate) fn new(data_dir: &Path) -> Self {
        Self {
            inner: Arc::new(Inner {
                path: data_dir.join(FILE_NAME),
                conn: Mutex::new(None),
            }),
        }
    }

    /// The stored first page for this query, its pull requests at their
    /// newest stored state, and when the query was fetched.
    pub(crate) async fn load(
        &self,
        account: &str,
        repository: &str,
        filter: ChangeRequestFilter,
    ) -> Option<ChangeRequestPage> {
        let inner = self.inner.clone();
        let (account, repository) = (account.to_owned(), repository.to_ascii_lowercase());
        let filter = filter_key(filter);
        tokio::task::spawn_blocking(move || {
            inner.with_conn(|conn| {
                let Some((fetched_at, next_cursor, total_count)) = conn
                    .query_row(
                        "SELECT fetched_at, next_cursor, total_count FROM queries
                         WHERE account = ?1 AND repository = ?2 AND filter = ?3",
                        params![account, repository, filter],
                        |row| {
                            Ok((
                                row.get::<_, i64>(0)?,
                                row.get::<_, Option<String>>(1)?,
                                row.get::<_, Option<i64>>(2)?,
                            ))
                        },
                    )
                    .optional()?
                else {
                    return Ok(None);
                };
                let mut statement = conn.prepare(
                    "SELECT p.item FROM query_results r
                     JOIN pull_requests p ON p.account = r.account AND p.url = r.url
                     WHERE r.account = ?1 AND r.repository = ?2 AND r.filter = ?3
                     ORDER BY r.position",
                )?;
                let items = statement
                    .query_map(params![account, repository, filter], |row| {
                        row.get::<_, String>(0)
                    })?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(Some((fetched_at, next_cursor, total_count, items)))
            })
        })
        .await
        .ok()
        .flatten()
        .flatten()
        .and_then(|(fetched_at, next_cursor, total_count, items)| {
            // A row this build can't read makes the whole page a miss: the
            // board then loads from GitHub rather than show a partial list.
            let items = items
                .iter()
                .map(|item| serde_json::from_str::<ChangeRequestListItem>(item).ok())
                .collect::<Option<Vec<_>>>()?;
            Some(ChangeRequestPage {
                items,
                next_cursor,
                total_count: total_count.and_then(|count| u64::try_from(count).ok()),
                fetched_at: Utc.timestamp_millis_opt(fetched_at).single(),
            })
        })
    }

    /// Keep `page` as this query's latest first page. Each pull request in
    /// it replaces the stored copy unless that copy is newer, whichever
    /// query fetched it. Then prune what has aged out.
    pub(crate) async fn save(
        &self,
        account: &str,
        repository: &str,
        filter: ChangeRequestFilter,
        page: &ChangeRequestPage,
    ) {
        let Some(items) = page
            .items
            .iter()
            .map(|item| Some((item.url.clone(), serde_json::to_string(item).ok()?)))
            .collect::<Option<Vec<_>>>()
        else {
            return;
        };
        // Milliseconds, so a stored page reports the exact fetch time.
        let fetched_at = page.fetched_at.unwrap_or_else(Utc::now).timestamp_millis();
        let next_cursor = page.next_cursor.clone();
        let total_count = page.total_count.and_then(|count| i64::try_from(count).ok());
        let inner = self.inner.clone();
        let (account, repository) = (account.to_owned(), repository.to_ascii_lowercase());
        let filter = filter_key(filter);
        let saved = tokio::task::spawn_blocking(move || {
            inner.with_conn_mut(|conn| {
                let tx = conn.transaction()?;
                for (url, item) in &items {
                    tx.execute(
                        "INSERT INTO pull_requests (account, url, fetched_at, item)
                         VALUES (?1, ?2, ?3, ?4)
                         ON CONFLICT (account, url) DO UPDATE SET
                             fetched_at = excluded.fetched_at, item = excluded.item
                         WHERE excluded.fetched_at >= pull_requests.fetched_at",
                        params![account, url, fetched_at, item],
                    )?;
                }
                // The query's membership is replaced whole, unless a newer
                // read of it already landed.
                let newer: Option<i64> = tx
                    .query_row(
                        "SELECT fetched_at FROM queries
                         WHERE account = ?1 AND repository = ?2 AND filter = ?3
                           AND fetched_at > ?4",
                        params![account, repository, filter, fetched_at],
                        |row| row.get(0),
                    )
                    .optional()?;
                if newer.is_none() {
                    tx.execute(
                        "INSERT INTO queries
                             (account, repository, filter, fetched_at, next_cursor, total_count)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                         ON CONFLICT (account, repository, filter) DO UPDATE SET
                             fetched_at = excluded.fetched_at,
                             next_cursor = excluded.next_cursor,
                             total_count = excluded.total_count",
                        params![
                            account,
                            repository,
                            filter,
                            fetched_at,
                            next_cursor,
                            total_count
                        ],
                    )?;
                    tx.execute(
                        "DELETE FROM query_results
                         WHERE account = ?1 AND repository = ?2 AND filter = ?3",
                        params![account, repository, filter],
                    )?;
                    for (position, (url, _)) in items.iter().enumerate() {
                        tx.execute(
                            "INSERT INTO query_results (account, repository, filter, position, url)
                             VALUES (?1, ?2, ?3, ?4, ?5)",
                            params![account, repository, filter, position as i64, url],
                        )?;
                    }
                }
                prune(&tx)?;
                tx.commit()
            })
        })
        .await;
        if !matches!(saved, Ok(Some(()))) {
            tracing::debug!("pull request board cache write skipped");
        }
    }
}

/// Drop aged-out and excess queries, their results, and every pull request
/// no remaining query returned.
fn prune(conn: &Connection) -> rusqlite::Result<()> {
    let cutoff = Utc::now().timestamp_millis() - MAX_AGE_DAYS * 24 * 60 * 60 * 1000;
    conn.execute("DELETE FROM queries WHERE fetched_at < ?1", params![cutoff])?;
    conn.execute(
        "DELETE FROM queries WHERE rowid NOT IN (
             SELECT rowid FROM queries ORDER BY fetched_at DESC LIMIT ?1)",
        params![MAX_QUERIES],
    )?;
    conn.execute(
        "DELETE FROM query_results WHERE NOT EXISTS (
             SELECT 1 FROM queries q
             WHERE q.account = query_results.account
               AND q.repository = query_results.repository
               AND q.filter = query_results.filter)",
        [],
    )?;
    conn.execute(
        "DELETE FROM pull_requests WHERE NOT EXISTS (
             SELECT 1 FROM query_results r
             WHERE r.account = pull_requests.account AND r.url = pull_requests.url)",
        [],
    )?;
    Ok(())
}

impl Inner {
    /// Run `f` on the open database, opening and migrating it first. `None`
    /// when the database can't be opened or `f` fails; the board then simply
    /// loads from GitHub.
    fn with_conn<T>(&self, f: impl FnOnce(&Connection) -> rusqlite::Result<T>) -> Option<T> {
        self.with_conn_mut(|conn| f(conn))
    }

    fn with_conn_mut<T>(
        &self,
        f: impl FnOnce(&mut Connection) -> rusqlite::Result<T>,
    ) -> Option<T> {
        let mut conn = self.conn.lock().ok()?;
        if conn.is_none() {
            *conn = Some(open(&self.path).ok()?);
        }
        f(conn.as_mut()?).ok()
    }
}

fn open(path: &Path) -> rusqlite::Result<Connection> {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let conn = Connection::open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.busy_timeout(std::time::Duration::from_secs(2))?;
    let version: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version != SCHEMA_VERSION {
        // A cache: rebuild rather than migrate.
        conn.execute_batch(
            "DROP TABLE IF EXISTS change_request_pages;
             DROP TABLE IF EXISTS query_results;
             DROP TABLE IF EXISTS queries;
             DROP TABLE IF EXISTS pull_requests;",
        )?;
    }
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS pull_requests (
             account TEXT NOT NULL,
             url TEXT NOT NULL,
             fetched_at INTEGER NOT NULL,
             item TEXT NOT NULL,
             PRIMARY KEY (account, url)
         );
         CREATE TABLE IF NOT EXISTS queries (
             account TEXT NOT NULL,
             repository TEXT NOT NULL,
             filter TEXT NOT NULL,
             fetched_at INTEGER NOT NULL,
             next_cursor TEXT,
             total_count INTEGER,
             PRIMARY KEY (account, repository, filter)
         );
         CREATE TABLE IF NOT EXISTS query_results (
             account TEXT NOT NULL,
             repository TEXT NOT NULL,
             filter TEXT NOT NULL,
             position INTEGER NOT NULL,
             url TEXT NOT NULL,
             PRIMARY KEY (account, repository, filter, position)
         );
         CREATE INDEX IF NOT EXISTS query_results_by_pull_request
             ON query_results (account, url);",
    )?;
    conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    Ok(conn)
}

fn filter_key(filter: ChangeRequestFilter) -> &'static str {
    match filter {
        ChangeRequestFilter::All => "all",
        ChangeRequestFilter::Authored => "authored",
        ChangeRequestFilter::Reviewing => "reviewing",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::DateTime;
    use zeron_proto::{ChangeRequestMergeability, ChangeRequestState};

    fn item(number: u64, title: &str) -> ChangeRequestListItem {
        ChangeRequestListItem {
            provider: "github".into(),
            author: Default::default(),
            repository: "acme/zeron".into(),
            head_ref_oid: String::new(),
            ci: Default::default(),
            viewer_did_author: None,
            viewer_review_requested: None,
            number,
            title: title.into(),
            url: format!("https://github.com/acme/zeron/pull/{number}"),
            state: ChangeRequestState::Open,
            is_draft: false,
            review_decision: Default::default(),
            additions: 0,
            deletions: 0,
            mergeability: ChangeRequestMergeability::Unknown,
            created_at: DateTime::UNIX_EPOCH,
            updated_at: DateTime::UNIX_EPOCH,
        }
    }

    fn page(items: Vec<ChangeRequestListItem>, fetched_at: DateTime<Utc>) -> ChangeRequestPage {
        ChangeRequestPage {
            total_count: Some(items.len() as u64),
            items,
            next_cursor: Some("Y3Vyc29yOjUw".into()),
            fetched_at: Some(fetched_at),
        }
    }

    fn titles(page: &ChangeRequestPage) -> Vec<&str> {
        page.items.iter().map(|item| item.title.as_str()).collect()
    }

    #[tokio::test]
    async fn queries_round_trip_per_account_repository_and_filter() {
        let dir = tempfile::tempdir().unwrap();
        let store = ChangeRequestStore::new(dir.path());
        let now = Utc::now();
        let all = ChangeRequestFilter::All;
        assert!(store.load("me", "acme/zeron", all).await.is_none());
        store
            .save(
                "me",
                "Acme/Zeron",
                all,
                &page(vec![item(2, "two"), item(1, "one")], now),
            )
            .await;
        let loaded = store.load("me", "acme/zeron", all).await.unwrap();
        assert_eq!(titles(&loaded), ["two", "one"], "order is kept");
        assert_eq!(loaded.next_cursor.as_deref(), Some("Y3Vyc29yOjUw"));
        assert_eq!(loaded.total_count, Some(2));
        assert_eq!(loaded.fetched_at.unwrap().timestamp(), now.timestamp());
        // Another account, filter or repository never sees it.
        for (account, repository, filter) in [
            ("other", "acme/zeron", all),
            ("me", "acme/zeron", ChangeRequestFilter::Authored),
            ("me", "acme/other", all),
        ] {
            assert!(store.load(account, repository, filter).await.is_none());
        }
        // It persists across instances (a relaunch).
        let reopened = ChangeRequestStore::new(dir.path());
        let loaded = reopened.load("me", "acme/zeron", all).await.unwrap();
        assert_eq!(titles(&loaded), ["two", "one"]);
    }

    #[tokio::test]
    async fn a_pull_request_reads_the_same_in_every_filter() {
        let dir = tempfile::tempdir().unwrap();
        let store = ChangeRequestStore::new(dir.path());
        let earlier = Utc::now() - chrono::Duration::minutes(10);
        let now = Utc::now();
        store
            .save(
                "me",
                "acme/zeron",
                ChangeRequestFilter::Authored,
                &page(vec![item(1, "CI running")], earlier),
            )
            .await;
        // Refreshing All later updates #1 for Authored too.
        store
            .save(
                "me",
                "acme/zeron",
                ChangeRequestFilter::All,
                &page(vec![item(2, "other"), item(1, "CI passed")], now),
            )
            .await;
        let authored = store
            .load("me", "acme/zeron", ChangeRequestFilter::Authored)
            .await
            .unwrap();
        assert_eq!(titles(&authored), ["CI passed"]);
        // The Authored query still carries its own (older) fetch time.
        assert_eq!(
            authored.fetched_at.unwrap().timestamp(),
            earlier.timestamp()
        );
        // An older read never overwrites a newer copy.
        store
            .save(
                "me",
                "acme/zeron",
                ChangeRequestFilter::Reviewing,
                &page(
                    vec![item(1, "stale")],
                    earlier - chrono::Duration::minutes(1),
                ),
            )
            .await;
        let all = store
            .load("me", "acme/zeron", ChangeRequestFilter::All)
            .await
            .unwrap();
        assert_eq!(titles(&all), ["other", "CI passed"]);
    }

    #[tokio::test]
    async fn a_refresh_replaces_membership_and_prunes_dropped_pull_requests() {
        let dir = tempfile::tempdir().unwrap();
        let store = ChangeRequestStore::new(dir.path());
        let all = ChangeRequestFilter::All;
        let now = Utc::now();
        store
            .save(
                "me",
                "acme/zeron",
                all,
                &page(vec![item(1, "one"), item(2, "two")], now),
            )
            .await;
        // #2 merged: the next read no longer returns it.
        store
            .save(
                "me",
                "acme/zeron",
                all,
                &page(vec![item(1, "one")], now + chrono::Duration::seconds(5)),
            )
            .await;
        assert_eq!(
            titles(&store.load("me", "acme/zeron", all).await.unwrap()),
            ["one"]
        );
        // An older read of the same query never replaces a newer one.
        store
            .save(
                "me",
                "acme/zeron",
                all,
                &page(vec![item(3, "three")], now - chrono::Duration::seconds(5)),
            )
            .await;
        assert_eq!(
            titles(&store.load("me", "acme/zeron", all).await.unwrap()),
            ["one"]
        );
        let count = |table: &str| {
            store
                .inner
                .with_conn(|conn| {
                    conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                        row.get::<_, i64>(0)
                    })
                })
                .unwrap()
        };
        assert_eq!(
            count("pull_requests"),
            1,
            "#2 and #3 are referenced by no query"
        );
    }

    #[tokio::test]
    async fn aged_out_queries_are_pruned_on_write() {
        let dir = tempfile::tempdir().unwrap();
        let store = ChangeRequestStore::new(dir.path());
        let old = Utc::now() - chrono::Duration::days(MAX_AGE_DAYS + 1);
        store
            .save(
                "me",
                "acme/old",
                ChangeRequestFilter::All,
                &page(vec![item(1, "one")], old),
            )
            .await;
        assert!(
            store
                .load("me", "acme/old", ChangeRequestFilter::All)
                .await
                .is_none()
        );
    }
}
