//! What a chat's calls to Zeron's own tools draw in its transcript, read off
//! the call as the doc keeps it (see `zeron_doc::parts::sanitize_tool_call`):
//! the chats a `create_chat`, `create_chats` or `create_task` call started,
//! and a coordinator's `report`. The desktop and mobile transcripts share it.

use crate::{HarnessId, ToolCall};

/// A chat a Zeron launch call started: titled and branded from the call's
/// arguments, and linked to the chat once the call returned (`chatId`).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LaunchedChat {
    pub chat_id: Option<String>,
    pub title: String,
    pub harness: Option<HarnessId>,
    pub model: Option<String>,
}

/// The chats a Zeron launch call starts, and whether it starts a task.
/// `None` for any other call.
pub fn launched_chats(call: &ToolCall) -> Option<(Vec<LaunchedChat>, bool)> {
    let ToolCall::Mcp {
        server,
        tool,
        input,
    } = call
    else {
        return None;
    };
    if server != "zeron" {
        return None;
    }
    let null = serde_json::Value::Null;
    let input = input.as_ref().unwrap_or(&null);
    let requests: Vec<&serde_json::Value> = match tool.as_str() {
        "create_chat" | "create_task" => vec![input],
        "create_chats" => input.get("requests")?.as_array()?.iter().collect(),
        _ => return None,
    };
    let text = |value: &serde_json::Value, key: &str| {
        value
            .get(key)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_owned)
    };
    let chats = requests
        .iter()
        .map(|request| LaunchedChat {
            chat_id: text(request, "chatId"),
            title: text(request, "title")
                .unwrap_or_else(|| if tool == "create_task" { "Task" } else { "Side chat" }.into()),
            harness: text(request, "harness")
                .and_then(|raw| serde_json::from_value(serde_json::Value::String(raw)).ok()),
            model: text(request, "model"),
        })
        .collect::<Vec<_>>();
    (!chats.is_empty()).then_some((chats, tool == "create_task"))
}

/// A coordinator's `report` call: its status (`done`, `question`,
/// `blocked`) and message.
pub fn task_report(call: &ToolCall) -> Option<(String, String)> {
    let ToolCall::Mcp {
        server,
        tool,
        input,
    } = call
    else {
        return None;
    };
    if server != "zeron" || tool != "report" {
        return None;
    }
    let field = |key: &str| {
        input
            .as_ref()
            .and_then(|i| i.get(key))
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_owned()
    };
    Some((field("status"), field("message")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn zeron(tool: &str, input: serde_json::Value) -> ToolCall {
        ToolCall::Mcp {
            server: "zeron".into(),
            tool: tool.into(),
            input: Some(input),
        }
    }

    #[test]
    fn launches_name_their_chats_and_tasks_say_so() {
        let (chats, task) = launched_chats(&zeron(
            "create_task",
            json!({"title": "Sitekit Audit", "harness": "claude-code", "chatId": "t1"}),
        ))
        .unwrap();
        assert!(task);
        assert_eq!(chats[0].title, "Sitekit Audit");
        assert_eq!(chats[0].chat_id.as_deref(), Some("t1"));
        assert_eq!(chats[0].harness, Some(HarnessId::ClaudeCode));
        let (chats, task) = launched_chats(&zeron(
            "create_chats",
            json!({"requests": [{"title": "A"}, {}]}),
        ))
        .unwrap();
        assert!(!task);
        assert_eq!(chats[1].title, "Side chat");
        assert!(launched_chats(&zeron("send_message", json!({}))).is_none());
        assert_eq!(
            task_report(&zeron("report", json!({"status": "done", "message": "Shipped"}))),
            Some(("done".into(), "Shipped".into()))
        );
    }
}
