//! MSP view notifications → [`AgentEvent`] normalization.
//!
//! The host streams transcript items: `agentMessage` text arrives as
//! `item/delta` appends and is restated whole on `item/completed` (the
//! normalizer reconciles so no byte is emitted twice), `toolCall` items
//! open on `item/started` and settle on `item/completed`, and the turn
//! itself ends with `turn/completed`. Kinds with no Zeron rendering
//! (user echoes, reminder children, hooks, compaction) are dropped.

use std::collections::HashMap;

use serde_json::Value;
use zeron_proto::{AgentEvent, DoneStatus, TodoItem, TodoStatus, ToolCall};

#[derive(Default)]
pub(super) struct Normalizer {
    /// Text already streamed per open agentMessage item.
    streamed: HashMap<String, String>,
    /// Tool items that already emitted their `ToolCall`.
    tools: HashMap<String, ()>,
    pub error: Option<String>,
    pub interrupted: bool,
}

impl Normalizer {
    pub fn reset_turn(&mut self) {
        self.error = None;
        self.interrupted = false;
        self.streamed.clear();
        self.tools.clear();
    }

    pub fn status(&self) -> DoneStatus {
        if self.interrupted {
            DoneStatus::Interrupted
        } else if self.error.is_some() {
            DoneStatus::Errored
        } else {
            DoneStatus::Completed
        }
    }

