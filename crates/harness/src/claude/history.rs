//! Claude Code's stored session as an editable history.
//!
//! The CLI keeps a conversation in `<config>/projects/<cwd>/<session>.jsonl`,
//! one JSON object per line. Conversation lines (`user`, `assistant`,
//! `system`, `attachment`) carry a `uuid` and a `parentUuid`; the rest
//! (`queue-operation`, `last-prompt`, `api-request`, `cost-state`, …) are
//! bookkeeping. What the model reads on resume is the chain of `parentUuid`
//! links from the newest line back to the root (or to a compaction
//! boundary), so that chain is what an editor lists.
//!
//! An API message is stored one content block per line (same `message.id`,
//! chained by `parentUuid`). An edit never touches the stored session: it
//! writes a copy under a new session id, with text replaced and removed
//! blocks dropped, and the chat resumes the copy.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde_json::Value;
use sha2::{Digest, Sha256};
use zeron_proto::native_history::{
    NativeEdit, NativeHistoryItem, NativeItemKind, NativeRole, ReasoningSeal,
};

use crate::{HarnessError, NativeHistoryRead};

/// Longest summary of a tool call's input or a tool result's output.
const SUMMARY_CHARS: usize = 160;

/// Shown when a stored session moved on since the editor listed it.
pub(super) const CHANGED: &str = "The conversation changed since you started editing";

fn refuse(message: &str) -> HarnessError {
    HarnessError::History(message.into())
}

/// The session file for `session_id` under `config_root`. Session ids are
/// global, so every project directory is searched rather than duplicating
/// the CLI's cwd encoding (which hashes long paths).
pub(super) fn session_file(config_root: &Path, session_id: &str) -> Option<PathBuf> {
    if session_id.is_empty()
        || !session_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        return None;
    }
    std::fs::read_dir(config_root.join("projects"))
        .ok()?
        .flatten()
        .map(|project| project.path().join(format!("{session_id}.jsonl")))
        .find(|path| path.is_file())
}

/// The listing of one stored session.
pub(super) fn read(text: &str) -> NativeHistoryRead {
    let lines = split(text);
    let scan = Scan::new(&lines);
    let listing = Listing::new(&lines, &scan);
    NativeHistoryRead {
        revision: listing.revision,
        items: listing
            .items
            .into_iter()
            .map(|listed| listed.item)
            .collect(),
        compacted: listing.compacted,
    }
}

/// `text` rewritten as session `new_id` with `edits` applied. The result
/// holds every line of the original, so the copy resumes like it.
pub(super) fn rewrite(
    text: &str,
    old_id: &str,
    new_id: &str,
    revision: &str,
    edits: &[NativeEdit],
) -> Result<String, HarnessError> {
    let lines = split(text);
    let scan = Scan::new(&lines);
    let listing = Listing::new(&lines, &scan);
    if listing.revision != revision {
        return Err(refuse(CHANGED));
    }
    let plan = Plan::new(&listing, edits)?;

    // Edited lines: re-serialized, or dropped once no block is left.
    let mut edited: HashMap<usize, Value> = HashMap::new();
    // Dropped line uuid → the uuid its children should hang from instead.
    let mut dropped: HashMap<String, Option<String>> = HashMap::new();
    let mut gone: HashSet<usize> = HashSet::new();
    for (&line, ops) in &plan.by_line {
        let mut value: Value = serde_json::from_str(lines[line])
            .map_err(|_| refuse("The stored conversation could not be read"))?;
        if !apply_ops(&mut value, ops) {
            return Err(refuse("The stored conversation could not be read"));
        }
        if has_content(&value) {
            edited.insert(line, value);
        } else if let Some(meta) = &scan.metas[line] {
            dropped.insert(meta.uuid.clone(), meta.parent.clone());
            gone.insert(line);
        }
    }
    let resolve = |mut uuid: String| -> Option<String> {
        let mut seen = HashSet::new();
        while let Some(parent) = dropped.get(&uuid) {
            if !seen.insert(uuid.clone()) {
                return None;
            }
            uuid = parent.clone()?;
        }
        Some(uuid)
    };

    let old_pattern = format!("\"sessionId\":\"{old_id}\"");
    let new_pattern = format!("\"sessionId\":\"{new_id}\"");
    let mut out = String::with_capacity(text.len());
    for (index, line) in lines.iter().enumerate() {
        if gone.contains(&index) {
            continue;
        }
        let relink = scan.metas[index]
            .as_ref()
            .and_then(|meta| meta.parent.as_ref())
            .is_some_and(|parent| dropped.contains_key(parent));
        let retarget = !dropped.is_empty()
            && line.contains("\"type\":\"last-prompt\"")
            && line.contains("\"leafUuid\"");
        if !(edited.contains_key(&index) || relink || retarget) {
            out.push_str(&line.replace(&old_pattern, &new_pattern));
            out.push('\n');
            continue;
        }
        let Some(mut value) = edited
            .remove(&index)
            .or_else(|| serde_json::from_str(line).ok())
        else {
            out.push_str(line);
            out.push('\n');
            continue;
        };
        if relink {
            let parent = scan.metas[index]
                .as_ref()
                .and_then(|meta| meta.parent.clone())
                .and_then(&resolve);
            value["parentUuid"] = parent.map_or(Value::Null, Value::String);
        }
        if retarget
            && let Some(leaf) = value.get("leafUuid").and_then(Value::as_str)
            && dropped.contains_key(leaf)
        {
            value["leafUuid"] = resolve(leaf.to_owned()).map_or(Value::Null, Value::String);
        }
        if value.get("sessionId").and_then(Value::as_str) == Some(old_id) {
            value["sessionId"] = Value::String(new_id.to_owned());
        }
        out.push_str(&value.to_string());
        out.push('\n');
    }
    Ok(out)
}

