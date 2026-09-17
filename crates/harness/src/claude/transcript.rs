//! Claude Code's own on-disk transcripts (`<config>/projects/<slug>/<id>.jsonl`), read for import.

use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use zeron_proto::AgentEvent;

use super::normalize::decode_tool_use;
use super::wire::ContentBlock;

/// Head budget for discovery: enough to reach the first record carrying a cwd and prompt.
const HEAD_PROBE_BYTES: u64 = 256 * 1024;
/// Tail budget for discovery: the title records cluster at the end of a transcript.
const TAIL_PROBE_BYTES: u64 = 64 * 1024;
/// Discovery skips a line longer than this; a title or cwd never is.
const MAX_PROBE_LINE: usize = 1024 * 1024;
/// Fallback titles are cut here, matching the engine's own title fallback.
const FALLBACK_TITLE_CHARS: usize = 48;

/// Root of the transcript store under a Claude config dir.
pub fn projects_dir(config_dir: &Path) -> PathBuf {
    config_dir.join("projects")
}

/// One importable transcript, as listed by [`discover`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredSession {
    pub session_id: String,
    pub path: PathBuf,
    /// Working directory the session ran in, read from the records themselves.
    pub cwd: String,
    pub title: String,
    pub git_branch: Option<String>,
    /// Epoch millis of the first timestamped record.
    pub started_ms: Option<i64>,
    /// Epoch millis of the file's last write.
    pub modified_ms: i64,
    pub size_bytes: u64,
}

/// One imported event with the wall-clock time it happened.
#[derive(Debug, Clone)]
pub struct TranscriptEvent {
    pub at_ms: i64,
    pub event: AgentEvent,
}

/// A fully read transcript, ready for the engine to fold into a chat doc.
#[derive(Debug, Clone)]
pub struct ImportedTranscript {
    pub session_id: String,
    pub cwd: String,
    pub title: Option<String>,
    pub git_branch: Option<String>,
    pub started_ms: Option<i64>,
    pub last_ms: Option<i64>,
    pub events: Vec<TranscriptEvent>,
    /// Records skipped because they were unreadable JSON.
    pub malformed_lines: usize,
}

/// List every importable transcript under a Claude config dir, newest first.
pub fn discover(config_dir: &Path) -> Vec<DiscoveredSession> {
    let root = projects_dir(config_dir);
    let Ok(projects) = std::fs::read_dir(&root) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for project in projects.flatten() {
        if !project.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(project.path()) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|e| e != "jsonl") {
                continue;
            }
            if let Some(found) = probe(&path) {
                out.push(found);
            }
        }
    }
    out.sort_by_key(|s| std::cmp::Reverse(s.modified_ms));
    out
}

/// Summarize one transcript by reading only its head and tail.
fn probe(path: &Path) -> Option<DiscoveredSession> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() == 0 {
        return None;
    }
    let modified_ms = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or_default();

    let mut cwd = None;
    let mut git_branch = None;
    let mut started_ms = None;
    let mut first_prompt = None;
    for value in probe_head(path)? {
        if cwd.is_none()
            && let Some(found) = value.get("cwd").and_then(Value::as_str)
            && !found.is_empty()
        {
            cwd = Some(found.to_string());
            git_branch = value
                .get("gitBranch")
                .and_then(Value::as_str)
                .filter(|b| !b.is_empty())
                .map(str::to_string);
        }
        if started_ms.is_none() {
            started_ms = value.get("timestamp").and_then(parse_timestamp);
        }
        if first_prompt.is_none()
            && is_user_prompt(&value)
            && let Some(text) = user_prompt_text(&value)
        {
            first_prompt = Some(text);
        }
        if cwd.is_some() && started_ms.is_some() && first_prompt.is_some() {
            break;
        }
    }

    let session_id = path.file_stem()?.to_string_lossy().to_string();
    let title = probe_title(path)
        .or_else(|| first_prompt.as_deref().map(summarize))
        .unwrap_or_else(|| session_id.clone());
    Some(DiscoveredSession {
        session_id,
        path: path.to_path_buf(),
        cwd: cwd?,
        title,
        git_branch,
        started_ms,
        modified_ms,
        size_bytes: meta.len(),
    })
}

/// Parse the leading records of a transcript, bounded by [`HEAD_PROBE_BYTES`].
fn probe_head(path: &Path) -> Option<Vec<Value>> {
    let file = File::open(path).ok()?;
    let mut reader = BufReader::new(file.take(HEAD_PROBE_BYTES));
    Some(collect_values(&mut reader))
}

