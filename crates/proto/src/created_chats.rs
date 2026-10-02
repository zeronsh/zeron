//! Zeron MCP `create_chat` / `create_chats` calls, as any consumer of a
//! transcript sees them: which tool calls they are (every harness names MCP
//! tools its own way) and which chats their result says they created.
//!
//! Claude `mcp__zeron__create_chat` and Pi `zeron_create_chat` decode to
//! `ToolCall::Mcp { server: "zeron", .. }`, as do Codex and Cursor; OpenCode
//! `zeron_create_chat` and ACP titles such as `zeron/create_chat` or
//! `create_chat (zeron MCP Server)` stay `ToolCall::Unknown` — so detection
//! keys on the tool token plus a mention of the Zeron server, never on one
//! spelling. The session-doc fold uses [`parse_created_chats`] to keep the
//! created chat ids on the tool part (outputs themselves never enter the
//! doc); the desktop transcript uses both to draw links to those chats.

use std::collections::HashSet;

use crate::ToolCall;

/// Which chat-creation tool a call is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CreateChatOp {
    /// `create_chat`: one chat.
    Single,
    /// `create_chats`: a batch, any number of chats.
    Batch,
}

/// `Some` when `call` is the Zeron MCP server's `create_chat`/`create_chats`
/// under any harness's naming scheme.
pub fn create_chat_op(call: &ToolCall) -> Option<CreateChatOp> {
    let (server, name) = match call {
        ToolCall::Mcp { server, tool, .. } => (Some(server.as_str()), tool.as_str()),
        ToolCall::Unknown { name, .. } => (None, name.as_str()),
        _ => return None,
    };
    let name = name.trim().to_ascii_lowercase().replace('-', "_");
    let (op, bare) = find_op(&name)?;
    let zeron = name.contains("zeron")
        || server.is_some_and(|server| server.to_ascii_lowercase().contains("zeron"));
    // A bare `create_chat` with no server at all (an ACP title that dropped
    // it) is still ours; another MCP server's `create_chat` is not.
    (zeron || (bare && server.is_none())).then_some(op)
}

/// The op token inside a normalized tool name, and whether the name is
/// nothing but the token. The token must stand alone: `create_chat` inside
/// `create_chats` or `create_chat_room` does not count.
fn find_op(name: &str) -> Option<(CreateChatOp, bool)> {
    for (token, op) in [
        ("create_chats", CreateChatOp::Batch),
        ("create_chat", CreateChatOp::Single),
    ] {
        let mut from = 0;
        while let Some(pos) = name[from..].find(token) {
            let start = from + pos;
            let end = start + token.len();
            let before = name[..start]
                .chars()
                .next_back()
                .is_none_or(|c| !c.is_ascii_alphanumeric());
            let after = name[end..]
                .chars()
                .next()
                .is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '_'));
            if before && after {
                return Some((op, name == token));
            }
            from = start + 1;
        }
    }
    None
}

/// One chat a create call reported in its result.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CreatedChat {
    pub chat_id: String,
    /// `kind: "side"` (or a parent id) → `Some(true)`; `kind: "chat"` →
    /// `Some(false)`; unknown when the result predates `kind`.
    pub side: Option<bool>,
    pub device_id: Option<String>,
    pub device_name: Option<String>,
    pub title: Option<String>,
}

