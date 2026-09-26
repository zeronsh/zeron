use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use tokio_util::sync::CancellationToken;
use zeron_doc::SessionDoc;
use zeron_sync::DocsStore;

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OpenFlags, params};
use serde_json::Value;
use zeron_doc::{MessagePart, MessageRole, MessageStatus, SessionMessageEntry};

use crate::workspace_host::WorkspaceHost;
use crate::{EngineError, new_id};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Source {
    Codex,
    Claude,
    Cursor,
    Opencode,
}

impl Source {
    fn prefix(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
            Self::Cursor => "cursor",
            Self::Opencode => "opencode",
        }
    }

    fn harness(self) -> zeron_proto::HarnessId {
        match self {
            Self::Codex => zeron_proto::HarnessId::Codex,
            Self::Claude => zeron_proto::HarnessId::ClaudeCode,
            Self::Cursor => zeron_proto::HarnessId::Cursor,
            Self::Opencode => zeron_proto::HarnessId::Opencode,
        }
    }
}

struct ImportedSession {
    source: Source,
    id: String,
    cwd: String,
    title: Option<String>,
    model: Option<String>,
    entries: Vec<SessionMessageEntry>,
    last_message_at: Option<i64>,
}

pub fn import(
    codex_home: &Path,
    claude_home: &Path,
    cursor_home: &Path,
    opencode_db: &Path,
    workspace: &WorkspaceHost,
    store: &DocsStore,
    device_id: &str,
    cancel: &CancellationToken,
) -> Result<usize, EngineError> {
    let mut imported = 0;
    let mut files = Vec::new();
    collect_jsonl(&codex_home.join("sessions"), &mut files, Source::Codex)?;
    collect_jsonl(&claude_home.join("projects"), &mut files, Source::Claude)?;
    collect_jsonl(&cursor_home.join("projects"), &mut files, Source::Cursor)?;
    files.sort_by(|a, b| a.0.cmp(&b.0));
    for (path, source) in files {
        if cancel.is_cancelled() {
            break;
        }
        let parsed = match source {
            Source::Codex => parse_codex(&path),
            Source::Claude => parse_claude(&path),
            Source::Cursor => parse_cursor(&path),
            Source::Opencode => unreachable!(),
        };
        match parsed {
            Ok(Some(session)) => {
                imported += persist(session, workspace, store, device_id)?;
            }
            Ok(None) => {}
            Err(error) => {
                tracing::warn!(source = source.prefix(), %error, "Could not import agent transcript")
            }
        }
    }
    if opencode_db.is_file() && !cancel.is_cancelled() {
        import_opencode(opencode_db, cancel, |session| {
            imported += persist(session, workspace, store, device_id)?;
            Ok(())
        })?;
    }
    Ok(imported)
}