/// The newest title a transcript names, read from its tail.
fn probe_title(path: &Path) -> Option<String> {
    let mut file = File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let from = len.saturating_sub(TAIL_PROBE_BYTES);
    file.seek(SeekFrom::Start(from)).ok()?;
    let mut reader = BufReader::new(file);
    if from > 0 {
        let mut partial = Vec::new();
        reader.read_until(b'\n', &mut partial).ok()?;
    }
    let mut custom = None;
    let mut ai = None;
    for value in collect_values(&mut reader) {
        match value.get("type").and_then(Value::as_str) {
            Some("custom-title") => {
                custom = value
                    .get("customTitle")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            }
            Some("ai-title") => {
                ai = value
                    .get("aiTitle")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            }
            _ => {}
        }
    }
    custom.or(ai).filter(|t| !t.trim().is_empty())
}

/// Read whole JSON lines from a bounded reader, dropping partial or oversized ones.
fn collect_values(reader: &mut impl BufRead) -> Vec<Value> {
    let mut out = Vec::new();
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        if !line.ends_with('\n') {
            break;
        }
        if line.len() > MAX_PROBE_LINE {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<Value>(line.trim_end()) {
            out.push(value);
        }
    }
    out
}

/// Read a whole transcript into the events an import needs.
pub fn read_transcript(path: &Path) -> std::io::Result<ImportedTranscript> {
    let file = File::open(path)?;
    let mut reader = BufReader::with_capacity(256 * 1024, file);
    let session_id = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();

    let mut out = ImportedTranscript {
        session_id,
        cwd: String::new(),
        title: None,
        git_branch: None,
        started_ms: None,
        last_ms: None,
        events: Vec::new(),
        malformed_lines: 0,
    };
    let mut custom_title = None;
    let mut ai_title = None;
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            break;
        }
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(trimmed) else {
            out.malformed_lines += 1;
            continue;
        };
        if out.cwd.is_empty()
            && let Some(cwd) = value.get("cwd").and_then(Value::as_str)
            && !cwd.is_empty()
        {
            out.cwd = cwd.to_string();
            out.git_branch = value
                .get("gitBranch")
                .and_then(Value::as_str)
                .filter(|b| !b.is_empty())
                .map(str::to_string);
        }
        let at_ms = value.get("timestamp").and_then(parse_timestamp);
        if let Some(ms) = at_ms {
            out.started_ms.get_or_insert(ms);
            out.last_ms = Some(ms);
        }
        match value.get("type").and_then(Value::as_str) {
            Some("custom-title") => {
                custom_title = value
                    .get("customTitle")
                    .and_then(Value::as_str)
                    .map(str::to_string);
            }
            Some("ai-title") => {
                ai_title = value
                    .get("aiTitle")
                    .and_then(Value::as_str)
                    .map(str::to_string);
            }
            Some("user") | Some("assistant") => {
                let at_ms = at_ms.or(out.last_ms).unwrap_or_default();
                for event in record_events(&value) {
                    out.events.push(TranscriptEvent { at_ms, event });
                }
            }
            _ => {}
        }
    }
    out.title = custom_title.or(ai_title).filter(|t| !t.trim().is_empty());
    Ok(out)
}

/// Map one `user`/`assistant` record to the events it contributes.
fn record_events(value: &Value) -> Vec<AgentEvent> {
    let kind = value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if value.get("isSidechain").and_then(Value::as_bool) == Some(true) {
        return Vec::new();
    }
    if kind == "user" && is_user_prompt(value) {
        return match user_prompt_text(value) {
            Some(text) => vec![AgentEvent::UserMessage { text }],
            None => Vec::new(),
        };
    }
    blocks(value)
        .filter_map(|block| match block.kind.as_str() {
            "text" if !block.text.trim().is_empty() => {
                Some(AgentEvent::TextDelta { text: block.text })
            }
            "thinking" if !block.thinking.trim().is_empty() => Some(AgentEvent::ReasoningDelta {
                text: block.thinking,
            }),
            "tool_use" => Some(AgentEvent::ToolCall {
                id: block.id.clone(),
                call: decode_tool_use(&block.name, &block.input),
            }),
            "tool_result" => Some(AgentEvent::ToolResult {
                id: block.tool_use_id.clone(),
                is_error: block.is_error.unwrap_or(false),
                output: None,
                diff: None,
            }),
            _ => None,
        })
        .collect()
}

/// Content blocks of a record, tolerant of the string form a plain prompt uses.
fn blocks(value: &Value) -> impl Iterator<Item = ContentBlock> + '_ {
    value
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(Value::as_array)
        .map(|a| a.as_slice())
        .unwrap_or_default()
        .iter()
        .filter_map(|b| serde_json::from_value::<ContentBlock>(b.clone()).ok())
}

