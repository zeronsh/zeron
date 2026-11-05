//! Message parts: the event fold, the render-only privacy policy, and continuation splitting.
//!
//! Ports of `packages/control/src/parts.ts` (fold) and
//! `packages/session-doc/src/{render-parts,messages}.ts`.

use serde::{Deserialize, Serialize};

use zeron_proto::{
    AgentEvent, SUBAGENT_INPUT_KEEP, ToolCall, ToolDiff, UserInputAnswer, UserInputQuestion,
};

use crate::constants::MSG_INLINE_MAX;

/// Char cap for the tool-output SUMMARY persisted into the doc: first
/// non-empty line, nothing more. The per-part 4KB cap (c951c3e) bounded each
/// part but not the session — chat 1b65e93d measured 917KB (85%) of a 1MB doc
/// in capped outputs across 426 tool parts (docs/chat2-sync.md). Full outputs
/// live in the R2 sidecar behind `output_ref`; t3code ships an 84-char
/// summary, so 160 is generous.
pub const TOOL_OUTPUT_SUMMARY_MAX: usize = 160;

/// The doc-resident form of a tool output (docs/chat2-sync.md A1; the R2
/// sidecar is PARKED as of 2026-08-10, so this IS the whole record in the
/// doc — the full text survives only in the host's local run journal):
///
/// - Markdown code fences are stripped first — ACP harnesses fence every
///   output, so the fence is transport wrapping, never content (pre-fix,
///   every summary read "```console…").
/// - Outputs that fit [`TOOL_OUTPUT_SUMMARY_MAX`] chars ride whole — a
///   summary of a two-line output is more UI than the output.
/// - Bigger outputs keep the first non-empty line, capped, with a `…`
///   marker meaning "there was more".
///
/// `None` for blank output.
pub fn summarize_tool_output(text: &str) -> Option<String> {
    let kept: Vec<&str> = text
        .lines()
        .filter(|l| !l.trim_start().starts_with("```"))
        .collect();
    let stripped = kept.join("\n");
    let stripped = stripped.trim();
    if stripped.is_empty() {
        return None;
    }
    if stripped.chars().count() <= TOOL_OUTPUT_SUMMARY_MAX {
        return Some(stripped.to_owned());
    }
    // Too big to inline: first non-empty line, capped. There is always more
    // than the summary here (whole output exceeded the budget), so the
    // marker is unconditional.
    let line = stripped
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or(stripped)
        .trim_end();
    let mut chars = 0usize;
    let mut end = line.len();
    for (i, _) in line.char_indices() {
        if chars == TOOL_OUTPUT_SUMMARY_MAX {
            end = i;
            break;
        }
        chars += 1;
    }
    let mut out = line[..end].to_owned();
    out.push('…');
    Some(out)
}

/// Lines of a command's output the doc keeps ([`command_output_tail`]).
pub const COMMAND_TAIL_MAX_LINES: usize = 10;
/// Bytes of a command's output the doc keeps ([`command_output_tail`]).
pub const COMMAND_TAIL_MAX_BYTES: usize = 1024;

/// What the doc keeps of a command's output (or, through
/// [`command_output_end`], what its host serves of it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandTail {
    /// The output's last [`COMMAND_TAIL_MAX_LINES`] lines or
    /// [`COMMAND_TAIL_MAX_BYTES`], whichever is less, secrets redacted.
    pub text: String,
    /// The output's whole size, as the harness gave it.
    pub bytes: u64,
    /// Lines in the whole redacted output, so "Show all N lines" can say N
    /// before the rest is fetched.
    pub lines: u64,
    /// Earlier output was let go.
    pub truncated: bool,
}

/// The tail of `command`'s output that rides its tool part into the doc —
/// the one tool output the doc keeps (see the `ToolResult` fold), and the
/// doc syncs to the person's other devices and the cloud. So:
/// - a command that prints secrets by nature (`env`, `cat .env`, `gh auth
///   token`, … — [`zeron_proto::redact::command_prints_secrets`]) keeps
///   none of it, only a note saying so;
/// - any other output is redacted
///   ([`zeron_proto::redact::redact_command_output`]: private key blocks,
///   env-style assignments, secret-named keys, url credentials and webhook
///   paths, tokens, terminal escapes) BEFORE it is cut, so a secret the cut
///   would split can't survive as an unrecognisable half.
///
/// The cut starts on a line where it can: a single line longer than the
/// byte budget keeps its end. `None` when nothing but whitespace is left.
pub fn command_output_tail(command: &str, text: &str) -> Option<CommandTail> {
    command_output_end(command, text, COMMAND_TAIL_MAX_LINES, COMMAND_TAIL_MAX_BYTES)
}

/// The end of `command`'s output, at most `max_lines` lines and `max_bytes`
/// bytes, made safe to leave its host exactly as [`command_output_tail`]
/// makes the doc's tail: the withheld note for a command that prints
/// secrets, else redacted first and cut after. The host's whole-output read
/// (`ReadToolOutput`) goes through here too, with a wider budget, so what a
/// person expands is never less redacted than what the chat kept.
pub fn command_output_end(
    command: &str,
    text: &str,
    max_lines: usize,
    max_bytes: usize,
) -> Option<CommandTail> {
    let bytes = text.len() as u64;
    if text.trim().is_empty() {
        return None;
    }
    if zeron_proto::redact::command_prints_secrets(command) {
        return Some(CommandTail {
            text: zeron_proto::redact::SECRET_OUTPUT_WITHHELD.to_owned(),
            bytes,
            lines: 1,
            truncated: false,
        });
    }
    let redacted = zeron_proto::redact::redact_command_output(text);
    let body = redacted.trim_end();
    if body.trim().is_empty() {
        return None;
    }
    let lines = body.bytes().filter(|b| *b == b'\n').count() as u64 + 1;
    // The last N lines: the start of the line after the Nth newline from
    // the end.
    let mut start = 0;
    let mut newlines = 0;
    for (i, b) in body.bytes().enumerate().rev() {
        if b == b'\n' {
            newlines += 1;
            if newlines == max_lines {
                start = i + 1;
                break;
            }
        }
    }
    if body.len() - start > max_bytes {
        let mut cut = body.len() - max_bytes;
        while !body.is_char_boundary(cut) {
            cut += 1;
        }
        // Begin on a whole line when one starts inside the budget.
        start = match body[cut..].find('\n') {
            Some(nl) if cut + nl + 1 < body.len() => cut + nl + 1,
            _ => cut,
        };
    }
    Some(CommandTail {
        text: body[start..].to_owned(),
        bytes,
        lines,
        truncated: start > 0,
    })
}

/// Per-file diff stats persisted in place of inline diff text (t3's shape).
/// The inline diff was the bigger bomb than outputs — 32KB/edit, unexercised
/// only because the claude harness emits none. Full diff text lives in the
/// sidecar behind `diff_ref`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolDiffStat {
    pub path: String,
    pub additions: u64,
    pub deletions: u64,
}

