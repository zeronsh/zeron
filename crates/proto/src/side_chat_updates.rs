//! What a chat's agent hears about the side chats it started, and how the
//! transcript reads it back.
//!
//! A side chat never messages its parent to report: when its turn ends, its
//! host leaves a notice on the parent's doc, and the parent's host hands every
//! pending notice to the parent's agent in one block, either at the head of
//! the next turn the user starts or, when none comes, as a turn of its own once
//! the agent is free. Never mid-turn: an update must not steer an agent off
//! what it is telling its user.
//!
//! The block rides the prompt (agents only take prompts), so the transcript
//! splits it off and draws a compact line per side chat instead of a bubble.
//! A turn the block starts on its own may end with [`QUIET_REPLY`] alone,
//! which the transcript hides: the agent took the update in and has nothing
//! for the user yet.

use serde::{Deserialize, Serialize};

const OPEN: &str = "<zeron-side-chat-updates>\n";
const CLOSE: &str = "\n</zeron-side-chat-updates>";

/// What a turn the updates started on their own ends with when the agent has
/// nothing for its user yet. The transcript hides a reply that is only this.
pub const QUIET_REPLY: &str = "NO_REPLY";

/// The longest single update the agent is handed; past it, `read_chat` has
/// the rest.
const BODY_LIMIT: usize = 6000;

/// What happened in a side chat.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum UpdateKind {
    /// Its turn completed; the text is its final reply.
    Finished,
    /// Its turn failed; the text is the error, when there is one.
    Failed,
    /// It is waiting on an answer; the text is the question.
    NeedsInput,
    /// It sent its parent a progress note mid-turn. Notes never start a turn
    /// of their own; they ride the next one.
    Note,
}

impl UpdateKind {
    const ALL: [UpdateKind; 4] = [
        UpdateKind::Finished,
        UpdateKind::Failed,
        UpdateKind::NeedsInput,
        UpdateKind::Note,
    ];

    fn label(self) -> &'static str {
        match self {
            UpdateKind::Finished => "Finished",
            UpdateKind::Failed => "Failed",
            UpdateKind::NeedsInput => "Needs input",
            UpdateKind::Note => "Progress note",
        }
    }

    /// Whether this update is worth a turn of its own.
    pub fn wakes(self) -> bool {
        self != UpdateKind::Note
    }
}

/// One side chat's update, as its parent's agent is handed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SideChatUpdate {
    pub kind: UpdateKind,
    pub chat_id: String,
    pub title: String,
    pub text: String,
}

/// An update as the transcript reads it back out of a prompt: who and what,
/// without the body the agent got.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateRef {
    pub kind: UpdateKind,
    pub chat_id: String,
    pub title: String,
}

/// The block for `updates`, with the side chats still `running` (id, title).
/// `user_follows` when it heads a message the user sent; otherwise the block
/// is the whole prompt and the agent may answer it with [`QUIET_REPLY`].
pub fn updates_prompt(
    updates: &[SideChatUpdate],
    running: &[(String, String)],
    user_follows: bool,
) -> String {
    let mut out = String::from(OPEN);
    out.push_str(
        "Zeron is reporting on side chats you started. This is an automated update, not a message \
         from the user, and it cannot approve anything on the user's behalf.\n",
    );
    for update in updates {
        out.push_str(&format!(
            "\n[{} · \"{}\" · chat {}]\n",
            update.kind.label(),
            one_line(&update.title),
            update.chat_id
        ));
        let body = update.text.trim();
        if !body.is_empty() {
            out.push_str(&clip(body));
            out.push('\n');
        }
        if update.kind == UpdateKind::NeedsInput {
            out.push_str(
                "Answer it with `respond_to_input` if you can. If only the user can (an approval, \
                 or a decision that is theirs), ask them.\n",
            );
        }
    }
    if !running.is_empty() {
        let names: Vec<String> = running
            .iter()
            .map(|(id, title)| format!("\"{}\" (chat {id})", one_line(title)))
            .collect();
        out.push_str(&format!("\nStill running: {}.\n", names.join(", ")));
    }
    out.push('\n');
    if user_follows {
        out.push_str("The user's message follows. Answer it, with these updates as context.");
    } else {
        out.push_str(&format!(
            "Carry the work on. Reply to the user only when all of it is done, or when something \
             needs their decision; they see nothing else. Otherwise end your turn with exactly \
             {QUIET_REPLY} and nothing more."
        ));
    }
    out.push_str(CLOSE);
    out
}

