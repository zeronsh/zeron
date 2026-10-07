//! Token usage per provider session, read from the agent CLIs' own transcripts.
//!
//! The live `Usage` events undercount (Claude's leave out cache tokens, Codex
//! reports only the last turn), and they only exist from the moment a chat
//! runs. The CLIs' transcripts hold every turn with its cache split, for
//! sessions that ran before anyone counted. Read-only: the files belong to
//! the CLIs.

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use serde_json::Value;
use zeron_proto::{ChatTokenUsage, HarnessId};

/// The totals one provider session's transcript records; `None` when this
/// harness keeps no readable transcript or the session's file isn't here.
pub fn session_usage(harness: HarnessId, session_id: &str, cwd: &Path) -> Option<ChatTokenUsage> {
    if !plain_id(session_id) {
        return None;
    }
    let home = crate::executable::home_or_current_dir();
    match harness {
        HarnessId::ClaudeCode => {
            let root = crate::model_context::root("CLAUDE_CONFIG_DIR", home.join(".claude"));
            claude(&root.join("projects"), session_id)
        }
        HarnessId::Codex => {
            let root = crate::model_context::root("CODEX_HOME", home.join(".codex"));
            codex(&root, session_id)
        }
        HarnessId::Pi => pi(&crate::pi::session_file(session_id, cwd)?),
        _ => None,
    }
}

/// Session ids are UUID-like; anything else could walk out of the CLI's
/// directory when joined into a path.
fn plain_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn lines(path: &Path) -> impl Iterator<Item = String> {
    std::fs::File::open(path)
        .ok()
        .into_iter()
        .flat_map(|file| BufReader::new(file).lines().map_while(Result::ok))
}

fn count(value: &Value, key: &str) -> u64 {
    value.get(key).and_then(Value::as_u64).unwrap_or(0)
}

/// `projects/<cwd-slug>/<id>.jsonl`, plus its subagents under
/// `projects/<cwd-slug>/<id>/`. Each API response appears once per content
/// block with the same usage, so responses are keyed by message id.
fn claude(projects: &Path, session_id: &str) -> Option<ChatTokenUsage> {
    let file_name = format!("{session_id}.jsonl");
    let slug = std::fs::read_dir(projects)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .find(|dir| dir.join(&file_name).is_file())?;
    let mut files = vec![slug.join(&file_name)];
    collect_jsonl(&slug.join(session_id), 4, &mut files);
    let mut responses: HashMap<String, ChatTokenUsage> = HashMap::new();
    for (ix, line) in files.iter().flat_map(|file| lines(file)).enumerate() {
        if !line.contains("\"usage\"") {
            continue;
        }
        let Ok(record) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let message = &record["message"];
        let usage = &message["usage"];
        if record["type"] != "assistant" || !usage.is_object() {
            continue;
        }
        let id = message["id"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| format!("line-{ix}"));
        responses.insert(
            id,
            ChatTokenUsage {
                input: count(usage, "input_tokens"),
                output: count(usage, "output_tokens"),
                cache_read: count(usage, "cache_read_input_tokens"),
                cache_write: count(usage, "cache_creation_input_tokens"),
            },
        );
    }
    let mut total = ChatTokenUsage::default();
    for usage in responses.into_values() {
        total.add(usage);
    }
    Some(total)
}

/// `sessions/YYYY/MM/DD/rollout-<time>-<id>.jsonl` (or `archived_sessions/`).
/// Its `token_count` events carry a running total, so the last one is the
/// session's. OpenAI counts cached tokens inside `input_tokens` and reports
/// no cache writes.
fn codex(root: &Path, session_id: &str) -> Option<ChatTokenUsage> {
    let suffix = format!("-{session_id}.jsonl");
    let mut files = Vec::new();
    collect_jsonl(&root.join("sessions"), 4, &mut files);
    collect_jsonl(&root.join("archived_sessions"), 1, &mut files);
    let file = files.into_iter().find(|file| {
        file.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with(&suffix))
    })?;
    let mut total = ChatTokenUsage::default();
    for line in lines(&file) {
        if !line.contains("\"token_count\"") {
            continue;
        }
        let Ok(record) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let usage = &record["payload"]["info"]["total_token_usage"];
        if !usage.is_object() {
            continue;
        }
        let cached = count(usage, "cached_input_tokens");
        total = ChatTokenUsage {
            input: count(usage, "input_tokens").saturating_sub(cached),
            output: count(usage, "output_tokens"),
            cache_read: cached,
            cache_write: 0,
        };
    }
    Some(total)
}