/// Line-level add/delete counts for one file's diff.
pub fn diff_stat(diff: &ToolDiff) -> ToolDiffStat {
    let (additions, deletions) = match &diff.old_text {
        None => (diff.new_text.lines().count() as u64, 0),
        Some(old) => {
            let text_diff = similar::TextDiff::from_lines(old.as_str(), diff.new_text.as_str());
            let mut additions = 0u64;
            let mut deletions = 0u64;
            for change in text_diff.iter_all_changes() {
                match change.tag() {
                    similar::ChangeTag::Insert => additions += 1,
                    similar::ChangeTag::Delete => deletions += 1,
                    similar::ChangeTag::Equal => {}
                }
            }
            (additions, deletions)
        }
    };
    ToolDiffStat {
        path: diff.path.clone(),
        additions,
        deletions,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MessageStatus {
    Streaming,
    Complete,
    Aborted,
}

/// Lifecycle of a spawned subagent, carried on its spawn chip.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SubagentStatus {
    Running,
    Done,
    Failed,
}

/// One rendered part of an assistant message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum MessagePart {
    Text {
        id: String,
        text: String,
    },
    #[serde(rename_all = "camelCase")]
    Image {
        id: String,
        path: String,
        name: String,
        mime_type: String,
    },
    /// An agent-authored visual (`show_visual`): a Luau script stored on the
    /// host at `path`, rendered natively. Old desktop builds' unknown-kind
    /// fallback yields an empty text part (invisible); iOS drops unknown kinds.
    Visual {
        id: String,
        path: String,
        title: String,
    },
    /// Model thinking. Carries its body in a dedicated doc field (`reasoning`,
    /// never `text`) so pre-reasoning desktop builds — whose unknown-kind
    /// fallback renders `text` as prose — degrade to an invisible empty text
    /// part instead of leaking raw thinking into the transcript. iOS drops
    /// unknown kinds entirely.
    Reasoning {
        id: String,
        text: String,
    },
    #[serde(rename_all = "camelCase")]
    Tool {
        id: String,
        call: ToolCall,
        #[serde(default)]
        is_error: bool,
        /// True once a ToolResult arrived.
        #[serde(default)]
        resolved: bool,
        /// One-line tool output summary ([`summarize_tool_output`]). Old
        /// entries (pre-strip) still carry up to 4KB here; old app versions
        /// render this field either way, so the strip is invisible to them.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output: Option<String>,
        /// Inline file diff — written by pre-strip app versions only; new
        /// folds persist [`Self::Tool::diff_stats`] + `diff_ref` instead.
        /// Kept so old docs render their diffs.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        diff: Option<ToolDiff>,
        /// Sidecar key (`{chatId}/{partId}`) of the full output — additive;
        /// stamped by [`apply_sidecar_refs`] (the fold is chat-agnostic).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output_ref: Option<String>,
        /// Full-output byte length, so the UI can say "Show full output (12 KB)".
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output_bytes: Option<u64>,
        /// Sidecar key (`{chatId}/{partId}.diff`) of the full diff JSON.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        diff_ref: Option<String>,
        /// Per-file diff stats (additive replacement for inline `diff`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        diff_stats: Option<Vec<ToolDiffStat>>,
        /// The SUBAGENT doc id this spawn chip indexes (additive; stamped by
        /// the engine like the sidecar refs — the fold is chat-agnostic).
        /// The chip IS the index: the client learns the doc/blob id from it,
        /// there is no listing endpoint.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subagent_ref: Option<String>,
        /// Subagent lifecycle, DISTINCT from `resolved`: under the eager-done
        /// policy the spawn tool's own result lands while the subagent still
        /// runs. `running` → the ref is a live doc (watch it); `done`/
        /// `failed` → frozen (blob first, doc as fallback).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subagent_status: Option<SubagentStatus>,
        /// One-line live tail of the subagent's latest output, folded from
        /// its tagged text deltas (capped; display-only).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subagent_tail: Option<String>,
        /// A command's exit status, as its harness reported it (command
        /// tools only — see [`command_output_tail`]).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        exit_code: Option<i32>,
        /// `output` is the end of a longer command output (its whole size in
        /// `output_bytes`): the UI says earlier output was trimmed.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        output_truncated: bool,
        /// Lines in the whole (redacted) command output, set only with
        /// `output_truncated`: the UI's "Show all N lines" before the rest
        /// is read from the host.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output_lines: Option<u64>,
    },
    #[serde(rename_all = "camelCase")]
    Input {
        id: String,
        request_id: String,
        questions: Vec<UserInputQuestion>,
        #[serde(default)]
        resolved: bool,
        /// What the person answered, stamped by the host as the run received
        /// it. Presence is the record, like a tool part's `isError`: `[]` when
        /// the questions closed unanswered, absent while pending and on parts
        /// written before answers were recorded (old readers ignore it).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        answers: Option<Vec<UserInputAnswer>>,
    },
    Error {
        id: String,
        message: String,
    },
    /// The seam in a forked chat's transcript: everything above was copied
    /// from `source_chat_id` when the fork was cut, everything below is this
    /// chat's own. Written once by the fork RPC; renders as a labeled
    /// divider. Old desktop builds' unknown-kind fallback yields an empty
    /// text part (invisible); iOS drops unknown kinds.
    #[serde(rename_all = "camelCase")]
    Fork {
        id: String,
        source_chat_id: String,
        /// The source's title as of the fork — the transcript reads it
        /// without a registry lookup, and a later rename or delete of the
        /// source does not rewrite history.
        source_title: String,
    },
    /// The seam where a chat changed provider: the turns above ran on
    /// `from`, the turns below run on `to`. Written by the host engine when
    /// the first run after a harness change is dispatched. Labels are
    /// display names ("Claude Code", "Codex"), frozen as of the switch.
    /// `from_harness` / `to_harness` are the harness slugs (`claude-code`,
    /// `codex`, ...) the UI draws provider logos from; absent on a seam written
    /// without them. Old builds' unknown-kind fallback yields an invisible empty
    /// text part.
    #[serde(rename_all = "camelCase")]
    Switch {
        id: String,
        from: String,
        to: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        from_harness: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        to_harness: Option<String>,
    },
}

impl MessagePart {
    pub fn id(&self) -> &str {
        match self {
            MessagePart::Text { id, .. }
            | MessagePart::Image { id, .. }
            | MessagePart::Visual { id, .. }
            | MessagePart::Reasoning { id, .. }
            | MessagePart::Tool { id, .. }
            | MessagePart::Input { id, .. }
            | MessagePart::Error { id, .. }
            | MessagePart::Fork { id, .. }
            | MessagePart::Switch { id, .. } => id,
        }
    }

    pub fn byte_len(&self) -> usize {
        match self {
            MessagePart::Text { text, .. } | MessagePart::Reasoning { text, .. } => text.len(),
            MessagePart::Tool {
                call,
                output,
                diff,
                diff_stats,
                ..
            } => {
                serde_json::to_vec(call).map_or(0, |v| v.len())
                    + output.as_ref().map_or(0, String::len)
                    + diff
                        .as_ref()
                        .map_or(0, |d| serde_json::to_vec(d).map_or(0, |v| v.len()))
                    + diff_stats
                        .as_ref()
                        .map_or(0, |s| serde_json::to_vec(s).map_or(0, |v| v.len()))
            }
            MessagePart::Input {
                questions, answers, ..
            } => {
                serde_json::to_vec(questions).map_or(0, |v| v.len())
                    + answers
                        .as_ref()
                        .map_or(0, |a| serde_json::to_vec(a).map_or(0, |v| v.len()))
            }
            MessagePart::Image {
                id,
                path,
                name,
                mime_type,
            } => id.len() + path.len() + name.len() + mime_type.len(),
            MessagePart::Visual { id, path, title } => id.len() + path.len() + title.len(),
            MessagePart::Error { message, .. } => message.len(),
            MessagePart::Fork {
                source_chat_id,
                source_title,
                ..
            } => source_chat_id.len() + source_title.len(),
            MessagePart::Switch { from, to, .. } => from.len() + to.len(),
        }
    }
}

