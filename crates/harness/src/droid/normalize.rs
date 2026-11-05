use serde_json::Value;
use std::collections::HashMap;
use zeron_proto::{AgentEvent, DoneStatus, TodoItem, TodoStatus, ToolCall, ToolDiff};

#[derive(Default)]
pub(super) struct Normalizer {
    tools: HashMap<String, (Value, bool)>,
    pub error: Option<String>,
}

impl Normalizer {
    pub fn permission(&mut self, item: &Value) -> Result<Vec<AgentEvent>, &'static str> {
        let mut tool = item["toolUse"].clone();
        if item["confirmationType"] == "exit_spec_mode" {
            let invalid = "Droid's plan is missing, invalid, or too large to display (64 KiB limit). The plan was not approved.";
            let proposal = (|| {
                if string(&tool, "name") != "ExitSpecMode" || string(&tool, "id").is_empty() {
                    return None;
                }
                let details = &item["details"];
                let args = &tool["input"];
                let body = details.get("plan").or_else(|| args.get("plan"))?.as_str()?;
                let title = match details.get("title").or_else(|| args.get("title")) {
                    None | Some(Value::Null) => None,
                    Some(value) => Some(value.as_str()?),
                };
                Some(zeron_proto::agent_plans::ProposedPlan::new(title, body)?.display_input())
            })();
            let Some(input) = proposal else {
                self.tools
                    .insert(string(&tool, "id").into(), (Value::Null, true));
                return Err(invalid);
            };
            tool["input"] = input;
            if self
                .tools
                .get(string(&tool, "id"))
                .is_some_and(|(old, emitted)| *emitted && old != &tool)
            {
                return Err(invalid);
            }
        }
        self.upsert(&tool);
        Ok(self.flush(string(&tool, "id")))
    }

    pub fn upsert(&mut self, tool: &Value) {
        let id = string(tool, "id");
        if !id.is_empty() {
            let entry = self.tools.entry(id.into()).or_default();
            if !entry.1 {
                entry.0 = tool.clone();
            }
        }
    }

    pub fn flush(&mut self, id: &str) -> Vec<AgentEvent> {
        let Some((tool, emitted)) = self.tools.get_mut(id) else {
            return vec![];
        };
        if std::mem::replace(emitted, true) {
            return vec![];
        }
        vec![AgentEvent::ToolCall {
            id: id.into(),
            call: tool_call(string(tool, "name"), &tool["input"]),
        }]
    }

    pub fn map(&mut self, frame: &Value) -> Vec<AgentEvent> {
        let mut events = vec![];
        match string(frame, "type") {
            "assistant_text_delta" => events.push(AgentEvent::TextDelta { text: string(frame, "textDelta").into() }),
            "thinking_text_delta" => events.push(AgentEvent::ReasoningDelta { text: string(frame, "textDelta").into() }),
            "assistant_message_retracted" => events.push(AgentEvent::Error {
                message: "Droid retracted an assistant message; its partial text above may no longer apply.".into(),
            }),
            "tool_call" => self.upsert(&frame["toolUse"]),
            "tool_execution_phase_changed" if frame["phase"] == "queued" => {
                let id = string(frame, "toolUseId");
                // Permission details, not preliminary arguments, are the plan
                // the person will approve. Results still flush no-permission calls.
                if !self.tools.get(id).is_some_and(|(tool, _)| tool["name"] == "ExitSpecMode") {
                    events.extend(self.flush(id));
                }
            }
            "tool_result" => {
                let id = string(frame, "toolUseId");
                events.extend(self.flush(id));
                let is_error = frame["isError"] == true;
                let diff = self.tools.get(id).and_then(|(tool, _)| {
                    if is_error { return None; }
                    let args = &tool["input"];
                    match string(tool, "name") {
                        "Create" => Some(ToolDiff { path: string(args, "file_path").into(), old_text: None, new_text: cap(string(args, "content"), 65536) }),
                        "Edit" => Some(ToolDiff { path: string(args, "file_path").into(), old_text: Some(cap(string(args, "old_str"), 65536)), new_text: cap(string(args, "new_str"), 65536) }),
                        _ => None,
                    }
                });
                let content = &frame["content"];
                let output = content.as_str().map(str::to_owned).or_else(|| content.as_array().map(|items| items.iter().filter_map(|v| v["text"].as_str()).collect::<Vec<_>>().join("\n")));
                events.push(AgentEvent::ToolResult { id: id.into(), is_error, output: output.map(|s| cap(&s, 16384)), diff });
                // Only the emitted bit is needed after the result.
                if let Some((tool, _)) = self.tools.get_mut(id) { *tool = Value::Null; }
            }
            "session_token_usage_changed" => {
                let usage = &frame["lastCallTokenUsage"];
                if let Some(input) = usage["inputTokens"].as_u64() {
                    events.push(AgentEvent::ContextUsage { tokens: Some(input + usage["cacheReadTokens"].as_u64().unwrap_or(0) + usage["cacheCreationTokens"].as_u64().unwrap_or(0)), window: None });
                }
            }
            "agent_turn_completed" => {
                let usage = &frame["tokenUsage"];
                if usage.is_object() {
                    let n = |key| usage[key].as_u64().unwrap_or(0);
                    events.push(AgentEvent::Usage { input_tokens: n("inputTokens") + n("cacheReadTokens") + n("cacheCreationTokens"), output_tokens: n("outputTokens") });
                }
            }
            "error" => {
                self.error = Some(frame["message"].as_str().or_else(|| frame["error"]["message"].as_str()).unwrap_or("Droid failed").into());
            }
            _ => {}
        }
        events
    }

    pub fn finish(&mut self, reason: &str, session: &str) -> AgentEvent {
        let (status, error) = match reason {
            "completed" => (DoneStatus::Completed, None),
            "cancelled" => (DoneStatus::Interrupted, None),
            "model_authentication_failed" => (DoneStatus::Errored, Some(sign_in_error().into())),
            _ => (
                DoneStatus::Errored,
                Some(
                    self.error
                        .take()
                        .unwrap_or_else(|| format!("Droid turn ended: {reason}")),
                ),
            ),
        };
        self.error = None;
        self.tools.retain(|_, (tool, _)| !tool.is_null());
        AgentEvent::Done {
            status,
            error,
            result: None,
            session_id: (!session.is_empty()).then(|| session.into()),
        }
    }
}

