//! omp session events to Zeron's [`AgentEvent`]s.
//!
//! Text and thinking stream from `message_update` deltas (the `delta`
//! projection the runner selects with `set_event_filter`; the full projection
//! carries the same deltas); `message_end` is authoritative and supplies
//! whatever the deltas missed. Tool calls come from `tool_execution_start` /
//! `_end` (their arguments final, their identity omp's `toolCallId`).
use serde_json::Value;
use std::collections::HashMap;
use zeron_proto::{AgentEvent, DoneStatus, ToolCall, ToolDiff};

/// Bounds on what a tool result carries (as the Pi driver caps them).
const OUTPUT_CAP: usize = 16 * 1024;
const DIFF_CAP: usize = 64 * 1024;

#[derive(Default)]
pub(super) struct Normalizer {
    text: HashMap<usize, String>,
    thinking: HashMap<usize, String>,
    tools: HashMap<String, (String, Value)>,
    /// Host tools by the name omp knows them under: (server, MCP tool).
    pub host_tools: HashMap<String, (String, String)>,
    pub window: Option<u64>,
    /// The last assistant message's outcome in this turn.
    stop: Option<(String, Option<String>)>,
}

impl Normalizer {
    pub fn reset(&mut self) {
        self.text.clear();
        self.thinking.clear();
        self.stop = None;
    }

    /// How the turn ended, read off its last assistant message (omp's own
    /// rule for `prompt_result`); for turns without one (a wake turn).
    pub fn status(&self) -> (DoneStatus, Option<String>) {
        match &self.stop {
            Some((reason, error)) if reason == "error" => (
                DoneStatus::Errored,
                Some(
                    error
                        .clone()
                        .unwrap_or_else(|| "omp provider request failed".into()),
                ),
            ),
            Some((reason, _)) if reason == "aborted" => (DoneStatus::Interrupted, None),
            _ => (DoneStatus::Completed, None),
        }
    }