    /// One MSP notification (`method`, `params`) → zero or more events.
    /// `turn/completed` itself is consumed by the runner (it owns `Done`);
    /// this only records its error.
    pub fn map(&mut self, method: &str, params: &Value) -> Vec<AgentEvent> {
        let mut events = Vec::new();
        match method {
            "item/delta" => {
                let item_id = str_of(params, "itemId").to_owned();
                let delta = str_of(params, "delta");
                if delta.is_empty() {
                    return events;
                }
                match str_of(params, "field") {
                    "text" | "" => {
                        self.streamed.entry(item_id).or_default().push_str(delta);
                        events.push(AgentEvent::TextDelta {
                            text: delta.to_owned(),
                        });
                    }
                    field if field == "summary" || field.starts_with("summary.") => {
                        events.push(AgentEvent::ReasoningDelta {
                            text: delta.to_owned(),
                        });
                    }
                    // `output` appends stream into the tool result, which is
                    // delivered whole on completion; reasoning `text` is
                    // never streamed in MSP v1.
                    _ => {}
                }
            }
            "item/started" | "item/updated" | "item/completed" => {
                let item = &params["item"];
                let id = str_of(item, "itemId").to_owned();
                let terminal = str_of(item, "status") != "inProgress";
                match str_of(item, "kind") {
                    "agentMessage" if terminal => {
                        if let Some(text) = item.get("text").and_then(Value::as_str) {
                            let seen = self.streamed.remove(&id).unwrap_or_default();
                            if let Some(suffix) = text.strip_prefix(&seen).filter(|s| !s.is_empty())
                            {
                                events.push(AgentEvent::TextDelta {
                                    text: suffix.to_owned(),
                                });
                            } else if seen.is_empty() && !text.is_empty() {
                                events.push(AgentEvent::TextDelta {
                                    text: text.to_owned(),
                                });
                            }
                        }
                    }
                    "reasoning" if terminal => {
                        let parts: Vec<&str> = item
                            .get("summary")
                            .and_then(Value::as_array)
                            .map(|a| a.iter().filter_map(Value::as_str).collect())
                            .unwrap_or_default();
                        let text = if parts.is_empty() {
                            str_of(item, "text").to_owned()
                        } else {
                            parts.join("\n")
                        };
                        if !text.is_empty() {
                            events.push(AgentEvent::ReasoningDelta { text });
                        }
                    }
                    "toolCall" => {
                        if !self.tools.contains_key(&id) {
                            self.tools.insert(id.clone(), ());
                            events.push(AgentEvent::ToolCall {
                                id: id.clone(),
                                call: tool_call(item),
                            });
                        }
                        if terminal {
                            self.tools.remove(&id);
                            events.push(AgentEvent::ToolResult {
                                id,
                                is_error: is_error_status(item),
                                output: capped_output(item),
                                diff: None,
                            });
                        }
                    }
                    "userShell" => {
                        if !self.tools.contains_key(&id) {
                            self.tools.insert(id.clone(), ());
                            events.push(AgentEvent::ToolCall {
                                id: id.clone(),
                                call: ToolCall::Exec {
                                    command: str_of(item, "commandText").to_owned(),
                                },
                            });
                        }
                        if terminal {
                            self.tools.remove(&id);
                            let code_ok = item
                                .get("exitCode")
                                .and_then(Value::as_i64)
                                .map(|c| c == 0)
                                .unwrap_or(true);
                            events.push(AgentEvent::ToolResult {
                                id,
                                is_error: is_error_status(item) || !code_ok,
                                output: capped_output(item),
                                diff: None,
                            });
                        }
                    }
                    "subagent" => {
                        if !self.tools.contains_key(&id) {
                            self.tools.insert(id.clone(), ());
                            let mut input = serde_json::Map::new();
                            let objective = str_of(item, "objective");
                            if !objective.is_empty() {
                                input.insert("prompt".into(), objective.into());
                            }
                            if let Some(child) = item.get("childSessionId").and_then(Value::as_str)
                            {
                                input.insert("childSessionId".into(), child.into());
                            }
                            events.push(AgentEvent::ToolCall {
                                id: id.clone(),
                                call: ToolCall::Unknown {
                                    name: "Agent".into(),
                                    input: (!input.is_empty()).then_some(Value::Object(input)),
                                },
                            });
                        }
                        if terminal {
                            self.tools.remove(&id);
                            events.push(AgentEvent::ToolResult {
                                id,
                                is_error: is_error_status(item),
                                output: capped_output(item),
                                diff: None,
                            });
                        }
                    }
                    "webSearch" if terminal && !self.tools.contains_key(&id) => {
                        self.tools.insert(id.clone(), ());
                        events.push(AgentEvent::ToolCall {
                            id: id.clone(),
                            call: ToolCall::WebSearch {
                                query: str_of(item, "text").to_owned(),
                            },
                        });
                        events.push(AgentEvent::ToolResult {
                            id,
                            is_error: is_error_status(item),
                            output: capped_output(item),
                            diff: None,
                        });
                    }
                    _ => {}
                }
            }
            "session/todoListChanged" => {
                let items = params
                    .get("items")
                    .and_then(Value::as_array)
                    .map(|list| {
                        list.iter()
                            .map(|item| {
                                let status = match str_of(item, "status") {
                                    "completed" | "complete" | "done" => TodoStatus::Completed,
                                    "inProgress" | "in_progress" | "active" => {
                                        TodoStatus::InProgress
                                    }
                                    _ => TodoStatus::Pending,
                                };
                                TodoItem::new(str_of(item, "text"), status)
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                events.push(AgentEvent::ToolCall {
                    id: zeron_proto::LIVE_PLAN_TOOL_ID.to_owned(),
                    call: ToolCall::Todo { items },
                });
            }
            "session/tokenUsage" => {
                if let Some(usage) = params.get("usage") {
                    events.push(AgentEvent::Usage {
                        input_tokens: usage
                            .get("inputTokens")
                            .and_then(Value::as_u64)
                            .unwrap_or(0),
                        output_tokens: usage
                            .get("outputTokens")
                            .and_then(Value::as_u64)
                            .unwrap_or(0),
                    });
                }
            }
            "session/contextUsage" => {
                events.push(AgentEvent::ContextUsage {
                    tokens: params.get("usedTokens").and_then(Value::as_u64),
                    window: params.get("windowTokens").and_then(Value::as_u64),
                });
            }
            "turn/completed" => {
                if str_of(params, "terminal") == "failed" {
                    self.error = Some(
                        params
                            .pointer("/error/message")
                            .and_then(Value::as_str)
                            .or_else(|| params.get("reason").and_then(Value::as_str))
                            .unwrap_or("Muse turn failed")
                            .to_owned(),
                    );
                } else if matches!(str_of(params, "terminal"), "cancelled" | "interrupted") {
                    self.interrupted = true;
                }
            }
            _ => {}
        }
        events
    }
}

fn is_error_status(item: &Value) -> bool {
    matches!(
        str_of(item, "status"),
        "failed" | "rejected" | "timedOut" | "cancelled"
    )
}

fn capped_output(item: &Value) -> Option<String> {
    let raw = item
        .get("visibleOutput")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())?;
    const LIMIT: usize = 16_384;
    if raw.len() <= LIMIT {
        return Some(raw.to_owned());
    }
    let mut at = LIMIT;
    while !raw.is_char_boundary(at) {
        at -= 1;
    }
    Some(format!("{}\n… [truncated]", &raw[..at]))
}

/// Decode an MSP `toolCall` item (`tool` name + verbatim `args` JSON string)
/// into Zeron's typed call, mirroring the other native drivers' mappings.
fn tool_call(item: &Value) -> ToolCall {
    let name = str_of(item, "tool");
    let args: Value = item
        .get("args")
        .and_then(Value::as_str)
        .and_then(|raw| serde_json::from_str(raw).ok())
        .unwrap_or(Value::Null);
    let arg = |key: &str| args.get(key).and_then(Value::as_str).unwrap_or("");
    match name {
        "muse.bash" | "bash" | "shell" | "exec" => ToolCall::Exec {
            command: arg("command").to_owned(),
        },
        "muse.read_file" | "read_file" | "read" => ToolCall::ReadFile {
            path: arg("path").to_owned(),
        },
        "muse.write_file" | "write_file" | "write" => ToolCall::WriteFile {
            path: arg("path").to_owned(),
            content: args
                .get("content")
                .and_then(Value::as_str)
                .map(str::to_owned),
        },
        "muse.edit_file" | "edit_file" | "edit" => ToolCall::EditFile {
            path: arg("path").to_owned(),
            old_string: args
                .get("old_str")
                .or_else(|| args.get("old_string"))
                .and_then(Value::as_str)
                .map(str::to_owned),
            new_string: args
                .get("new_str")
                .or_else(|| args.get("new_string"))
                .and_then(Value::as_str)
                .map(str::to_owned),
        },
        "muse.search" | "search" | "grep" => ToolCall::Search {
            pattern: arg("pattern").to_owned(),
            path: args.get("path").and_then(Value::as_str).map(str::to_owned),
        },
        "muse.web_fetch" | "web_fetch" => ToolCall::WebFetch {
            url: arg("url").to_owned(),
            prompt: args
                .get("prompt")
                .and_then(Value::as_str)
                .map(str::to_owned),
        },
        "muse.web_search" | "web_search" => ToolCall::WebSearch {
            query: arg("query").to_owned(),
        },
        "muse.write_todos" | "write_todos" => {
            let items = args
                .get("todos")
                .and_then(Value::as_array)
                .map(|list| {
                    list.iter()
                        .map(|todo| {
                            TodoItem::new(
                                arg_of(todo, "text"),
                                TodoStatus::parse(arg_of(todo, "status")),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default();
            ToolCall::Todo { items }
        }
        other => ToolCall::Unknown {
            name: other.to_owned(),
            input: if args.is_null() { None } else { Some(args) },
        },
    }
}

fn arg_of<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or("")
}

pub(super) fn str_of<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn deltas_then_completion_do_not_duplicate_text() {
        let mut n = Normalizer::default();
        let d = |delta: &str| json!({"itemId": "m1", "field": "text", "delta": delta, "sessionId": "s", "viewCursor": "1"});
        assert_eq!(
            n.map("item/delta", &d("hel")),
            vec![AgentEvent::TextDelta { text: "hel".into() }]
        );
        n.map("item/delta", &d("lo"));
        let done = json!({"item": {"itemId": "m1", "kind": "agentMessage", "status": "completed", "text": "hello"}});
        assert!(n.map("item/completed", &done).is_empty());
        // A completion with no prior deltas emits the whole text once.
        let done2 = json!({"item": {"itemId": "m2", "kind": "agentMessage", "status": "completed", "text": "whole"}});
        assert_eq!(
            n.map("item/completed", &done2),
            vec![AgentEvent::TextDelta {
                text: "whole".into()
            }]
        );
    }

    #[test]
    fn tool_call_opens_once_and_settles_with_output() {
        let mut n = Normalizer::default();
        let started = json!({"item": {"itemId": "t1", "kind": "toolCall", "status": "inProgress", "tool": "muse.bash", "args": "{\"command\":\"ls\"}"}});
        let events = n.map("item/started", &started);
        assert!(matches!(
            &events[0],
            AgentEvent::ToolCall { call: ToolCall::Exec { command }, .. } if command == "ls"
        ));
        assert!(n.map("item/started", &started).is_empty());
        let done = json!({"item": {"itemId": "t1", "kind": "toolCall", "status": "failed", "tool": "muse.bash", "args": "{}", "visibleOutput": "boom"}});
        let events = n.map("item/completed", &done);
        assert!(matches!(
            &events[0],
            AgentEvent::ToolResult { is_error: true, output: Some(o), .. } if o == "boom"
        ));
    }

    #[test]
    fn failed_turn_records_error_and_todos_map() {
        let mut n = Normalizer::default();
        n.map(
            "turn/completed",
            &json!({"terminal": "failed", "error": {"message": "quota"}}),
        );
        assert_eq!(n.status(), DoneStatus::Errored);
        assert_eq!(n.error.as_deref(), Some("quota"));
        let events = n.map(
            "session/todoListChanged",
            &json!({"items": [{"text": "a", "status": "inProgress"}, {"text": "b", "status": "completed"}]}),
        );
        match &events[0] {
            AgentEvent::ToolCall {
                call: ToolCall::Todo { items },
                ..
            } => {
                assert_eq!(items[0].status(), TodoStatus::InProgress);
                assert_eq!(items[1].status(), TodoStatus::Completed);
            }
            other => panic!("unexpected {other:?}"),
        }
    }
}
