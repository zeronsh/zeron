//! Versioned OpenCode credentials. v2's public credential API only edits labels
//! and activates/removes IDs; it cannot import Zeron's OAuth token sets. Keep
//! that compatibility boundary here, with a schema check and atomic SQLite
//! transactions. OpenCode owns schema creation and migration.
use super::*;
use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior, params};
use serde_json::{Map, Value, json};

const SNAPSHOT: &str = "_opencodeCredential";

fn failure(message: impl std::fmt::Display) -> EngineError {
    EngineError::Other(format!(
        "OpenCode credential store: {message}. Use `opencode auth login` if this CLI's storage format has changed"
    ))
}

pub(super) fn database(auth: &Path) -> PathBuf {
    auth.with_file_name("opencode.db")
}

pub(super) fn has_credentials(auth: &Path) -> Result<bool, EngineError> {
    let path = database(auth);
    if !path.exists() {
        return Ok(false);
    }
    let db =
        Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(failure)?;
    db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='credential')",
        [],
        |row| row.get(0),
    )
    .map_err(failure)
}

fn open(auth: &Path, write: bool) -> Result<Connection, EngineError> {
    // Deliberately exclude CREATE: a missing/uninitialized DB must never be
    // mistaken for an empty login and overwritten with a partial schema.
    let flags = if write {
        OpenFlags::SQLITE_OPEN_READ_WRITE
    } else {
        OpenFlags::SQLITE_OPEN_READ_ONLY
    };
    let db = Connection::open_with_flags(database(auth), flags).map_err(failure)?;
    db.busy_timeout(Duration::from_secs(3)).map_err(failure)?;
    let mut query = db
        .prepare("PRAGMA table_info(credential)")
        .map_err(failure)?;
    let columns = query
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(failure)?
        .collect::<Result<std::collections::HashSet<_>, _>>()
        .map_err(failure)?;
    for column in [
        "id",
        "integration_id",
        "label",
        "value",
        "active",
        "time_created",
        "time_updated",
    ] {
        if !columns.contains(column) {
            return Err(failure(format!(
                "unsupported SQLite schema (missing {column})"
            )));
        }
    }
    drop(query);
    Ok(db)
}

/// Same ordering as OpenCode's Integration service: active first, then newest.
/// An active API key must hide older OAuth records, not become a phantom login.
pub(super) fn read(auth: &Path) -> Result<Value, EngineError> {
    if !database(auth).exists() {
        return Ok(json!({}));
    }
    let db = open(auth, false)?;
    let mut query = db.prepare("SELECT id, integration_id, value FROM credential WHERE integration_id IS NOT NULL ORDER BY active DESC, time_created DESC, id DESC").map_err(failure)?;
    let rows = query
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(failure)?;
    let mut entries = Map::new();
    for row in rows {
        let (id, provider, raw) = row.map_err(failure)?;
        if entries.contains_key(&provider) {
            continue;
        }
        let value: Value =
            serde_json::from_str(&raw).map_err(|_| failure("invalid credential JSON"))?;
        let entry = legacy_entry(&id, &value)?;
        entries.insert(provider, entry);
    }
    Ok(Value::Object(entries))
}

fn legacy_entry(id: &str, value: &Value) -> Result<Value, EngineError> {
    if value["type"] != "oauth" {
        return Ok(value.clone());
    }
    for key in ["access", "refresh", "methodID"] {
        if !value[key].is_string() {
            return Err(failure(format!("invalid OAuth {key}")));
        }
    }
    if !value["expires"].is_u64() {
        return Err(failure("invalid OAuth expiry"));
    }
    if value
        .get("metadata")
        .is_some_and(|metadata| !metadata.is_object() && !metadata.is_null())
    {
        return Err(failure("invalid OAuth metadata"));
    }
    let mut legacy = json!({"type":"oauth", "access":value["access"], "refresh":value["refresh"], "expires":value["expires"]});
    for (native, old) in [
        ("accountID", "accountId"),
        ("enterpriseUrl", "enterpriseUrl"),
    ] {
        if let Some(field) = value.get("metadata").and_then(|v| v.get(native)) {
            legacy[old] = field.clone();
        }
    }
    // Preserve method and plugin metadata through existing private
    // account snapshots, without changing the common OAuth identity adapter.
    legacy[SNAPSHOT] = json!({"id":id, "value":value});
    Ok(legacy)
}

