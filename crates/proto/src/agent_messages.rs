//! Messages between chats through Zeron's `send_message`. The receiving side
//! carries a routing header the agent reads in the stored prompt, while
//! transcripts split it off and show the sender over the bubble. The sending
//! side is the tool call itself, which transcripts show as the message it sent
//! rather than as a tool.

use crate::ToolCall;

const PREFIX: &str = "[Message from Zeron chat ";
const REPLY: &str = ". Reply to it with the Zeron `send_message` tool, chat ";

/// `text` as sent from chat `chat_id`, labelled `Title (short-id)` (or the
/// short id alone when untitled).
pub fn agent_message(label: &str, chat_id: &str, text: &str) -> String {
    format!("{PREFIX}{label}{REPLY}{chat_id}.]\n\n{text}")
}

/// The chat an agent-to-agent message came from: its id, and its title as the
/// header recorded it (the short id for an untitled chat).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentSender {
    pub chat_id: String,
    pub title: String,
}

/// Split the routing header off a message an agent sent: its sender and the
/// body under the header. `None` for anything else, including a typed message
/// that merely starts alike.
pub fn agent_message_sender(text: &str) -> Option<(AgentSender, &str)> {
    let rest = text.strip_prefix(PREFIX)?;
    let (header, body) = rest.split_once("]\n\n")?;
    let (label, id) = header.rsplit_once(REPLY)?;
    let id = id.strip_suffix('.')?;
    // Only the trailing short id goes; a title's own parens stay.
    let title = label
        .strip_suffix(')')
        .and_then(|label| label.rsplit_once(" ("))
        .filter(|(_, short)| !short.is_empty() && id.starts_with(short))
        .map_or(label, |(title, _)| title);
    Some((
        AgentSender {
            chat_id: id.to_owned(),
            title: title.split_whitespace().collect::<Vec<_>>().join(" "),
        },
        body,
    ))
}

/// A message a chat sent another: the recipient as the call named it (an id,
/// a unique id prefix, or an exact title; see [`resolve_chat_key`]) and the
/// text it sent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutgoingMessage {
    pub chat: String,
    pub text: String,
}

/// The messages a Zeron `send_message` or `send_messages` call sends, in
/// order. `None` for any other call, and for one whose arguments haven't
/// named a recipient and a text yet; it then reads as an ordinary tool.
pub fn outgoing_messages(call: &ToolCall) -> Option<Vec<OutgoingMessage>> {
    let ToolCall::Mcp { server, tool, input } = call else {
        return None;
    };
    if server != "zeron" {
        return None;
    }
    let one = |args: &serde_json::Value| {
        let field = |key| args.get(key)?.as_str().map(str::trim).filter(|v| !v.is_empty());
        Some(OutgoingMessage {
            chat: field("chat")?.to_owned(),
            text: field("text")?.to_owned(),
        })
    };
    let input = input.as_ref()?;
    let messages = match tool.as_str() {
        "send_message" => vec![one(input)?],
        "send_messages" => input
            .get("requests")?
            .as_array()?
            .iter()
            .map(one)
            .collect::<Option<Vec<_>>>()?,
        _ => return None,
    };
    (!messages.is_empty()).then_some(messages)
}

/// The chat a `send_message` key names, as the Zeron MCP server resolves it:
/// an exact id, else a unique id prefix, else a unique title (any case).
/// `chats` holds each chat's id and title.
pub fn resolve_chat_key<'a>(key: &str, chats: &[(&'a str, Option<&str>)]) -> Option<&'a str> {
    let key = key.trim();
    if key.is_empty() {
        return None;
    }
    if let Some((id, _)) = chats.iter().find(|(id, _)| *id == key) {
        return Some(id);
    }
    let only = |mut found: Vec<&'a str>| (found.len() == 1).then(|| found.remove(0));
    let by_prefix: Vec<&'a str> = chats.iter().map(|(id, _)| *id).filter(|id| id.starts_with(key)).collect();
    match by_prefix.len() {
        1 => only(by_prefix),
        0 => only(
            chats
                .iter()
                .filter(|(_, title)| title.is_some_and(|t| t.trim().eq_ignore_ascii_case(key)))
                .map(|(id, _)| *id)
                .collect(),
        ),
        // An ambiguous prefix names nothing: the server refuses it.
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_the_sender_off_the_body() {
        let id = "cd27c5c2-0000-4000-8000-000000000000";
        let text = agent_message("zgui prod: Windows (WING-PC) (cd27c5c2)", id, "Windows lane: done.");
        let (sender, body) = agent_message_sender(&text).unwrap();
        assert_eq!(body, "Windows lane: done.");
        assert_eq!(
            sender,
            AgentSender {
                chat_id: id.into(),
                title: "zgui prod: Windows (WING-PC)".into(),
            }
        );
        let untitled = agent_message("cd27c5c2", id, "hi");
        let (sender, body) = agent_message_sender(&untitled).unwrap();
        assert_eq!((sender.title.as_str(), body), ("cd27c5c2", "hi"));
        assert!(agent_message_sender("[Message from Zeron chat x]\n\nhi").is_none());
    }

    #[test]
    fn reads_the_messages_a_send_call_sends() {
        let call = |tool: &str, input: serde_json::Value| ToolCall::Mcp {
            server: "zeron".into(),
            tool: tool.into(),
            input: Some(input),
        };
        let sent = outgoing_messages(&call("send_message", serde_json::json!({ "chat": "Windows lane", "text": " Run it. ", "wait": true })));
        assert_eq!(sent, Some(vec![OutgoingMessage { chat: "Windows lane".into(), text: "Run it.".into() }]));
        let batch = outgoing_messages(&call(
            "send_messages",
            serde_json::json!({ "requests": [{ "chat": "a", "text": "one" }, { "chat": "b", "text": "two" }] }),
        ))
        .unwrap();
        assert_eq!(batch.iter().map(|m| m.chat.as_str()).collect::<Vec<_>>(), ["a", "b"]);
        // Not yet a message: no text; another tool; another server.
        assert!(outgoing_messages(&call("send_message", serde_json::json!({ "chat": "a" }))).is_none());
        assert!(outgoing_messages(&call("read_chat", serde_json::json!({ "chat": "a", "text": "x" }))).is_none());
        let other = ToolCall::Mcp { server: "github".into(), tool: "send_message".into(), input: Some(serde_json::json!({ "chat": "a", "text": "x" })) };
        assert!(outgoing_messages(&other).is_none());
    }

    #[test]
    fn resolves_a_chat_key_like_the_mcp_server() {
        let chats = [("cd27c5c2-1", Some("Windows lane")), ("cd27c5c2-2", None), ("ab12", Some("Docs"))];
        let resolve = |key| resolve_chat_key(key, &chats);
        assert_eq!(resolve("ab12"), Some("ab12"));
        assert_eq!(resolve("ab"), Some("ab12"));
        assert_eq!(resolve("windows LANE"), Some("cd27c5c2-1"));
        assert_eq!(resolve("cd27"), None);
        assert_eq!(resolve("nope"), None);
    }
}
