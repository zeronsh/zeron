//! Zeron MCP `create_chat` / `create_chats` calls in the spawner's
//! transcript: which tool calls they are, and which chats they created.
//!
//! Every harness names MCP tools its own way — Claude `mcp__zeron__create_chat`
//! (decoded to `ToolCall::Mcp { server: "zeron", .. }`), Pi `zeron_create_chat`
//! (also decoded to `Mcp`), Codex/Cursor `Mcp { server, tool }`, OpenCode
//! `zeron_create_chat` and ACP titles such as `zeron/create_chat` or
//! `create_chat (zeron MCP Server)` (both left as `ToolCall::Unknown`) — so
//! detection keys on the tool token plus a mention of the Zeron server,
//! never on one spelling.
//!
//! The created chat ids come from the tool part itself: the doc fold keeps
//! a create call's `created_chat_ids` (read from its result, whose text
//! never enters the doc). A result still on the part (pre-strip docs, local
//! journals) is parsed as well. Transcripts written before the fold kept ids
//! fall back to the chats' own provenance (`spawnedByChatId`), matched back
//! to the calls by time: [`attribute_spawned_chats`].

use std::collections::{HashMap, HashSet};

use zeron_doc::{MessagePart, SessionMessageEntry};
use zeron_proto::ToolCall;

pub(crate) use zeron_proto::created_chats::{
    CreateChatOp, CreatedChat, create_chat_op, parse_created_chats,
};

/// The chats a create call's part names: the fold's `created_chat_ids`,
/// else whatever an inline result says (richer: kind, host, title).
pub(crate) fn named_chats(created_chat_ids: &[String], output: Option<&str>) -> Vec<CreatedChat> {
    let parsed = output.map(parse_created_chats).unwrap_or_default();
    if created_chat_ids.is_empty() {
        return parsed;
    }
    created_chat_ids
        .iter()
        .map(|id| {
            parsed
                .iter()
                .find(|chat| &chat.chat_id == id)
                .cloned()
                .unwrap_or_else(|| CreatedChat {
                    chat_id: id.clone(),
                    ..Default::default()
                })
        })
        .collect()
}

/// A settled, successful create call in a transcript.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CreateCallSite {
    pub part_id: String,
    pub entry_id: String,
    /// The carrying message's start (epoch ms).
    pub at_ms: i64,
    pub op: CreateChatOp,
}

/// The create calls in `entries` whose chats must be recovered from
/// provenance, and the chat ids other calls' results already name (those
/// are spoken for). Running and failed calls create nothing to link.
pub(crate) fn create_call_sites(
    entries: &[SessionMessageEntry],
) -> (Vec<CreateCallSite>, HashSet<String>) {
    let mut sites = Vec::new();
    let mut claimed = HashSet::new();
    for entry in entries {
        for part in &entry.parts {
            let MessagePart::Tool {
                id,
                call,
                is_error,
                resolved,
                output,
                created_chat_ids,
                ..
            } = part
            else {
                continue;
            };
            let Some(op) = create_chat_op(call) else {
                continue;
            };
            if !*resolved || *is_error {
                continue;
            }
            let named = named_chats(created_chat_ids, output.as_deref());
            if named.is_empty() {
                sites.push(CreateCallSite {
                    part_id: id.clone(),
                    entry_id: entry.id.clone(),
                    at_ms: entry.created_at,
                    op,
                });
            } else {
                claimed.extend(named.into_iter().map(|chat| chat.chat_id));
            }
        }
    }
    (sites, claimed)
}