/// Fold one agent event into a parts accumulator, in place.
///
/// In place because the fold runs once per streamed event: rebuilding the
/// accumulator each time made long turns O(n²) in allocations.
///
/// Semantics from zeron `foldEventIntoParts`:
/// - `SessionStarted` / `Steered` reset the accumulator (turn boundary — makes replay safe).
/// - `TextDelta` appends to the trailing text part, or starts a new one if the trail is not text
///   (a tool call in between breaks the text block).
/// - `ToolCall` appends, or refreshes in place when the id already exists (SDK retry idempotence).
/// - `ToolResult` marks the matching tool part resolved / errored in place.
/// - `InputRequested` appends an input part; `InputResolved` marks it resolved and records the
///   answers it carries.
/// - `Error` and `Done{error}` become visible error parts.
pub fn fold_event_into_parts(out: &mut Vec<MessagePart>, event: &AgentEvent) {
    match event {
        AgentEvent::SessionStarted { .. } | AgentEvent::Steered { .. } => {
            out.clear();
        }
        AgentEvent::TextDelta { text } => {
            if let Some(MessagePart::Text { text: tail, .. }) = out.last_mut() {
                tail.push_str(text);
            } else {
                let id = format!("t{}", out.len());
                out.push(MessagePart::Text {
                    id,
                    text: text.clone(),
                });
            }
        }
        AgentEvent::GeneratedImage {
            id,
            path,
            name,
            mime_type,
        } => {
            let image = MessagePart::Image {
                id: id.clone(),
                path: path.clone(),
                name: name.clone(),
                mime_type: mime_type.clone(),
            };
            if let Some(existing) = out.iter_mut().find(|p| p.id() == id) {
                *existing = image;
            } else {
                out.push(image);
            }
        }
        AgentEvent::Visual { id, path, title } => {
            let visual = MessagePart::Visual {
                id: id.clone(),
                path: path.clone(),
                title: title.clone(),
            };
            if let Some(existing) = out.iter_mut().find(|p| p.id() == id) {
                *existing = visual;
            } else {
                out.push(visual);
            }
        }
        AgentEvent::ReasoningDelta { text } => {
            // Empty deltas are liveness heartbeats (redacted thinking) — the
            // engine drops them before the fold, but stay tolerant here too.
            if text.is_empty() {
                return;
            }
            if let Some(MessagePart::Reasoning { text: tail, .. }) = out.last_mut() {
                tail.push_str(text);
            } else {
                let id = format!("r{}", out.len());
                out.push(MessagePart::Reasoning {
                    id,
                    text: text.clone(),
                });
            }
        }
        AgentEvent::ToolCall { id, call } => {
            if let Some(existing) = out.iter_mut().find_map(|p| match p {
                MessagePart::Tool {
                    id: pid, call: c, ..
                } if pid == id => Some(c),
                _ => None,
            }) {
                *existing = call.clone();
            } else {
                out.push(MessagePart::Tool {
                    id: id.clone(),
                    call: call.clone(),
                    is_error: false,
                    resolved: false,
                    output: None,
                    diff: None,
                    output_ref: None,
                    output_bytes: None,
                    diff_ref: None,
                    diff_stats: None,
                    subagent_ref: None,
                    subagent_status: None,
                    subagent_tail: None,
                    exit_code: None,
                    output_truncated: false,
                    output_lines: None,
                });
            }
        }
        AgentEvent::ToolResult {
            id,
            is_error,
            output,
            diff,
            exit_code,
        } => {
            for p in out.iter_mut() {
                if let MessagePart::Tool {
                    id: pid,
                    call,
                    is_error: e,
                    resolved,
                    output: out_slot,
                    diff: diff_slot,
                    output_bytes,
                    diff_stats,
                    exit_code: exit_slot,
                    output_truncated,
                    output_lines,
                    ..
                } = p
                    && pid == id
                {
                    *e = *is_error;
                    *resolved = true;
                    // Tool OUTPUTS never enter the doc (2026-08-10 product
                    // call: chips are one-liners — name + call info — like
                    // pre-output builds; the R2 sidecar is parked with them,
                    // docs/chat2-sync.md A2). Full text lives only in the
                    // host's run journal. Inline diffs die the same way:
                    // stats only, never text. `is_error` still folds so
                    // failed chips read as failed.
                    //
                    // The one exception (2026-11-03, user decision): a
                    // COMMAND's output keeps a short, redacted tail — its
                    // last 10 lines or 1 KB, whichever is less — with its
                    // exit code, so an opened Run row shows how it ended (a
                    // build's error, a test run's summary) the way a
                    // terminal would. Bounded per part, so a long session
                    // grows by at most ~1 KB per command; file reads,
                    // searches, fetches and MCP results stay out. The whole
                    // output stays in the host's run journal, never synced;
                    // a reader who expands it asks the host for it
                    // (`ReadToolOutput`, redacted the same way).
                    let tail = match (&*call, output.as_deref()) {
                        (ToolCall::Exec { command }, Some(output)) => {
                            command_output_tail(command, output)
                        }
                        _ => None,
                    };
                    *exit_slot = if matches!(call, ToolCall::Exec { .. }) {
                        *exit_code
                    } else {
                        None
                    };
                    match tail {
                        Some(tail) => {
                            *out_slot = Some(tail.text);
                            *output_bytes = Some(tail.bytes);
                            *output_truncated = tail.truncated;
                            *output_lines = tail.truncated.then_some(tail.lines);
                        }
                        None => {
                            *out_slot = None;
                            *output_bytes = None;
                            *output_truncated = false;
                            *output_lines = None;
                        }
                    }
                    *diff_slot = None;
                    *diff_stats = diff.as_ref().map(|d| vec![diff_stat(d)]);
                }
            }
        }
        AgentEvent::InputRequested {
            request_id,
            questions,
        } => {
            let id = format!("in-{request_id}");
            if !out.iter().any(|p| p.id() == id) {
                out.push(MessagePart::Input {
                    id,
                    request_id: request_id.clone(),
                    questions: questions.clone(),
                    resolved: false,
                    answers: None,
                });
            }
        }
        AgentEvent::InputResolved {
            request_id,
            answers,
        } => {
            for p in out.iter_mut() {
                if let MessagePart::Input {
                    request_id: rid,
                    resolved,
                    answers: recorded,
                    ..
                } = p
                    && rid == request_id
                {
                    *resolved = true;
                    // An event from before answers were recorded knows
                    // nothing about them: leave the part's record alone.
                    if let Some(answers) = answers {
                        *recorded = Some(answers.clone());
                    }
                }
            }
        }
        AgentEvent::Error { message } => {
            let id = format!("e{}", out.len());
            out.push(MessagePart::Error {
                id,
                message: message.clone(),
            });
        }
        AgentEvent::Done { error, .. } => {
            if let Some(message) = error {
                let id = format!("e{}", out.len());
                out.push(MessagePart::Error {
                    id,
                    message: message.clone(),
                });
            }
        }
        // Subagent-attributed CONTENT belongs to the subagent's own doc (the
        // engine routes it there); the parent doc keeps only the spawn chip —
        // which these events refresh in place: LIFECYCLE ONLY. A live tail
        // was tried and rejected: rewriting the chip per delta batch grew
        // the parent doc's oplog for the whole subagent run and rendered as
        // distracting mid-stream fragments (user call, 2026-08-18). The
        // `subagent_tail` field stays in the schema for docs that carry it.
        AgentEvent::Subagent {
            parent_tool_use_id,
            event,
        } => {
            let status = match event.as_ref() {
                AgentEvent::Done { status, .. } => Some(match status {
                    zeron_proto::DoneStatus::Errored => SubagentStatus::Failed,
                    _ => SubagentStatus::Done,
                }),
                // A new assignment reopens a settled chip. Providers may
                // announce it with user text or a confirmed turn boundary.
                AgentEvent::UserMessage { .. } | AgentEvent::Steered { .. } => {
                    Some(SubagentStatus::Running)
                }
                _ => None,
            };
            for p in out.iter_mut() {
                if let MessagePart::Tool {
                    id,
                    call,
                    subagent_status,
                    ..
                } = p
                    && id == parent_tool_use_id
                    // Genus gate: only a SPAWN call ever carries subagent
                    // lifecycle. A driver keying bug (claude's background
                    // shells settled through the subagent subtype,
                    // 2026-08-20) must not decorate an ordinary chip.
                    && call.is_subagent_spawn()
                {
                    match status {
                        Some(s) => *subagent_status = Some(s),
                        // Any tagged traffic proves the subagent is live;
                        // never regress a terminal state.
                        None if !matches!(
                            subagent_status,
                            Some(SubagentStatus::Done) | Some(SubagentStatus::Failed)
                        ) =>
                        {
                            *subagent_status = Some(SubagentStatus::Running);
                        }
                        None => {}
                    }
                }
            }
        }
        // AvailableCommands feeds the engine's per-harness command cache, not
        // the transcript. UserMessage becomes its own doc ENTRY (the engine's
        // subagent sink writes it), never a part of the assistant message.
        AgentEvent::AssistantMessageCompleted { .. }
        | AgentEvent::Usage { .. }
        | AgentEvent::PlanUsage { .. }
        | AgentEvent::ContextUsage { .. }
        | AgentEvent::AvailableCommands { .. }
        | AgentEvent::UserMessage { .. } => {}
    }
}

