//! Factory notifications are deltas, not complete assistant messages. Tool
//! snapshots upsert by id; ExitSpecMode belongs to the approval bridge.
use std::collections::HashMap;

use serde_json::Value;
use zeron_proto::{AgentEvent, DoneStatus, ToolCall};

pub(super) fn string<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or("")
}

/// One transcript's tool calls, by id.
#[derive(Default)]
pub(super) struct Normalizer {
    tools: HashMap<String, ToolCall>,
}

impl Normalizer {
    pub fn is_spawn(&self, id: &str) -> bool {
        self.tools.get(id).is_some_and(ToolCall::is_subagent_spawn)
    }

    pub fn spawn_prompt(&self, id: &str) -> Option<&str> {
        let ToolCall::Unknown {
            input: Some(input), ..
        } = self.tools.get(id)?
        else {
            return None;
        };
        input["prompt"]
            .as_str()
            .filter(|text| !text.is_empty())
            .or_else(|| input["description"].as_str())
    }

    pub fn event(&mut self, notification: &Value) -> Option<AgentEvent> {
        match string(notification, "type") {
            "assistant_text_delta" => Some(AgentEvent::TextDelta {
                text: string(notification, "textDelta").into(),
            }),
            "thinking_text_delta" => Some(AgentEvent::ReasoningDelta {
                text: string(notification, "textDelta").into(),
            }),
            "tool_call" => self.tool_call(&notification["toolUse"]),
            "tool_result" => self.tool_result(
                string(notification, "toolUseId"),
                &notification["content"],
                notification["isError"].as_bool().unwrap_or(false),
            ),
            "session_token_usage_changed" => {
                let usage = &notification["tokenUsage"];
                Some(AgentEvent::Usage {
                    input_tokens: usage["inputTokens"].as_u64().unwrap_or(0)
                        + usage["cacheReadTokens"].as_u64().unwrap_or(0)
                        + usage["cacheCreationTokens"].as_u64().unwrap_or(0),
                    output_tokens: usage["outputTokens"].as_u64().unwrap_or(0),
                })
            }
            "error" => Some(AgentEvent::Error {
                message: sign_in_error(string(notification, "message")),
            }),
            _ => None,
        }
    }

    /// A `tool_use` snapshot: each call arrives with empty input first, then
    /// again once its input is known.
    pub fn tool_call(&mut self, tool: &Value) -> Option<AgentEvent> {
        let id = string(tool, "id");
        let name = string(tool, "name");
        if id.is_empty() || name.is_empty() || name == "ExitSpecMode" {
            return None;
        }
        let call = tool_call(name, &tool["input"]);
        if self.tools.get(id) == Some(&call) {
            return None;
        }
        self.tools.insert(id.into(), call.clone());
        Some(AgentEvent::ToolCall {
            id: id.into(),
            call,
        })
    }

    /// The result of a call this transcript showed.
    pub fn tool_result(&mut self, id: &str, content: &Value, is_error: bool) -> Option<AgentEvent> {
        self.tools.contains_key(id).then(|| AgentEvent::ToolResult {
            id: id.into(),
            is_error,
            output: zeron_proto::command_output_cap(&text(content)),
            diff: None,
            exit_code: None,
            preview_paths: Vec::new(),
            image_payloads: Vec::new(),
        })
    }
}

fn text(content: &Value) -> String {
    if let Some(text) = content.as_str() {
        return text.into();
    }
    content
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|block| block.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n")
}

pub(super) fn terminal(notification: &Value) -> DoneStatus {
    match string(notification, "reason") {
        "completed" => DoneStatus::Completed,
        "cancelled" => DoneStatus::Interrupted,
        _ => DoneStatus::Errored,
    }
}

/// The request ids `queued_messages_discarded` hands back, read as Factory's
/// SDK reads them: the drained ids when listed, else each message's, else the
/// notification's own (the two older shapes).
pub(super) fn discarded(notification: &Value) -> Vec<&str> {
    if let Some(drained) = notification["drainedRequestIds"].as_array() {
        drained.iter().filter_map(Value::as_str).collect()
    } else if let Some(messages) = notification["messages"].as_array() {
        messages
            .iter()
            .filter_map(|message| message["requestId"].as_str())
            .collect()
    } else {
        notification["requestId"].as_str().into_iter().collect()
    }
}