/// A fresh session id and the file the copy lands in (beside the original).
pub(super) fn copy_target(original: &Path) -> (String, PathBuf) {
    let id = uuid::Uuid::new_v4().to_string();
    let file = original.with_file_name(format!("{id}.jsonl"));
    (id, file)
}

/// Write `contents` to `target` through a temp file in the same directory,
/// so the CLI never sees a half-written session.
pub(super) fn write_atomic(target: &Path, contents: &str) -> Result<(), HarnessError> {
    let temp = target.with_extension("jsonl.tmp");
    let result = std::fs::write(&temp, contents).and_then(|()| std::fs::rename(&temp, target));
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    Ok(result?)
}

fn split(text: &str) -> Vec<&str> {
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .collect()
}

// ── scan: the active chain ──────────────────────────────────────────────────

struct Meta {
    uuid: String,
    parent: Option<String>,
}

struct Scan {
    /// Per line: its identity when it is a conversation line.
    metas: Vec<Option<Meta>>,
    /// Line indexes of the active branch, oldest first.
    chain: Vec<usize>,
}

impl Scan {
    fn new(lines: &[&str]) -> Self {
        let mut metas: Vec<Option<Meta>> = Vec::with_capacity(lines.len());
        let mut by_uuid: HashMap<String, usize> = HashMap::new();
        let mut leaf = None;
        for (index, line) in lines.iter().enumerate() {
            let meta = line
                .contains("\"uuid\"")
                .then(|| serde_json::from_str::<Value>(line).ok())
                .flatten()
                .and_then(|value| {
                    let uuid = value.get("uuid")?.as_str()?.to_owned();
                    let parent = value
                        .get("parentUuid")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                    let sidechain = value
                        .get("isSidechain")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    Some((Meta { uuid, parent }, sidechain))
                });
            match meta {
                Some((meta, sidechain)) => {
                    by_uuid.insert(meta.uuid.clone(), index);
                    if !sidechain {
                        leaf = Some(index);
                    }
                    metas.push(Some(meta));
                }
                None => metas.push(None),
            }
        }
        let mut chain = Vec::new();
        let mut seen = HashSet::new();
        let mut at = leaf;
        while let Some(index) = at {
            if !seen.insert(index) {
                break;
            }
            chain.push(index);
            at = metas[index]
                .as_ref()
                .and_then(|meta| meta.parent.as_ref())
                .and_then(|parent| by_uuid.get(parent))
                .copied();
        }
        chain.reverse();
        Self { metas, chain }
    }
}

// ── listing ─────────────────────────────────────────────────────────────────

/// What an item is, as far as edits care.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Slot {
    /// Message content that is one plain string.
    Whole,
    /// A content block.
    Block(usize),
}

struct Listed {
    item: NativeHistoryItem,
    line: usize,
    slot: Slot,
}

struct Listing {
    items: Vec<Listed>,
    compacted: bool,
    revision: String,
}