/// Stamp sidecar keys onto resolved tool parts that have sidecar content.
///
/// Separate from the fold because the fold is chat-agnostic and pure; the
/// caller (who knows the chat id) runs this right after each fold step, before
/// the parts hit the doc. Idempotent. Key shape `{chatId}/{partId}` (+
/// `.diff`) matches the edge's `/blob/{chatId}/{partId}` route.
pub fn apply_sidecar_refs(chat_id: &str, parts: &mut [MessagePart]) {
    for part in parts.iter_mut() {
        if let MessagePart::Tool {
            id,
            resolved: true,
            output_ref,
            output_bytes,
            diff_ref,
            diff_stats,
            ..
        } = part
        {
            if output_ref.is_none() && output_bytes.is_some() {
                *output_ref = Some(format!("{chat_id}/{id}"));
            }
            if diff_ref.is_none() && diff_stats.is_some() {
                *diff_ref = Some(format!("{chat_id}/{id}.diff"));
            }
        }
    }
}

/// What a [`AgentEvent::ToolResult`] owes the sidecar: the full output text
/// and/or the full diff (as JSON), keyed by part id. `None` when the event
/// carries nothing worth uploading.
#[derive(Debug, Clone, PartialEq)]
pub struct SidecarPayload {
    pub part_id: String,
    pub output: Option<String>,
    pub diff: Option<ToolDiff>,
}

pub fn sidecar_payload(event: &AgentEvent) -> Option<SidecarPayload> {
    let AgentEvent::ToolResult {
        id, output, diff, ..
    } = event
    else {
        return None;
    };
    let output = output.clone().filter(|o| !o.trim().is_empty());
    if output.is_none() && diff.is_none() {
        return None;
    }
    Some(SidecarPayload {
        part_id: id.clone(),
        output,
        diff: diff.clone(),
    })
}

/// Render-only privacy policy — strip heavy/sensitive tool inputs before a call enters the doc.
///
/// Keeps: command / path / pattern / url / query / todo items / server+tool names,
/// and a subagent spawn's model/type (see [`spawn_badge`] — a couple of short
/// identifiers the chip names the child by).
/// Drops: WriteFile content, EditFile old/new strings, WebFetch prompt, Mcp/Unknown input.
/// Full inputs remain only in the host's local run journal. Idempotent.
pub fn sanitize_tool_call(call: &ToolCall) -> ToolCall {
    match call {
        ToolCall::WriteFile { path, .. } => ToolCall::WriteFile {
            path: path.clone(),
            content: None,
        },
        ToolCall::EditFile { path, .. } => ToolCall::EditFile {
            path: path.clone(),
            old_string: None,
            new_string: None,
        },
        ToolCall::WebFetch { url, .. } => ToolCall::WebFetch {
            url: url.clone(),
            prompt: None,
        },
        ToolCall::Mcp { server, tool, .. } => ToolCall::Mcp {
            server: server.clone(),
            tool: tool.clone(),
            input: spawn_badge(call),
        },
        ToolCall::Unknown { name, .. } => ToolCall::Unknown {
            name: name.clone(),
            input: spawn_badge(call),
        },
        other => other.clone(),
    }
}

/// The only slice of a tool input allowed into the doc: a subagent spawn's
/// [`SUBAGENT_INPUT_KEEP`] keys, so the chip can say WHICH model the child
/// runs on (`Agent · haiku`) without the reader opening the subagent tab.
///
/// Everything else — the prompt above all — stays in the host's run journal,
/// so this stays a whitelist of short identifiers rather than a size cap.
/// `None` for anything that is not a spawn, and for a spawn that named
/// neither, which keeps it idempotent: re-sanitizing a sanitized call is a
/// fixpoint (the kept keys are themselves kept).
fn spawn_badge(call: &ToolCall) -> Option<serde_json::Value> {
    if !call.is_subagent_spawn() {
        return None;
    }
    let input = match call {
        ToolCall::Unknown { input, .. } | ToolCall::Mcp { input, .. } => input.as_ref()?,
        _ => return None,
    };
    let kept: serde_json::Map<String, serde_json::Value> = SUBAGENT_INPUT_KEEP
        .iter()
        .filter_map(|key| {
            let value = input.get(key)?.as_str()?.trim();
            (!value.is_empty()).then(|| ((*key).to_owned(), serde_json::Value::from(value)))
        })
        .collect();
    (!kept.is_empty()).then(|| serde_json::Value::Object(kept))
}

/// Deterministic continuation id: `"{root}#c{n}"`.
pub fn continuation_id(root: &str, index: usize) -> String {
    format!("{root}#c{index}")
}