/// Match the chats a spawner's agent created (`spawned`: `(chat id,
/// created_at ms)` of every chat whose `spawnedByChatId` is the spawner) to
/// the create calls in its transcript, keyed by the call's part id.
///
/// A chat belongs to the last message that started at or before it was
/// created. Within one message, `create_chat` calls take one chat each in
/// order; a single `create_chats` batch takes whatever the `create_chat`s
/// around it leave. Anything ambiguous (two batches in one message) stays
/// unlinked rather than guessed — the card then falls back to the plain
/// chip. Deleted chats simply leave their call unlinked.
pub(crate) fn attribute_spawned_chats(
    sites: &[CreateCallSite],
    spawned: &[(String, i64)],
) -> HashMap<String, Vec<String>> {
    // Consecutive calls of one message form a group.
    let mut groups: Vec<(i64, Vec<&CreateCallSite>)> = Vec::new();
    let mut last_entry: Option<&str> = None;
    for site in sites {
        if last_entry == Some(site.entry_id.as_str())
            && let Some((_, calls)) = groups.last_mut()
        {
            calls.push(site);
        } else {
            groups.push((site.at_ms, vec![site]));
        }
        last_entry = Some(site.entry_id.as_str());
    }
    let mut spawned: Vec<&(String, i64)> = spawned.iter().collect();
    spawned.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
    let mut buckets: Vec<Vec<&str>> = vec![Vec::new(); groups.len()];
    for (chat_id, created_ms) in spawned {
        if let Some(group) = groups.iter().rposition(|(at, _)| *at <= *created_ms) {
            buckets[group].push(chat_id);
        }
    }
    let mut out: HashMap<String, Vec<String>> = HashMap::new();
    for ((_, calls), chats) in groups.iter().zip(buckets) {
        let batches: Vec<usize> = calls
            .iter()
            .enumerate()
            .filter(|(_, call)| call.op == CreateChatOp::Batch)
            .map(|(ix, _)| ix)
            .collect();
        let mut chats = chats.into_iter();
        let mut give = |call: &CreateCallSite, ids: Vec<&str>| {
            if !ids.is_empty() {
                out.entry(call.part_id.clone())
                    .or_default()
                    .extend(ids.into_iter().map(str::to_owned));
            }
        };
        match batches.as_slice() {
            [] => {
                for call in calls {
                    give(call, chats.next().into_iter().collect());
                }
            }
            [batch] => {
                let (before, rest) = calls.split_at(*batch);
                let after = &rest[1..];
                for call in before {
                    give(call, chats.next().into_iter().collect());
                }
                let remaining: Vec<&str> = chats.collect();
                let keep = remaining.len().saturating_sub(after.len());
                give(rest[0], remaining[..keep].to_vec());
                for (call, chat) in after.iter().zip(&remaining[keep..]) {
                    give(call, vec![chat]);
                }
            }
            [first, ..] => {
                for call in &calls[..*first] {
                    give(call, chats.next().into_iter().collect());
                }
            }
        }
    }
    out
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

    fn site(part: &str, entry: &str, at: i64, op: CreateChatOp) -> CreateCallSite {
        CreateCallSite {
            part_id: part.into(),
            entry_id: entry.into(),
            at_ms: at,
            op,
        }
    }

    fn spawned(chats: &[(&str, i64)]) -> Vec<(String, i64)> {
        chats.iter().map(|(id, at)| ((*id).into(), *at)).collect()
    }

    fn ids(map: &HashMap<String, Vec<String>>, part: &str) -> Vec<String> {
        map.get(part).cloned().unwrap_or_default()
    }

    #[test]
    fn spawned_chats_attribute_to_the_message_that_created_them() {
        use CreateChatOp::{Batch, Single};
        let sites = [
            site("t1", "e1", 100, Single),
            site("t2", "e2", 200, Batch),
            site("t3", "e3", 300, Single),
        ];
        // `late` predates nothing it could belong to: before any call.
        let map = attribute_spawned_chats(
            &sites,
            &spawned(&[
                ("early", 50),
                ("one", 110),
                ("batch-a", 210),
                ("batch-b", 215),
                ("three", 320),
            ]),
        );
        assert_eq!(ids(&map, "t1"), ["one"]);
        assert_eq!(ids(&map, "t2"), ["batch-a", "batch-b"]);
        assert_eq!(ids(&map, "t3"), ["three"]);
        assert!(!map.values().flatten().any(|id| id == "early"));

        // A deleted chat leaves its own call empty without shifting others.
        let map = attribute_spawned_chats(&sites, &spawned(&[("batch-a", 210), ("three", 320)]));
        assert!(ids(&map, "t1").is_empty());
        assert_eq!(ids(&map, "t2"), ["batch-a"]);
        assert_eq!(ids(&map, "t3"), ["three"]);
    }

    #[test]
    fn calls_sharing_one_message_split_its_chats_in_order() {
        use CreateChatOp::{Batch, Single};
        // create_chat, create_chats, create_chat in one message.
        let sites = [
            site("s1", "e", 100, Single),
            site("b", "e", 100, Batch),
            site("s2", "e", 100, Single),
        ];
        let map = attribute_spawned_chats(
            &sites,
            &spawned(&[("c1", 101), ("c2", 102), ("c3", 103), ("c4", 104)]),
        );
        assert_eq!(ids(&map, "s1"), ["c1"]);
        assert_eq!(ids(&map, "b"), ["c2", "c3"]);
        assert_eq!(ids(&map, "s2"), ["c4"]);

        // Two batches in one message are ambiguous: only the leading
        // create_chat links.
        let sites = [
            site("s1", "e", 100, Single),
            site("b1", "e", 100, Batch),
            site("b2", "e", 100, Batch),
        ];
        let map = attribute_spawned_chats(&sites, &spawned(&[("c1", 101), ("c2", 102)]));
        assert_eq!(ids(&map, "s1"), ["c1"]);
        assert!(ids(&map, "b1").is_empty() && ids(&map, "b2").is_empty());
    }

    #[test]
    fn call_sites_skip_running_failed_and_self_describing_calls() {
        let tool = |id: &str, tool: &str, resolved: bool, is_error: bool, output: Option<&str>| {
            MessagePart::Tool {
                id: id.into(),
                call: mcp("zeron", tool),
                is_error,
                resolved,
                output: output.map(str::to_owned),
                diff: None,
                output_ref: None,
                output_bytes: None,
                diff_ref: None,
                diff_stats: None,
                subagent_ref: None,
                subagent_status: None,
                subagent_tail: None,
                created_chat_ids: Vec::new(),
            }
        };
        let entry = SessionMessageEntry {
            id: "e".into(),
            role: zeron_doc::MessageRole::Assistant,
            parts: vec![
                tool("running", "create_chat", false, false, None),
                tool("failed", "create_chat", true, true, None),
                tool("named", "create_chat", true, false, Some(r#"{"chatId":"known"}"#)),
                tool("bare", "create_chats", true, false, None),
                tool("other", "list_chats", true, false, None),
            ],
            created_at: 42,
            device_id: "dev".into(),
            status: None,
            continuation_of: None,
            duration_ms: None,
        };
        let (sites, claimed) = create_call_sites(std::slice::from_ref(&entry));
        assert_eq!(sites, [site("bare", "e", 42, CreateChatOp::Batch)]);
        assert_eq!(claimed, HashSet::from(["known".to_owned()]));

        // The doc fold's ids name the call's chats exactly: no timing guess.
        let mut kept = tool("kept", "create_chats", true, false, None);
        if let MessagePart::Tool {
            created_chat_ids, ..
        } = &mut kept
        {
            *created_chat_ids = vec!["w1".into(), "w2".into()];
        }
        let entry = SessionMessageEntry {
            parts: vec![kept],
            ..entry
        };
        let (sites, claimed) = create_call_sites(std::slice::from_ref(&entry));
        assert!(sites.is_empty());
        assert_eq!(claimed, HashSet::from(["w1".to_owned(), "w2".to_owned()]));
        let named = named_chats(&["w1".into()], Some(r#"{"chatId":"w1","kind":"side"}"#));
        assert_eq!(named[0].side, Some(true), "inline detail enriches kept ids");
    }
}