impl Listing {
    fn new(lines: &[&str], scan: &Scan) -> Self {
        let mut items = Vec::new();
        let mut compacted = false;
        let mut hasher = Sha256::new();
        // Prompts seen so far; items answer the latest one.
        let mut prompts: u32 = 0;
        for &line in &scan.chain {
            let Some(meta) = &scan.metas[line] else {
                continue;
            };
            let Ok(value) = serde_json::from_str::<Value>(lines[line]) else {
                continue;
            };
            hasher.update(meta.uuid.as_bytes());
            let kind = value.get("type").and_then(Value::as_str).unwrap_or("");
            if kind == "system"
                && value.get("subtype").and_then(Value::as_str) == Some("compact_boundary")
            {
                compacted = true;
                hasher.update(b"compact");
                continue;
            }
            let role = match kind {
                "user" => NativeRole::User,
                "assistant" => NativeRole::Assistant,
                _ => continue,
            };
            if flag(&value, "isMeta") {
                continue;
            }
            let Some(content) = value.pointer("/message/content") else {
                continue;
            };
            hasher.update(content.to_string().as_bytes());
            let summary_line = flag(&value, "isCompactSummary");
            let mut line_prompted = false;
            let mut push = |slot: Slot, kind: NativeItemKind, editable: bool, removable: bool| {
                let index = match slot {
                    Slot::Whole => 0,
                    Slot::Block(index) => index,
                };
                if matches!(kind, NativeItemKind::Text { .. })
                    && role == NativeRole::User
                    && !line_prompted
                {
                    line_prompted = true;
                    prompts += 1;
                }
                items.push(Listed {
                    item: NativeHistoryItem {
                        id: format!("{}:{index}", meta.uuid),
                        role,
                        turn: prompts.saturating_sub(1),
                        kind,
                        editable,
                        removable,
                    },
                    line,
                    slot,
                });
            };
            let text_kind = |text: String| {
                if summary_line {
                    NativeItemKind::CompactionSummary { text }
                } else {
                    NativeItemKind::Text { text }
                }
            };
            match content {
                Value::String(text) => push(Slot::Whole, text_kind(text.clone()), true, false),
                Value::Array(blocks) => {
                    for (index, block) in blocks.iter().enumerate() {
                        let slot = Slot::Block(index);
                        match block.get("type").and_then(Value::as_str) {
                            Some("text") => {
                                let text = str_field(block, "text");
                                if !text.trim().is_empty() {
                                    push(slot, text_kind(text.to_owned()), true, false);
                                }
                            }
                            Some("thinking") => {
                                let text = str_field(block, "thinking");
                                let readable = !text.trim().is_empty();
                                push(
                                    slot,
                                    NativeItemKind::Reasoning {
                                        text: readable.then(|| text.to_owned()),
                                        seal: if readable {
                                            ReasoningSeal::Signed
                                        } else {
                                            ReasoningSeal::Encrypted
                                        },
                                    },
                                    false,
                                    true,
                                );
                            }
                            Some("redacted_thinking") => push(
                                slot,
                                NativeItemKind::Reasoning {
                                    text: None,
                                    seal: ReasoningSeal::Encrypted,
                                },
                                false,
                                true,
                            ),
                            Some("tool_use") => push(
                                slot,
                                NativeItemKind::ToolCall {
                                    name: str_field(block, "name").to_owned(),
                                    summary: input_summary(block.get("input")),
                                },
                                false,
                                false,
                            ),
                            Some("tool_result") => push(
                                slot,
                                NativeItemKind::ToolResult {
                                    summary: clip(&result_text(block.get("content"))),
                                    is_error: flag(block, "is_error"),
                                },
                                false,
                                false,
                            ),
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
        let digest = hasher.finalize();
        let revision = digest[..16].iter().map(|b| format!("{b:02x}")).collect();
        Self {
            items,
            compacted,
            revision,
        }
    }
}

fn flag(value: &Value, key: &str) -> bool {
    value.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn str_field<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or("")
}

fn clip(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= SUMMARY_CHARS {
        return flat;
    }
    let mut clipped: String = flat.chars().take(SUMMARY_CHARS).collect();
    clipped.push('…');
    clipped
}

/// A tool call's input on one line: the field that names what it does, else
/// the JSON.
fn input_summary(input: Option<&Value>) -> String {
    let Some(input) = input else {
        return String::new();
    };
    for key in [
        "command",
        "file_path",
        "path",
        "pattern",
        "url",
        "query",
        "description",
        "prompt",
    ] {
        if let Some(text) = input.get(key).and_then(Value::as_str) {
            return clip(text);
        }
    }
    clip(&input.to_string())
}

fn result_text(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter_map(|block| block.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

// ── edits ───────────────────────────────────────────────────────────────────

enum Op {
    Replace(String),
    Remove,
}

struct Plan {
    /// Line → (slot, op), in the order the edits named them.
    by_line: BTreeMap<usize, Vec<(Slot, Op)>>,
}

impl Plan {
    fn new(listing: &Listing, edits: &[NativeEdit]) -> Result<Self, HarnessError> {
        let by_id: HashMap<&str, &Listed> = listing
            .items
            .iter()
            .map(|listed| (listed.item.id.as_str(), listed))
            .collect();
        let mut by_line: BTreeMap<usize, Vec<(Slot, Op)>> = BTreeMap::new();
        let mut touched = HashSet::new();
        for edit in edits {
            let (id, op) = match edit {
                NativeEdit::Replace { id, text } => (id, Op::Replace(text.clone())),
                NativeEdit::Remove { id } => (id, Op::Remove),
            };
            let listed = by_id.get(id.as_str()).ok_or_else(|| refuse(CHANGED))?;
            if !touched.insert(id.as_str()) {
                return Err(refuse("Each message can only be edited once"));
            }
            match &op {
                Op::Replace(text) => {
                    if !listed.item.editable {
                        return Err(refuse("This message can't be edited"));
                    }
                    if text.trim().is_empty() {
                        return Err(refuse("Edited text can't be empty"));
                    }
                }
                Op::Remove => {
                    if !listed.item.removable {
                        return Err(refuse("This message can't be removed"));
                    }
                }
            }
            by_line
                .entry(listed.line)
                .or_default()
                .push((listed.slot, op));
        }
        Ok(Self { by_line })
    }
}

/// Apply `ops` to one conversation line. False when the line's shape is not
/// what the listing saw.
fn apply_ops(value: &mut Value, ops: &[(Slot, Op)]) -> bool {
    let Some(content) = value.pointer_mut("/message/content") else {
        return false;
    };
    let mut removed = HashSet::new();
    for (slot, op) in ops {
        match (slot, op, &mut *content) {
            (Slot::Whole, Op::Replace(text), Value::String(current)) => *current = text.clone(),
            (Slot::Block(index), Op::Replace(text), Value::Array(blocks)) => {
                match blocks.get_mut(*index) {
                    Some(block) if block.get("text").is_some_and(Value::is_string) => {
                        block["text"] = Value::String(text.clone());
                    }
                    _ => return false,
                }
            }
            (Slot::Block(index), Op::Remove, Value::Array(blocks)) if *index < blocks.len() => {
                removed.insert(*index);
            }
            _ => return false,
        }
    }
    if let Value::Array(blocks) = content
        && !removed.is_empty()
    {
        let mut index = 0;
        blocks.retain(|_| {
            index += 1;
            !removed.contains(&(index - 1))
        });
    }
    true
}

/// Whether a conversation line still carries something after removals.
fn has_content(value: &Value) -> bool {
    match value.pointer("/message/content") {
        Some(Value::Array(blocks)) => !blocks.is_empty(),
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASIC: &str = include_str!("../../tests/fixtures/claude/native-history-basic.jsonl");
    const COMPACTED: &str =
        include_str!("../../tests/fixtures/claude/native-history-compacted.jsonl");
    const OLD: &str = "11111111-1111-4111-8111-111111111111";
    const NEW: &str = "22222222-2222-4222-8222-222222222222";

    fn text_of(item: &NativeHistoryItem) -> &str {
        match &item.kind {
            NativeItemKind::Text { text } | NativeItemKind::CompactionSummary { text } => text,
            other => panic!("not text: {other:?}"),
        }
    }

    fn ids(read: &NativeHistoryRead) -> Vec<&str> {
        read.items.iter().map(|item| item.id.as_str()).collect()
    }

    #[test]
    fn lists_the_active_chain_in_turns() {
        let read = read(BASIC);
        assert!(!read.compacted);
        // The sidechain line, the `isMeta` line, the abandoned branch and
        // the bookkeeping lines are not part of what the model reads.
        assert_eq!(
            ids(&read),
            [
                "u1:0", "a1:0", "a2:0", "a3:0", "a4:0", "a5:0", "r1:0", "a6:0", "u2:0", "a7:0"
            ]
        );
        let turns: Vec<u32> = read.items.iter().map(|i| i.turn).collect();
        assert_eq!(turns, [0, 0, 0, 0, 0, 0, 0, 0, 1, 1]);
        assert_eq!(text_of(&read.items[0]), "What is the capital of France?");
        assert_eq!(read.items[0].role, NativeRole::User);
        assert!(read.items[0].editable && !read.items[0].removable);
    }

    #[test]
    fn split_assistant_message_lines_list_per_block() {
        let read = read(BASIC);
        let by_id = |id: &str| read.items.iter().find(|i| i.id == id).unwrap();
        // Readable thinking: signed, removable, not editable.
        let thinking = by_id("a1:0");
        assert_eq!(
            thinking.kind,
            NativeItemKind::Reasoning {
                text: Some("Simple geography.".into()),
                seal: ReasoningSeal::Signed
            }
        );
        assert!(!thinking.editable && thinking.removable);
        // Empty thinking and redacted thinking: only the sealed form exists.
        for id in ["a3:0", "a4:0"] {
            assert_eq!(
                by_id(id).kind,
                NativeItemKind::Reasoning {
                    text: None,
                    seal: ReasoningSeal::Encrypted
                }
            );
        }
        assert_eq!(text_of(by_id("a2:0")), "Paris.");
        assert!(by_id("a2:0").editable);
        match &by_id("a5:0").kind {
            NativeItemKind::ToolCall { name, summary } => {
                assert_eq!(name, "Bash");
                assert_eq!(summary, "ls -la");
            }
            other => panic!("{other:?}"),
        }
        assert!(!by_id("a5:0").editable && !by_id("a5:0").removable);
        match &by_id("r1:0").kind {
            NativeItemKind::ToolResult { summary, is_error } => {
                assert_eq!(summary, "total 0");
                assert!(!is_error);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn compaction_starts_the_chain_at_the_summary() {
        let read = read(COMPACTED);
        assert!(read.compacted);
        assert_eq!(ids(&read), ["s1:0", "c1:0", "c2:0"]);
        assert!(matches!(
            read.items[0].kind,
            NativeItemKind::CompactionSummary { .. }
        ));
        assert!(read.items[0].editable);
        // The summary is not a prompt: the next real prompt is turn 0.
        assert_eq!(read.items[1].turn, 0);
    }

    #[test]
    fn revision_follows_the_conversation_not_the_bookkeeping() {
        let before = read(BASIC).revision;
        let with_cost = format!(
            "{BASIC}{{\"type\":\"cost-state\",\"sessionId\":\"{OLD}\",\"totalCostUSD\":1}}\n"
        );
        assert_eq!(read(&with_cost).revision, before);
        let longer = format!(
            "{BASIC}{{\"parentUuid\":\"a7\",\"isSidechain\":false,\"type\":\"user\",\"uuid\":\"u3\",\
             \"sessionId\":\"{OLD}\",\"message\":{{\"role\":\"user\",\"content\":\"more\"}}}}\n"
        );
        assert_ne!(read(&longer).revision, before);
    }

    #[test]
    fn replace_edits_text_and_keeps_string_content_a_string() {
        let revision = read(BASIC).revision;
        let out = rewrite(
            BASIC,
            OLD,
            NEW,
            &revision,
            &[
                NativeEdit::Replace {
                    id: "u1:0".into(),
                    text: "What is the capital of Spain?".into(),
                },
                NativeEdit::Replace {
                    id: "a2:0".into(),
                    text: "Madrid.".into(),
                },
            ],
        )
        .unwrap();
        let edited = read(&out);
        assert_eq!(text_of(&edited.items[0]), "What is the capital of Spain?");
        let a2 = edited.items.iter().find(|i| i.id == "a2:0").unwrap();
        assert_eq!(text_of(a2), "Madrid.");
        let u1: Value = out
            .lines()
            .map(|l| serde_json::from_str::<Value>(l).unwrap())
            .find(|v| v["uuid"] == "u1")
            .unwrap();
        assert!(u1["message"]["content"].is_string());
        assert_eq!(ids(&edited), ids(&read(BASIC)));
    }

    #[test]
    fn every_line_moves_to_the_new_session() {
        let revision = read(BASIC).revision;
        let out = rewrite(BASIC, OLD, NEW, &revision, &[]).unwrap();
        assert!(!out.contains(OLD));
        assert_eq!(out.lines().count(), BASIC.lines().count());
        assert!(out.contains(&format!("\"sessionId\":\"{NEW}\"")));
        // Lines the edit never touched come through byte for byte.
        let untouched = BASIC.lines().next().unwrap().replace(OLD, NEW);
        assert_eq!(out.lines().next().unwrap(), untouched);
        assert_eq!(read(&out).revision, revision);
    }

    #[test]
    fn removed_blocks_drop_their_lines_and_relink_children() {
        let revision = read(BASIC).revision;
        let out = rewrite(
            BASIC,
            OLD,
            NEW,
            &revision,
            &[
                NativeEdit::Remove { id: "a1:0".into() },
                // Two dropped lines in a row.
                NativeEdit::Remove { id: "a3:0".into() },
                NativeEdit::Remove { id: "a4:0".into() },
            ],
        )
        .unwrap();
        let parents: HashMap<String, Option<String>> = out
            .lines()
            .map(|l| serde_json::from_str::<Value>(l).unwrap())
            .filter_map(|v| {
                Some((
                    v.get("uuid")?.as_str()?.to_owned(),
                    v["parentUuid"].as_str().map(str::to_owned),
                ))
            })
            .collect();
        for dropped in ["a1", "a3", "a4"] {
            assert!(!parents.contains_key(dropped));
        }
        assert_eq!(parents["a2"].as_deref(), Some("meta0"));
        // a5 hung from a4, which hung from a3, which hung from a2.
        assert_eq!(parents["a5"].as_deref(), Some("a2"));
        let edited = read(&out);
        assert_eq!(
            ids(&edited),
            ["u1:0", "a2:0", "a5:0", "r1:0", "a6:0", "u2:0", "a7:0"]
        );
        // The bookkeeping line that named a dropped line names its parent.
        assert!(!out.contains("\"leafUuid\":\"a4\""));
        assert!(out.contains("\"leafUuid\":\"a2\""));
    }

    #[test]
    fn removing_one_block_of_a_shared_line_keeps_the_line() {
        let shared = format!(
            "{{\"parentUuid\":null,\"isSidechain\":false,\"type\":\"assistant\",\"uuid\":\"m1\",\
             \"sessionId\":\"{OLD}\",\"message\":{{\"role\":\"assistant\",\"id\":\"msg_1\",\
             \"content\":[{{\"type\":\"thinking\",\"thinking\":\"hm\",\"signature\":\"s\"}},\
             {{\"type\":\"text\",\"text\":\"hello\"}}]}}}}\n"
        );
        let read0 = read(&shared);
        assert_eq!(ids(&read0), ["m1:0", "m1:1"]);
        let out = rewrite(
            &shared,
            OLD,
            NEW,
            &read0.revision,
            &[NativeEdit::Remove { id: "m1:0".into() }],
        )
        .unwrap();
        let edited = read(&out);
        assert_eq!(ids(&edited), ["m1:0"]);
        assert_eq!(text_of(&edited.items[0]), "hello");
    }

    #[test]
    fn refuses_what_would_break_the_session() {
        let revision = read(BASIC).revision;
        let attempt = |edit: NativeEdit, revision: &str| {
            rewrite(BASIC, OLD, NEW, revision, &[edit])
                .unwrap_err()
                .to_string()
        };
        assert_eq!(
            attempt(NativeEdit::Remove { id: "a5:0".into() }, &revision),
            "This message can't be removed"
        );
        assert_eq!(
            attempt(NativeEdit::Remove { id: "r1:0".into() }, &revision),
            "This message can't be removed"
        );
        assert_eq!(
            attempt(
                NativeEdit::Replace {
                    id: "a1:0".into(),
                    text: "x".into()
                },
                &revision
            ),
            "This message can't be edited"
        );
        assert_eq!(
            attempt(
                NativeEdit::Replace {
                    id: "u1:0".into(),
                    text: "  ".into()
                },
                &revision
            ),
            "Edited text can't be empty"
        );
        assert_eq!(
            attempt(NativeEdit::Remove { id: "a1:0".into() }, "stale"),
            CHANGED
        );
        assert_eq!(
            attempt(
                NativeEdit::Remove {
                    id: "gone:0".into()
                },
                &revision
            ),
            CHANGED
        );
    }
}