fn persist(
    session: ImportedSession,
    workspace: &WorkspaceHost,
    store: &DocsStore,
    device_id: &str,
) -> Result<usize, EngineError> {
    if session.entries.is_empty() {
        return Ok(0);
    }
    let chat_id = format!("{}-{}", session.source.prefix(), session.id);
    if workspace.chat(&chat_id)?.is_some() {
        return Ok(0);
    }
    if workspace.read_chats()?.iter().any(|chat| {
        chat.device_id == device_id
            && chat
                .config
                .as_ref()
                .is_some_and(|config| config.harness == session.source.harness())
            && chat.harness_session_id.as_deref() == Some(&session.id)
    }) {
        return Ok(0);
    }
    let space_id = match workspace
        .read_spaces()?
        .into_iter()
        .find(|space| space.device_id == device_id && space.path == session.cwd)
    {
        Some(space) => space.id,
        None => {
            let id = format!("import-space-{:x}", Sha256::digest(session.cwd.as_bytes()));
            workspace.create_space(&id, device_id, &session.cwd, None, false)?;
            id
        }
    };
    let doc = SessionDoc::init(&chat_id)?;
    doc.doc()
        .get_map("meta")
        .insert("epoch", crate::chat2_host::CHAT2_DOC_EPOCH as i64)
        .map_err(zeron_doc::DocError::from)?;
    for entry in &session.entries {
        let mut entry = entry.clone();
        entry.device_id = device_id.to_owned();
        doc.push_message(&entry)?;
    }
    let created_at = session
        .entries
        .first()
        .and_then(|entry| DateTime::from_timestamp_millis(entry.created_at))
        .unwrap_or_else(Utc::now);
    let resume = (session.source != Source::Cursor).then_some(session.id);
    store.save_snapshot_with_cursor(
        &chat_id,
        &doc.export_snapshot()?,
        0,
        crate::chat2_host::CHAT2_DOC_EPOCH,
    )?;
    workspace.import_chat_row(&zeron_proto::Chat {
        id: chat_id,
        device_id: device_id.to_owned(),
        title: session.title,
        archived: false,
        cwd: Some(session.cwd.clone()),
        branch: None,
        checkout_id: None,
        source_context: None,
        config: Some(zeron_proto::ChatConfig {
            harness: session.source.harness(),
            model: session.model,
            reasoning: None,
            model_options: Default::default(),
            sandbox: zeron_proto::SandboxLevel::WorkspaceWrite,
        }),
        last_message_preview: None,
        last_message_at: session
            .last_message_at
            .and_then(DateTime::from_timestamp_millis),
        created_at,
        harness_session_id: resume.clone(),
        harness_session_cwd: resume.map(|_| session.cwd),
        space_id: Some(space_id),
        last_seen_at: None,
        room_gen: Some(crate::chat2_host::CHAT2_DOC_EPOCH),
        parent_chat_id: None,
    })?;
    Ok(1)
}

fn collect_jsonl(
    dir: &Path,
    files: &mut Vec<(PathBuf, Source)>,
    source: Source,
) -> Result<(), EngineError> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    for entry in entries {
        let entry = entry?;
        let kind = entry.file_type()?;
        let path = entry.path();
        if kind.is_dir() {
            if ["subagents", "tool-results", "memory", "mcps"]
                .iter()
                .any(|name| entry.file_name() == *name)
            {
                continue;
            }
            collect_jsonl(&path, files, source)?;
        } else if kind.is_file() && path.extension().is_some_and(|ext| ext == "jsonl") {
            if source != Source::Cursor
                || path.ancestors().any(|parent| {
                    parent
                        .file_name()
                        .is_some_and(|name| name == "agent-transcripts")
                })
            {
                files.push((path, source));
            }
        }
    }
    Ok(())
}

fn json_lines(path: &Path) -> Result<Vec<Value>, EngineError> {
    let mut values = Vec::new();
    let mut reader = BufReader::new(std::fs::File::open(path)?);
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            break;
        }
        match serde_json::from_str(&line) {
            Ok(value) => values.push(value),
            Err(_) if !line.ends_with('\n') => break,
            Err(error) => {
                return Err(EngineError::Other(format!(
                    "Invalid transcript JSON: {error}"
                )));
            }
        }
    }
    Ok(values)
}

