//! Chat-to-chat traffic as it is stored and as people see it.
//!
//! When one chat's agent messages another through the Zeron MCP tools, the
//! prompt carries a bracketed header naming the sender and saying how the
//! answer is routed; when the receiving turn ends, the sender gets an
//! automatic notice with the result. Agents need those headers verbatim.
//! People reading a transcript or a sidebar preview want "Message from
//! Worker" / "Update from Worker" instead. Both sides live here so the
//! writer (`zeron-mcp`) and the readers (UI, previews) never drift.

/// Opens an agent-to-agent message: `[Message from Zeron chat {label}, id
/// {id}. {routing}]\n\n{text}`.
pub const MESSAGE_PREFIX: &str = "[Message from Zeron chat ";
/// Opens each section of an automatic notice: `[Zeron notice: chat {label}
/// finished its turn.]\n\n{response}`.
pub const NOTICE_PREFIX: &str = "[Zeron notice: chat ";
/// Closes every notice.
pub const NOTICE_FOOTER: &str = "[Automatic notice from Zeron, not a message from a person or another agent. No reply is needed: carry on, or end your turn if nothing is left to do. Use read_chat for a full transcript and send_message to follow up.]";

/// The section endings a notice header can carry, with how people see them.
const NOTICE_KINDS: [(&str, &str); 3] = [
    (" finished its turn.]", "Update from "),
    (" is waiting for an answer.", "Question from "),
    (" stopped with an error.]", "Error from "),
];

/// Display leads; a line starting with one of these names a sender.
pub const DISPLAY_LEADS: [&str; 4] = [
    "Message from ",
    "Update from ",
    "Question from ",
    "Error from ",
];

/// A notice section label: `"Title" (id)` or `(id)`.
pub fn notice_label(title: Option<&str>, chat_id: &str) -> String {
    match title.map(str::trim).filter(|t| !t.is_empty()) {
        Some(title) => format!("\"{}\" ({chat_id})", title.replace('"', "'")),
        None => format!("({chat_id})"),
    }
}

/// How a person should see `text`, or `None` when it is not chat-to-chat
/// traffic. Headers become "Message from …" / "Update from …" lines and the
/// notice footer is dropped; bodies are untouched.
pub fn display(text: &str) -> Option<String> {
    if text.starts_with(NOTICE_PREFIX) {
        return Some(notice_display(text));
    }
    message_display(text)
}

/// A one-line preview (sidebar rows): the display text with paragraph
/// breaks folded, e.g. "Update from Worker: All tests pass".
pub fn preview(text: &str) -> String {
    let Some(display) = display(text) else {
        return text.to_owned();
    };
    let mut lines = display.lines().map(str::trim).filter(|l| !l.is_empty());
    let Some(first) = lines.next() else {
        return String::new();
    };
    let rest: Vec<&str> = lines.collect();
    if rest.is_empty() {
        first.to_owned()
    } else {
        format!("{first}: {}", rest.join(" "))
    }
}

/// Byte ranges of sender names on [`DISPLAY_LEADS`] lines, for bolding.
/// Only display text that itself opens with a lead qualifies, so a typed
/// message that happens to contain "Update from" mid-way is left alone.
pub fn sender_ranges(display: &str) -> Vec<std::ops::Range<usize>> {
    if !DISPLAY_LEADS.iter().any(|lead| display.starts_with(lead)) {
        return Vec::new();
    }
    let mut ranges = Vec::new();
    let mut at = 0;
    for line in display.split_inclusive('\n') {
        let content = line.trim_end_matches('\n');
        if let Some(lead) = DISPLAY_LEADS.iter().find(|lead| content.starts_with(*lead))
            && content.len() > lead.len()
        {
            ranges.push(at + lead.len()..at + content.len());
        }
        at += line.len();
    }
    ranges
}

fn message_display(text: &str) -> Option<String> {
    let rest = text.strip_prefix(MESSAGE_PREFIX)?;
    let (header, body) = rest.split_once("]\n\n")?;
    // Current: "{label}, id {id}. {routing}". Before that: "{label}. Reply to
    // it with the Zeron `send_message` tool, chat {id}." (still in old
    // transcripts).
    let (label, id) = if let Some((label, tail)) = header.split_once(", id ") {
        (label, tail.split_once(". ").map_or(tail, |(id, _)| id))
    } else {
        let (label, id) =
            header.rsplit_once(". Reply to it with the Zeron `send_message` tool, chat ")?;
        (label, id.strip_suffix('.')?)
    };
    let short = &id[..id.len().min(8)];
    let name = label
        .strip_suffix(&format!(" ({short})"))
        .or_else(|| label.strip_suffix(&format!(" ({id})")))
        .unwrap_or(label);
    Some(format!("Message from {name}\n\n{body}"))
}

fn notice_display(text: &str) -> String {
    let text = text.strip_suffix(NOTICE_FOOTER).unwrap_or(text).trim_end();
    text.split('\n')
        .map(|line| notice_header_display(line).unwrap_or_else(|| line.to_owned()))
        .collect::<Vec<_>>()
        .join("\n")
}