/// Whether a user record is something a person actually sent.
fn is_user_prompt(value: &Value) -> bool {
    if value.get("type").and_then(Value::as_str) != Some("user") {
        return false;
    }
    match value.get("promptSource").and_then(Value::as_str) {
        Some("typed") | Some("queued") | Some("suggestion_accepted") | Some("sdk") => true,
        Some(_) => false,
        // Transcripts written before promptSource existed: a string body is the prompt.
        None => value
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(Value::as_str)
            .is_some_and(|text| !is_synthetic(text)),
    }
}

/// The prompt text of a user record, whether it is a string or text blocks.
fn user_prompt_text(value: &Value) -> Option<String> {
    let content = value.get("message").and_then(|m| m.get("content"))?;
    let text = match content {
        Value::String(text) => text.clone(),
        Value::Array(_) => blocks(value)
            .filter(|b| b.kind == "text")
            .map(|b| b.text)
            .collect::<Vec<_>>()
            .join("\n"),
        _ => return None,
    };
    let text = text.trim();
    (!text.is_empty() && !is_synthetic(text)).then(|| text.to_string())
}

/// CLI-synthesized text that rides a user record but is not conversation.
fn is_synthetic(text: &str) -> bool {
    let text = text.trim_start();
    text.starts_with("<system-reminder>")
        || text.starts_with("<task-notification>")
        || text.starts_with("[Request interrupted")
}

/// Cut a prompt down to a sidebar-sized title.
fn summarize(prompt: &str) -> String {
    let line = prompt.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    let line = line.trim();
    if line.chars().count() <= FALLBACK_TITLE_CHARS {
        return line.to_string();
    }
    let cut: String = line.chars().take(FALLBACK_TITLE_CHARS).collect();
    format!("{}…", cut.trim_end())
}

/// Parse an ISO-8601 transcript timestamp into epoch millis.
fn parse_timestamp(value: &Value) -> Option<i64> {
    let raw = value.as_str()?;
    chrono_parse(raw)
}

/// Minimal RFC-3339 parse: the CLI always writes `YYYY-MM-DDTHH:MM:SS(.sss)Z`.
fn chrono_parse(raw: &str) -> Option<i64> {
    let (date, rest) = raw.split_once('T')?;
    let mut date = date.split('-');
    let year: i64 = date.next()?.parse().ok()?;
    let month: i64 = date.next()?.parse().ok()?;
    let day: i64 = date.next()?.parse().ok()?;
    let time = rest.trim_end_matches('Z');
    let (time, millis) = match time.split_once('.') {
        Some((time, frac)) => {
            let frac: String = frac.chars().take(3).collect();
            let scale = 10i64.pow(3 - frac.len() as u32);
            (time, frac.parse::<i64>().ok()? * scale)
        }
        None => (time, 0),
    };
    let mut time = time.split(':');
    let hour: i64 = time.next()?.parse().ok()?;
    let minute: i64 = time.next()?.parse().ok()?;
    let second: i64 = time.next()?.parse().ok()?;
    let days = days_from_civil(year, month, day);
    Some(((days * 24 + hour) * 60 + minute) * 60_000 + second * 1000 + millis)
}

