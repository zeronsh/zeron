//! Codex's stored thread as an editable history.
//!
//! Codex keeps a thread as a rollout, a JSONL file of `{type, payload}`
//! records. What the model reads is the raw Responses API items the rollout
//! holds: the `replacement_history` of the last `compacted` record (if any)
//! followed by every later `response_item`. Everything else (`event_msg`,
//! `turn_context`, `world_state`, token records) is Codex's own bookkeeping.
//!
//! An edit never touches the stored thread. It starts a new thread and
//! injects the edited items (`thread/inject_items`); the chat resumes that
//! one. Codex builds the new thread's initial context (tools, instructions,
//! environment) itself, so that block is left out of what is injected.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use zeron_proto::native_history::{
    NativeEdit, NativeHistoryItem, NativeItemKind, NativeRole, ReasoningSeal,
};

use super::{AppServer, CodexHarness, initialize_params, spawn_app_server};
use crate::jsonrpc::RpcClient;
use crate::{HarnessError, NativeHistoryRead};

/// Longest summary of a tool call's arguments or a tool result's output.
const SUMMARY_CHARS: usize = 160;
/// Longest a history operation may keep its app-server.
const OPERATION_TIMEOUT: Duration = Duration::from_secs(60);

/// Shown when a stored thread moved on since the editor listed it.
const CHANGED: &str = "The conversation changed since you started editing";

/// Shown for the compaction record Codex stores sealed.
const COMPACTED_TEXT: &str = "Earlier conversation, compacted by Codex";

/// Texts that open the context messages Codex injects around the user's own
/// prompts. They are not conversation: hidden from the listing.
const CONTEXT_PREFIXES: &[&str] = &[
    "<environment_context>",
    "<user_instructions>",
    "<INSTRUCTIONS>",
    "# AGENTS.md instructions",
    "<permissions",
    "<collaboration_mode>",
    "<skills",
];

fn refuse(message: &str) -> HarnessError {
    HarnessError::History(message.into())
}

// ── the model-visible items ─────────────────────────────────────────────────

/// What the model reads from a rollout.
pub(super) struct Model {
    pub items: Vec<Value>,
    pub compacted: bool,
}

pub(super) fn model_history(rollout: &str) -> Result<Model, HarnessError> {
    let mut items: Vec<Value> = Vec::new();
    let mut compacted = false;
    for line in rollout.lines() {
        // A torn or unknown line is not history.
        let Ok(record) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let payload = &record["payload"];
        match record["type"].as_str() {
            Some("compacted") => {
                let Some(replacement) = payload["replacement_history"].as_array() else {
                    return Err(refuse(
                        "This conversation was compacted by an older Codex and can't be edited",
                    ));
                };
                items = replacement.clone();
                compacted = true;
            }
            Some("response_item") if payload.is_object() => items.push(payload.clone()),
            _ => {}
        }
    }
    Ok(Model { items, compacted })
}

