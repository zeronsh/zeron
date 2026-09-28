//! Agent-orchestration contracts shared by the engine, the `zeron chat` CLI,
//! and the UI: child-outcome detection, agent-to-agent message attribution,
//! `@chat:` mention scanning, and the ack ledger's wire payloads.

use std::ops::Range;

use serde::{Deserialize, Serialize};

use crate::entities::{Session, SessionStatus};

/// The wire prefix a `@chat:` mention carries. A mention is only ever a full
/// UUID behind it — see [`chat_mentions`].
pub const CHAT_MENTION_PREFIX: &str = "@chat:";

/// How a child chat's turn settled, from the parent's point of view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ChildOutcome {
    Completed,
    Errored,
    Interrupted,
    NeedsInput,
}

/// One detected child state change. `key` is stable across observers and
/// restarts — the notification ledger dedupes on it, so the same transition
/// never notifies twice.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChildUpdate {
    pub outcome: ChildOutcome,
    pub key: String,
}

fn millis(dt: &chrono::DateTime<chrono::Utc>) -> i64 {
    dt.timestamp_millis()
}

/// Compare a child's registry session row before and after a change and name
/// the transition the parent should hear about. Pure: watchers on every
/// device run the same function and agree on the same key.
///
/// `last_completed_turn == None` means "no information", never a completion:
/// the registry row just may not have been written yet.
pub fn child_update(prev: Option<&Session>, next: &Session) -> Option<ChildUpdate> {
    let update = |outcome, key: String| ChildUpdate { outcome, key };
    // Errored / AwaitingInput settle regardless of what preceded them; the
    // started-at stamp (row write time as fallback) pins the key to this turn.
    if next.status == SessionStatus::Errored {
        let at = next
            .started_at
            .as_ref()
            .map_or_else(|| millis(&next.updated_at), millis);
        return Some(update(ChildOutcome::Errored, format!("error:{at}")));
    }
    if next.status == SessionStatus::AwaitingInput {
        let at = next
            .started_at
            .as_ref()
            .map_or_else(|| millis(&next.updated_at), millis);
        return Some(update(ChildOutcome::NeedsInput, format!("input:{at}")));
    }
    // Working -> Idle without a new completed-turn marker = the run was
    // stopped. The marker simply being ABSENT qualifies too: a host that
    // restarts mid-turn rewrites the row Idle with the marker wiped (boot
    // recovery has no turn id to keep), and that death must still report.
    // Checked BEFORE completion: a Working predecessor satisfies the
    // "prev was not Idle" clause there too, and the interrupted reading wins.
    if next.status == SessionStatus::Idle
        && let Some(p) = prev
        && p.status == SessionStatus::Working
        && (next.last_completed_turn.is_none() || next.last_completed_turn == p.last_completed_turn)
    {
        let at = p
            .started_at
            .as_ref()
            .map_or_else(|| millis(&next.updated_at), millis);
        return Some(update(ChildOutcome::Interrupted, format!("stopped:{at}")));
    }
    // Completion: the row landed Idle WITH a fresh completed-turn marker —
    // fresh relative to what we last saw, so replays of the same settle do
    // not re-fire.
    if next.status == SessionStatus::Idle
        && let Some(turn) = &next.last_completed_turn
        && prev.is_none_or(|p| {
            p.last_completed_turn.as_ref() != Some(turn) || p.status != SessionStatus::Idle
        })
    {
        return Some(update(ChildOutcome::Completed, format!("done:{turn}")));
    }
    None
}

/// The attribution header every agent-to-agent message carries, so both the
/// receiving agent and the human reading the transcript can tell it from a
/// typed message. `sender_title` falls back to the id's 8-char prefix.
pub fn agent_message(sender_title: Option<&str>, sender_chat_id: &str, text: &str) -> String {
    let label = sender_title
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| sender_chat_id.chars().take(8).collect());
    let id8: String = sender_chat_id.chars().take(8).collect();
    format!(
        "[Message from Zeron chat {label} ({CHAT_MENTION_PREFIX}{sender_chat_id}). Reply with `zeron chat tell {id8} <message>`.]\n\n{text}"
    )
}