/// The chats a `create_chat`/`create_chats` result names, in result order.
/// Accepts the bare result object, the batch's `{results: [{isError,
/// result}]}`, an MCP `CallToolResult` wrapper (`structuredContent`, or
/// `content[].text` holding the JSON), and fenced or double-encoded text.
/// Failed batch entries are skipped; anything unparseable yields nothing.
pub fn parse_created_chats(output: &str) -> Vec<CreatedChat> {
    let text: String = output
        .lines()
        .filter(|line| !line.trim_start().starts_with("```"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut out = Vec::new();
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(text.trim()) {
        collect(&value, &mut out, 0);
    }
    let mut seen = HashSet::new();
    out.retain(|chat: &CreatedChat| seen.insert(chat.chat_id.clone()));
    out
}

fn collect(value: &serde_json::Value, out: &mut Vec<CreatedChat>, depth: usize) {
    use serde_json::Value;
    if depth > 6 {
        return;
    }
    match value {
        Value::Object(map) => {
            if map.get("isError").and_then(Value::as_bool) == Some(true) {
                return;
            }
            if let Some(chat_id) = map
                .get("chatId")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|id| !id.is_empty())
            {
                let text = |key: &str| {
                    map.get(key)
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned)
                };
                let side = match map.get("kind").and_then(Value::as_str) {
                    Some("side") => Some(true),
                    Some("chat") => Some(false),
                    _ => map
                        .get("parentChatId")
                        .map(|parent| !parent.is_null())
                        .filter(|side| *side),
                };
                out.push(CreatedChat {
                    chat_id: chat_id.to_owned(),
                    side,
                    device_id: text("deviceId"),
                    device_name: text("deviceName"),
                    title: text("title"),
                });
                return;
            }
            for key in ["structuredContent", "results", "result"] {
                if let Some(inner) = map.get(key) {
                    let before = out.len();
                    collect(inner, out, depth + 1);
                    if out.len() > before {
                        return;
                    }
                }
            }
            if let Some(Value::Array(content)) = map.get("content") {
                for part in content {
                    if let Some(text) = part.get("text").and_then(Value::as_str) {
                        collect(&Value::String(text.to_owned()), out, depth + 1);
                    }
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                collect(item, out, depth + 1);
            }
        }
        Value::String(text) => {
            if let Ok(inner) = serde_json::from_str::<Value>(text.trim()) {
                collect(&inner, out, depth + 1);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mcp(server: &str, tool: &str) -> ToolCall {
        ToolCall::Mcp {
            server: server.into(),
            tool: tool.into(),
            input: None,
        }
    }

    fn unknown(name: &str) -> ToolCall {
        ToolCall::Unknown {
            name: name.into(),
            input: None,
        }
    }

    #[test]
    fn create_chat_calls_are_recognized_under_every_naming_scheme() {
        use CreateChatOp::{Batch, Single};
        // Claude (`mcp__zeron__create_chat`), Pi (`zeron_create_chat`),
        // Codex and Cursor all decode to `Mcp { server, tool }`.
        assert_eq!(create_chat_op(&mcp("zeron", "create_chat")), Some(Single));
        assert_eq!(create_chat_op(&mcp("zeron", "create_chats")), Some(Batch));
        assert_eq!(create_chat_op(&mcp("Zeron", "create_chat")), Some(Single));
        // OpenCode and ACP titles stay `Unknown`.
        for name in [
            "zeron_create_chat",
            "mcp__zeron__create_chat",
            "zeron/create_chat",
            "zeron.create_chat",
            "create_chat (zeron MCP Server)",
            "Zeron: create-chat",
            "create_chat",
        ] {
            assert_eq!(create_chat_op(&unknown(name)), Some(Single), "{name}");
        }
        for name in ["zeron_create_chats", "mcp__zeron__create_chats", "create_chats"] {
            assert_eq!(create_chat_op(&unknown(name)), Some(Batch), "{name}");
        }
        // Not ours: other servers, other zeron tools, lookalike tokens, and
        // ordinary tool kinds.
        assert_eq!(create_chat_op(&mcp("github", "create_chat")), None);
        assert_eq!(create_chat_op(&mcp("zeron", "list_chats")), None);
        assert_eq!(create_chat_op(&mcp("zeron", "create_chat_room")), None);
        assert_eq!(create_chat_op(&unknown("slack_create_chat")), None);
        assert_eq!(create_chat_op(&unknown("zeron_recreate_chat")), None);
        assert_eq!(create_chat_op(&unknown("Agent: create_chat")), None);
        assert_eq!(
            create_chat_op(&ToolCall::Exec {
                command: "zeron create_chat".into()
            }),
            None
        );
    }

    #[test]
    fn single_results_name_their_chat() {
        let output = serde_json::to_string_pretty(&serde_json::json!({
            "chatId": "c-1",
            "kind": "chat",
            "deviceId": "gpu",
            "deviceName": "GPU box",
            "title": "Train the tokenizer",
            "parentChatId": null,
            "spawnedByChatId": "coordinator",
        }))
        .unwrap();
        assert_eq!(
            parse_created_chats(&output),
            [CreatedChat {
                chat_id: "c-1".into(),
                side: Some(false),
                device_id: Some("gpu".into()),
                device_name: Some("GPU box".into()),
                title: Some("Train the tokenizer".into()),
            }]
        );
        // A side chat without `kind` (older server) reads its parent.
        let side = parse_created_chats(r#"{"chatId":"s-1","parentChatId":"coordinator"}"#);
        assert_eq!(side[0].side, Some(true));
        // Fenced text and a CallToolResult wrapper resolve the same way.
        let fenced = format!("```json\n{output}\n```");
        assert_eq!(parse_created_chats(&fenced)[0].chat_id, "c-1");
        let wrapped = serde_json::json!({
            "content": [{ "type": "text", "text": output }],
        })
        .to_string();
        assert_eq!(parse_created_chats(&wrapped)[0].chat_id, "c-1");
    }

    #[test]
    fn batch_results_name_every_created_chat_and_skip_failures() {
        let output = serde_json::json!({
            "results": [
                { "index": 0, "isError": false, "result": { "chatId": "a", "kind": "chat", "deviceName": "GPU box" } },
                { "index": 1, "isError": true, "result": "device offline" },
                { "index": 2, "isError": false, "result": { "chatId": "b", "kind": "side" } },
            ]
        })
        .to_string();
        let chats = parse_created_chats(&output);
        assert_eq!(
            chats.iter().map(|c| c.chat_id.as_str()).collect::<Vec<_>>(),
            ["a", "b"]
        );
        assert_eq!(chats[0].side, Some(false));
        assert_eq!(chats[1].side, Some(true));
        // structuredContent carries the same batch.
        let structured = serde_json::json!({ "structuredContent": serde_json::from_str::<serde_json::Value>(&output).unwrap() }).to_string();
        assert_eq!(parse_created_chats(&structured).len(), 2);
        // Error text and truncated summaries name nothing.
        assert!(parse_created_chats("device gpu is offline").is_empty());
        assert!(parse_created_chats("{…").is_empty());
        assert!(parse_created_chats(r#"{"isError":true,"chatId":"x"}"#).is_empty());
    }
}
