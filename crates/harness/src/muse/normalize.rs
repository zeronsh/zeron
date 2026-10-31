//! Muse view items → [`AgentEvent`]s.
//!
//! A turn's work arrives as items, each re-sent whole on `item/started`,
//! `item/updated` and `item/completed` with a rising `revision`, plus
//! `item/delta` appends to one field (`text`, `summary.<n>`, `output`).
//! Text is emitted as the suffix past what was already sent, so a full
//! re-send after deltas never duplicates. Items Zeron does not show:
//! `userMessage` (the prompt itself), `reminderChild` (Muse's per-step
//! housekeeping agents) and `compaction`.
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use zeron_proto::{AgentEvent, DoneStatus, TodoItem, TodoStatus, ToolCall, ToolDiff};

/// Tools whose own item carries the work (subagents, the todo list, a
/// question): their tool row would only duplicate it.
const SHADOWED_TOOLS: &[&str] = &["subagent_spawn", "request_user_input"];

#[derive(Default)]
pub(super) struct Normalizer {
    revisions: HashMap<String, i64>,
    kinds: HashMap<String, String>,
    /// What was emitted of each text-bearing item (message text or
    /// reasoning summary), to emit only new suffixes.
    emitted: HashMap<String, String>,
    /// Whether any assistant text went out this turn (later messages are
    /// separated from it by a blank line).
    wrote_text: bool,
    last_message: Option<String>,
    announced: HashSet<String>,
    finished: HashSet<String>,
    /// Subagents still running: background work the runtime holds.
    pub running_subagents: HashSet<String>,
}

impl Normalizer {
    /// A new turn: item bookkeeping restarts, running subagents carry over.
    pub fn reset(&mut self) {
        let running = std::mem::take(&mut self.running_subagents);
        *self = Self::default();
        self.running_subagents = running;
    }

    /// One `item/*` notification's item.
    pub fn item(&mut self, item: &Value) -> Vec<AgentEvent> {
        let id = string(item, "itemId");
        if id.is_empty() {
            return vec![];
        }
        let revision = item["revision"].as_i64().unwrap_or(0);
        if self.revisions.get(id).is_some_and(|seen| *seen >= revision) {
            return vec![];
        }
        self.revisions.insert(id.into(), revision);
        self.kinds.insert(id.into(), string(item, "kind").into());
        match string(item, "kind") {
            "agentMessage" => {
                let text = string(item, "text");
                self.text(id, text, false)
            }
            "reasoning" => {
                let text = reasoning_text(item);
                self.text(id, &text, true)
            }
            "toolCall" | "userShell" => self.tool(id, item),
            "subagent" => self.subagent(id, item),
            "workflow" => self.workflow(id, item),
            _ => vec![],
        }
    }

    /// One `item/delta`: `field` names what grows.
    pub fn delta(&mut self, params: &Value) -> Vec<AgentEvent> {
        let id = string(params, "itemId");
        let kind = self.kinds.get(id).map(String::as_str);
        let delta = string(params, "delta");
        if id.is_empty() || delta.is_empty() || self.finished.contains(id) {
            return vec![];
        }
        let field = params["field"].as_str().unwrap_or("text");
        let reasoning = match (field, kind) {
            ("text", Some("reasoning")) => true,
            ("text", _) => false,
            (field, _) if field.starts_with("summary.") => true,
            _ => return vec![],
        };
        let mut so_far = self.emitted.get(id).cloned().unwrap_or_default();
        if reasoning && field.starts_with("summary.") && !so_far.is_empty() {
            // Summary parts are separate paragraphs; a new index starts one.
            let index: usize = field[8..].parse().unwrap_or(0);
            let parts = so_far.split("\n\n").count();
            if index >= parts {
                so_far.push_str("\n\n");
            }
        }
        so_far.push_str(delta);
        self.text(id, &so_far, reasoning)
    }

