//! A chat's subagents, read from its transcript's spawn chips: the model
//! behind the desktop Explorer's Subagents section (`crates/ui/src/files`),
//! kept free of any UI so every thin client draws the same list.
//!
//! The chip IS the index — there is no listing endpoint — so a subagent is a
//! spawn part (`ToolCall::is_subagent_spawn`) whose doc ref the engine has
//! stamped. Nothing here hides or removes one.
//!
//! Order and grouping match the desktop exactly:
//!
//! - running subagents lead, longest-running first (one started long ago is
//!   the one to find and steer, and oldest-first keeps the group still as new
//!   ones join at its foot);
//! - a subagent whose status is not stamped yet follows them;
//! - everything settled folds into Finished, split into Completed and Failed,
//!   each most recently updated first (later spawns lead within one turn).
//!
//! Paging (ten rows, "Show more" adds ten) and the open/closed state of the
//! groups are view state and live with the platform.

use std::sync::Arc;

use zeron_doc::{MessagePart, SessionMessageEntry, SubagentStatus};
use zeron_proto::ToolCall;

/// Rows a finished list shows before "Show more", and how many each press
/// adds (the desktop's `INITIAL_ROWS` / `PAGE_ROWS`).
pub const SUBAGENT_PAGE_ROWS: usize = 10;

/// Past this a running count reads "99+" (the desktop pill's cap).
pub const RUNNING_COUNT_CAP: u32 = 99;

/// Spawn titles are one line, capped like the desktop's subagent tab title.
const TITLE_MAX: usize = 40;
/// The output summary kept for a settled subagent's card.
const SUMMARY_MAX: usize = 600;

/// Where a subagent's lifecycle stands, as its spawn chip records it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SubagentState {
    Running,
    /// Spawned and referenced, but no status stamped yet.
    Pending,
    Completed,
    Failed,
}

impl SubagentState {
    fn from_chip(status: Option<SubagentStatus>) -> Self {
        match status {
            Some(SubagentStatus::Running) => Self::Running,
            Some(SubagentStatus::Done) => Self::Completed,
            Some(SubagentStatus::Failed) => Self::Failed,
            None => Self::Pending,
        }
    }

    /// Settled: its transcript is frozen (the engine uploads it as a blob).
    pub fn settled(self) -> bool {
        matches!(self, Self::Completed | Self::Failed)
    }
}

/// One subagent of a chat.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubagentItem {
    /// The subagent's own doc (`{chatId}--sub--{suffix}`): open it to read
    /// its transcript.
    pub doc_id: String,
    /// The spawn tool part's id (the harness's tool-use id).
    pub spawn_id: String,
    /// The bare task ("verify the marker pipeline"), else "Subagent".
    pub title: String,
    /// The chip's detail line: the spawn's description, single-line.
    pub description: String,
    /// The harness's agent type (`subagent_type`: "Explore", "general-purpose"…).
    pub agent_type: Option<String>,
    /// The model the spawn named; `None` = inherits the chat's model.
    pub model: Option<String>,
    pub state: SubagentState,
    /// When the turn that first spawned it was written.
    pub started_at_ms: i64,
    /// When the latest turn that spawned (or steered) it was written — the
    /// closest thing a subagent has to a last-updated time, and the key the
    /// desktop sorts by.
    pub updated_at_ms: i64,
    /// What the spawn returned to the parent (the subagent's report), else
    /// its last streamed line; trimmed, capped.
    pub summary: Option<String>,
    /// The spawn call itself failed (not only the subagent's run).
    pub spawn_failed: bool,
}

impl SubagentItem {
    /// How long it has run, while running; `None` once settled (the chip does
    /// not record when a subagent finished).
    pub fn running_for_ms(&self, now_ms: i64) -> Option<i64> {
        (self.state == SubagentState::Running).then(|| (now_ms - self.started_at_ms).max(0))
    }
}