/// Split an oversized parts list into chunks each under `MSG_INLINE_MAX` bytes.
///
/// Splitting happens at part boundaries; an oversized text part is itself chunked at char
/// boundaries. Returns one Vec per resulting entry — the first keeps the root id, the rest are
/// continuations (`continuation_id(root, i)`), matching `splitMessageEntry` in zeron.
pub fn split_parts(parts: &[MessagePart]) -> Vec<Vec<MessagePart>> {
    let mut chunks: Vec<Vec<MessagePart>> = vec![Vec::new()];
    let mut current_bytes = 0usize;

    let push_part = |chunks: &mut Vec<Vec<MessagePart>>, current: &mut usize, part: MessagePart| {
        let len = part.byte_len();
        if *current > 0 && *current + len > MSG_INLINE_MAX {
            chunks.push(Vec::new());
            *current = 0;
        }
        *current += len;
        chunks.last_mut().unwrap().push(part);
    };

    for part in parts {
        // Both text-bodied kinds chunk the same way; extended thinking can
        // exceed the cap just as easily as a long reply.
        let (id, text, reasoning) = match part {
            MessagePart::Text { id, text } if text.len() > MSG_INLINE_MAX => (id, text, false),
            MessagePart::Reasoning { id, text } if text.len() > MSG_INLINE_MAX => (id, text, true),
            other => {
                push_part(&mut chunks, &mut current_bytes, other.clone());
                continue;
            }
        };
        // Chunk oversized text at char boundaries.
        let mut start = 0usize;
        let mut piece = 0usize;
        while start < text.len() {
            let mut end = (start + MSG_INLINE_MAX).min(text.len());
            while end < text.len() && !text.is_char_boundary(end) {
                end -= 1;
            }
            // Guard: ensure forward progress on pathological boundaries.
            if end <= start {
                end = text.len();
            }
            let sub_id = if piece == 0 {
                id.clone()
            } else {
                format!("{id}~{piece}")
            };
            let body = text[start..end].to_string();
            let sub = if reasoning {
                MessagePart::Reasoning {
                    id: sub_id,
                    text: body,
                }
            } else {
                MessagePart::Text {
                    id: sub_id,
                    text: body,
                }
            };
            push_part(&mut chunks, &mut current_bytes, sub);
            start = end;
            piece += 1;
        }
    }
    chunks
}