    fn text(&mut self, id: &str, text: &str, reasoning: bool) -> Vec<AgentEvent> {
        let previous = self.emitted.entry(id.into()).or_default();
        let Some(suffix) = text.strip_prefix(previous.as_str()) else {
            // A rewrite, not a growth: what went out stands.
            return vec![];
        };
        if suffix.is_empty() {
            return vec![];
        }
        let mut suffix = suffix.to_string();
        let first = previous.is_empty();
        *previous = text.into();
        if reasoning {
            return vec![AgentEvent::ReasoningDelta { text: suffix }];
        }
        if first && self.wrote_text && self.last_message.as_deref() != Some(id) {
            suffix.insert_str(0, "\n\n");
        }
        self.wrote_text = true;
        self.last_message = Some(id.into());
        vec![AgentEvent::TextDelta { text: suffix }]
    }

    fn tool(&mut self, id: &str, item: &Value) -> Vec<AgentEvent> {
        let tool = string(item, "tool");
        let status = string(item, "status");
        let failed = status == "failed";
        if SHADOWED_TOOLS.contains(&tool) && !failed {
            return vec![];
        }
        let args = arguments(item);
        let mut events = Vec::new();
        // Arguments stream as model-authored JSON: announce once they parse,
        // or at the latest when the call ends.
        if !self.announced.contains(id) && (args.is_some() || status != "inProgress") {
            self.announced.insert(id.into());
            let call = if string(item, "kind") == "userShell" {
                ToolCall::Exec {
                    command: string(item, "commandText").into(),
                }
            } else {
                tool_call(tool, args.as_ref().unwrap_or(&Value::Null))
            };
            events.push(AgentEvent::ToolCall {
                id: id.into(),
                call,
            });
        }
        if status != "inProgress" && self.announced.contains(id) && self.finished.insert(id.into())
        {
            let output = item["visibleOutput"]
                .as_str()
                .or_else(|| item["failureReason"].as_str())
                .map(|text| cap(text, 16384));
            let diff = (!failed && status == "completed")
                .then(|| diff(tool, args.as_ref()?))
                .flatten();
            events.push(AgentEvent::ToolResult {
                id: id.into(),
                is_error: status != "completed"
                    || item["exitCode"].as_i64().is_some_and(|c| c != 0),
                output,
                diff,
            });
        }
        events
    }

    /// A subagent is shown as its spawn chip, `Agent: <objective>`; its
    /// result ends the chip. Muse runs it as a child session the host
    /// cannot address, so its own transcript is not streamed.
    fn subagent(&mut self, id: &str, item: &Value) -> Vec<AgentEvent> {
        let status = string(item, "status");
        let mut events = Vec::new();
        if self.announced.insert(id.into()) {
            let label = [string(item, "objective"), string(item, "role")]
                .into_iter()
                .find(|s| !s.trim().is_empty())
                .unwrap_or("Muse agent");
            events.push(AgentEvent::ToolCall {
                id: id.into(),
                call: ToolCall::Unknown {
                    name: format!("Agent: {}", label.trim()),
                    input: Some(json!({
                        "objective": item["objective"],
                        "role": item["role"],
                    })),
                },
            });
        }
        if status == "inProgress" {
            self.running_subagents.insert(id.into());
        } else {
            self.running_subagents.remove(id);
            if self.finished.insert(id.into()) {
                let output = item["result"]["summary"]
                    .as_str()
                    .or_else(|| item["finalSummary"].as_str())
                    .or_else(|| item["failureReason"].as_str())
                    .map(|text| cap(text, 16384));
                events.push(AgentEvent::ToolResult {
                    id: id.into(),
                    is_error: status != "completed",
                    output,
                    diff: None,
                });
            }
        }
        events
    }

    fn workflow(&mut self, id: &str, item: &Value) -> Vec<AgentEvent> {
        let status = string(item, "status");
        let mut events = Vec::new();
        if self.announced.insert(id.into()) {
            let name = item["entryId"].as_str().unwrap_or("Workflow");
            events.push(AgentEvent::ToolCall {
                id: id.into(),
                call: ToolCall::Unknown {
                    name: format!("Workflow: {name}"),
                    input: None,
                },
            });
        }
        if status == "inProgress" {
            self.running_subagents.insert(id.into());
        } else {
            self.running_subagents.remove(id);
            if self.finished.insert(id.into()) {
                events.push(AgentEvent::ToolResult {
                    id: id.into(),
                    is_error: status != "completed",
                    output: item["finalSummary"]
                        .as_str()
                        .or_else(|| item["failureReason"].as_str())
                        .map(|text| cap(text, 16384)),
                    diff: None,
                });
            }
        }
        events
    }
}