/// A chat's subagents in display order (see the module docs).
pub fn subagents(entries: &[Arc<SessionMessageEntry>]) -> Vec<SubagentItem> {
    let mut rows: Vec<SubagentItem> = Vec::new();
    for entry in entries {
        for part in &entry.parts {
            let MessagePart::Tool {
                id,
                call,
                is_error,
                output,
                subagent_ref: Some(doc_id),
                subagent_status,
                subagent_tail,
                ..
            } = part
            else {
                continue;
            };
            // A stray ref on a non-Agent tool must not surface as a phantom.
            if !call.is_subagent_spawn() {
                continue;
            }
            let summary = output
                .as_deref()
                .and_then(summary_text)
                .or_else(|| subagent_tail.as_deref().and_then(summary_text));
            let row = SubagentItem {
                doc_id: doc_id.clone(),
                spawn_id: id.clone(),
                title: subagent_title(call),
                description: zeron_proto::view::tool_chip_content(call).1,
                agent_type: spawn_input_str(call, "subagent_type"),
                model: call.subagent_model().map(str::to_owned),
                state: SubagentState::from_chip(*subagent_status),
                started_at_ms: entry.created_at,
                updated_at_ms: entry.created_at,
                summary,
                spawn_failed: *is_error,
            };
            match rows.iter_mut().find(|r| r.doc_id == row.doc_id) {
                // A reopened (steered) subagent updates its row in place; it
                // keeps its first start.
                Some(existing) => {
                    let started = existing.started_at_ms.min(row.started_at_ms);
                    let summary = row.summary.clone().or_else(|| existing.summary.take());
                    *existing = SubagentItem {
                        started_at_ms: started,
                        summary,
                        ..row
                    };
                }
                None => rows.push(row),
            }
        }
    }
    // Stable sort over the reversed spawn order: ties keep the later spawn
    // on top.
    rows.reverse();
    rows.sort_by_key(|row| std::cmp::Reverse(row.updated_at_ms));
    let (mut running, settled): (Vec<_>, Vec<_>) = rows
        .into_iter()
        .partition(|row| row.state == SubagentState::Running);
    // Longest-running first; ties keep spawn order.
    running.reverse();
    running.sort_by_key(|row| row.updated_at_ms);
    running.extend(settled);
    running
}

/// The lists the Subagents panel draws, in order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SubagentGroups {
    /// Not settled: running (longest-running first), then unstamped.
    pub active: Vec<SubagentItem>,
    pub completed: Vec<SubagentItem>,
    pub failed: Vec<SubagentItem>,
}

impl SubagentGroups {
    /// Sort `rows` (already in display order) into the three lists.
    pub fn new(rows: Vec<SubagentItem>) -> Self {
        let mut groups = Self::default();
        for row in rows {
            match row.state {
                SubagentState::Completed => groups.completed.push(row),
                SubagentState::Failed => groups.failed.push(row),
                SubagentState::Running | SubagentState::Pending => groups.active.push(row),
            }
        }
        groups
    }

    /// Straight from a transcript.
    pub fn from_entries(entries: &[Arc<SessionMessageEntry>]) -> Self {
        Self::new(subagents(entries))
    }

    /// Subagents streaming right now.
    pub fn running(&self) -> usize {
        self.active
            .iter()
            .filter(|row| row.state == SubagentState::Running)
            .count()
    }

    pub fn finished(&self) -> usize {
        self.completed.len() + self.failed.len()
    }

    pub fn total(&self) -> usize {
        self.active.len() + self.finished()
    }

    pub fn find(&self, doc_id: &str) -> Option<&SubagentItem> {
        self.active
            .iter()
            .chain(&self.completed)
            .chain(&self.failed)
            .find(|row| row.doc_id == doc_id)
    }
}

/// A running count as the pill and badge draw it: exact up to
/// [`RUNNING_COUNT_CAP`], then "99+".
pub fn running_count_label(count: u32) -> String {
    if count > RUNNING_COUNT_CAP {
        format!("{RUNNING_COUNT_CAP}+")
    } else {
        count.to_string()
    }
}

/// A subagent doc id's namespace: the chat whose engine wrote it. Copied
/// spawn chips keep their original namespace, so this can differ from the
/// chat the chip is read in.
pub fn subagent_source_chat(doc_id: &str) -> Option<&str> {
    doc_id
        .split_once("--sub--")
        .map(|(chat, _)| chat)
        .filter(|chat| !chat.is_empty())
}

/// A subagent doc id (as opposed to a chat id).
pub fn is_subagent_doc(doc_id: &str) -> bool {
    subagent_source_chat(doc_id).is_some()
}

/// The subagent's title: the BARE task ("verify the marker pipeline"). The
/// "Agent: " genus is stripped, and the call input's description/prompt back
/// up a bare name; "Subagent" only as the last resort (the desktop's
/// `subagent_tab_title`).
pub fn subagent_title(call: &ToolCall) -> String {
    let (name, input) = match call {
        ToolCall::Unknown { name, input } => (name.as_str(), input.as_ref()),
        ToolCall::Mcp { tool, input, .. } => (tool.as_str(), input.as_ref()),
        _ => return "Subagent".into(),
    };
    let candidates = [
        Some(name),
        input.and_then(|i| i.get("description")?.as_str()),
        input.and_then(|i| i.get("prompt")?.as_str()),
    ];
    candidates
        .into_iter()
        .flatten()
        .find_map(|text| title_line(strip_spawn_prefix(text), TITLE_MAX))
        .unwrap_or_else(|| "Subagent".into())
}