/// An agent-to-agent message decoded from its attribution header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentMessage<'a> {
    /// Display label — the sender's title, or its id prefix when untitled.
    pub sender_label: &'a str,
    /// The sender's full chat id when the header carried one (`@chat:` form);
    /// `None` for the legacy MCP header, which only ever carried the prefix.
    pub sender_chat_id: Option<&'a str>,
    /// Everything after the header's blank line.
    pub body: &'a str,
}

/// Parse the attribution headers [`agent_message`] writes, plus the legacy
/// MCP form `[Message from Zeron chat <label> (<id8>). Reply to it with the
/// Zeron \`send_message\` tool, chat <id8>.]` so transcripts written before
/// the CLI existed still render as attributions.
pub fn parse_agent_message(text: &str) -> Option<AgentMessage<'_>> {
    let rest = text.strip_prefix("[Message from Zeron chat ")?;
    let (header, body) = rest.split_once("]\n\n")?;
    parse_new(header, body).or_else(|| parse_legacy(header, body))
}

fn parse_new<'a>(header: &'a str, body: &'a str) -> Option<AgentMessage<'a>> {
    // "<label> (@chat:<uuid>). Reply with `zeron chat tell <id8> <message>`."
    let (label, rest) = header.rsplit_once(" (")?;
    let rest = rest.strip_prefix(CHAT_MENTION_PREFIX)?;
    let (id, tail) = rest.split_once("). Reply with `zeron chat tell ")?;
    let id8 = tail.strip_suffix(" <message>`.")?;
    if !is_uuid(id) || id8 != &id[..8] {
        return None;
    }
    Some(AgentMessage {
        sender_label: label,
        sender_chat_id: Some(id),
        body,
    })
}

fn parse_legacy<'a>(header: &'a str, body: &'a str) -> Option<AgentMessage<'a>> {
    // "<label> (<id8>). Reply to it with the Zeron `send_message` tool, chat <id8>."
    // The legacy header carried only the id prefix, so no full id comes back.
    let (label, rest) =
        header.rsplit_once(". Reply to it with the Zeron `send_message` tool, chat ")?;
    let id8 = rest.strip_suffix('.')?;
    // The label itself trails " (<id8>)" — strip it for display.
    let name = match label.rsplit_once(" (") {
        Some((name, suffix)) if suffix == format!("{id8})") => name,
        _ => label,
    };
    Some(AgentMessage {
        sender_label: name,
        sender_chat_id: None,
        body,
    })
}

fn is_uuid(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 36 {
        return false;
    }
    for (i, &c) in b.iter().enumerate() {
        match i {
            8 | 13 | 18 | 23 if c != b'-' => return false,
            8 | 13 | 18 | 23 => {}
            _ if !c.is_ascii_hexdigit() => return false,
            _ => {}
        }
    }
    true
}

/// One `@chat:<uuid>` occurrence: the byte range covers the prefix and the
/// id, so callers can swap the whole span for a pill.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatMention<'a> {
    pub range: Range<usize>,
    pub chat_id: &'a str,
}

/// Find every `@chat:<uuid>` mention in `text`. Boundary rules: the prefix
/// must be preceded by start or a non-alphanumeric char (an email's
/// `foo@chat:` does not match), and the id must be followed by end or a char
/// that is not alphanumeric, `-` or `_` (a partial streaming id, or a longer
/// slug like `<id>-x`, never matches).
pub fn chat_mentions(text: &str) -> Vec<ChatMention<'_>> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(found) = text[at..].find(CHAT_MENTION_PREFIX) {
        let start = at + found;
        let id_start = start + CHAT_MENTION_PREFIX.len();
        if start > 0 && bytes[start - 1].is_ascii_alphanumeric() {
            at = id_start;
            continue;
        }
        let id_end = id_start + 36;
        if id_end <= text.len()
            && text.is_char_boundary(id_end)
            && is_uuid(&text[id_start..id_end])
            && (id_end == text.len()
                || !matches!(bytes[id_end], b'-' | b'_') && !bytes[id_end].is_ascii_alphanumeric())
        {
            out.push(ChatMention {
                range: start..id_end,
                chat_id: &text[id_start..id_end],
            });
        }
        at = id_start;
    }
    out
}