/// `[Zeron notice: chat "Title" (id) finished its turn.]` → `Update from Title`.
fn notice_header_display(line: &str) -> Option<String> {
    let event = notice_event(line)?;
    let lead = match event.kind {
        NoticeKind::Finished => "Update from ",
        NoticeKind::Question => "Question from ",
        NoticeKind::Error => "Error from ",
    };
    Some(format!("{lead}{}", event.name))
}

/// What a notice section reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeKind {
    Finished,
    Question,
    Error,
}

/// One chat's report inside a notice, as the transcript shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoticeEvent {
    pub chat_id: String,
    /// The chat's title, else its short id.
    pub name: String,
    pub kind: NoticeKind,
}

/// The reports in a notice message, in order; `None` when `text` is not one.
pub fn notice_events(text: &str) -> Option<Vec<NoticeEvent>> {
    if !text.starts_with(NOTICE_PREFIX) {
        return None;
    }
    let events: Vec<NoticeEvent> = text.lines().filter_map(notice_event).collect();
    (!events.is_empty()).then_some(events)
}

fn notice_event(line: &str) -> Option<NoticeEvent> {
    let rest = line.strip_prefix(NOTICE_PREFIX)?;
    let (name, chat_id, after) = if let Some(quoted) = rest.strip_prefix('"') {
        let (title, after) = quoted.split_once("\" (")?;
        let (id, after) = after.split_once(')')?;
        (title.to_owned(), id, after)
    } else {
        let inner = rest.strip_prefix('(')?;
        let (id, after) = inner.split_once(')')?;
        (id[..id.len().min(8)].to_owned(), id, after)
    };
    let kind = [
        NoticeKind::Finished,
        NoticeKind::Question,
        NoticeKind::Error,
    ]
    .into_iter()
    .zip(NOTICE_KINDS)
    .find(|(_, (ending, _))| after.starts_with(ending))?
    .0;
    Some(NoticeEvent {
        chat_id: chat_id.to_owned(),
        name,
        kind,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_show_their_sender() {
        let current = "[Message from Zeron chat Lead (0f3a9c21), id 0f3a9c21-aaaa-bbbb. Your final response goes back to it automatically when this turn ends, so don't reply with send_message.]\n\nRun the tests";
        assert_eq!(
            display(current).unwrap(),
            "Message from Lead\n\nRun the tests"
        );
        let untitled =
            "[Message from Zeron chat 0f3a9c21, id 0f3a9c21-aaaa. It doesn't need a reply.]\n\nfyi";
        assert_eq!(display(untitled).unwrap(), "Message from 0f3a9c21\n\nfyi");
        let legacy = "[Message from Zeron chat Beta (chat-bet). Reply to it with the Zeron `send_message` tool, chat chat-beta-2.]\n\nplease review";
        assert_eq!(
            display(legacy).unwrap(),
            "Message from Beta\n\nplease review"
        );
        assert_eq!(display("just text"), None);
    }

    #[test]
    fn notices_read_as_updates_without_the_footer() {
        let notice = format!(
            "{NOTICE_PREFIX}{} finished its turn.]\n\nAll green.\n\n{NOTICE_PREFIX}{} is waiting for an answer. Answer it with respond_to_input.]\n\nWhich API?\n\n{NOTICE_PREFIX}{} stopped with an error.]\n\nboom\n\n{NOTICE_FOOTER}",
            notice_label(Some("Fix \"tests\""), "child-a"),
            notice_label(None, "abcdef0123456"),
            notice_label(Some("Docs"), "child-c"),
        );
        let shown = display(&notice).unwrap();
        assert_eq!(
            shown,
            "Update from Fix 'tests'\n\nAll green.\n\nQuestion from abcdef01\n\nWhich API?\n\nError from Docs\n\nboom"
        );
        let names: Vec<&str> = sender_ranges(&shown)
            .into_iter()
            .map(|r| &shown[r])
            .collect();
        assert_eq!(names, ["Fix 'tests'", "abcdef01", "Docs"]);
        assert_eq!(preview(&notice).lines().count(), 1);
        assert!(preview(&notice).starts_with("Update from Fix 'tests': All green."));
    }

    #[test]
    fn notice_events_name_each_reporting_chat() {
        let notice = format!(
            "{NOTICE_PREFIX}{} finished its turn.]\n\nok\n\n{NOTICE_PREFIX}{} is waiting for an answer. Answer it with respond_to_input.]\n\n?\n\n{NOTICE_FOOTER}",
            notice_label(Some("Read zasm files C"), "child-c"),
            notice_label(None, "abcdef0123456"),
        );
        assert_eq!(
            notice_events(&notice).unwrap(),
            [
                NoticeEvent {
                    chat_id: "child-c".into(),
                    name: "Read zasm files C".into(),
                    kind: NoticeKind::Finished,
                },
                NoticeEvent {
                    chat_id: "abcdef0123456".into(),
                    name: "abcdef01".into(),
                    kind: NoticeKind::Question,
                },
            ]
        );
        assert_eq!(notice_events("hello"), None);
    }

    #[test]
    fn only_display_text_opening_with_a_lead_is_bolded() {
        assert!(sender_ranges("hi\nUpdate from me").is_empty());
        assert_eq!(sender_ranges("Message from Lead\n\nx"), [13..17]);
    }
}