fn spawn_input_str(call: &ToolCall, key: &str) -> Option<String> {
    let input = match call {
        ToolCall::Unknown { input, .. } | ToolCall::Mcp { input, .. } => input.as_ref()?,
        _ => return None,
    };
    input
        .get(key)?
        .as_str()
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_owned)
}

/// First non-blank line of `text`, trimmed, capped at `max` chars with an
/// ellipsis.
fn title_line(text: &str, max: usize) -> Option<String> {
    let line = text.lines().find(|l| !l.trim().is_empty())?.trim();
    let mut out: String = line.chars().take(max).collect();
    if line.chars().count() > max {
        out.push('…');
    }
    Some(out)
}

/// Drop a leading "Agent"/"Task" genus (with its `:` and spacing). Only a
/// real word boundary strips — "Taskmaster" keeps its name; a bare
/// "Agent"/"Task" strips to "".
fn strip_spawn_prefix(text: &str) -> &str {
    let t = text.trim();
    for prefix in ["agent", "task"] {
        if t.len() >= prefix.len()
            && t.is_char_boundary(prefix.len())
            && t[..prefix.len()].eq_ignore_ascii_case(prefix)
        {
            let rest = &t[prefix.len()..];
            if rest.is_empty() {
                return "";
            }
            if rest.starts_with(':') || rest.starts_with(char::is_whitespace) {
                return rest.trim_start_matches(':').trim();
            }
        }
    }
    t
}