/// How a `turn/completed` terminal reads: `(status, error)`.
pub(super) fn terminal(params: &Value, interrupted: bool) -> (DoneStatus, Option<String>) {
    let reason = params["error"]["message"]
        .as_str()
        .or_else(|| params["reason"].as_str())
        .map(str::to_owned);
    match string(params, "terminal") {
        "completed" => (DoneStatus::Completed, None),
        "cancelled" if interrupted => (DoneStatus::Interrupted, None),
        "cancelled" => (
            DoneStatus::Interrupted,
            reason.filter(|r| !r.contains("cancelled")),
        ),
        other => (
            DoneStatus::Errored,
            Some(reason.unwrap_or_else(|| format!("Muse turn {other}"))),
        ),
    }
}

fn reasoning_text(item: &Value) -> String {
    match item["summary"].as_array() {
        Some(parts) if !parts.is_empty() => parts
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join("\n\n"),
        _ => string(item, "text").into(),
    }
}

fn arguments(item: &Value) -> Option<Value> {
    match &item["args"] {
        Value::String(raw) => serde_json::from_str::<Value>(raw)
            .ok()
            .filter(Value::is_object),
        Value::Object(_) => Some(item["args"].clone()),
        _ => None,
    }
}

fn arg<'a>(args: &'a Value, keys: &[&str]) -> Option<&'a str> {
    keys.iter().find_map(|key| args[*key].as_str())
}

fn tool_call(tool: &str, args: &Value) -> ToolCall {
    let path = || {
        arg(args, &["file_path", "path", "filePath"])
            .unwrap_or("")
            .to_string()
    };
    match tool {
        "bash" | "shell" | "exec_command" => ToolCall::Exec {
            command: arg(args, &["command", "cmd"]).unwrap_or("").into(),
        },
        "read" | "read_file" | "view" => ToolCall::ReadFile { path: path() },
        "write" | "write_file" | "create_file" => ToolCall::WriteFile {
            path: path(),
            content: arg(args, &["content", "text"]).map(str::to_owned),
        },
        "edit" | "edit_file" | "str_replace" | "replace" => ToolCall::EditFile {
            path: path(),
            old_string: arg(args, &["old_string", "old_str", "oldText"]).map(str::to_owned),
            new_string: arg(args, &["new_string", "new_str", "newText"]).map(str::to_owned),
        },
        "apply_patch" | "patch" => ToolCall::ApplyPatch {
            path: arg(args, &["path", "file_path"]).map(str::to_owned),
        },
        "grep" | "search" | "rg" => ToolCall::Search {
            pattern: arg(args, &["pattern", "query"]).unwrap_or("").into(),
            path: arg(args, &["path"]).map(str::to_owned),
        },
        "glob" | "find" | "list_files" => ToolCall::Glob {
            pattern: arg(args, &["pattern", "glob"]).unwrap_or("").into(),
        },
        "web_fetch" | "fetch" | "browse" => ToolCall::WebFetch {
            url: arg(args, &["url"]).unwrap_or("").into(),
            prompt: arg(args, &["prompt"]).map(str::to_owned),
        },
        "web_search" => ToolCall::WebSearch {
            query: arg(args, &["query"]).unwrap_or("").into(),
        },
        "write_todos" | "todo_write" => ToolCall::Todo {
            items: args["todos"]
                .as_array()
                .or_else(|| args["items"].as_array())
                .into_iter()
                .flatten()
                .filter_map(|todo| {
                    let text = arg(todo, &["content", "text", "description"])?.trim();
                    (!text.is_empty())
                        .then(|| TodoItem::new(text, TodoStatus::parse(string(todo, "status"))))
                })
                .collect(),
        },
        name if name.starts_with("zeron_") => ToolCall::Mcp {
            server: "zeron".into(),
            tool: name[6..].into(),
            input: Some(args.clone()),
        },
        name => match name.split_once("__") {
            // MCP tools arrive as `<server>__<tool>`.
            Some((server, tool)) if !server.is_empty() && !tool.is_empty() => ToolCall::Mcp {
                server: server.into(),
                tool: tool.into(),
                input: Some(args.clone()),
            },
            _ => ToolCall::Unknown {
                name: name.into(),
                input: (!args.is_null()).then(|| args.clone()),
            },
        },
    }
}