fn parse_codex(path: &Path) -> Result<Option<ImportedSession>, EngineError> {
    let values = json_lines(path)?;
    let Some(meta) = values.iter().find(|value| value["type"] == "session_meta") else {
        return Ok(None);
    };
    if meta.pointer("/payload/source/subagent").is_some() {
        return Ok(None);
    }
    let Some(id) = meta
        .pointer("/payload/id")
        .or_else(|| meta.pointer("/payload/session_id"))
        .and_then(Value::as_str)
    else {
        return Ok(None);
    };
    let Some(cwd) = meta.pointer("/payload/cwd").and_then(Value::as_str) else {
        return Ok(None);
    };
    let completed = values.iter().any(|value| {
        value.pointer("/payload/type").and_then(Value::as_str) == Some("item_completed")
            && role(value.pointer("/payload/item/type").and_then(Value::as_str)).is_some()
    });
    let legacy = values.iter().any(|value| {
        value["type"] == "event_msg"
            && matches!(
                value.pointer("/payload/type").and_then(Value::as_str),
                Some("user_message" | "agent_message")
            )
    });
    let mut entries = Vec::new();
    let mut title = None;
    let mut last_message_at = None;
    let mut seen = HashSet::new();
    for value in &values {
        let (role, text, item_id) = if completed {
            if value["type"] != "event_msg" || value["payload"]["type"] != "item_completed" {
                continue;
            }
            let item = &value["payload"]["item"];
            let Some(role) = role(item["type"].as_str()) else {
                continue;
            };
            (role, text_content(&item["content"]), item["id"].as_str())
        } else if legacy {
            if value["type"] != "event_msg" {
                continue;
            }
            let role = match value["payload"]["type"].as_str() {
                Some("user_message") => MessageRole::User,
                Some("agent_message") => MessageRole::Assistant,
                _ => continue,
            };
            (
                role,
                value["payload"]["message"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
                None,
            )
        } else {
            if value["type"] != "response_item" || value["payload"]["type"] != "message" {
                continue;
            }
            let item = &value["payload"];
            let Some(role) = role(item["role"].as_str()) else {
                continue;
            };
            (role, text_content(&item["content"]), item["id"].as_str())
        };
        if text.trim().is_empty() || item_id.is_some_and(|id| !seen.insert(id)) {
            continue;
        }
        let timestamp = value["timestamp"]
            .as_str()
            .and_then(parse_time)
            .unwrap_or(file_time(path)?);
        title = first_title(title, role, &text);
        last_message_at = max_time(last_message_at, timestamp);
        entries.push(text_entry(role, text, timestamp));
    }
    Ok(Some(ImportedSession {
        source: Source::Codex,
        id: id.to_owned(),
        cwd: cwd.to_owned(),
        title,
        model: None,
        entries,
        last_message_at,
    }))
}

fn file_time(path: &Path) -> Result<i64, EngineError> {
    Ok(DateTime::<Utc>::from(std::fs::metadata(path)?.modified()?).timestamp_millis())
}

fn parse_claude(path: &Path) -> Result<Option<ImportedSession>, EngineError> {
    let values = json_lines(path)?;
    let mut id = None;
    let mut cwd = None;
    let mut title = None;
    let mut entries = Vec::new();
    let mut last_message_at = None;
    let mut seen = HashSet::new();
    for value in values {
        if value
            .get("uuid")
            .and_then(Value::as_str)
            .is_some_and(|id| !seen.insert(id.to_owned()))
        {
            continue;
        }
        if value.get("isSidechain").and_then(Value::as_bool) == Some(true) {
            return Ok(None);
        }
        {
            id = value
                .get("sessionId")
                .or_else(|| value.get("session_id"))
                .and_then(Value::as_str)
                .map(str::to_owned)
                .or(id);
            cwd = value
                .get("cwd")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .or(cwd);
        }
        let Some(role) = role(
            value
                .get("type")
                .and_then(Value::as_str)
                .or_else(|| value.pointer("/message/role").and_then(Value::as_str)),
        ) else {
            continue;
        };
        let text = text_content(value.pointer("/message/content").unwrap_or(&Value::Null));
        if text.trim().is_empty() {
            continue;
        }
        let timestamp = value
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(parse_time)
            .unwrap_or(file_time(path)?);
        title = first_title(title, role, &text);
        last_message_at = max_time(last_message_at, timestamp);
        entries.push(text_entry(role, text, timestamp));
    }
    Ok(Some(ImportedSession {
        source: Source::Claude,
        id: session_id(id, path),
        cwd: cwd.unwrap_or_else(|| "~".into()),
        title,
        model: None,
        entries,
        last_message_at,
    }))
}

fn parse_cursor(path: &Path) -> Result<Option<ImportedSession>, EngineError> {
    let values = json_lines(path)?;
    let mut id = None;
    let mut title = None;
    let mut entries = Vec::new();
    let mut last_message_at = None;
    for value in values {
        let Some(role) = role(value.get("role").and_then(Value::as_str)) else {
            continue;
        };
        id = id.or_else(|| {
            value
                .get("session_id")
                .and_then(Value::as_str)
                .map(str::to_owned)
        });
        let text = text_content(value.pointer("/message/content").unwrap_or(&Value::Null));
        if text.trim().is_empty() {
            continue;
        }
        let timestamp = value
            .get("timestamp")
            .and_then(epoch_time)
            .unwrap_or(file_time(path)?);
        title = first_title(title, role, &text);
        last_message_at = max_time(last_message_at, timestamp);
        entries.push(text_entry(role, text, timestamp));
    }
    Ok(Some(ImportedSession {
        source: Source::Cursor,
        id: session_id(id, path),
        cwd: cursor_cwd(path)?.unwrap_or_else(|| "~".into()),
        title,
        model: None,
        entries,
        last_message_at,
    }))
}

fn import_opencode(
    path: &Path,
    cancel: &CancellationToken,
    mut emit: impl FnMut(ImportedSession) -> Result<(), EngineError>,
) -> Result<(), EngineError> {
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|error| EngineError::Other(format!("OpenCode database: {error}")))?;
    connection
        .busy_timeout(std::time::Duration::from_secs(2))
        .map_err(|error| EngineError::Other(error.to_string()))?;
    connection
        .execute_batch("BEGIN")
        .map_err(|error| EngineError::Other(error.to_string()))?;
    let mut query = connection
        .prepare("SELECT id, directory, title, model, time_updated FROM session WHERE parent_id IS NULL AND time_archived IS NULL ORDER BY time_updated, id")
        .map_err(|error| EngineError::Other(format!("OpenCode sessions: {error}")))?;
    let rows = query
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, i64>(4)?,
            ))
        })
        .map_err(|error| EngineError::Other(format!("OpenCode session rows: {error}")))?;
    for row in rows {
        if cancel.is_cancelled() {
            break;
        }
        let (id, cwd, title, model, updated) =
            row.map_err(|error| EngineError::Other(format!("OpenCode session row: {error}")))?;
        let mut entries = Vec::new();
        let mut messages = connection
            .prepare("SELECT id, data, time_created FROM message WHERE session_id = ?1 ORDER BY time_created, id")
            .map_err(|error| EngineError::Other(format!("OpenCode messages: {error}")))?;
        let message_rows = messages
            .query_map(params![id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            })
            .map_err(|error| EngineError::Other(format!("OpenCode message rows: {error}")))?;
        for message in message_rows {
            let (message_id, raw, timestamp) = message
                .map_err(|error| EngineError::Other(format!("OpenCode message row: {error}")))?;
            let data: Value = serde_json::from_str(&raw).map_err(zeron_doc::DocError::from)?;
            let Some(role) = role(data.get("role").and_then(Value::as_str)) else {
                continue;
            };
            let mut parts = Vec::new();
            let mut part_rows = connection
                .prepare("SELECT data FROM part WHERE message_id = ?1 ORDER BY time_created, id")
                .map_err(|error| EngineError::Other(format!("OpenCode parts: {error}")))?;
            let parts_iter = part_rows
                .query_map(params![message_id], |row| row.get::<_, String>(0))
                .map_err(|error| EngineError::Other(format!("OpenCode part rows: {error}")))?;
            for part in parts_iter {
                let raw_part = part
                    .map_err(|error| EngineError::Other(format!("OpenCode part row: {error}")))?;
                let data: Value =
                    serde_json::from_str(&raw_part).map_err(zeron_doc::DocError::from)?;
                match data.get("type").and_then(Value::as_str) {
                    Some("text") => {
                        if let Some(text) = data
                            .get("text")
                            .and_then(Value::as_str)
                            .filter(|text| !text.trim().is_empty())
                        {
                            parts.push(MessagePart::Text {
                                id: format!("text-{}", parts.len()),
                                text: text.to_owned(),
                            });
                        }
                    }
                    Some("reasoning") => {
                        if let Some(text) = data
                            .get("text")
                            .and_then(Value::as_str)
                            .filter(|text| !text.trim().is_empty())
                        {
                            parts.push(MessagePart::Reasoning {
                                id: format!("reasoning-{}", parts.len()),
                                text: text.to_owned(),
                            });
                        }
                    }
                    _ => {}
                }
            }
            if !parts.is_empty() {
                entries.push(SessionMessageEntry {
                    id: new_id(),
                    role,
                    parts,
                    created_at: timestamp,
                    device_id: String::new(),
                    status: Some(MessageStatus::Complete),
                    continuation_of: None,
                    duration_ms: None,
                });
            }
        }
        if !entries.is_empty() {
            emit(ImportedSession {
                source: Source::Opencode,
                id,
                cwd,
                title: Some(title),
                model,
                entries,
                last_message_at: Some(updated),
            })?;
        }
    }
    Ok(())
}