fn revision(items: &[Value]) -> String {
    let mut hasher = Sha256::new();
    for item in items {
        hasher.update(item.to_string().as_bytes());
        hasher.update(b"\n");
    }
    hasher.finalize()[..16]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

// ── classification ──────────────────────────────────────────────────────────

fn item_type(item: &Value) -> &str {
    item["type"].as_str().unwrap_or("")
}

fn role(item: &Value) -> &str {
    item["role"].as_str().unwrap_or("")
}

/// The text parts of a message, in order.
fn text_parts(item: &Value) -> Vec<&str> {
    match &item["content"] {
        Value::String(text) => vec![text.as_str()],
        Value::Array(parts) => parts
            .iter()
            .filter(|part| matches!(item_type(part), "input_text" | "output_text"))
            .filter_map(|part| part["text"].as_str())
            .collect(),
        _ => Vec::new(),
    }
}

/// Context Codex injects (tools, instructions, environment): not part of
/// what the user said.
fn is_context(item: &Value) -> bool {
    match item_type(item) {
        "additional_tools" => true,
        "message" => match role(item) {
            "developer" | "system" => true,
            "user" => text_parts(item).first().is_some_and(|text| {
                let text = text.trim_start();
                CONTEXT_PREFIXES
                    .iter()
                    .any(|prefix| text.starts_with(prefix))
            }),
            _ => false,
        },
        _ => false,
    }
}

/// The initial context block: the context items before anything else.
fn initial_context_len(items: &[Value]) -> usize {
    items.iter().take_while(|item| is_context(item)).count()
}

// ── listing ─────────────────────────────────────────────────────────────────

struct Listed {
    item: NativeHistoryItem,
    /// Position in the model-visible list.
    index: usize,
}

fn listing(items: &[Value]) -> Vec<Listed> {
    let mut listed = Vec::new();
    let mut seen = HashSet::new();
    let mut prompts: u32 = 0;
    for (index, item) in items.iter().enumerate() {
        if is_context(item) {
            continue;
        }
        let kind = item_type(item);
        let (role, kind, editable, removable) = match kind {
            "message" => {
                let text = text_parts(item).join("\n");
                if text.trim().is_empty() {
                    continue;
                }
                let role = match role(item) {
                    "user" => {
                        prompts += 1;
                        NativeRole::User
                    }
                    "assistant" => NativeRole::Assistant,
                    _ => continue,
                };
                (role, NativeItemKind::Text { text }, true, false)
            }
            "reasoning" => {
                let summary = item["summary"]
                    .as_array()
                    .map(|parts| {
                        parts
                            .iter()
                            .filter_map(|part| part["text"].as_str())
                            .collect::<Vec<_>>()
                            .join("\n")
                    })
                    .unwrap_or_default();
                (
                    NativeRole::Assistant,
                    NativeItemKind::Reasoning {
                        text: (!summary.trim().is_empty()).then_some(summary),
                        seal: ReasoningSeal::Encrypted,
                    },
                    false,
                    true,
                )
            }
            "compaction" => (
                NativeRole::Assistant,
                NativeItemKind::CompactionSummary {
                    text: COMPACTED_TEXT.into(),
                },
                false,
                false,
            ),
            other if other.ends_with("_call_output") || other.ends_with("_output") => (
                NativeRole::User,
                NativeItemKind::ToolResult {
                    summary: clip(&output_text(&item["output"])),
                    is_error: item["success"].as_bool() == Some(false),
                },
                false,
                false,
            ),
            other if other.ends_with("_call") => (
                NativeRole::Assistant,
                NativeItemKind::ToolCall {
                    name: item["name"]
                        .as_str()
                        .unwrap_or_else(|| other.trim_end_matches("_call"))
                        .to_owned(),
                    summary: call_summary(item),
                },
                false,
                false,
            ),
            _ => continue,
        };
        let id = match item["id"].as_str().filter(|id| !id.is_empty()) {
            Some(id) if seen.insert(id.to_owned()) => id.to_owned(),
            _ => format!("idx:{index}"),
        };
        listed.push(Listed {
            item: NativeHistoryItem {
                id,
                role,
                turn: prompts.saturating_sub(1),
                kind,
                editable,
                removable,
            },
            index,
        });
    }
    listed
}

fn clip(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= SUMMARY_CHARS {
        return flat;
    }
    let mut clipped: String = flat.chars().take(SUMMARY_CHARS).collect();
    clipped.push('…');
    clipped
}

/// A tool call's input on one line.
fn call_summary(item: &Value) -> String {
    for key in ["arguments", "input", "action"] {
        match &item[key] {
            Value::Null => {}
            Value::String(text) => {
                // Function arguments are a JSON string: show the command when
                // there is one.
                if let Ok(args) = serde_json::from_str::<Value>(text)
                    && let Some(command) = args.get("command").or_else(|| args.get("cmd"))
                {
                    return clip(&output_text(command));
                }
                return clip(text);
            }
            other => return clip(&output_text(other)),
        }
    }
    String::new()
}

/// Text of an output: a string, a list of text parts, or an array of words
/// (a command line).
fn output_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Array(parts) => {
            if parts.iter().all(Value::is_string) {
                return parts
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(" ");
            }
            parts
                .iter()
                .filter_map(|part| part["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n")
        }
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

pub(super) fn read(rollout: &str) -> Result<NativeHistoryRead, HarnessError> {
    let model = model_history(rollout)?;
    Ok(NativeHistoryRead {
        revision: revision(&model.items),
        items: listing(&model.items)
            .into_iter()
            .map(|listed| listed.item)
            .collect(),
        compacted: model.compacted,
    })
}

// ── edits ───────────────────────────────────────────────────────────────────

/// `rollout`'s history with `edits` applied, as the items to inject into a
/// new thread: the initial context block is left to Codex.
pub(super) fn edited_items(
    rollout: &str,
    revision_expected: &str,
    edits: &[NativeEdit],
) -> Result<Vec<Value>, HarnessError> {
    let model = model_history(rollout)?;
    if revision(&model.items) != revision_expected {
        return Err(refuse(CHANGED));
    }
    let listed = listing(&model.items);
    let by_id: HashMap<&str, &Listed> = listed
        .iter()
        .map(|listed| (listed.item.id.as_str(), listed))
        .collect();
    let mut replaced: HashMap<usize, &str> = HashMap::new();
    let mut removed: HashSet<usize> = HashSet::new();
    for edit in edits {
        let id = match edit {
            NativeEdit::Replace { id, .. } | NativeEdit::Remove { id } => id,
        };
        let listed = by_id.get(id.as_str()).ok_or_else(|| refuse(CHANGED))?;
        if replaced.contains_key(&listed.index) || removed.contains(&listed.index) {
            return Err(refuse("Each message can only be edited once"));
        }
        match edit {
            NativeEdit::Replace { text, .. } => {
                if !listed.item.editable {
                    return Err(refuse("This message can't be edited"));
                }
                if text.trim().is_empty() {
                    return Err(refuse("Edited text can't be empty"));
                }
                replaced.insert(listed.index, text);
            }
            NativeEdit::Remove { .. } => {
                if !listed.item.removable {
                    return Err(refuse("This message can't be removed"));
                }
                removed.insert(listed.index);
            }
        }
    }
    let start = initial_context_len(&model.items);
    Ok(model
        .items
        .into_iter()
        .enumerate()
        .skip(start)
        .filter(|(index, _)| !removed.contains(index))
        .map(|(index, mut item)| {
            if let Some(text) = replaced.get(&index) {
                set_text(&mut item, text);
            }
            item
        })
        .collect())
}

/// Put `text` in a message as its one text part, keeping other parts (images)
/// and the item's other fields.
fn set_text(item: &mut Value, text: &str) {
    let part_type = if role(item) == "assistant" {
        "output_text"
    } else {
        "input_text"
    };
    match &mut item["content"] {
        Value::Array(parts) => {
            let mut placed = false;
            parts.retain_mut(|part| {
                if !matches!(item_type(part), "input_text" | "output_text") {
                    return true;
                }
                if placed {
                    return false;
                }
                placed = true;
                part["text"] = Value::String(text.to_owned());
                if part.get("annotations").is_some() {
                    part["annotations"] = json!([]);
                }
                true
            });
            if !placed {
                parts.insert(0, json!({ "type": part_type, "text": text }));
            }
        }
        content => *content = Value::String(text.to_owned()),
    }
}

// ── the app-server side ─────────────────────────────────────────────────────

impl CodexHarness {
    /// Run `operation` against a short-lived `codex app-server` (handshake,
    /// then the operation, then teardown), like the discovery probes.
    async fn with_history_server<T, F, Fut>(
        &self,
        cwd: &Path,
        env: &std::collections::BTreeMap<String, String>,
        operation: F,
    ) -> Result<T, HarnessError>
    where
        F: FnOnce(RpcClient) -> Fut,
        Fut: std::future::Future<Output = Result<T, HarnessError>>,
    {
        let exe = self.resolve_executable()?;
        let cwd = cwd.to_string_lossy();
        let AppServer {
            mut child,
            client,
            incoming,
            stderr_tail: _,
        } = spawn_app_server(&exe, &cwd, env)?;
        // Nothing here answers the server's own requests or reads its
        // notifications; the receiver only has to outlive the client.
        let _incoming = incoming;
        let work = async {
            client
                .request("initialize", initialize_params(true))
                .await?;
            client.notify("initialized", None);
            operation(client.clone()).await
        };
        let result = tokio::time::timeout(OPERATION_TIMEOUT, work).await;
        client.close_stdin_for_teardown();
        crate::shutdown_child(&mut child, self.kill_grace).await;
        match result {
            Ok(inner) => inner,
            Err(_) => Err(HarnessError::Protocol(
                "Codex took too long to answer".into(),
            )),
        }
    }
}

/// The rollout text of `session_id`, located through the app-server (a
/// thread's file can move, so a path is never cached), else by name under
/// the Codex home.
async fn rollout_text(
    client: &RpcClient,
    session_id: &str,
    env: &std::collections::BTreeMap<String, String>,
) -> Result<String, HarnessError> {
    let read = client
        .request("thread/read", json!({ "threadId": session_id }))
        .await
        .map_err(|_| refuse("This chat's Codex conversation is no longer stored on this device"))?;
    let path = read["thread"]["path"]
        .as_str()
        .map(PathBuf::from)
        .filter(|path| path.is_file())
        .or_else(|| find_rollout(env, session_id))
        .ok_or_else(|| {
            refuse("This chat's Codex conversation is no longer stored on this device")
        })?;
    let text = tokio::task::spawn_blocking(move || std::fs::read_to_string(path))
        .await
        .map_err(|e| HarnessError::Protocol(e.to_string()))??;
    Ok(text)
}

/// `rollout-<time>-<session>.jsonl` under the Codex home's sessions tree.
fn find_rollout(
    env: &std::collections::BTreeMap<String, String>,
    session_id: &str,
) -> Option<PathBuf> {
    let home = env
        .get("CODEX_HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("CODEX_HOME").map(PathBuf::from))
        .or_else(|| Some(crate::executable::home_dir()?.join(".codex")))?;
    let suffix = format!("-{session_id}.jsonl");
    let mut pending = vec![home.join("sessions")];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).ok()?.flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("rollout-") && name.ends_with(&suffix))
            {
                return Some(path);
            }
        }
    }
    None
}