    pub fn map(&mut self, frame: &Value) -> Vec<AgentEvent> {
        let mut events = Vec::new();
        match string(frame, "type") {
            "message_start" if frame["message"]["role"] == "assistant" => {
                self.text.clear();
                self.thinking.clear();
            }
            "message_update" => {
                let update = &frame["assistantMessageEvent"];
                let index = update["contentIndex"].as_u64().unwrap_or(0) as usize;
                let delta = string(update, "delta");
                match string(update, "type") {
                    "text_delta" if !delta.is_empty() => {
                        self.text.entry(index).or_default().push_str(delta);
                        events.push(AgentEvent::TextDelta { text: delta.into() });
                    }
                    "thinking_delta" if !delta.is_empty() => {
                        self.thinking.entry(index).or_default().push_str(delta);
                        events.push(AgentEvent::ReasoningDelta { text: delta.into() });
                    }
                    _ => {}
                }
            }
            "message_end" if frame["message"]["role"] == "assistant" => {
                let message = &frame["message"];
                self.stop = Some((
                    string(message, "stopReason").to_owned(),
                    message["errorMessage"].as_str().map(str::to_owned),
                ));
                for (index, part) in message["content"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .enumerate()
                {
                    let (cache, value, reasoning) = match string(part, "type") {
                        "text" => (&mut self.text, string(part, "text"), false),
                        "thinking" => (&mut self.thinking, string(part, "thinking"), true),
                        _ => continue,
                    };
                    let seen = cache.entry(index).or_default();
                    if let Some(rest) = value.strip_prefix(seen.as_str()).filter(|s| !s.is_empty())
                    {
                        events.push(if reasoning {
                            AgentEvent::ReasoningDelta { text: rest.into() }
                        } else {
                            AgentEvent::TextDelta { text: rest.into() }
                        });
                    }
                    *seen = value.into();
                }
                let usage = &message["usage"];
                if usage.is_object() {
                    let n = |key| usage[key].as_u64().unwrap_or(0);
                    let input = n("input") + n("cacheRead") + n("cacheWrite");
                    events.push(AgentEvent::Usage {
                        input_tokens: input,
                        output_tokens: n("output"),
                    });
                    if input + n("output") > 0 {
                        events.push(AgentEvent::ContextUsage {
                            tokens: Some(
                                usage["contextTokens"]
                                    .as_u64()
                                    .unwrap_or(input + n("output")),
                            ),
                            window: self.window,
                        });
                    }
                }
            }
            "tool_execution_start" => {
                let id = string(frame, "toolCallId").to_owned();
                let name = string(frame, "toolName");
                if !id.is_empty() && !self.tools.contains_key(&id) {
                    let args = frame["args"].clone();
                    events.push(AgentEvent::ToolCall {
                        id: id.clone(),
                        call: self.tool(name, &args),
                    });
                    self.tools.insert(id, (name.into(), args));
                }
            }
            "tool_execution_end" => {
                let id = string(frame, "toolCallId").to_owned();
                let info = self.tools.remove(&id);
                let result = &frame["result"];
                let is_error = frame["isError"].as_bool().unwrap_or(false);
                let output = result["content"].as_array().map(|items| {
                    items
                        .iter()
                        .filter_map(|v| v["text"].as_str())
                        .collect::<Vec<_>>()
                        .join("\n")
                });
                let diff = info
                    .as_ref()
                    .filter(|_| !is_error)
                    .and_then(|(name, args)| diff(name, args, &result["details"]));
                events.push(AgentEvent::ToolResult {
                    id,
                    is_error,
                    output: output.map(|s| cap(&s, OUTPUT_CAP)),
                    diff,
                    preview_paths: Vec::new(),
                    image_payloads: Vec::new(),
                    exit_code: None,
                });
            }
            // Compaction reports what the context holds now.
            "auto_compaction_end" => {
                if let Some(tokens) = frame["result"]["tokensAfter"].as_u64() {
                    events.push(AgentEvent::ContextUsage {
                        tokens: Some(tokens),
                        window: self.window,
                    });
                }
            }
            // A throwing extension handler is the extension's failure, not
            // the turn's: the run goes on, so it is shown, not fatal.
            "extension_error" => events.push(AgentEvent::Error {
                message: format!("omp extension: {}", string(frame, "error")),
            }),
            "notice" if frame["level"] == "error" => events.push(AgentEvent::Error {
                message: format!("omp: {}", string(frame, "message")),
            }),
            _ => {}
        }
        events
    }

    fn tool(&self, name: &str, args: &Value) -> ToolCall {
        if let Some((server, tool)) = self.host_tools.get(name) {
            return ToolCall::Mcp {
                server: server.clone(),
                tool: tool.clone(),
                input: Some(args.clone()),
            };
        }
        tool(name, args)
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

/// omp's built-in tools (18.x) as Zeron's tool calls.
fn tool(name: &str, args: &Value) -> ToolCall {
    let opt = |key: &str| {
        args[key]
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    match name {
        "bash" => ToolCall::Exec {
            command: string(args, "command").into(),
        },
        // `read` reads URLs too ("use read for static web").
        "read" => match opt("path") {
            Some(url) if url.starts_with("https://") || url.starts_with("http://") => {
                ToolCall::WebFetch { url, prompt: None }
            }
            path => ToolCall::ReadFile {
                path: path.unwrap_or_default(),
            },
        },
        "write" => ToolCall::WriteFile {
            path: string(args, "path").into(),
            content: opt("content"),
        },
        // Hashline patches: `[PATH#TAG]` headers name the files.
        "edit" => {
            let paths = patch_paths(string(args, "input"));
            ToolCall::ApplyPatch {
                path: (paths.len() == 1).then(|| paths[0].clone()),
            }
        }
        "grep" => ToolCall::Search {
            pattern: string(args, "pattern").into(),
            path: opt("path"),
        },
        "glob" => ToolCall::Glob {
            pattern: string(args, "path").into(),
        },
        "web_search" => ToolCall::WebSearch {
            query: string(args, "query").into(),
        },
        _ => ToolCall::Unknown {
            name: name.into(),
            input: Some(args.clone()),
        },
    }
}

/// The files a hashline patch edits, in order: each `[PATH]` or
/// `[PATH#XXXX]` header line (a 4-hex tag), as omp's
/// `unwrapHashlineHeaderPath` reads them.
fn patch_paths(input: &str) -> Vec<String> {
    let mut paths: Vec<String> = Vec::new();
    for line in input.lines() {
        let line = line.trim();
        let Some(inner) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) else {
            continue;
        };
        let path = match inner.rsplit_once('#') {
            Some((path, tag)) if tag.len() == 4 && tag.chars().all(|c| c.is_ascii_hexdigit()) => {
                path
            }
            Some(_) => continue,
            None => inner,
        };
        if !path.is_empty() && !paths.iter().any(|p| p == path) {
            paths.push(path.to_owned());
        }
    }
    paths
}

/// The change an edit-shaped tool made: `edit` reports whole-file
/// snapshots in its details (`path`, `oldText`, `newText`; the first of
/// `perFileResults` for several files), `write` the content it wrote.
fn diff(name: &str, args: &Value, details: &Value) -> Option<ToolDiff> {
    match name {
        "edit" => {
            let file = details["perFileResults"]
                .as_array()
                .and_then(|files| files.first())
                .unwrap_or(details);
            let path = file["path"].as_str()?;
            let new_text = file["newText"].as_str()?;
            Some(ToolDiff {
                path: path.into(),
                old_text: file["oldText"].as_str().map(|t| cap(t, DIFF_CAP)),
                new_text: cap(new_text, DIFF_CAP),
            })
        }
        "write" => Some(ToolDiff {
            path: string(args, "path").into(),
            old_text: None,
            new_text: cap(args["content"].as_str()?, DIFF_CAP),
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn deltas_stream_and_the_final_message_fills_only_what_they_missed() {
        let mut n = Normalizer::default();
        n.map(&json!({"type":"message_start","message":{"role":"assistant","content":[]}}));
        let events = n.map(&json!({"type":"message_update","message":{"role":"assistant"},"assistantMessageEvent":{"type":"thinking_delta","contentIndex":0,"delta":"plan"}}));
        assert!(matches!(&events[0], AgentEvent::ReasoningDelta { text } if text == "plan"));
        n.map(&json!({"type":"message_update","message":{"role":"assistant"},"assistantMessageEvent":{"type":"text_delta","contentIndex":1,"delta":"hel"}}));
        // Recorded shape (omp 18.20.0): the end carries the whole message.
        let events = n.map(
            &json!({"type":"message_end","message":{"role":"assistant","stopReason":"stop",
            "content":[{"type":"thinking","thinking":"planned"},{"type":"text","text":"hello"}],
            "usage":{"input":6196,"output":5,"cacheRead":10,"cacheWrite":0,"totalTokens":6211}}}),
        );
        assert!(matches!(&events[0], AgentEvent::ReasoningDelta { text } if text == "ned"));
        assert!(matches!(&events[1], AgentEvent::TextDelta { text } if text == "lo"));
        assert!(matches!(
            events[2],
            AgentEvent::Usage {
                input_tokens: 6206,
                output_tokens: 5
            }
        ));
        assert_eq!(n.status(), (DoneStatus::Completed, None));
    }

    #[test]
    fn the_last_assistant_message_decides_the_status() {
        let mut n = Normalizer::default();
        n.map(&json!({"type":"message_end","message":{"role":"assistant","stopReason":"error","errorMessage":"rate limited","content":[]}}));
        assert_eq!(
            n.status(),
            (DoneStatus::Errored, Some("rate limited".into()))
        );
        n.map(&json!({"type":"message_end","message":{"role":"assistant","stopReason":"aborted","errorMessage":"Interrupted by user","content":[]}}));
        assert_eq!(n.status().0, DoneStatus::Interrupted);
        n.reset();
        assert_eq!(n.status().0, DoneStatus::Completed);
    }

    #[test]
    fn tools_map_once_with_results_and_diffs() {
        let mut n = Normalizer::default();
        // Recorded (omp 18.20.0).
        let start = json!({"type":"tool_execution_start","toolCallId":"call_1|fc_2","toolName":"bash",
            "args":{"command":"sleep 4; echo done","timeout":10,"pty":false,"async":false},"intent":"Running requested command"});
        assert!(
            matches!(&n.map(&start)[..], [AgentEvent::ToolCall { id, call: ToolCall::Exec { command } }]
            if id == "call_1|fc_2" && command == "sleep 4; echo done")
        );
        assert!(n.map(&start).is_empty(), "a repeated start is one call");
        let end = json!({"type":"tool_execution_end","toolCallId":"call_1|fc_2","toolName":"bash",
            "result":{"content":[{"type":"text","text":"done\n\n\nWall time: 4.04 seconds"}],"details":{}}});
        assert!(
            matches!(&n.map(&end)[..], [AgentEvent::ToolResult { id, is_error: false, output: Some(out), .. }]
            if id == "call_1|fc_2" && out.starts_with("done"))
        );
        n.map(
            &json!({"type":"tool_execution_start","toolCallId":"e","toolName":"edit",
            "args":{"input":"[src/lib.rs#1a2B]\n- old\n+ new\n"}}),
        );
        let events = n.map(&json!({"type":"tool_execution_end","toolCallId":"e","toolName":"edit",
            "result":{"content":[{"type":"text","text":"ok"}],"details":{"path":"src/lib.rs","oldText":"old\n","newText":"new\n","diff":"…"}}}));
        assert!(
            matches!(&events[0], AgentEvent::ToolResult { diff: Some(d), .. }
            if d.path == "src/lib.rs" && d.old_text.as_deref() == Some("old\n") && d.new_text == "new\n")
        );
        // A failed write shows no diff.
        n.map(&json!({"type":"tool_execution_start","toolCallId":"w","toolName":"write","args":{"path":"a","content":"x"}}));
        let events = n.map(
            &json!({"type":"tool_execution_end","toolCallId":"w","toolName":"write","isError":true,
            "result":{"content":[{"type":"text","text":"Tool call denied by user: write"}]}}),
        );
        assert!(matches!(
            &events[0],
            AgentEvent::ToolResult {
                is_error: true,
                diff: None,
                ..
            }
        ));
    }

    #[test]
    fn omp_tools_and_host_tools_map_to_zeron_calls() {
        let mut n = Normalizer::default();
        n.host_tools.insert(
            "zeron_send_message".into(),
            ("zeron".into(), "send_message".into()),
        );
        assert!(matches!(n.tool("zeron_send_message", &json!({"chat":"c"})),
            ToolCall::Mcp { server, tool, .. } if server == "zeron" && tool == "send_message"));
        assert!(
            matches!(tool("read", &json!({"path":"src/a.rs:10-20"})), ToolCall::ReadFile { path } if path == "src/a.rs:10-20")
        );
        assert!(
            matches!(tool("read", &json!({"path":"https://omp.sh/docs"})), ToolCall::WebFetch { url, .. } if url == "https://omp.sh/docs")
        );
        assert!(
            matches!(tool("grep", &json!({"pattern":"fn main","path":"src"})), ToolCall::Search { pattern, path: Some(p) } if pattern == "fn main" && p == "src")
        );
        assert!(
            matches!(tool("glob", &json!({"path":"**/*.rs"})), ToolCall::Glob { pattern } if pattern == "**/*.rs")
        );
        assert!(matches!(
            tool("edit", &json!({"input":"[a.rs#00ff]\n+x\n[b.rs]\n+y\n"})),
            ToolCall::ApplyPatch { path: None }
        ));
        assert!(
            matches!(tool("task", &json!({})), ToolCall::Unknown { name, .. } if name == "task")
        );
        assert_eq!(
            patch_paths("[conf.yaml##host#beef]\n[x#notatag]\n[plain.rs]"),
            ["conf.yaml##host", "plain.rs"]
        );
    }

    #[test]
    fn extension_errors_and_error_notices_show_without_ending_the_turn() {
        let mut n = Normalizer::default();
        let events = n.map(&json!({"type":"extension_error","extensionPath":"/x.ts","event":"tool_call","error":"boom"}));
        assert!(
            matches!(&events[0], AgentEvent::Error { message } if message == "omp extension: boom")
        );
        assert!(
            n.map(&json!({"type":"notice","level":"info","message":"fyi"}))
                .is_empty()
        );
        assert_eq!(
            n.map(&json!({"type":"notice","level":"error","message":"store failed"}))
                .len(),
            1
        );
        assert_eq!(n.status().0, DoneStatus::Completed);
    }
}