fn native_entry(provider: &str, entry: &Value) -> Result<Value, EngineError> {
    if !stores::oauth_entry(entry) || !entry["refresh"].is_string() || !entry["expires"].is_u64() {
        return Err(failure("invalid OAuth token set; refusing to write"));
    }
    let mut value = entry.get(SNAPSHOT).and_then(|v| v.get("value")).cloned()
        .unwrap_or_else(|| json!({"type":"oauth", "methodID":match provider { "openai" => "chatgpt-browser", "github-copilot" => "device", _ => "oauth" }}));
    if !value.is_object()
        || value["type"] != "oauth"
        || !value["methodID"].is_string()
        || value
            .get("metadata")
            .is_some_and(|metadata| !metadata.is_object() && !metadata.is_null())
    {
        return Err(failure("invalid native OAuth snapshot; refusing to write"));
    }
    for field in ["access", "refresh", "expires"] {
        value[field] = entry[field].clone();
    }
    for (old, native) in [
        ("accountId", "accountID"),
        ("enterpriseUrl", "enterpriseUrl"),
    ] {
        if let Some(field) = entry.get(old) {
            if value.get("metadata").is_none() {
                value["metadata"] = json!({});
            }
            value["metadata"][native] = field.clone();
        }
    }
    Ok(value)
}

#[cfg(test)]
pub(super) fn write(auth: &Path, provider: &str, entry: Option<&Value>) -> Result<(), EngineError> {
    write_checked(auth, provider, entry, None)
}