fn summary_text(text: &str) -> Option<String> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let mut out: String = text.chars().take(SUMMARY_MAX).collect();
    if text.chars().count() > SUMMARY_MAX {
        out.push('…');
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use zeron_doc::{MessageRole, MessageStatus};

    use super::*;

    fn spawn(id: &str, name: &str, status: Option<SubagentStatus>) -> MessagePart {
        MessagePart::Tool {
            id: id.into(),
            call: ToolCall::Unknown {
                name: name.into(),
                input: None,
            },
            is_error: false,
            resolved: true,
            output: None,
            diff: None,
            output_ref: None,
            output_bytes: None,
            diff_ref: None,
            diff_stats: None,
            subagent_ref: Some(format!("chat--sub--{id}")),
            subagent_status: status,
            subagent_tail: None,
        }
    }

    fn turn(at: i64, parts: Vec<MessagePart>) -> Arc<SessionMessageEntry> {
        Arc::new(SessionMessageEntry {
            id: format!("m{at}"),
            role: MessageRole::Assistant,
            parts,
            created_at: at,
            device_id: "host".into(),
            status: Some(MessageStatus::Complete),
            continuation_of: None,
            duration_ms: None,
        })
    }

    fn ids(rows: &[SubagentItem]) -> Vec<&str> {
        rows.iter()
            .map(|r| r.doc_id.trim_start_matches("chat--sub--"))
            .collect()
    }

    const RUN: Option<SubagentStatus> = Some(SubagentStatus::Running);
    const DONE: Option<SubagentStatus> = Some(SubagentStatus::Done);
    const FAIL: Option<SubagentStatus> = Some(SubagentStatus::Failed);

    #[test]
    fn running_lead_longest_first_then_settled_newest_first() {
        let entries = vec![
            turn(
                1_000,
                vec![spawn("a", "Agent: a", RUN), spawn("b", "Agent: b", DONE)],
            ),
            turn(
                2_000,
                vec![spawn("c", "Agent: c", RUN), spawn("d", "Agent: d", FAIL)],
            ),
            turn(3_000, vec![spawn("e", "Agent: e", DONE)]),
        ];
        // Running: a (oldest) then c. Settled: e (newest), d, b.
        assert_eq!(ids(&subagents(&entries)), ["a", "c", "e", "d", "b"]);
    }

    #[test]
    fn ties_keep_spawn_order_for_running_and_later_spawn_first_for_settled() {
        let entries = vec![turn(
            1_000,
            vec![
                spawn("r1", "Agent: r1", RUN),
                spawn("r2", "Agent: r2", RUN),
                spawn("s1", "Agent: s1", DONE),
                spawn("s2", "Agent: s2", DONE),
            ],
        )];
        assert_eq!(ids(&subagents(&entries)), ["r1", "r2", "s2", "s1"]);
    }

    #[test]
    fn a_steered_subagent_updates_in_place_and_keeps_its_start() {
        let entries = vec![
            turn(1_000, vec![spawn("a", "Agent: first", DONE)]),
            turn(2_000, vec![spawn("b", "Agent: b", DONE)]),
            turn(5_000, vec![spawn("a", "Agent: again", RUN)]),
        ];
        let rows = subagents(&entries);
        assert_eq!(ids(&rows), ["a", "b"]);
        assert_eq!(rows[0].state, SubagentState::Running);
        assert_eq!(rows[0].title, "again");
        assert_eq!(
            (rows[0].started_at_ms, rows[0].updated_at_ms),
            (1_000, 5_000)
        );
        assert_eq!(rows[0].running_for_ms(6_000), Some(5_000));
        assert_eq!(rows[1].running_for_ms(6_000), None);
    }

    #[test]
    fn only_stamped_spawn_chips_count() {
        let mut unstamped = spawn("x", "Agent: x", None);
        if let MessagePart::Tool { subagent_ref, .. } = &mut unstamped {
            *subagent_ref = None;
        }
        let mut stray = spawn("y", "Bash", DONE);
        if let MessagePart::Tool { call, .. } = &mut stray {
            *call = ToolCall::Exec {
                command: "ls".into(),
            };
        }
        let entries = vec![turn(
            1_000,
            vec![unstamped, stray, spawn("z", "Agent", None)],
        )];
        let rows = subagents(&entries);
        assert_eq!(ids(&rows), ["z"]);
        assert_eq!(rows[0].state, SubagentState::Pending);
        assert_eq!(rows[0].title, "Subagent");
    }

    #[test]
    fn groups_split_active_completed_failed() {
        let entries = vec![turn(
            1_000,
            vec![
                spawn("run", "Agent: run", RUN),
                spawn("new", "Agent: new", None),
                spawn("ok-1", "Agent: ok-1", DONE),
                spawn("bad", "Agent: bad", FAIL),
                spawn("ok-2", "Agent: ok-2", DONE),
            ],
        )];
        let groups = SubagentGroups::from_entries(&entries);
        assert_eq!(ids(&groups.active), ["run", "new"]);
        assert_eq!(ids(&groups.completed), ["ok-2", "ok-1"]);
        assert_eq!(ids(&groups.failed), ["bad"]);
        assert_eq!(
            (groups.running(), groups.finished(), groups.total()),
            (1, 3, 5)
        );
        assert!(groups.find("chat--sub--bad").is_some());
        assert!(SubagentGroups::default().find("chat--sub--bad").is_none());
    }

    #[test]
    fn details_come_from_the_spawn_input_and_result() {
        let mut part = spawn("a", "Agent", DONE);
        if let MessagePart::Tool { call, output, .. } = &mut part {
            // A bare "Agent" digs the task out of the call input.
            *call = ToolCall::Unknown {
                name: "Agent".into(),
                input: Some(serde_json::json!({
                    "description": "Agent: audit the auth flow",
                    "subagent_type": "Explore",
                    "model": "haiku",
                })),
            };
            *output = Some("  Found 3 issues.\n".into());
        }
        let rows = subagents(&[turn(1, vec![part])]);
        let row = &rows[0];
        assert_eq!(row.title, "audit the auth flow");
        assert_eq!(row.agent_type.as_deref(), Some("Explore"));
        assert_eq!(row.model.as_deref(), Some("haiku"));
        assert_eq!(row.summary.as_deref(), Some("Found 3 issues."));
    }

    #[test]
    fn titles_strip_the_genus_on_word_boundaries_only() {
        let call = |name: &str| ToolCall::Unknown {
            name: name.into(),
            input: None,
        };
        assert_eq!(subagent_title(&call("Agent: scan repo")), "scan repo");
        assert_eq!(subagent_title(&call("Taskmaster")), "Taskmaster");
        assert_eq!(subagent_title(&call("agent")), "Subagent");
        let long = subagent_title(&call(&format!("Agent: {}", "x".repeat(60))));
        assert_eq!(long.chars().count(), TITLE_MAX + 1);
        assert!(long.ends_with('…'));
    }

    #[test]
    fn count_label_caps_at_ninety_nine() {
        assert_eq!(running_count_label(0), "0");
        assert_eq!(running_count_label(9), "9");
        assert_eq!(running_count_label(99), "99");
        assert_eq!(running_count_label(100), "99+");
    }

    #[test]
    fn subagent_doc_ids_name_their_source_chat() {
        assert_eq!(subagent_source_chat("chat-1--sub--abc"), Some("chat-1"));
        assert!(is_subagent_doc("chat-1--sub--abc"));
        assert!(!is_subagent_doc("chat-1"));
        assert!(!is_subagent_doc("--sub--abc"));
    }
}
