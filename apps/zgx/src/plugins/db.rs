//! Each plugin's database (`zeron.db`): a SQLite file of its own, beside its
//! storage (`<data>/plugins/.storage/<id>.sqlite3`; removing the plugin
//! removes it, the engine's `RemovePlugin`). No permission: it's the
//! plugin's alone, as its storage is. For a cache, an index, a history:
//! anything past what `zeron.store`'s one small JSON file holds.
//!
//!   { op = "exec", sql, params? }   → { changes, last_id }
//!     (no params: a script of several statements, a schema)
//!   { op = "query", sql, params? }  → { { column = value, … }, … }
//!
//! Parameters are always bound (`?`, `?1`), never put in the SQL. A
//! database holds at most [`MAX_BYTES`], attaches no other file, and loads
//! no extension.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
};

use rusqlite::{
    Connection,
    hooks::{AuthAction, AuthContext, Authorization},
    limits::Limit,
    types::Value as Sql,
};
use serde_json::{Map, Value as Json, json};

/// A plugin's database at most.
pub const MAX_BYTES: u64 = 100 * 1024 * 1024;
const PAGE: u64 = 4096;

/// Where plugin `id`'s database is under `dir` (the plugins' storage).
pub fn path_in(dir: &Path, id: &str) -> Option<PathBuf> {
    let safe = !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    safe.then(|| dir.join(format!("{id}.sqlite3")))
}

/// Where plugin `id`'s database is (none without a data dir).
pub fn path(id: &str) -> Option<PathBuf> {
    path_in(&crate::engine::data_dir()?.join("plugins").join(".storage"), id)
}

