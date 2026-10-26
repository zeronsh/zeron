//! The routing header on a message one chat sends another through Zeron's
//! `send_message`: the receiving agent reads it in the stored prompt, while
//! transcripts split it off and show the sender over the bubble.

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
}