pub(super) fn sign_in_error() -> &'static str {
    "Droid is not signed in. Run `droid` in a terminal and use `/login`, or set FACTORY_API_KEY."
}

pub(super) fn string<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().unwrap_or("")
}

fn cap(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.into();
    }
    let mut end = limit;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n… [truncated]", &text[..end])
}

fn tool_call(name: &str, args: &Value) -> ToolCall {
    let field = |key| args[key].as_str().map(str::to_owned);
    match name {
        "Execute" => ToolCall::Exec {
            command: string(args, "command").into(),
        },
        "Read" | "LS" => ToolCall::ReadFile {
            path: field("file_path")
                .or_else(|| field("directory_path"))
                .unwrap_or_default(),
        },
        "Create" => ToolCall::WriteFile {
            path: string(args, "file_path").into(),
            content: field("content"),
        },
        "Edit" | "MultiEdit" => ToolCall::EditFile {
            path: string(args, "file_path").into(),
            old_string: field("old_str").or_else(|| field("old_string")),
            new_string: field("new_str").or_else(|| field("new_string")),
        },
        "ApplyPatch" => ToolCall::ApplyPatch {
            path: field("file_path"),
        },
        "Grep" => ToolCall::Search {
            pattern: string(args, "pattern").into(),
            path: field("path"),
        },
        "Glob" => ToolCall::Glob {
            pattern: field("patterns")
                .or_else(|| field("pattern"))
                .unwrap_or_else(|| args["patterns"].to_string()),
        },
        "FetchUrl" => ToolCall::WebFetch {
            url: string(args, "url").into(),
            prompt: None,
        },
        "WebSearch" => ToolCall::WebSearch {
            query: string(args, "query").into(),
        },
        "TodoWrite" => ToolCall::Todo {
            items: string(args, "todos")
                .lines()
                .filter_map(|line| {
                    let (prefix, text) = line.split_once(']')?;
                    let status = prefix.rsplit_once('[')?.1;
                    Some(TodoItem::new(text.trim(), TodoStatus::parse(status)))
                })
                .collect(),
        },
        "Task" => ToolCall::Unknown {
            name: format!(
                "Agent: {}",
                args["description"].as_str().unwrap_or("Droid task")
            ),
            input: Some(args.clone()),
        },
        name if name.contains("___") => {
            let (server, tool) = name.split_once("___").unwrap();
            ToolCall::Mcp {
                server: server.into(),
                tool: tool.into(),
                input: Some(args.clone()),
            }
        }
        _ => ToolCall::Unknown {
            name: name.into(),
            input: Some(args.clone()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn plan_permission_supplies_the_exact_proposal_once_before_approval() {
        let mut norm = Normalizer::default();
        norm.map(&json!({"type":"tool_call","toolUse":{"id":"plan","name":"ExitSpecMode","input":{"plan":"Preliminary"}}}));
        assert!(
            norm.map(
                &json!({"type":"tool_execution_phase_changed","toolUseId":"plan","phase":"queued"})
            )
            .is_empty()
        );
        let permission = json!({
            "confirmationType":"exit_spec_mode",
            "toolUse":{"id":"plan","name":"ExitSpecMode","input":{"plan":"Preliminary"}},
            "details":{"title":"Actual proposal","plan":"1. Create a file\n\n```text\nhello\n```"}
        });
        let events = norm.permission(&permission).unwrap();
        let [AgentEvent::ToolCall { id, call }] = events.as_slice() else {
            panic!("one proposal")
        };
        assert_eq!(id, "plan");
        let plan = zeron_proto::agent_plans::proposed_plan(call).unwrap();
        assert_eq!(plan.title, Some("Actual proposal"));
        assert!(plan.body.starts_with("1. Create"));
        assert!(norm.permission(&permission).unwrap().is_empty());
        let result =
            norm.map(&json!({"type":"tool_result","toolUseId":"plan","content":"accepted"}));
        assert!(matches!(result.as_slice(), [AgentEvent::ToolResult { .. }]));
    }

    #[test]
    fn plan_validation_falls_back_only_when_details_are_absent() {
        let mut norm = Normalizer::default();
        let item = json!({"confirmationType":"exit_spec_mode","toolUse":{"id":"p","name":"ExitSpecMode","input":{"plan":"Fallback"}}});
        assert_eq!(norm.permission(&item).unwrap().len(), 1);
        for body in [json!(""), json!(false), json!("x".repeat(65536))] {
            let mut item = item.clone();
            item["details"] = json!({"plan":body});
            let mut norm = Normalizer::default();
            norm.upsert(&item["toolUse"]);
            assert!(norm.permission(&item).is_err());
            assert!(matches!(
                norm.map(&json!({"type":"tool_result","toolUseId":"p","content":"cancelled"}))
                    .as_slice(),
                [AgentEvent::ToolResult { .. }]
            ));
        }
    }

    #[test]
    fn results_are_capped_and_retractions_are_visible_not_duplicated() {
        let mut norm = Normalizer::default();
        norm.upsert(&json!({"id":"t","name":"Execute","input":{"command":"hello"}}));
        let events = norm.map(&json!({"type":"tool_result","toolUseId":"t","content":"界".repeat(10000),"isError":false}));
        assert_eq!(events.len(), 2);
        assert!(
            matches!(&events[1],AgentEvent::ToolResult { output:Some(output),.. } if output.len()<17000 && output.ends_with("[truncated]"))
        );
        assert!(matches!(
            &norm.map(&json!({"type":"assistant_message_retracted","messageId":"a"}))[0],
            AgentEvent::Error { .. }
        ));
    }
    #[test]
    fn native_tools_and_mcp_keep_their_identities() {
        assert!(
            matches!(tool_call("zeron___whoami",&json!({})),ToolCall::Mcp {server,tool,..} if server=="zeron" && tool=="whoami")
        );
        assert!(tool_call("Task", &json!({"description":"Inspect"})).is_subagent_spawn());
        assert!(
            matches!(tool_call("TodoWrite",&json!({"todos":"1. [completed] done\n2. [in_progress] working"})),ToolCall::Todo {items} if items.len()==2 && items[0].done && items[1].status()==TodoStatus::InProgress)
        );
    }
}