/// Each database open, by its file: one connection, its calls one at a time.
fn open(path: &Path) -> Result<Arc<Mutex<Connection>>, String> {
    static OPEN: OnceLock<Mutex<HashMap<PathBuf, Arc<Mutex<Connection>>>>> = OnceLock::new();
    let mut open = OPEN.get_or_init(Default::default).lock().map_err(|_| "the database is stuck")?;
    if let Some(conn) = open.get(path) {
        return Ok(conn.clone());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let conn = Connection::open(path).map_err(|e| e.to_string())?;
    // No other file reached through it, and room for MAX_BYTES at most.
    conn.set_limit(Limit::SQLITE_LIMIT_ATTACHED, 0);
    conn.execute_batch(&format!("PRAGMA page_size = {PAGE}; PRAGMA journal_mode = WAL; PRAGMA max_page_count = {};", MAX_BYTES / PAGE))
        .map_err(|e| e.to_string())?;
    // From here, what the plugin's SQL may do: never lift those limits.
    conn.authorizer(Some(authorize));
    let conn = Arc::new(Mutex::new(conn));
    open.insert(path.to_path_buf(), conn.clone());
    Ok(conn)
}

/// Pragmas a plugin may set (how its own tables behave); any other pragma
/// only read. `max_page_count`, `journal_mode`, `page_size`,
/// `writable_schema`, … stay as Zeron set them (R29).
const SETTABLE_PRAGMAS: &[&str] =
    &["user_version", "application_id", "foreign_keys", "defer_foreign_keys", "recursive_triggers", "case_sensitive_like", "busy_timeout", "cache_size"];

/// What a plugin's SQL may do: anything to its own database but set the
/// pragmas that bound it, or attach another file.
fn authorize(context: AuthContext<'_>) -> Authorization {
    match context.action {
        AuthAction::Pragma { pragma_name, pragma_value: Some(_) }
            if !SETTABLE_PRAGMAS.contains(&pragma_name.to_ascii_lowercase().as_str()) =>
        {
            Authorization::Deny
        }
        // (A pragma that changes things without a value.)
        AuthAction::Pragma { pragma_name, pragma_value: None }
            if matches!(pragma_name.to_ascii_lowercase().as_str(), "shrink_memory" | "wal_checkpoint" | "incremental_vacuum") =>
        {
            Authorization::Allow
        }
        AuthAction::Attach { .. } | AuthAction::Detach { .. } => Authorization::Deny,
        _ => Authorization::Allow,
    }
}

fn bind(params: &Json) -> Result<Vec<Sql>, String> {
    params
        .as_array()
        .into_iter()
        .flatten()
        .map(|p| match p {
            Json::Null => Ok(Sql::Null),
            Json::Bool(b) => Ok(Sql::Integer(*b as i64)),
            Json::Number(n) => Ok(n.as_i64().map(Sql::Integer).unwrap_or_else(|| Sql::Real(n.as_f64().unwrap_or(0.)))),
            Json::String(s) => Ok(Sql::Text(s.clone())),
            _ => Err("A parameter is text, a number, a boolean or nil (keep a table as JSON text)".to_string()),
        })
        .collect()
}

fn value(v: Sql) -> Json {
    match v {
        Sql::Null => Json::Null,
        Sql::Integer(i) => json!(i),
        Sql::Real(f) => json!(f),
        Sql::Text(s) => json!(s),
        // (Blobs as their bytes' text: what a plugin wrote, it wrote as text.)
        Sql::Blob(b) => json!(String::from_utf8_lossy(&b)),
    }
}

/// Does what `asked` says to the database at `path`. Blocks: call it off
/// the window's thread.
pub fn call(path: &Path, asked: &Json) -> Result<Json, String> {
    let sql = asked["sql"].as_str().ok_or("Which SQL: `sql`")?;
    let params = bind(&asked["params"])?;
    let conn = open(path)?;
    let conn = conn.lock().map_err(|_| "the database is stuck")?;
    match asked["op"].as_str() {
        Some("exec") if params.is_empty() => {
            conn.execute_batch(sql).map_err(|e| e.to_string())?;
            Ok(json!({ "changes": conn.changes(), "last_id": conn.last_insert_rowid() }))
        }
        Some("exec") => {
            let changes = conn.execute(sql, rusqlite::params_from_iter(params)).map_err(|e| e.to_string())?;
            Ok(json!({ "changes": changes, "last_id": conn.last_insert_rowid() }))
        }
        Some("query") => {
            let mut statement = conn.prepare(sql).map_err(|e| e.to_string())?;
            let names: Vec<String> = statement.column_names().iter().map(|s| s.to_string()).collect();
            let mut rows = statement.query(rusqlite::params_from_iter(params)).map_err(|e| e.to_string())?;
            let mut out = Vec::new();
            while let Some(row) = rows.next().map_err(|e| e.to_string())? {
                let mut record = Map::new();
                for (i, name) in names.iter().enumerate() {
                    record.insert(name.clone(), value(row.get::<_, Sql>(i).map_err(|e| e.to_string())?));
                }
                out.push(Json::Object(record));
            }
            Ok(Json::Array(out))
        }
        _ => Err("`op` is \"exec\" or \"query\"".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("zgx-plugin-db-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        path_in(&dir, "p").unwrap()
    }

    #[test]
    fn a_plugin_keeps_and_reads_rows_with_bound_parameters() {
        let db = temp("rows");
        call(&db, &json!({ "op": "exec", "sql": "CREATE TABLE kv (k TEXT PRIMARY KEY, v TEXT, n INTEGER); CREATE INDEX kv_n ON kv(n);" })).unwrap();
        let put = |k: &str, v: &str, n: i64| call(&db, &json!({ "op": "exec", "sql": "INSERT OR REPLACE INTO kv VALUES (?, ?, ?)", "params": [k, v, n] })).unwrap();
        put("a", "one", 1);
        // (Bound, never spliced: quotes and SQL in a value are just text.)
        assert_eq!(put("b", "'); DROP TABLE kv; --", 2)["changes"], 1);
        let rows = call(&db, &json!({ "op": "query", "sql": "SELECT k, v, n FROM kv WHERE n >= ? ORDER BY n", "params": [1] })).unwrap();
        assert_eq!(rows, json!([{ "k": "a", "v": "one", "n": 1 }, { "k": "b", "v": "'); DROP TABLE kv; --", "n": 2 }]));
        assert!(call(&db, &json!({ "op": "query", "sql": "SELECT * FROM nope" })).is_err());
    }

    #[test]
    fn a_plugin_reaches_no_other_file_and_no_other_plugin() {
        let db = temp("attach");
        let other = db.with_file_name("other.sqlite3");
        let attach = call(&db, &json!({ "op": "exec", "sql": format!("ATTACH DATABASE '{}' AS o", other.display()) }));
        assert!(attach.is_err(), "{attach:?}");
        assert!(path_in(Path::new("/x"), "../evil").is_none() && path_in(Path::new("/x"), "").is_none());
        assert_eq!(path_in(Path::new("/x"), "github").unwrap(), Path::new("/x/github.sqlite3"));
    }

    /// R29: the size limit is Zeron's, not the plugin's to lift (it was
    /// one `PRAGMA max_page_count` away), and VACUUM INTO writes no file.
    #[test]
    fn a_plugin_cant_lift_its_limits() {
        let db = temp("limits");
        call(&db, &json!({ "op": "exec", "sql": "CREATE TABLE t (x)" })).unwrap();
        for sql in [
            "PRAGMA max_page_count = 2147483646",
            "PRAGMA main.max_page_count = 2147483646",
            "PRAGMA MAX_PAGE_COUNT=2147483646; INSERT INTO t VALUES (1)",
            "PRAGMA journal_mode = OFF",
            "PRAGMA writable_schema = 1",
            "PRAGMA page_size = 65536",
        ] {
            assert!(call(&db, &json!({ "op": "exec", "sql": sql })).is_err(), "{sql}");
        }
        let limit = call(&db, &json!({ "op": "query", "sql": "PRAGMA max_page_count" })).unwrap();
        assert_eq!(limit[0]["max_page_count"], json!(MAX_BYTES / PAGE), "{limit}");
        // What a plugin may set, it can.
        call(&db, &json!({ "op": "exec", "sql": "PRAGMA user_version = 3; PRAGMA foreign_keys = ON" })).unwrap();
        let out = db.with_file_name("copied.sqlite3");
        assert!(call(&db, &json!({ "op": "exec", "sql": format!("VACUUM INTO '{}'", out.display()) })).is_err());
        assert!(!out.exists());
    }
}