fn role(value: Option<&str>) -> Option<MessageRole> {
    match value {
        Some("user") | Some("UserMessage") => Some(MessageRole::User),
        Some("assistant") | Some("AgentMessage") => Some(MessageRole::Assistant),
        _ => None,
    }
}

fn text_content(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Array(values) => values
            .iter()
            .filter_map(|value| {
                if value
                    .get("type")
                    .and_then(Value::as_str)
                    .is_some_and(|kind| {
                        !matches!(kind, "text" | "Text" | "input_text" | "output_text")
                    })
                {
                    return None;
                }
                value
                    .get("text")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .or_else(|| value.as_str().map(str::to_owned))
            })
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

fn text_entry(role: MessageRole, text: String, timestamp: i64) -> SessionMessageEntry {
    SessionMessageEntry {
        id: new_id(),
        role,
        parts: vec![MessagePart::Text {
            id: "text".into(),
            text,
        }],
        created_at: timestamp,
        device_id: String::new(),
        status: Some(MessageStatus::Complete),
        continuation_of: None,
        duration_ms: None,
    }
}

fn first_title(title: Option<String>, role: MessageRole, text: &str) -> Option<String> {
    title.or_else(|| {
        (role == MessageRole::User).then(|| {
            text.lines()
                .next()
                .unwrap_or("Imported chat")
                .chars()
                .take(80)
                .collect()
        })
    })
}

fn max_time(previous: Option<i64>, current: i64) -> Option<i64> {
    Some(previous.unwrap_or(current).max(current))
}

fn parse_time(value: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|time| time.with_timezone(&Utc).timestamp_millis())
}