/// Days since the Unix epoch, by Howard Hinnant's civil-date algorithm.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let yoe = year - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, lines: &[&str]) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, format!("{}\n", lines.join("\n"))).expect("write transcript");
        path
    }

    #[test]
    fn civil_dates_match_known_epochs() {
        assert_eq!(chrono_parse("1970-01-01T00:00:00.000Z"), Some(0));
        assert_eq!(
            chrono_parse("2026-09-16T20:27:13.490Z"),
            Some(1789590433490)
        );
        assert_eq!(chrono_parse("2026-09-16T20:27:13Z"), Some(1789590433000));
    }

    #[test]
    fn only_human_prompts_become_user_messages() {
        let typed = serde_json::json!({
            "type": "user", "promptSource": "typed",
            "message": {"role": "user", "content": "hello"}
        });
        let injected = serde_json::json!({
            "type": "user", "promptSource": "system",
            "message": {"role": "user", "content": "<task-notification>done</task-notification>"}
        });
        let legacy = serde_json::json!({
            "type": "user",
            "message": {"role": "user", "content": "from an older CLI"}
        });
        let reminder = serde_json::json!({
            "type": "user",
            "message": {"role": "user", "content": "<system-reminder>context</system-reminder>"}
        });
        assert!(is_user_prompt(&typed));
        assert!(!is_user_prompt(&injected));
        assert!(is_user_prompt(&legacy));
        assert!(!is_user_prompt(&reminder));
    }

    #[test]
    fn assistant_blocks_map_to_events() {
        let record = serde_json::json!({
            "type": "assistant",
            "message": {"role": "assistant", "content": [
                {"type": "thinking", "thinking": "weighing it up", "signature": "sig"},
                {"type": "text", "text": "Here is the answer."},
                {"type": "tool_use", "id": "toolu_1", "name": "Bash",
                 "input": {"command": "ls", "description": "list"}},
            ]}
        });
        let events = record_events(&record);
        assert_eq!(events.len(), 3);
        assert!(
            matches!(&events[0], AgentEvent::ReasoningDelta { text } if text == "weighing it up")
        );
        assert!(
            matches!(&events[1], AgentEvent::TextDelta { text } if text == "Here is the answer.")
        );
        assert!(matches!(&events[2], AgentEvent::ToolCall { id, .. } if id == "toolu_1"));
    }

    #[test]
    fn tool_results_carry_their_error_flag() {
        let record = serde_json::json!({
            "type": "user",
            "message": {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "toolu_1", "is_error": true, "content": "boom"},
            ]}
        });
        let events = record_events(&record);
        assert!(
            matches!(&events[0], AgentEvent::ToolResult { id, is_error, .. } if id == "toolu_1" && *is_error)
        );
    }

    #[test]
    fn sidechain_records_are_left_to_their_own_transcript() {
        let record = serde_json::json!({
            "type": "assistant", "isSidechain": true,
            "message": {"role": "assistant", "content": [{"type": "text", "text": "child"}]}
        });
        assert!(record_events(&record).is_empty());
    }

    #[test]
    fn reads_a_transcript_end_to_end() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write(
            dir.path(),
            "sess-1.jsonl",
            &[
                r#"{"type":"mode","mode":"default"}"#,
                r#"{"type":"user","promptSource":"typed","cwd":"/w/proj","gitBranch":"main","timestamp":"2026-09-16T20:27:13.490Z","message":{"role":"user","content":"fix the test"}}"#,
                r#"{"type":"assistant","timestamp":"2026-09-16T20:27:20.000Z","message":{"role":"assistant","content":[{"type":"text","text":"On it."},{"type":"tool_use","id":"toolu_1","name":"Bash","input":{"command":"cargo test"}}]}}"#,
                r#"{"type":"user","timestamp":"2026-09-16T20:27:25.000Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_1","is_error":false,"content":"ok"}]}}"#,
                r#"{"type":"attachment","attachment":{"type":"total_tokens_reminder"}}"#,
                r#"not json at all"#,
                r#"{"type":"ai-title","aiTitle":"Fixing the flaky test"}"#,
            ],
        );
        let read = read_transcript(&path).expect("read");
        assert_eq!(read.session_id, "sess-1");
        assert_eq!(read.cwd, "/w/proj");
        assert_eq!(read.git_branch.as_deref(), Some("main"));
        assert_eq!(read.title.as_deref(), Some("Fixing the flaky test"));
        assert_eq!(read.malformed_lines, 1);
        assert_eq!(read.started_ms, Some(1789590433490));
        assert_eq!(read.last_ms, Some(1789590445000));
        let kinds: Vec<&str> = read
            .events
            .iter()
            .map(|e| match e.event {
                AgentEvent::UserMessage { .. } => "user",
                AgentEvent::TextDelta { .. } => "text",
                AgentEvent::ToolCall { .. } => "call",
                AgentEvent::ToolResult { .. } => "result",
                _ => "other",
            })
            .collect();
        assert_eq!(kinds, ["user", "text", "call", "result"]);
    }

    #[test]
    fn discovery_summarizes_without_reading_everything() {
        let dir = tempfile::tempdir().expect("tempdir");
        let projects = projects_dir(dir.path()).join("-w-proj");
        std::fs::create_dir_all(&projects).expect("mkdir");
        write(
            &projects,
            "sess-titled.jsonl",
            &[
                r#"{"type":"user","promptSource":"typed","cwd":"/w/proj","timestamp":"2026-09-16T20:27:13.490Z","message":{"role":"user","content":"hello"}}"#,
                r#"{"type":"custom-title","customTitle":"My renamed chat"}"#,
            ],
        );
        write(
            &projects,
            "sess-untitled.jsonl",
            &[
                r#"{"type":"user","promptSource":"typed","cwd":"/w/proj","timestamp":"2026-09-16T20:27:13.490Z","message":{"role":"user","content":"a prompt long enough that the fallback title has to cut it short"}}"#,
            ],
        );
        std::fs::write(projects.join("empty.jsonl"), "").expect("empty");
        let found = discover(dir.path());
        assert_eq!(found.len(), 2, "empty transcripts are not offered");
        let titled = found
            .iter()
            .find(|s| s.session_id == "sess-titled")
            .expect("titled");
        assert_eq!(titled.title, "My renamed chat");
        assert_eq!(titled.cwd, "/w/proj");
        let untitled = found
            .iter()
            .find(|s| s.session_id == "sess-untitled")
            .expect("untitled");
        assert_eq!(
            untitled.title,
            "a prompt long enough that the fallback title has…"
        );
    }
}