/// Every `@chat:` mention to upgrade to a pill in rendered prose: the strict
/// full-uuid form AND the short form agents type — an id prefix immediately
/// followed by an ellipsis (`...` or `…`). The short form's `chat_id` is the
/// prefix WITHOUT the ellipsis; `range` covers the whole `@chat:…` token
/// including the ellipsis so a pill replaces it entirely. Resolution (unique
/// id-prefix) happens in the UI. Never used for wire or CLI-output parsing.
pub fn chat_display_mentions(text: &str) -> Vec<ChatMention<'_>> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut at = 0;
    while let Some(found) = text[at..].find(CHAT_MENTION_PREFIX) {
        let start = at + found;
        let id_start = start + CHAT_MENTION_PREFIX.len();
        if start > 0 && bytes[start - 1].is_ascii_alphanumeric() {
            at = id_start;
            continue;
        }
        // A strict full uuid wins at this occurrence — same shape and
        // boundary rules as [`chat_mentions`].
        let id_end = id_start + 36;
        if id_end <= text.len()
            && text.is_char_boundary(id_end)
            && is_uuid(&text[id_start..id_end])
            && (id_end == text.len()
                || !matches!(bytes[id_end], b'-' | b'_') && !bytes[id_end].is_ascii_alphanumeric())
        {
            out.push(ChatMention {
                range: start..id_end,
                chat_id: &text[id_start..id_end],
            });
            at = id_end;
            continue;
        }
        // Otherwise the short form qualifies only when the hex/dash run is
        // at least 8 chars and is immediately followed by an ellipsis — a
        // still-streaming uuid continues with `-` and more hex, not dots,
        // so it never matches early. The mention's own boundary rule moves
        // behind the ellipsis: end of text or a char that is not `-`, `_`
        // or alphanumeric.
        let run_end = text[id_start..]
            .find(|c: char| !c.is_ascii_hexdigit() && c != '-')
            .map_or(text.len(), |rel| id_start + rel);
        if run_end - id_start >= 8 {
            let ellipsis = ["...", "…"]
                .iter()
                .find_map(|dots| text[run_end..].strip_prefix(*dots).map(|_| dots.len()));
            if let Some(elen) = ellipsis {
                let end = run_end + elen;
                if end == text.len()
                    || !matches!(bytes[end], b'-' | b'_') && !bytes[end].is_ascii_alphanumeric()
                {
                    out.push(ChatMention {
                        range: start..end,
                        chat_id: &text[id_start..run_end],
                    });
                    at = end;
                    continue;
                }
            }
        }
        at = id_start;
    }
    out
}

/// One `(childChatId, turnKey)` the parent has consumed — acked keys are
/// never delivered again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChildUpdateRef {
    pub child_chat_id: String,
    pub turn_key: String,
}