fn epoch_time(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_u64().map(|value| value as i64))
        .or_else(|| value.as_str().and_then(parse_time))
}

fn session_id(id: Option<String>, path: &Path) -> String {
    id.unwrap_or_else(|| {
        path.file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or_default()
            .to_owned()
    })
}

fn cursor_cwd(path: &Path) -> Result<Option<String>, EngineError> {
    let Some(projects) = path
        .ancestors()
        .find(|parent| parent.file_name().is_some_and(|name| name == "projects"))
    else {
        return Ok(None);
    };
    let Some(encoded) = path
        .strip_prefix(projects)
        .ok()
        .and_then(|path| path.components().next())
        .and_then(|component| component.as_os_str().to_str())
    else {
        return Ok(None);
    };
    let home = crate::repos::home_dir();
    let data = if cfg!(windows) {
        std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join("AppData/Roaming"))
    } else if cfg!(target_os = "macos") {
        home.join("Library/Application Support")
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"))
    };
    let root = data.join("Cursor/User/workspaceStorage");
    let directories = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    for entry in directories {
        let file = entry?.path().join("workspace.json");
        if !file.is_file() {
            continue;
        }
        let value: Value =
            serde_json::from_slice(&std::fs::read(file)?).map_err(zeron_doc::DocError::from)?;
        let Some(folder) = value["folder"]
            .as_str()
            .and_then(|value| reqwest::Url::parse(value).ok())
            .and_then(|url| url.to_file_path().ok())
        else {
            continue;
        };
        let folder = folder.to_string_lossy().into_owned();
        let key: String = folder
            .chars()
            .map(|ch| {
                if ch.is_alphanumeric() || ch == '-' {
                    ch
                } else {
                    '-'
                }
            })
            .collect();
        let compact = key
            .split('-')
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("-");
        if key.eq_ignore_ascii_case(encoded) || compact.eq_ignore_ascii_case(encoded) {
            return Ok(Some(folder));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imports_claude_jsonl() {
        let path = std::env::temp_dir().join(format!("zeron-claude-{}.jsonl", std::process::id()));
        std::fs::write(&path, concat!(
            "{\"type\":\"system\",\"session_id\":\"claude-1\",\"cwd\":\"C:\\\\work\"}\n",
            "{\"type\":\"user\",\"timestamp\":\"2026-01-01T00:00:00Z\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"hello\"}]}}\n"
        )).unwrap();
        let session = parse_claude(&path).unwrap().unwrap();
        std::fs::remove_file(path).unwrap();
        assert_eq!(session.id, "claude-1");
        assert_eq!(session.entries.len(), 1);
    }

    #[test]
    fn imports_cursor_jsonl() {
        let path = std::env::temp_dir().join(format!("zeron-cursor-{}.jsonl", std::process::id()));
        std::fs::write(&path, "{\"role\":\"user\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"hello\"}]}}\n").unwrap();
        let session = parse_cursor(&path).unwrap().unwrap();
        std::fs::remove_file(path).unwrap();
        assert_eq!(session.entries.len(), 1);
    }

    #[test]
    fn imports_codex_jsonl() {
        let path = std::env::temp_dir().join(format!("zeron-codex-{}.jsonl", std::process::id()));
        std::fs::write(&path, concat!(
            "{\"type\":\"session_meta\",\"payload\":{\"session_id\":\"codex-1\",\"cwd\":\"C:\\\\work\"}}\n",
            "{\"timestamp\":\"2026-01-01T00:00:00Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"item_completed\",\"item\":{\"type\":\"UserMessage\",\"content\":[{\"type\":\"Text\",\"text\":\"hello\"}]}}}\n"
        )).unwrap();
        let session = parse_codex(&path).unwrap().unwrap();
        std::fs::remove_file(path).unwrap();
        assert_eq!(session.id, "codex-1");
        assert_eq!(session.entries.len(), 1);
    }

    #[test]
    fn opencode_db_imports_text_parts() {
        let path = std::env::temp_dir().join(format!("zeron-opencode-{}.db", std::process::id()));
        let connection = Connection::open(&path).unwrap();
        connection.execute_batch("CREATE TABLE session (id TEXT PRIMARY KEY, directory TEXT NOT NULL, title TEXT NOT NULL, model TEXT, time_updated INTEGER, parent_id TEXT, time_archived INTEGER); CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER, data TEXT); CREATE TABLE part (id TEXT PRIMARY KEY, message_id TEXT, time_created INTEGER, data TEXT);") .unwrap();
        connection
            .execute(
                "INSERT INTO session VALUES ('s1','C:/work','title',NULL,1,NULL,NULL)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO message VALUES ('m1','s1',1,'{\"role\":\"user\"}')",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO part VALUES ('p1','m1',1,'{\"type\":\"text\",\"text\":\"hello\"}')",
                [],
            )
            .unwrap();
        drop(connection);
        let cancel = CancellationToken::new();
        let mut count = 0;
        import_opencode(&path, &cancel, |session| {
            count += session.entries.len();
            Ok(())
        })
        .unwrap();
        std::fs::remove_file(path).unwrap();
        assert_eq!(count, 1);
    }
}