pub(super) fn sign_in_error(message: &str) -> String {
    let lower = message.to_ascii_lowercase();
    let words: Vec<_> = lower
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .collect();
    if [
        &["unauthorized"][..],
        &["authentication", "required"],
        &["authentication", "failed"],
        &["authentication", "failure"],
        &["authentication", "error"],
        &["not", "authenticated"],
        &["invalid", "api", "key"],
        &["missing", "api", "key"],
        &["api", "key", "required"],
        &["api", "key", "is", "invalid"],
        &["api", "key", "is", "missing"],
        &["api", "key", "not", "found"],
        &["not", "logged", "in"],
        &["login", "required"],
        &["sign", "in", "required"],
        &["please", "log", "in"],
        &["please", "sign", "in"],
    ]
    .iter()
    .any(|phrase| words.windows(phrase.len()).any(|part| part == *phrase))
    {
        authentication_error(message)
    } else {
        message.into()
    }
}

pub(super) fn authentication_error(message: &str) -> String {
    const GUIDANCE: &str = "Run `droid` and `/login`, or set FACTORY_API_KEY, then retry.";
    if message.ends_with(GUIDANCE) {
        message.into()
    } else {
        format!("{message}\n{GUIDANCE}")
    }
}

pub(super) fn tool_call(name: &str, input: &Value) -> ToolCall {
    let path = || string(input, "file_path").to_owned();
    match name {
        "Execute" => ToolCall::Exec {
            command: string(input, "command").into(),
        },
        "Read" => ToolCall::ReadFile { path: path() },
        "Create" => ToolCall::WriteFile {
            path: path(),
            content: input["content"].as_str().map(str::to_owned),
        },
        "Edit" => ToolCall::EditFile {
            path: path(),
            old_string: input["old_str"].as_str().map(str::to_owned),
            new_string: input["new_str"].as_str().map(str::to_owned),
        },
        "ApplyPatch" => ToolCall::ApplyPatch { path: None },
        "Grep" => ToolCall::Search {
            pattern: string(input, "pattern").into(),
            path: input["path"].as_str().map(str::to_owned),
        },
        "Glob" => ToolCall::Glob {
            pattern: match &input["patterns"] {
                Value::String(pattern) => pattern.clone(),
                Value::Array(patterns) => patterns
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(", "),
                _ => String::new(),
            },
        },
        "FetchUrl" => ToolCall::WebFetch {
            url: string(input, "url").into(),
            prompt: None,
        },
        "WebSearch" => ToolCall::WebSearch {
            query: string(input, "query").into(),
        },
        "TodoWrite" => ToolCall::Todo {
            items: todos(string(input, "todos")),
        },
        "Task" => ToolCall::Unknown {
            name: match string(input, "description") {
                "" => "Agent".into(),
                description => format!("Agent: {description}"),
            },
            input: Some(input.clone()),
        },
        _ => match name.split_once("___") {
            Some((server, tool)) => ToolCall::Mcp {
                server: server.into(),
                tool: tool.into(),
                input: Some(input.clone()),
            },
            None => ToolCall::Unknown {
                name: name.into(),
                input: Some(input.clone()),
            },
        },
    }
}

fn todos(text: &str) -> Vec<zeron_proto::TodoItem> {
    text.lines()
        .filter_map(|line| {
            let (prefix, body) = line.split_once(']')?;
            let status = prefix.rsplit_once('[')?.1;
            Some(zeron_proto::TodoItem::new(
                body.trim(),
                zeron_proto::TodoStatus::parse(status),
            ))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn droid_auth_guidance_matches_precise_failures_not_author_words() {
        for message in [
            "Unauthorized",
            "Authentication required",
            "Invalid API key",
            "API key not found",
            "You are not logged in.",
            "Please sign in.",
        ] {
            assert!(sign_in_error(message).ends_with("then retry."), "{message}");
        }
        for message in [
            "Could not find the author",
            "Missing authorization metadata in the document",
            "Failed to write authentication.md",
            "Edit login settings",
            "Read the API key documentation",
            "The user is unauthorizedly editing",
        ] {
            assert_eq!(sign_in_error(message), message);
        }
        let guided = sign_in_error("Authentication failed");
        assert_eq!(authentication_error(&guided), guided);
    }

    #[test]
    fn droid_discarded_requests_follow_the_sdk_precedence() {
        let both = json!({"drainedRequestIds":["1","2"],"messages":[{"requestId":"3","text":""}],"requestId":"4"});
        assert_eq!(discarded(&both), ["1", "2"]);
        assert!(discarded(&json!({"drainedRequestIds":[],"requestId":"4"})).is_empty());
        let messages =
            json!({"messages":[{"requestId":"3","text":""},{"text":""}],"requestId":"4"});
        assert_eq!(discarded(&messages), ["3"]);
        assert_eq!(discarded(&json!({"requestId":"4","text":""})), ["4"]);
        assert!(discarded(&json!({"text":""})).is_empty());
    }
}