/// `AckChildUpdates` payload: `zeron chat wait`/`output` run inside the
/// parent (`ZERON_CHAT_ID`) report which child updates they already saw.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AckChildUpdatesParams {
    pub parent_chat_id: String,
    pub updates: Vec<ChildUpdateRef>,
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::*;

    fn session(
        status: SessionStatus,
        turn: Option<&str>,
        started_ms: Option<i64>,
        updated_ms: i64,
    ) -> Session {
        Session {
            last_completed_turn: turn.map(str::to_owned),
            chat_id: "child".into(),
            device_id: "dev".into(),
            status,
            started_at: started_ms.map(|ms| Utc.timestamp_millis_opt(ms).unwrap()),
            updated_at: Utc.timestamp_millis_opt(updated_ms).unwrap(),
        }
    }

    #[test]
    fn child_update_covers_the_transition_table() {
        let working = || session(SessionStatus::Working, Some("t1"), Some(1_000), 2_000);
        // Idle with a fresh marker, seen from every predecessor state.
        let idle = session(SessionStatus::Idle, Some("t2"), Some(1_000), 3_000);
        assert_eq!(
            child_update(None, &idle),
            Some(ChildUpdate {
                outcome: ChildOutcome::Completed,
                key: "done:t2".into()
            })
        );
        assert_eq!(
            child_update(Some(&working()), &idle),
            Some(ChildUpdate {
                outcome: ChildOutcome::Completed,
                key: "done:t2".into()
            })
        );
        assert_eq!(
            child_update(
                Some(&session(SessionStatus::Errored, Some("t1"), None, 2_500)),
                &idle
            ),
            Some(ChildUpdate {
                outcome: ChildOutcome::Completed,
                key: "done:t2".into()
            })
        );
        // Idle->Idle repeating the same marker is a replay, not a completion.
        assert_eq!(child_update(Some(&idle), &idle), None);
        // Idle->Idle with an ADVANCED marker is a fresh completion.
        let idle3 = session(SessionStatus::Idle, Some("t3"), None, 4_000);
        assert_eq!(
            child_update(Some(&idle), &idle3),
            Some(ChildUpdate {
                outcome: ChildOutcome::Completed,
                key: "done:t3".into()
            })
        );
        // A marker of None is "no information", never a completion — but a
        // Working->Idle settle whose marker VANISHED is an interruption:
        // that's the row a child host's restart writes when it recovers.
        let idle_none = session(SessionStatus::Idle, None, None, 4_000);
        assert_eq!(child_update(None, &idle_none), None);
        assert_eq!(
            child_update(Some(&working()), &idle_none),
            Some(ChildUpdate {
                outcome: ChildOutcome::Interrupted,
                key: "stopped:1000".into()
            })
        );
        // Working->Idle with an UNCHANGED marker = interrupted, not completed.
        assert_eq!(
            child_update(
                Some(&working()),
                &session(SessionStatus::Idle, Some("t1"), Some(1_000), 3_000)
            ),
            Some(ChildUpdate {
                outcome: ChildOutcome::Interrupted,
                key: "stopped:1000".into()
            })
        );
        // Working->Working and Idle->Working settle nothing.
        assert_eq!(child_update(Some(&working()), &working()), None);
        assert_eq!(child_update(Some(&idle), &working()), None);
        // Errored keys off started_at, falling back to updated_at.
        let errored = session(SessionStatus::Errored, Some("t1"), Some(1_500), 3_000);
        assert_eq!(
            child_update(Some(&working()), &errored),
            Some(ChildUpdate {
                outcome: ChildOutcome::Errored,
                key: "error:1500".into()
            })
        );
        let errored_no_start = session(SessionStatus::Errored, Some("t1"), None, 3_000);
        assert_eq!(
            child_update(None, &errored_no_start),
            Some(ChildUpdate {
                outcome: ChildOutcome::Errored,
                key: "error:3000".into()
            })
        );
        // AwaitingInput likewise, from any predecessor.
        let waiting = session(SessionStatus::AwaitingInput, Some("t1"), Some(1_500), 3_000);
        assert_eq!(
            child_update(Some(&working()), &waiting),
            Some(ChildUpdate {
                outcome: ChildOutcome::NeedsInput,
                key: "input:1500".into()
            })
        );
        let waiting_no_start = session(SessionStatus::AwaitingInput, None, None, 7_000);
        assert_eq!(
            child_update(None, &waiting_no_start),
            Some(ChildUpdate {
                outcome: ChildOutcome::NeedsInput,
                key: "input:7000".into()
            })
        );
    }

    const ID: &str = "3f6b2a18-9c4d-4e5f-8a7b-1c2d3e4f5a6b";

    #[test]
    fn agent_message_round_trips() {
        let wire = agent_message(Some("  Reviewer "), ID, "look at this");
        assert_eq!(
            wire,
            format!(
                "[Message from Zeron chat Reviewer (@chat:{ID}). Reply with `zeron chat tell 3f6b2a18 <message>`.]\n\nlook at this"
            )
        );
        let parsed = parse_agent_message(&wire).unwrap();
        assert_eq!(parsed.sender_label, "Reviewer");
        assert_eq!(parsed.sender_chat_id, Some(ID));
        assert_eq!(parsed.body, "look at this");
        // Untitled senders label with the id prefix.
        let wire = agent_message(None, ID, "hi");
        let parsed = parse_agent_message(&wire).unwrap();
        assert_eq!(parsed.sender_label, "3f6b2a18");
        assert_eq!(parsed.sender_chat_id, Some(ID));
        assert_eq!(parsed.body, "hi");
    }

    #[test]
    fn legacy_mcp_header_parses_without_a_chat_id() {
        let legacy = "[Message from Zeron chat Main (3f6b2a18). Reply to it with the Zeron `send_message` tool, chat 3f6b2a18.]\n\nbody here";
        let parsed = parse_agent_message(legacy).unwrap();
        assert_eq!(parsed.sender_label, "Main");
        assert_eq!(parsed.sender_chat_id, None);
        assert_eq!(parsed.body, "body here");
        // An untitled legacy sender has no parenthesized suffix to strip.
        let bare = "[Message from Zeron chat 3f6b2a18. Reply to it with the Zeron `send_message` tool, chat 3f6b2a18.]\n\nb";
        assert_eq!(parse_agent_message(bare).unwrap().sender_label, "3f6b2a18");
    }

    #[test]
    fn parse_agent_message_ignores_ordinary_text() {
        assert_eq!(parse_agent_message("hello world"), None);
        assert_eq!(
            parse_agent_message("[Message from Zeron chat x]\n\nno tail"),
            None
        );
        assert_eq!(
            parse_agent_message(
                "mid [Message from Zeron chat a (@chat:3f6b2a18-9c4d-4e5f-8a7b-1c2d3e4f5a6b). Reply with `zeron chat tell 3f6b2a18 <message>`.]\n\nb"
            ),
            None
        );
    }

    #[test]
    fn chat_mentions_finds_only_full_uuids_at_boundaries() {
        let id = "3f6b2a18-9c4d-4e5f-8a7b-1c2d3e4f5a6b";
        let up = "3F6B2A18-9C4D-4E5F-8A7B-1C2D3E4F5A6B";
        // Start and end of string, upper- or lowercase.
        let only = format!("@chat:{id}");
        let m = chat_mentions(&only);
        assert_eq!(
            m,
            [ChatMention {
                range: 0..42,
                chat_id: id
            }]
        );
        let with_prefix = format!("see @chat:{up}");
        let m = chat_mentions(&with_prefix);
        assert_eq!(m[0].chat_id, up);
        assert_eq!(&with_prefix[m[0].range.clone()], format!("@chat:{up}"));
        // Inside punctuation.
        let parens = format!("(@chat:{id}).");
        assert_eq!(chat_mentions(&parens).len(), 1);
        // A partial (still streaming) id never matches.
        assert!(chat_mentions("@chat:3f6b2a18-9c4d-4e5f").is_empty());
        // Trailing `-x` makes it not-a-uuid; `_` and alnum tails too.
        assert!(chat_mentions(&format!("@chat:{id}-x")).is_empty());
        assert!(chat_mentions(&format!("@chat:{id}_x")).is_empty());
        assert!(chat_mentions(&format!("@chat:{id}z")).is_empty());
        // Email-like: preceded by an alphanumeric char.
        assert!(chat_mentions(&format!("foo@chat:{id}")).is_empty());
        // Two mentions in one text.
        let both = format!("@chat:{id} and @chat:{up}");
        let m = chat_mentions(&both);
        assert_eq!(m.len(), 2);
        assert_eq!(m[1].chat_id, up);
    }

    #[test]
    fn display_mentions_match_full_uuids_and_ellipsis_short_forms() {
        let id = "3f6b2a18-9c4d-4e5f-8a7b-1c2d3e4f5a6b";
        // The full form is unchanged.
        let only = format!("@chat:{id}");
        let m = chat_display_mentions(&only);
        assert_eq!(
            m,
            [ChatMention {
                range: 0..42,
                chat_id: id
            }]
        );
        // The short form: an 8+ char prefix glued to `...`. The range covers
        // the ellipsis so the pill replaces it; the id does not include it.
        let short = "Launched @chat:ba6888c8... to review";
        let m = chat_display_mentions(short);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].chat_id, "ba6888c8");
        assert_eq!(&short[m[0].range.clone()], "@chat:ba6888c8...");
        // The single-char ellipsis matches too.
        let uni = "Launched @chat:ba6888c8… to review";
        let m = chat_display_mentions(uni);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].chat_id, "ba6888c8");
        assert_eq!(&uni[m[0].range.clone()], "@chat:ba6888c8…");
        // Two short mentions on one line.
        let both = "see @chat:ba6888c8... and @chat:deadbeef…!";
        let m = chat_display_mentions(both);
        assert_eq!(m.len(), 2);
        assert_eq!(m[0].chat_id, "ba6888c8");
        assert_eq!(m[1].chat_id, "deadbeef");
        // A mixed line: full uuid then short.
        let mixed = format!("@chat:{id} vs @chat:ba6888c8...");
        let m = chat_display_mentions(&mixed);
        assert_eq!(m.len(), 2);
        assert_eq!(m[0].chat_id, id);
        assert_eq!(m[1].chat_id, "ba6888c8");
    }

    #[test]
    fn display_mentions_reject_incomplete_or_unbounded_forms() {
        let id = "3f6b2a18-9c4d-4e5f-8a7b-1c2d3e4f5a6b";
        // A bare prefix without the ellipsis is still-streaming text, not a
        // mention.
        assert!(chat_display_mentions("@chat:ba6888c8").is_empty());
        // A uuid mid-stream: longer than 8 chars, but no ellipsis either.
        assert!(chat_display_mentions("@chat:ba6888c8-6e5b-4c9f-8c35").is_empty());
        // Fewer than 8 chars before the ellipsis never qualifies.
        assert!(chat_display_mentions("@chat:ba6888c...").is_empty());
        // Glued to a preceding alphanumeric, like an email local-part.
        assert!(chat_display_mentions("foo@chat:ba6888c8...").is_empty());
        // An alphanumeric glued to the ellipsis breaks the boundary.
        assert!(chat_display_mentions("@chat:ba6888c8...x").is_empty());
        assert!(chat_display_mentions("@chat:ba6888c8…-y").is_empty());
        // Same trailing-token rules for the full form.
        assert!(chat_display_mentions(&format!("@chat:{id}z")).is_empty());
        assert!(chat_display_mentions(&format!("foo@chat:{id}")).is_empty());
    }

    #[test]
    fn ack_params_serialize_camel_case() {
        let params = AckChildUpdatesParams {
            parent_chat_id: "p".into(),
            updates: vec![ChildUpdateRef {
                child_chat_id: "c".into(),
                turn_key: "done:t1".into(),
            }],
        };
        let json = serde_json::to_value(&params).unwrap();
        assert_eq!(json["parentChatId"], "p");
        assert_eq!(json["updates"][0]["childChatId"], "c");
        assert_eq!(json["updates"][0]["turnKey"], "done:t1");
    }
}