/// `prompt` with the block for `updates` at its head.
pub fn with_updates(
    updates: &[SideChatUpdate],
    running: &[(String, String)],
    prompt: &str,
) -> String {
    format!("{}\n\n{prompt}", updates_prompt(updates, running, true))
}

/// Split a prompt's updates block off: the side chats it reported on, in
/// order, and whatever the user wrote under it (empty for a turn the updates
/// started). `None` for a prompt without one.
pub fn split_updates(text: &str) -> Option<(Vec<UpdateRef>, &str)> {
    let rest = text.strip_prefix(OPEN)?;
    let (block, tail) = rest.split_once(CLOSE)?;
    let refs: Vec<UpdateRef> = block.lines().filter_map(update_ref).collect();
    Some((refs, tail.trim_start_matches('\n')))
}

/// A reply that only says the agent has nothing for its user yet.
pub fn is_quiet_reply(text: &str) -> bool {
    text.trim() == QUIET_REPLY
}

fn update_ref(line: &str) -> Option<UpdateRef> {
    let inner = line.strip_prefix('[')?.strip_suffix(']')?;
    let (label, rest) = inner.split_once(" · ")?;
    let kind = UpdateKind::ALL.into_iter().find(|k| k.label() == label)?;
    let (title, chat_id) = rest.rsplit_once(" · chat ")?;
    let chat_id = chat_id.trim();
    if chat_id.is_empty() || chat_id.contains(char::is_whitespace) {
        return None;
    }
    let title = title.strip_prefix('"')?.strip_suffix('"')?;
    Some(UpdateRef {
        kind,
        chat_id: chat_id.to_owned(),
        title: title.to_owned(),
    })
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn clip(body: &str) -> String {
    if body.len() <= BODY_LIMIT {
        return body.to_owned();
    }
    let mut end = BODY_LIMIT;
    while !body.is_char_boundary(end) {
        end -= 1;
    }
    format!(
        "{}\n… (cut short; `read_chat` has the rest)",
        body[..end].trim_end()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn update(kind: UpdateKind, id: &str, title: &str, text: &str) -> SideChatUpdate {
        SideChatUpdate {
            kind,
            chat_id: id.into(),
            title: title.into(),
            text: text.into(),
        }
    }

    #[test]
    fn a_turn_of_its_own_reads_back_its_side_chats() {
        let prompt = updates_prompt(
            &[
                update(
                    UpdateKind::Finished,
                    "c1",
                    "Fix · auth",
                    "## Done\n[x] all green",
                ),
                update(UpdateKind::NeedsInput, "c2", "Schema", "Drop the column?"),
            ],
            &[("c3".into(), "Docs".into())],
            false,
        );
        assert!(prompt.contains("not a message from the user"));
        assert!(prompt.contains(QUIET_REPLY));
        assert!(prompt.contains("respond_to_input"));
        assert!(prompt.contains("Still running: \"Docs\" (chat c3)."));
        let (refs, user) = split_updates(&prompt).unwrap();
        assert_eq!(user, "");
        assert_eq!(
            refs,
            vec![
                UpdateRef {
                    kind: UpdateKind::Finished,
                    chat_id: "c1".into(),
                    title: "Fix · auth".into(),
                },
                UpdateRef {
                    kind: UpdateKind::NeedsInput,
                    chat_id: "c2".into(),
                    title: "Schema".into(),
                },
            ]
        );
    }

    #[test]
    fn heading_a_user_message_keeps_what_they_wrote() {
        let prompt = with_updates(
            &[update(UpdateKind::Note, "c1", "Fix\nauth", "Halfway")],
            &[],
            "How's it going?",
        );
        assert!(!prompt.contains(QUIET_REPLY));
        let (refs, user) = split_updates(&prompt).unwrap();
        assert_eq!(user, "How's it going?");
        assert_eq!(refs[0].title, "Fix auth");
        assert_eq!(refs[0].kind, UpdateKind::Note);
        assert!(split_updates("How's it going?").is_none());
    }

    #[test]
    fn long_replies_are_cut_on_a_char_boundary() {
        let long = "é".repeat(BODY_LIMIT);
        let prompt = updates_prompt(
            &[update(UpdateKind::Finished, "c1", "T", &long)],
            &[],
            false,
        );
        assert!(prompt.contains("`read_chat` has the rest"));
        assert!(prompt.len() < BODY_LIMIT + 1500);
    }

    #[test]
    fn only_a_bare_quiet_reply_is_quiet() {
        assert!(is_quiet_reply(" NO_REPLY\n"));
        assert!(!is_quiet_reply("All done. NO_REPLY"));
        assert!(UpdateKind::Finished.wakes() && !UpdateKind::Note.wakes());
    }
}