pub(super) fn write_checked(
    auth: &Path,
    provider: &str,
    entry: Option<&Value>,
    expected_id: Option<&str>,
) -> Result<(), EngineError> {
    let mut db = open(auth, true)?;
    let tx = db
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(failure)?;
    let current: Option<(String, String)> = tx.query_row(
        "SELECT id, value FROM credential WHERE integration_id=?1 ORDER BY active DESC, time_created DESC, id DESC LIMIT 1",
        [provider], |row| Ok((row.get(0)?, row.get(1)?))
    ).optional().map_err(failure)?;
    let now = now_ms();
    if let Some(expected) = expected_id
        && current.as_ref().map(|(id, _)| id.as_str()) != Some(expected)
    {
        return Err(failure(
            "the live login changed while it was being removed; refresh and retry",
        ));
    }
    if let Some(entry) = entry {
        let native = native_entry(provider, entry)?;
        let saved_id = entry
            .get(SNAPSHOT)
            .and_then(|v| v.get("id"))
            .and_then(Value::as_str);
        let existing = match saved_id {
            Some(id) => tx.query_row("SELECT id FROM credential WHERE id=?1 AND integration_id=?2 AND json_extract(value,'$.type')='oauth'", params![id, provider], |row| row.get::<_, String>(0)).optional().map_err(failure)?,
            None => None,
        };
        let id = if let Some(id) = existing {
            // Native refresh tokens may be newer than the saved snapshot.
            // Activating a native ID never restores stale token bytes over it.
            id
        } else {
            let id = format!("cred_zeron_{}", new_id());
            tx.execute("INSERT INTO credential (id,integration_id,label,value,active,time_created,time_updated) VALUES (?1,?2,'Zeron OAuth',?3,0,?4,?4)", params![id, provider, native.to_string(), now]).map_err(failure)?;
            id
        };
        tx.execute(
            "UPDATE credential SET active=0,time_updated=?2 WHERE integration_id=?1",
            params![provider, now],
        )
        .map_err(failure)?;
        tx.execute(
            "UPDATE credential SET active=1,time_updated=?2 WHERE id=?1",
            params![id, now],
        )
        .map_err(failure)?;
    } else if let Some((id, raw)) = current {
        let value: Value =
            serde_json::from_str(&raw).map_err(|_| failure("invalid credential JSON"))?;
        if value["type"] != "oauth" {
            return Err(failure(
                "live credential is no longer an OAuth login; refresh and retry",
            ));
        }
        tx.execute("DELETE FROM credential WHERE id=?1", [id])
            .map_err(failure)?;
        // Match OpenCode's removal semantics, preserving other native logins
        // and API keys rather than deleting an integration's entire pool.
        tx.execute(
            "UPDATE credential SET active=0,time_updated=?2 WHERE integration_id=?1",
            params![provider, now],
        )
        .map_err(failure)?;
        tx.execute("UPDATE credential SET active=1,time_updated=?2 WHERE id=(SELECT id FROM credential WHERE integration_id=?1 ORDER BY time_created DESC,id DESC LIMIT 1)", params![provider, now]).map_err(failure)?;
    }
    tx.commit().map_err(failure)
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    pub(crate) fn fixture(auth: &Path) -> Connection {
        std::fs::create_dir_all(auth.parent().unwrap()).unwrap();
        let db = Connection::open(database(auth)).unwrap();
        db.execute_batch("CREATE TABLE credential (id TEXT PRIMARY KEY, integration_id TEXT, label TEXT NOT NULL, value TEXT NOT NULL, active INTEGER, time_created INTEGER NOT NULL, time_updated INTEGER); PRAGMA journal_mode=WAL;").unwrap();
        db
    }

    #[test]
    fn imports_and_switches_without_touching_keys_or_restoring_stale_refresh_tokens() {
        let temp = tempfile::tempdir().unwrap();
        let auth = temp.path().join("auth.json");
        let db = fixture(&auth);
        db.execute(
            "INSERT INTO credential VALUES ('key','anthropic','API key',?1,1,1,1)",
            [json!({"type":"key","key":"keep"}).to_string()],
        )
        .unwrap();
        let entry = json!({"type":"oauth","access":"access-a","refresh":"refresh-a","expires":123,"accountId":"acct-a"});
        write(&auth, "openai", Some(&entry)).unwrap();
        let saved = read(&auth).unwrap()["openai"].clone();
        assert_eq!(saved["accountId"], "acct-a");
        assert_eq!(saved[SNAPSHOT]["value"]["methodID"], "chatgpt-browser");
        let id = saved[SNAPSHOT]["id"].as_str().unwrap();
        db.execute(
            "UPDATE credential SET value=json_set(value,'$.refresh','rotated') WHERE id=?1",
            [id],
        )
        .unwrap();
        let mut other = entry.clone();
        other["access"] = json!("access-b");
        write(&auth, "openai", Some(&other)).unwrap();
        write(&auth, "openai", Some(&saved)).unwrap();
        assert_eq!(read(&auth).unwrap()["openai"]["refresh"], "rotated");
        assert_eq!(read(&auth).unwrap()["anthropic"]["key"], "keep");
        write(&auth, "openai", None).unwrap();
        assert_eq!(read(&auth).unwrap()["openai"]["access"], "access-b");
        assert!(
            !auth.exists(),
            "v2 writes must never create an ignored auth.json"
        );
    }

    #[test]
    fn concurrent_native_switch_cannot_remove_the_wrong_login() {
        let temp = tempfile::tempdir().unwrap();
        let auth = temp.path().join("auth.json");
        let _db = fixture(&auth);
        let entry =
            |access| json!({"type":"oauth","access":access,"refresh":"refresh","expires":0});
        write(&auth, "openai", Some(&entry("a"))).unwrap();
        let before = read(&auth).unwrap();
        let id = before["openai"][SNAPSHOT]["id"].as_str().unwrap();
        write(&auth, "openai", Some(&entry("b"))).unwrap();
        assert!(
            write_checked(&auth, "openai", None, Some(id))
                .unwrap_err()
                .to_string()
                .contains("live login changed")
        );
        assert_eq!(read(&auth).unwrap()["openai"]["access"], "b");
    }

    #[test]
    fn schema_mismatch_and_bad_tokens_leave_the_database_unchanged() {
        let temp = tempfile::tempdir().unwrap();
        let auth = temp.path().join("auth.json");
        assert!(write(&auth, "openai", None).is_err());
        assert!(!database(&auth).exists());
        let db = fixture(&auth);
        assert!(
            write(
                &auth,
                "openai",
                Some(&json!({"type":"oauth","access":"secret"}))
            )
            .is_err()
        );
        let count: i64 = db
            .query_row("SELECT count(*) FROM credential", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
        db.execute(
            "ALTER TABLE credential RENAME COLUMN active TO unsupported",
            [],
        )
        .unwrap();
        assert!(
            read(&auth)
                .unwrap_err()
                .to_string()
                .contains("unsupported SQLite schema")
        );
    }

    #[test]
    fn active_key_hides_old_oauth_and_metadata_round_trips() {
        let value = json!({"type":"oauth","methodID":"chatgpt-headless","access":"a","refresh":"r","expires":0,"metadata":{"accountID":"acct","plugin":{"keep":true}}});
        let legacy = legacy_entry("cred_native", &value).unwrap();
        assert_eq!(native_entry("openai", &legacy).unwrap(), value);
        let temp = tempfile::tempdir().unwrap();
        let auth = temp.path().join("auth.json");
        let db = fixture(&auth);
        db.execute(
            "INSERT INTO credential VALUES ('oauth','openai','OAuth',?1,0,1,1)",
            [value.to_string()],
        )
        .unwrap();
        db.execute(
            "INSERT INTO credential VALUES ('key','openai','API',?1,1,2,2)",
            [json!({"type":"key","key":"active-key"}).to_string()],
        )
        .unwrap();
        assert_eq!(read(&auth).unwrap()["openai"]["type"], "key");
        assert!(write(&auth, "openai", None).is_err());
        assert_eq!(read(&auth).unwrap()["openai"]["key"], "active-key");
    }
}