/// Pi records each assistant message's usage, cache split included.
fn pi(file: &Path) -> Option<ChatTokenUsage> {
    let mut total = ChatTokenUsage::default();
    for line in lines(file) {
        if !line.contains("\"usage\"") {
            continue;
        }
        let Ok(record) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let message = &record["message"];
        let usage = &message["usage"];
        if record["type"] != "message" || message["role"] != "assistant" || !usage.is_object() {
            continue;
        }
        total.add(ChatTokenUsage {
            input: count(usage, "input"),
            output: count(usage, "output"),
            cache_read: count(usage, "cacheRead"),
            cache_write: count(usage, "cacheWrite"),
        });
    }
    Some(total)
}

/// `.jsonl` files under `dir`, `depth` directory levels deep, no symlinks.
fn collect_jsonl(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if kind.is_dir() && depth > 0 {
            collect_jsonl(&path, depth - 1, out);
        } else if kind.is_file() && path.extension().is_some_and(|ext| ext == "jsonl") {
            out.push(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, lines: &[&str]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, lines.join("\n")).unwrap();
    }

    #[test]
    fn claude_counts_each_response_once_with_its_subagents() {
        let dir = tempfile::tempdir().unwrap();
        let slug = dir.path().join("C--repo");
        let usage = r#""usage":{"input_tokens":10,"output_tokens":5,"cache_read_input_tokens":100,"cache_creation_input_tokens":20}"#;
        write(
            &slug.join("abc-1.jsonl"),
            &[
                &format!(r#"{{"type":"assistant","message":{{"id":"m1",{usage}}}}}"#),
                // The same response's next content block repeats its usage.
                &format!(r#"{{"type":"assistant","message":{{"id":"m1",{usage}}}}}"#),
                &format!(r#"{{"type":"assistant","message":{{"id":"m2",{usage}}}}}"#),
                r#"{"type":"user","message":{"content":"has \"usage\" in it"}}"#,
            ],
        );
        write(
            &slug.join("abc-1/subagents/agent-x.jsonl"),
            &[&format!(
                r#"{{"type":"assistant","message":{{"id":"s1",{usage}}}}}"#
            )],
        );
        let total = claude(dir.path(), "abc-1").unwrap();
        assert_eq!(
            total,
            ChatTokenUsage {
                input: 30,
                output: 15,
                cache_read: 300,
                cache_write: 60
            }
        );
        assert!(claude(dir.path(), "missing").is_none());
    }

    #[test]
    fn codex_takes_the_last_running_total_and_splits_cached_input() {
        let dir = tempfile::tempdir().unwrap();
        let total = |input: u64, cached: u64, output: u64| {
            format!(
                r#"{{"type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{{"input_tokens":{input},"cached_input_tokens":{cached},"output_tokens":{output}}}}}}}}}"#
            )
        };
        write(
            &dir.path()
                .join("sessions/2026/10/03/rollout-2026-10-03T01-02-03-019f-aa.jsonl"),
            &[&total(100, 40, 10), &total(500, 300, 70)],
        );
        assert_eq!(
            codex(dir.path(), "019f-aa"),
            Some(ChatTokenUsage {
                input: 200,
                output: 70,
                cache_read: 300,
                cache_write: 0
            })
        );
        assert!(codex(dir.path(), "019f").is_none(), "ids match whole");
    }

    #[test]
    fn pi_sums_assistant_messages() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("s.jsonl");
        let message = r#"{"type":"message","message":{"role":"assistant","usage":{"input":7,"output":3,"cacheRead":50,"cacheWrite":2}}}"#;
        write(
            &file,
            &[
                message,
                message,
                r#"{"type":"message","message":{"role":"user"}}"#,
            ],
        );
        assert_eq!(
            pi(&file),
            Some(ChatTokenUsage {
                input: 14,
                output: 6,
                cache_read: 100,
                cache_write: 4
            })
        );
    }

    #[test]
    fn ids_that_could_leave_the_directory_are_refused() {
        assert!(plain_id("019f7410-9429-76f3-b9a4-994f151309ab"));
        for id in ["", "../x", "a/b", r"a\b", "a.jsonl"] {
            assert!(!plain_id(id), "{id}");
        }
    }
}