/// Render-time inverse of splitting: concatenate continuation entries' parts in list order.
pub fn join_continuations(entries: Vec<Vec<MessagePart>>) -> Vec<MessagePart> {
    entries.into_iter().flatten().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text_delta(s: &str) -> AgentEvent {
        AgentEvent::TextDelta { text: s.into() }
    }

    #[test]
    fn reasoning_deltas_fold_into_their_own_part() {
        let mut parts = Vec::new();
        fold_event_into_parts(
            &mut parts,
            &AgentEvent::ReasoningDelta {
                text: "let me ".into(),
            },
        );
        fold_event_into_parts(
            &mut parts,
            &AgentEvent::ReasoningDelta {
                text: "think".into(),
            },
        );
        // Empty deltas (redacted-thinking heartbeats) never mint a part.
        fold_event_into_parts(&mut parts, &AgentEvent::ReasoningDelta { text: "".into() });
        assert_eq!(parts.len(), 1);
        assert_eq!(
            parts[0],
            MessagePart::Reasoning {
                id: "r0".into(),
                text: "let me think".into()
            }
        );
        // Text breaks the reasoning block; a later thought starts a new part.
        fold_event_into_parts(&mut parts, &text_delta("Answer"));
        fold_event_into_parts(
            &mut parts,
            &AgentEvent::ReasoningDelta {
                text: "more".into(),
            },
        );
        assert_eq!(parts.len(), 3);
        assert!(matches!(
            &parts[2],
            MessagePart::Reasoning { id, text } if id == "r2" && text == "more"
        ));
    }

    #[test]
    fn input_resolution_records_what_the_run_received() {
        let asked = |id: &str| AgentEvent::InputRequested {
            request_id: id.into(),
            questions: vec![],
        };
        let picked = vec![UserInputAnswer {
            question_id: "q".into(),
            labels: vec!["B".into()],
        }];
        let mut parts = Vec::new();
        for id in ["answered", "closed", "legacy"] {
            fold_event_into_parts(&mut parts, &asked(id));
        }
        assert!(matches!(
            &parts[0],
            MessagePart::Input { answers: None, .. }
        ));
        fold_event_into_parts(
            &mut parts,
            &AgentEvent::InputResolved {
                request_id: "answered".into(),
                answers: Some(picked.clone()),
            },
        );
        fold_event_into_parts(
            &mut parts,
            &AgentEvent::InputResolved {
                request_id: "closed".into(),
                answers: Some(vec![]),
            },
        );
        // An event from before answers were recorded resolves the part but
        // records nothing.
        fold_event_into_parts(
            &mut parts,
            &AgentEvent::InputResolved {
                request_id: "legacy".into(),
                answers: None,
            },
        );
        let recorded: Vec<_> = parts
            .iter()
            .map(|p| match p {
                MessagePart::Input {
                    resolved, answers, ..
                } => (*resolved, answers.clone()),
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(
            recorded,
            vec![(true, Some(picked)), (true, Some(vec![])), (true, None)]
        );
        // The splitter counts answers toward the entry's byte budget.
        let bare = MessagePart::Input {
            id: "i".into(),
            request_id: "i".into(),
            questions: vec![],
            resolved: true,
            answers: None,
        };
        assert!(parts[0].byte_len() > bare.byte_len());
    }

    #[test]
    fn oversized_reasoning_splits_like_text() {
        let big = "x".repeat(MSG_INLINE_MAX + 10);
        let parts = vec![MessagePart::Reasoning {
            id: "r0".into(),
            text: big,
        }];
        let chunks = split_parts(&parts);
        let flat = join_continuations(chunks);
        assert!(flat.len() >= 2, "{}", flat.len());
        assert!(
            flat.iter()
                .all(|p| matches!(p, MessagePart::Reasoning { .. }))
        );
        assert!(flat.iter().all(|p| p.byte_len() <= MSG_INLINE_MAX));
        assert_eq!(flat[1].id(), "r0~1");
    }

    #[test]
    fn generated_image_fold_is_ordered_atomic_and_idempotent() {
        let mut parts = vec![];
        fold_event_into_parts(&mut parts, &text_delta("before"));
        fold_event_into_parts(
            &mut parts,
            &AgentEvent::ToolCall {
                id: "i".into(),
                call: ToolCall::Unknown {
                    name: "Generate image".into(),
                    input: None,
                },
            },
        );
        let event = AgentEvent::GeneratedImage {
            id: "i:image".into(),
            path: "/uploads/i.png".into(),
            name: "generated.png".into(),
            mime_type: "image/png".into(),
        };
        fold_event_into_parts(&mut parts, &event);
        fold_event_into_parts(&mut parts, &text_delta("after"));
        fold_event_into_parts(&mut parts, &event);
        assert_eq!(parts.len(), 4);
        assert!(matches!(&parts[2], MessagePart::Image { id, .. } if id == "i:image"));
        assert_eq!(join_continuations(split_parts(&parts)), parts);
    }

    #[test]
    fn text_deltas_merge_until_broken_by_tool() {
        let mut parts = Vec::new();
        fold_event_into_parts(&mut parts, &text_delta("Hello "));
        fold_event_into_parts(&mut parts, &text_delta("world"));
        assert_eq!(parts.len(), 1);
        fold_event_into_parts(
            &mut parts,
            &AgentEvent::ToolCall {
                id: "tool-1".into(),
                call: ToolCall::Exec {
                    command: "ls".into(),
                },
            },
        );
        fold_event_into_parts(&mut parts, &text_delta("after"));
        assert_eq!(parts.len(), 3);
        match &parts[2] {
            MessagePart::Text { text, .. } => assert_eq!(text, "after"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn session_started_resets_accumulator() {
        let mut parts = Vec::new();
        fold_event_into_parts(&mut parts, &text_delta("junk"));
        fold_event_into_parts(
            &mut parts,
            &AgentEvent::SessionStarted {
                harness: zeron_proto::HarnessId::Mock,
                model: "m".into(),
                tools: vec![],
                cwd: "/".into(),
                session_id: "s".into(),
                assistant_message_id: "a".into(),
            },
        );
        assert!(parts.is_empty());
    }

    #[test]
    fn tool_call_refresh_is_idempotent() {
        let call = AgentEvent::ToolCall {
            id: "t".into(),
            call: ToolCall::Exec {
                command: "ls".into(),
            },
        };
        let mut once = Vec::new();
        fold_event_into_parts(&mut once, &call);
        let mut twice = once.clone();
        fold_event_into_parts(&mut twice, &call);
        assert_eq!(once, twice);
    }

    #[test]
    fn tool_result_marks_resolution() {
        let mut parts = Vec::new();
        fold_event_into_parts(
            &mut parts,
            &AgentEvent::ToolCall {
                id: "t".into(),
                call: ToolCall::Exec {
                    command: "ls".into(),
                },
            },
        );
        fold_event_into_parts(
            &mut parts,
            &AgentEvent::ToolResult {
                id: "t".into(),
                is_error: true,
                output: None,
                diff: None,
                exit_code: None,
            },
        );
        match &parts[0] {
            MessagePart::Tool {
                is_error, resolved, ..
            } => {
                assert!(*is_error);
                assert!(*resolved);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn sanitize_strips_heavy_inputs_and_is_idempotent() {
        let call = ToolCall::WriteFile {
            path: "/x".into(),
            content: Some("secret".into()),
        };
        let clean = sanitize_tool_call(&call);
        assert_eq!(
            clean,
            ToolCall::WriteFile {
                path: "/x".into(),
                content: None
            }
        );
        assert_eq!(sanitize_tool_call(&clean), clean);
    }

    /// A spawn keeps the two short identifiers its chip names the child by and
    /// drops the prompt — the whole point of the whitelist. Still a fixpoint.
    #[test]
    fn sanitize_keeps_a_spawns_model_and_drops_its_prompt() {
        let call = ToolCall::Unknown {
            name: "Agent: Explore theme system".into(),
            input: Some(serde_json::json!({
                "description": "Explore theme system",
                "subagent_type": "Explore",
                "model": "haiku",
                "prompt": "a very long private prompt",
            })),
        };
        let clean = sanitize_tool_call(&call);
        assert_eq!(
            clean,
            ToolCall::Unknown {
                name: "Agent: Explore theme system".into(),
                input: Some(serde_json::json!({
                    "model": "haiku",
                    "subagent_type": "Explore",
                })),
            }
        );
        assert_eq!(clean.subagent_model(), Some("haiku"));
        assert_eq!(sanitize_tool_call(&clean), clean);
    }

    /// An ordinary tool's input still goes, even when it happens to carry a
    /// `model` argument — the badge is gated on the spawn genus, not the key.
    #[test]
    fn sanitize_still_strips_a_non_spawn_carrying_a_model_argument() {
        let call = ToolCall::Unknown {
            name: "SomeTool".into(),
            input: Some(serde_json::json!({ "model": "haiku", "prompt": "secret" })),
        };
        assert_eq!(
            sanitize_tool_call(&call),
            ToolCall::Unknown {
                name: "SomeTool".into(),
                input: None,
            }
        );
    }

    /// A spawn that named no model keeps no input at all — `None`, not an
    /// empty object, so the doc gains nothing and the fixpoint is exact.
    #[test]
    fn sanitize_drops_a_spawn_input_that_names_nothing_worth_keeping() {
        let call = ToolCall::Unknown {
            name: "Agent".into(),
            input: Some(serde_json::json!({ "prompt": "secret", "model": "  " })),
        };
        let clean = sanitize_tool_call(&call);
        assert_eq!(
            clean,
            ToolCall::Unknown {
                name: "Agent".into(),
                input: None,
            }
        );
        assert_eq!(clean.subagent_model(), None);
    }

    #[test]
    fn split_and_join_round_trip() {
        let big = "x".repeat(MSG_INLINE_MAX * 2 + 100);
        let parts = vec![
            MessagePart::Text {
                id: "t0".into(),
                text: big.clone(),
            },
            MessagePart::Tool {
                id: "tool-1".into(),
                call: ToolCall::Exec {
                    command: "ls".into(),
                },
                is_error: false,
                resolved: true,
                output: None,
                diff: None,
                output_ref: None,
                output_bytes: None,
                diff_ref: None,
                diff_stats: None,
                subagent_ref: None,
                subagent_status: None,
                subagent_tail: None,
                exit_code: None,
                output_truncated: false,
                output_lines: None,
            },
        ];
        let chunks = split_parts(&parts);
        assert!(
            chunks.len() >= 3,
            "expected >=3 chunks, got {}",
            chunks.len()
        );
        for chunk in &chunks {
            let bytes: usize = chunk.iter().map(|p| p.byte_len()).sum();
            assert!(bytes <= MSG_INLINE_MAX, "chunk over cap: {bytes}");
        }
        let joined = join_continuations(chunks);
        let text: String = joined
            .iter()
            .filter_map(|p| match p {
                MessagePart::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(text, big);
        assert!(matches!(joined.last().unwrap(), MessagePart::Tool { .. }));
    }

    #[test]
    fn continuation_ids_are_deterministic() {
        assert_eq!(continuation_id("m1", 1), "m1#c1");
    }

    // ── A1 strip (docs/chat2-sync.md) ───────────────────────────────────────

    #[test]
    fn summarize_inlines_small_outputs_and_marks_big_cuts() {
        assert_eq!(summarize_tool_output(""), None);
        assert_eq!(summarize_tool_output("  \n\t\n"), None);
        assert_eq!(summarize_tool_output("one line"), Some("one line".into()));
        // Small multi-line outputs ride whole — no summary, no "…".
        assert_eq!(
            summarize_tool_output("\n\nfirst real\nsecond"),
            Some("first real\nsecond".into())
        );
        assert_eq!(
            summarize_tool_output("only line\n\n  \n"),
            Some("only line".into())
        );
        // Markdown fences are transport wrapping, never content: stripped
        // even when they'd otherwise be the first line, and a fence-only
        // output is blank.
        assert_eq!(
            summarize_tool_output("```console\nreal content\n```"),
            Some("real content".into())
        );
        assert_eq!(summarize_tool_output("```\n```"), None);
        // Big outputs: first non-empty (post-fence) line + unconditional "…".
        let big = format!("```console\nhead line\n{}\n```", "x".repeat(300));
        assert_eq!(summarize_tool_output(&big), Some("head line…".into()));
        let long = "x".repeat(TOOL_OUTPUT_SUMMARY_MAX + 40);
        let summary = summarize_tool_output(&long).unwrap();
        assert_eq!(summary.chars().count(), TOOL_OUTPUT_SUMMARY_MAX + 1);
        assert!(summary.ends_with('…'));
        // Char-boundary safety on multibyte input.
        let wide = "é".repeat(TOOL_OUTPUT_SUMMARY_MAX + 5);
        let summary = summarize_tool_output(&wide).unwrap();
        assert_eq!(summary.chars().count(), TOOL_OUTPUT_SUMMARY_MAX + 1);
    }

    #[test]
    fn diff_stat_counts_line_changes() {
        let stat = diff_stat(&ToolDiff {
            path: "/w/a.rs".into(),
            old_text: Some("a\nb\nc\n".into()),
            new_text: "a\nB\nc\nd\n".into(),
        });
        assert_eq!(stat.path, "/w/a.rs");
        assert_eq!(stat.additions, 2); // B + d
        assert_eq!(stat.deletions, 1); // b
        // New file: every line is an addition.
        let stat = diff_stat(&ToolDiff {
            path: "/w/new.rs".into(),
            old_text: None,
            new_text: "one\ntwo\n".into(),
        });
        assert_eq!((stat.additions, stat.deletions), (2, 0));
    }

    fn fold_command(output: &str, exit_code: Option<i32>) -> MessagePart {
        let mut parts = Vec::new();
        for event in [
            AgentEvent::ToolCall {
                id: "c".into(),
                call: ToolCall::Exec {
                    command: "make".into(),
                },
            },
            AgentEvent::ToolResult {
                id: "c".into(),
                is_error: exit_code.is_some_and(|c| c != 0),
                output: Some(output.into()),
                diff: None,
                exit_code,
            },
        ] {
            fold_event_into_parts(&mut parts, &event);
        }
        parts.remove(0)
    }

    #[test]
    fn a_command_keeps_its_last_ten_lines() {
        let full: String = (1..=40).map(|i| format!("line {i}\n")).collect();
        let MessagePart::Tool {
            output: Some(tail),
            output_bytes,
            output_truncated,
            output_lines,
            exit_code,
            ..
        } = fold_command(&full, Some(1))
        else {
            panic!("no tail");
        };
        assert_eq!(output_lines, Some(40), "the whole output's lines");
        assert_eq!(tail.lines().count(), COMMAND_TAIL_MAX_LINES);
        assert!(tail.starts_with("line 31\n") && tail.ends_with("line 40"));
        assert!(output_truncated);
        assert_eq!(output_bytes, Some(full.len() as u64));
        assert_eq!(exit_code, Some(1));
    }

    #[test]
    fn a_command_keeps_at_most_one_kilobyte() {
        // 8 lines of 299 bytes: under 10 lines, over 1 KB. (Words: a long
        // letters-and-digits run would read as a token and be redacted.)
        let full: String = (1..=8)
            .map(|i| format!("{i:03} {}\n", "ab ".repeat(98)))
            .collect();
        let MessagePart::Tool {
            output: Some(tail),
            output_truncated,
            ..
        } = fold_command(&full, Some(0))
        else {
            panic!("no tail");
        };
        assert!(tail.len() <= COMMAND_TAIL_MAX_BYTES, "{}", tail.len());
        assert!(tail.starts_with("006 ab"), "starts on a whole line");
        assert!(tail.ends_with("ab ab"));
        assert!(output_truncated);
        // One huge line keeps its end, on a char boundary.
        let line = "é".repeat(5000);
        let Some(cut) = command_output_tail("make", &line) else {
            panic!("no tail");
        };
        assert!(cut.truncated && cut.text.len() <= COMMAND_TAIL_MAX_BYTES);
        assert!(cut.text.chars().all(|c| c == 'é'));
    }

    #[test]
    fn a_short_command_output_rides_whole_and_unflagged() {
        let part = fold_command("\x1b[32mok\x1b[0m\n\n", Some(0));
        assert!(
            matches!(
                &part,
                MessagePart::Tool { output: Some(o), output_truncated: false, output_lines: None, exit_code: Some(0), output_bytes: Some(13), .. }
                    if o == "ok"
            ),
            "{part:?}"
        );
        // Blank output: nothing kept but the exit code.
        assert!(matches!(
            fold_command("  \n", Some(3)),
            MessagePart::Tool {
                output: None,
                output_bytes: None,
                exit_code: Some(3),
                ..
            }
        ));
    }

    #[test]
    fn a_command_tail_is_redacted() {
        let out = "deploying\nexport GITHUB_TOKEN=ghp_0123456789abcdefghijABCDEFGHIJ\n\
                   Authorization: Bearer abc.def.ghi\n\
                   cloning https://user:pw@example.com/repo.git?token=zzz\ndone\n";
        let MessagePart::Tool {
            output: Some(tail), ..
        } = fold_command(out, Some(0))
        else {
            panic!("no tail");
        };
        for secret in ["ghp_0123", "abc.def.ghi", "user:pw", "token=zzz"] {
            assert!(!tail.contains(secret), "{secret} survived: {tail}");
        }
        assert!(tail.starts_with("deploying\n") && tail.ends_with("done"));
    }

    #[test]
    fn a_command_that_prints_secrets_keeps_none_of_its_output() {
        for command in [
            "env",
            "printenv",
            "cat .env.local",
            "/usr/bin/zsh -lc 'cat ~/.aws/credentials'",
            "gh auth token",
            "gcloud auth print-access-token",
        ] {
            let mut parts = Vec::new();
            for event in [
                AgentEvent::ToolCall {
                    id: "c".into(),
                    call: ToolCall::Exec {
                        command: command.into(),
                    },
                },
                AgentEvent::ToolResult {
                    id: "c".into(),
                    is_error: false,
                    output: Some("hunter2-but-unlabelled\n".into()),
                    diff: None,
                    exit_code: Some(0),
                },
            ] {
                fold_event_into_parts(&mut parts, &event);
            }
            let MessagePart::Tool {
                output: Some(output),
                exit_code,
                output_truncated,
                ..
            } = &parts[0]
            else {
                panic!("{command}: {parts:?}");
            };
            assert_eq!(
                output,
                zeron_proto::redact::SECRET_OUTPUT_WITHHELD,
                "{command}"
            );
            assert_eq!(*exit_code, Some(0));
            assert!(!output_truncated);
        }
    }

    #[test]
    fn a_command_tail_drops_keys_and_environment_values() {
        let out = "AWS_SECRET_ACCESS_KEY=wJalrXUtnFEMI\n\
                   export DB_PASS=letmein\n\
                   -----BEGIN OPENSSH PRIVATE KEY-----\n\
                   b3BlbnNzaC1rZXktdjEAAAAA\n\
                   -----END OPENSSH PRIVATE KEY-----\n\
                   posting to https://hooks.slack.com/services/T000/B000/XXXX\n\
                   test result: ok. 5 passed; 0 failed\n";
        let MessagePart::Tool {
            output: Some(tail), ..
        } = fold_command(out, Some(0))
        else {
            panic!("no tail");
        };
        for secret in [
            "wJalrXUtnFEMI",
            "letmein",
            "b3BlbnNzaC1rZXkt",
            "T000",
            "XXXX",
        ] {
            assert!(!tail.contains(secret), "{secret} survived: {tail}");
        }
        assert!(tail.contains("AWS_SECRET_ACCESS_KEY=[redacted]"), "{tail}");
        assert!(tail.contains("[private key redacted]"), "{tail}");
        assert!(
            tail.contains("https://hooks.slack.com/[redacted]"),
            "{tail}"
        );
        assert!(
            tail.ends_with("test result: ok. 5 passed; 0 failed"),
            "{tail}"
        );
    }

    #[test]
    fn a_non_command_keeps_no_output_or_exit_code() {
        let mut parts = Vec::new();
        for event in [
            AgentEvent::ToolCall {
                id: "r".into(),
                call: ToolCall::ReadFile {
                    path: "/w/.env".into(),
                },
            },
            AgentEvent::ToolResult {
                id: "r".into(),
                is_error: false,
                output: Some("API_KEY=hunter2".into()),
                diff: None,
                exit_code: Some(0),
            },
        ] {
            fold_event_into_parts(&mut parts, &event);
        }
        assert!(matches!(
            &parts[0],
            MessagePart::Tool {
                output: None,
                output_bytes: None,
                exit_code: None,
                output_truncated: false,
                resolved: true,
                ..
            }
        ));
    }

    #[test]
    fn fold_strips_output_to_summary_and_diff_to_stats() {
        let mut parts = Vec::new();
        fold_event_into_parts(
            &mut parts,
            &AgentEvent::ToolCall {
                id: "t".into(),
                call: ToolCall::ReadFile {
                    path: "/w/test.log".into(),
                },
            },
        );
        let full = "running 42 tests\n".repeat(300); // ~5KB, was 4KB inline pre-strip
        fold_event_into_parts(
            &mut parts,
            &AgentEvent::ToolResult {
                id: "t".into(),
                is_error: false,
                output: Some(full.clone()),
                diff: Some(ToolDiff {
                    path: "/w/a.rs".into(),
                    old_text: Some("a\n".into()),
                    new_text: "b\n".into(),
                }),
                exit_code: None,
            },
        );
        match &parts[0] {
            MessagePart::Tool {
                output,
                output_bytes,
                diff,
                diff_stats,
                ..
            } => {
                // One-liner chips: a non-command's output never enters the
                // doc (journal-only); diff text neither — stats survive.
                assert_eq!(output.as_deref(), None);
                assert_eq!(*output_bytes, None);
                assert!(diff.is_none(), "inline diff text must not enter the doc");
                let stats = diff_stats.as_ref().unwrap();
                assert_eq!(stats.len(), 1);
                assert_eq!((stats[0].additions, stats[0].deletions), (1, 1));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn sidecar_refs_stamp_once_and_only_where_content_exists() {
        let mut parts = Vec::new();
        fold_event_into_parts(
            &mut parts,
            &AgentEvent::ToolCall {
                id: "t1".into(),
                call: ToolCall::WriteFile {
                    path: "/w/a".into(),
                    content: None,
                },
            },
        );
        // Unresolved: no refs yet.
        apply_sidecar_refs("chat-9", &mut parts);
        assert!(matches!(
            &parts[0],
            MessagePart::Tool {
                output_ref: None,
                diff_ref: None,
                ..
            }
        ));
        fold_event_into_parts(
            &mut parts,
            &AgentEvent::ToolResult {
                id: "t1".into(),
                is_error: false,
                output: Some("hello".into()),
                diff: Some(ToolDiff {
                    path: "/w/a".into(),
                    old_text: None,
                    new_text: "x\n".into(),
                }),
                exit_code: None,
            },
        );
        apply_sidecar_refs("chat-9", &mut parts);
        apply_sidecar_refs("chat-9", &mut parts); // idempotent
        match &parts[0] {
            MessagePart::Tool {
                output_ref,
                diff_ref,
                ..
            } => {
                // One-liner fold: outputs never reach the doc, so there is
                // no output content to key even after resolution; diff
                // STATS exist, so the diff ref still stamps.
                assert_eq!(output_ref.as_deref(), None);
                assert_eq!(diff_ref.as_deref(), Some("chat-9/t1.diff"));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn subagent_events_refresh_the_spawn_chip_in_place() {
        use zeron_proto::DoneStatus;
        let mut parts = Vec::new();
        fold_event_into_parts(
            &mut parts,
            &AgentEvent::ToolCall {
                id: "toolu_sub".into(),
                call: ToolCall::Unknown {
                    name: "Agent".into(),
                    input: None,
                },
            },
        );
        // Tagged traffic marks the chip running — and ONLY that: the tail
        // stays untouched (per-delta chip rewrites polluted the parent doc).
        fold_event_into_parts(
            &mut parts,
            &AgentEvent::Subagent {
                parent_tool_use_id: "toolu_sub".into(),
                event: Box::new(AgentEvent::TextDelta {
                    text: "scanning\nfound 3 issues".into(),
                }),
            },
        );
        match &parts[0] {
            MessagePart::Tool {
                subagent_status,
                subagent_tail,
                ..
            } => {
                assert_eq!(*subagent_status, Some(SubagentStatus::Running));
                assert_eq!(*subagent_tail, None);
            }
            other => panic!("{other:?}"),
        }
        // A tagged Done is terminal; later traffic must not regress it.
        fold_event_into_parts(
            &mut parts,
            &AgentEvent::Subagent {
                parent_tool_use_id: "toolu_sub".into(),
                event: Box::new(AgentEvent::Done {
                    status: DoneStatus::Completed,
                    result: None,
                    error: None,
                    session_id: None,
                }),
            },
        );
        fold_event_into_parts(
            &mut parts,
            &AgentEvent::Subagent {
                parent_tool_use_id: "toolu_sub".into(),
                event: Box::new(AgentEvent::TextDelta {
                    text: "late flush".into(),
                }),
            },
        );
        match &parts[0] {
            MessagePart::Tool {
                subagent_status, ..
            } => assert_eq!(*subagent_status, Some(SubagentStatus::Done)),
            other => panic!("{other:?}"),
        }
        // A confirmed follow-up can reopen the same chip without user text.
        fold_event_into_parts(
            &mut parts,
            &AgentEvent::Subagent {
                parent_tool_use_id: "toolu_sub".into(),
                event: Box::new(AgentEvent::Steered {
                    assistant_message_id: None,
                    next_assistant_message_id: None,
                }),
            },
        );
        assert!(matches!(
            &parts[0],
            MessagePart::Tool {
                subagent_status: Some(SubagentStatus::Running),
                ..
            }
        ));
        // Content never leaked into the parent parts.
        assert_eq!(parts.len(), 1);
    }

    #[test]
    fn subagent_events_never_decorate_a_non_spawn_chip() {
        // Mis-keyed tagged traffic (claude's background shells settled
        // through the subagent subtype, 2026-08-20) must not stamp lifecycle
        // onto an ordinary tool chip — the genus gate is the CALL.
        use zeron_proto::DoneStatus;
        let mut parts = Vec::new();
        fold_event_into_parts(
            &mut parts,
            &AgentEvent::ToolCall {
                id: "toolu_bash".into(),
                call: ToolCall::Exec {
                    command: "git clone …".into(),
                },
            },
        );
        fold_event_into_parts(
            &mut parts,
            &AgentEvent::Subagent {
                parent_tool_use_id: "toolu_bash".into(),
                event: Box::new(AgentEvent::Done {
                    status: DoneStatus::Completed,
                    result: None,
                    error: None,
                    session_id: None,
                }),
            },
        );
        match &parts[0] {
            MessagePart::Tool {
                subagent_status, ..
            } => assert_eq!(*subagent_status, None),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn sidecar_payload_carries_full_texts() {
        assert_eq!(
            sidecar_payload(&AgentEvent::TextDelta { text: "x".into() }),
            None
        );
        assert_eq!(
            sidecar_payload(&AgentEvent::ToolResult {
                id: "t".into(),
                is_error: false,
                output: Some("   \n".into()),
                diff: None,
                exit_code: None,
            }),
            None,
            "blank output uploads nothing"
        );
        let payload = sidecar_payload(&AgentEvent::ToolResult {
            id: "t".into(),
            is_error: true,
            output: Some("full output".into()),
            diff: None,
            exit_code: None,
        })
        .unwrap();
        assert_eq!(payload.part_id, "t");
        assert_eq!(payload.output.as_deref(), Some("full output"));
    }
}