impl CodexHarness {
    pub(super) async fn read_history(
        &self,
        session_id: &str,
        cwd: &Path,
        env: &std::collections::BTreeMap<String, String>,
    ) -> Result<NativeHistoryRead, HarnessError> {
        let session_id = session_id.to_owned();
        let rollout = self
            .with_history_server(cwd, env, |client| async move {
                rollout_text(&client, &session_id, env).await
            })
            .await?;
        tokio::task::spawn_blocking(move || read(&rollout))
            .await
            .map_err(|e| HarnessError::Protocol(e.to_string()))?
    }

    pub(super) async fn write_history(
        &self,
        session_id: &str,
        cwd: &Path,
        env: &std::collections::BTreeMap<String, String>,
        revision: &str,
        edits: &[NativeEdit],
    ) -> Result<String, HarnessError> {
        let session_id = session_id.to_owned();
        let (revision, edits) = (revision.to_owned(), edits.to_vec());
        let cwd_param = cwd.to_string_lossy().into_owned();
        self.with_history_server(cwd, env, |client| async move {
            let rollout = rollout_text(&client, &session_id, env).await?;
            let items =
                tokio::task::spawn_blocking(move || edited_items(&rollout, &revision, &edits))
                    .await
                    .map_err(|e| HarnessError::Protocol(e.to_string()))??;
            // The same core `thread/start` parameters a run uses; the run
            // that resumes this thread supplies its own model and sandbox.
            let mut params = serde_json::Map::new();
            if !cwd_param.is_empty() {
                params.insert("cwd".into(), Value::String(cwd_param));
            }
            params.insert("approvalPolicy".into(), "never".into());
            params.insert("sandbox".into(), "read-only".into());
            let started = client
                .request("thread/start", Value::Object(params))
                .await?;
            let thread_id = started["thread"]["id"]
                .as_str()
                .filter(|id| !id.is_empty())
                .ok_or_else(|| HarnessError::Protocol("thread/start returned no thread".into()))?
                .to_owned();
            client
                .request(
                    "thread/inject_items",
                    json!({ "threadId": thread_id, "items": items }),
                )
                .await?;
            Ok(thread_id)
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROLLOUT: &str = include_str!("../../tests/fixtures/codex/native-history-rollout.jsonl");
    const COMPACTED: &str =
        include_str!("../../tests/fixtures/codex/native-history-compacted.jsonl");

    fn ids(read: &NativeHistoryRead) -> Vec<&str> {
        read.items.iter().map(|item| item.id.as_str()).collect()
    }

    fn ids_of(items: &[Value]) -> Vec<String> {
        items
            .iter()
            .map(|item| item["id"].as_str().unwrap_or("-").to_owned())
            .collect()
    }

    #[test]
    fn lists_messages_reasoning_and_tools_without_the_context() {
        let read = read(ROLLOUT).unwrap();
        assert!(!read.compacted);
        // Developer messages and the environment block stay out; an item
        // without an id is named by its position in the model's list.
        assert_eq!(
            ids(&read),
            ["m1", "r1", "m2", "idx:7", "fc1", "idx:9", "r2", "m3"]
        );
        let turns: Vec<u32> = read.items.iter().map(|i| i.turn).collect();
        assert_eq!(turns, [0, 0, 0, 1, 1, 1, 1, 1]);
        let by_id = |id: &str| read.items.iter().find(|i| i.id == id).unwrap();
        assert_eq!(
            by_id("m1").kind,
            NativeItemKind::Text {
                text: "Say hello".into()
            }
        );
        assert_eq!(by_id("m1").role, NativeRole::User);
        assert!(by_id("m2").editable && !by_id("m2").removable);
        assert_eq!(by_id("m2").role, NativeRole::Assistant);
        assert_eq!(
            by_id("r1").kind,
            NativeItemKind::Reasoning {
                text: None,
                seal: ReasoningSeal::Encrypted
            }
        );
        assert!(!by_id("r1").editable && by_id("r1").removable);
        assert_eq!(
            by_id("r2").kind,
            NativeItemKind::Reasoning {
                text: Some("Looking at the listing.".into()),
                seal: ReasoningSeal::Encrypted
            }
        );
        assert_eq!(
            by_id("fc1").kind,
            NativeItemKind::ToolCall {
                name: "shell".into(),
                summary: "ls -la".into()
            }
        );
        assert!(!by_id("fc1").editable && !by_id("fc1").removable);
        assert_eq!(
            by_id("idx:9").kind,
            NativeItemKind::ToolResult {
                summary: "a.txt b.txt".into(),
                is_error: false
            }
        );
    }

    #[test]
    fn a_compaction_restarts_history_at_its_replacement() {
        let read = read(COMPACTED).unwrap();
        assert!(read.compacted);
        // Before the compaction nothing is listed; the replacement's retained
        // prompts, the sealed compaction and what followed are.
        assert_eq!(ids(&read), ["m1", "m5", "cmp_1", "m6"]);
        assert!(matches!(
            read.items[2].kind,
            NativeItemKind::CompactionSummary { .. }
        ));
        assert!(!read.items[2].editable && !read.items[2].removable);
    }

    #[test]
    fn without_edits_only_the_initial_context_is_left_out() {
        let revision = read(ROLLOUT).unwrap().revision;
        let items = edited_items(ROLLOUT, &revision, &[]).unwrap();
        // at_1, d1 and e1 are Codex's to rebuild; the mid-thread developer
        // message is conversation and stays.
        assert_eq!(
            ids_of(&items),
            ["m1", "r1", "m2", "d2", "-", "fc1", "-", "r2", "m3"]
        );
        let compacted_revision = read(COMPACTED).unwrap().revision;
        let items = edited_items(COMPACTED, &compacted_revision, &[]).unwrap();
        // The replacement's own leading context goes; the context it kept
        // between prompts is re-injected unchanged.
        assert_eq!(ids_of(&items), ["m1", "d2", "e2", "m5", "cmp_1", "m6"]);
    }

    #[test]
    fn replace_sets_one_text_part_and_keeps_the_rest_of_the_item() {
        let revision = read(ROLLOUT).unwrap().revision;
        let items = edited_items(
            ROLLOUT,
            &revision,
            &[
                NativeEdit::Replace {
                    id: "m2".into(),
                    text: "Hi there".into(),
                },
                NativeEdit::Replace {
                    id: "idx:7".into(),
                    text: "Now list the folders".into(),
                },
            ],
        )
        .unwrap();
        let m2 = items.iter().find(|i| i["id"] == "m2").unwrap();
        assert_eq!(m2["phase"], "final_answer");
        assert_eq!(
            m2["content"],
            json!([{ "type": "output_text", "text": "Hi there" }])
        );
        let prompt = &items[4];
        assert_eq!(
            prompt["content"],
            json!([{ "type": "input_text", "text": "Now list the folders" }])
        );
        let edited = read_items(&items);
        assert_eq!(
            edited[2].kind,
            NativeItemKind::Text {
                text: "Hi there".into()
            }
        );
    }

    /// The listing of an injected list, which has no initial context.
    fn read_items(items: &[Value]) -> Vec<NativeHistoryItem> {
        listing(items).into_iter().map(|l| l.item).collect()
    }

    #[test]
    fn remove_drops_reasoning_and_nothing_else() {
        let revision = read(ROLLOUT).unwrap().revision;
        let items = edited_items(
            ROLLOUT,
            &revision,
            &[
                NativeEdit::Remove { id: "r1".into() },
                NativeEdit::Remove { id: "r2".into() },
            ],
        )
        .unwrap();
        assert_eq!(ids_of(&items), ["m1", "m2", "d2", "-", "fc1", "-", "m3"]);
    }

    #[test]
    fn set_text_replaces_a_text_part_among_images() {
        let mut item = json!({
            "type": "message", "role": "user",
            "content": [
                { "type": "input_text", "text": "look" },
                { "type": "input_image", "image_url": "data:x" },
                { "type": "input_text", "text": "at this" }
            ]
        });
        set_text(&mut item, "see");
        assert_eq!(
            item["content"],
            json!([
                { "type": "input_text", "text": "see" },
                { "type": "input_image", "image_url": "data:x" }
            ])
        );
    }

    #[test]
    fn refuses_what_would_break_the_thread() {
        let revision = read(ROLLOUT).unwrap().revision;
        let attempt = |edit: NativeEdit, revision: &str| {
            edited_items(ROLLOUT, revision, &[edit])
                .unwrap_err()
                .to_string()
        };
        assert_eq!(
            attempt(NativeEdit::Remove { id: "fc1".into() }, &revision),
            "This message can't be removed"
        );
        assert_eq!(
            attempt(NativeEdit::Remove { id: "m1".into() }, &revision),
            "This message can't be removed"
        );
        assert_eq!(
            attempt(
                NativeEdit::Replace {
                    id: "r1".into(),
                    text: "x".into()
                },
                &revision
            ),
            "This message can't be edited"
        );
        assert_eq!(
            attempt(
                NativeEdit::Replace {
                    id: "m1".into(),
                    text: "".into()
                },
                &revision
            ),
            "Edited text can't be empty"
        );
        assert_eq!(
            attempt(NativeEdit::Remove { id: "r1".into() }, "stale"),
            CHANGED
        );
        assert_eq!(
            attempt(NativeEdit::Remove { id: "d1".into() }, &revision),
            CHANGED
        );
    }

    #[test]
    fn revision_ignores_bookkeeping_records() {
        let before = read(ROLLOUT).unwrap().revision;
        let with_tokens = format!(
            "{ROLLOUT}{}\n",
            r#"{"type":"event_msg","payload":{"type":"token_count"}}"#
        );
        assert_eq!(read(&with_tokens).unwrap().revision, before);
        let longer = format!(
            "{ROLLOUT}{}\n",
            r#"{"type":"response_item","payload":{"type":"message","id":"m9","role":"user","content":[{"type":"input_text","text":"more"}]}}"#
        );
        assert_ne!(read(&longer).unwrap().revision, before);
    }

    #[test]
    fn a_compaction_without_its_history_is_refused() {
        let old = r#"{"type":"compacted","payload":{"message":"summary"}}"#;
        assert!(read(old).is_err());
    }
}