fn diff(tool: &str, args: &Value) -> Option<ToolDiff> {
    let path = arg(args, &["file_path", "path", "filePath"])?.to_string();
    match tool {
        "edit" | "edit_file" | "str_replace" | "replace" => Some(ToolDiff {
            path,
            old_text: arg(args, &["old_string", "old_str", "oldText"]).map(|s| cap(s, 65536)),
            new_text: cap(arg(args, &["new_string", "new_str", "newText"])?, 65536),
        }),
        "write" | "write_file" | "create_file" => Some(ToolDiff {
            path,
            old_text: None,
            new_text: cap(arg(args, &["content", "text"])?, 65536),
        }),
        _ => None,
    }
}

pub(super) fn string<'a>(v: &'a Value, key: &str) -> &'a str {
    v[key].as_str().unwrap_or("")
}

fn cap(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.into();
    }
    let mut at = limit;
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    format!("{}\n… [truncated]", &text[..at])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: &str, kind: &str, revision: i64, status: &str, extra: Value) -> Value {
        let mut item = json!({"itemId":id,"kind":kind,"revision":revision,"status":status});
        item.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        item
    }

    #[test]
    fn message_text_streams_once_across_deltas_and_full_resends() {
        let mut n = Normalizer::default();
        assert!(
            n.item(&item(
                "m",
                "agentMessage",
                1,
                "inProgress",
                json!({"text":""})
            ))
            .is_empty()
        );
        let events = n.delta(&json!({"itemId":"m","field":"text","delta":"Hel"}));
        assert!(matches!(&events[..], [AgentEvent::TextDelta { text }] if text == "Hel"));
        n.delta(&json!({"itemId":"m","field":"text","delta":"lo"}));
        // The completed re-send adds only what deltas did not carry.
        let events = n.item(&item(
            "m",
            "agentMessage",
            2,
            "completed",
            json!({"text":"Hello!"}),
        ));
        assert!(matches!(&events[..], [AgentEvent::TextDelta { text }] if text == "!"));
        // A stale revision is ignored.
        assert!(
            n.item(&item(
                "m",
                "agentMessage",
                1,
                "inProgress",
                json!({"text":"x"})
            ))
            .is_empty()
        );
        // A second message is its own paragraph.
        let events = n.item(&item(
            "m2",
            "agentMessage",
            1,
            "completed",
            json!({"text":"Next"}),
        ));
        assert!(matches!(&events[..], [AgentEvent::TextDelta { text }] if text == "\n\nNext"));
    }

    #[test]
    fn reasoning_summaries_stream_as_reasoning() {
        let mut n = Normalizer::default();
        n.item(&item("r", "reasoning", 1, "inProgress", json!({})));
        let a = n.delta(&json!({"itemId":"r","field":"summary.0","delta":"Plan"}));
        let b = n.delta(&json!({"itemId":"r","field":"summary.1","delta":"Then"}));
        assert!(matches!(&a[..], [AgentEvent::ReasoningDelta { text }] if text == "Plan"));
        assert!(matches!(&b[..], [AgentEvent::ReasoningDelta { text }] if text == "\n\nThen"));
        let done = n.item(&item(
            "r",
            "reasoning",
            2,
            "completed",
            json!({"summary":["Plan","Then"]}),
        ));
        assert!(done.is_empty());
    }

    #[test]
    fn tools_announce_once_and_end_with_their_output() {
        let mut n = Normalizer::default();
        let args = r#"{"command":"touch x","description":"marker"}"#;
        let started = n.item(&item(
            "t",
            "toolCall",
            1,
            "inProgress",
            json!({"tool":"bash","args":args}),
        ));
        assert!(
            matches!(&started[..], [AgentEvent::ToolCall { id, call: ToolCall::Exec { command } }]
            if id == "t" && command == "touch x")
        );
        let done = n.item(&item(
            "t",
            "toolCall",
            2,
            "completed",
            json!({"tool":"bash","args":args,"visibleOutput":"ok"}),
        ));
        assert!(
            matches!(&done[..], [AgentEvent::ToolResult { id, is_error: false, output: Some(o), .. }]
            if id == "t" && o == "ok")
        );
        // A failed edit reports its reason, and no diff.
        let edit = r#"{"file_path":"a.rs","old_string":"a","new_string":"b"}"#;
        n.item(&item(
            "e",
            "toolCall",
            1,
            "inProgress",
            json!({"tool":"edit","args":edit}),
        ));
        let failed = n.item(&item(
            "e",
            "toolCall",
            2,
            "failed",
            json!({"tool":"edit","args":edit,"failureReason":"denied"}),
        ));
        assert!(
            matches!(&failed[..], [AgentEvent::ToolResult { is_error: true, output: Some(o), diff: None, .. }]
            if o == "denied")
        );
        // A completed edit carries its diff.
        n.item(&item(
            "e2",
            "toolCall",
            1,
            "inProgress",
            json!({"tool":"edit","args":edit}),
        ));
        let ok = n.item(&item(
            "e2",
            "toolCall",
            2,
            "completed",
            json!({"tool":"edit","args":edit}),
        ));
        assert!(
            matches!(&ok[..], [AgentEvent::ToolResult { diff: Some(d), .. }] if d.new_text == "b")
        );
    }

    #[test]
    fn subagents_are_spawn_chips_and_background_work() {
        let mut n = Normalizer::default();
        // The spawn tool's own row is shadowed by the subagent item.
        assert!(
            n.item(&item(
                "s",
                "toolCall",
                1,
                "inProgress",
                json!({"tool":"subagent_spawn"})
            ))
            .is_empty()
        );
        let started = n.item(&item(
            "a",
            "subagent",
            1,
            "inProgress",
            json!({"objective":"Reply with exactly: Hello","role":"hello agent"}),
        ));
        assert!(
            matches!(&started[..], [AgentEvent::ToolCall { call: ToolCall::Unknown { name, .. }, .. }]
            if name == "Agent: Reply with exactly: Hello")
        );
        assert!(n.running_subagents.contains("a"));
        n.reset();
        assert!(
            n.running_subagents.contains("a"),
            "a new turn keeps background work"
        );
        let done = n.item(&item(
            "a",
            "subagent",
            9,
            "completed",
            json!({"result":{"summary":"Hello"}}),
        ));
        assert!(
            done.iter().any(
                |e| matches!(e, AgentEvent::ToolResult { output: Some(o), .. } if o == "Hello")
            )
        );
        assert!(n.running_subagents.is_empty());
    }

    #[test]
    fn housekeeping_items_are_not_shown() {
        let mut n = Normalizer::default();
        for kind in ["userMessage", "reminderChild", "compaction"] {
            assert!(
                n.item(&item(kind, kind, 1, "completed", json!({"text":"x"})))
                    .is_empty()
            );
        }
    }

    #[test]
    fn terminals_map_to_done_statuses() {
        let t = |v: Value, i| terminal(&v, i);
        assert_eq!(
            t(json!({"terminal":"completed"}), false),
            (DoneStatus::Completed, None)
        );
        assert_eq!(
            t(
                json!({"terminal":"cancelled","reason":"cancelled during model step"}),
                true
            ),
            (DoneStatus::Interrupted, None)
        );
        assert_eq!(
            t(
                json!({"terminal":"failed","error":{"message":"usage limit reached"}}),
                false
            ),
            (DoneStatus::Errored, Some("usage limit reached".into()))
        );
    }

    #[test]
    fn tool_names_map_to_typed_calls() {
        assert!(
            matches!(tool_call("write_todos", &json!({"todos":[{"content":"a","status":"in_progress"}]})),
            ToolCall::Todo { items } if items.len() == 1 && items[0].status() == TodoStatus::InProgress)
        );
        assert!(matches!(tool_call("github__create_pr", &json!({})),
            ToolCall::Mcp { server, tool, .. } if server == "github" && tool == "create_pr"));
        assert!(matches!(tool_call("web_search", &json!({"query":"q"})),
            ToolCall::WebSearch { query } if query == "q"));
    }
}
