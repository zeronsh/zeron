//! Double-Escape rewind list: jump back to one of your earlier prompts in the
//! open chat and put it back in the composer to edit and resend.
//!
//! ## What this does and does not do
//!
//! Selecting a prompt restores its text and scrolls the transcript to it. It
//! does **not** delete the turns that followed, and it does not roll the agent
//! back. It cannot: the harness CLI owns the conversation, and the app resumes
//! it by `RunRequest::resume` (a harness-native session id) rather than
//! replaying history out of the doc. Truncating our mirror would leave the
//! agent still remembering everything the transcript no longer showed — a
//! worse lie than not truncating at all. Real truncation needs harness-side
//! support plumbed through the `Harness` trait and the command ledger.
//!
//! The extraction below is pure and tested; the shell owns the overlay.

use zeron_doc::{MessagePart, MessageRole, SessionMessageEntry};

/// One restorable prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RewindPrompt {
    /// Transcript entry id — the scroll target.
    pub message_id: String,
    /// The visible prompt, attachment trailer stripped.
    pub text: String,
    /// Epoch millis, for the age label.
    pub created_at: i64,
}

/// Restorable prompts for a transcript, **newest first** — the order the list
/// is read in, so the highlight starts on the most recent prompt.
///
/// Skips anything with nothing to restore: non-user entries, continuations
/// (folded tails of an entry already listed, which would double it up), and
/// image-only sends whose visible text is empty once the attachment trailer
/// comes off.
///
/// Allocates the result at the transcript's length as an upper bound — one
/// allocation regardless of how many entries qualify.
pub fn rewind_prompts(transcript: &[SessionMessageEntry]) -> Vec<RewindPrompt> {
    let mut out = Vec::with_capacity(transcript.len());
    for entry in transcript {
        if entry.role != MessageRole::User || entry.continuation_of.is_some() {
            continue;
        }
        let text = visible_prompt(entry);
        if text.is_empty() {
            continue;
        }
        out.push(RewindPrompt {
            message_id: entry.id.clone(),
            text,
            created_at: entry.created_at,
        });
    }
    out.reverse();
    out
}

/// The prompt a user actually typed: text parts joined, then the machine-added
/// `Attached images (local files …)` trailer stripped. Restoring that trailer
/// would put generated bookkeeping into the composer as if it were prose.
fn visible_prompt(entry: &SessionMessageEntry) -> String {
    let mut joined = String::new();
    for part in &entry.parts {
        if let MessagePart::Text { text, .. } = part {
            if !joined.is_empty() {
                joined.push('\n');
            }
            joined.push_str(text);
        }
    }
    crate::attachments::parse_user_message_images(&joined)
        .text
        .trim()
        .to_string()
}

/// One line of a prompt for the list row. Collapses the prompt to its first
/// non-empty line and caps it, so a pasted wall of text cannot blow the row
/// height out. Cuts on a `char` boundary, never a byte one.
pub fn preview(text: &str, max_chars: usize) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    if line.chars().count() <= max_chars {
        return line.to_string();
    }
    let cut: String = line.chars().take(max_chars.saturating_sub(1)).collect();
    format!("{}…", cut.trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: &str, role: MessageRole, text: &str, at: i64) -> SessionMessageEntry {
        SessionMessageEntry {
            id: id.into(),
            role,
            parts: vec![MessagePart::Text {
                id: format!("{id}-p"),
                text: text.into(),
            }],
            created_at: at,
            device_id: "d".into(),
            status: None,
            continuation_of: None,
        }
    }

    #[test]
    fn lists_user_prompts_newest_first() {
        let transcript = vec![
            entry("a", MessageRole::User, "first", 1),
            entry("b", MessageRole::Assistant, "reply", 2),
            entry("c", MessageRole::User, "second", 3),
        ];
        let prompts = rewind_prompts(&transcript);
        let ids: Vec<&str> = prompts.iter().map(|p| p.message_id.as_str()).collect();
        assert_eq!(ids, ["c", "a"]);
    }

    #[test]
    fn strips_the_attachment_trailer() {
        let raw =
            "look at this\n\nAttached images (local files — open them to view):\n- /tmp/a.png";
        let prompts = rewind_prompts(&[entry("a", MessageRole::User, raw, 1)]);
        assert_eq!(prompts[0].text, "look at this");
    }

    #[test]
    fn skips_image_only_sends() {
        let raw = "\n\nAttached images (local files — open them to view):\n- /tmp/a.png";
        assert!(rewind_prompts(&[entry("a", MessageRole::User, raw, 1)]).is_empty());
    }

    #[test]
    fn skips_continuations_so_one_send_lists_once() {
        let mut tail = entry("a2", MessageRole::User, "more", 2);
        tail.continuation_of = Some("a".into());
        let prompts = rewind_prompts(&[entry("a", MessageRole::User, "start", 1), tail]);
        assert_eq!(prompts.len(), 1);
        assert_eq!(prompts[0].message_id, "a");
    }

    #[test]
    fn joins_multiple_text_parts() {
        let mut e = entry("a", MessageRole::User, "one", 1);
        e.parts.push(MessagePart::Text {
            id: "p2".into(),
            text: "two".into(),
        });
        assert_eq!(rewind_prompts(&[e])[0].text, "one\ntwo");
    }

    #[test]
    fn preview_takes_the_first_real_line() {
        assert_eq!(preview("\n\nhello\nworld", 40), "hello");
    }

    #[test]
    fn preview_caps_length_without_splitting_a_char() {
        // Multi-byte throughout: a byte-slice cut here would panic.
        let out = preview("ααααααααααααα", 5);
        assert_eq!(out.chars().count(), 5);
        assert!(out.ends_with('…'));
    }

    #[test]
    fn preview_of_empty_text_is_empty() {
        assert_eq!(preview("   \n  ", 10), "");
    }
}
