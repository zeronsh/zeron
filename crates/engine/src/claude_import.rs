//! Import Claude Code's own transcripts into chats, so past CLI work opens here.
//!
//! The chat id is the Claude session id: re-importing is a no-op, and the row's
//! `harnessSessionId` then makes the next message resume that very session.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use zeron_doc::{MessagePart, MessageRole, MessageStatus, SessionMessageEntry};
use zeron_harness::claude::transcript::{self, DiscoveredSession, ImportedTranscript};
use zeron_proto::{AgentEvent, ChatConfig, HarnessId, SandboxLevel};

use crate::EngineError;
use crate::doc_host::DocHost;
use crate::workspace_host::WorkspaceHost;

/// Cap on imported messages per session; a runaway transcript never blocks the UI.
const MAX_ENTRIES: usize = 20_000;

/// One listed transcript plus whether this workspace already holds it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportableSession {
    #[serde(flatten)]
    pub session: DiscoveredSession,
    pub already_imported: bool,
}

/// Progress for one import run; the last item is always `Summary`.
#[derive(Debug, Serialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
pub enum ClaudeImportEvent {
    /// Emitted once, before any transcript is read.
    Start { sessions: usize },
    /// One session, emitted before it is read.
    Session {
        index: usize,
        total: usize,
        session_id: String,
        title: String,
    },
    /// Terminal summary, and the RPC stream's final item.
    Summary {
        imported: usize,
        skipped: usize,
        messages: usize,
        errors: Vec<String>,
    },
}

#[derive(Clone)]
pub struct ClaudeImporter {
    inner: Arc<Inner>,
}

struct Inner {
    config_dir: PathBuf,
    device_id: String,
    workspace: WorkspaceHost,
    doc_host: DocHost,
}

impl ClaudeImporter {
    pub fn new(
        config_dir: PathBuf,
        device_id: &str,
        workspace: WorkspaceHost,
        doc_host: DocHost,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                config_dir,
                device_id: device_id.to_string(),
                workspace,
                doc_host,
            }),
        }
    }

    /// Every transcript Claude Code has on this device, newest first.
    pub fn list(&self) -> Vec<ImportableSession> {
        transcript::discover(&self.inner.config_dir)
            .into_iter()
            .map(|session| ImportableSession {
                already_imported: self.chat_exists(&session.session_id),
                session,
            })
            .collect()
    }

    fn chat_exists(&self, chat_id: &str) -> bool {
        matches!(self.inner.workspace.chat(chat_id), Ok(Some(_)))
    }

    /// Import the named sessions, or every un-imported one when the list is empty.
    pub fn run(
        &self,
        session_ids: &[String],
        mut emit: impl FnMut(ClaudeImportEvent),
    ) -> Result<(), EngineError> {
        let available = transcript::discover(&self.inner.config_dir);
        let selected: Vec<DiscoveredSession> = if session_ids.is_empty() {
            available
                .into_iter()
                .filter(|s| !self.chat_exists(&s.session_id))
                .collect()
        } else {
            available
                .into_iter()
                .filter(|s| session_ids.contains(&s.session_id))
                .collect()
        };

        emit(ClaudeImportEvent::Start {
            sessions: selected.len(),
        });
        let total = selected.len();
        let mut imported = 0usize;
        let mut skipped = 0usize;
        let mut messages = 0usize;
        let mut errors = Vec::new();
        for (index, session) in selected.iter().enumerate() {
            emit(ClaudeImportEvent::Session {
                index,
                total,
                session_id: session.session_id.clone(),
                title: session.title.clone(),
            });
            if self.chat_exists(&session.session_id) {
                skipped += 1;
                continue;
            }
            match self.import_one(session) {
                Ok(written) => {
                    imported += 1;
                    messages += written;
                }
                Err(err) => errors.push(format!("{}: {err}", session.title)),
            }
        }
        self.inner.doc_host.flush_all();
        emit(ClaudeImportEvent::Summary {
            imported,
            skipped,
            messages,
            errors,
        });
        Ok(())
    }

    /// Import one transcript; returns how many messages it wrote.
    fn import_one(&self, session: &DiscoveredSession) -> Result<usize, EngineError> {
        let read = transcript::read_transcript(&session.path)?;
        let entries = fold_entries(&read, &self.inner.device_id);
        if entries.is_empty() {
            return Err(EngineError::Other("transcript holds no messages".into()));
        }

        let chat_id = read.session_id.clone();
        let cwd = if read.cwd.is_empty() {
            session.cwd.clone()
        } else {
            read.cwd.clone()
        };
        // Claim rather than create: the same path that gives a live run its project.
        self.inner.workspace.claim_chat(&chat_id, Some(&cwd))?;
        self.inner.workspace.set_chat_config(
            &chat_id,
            &ChatConfig {
                harness: HarnessId::ClaudeCode,
                model: None,
                reasoning: None,
                model_options: Default::default(),
                sandbox: SandboxLevel::WorkspaceWrite,
            },
        )?;

        let handle = self.inner.doc_host.open(&chat_id)?;
        // Deleting a chat tombstones its row but keeps the doc, so a re-import
        // opens a doc that may already hold these entries. Entry ids are
        // deterministic, which makes skipping them enough to avoid doubling.
        let existing: std::collections::HashSet<String> = handle
            .doc()
            .read_entries()?
            .into_iter()
            .map(|entry| entry.id)
            .collect();
        let mut written = 0usize;
        for entry in entries.iter().filter(|e| !existing.contains(&e.id)) {
            handle.doc().push_message(entry)?;
            written += 1;
        }

        if let Some(title) = read.title.as_deref().filter(|t| !t.trim().is_empty()) {
            self.inner.workspace.rename_chat(&chat_id, title)?;
        } else {
            self.inner.workspace.rename_chat(&chat_id, &session.title)?;
        }
        if let Some(preview) = entries.iter().rev().find_map(last_text) {
            self.inner.workspace.note_message(&chat_id, &preview);
        }
        // The resume handshake: the next message continues this Claude session.
        self.inner
            .workspace
            .set_chat_harness_session(&chat_id, &read.session_id, &cwd);
        if let Some(branch) = read.git_branch.as_deref() {
            self.inner.workspace.set_chat_branch(&chat_id, branch)?;
        }
        // Last, because note_message stamps the preview with the wall clock and
        // would otherwise date an imported chat to the moment it was imported.
        let last_ms = read
            .last_ms
            .or_else(|| entries.last().map(|e| e.created_at));
        self.inner
            .workspace
            .set_chat_activity(&chat_id, last_ms, read.started_ms)?;
        Ok(written)
    }
}

/// The text of an entry, for the sidebar preview.
fn last_text(entry: &SessionMessageEntry) -> Option<String> {
    entry.parts.iter().rev().find_map(|part| match part {
        MessagePart::Text { text, .. } if !text.trim().is_empty() => Some(text.clone()),
        _ => None,
    })
}

/// Fold transcript events into doc entries: one per user turn, one per reply.
fn fold_entries(read: &ImportedTranscript, device_id: &str) -> Vec<SessionMessageEntry> {
    let mut entries: Vec<SessionMessageEntry> = Vec::new();
    let mut parts: Vec<MessagePart> = Vec::new();
    let mut started_at = read.started_ms.unwrap_or_default();
    let mut next_id = {
        let session = read.session_id.clone();
        let mut counter = 0usize;
        move || {
            counter += 1;
            format!("{session}:{counter}")
        }
    };

    for item in &read.events {
        if entries.len() >= MAX_ENTRIES {
            break;
        }
        match &item.event {
            AgentEvent::UserMessage { text } => {
                if !parts.is_empty() {
                    entries.push(assistant_entry(
                        next_id(),
                        std::mem::take(&mut parts),
                        started_at,
                        device_id,
                    ));
                }
                entries.push(SessionMessageEntry {
                    id: next_id(),
                    role: MessageRole::User,
                    parts: vec![MessagePart::Text {
                        id: "t0".into(),
                        text: text.clone(),
                    }],
                    created_at: item.at_ms,
                    device_id: device_id.to_string(),
                    status: Some(MessageStatus::Complete),
                    continuation_of: None,
                });
                started_at = item.at_ms;
            }
            AgentEvent::TextDelta { text } => {
                started_at = first_of(started_at, item.at_ms, &parts);
                append_text(&mut parts, text);
            }
            AgentEvent::ReasoningDelta { text } => {
                started_at = first_of(started_at, item.at_ms, &parts);
                append_reasoning(&mut parts, text);
            }
            AgentEvent::ToolCall { id, call } => {
                started_at = first_of(started_at, item.at_ms, &parts);
                parts.push(MessagePart::Tool {
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
                });
            }
            AgentEvent::ToolResult { id, is_error, .. } => {
                resolve_tool(&mut parts, id, *is_error);
            }
            _ => {}
        }
    }
    if !parts.is_empty() && entries.len() < MAX_ENTRIES {
        entries.push(assistant_entry(next_id(), parts, started_at, device_id));
    }
    entries
}

/// The timestamp an assistant entry starts at: its first part's, not its last.
fn first_of(current: i64, at_ms: i64, parts: &[MessagePart]) -> i64 {
    if parts.is_empty() { at_ms } else { current }
}

fn assistant_entry(
    id: String,
    parts: Vec<MessagePart>,
    created_at: i64,
    device_id: &str,
) -> SessionMessageEntry {
    SessionMessageEntry {
        id,
        role: MessageRole::Assistant,
        parts,
        created_at,
        device_id: device_id.to_string(),
        status: Some(MessageStatus::Complete),
        continuation_of: None,
    }
}

/// Merge into the trailing text part, so one reply is one bubble.
fn append_text(parts: &mut Vec<MessagePart>, text: &str) {
    if let Some(MessagePart::Text { text: body, .. }) = parts.last_mut() {
        body.push_str("\n\n");
        body.push_str(text);
        return;
    }
    parts.push(MessagePart::Text {
        id: format!("t{}", parts.len()),
        text: text.to_string(),
    });
}

fn append_reasoning(parts: &mut Vec<MessagePart>, text: &str) {
    if let Some(MessagePart::Reasoning { text: body, .. }) = parts.last_mut() {
        body.push_str("\n\n");
        body.push_str(text);
        return;
    }
    parts.push(MessagePart::Reasoning {
        id: format!("r{}", parts.len()),
        text: text.to_string(),
    });
}

/// Mark the call this result answers, matching the live fold's pairing by id.
fn resolve_tool(parts: &mut [MessagePart], tool_id: &str, errored: bool) {
    for part in parts.iter_mut().rev() {
        if let MessagePart::Tool {
            id,
            resolved,
            is_error,
            ..
        } = part
            && id == tool_id
        {
            *resolved = true;
            *is_error = errored;
            return;
        }
    }
}

/// Resolve the Claude config dir the same way account switching does.
pub fn default_config_dir() -> PathBuf {
    std::env::var_os("CLAUDE_CONFIG_DIR")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir().join(".claude"))
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(".").to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeron_harness::claude::transcript::TranscriptEvent;
    use zeron_proto::ToolCall;

    fn transcript(events: Vec<(i64, AgentEvent)>) -> ImportedTranscript {
        ImportedTranscript {
            session_id: "sess-1".into(),
            cwd: "/w/proj".into(),
            title: Some("A title".into()),
            git_branch: Some("main".into()),
            started_ms: Some(1_000),
            last_ms: events.last().map(|(ms, _)| *ms),
            events: events
                .into_iter()
                .map(|(at_ms, event)| TranscriptEvent { at_ms, event })
                .collect(),
            malformed_lines: 0,
        }
    }

    fn call(command: &str) -> ToolCall {
        ToolCall::Exec {
            command: command.into(),
        }
    }

    #[test]
    fn a_turn_becomes_a_user_entry_then_one_assistant_entry() {
        let read = transcript(vec![
            (
                1_000,
                AgentEvent::UserMessage {
                    text: "fix it".into(),
                },
            ),
            (
                2_000,
                AgentEvent::ReasoningDelta {
                    text: "thinking".into(),
                },
            ),
            (
                2_100,
                AgentEvent::TextDelta {
                    text: "On it.".into(),
                },
            ),
            (
                2_200,
                AgentEvent::ToolCall {
                    id: "toolu_1".into(),
                    call: call("cargo test"),
                },
            ),
            (
                2_300,
                AgentEvent::ToolResult {
                    id: "toolu_1".into(),
                    is_error: false,
                    output: None,
                    diff: None,
                },
            ),
            (
                2_400,
                AgentEvent::TextDelta {
                    text: "Done.".into(),
                },
            ),
        ]);
        let entries = fold_entries(&read, "device-1");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].role, MessageRole::User);
        assert_eq!(entries[0].created_at, 1_000);
        assert_eq!(entries[1].role, MessageRole::Assistant);
        assert_eq!(
            entries[1].created_at, 2_000,
            "the reply is stamped at its first part"
        );
        let kinds: Vec<&str> = entries[1]
            .parts
            .iter()
            .map(|p| match p {
                MessagePart::Text { .. } => "text",
                MessagePart::Reasoning { .. } => "reasoning",
                MessagePart::Tool { .. } => "tool",
                _ => "other",
            })
            .collect();
        assert_eq!(kinds, ["reasoning", "text", "tool", "text"]);
        assert!(
            matches!(&entries[1].parts[2], MessagePart::Tool { resolved, is_error, .. } if *resolved && !*is_error)
        );
        let ids: Vec<&str> = entries.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, ["sess-1:1", "sess-1:2"], "entry ids are deterministic");
    }

    #[test]
    fn an_unanswered_call_stays_unresolved() {
        let read = transcript(vec![
            (1_000, AgentEvent::UserMessage { text: "go".into() }),
            (
                1_100,
                AgentEvent::ToolCall {
                    id: "toolu_9".into(),
                    call: call("sleep 100"),
                },
            ),
        ]);
        let entries = fold_entries(&read, "device-1");
        assert!(matches!(
            &entries[1].parts[0],
            MessagePart::Tool {
                resolved: false,
                ..
            }
        ));
    }

    #[test]
    fn a_failed_tool_keeps_its_error_flag() {
        let read = transcript(vec![
            (1_000, AgentEvent::UserMessage { text: "go".into() }),
            (
                1_100,
                AgentEvent::ToolCall {
                    id: "toolu_9".into(),
                    call: call("false"),
                },
            ),
            (
                1_200,
                AgentEvent::ToolResult {
                    id: "toolu_9".into(),
                    is_error: true,
                    output: None,
                    diff: None,
                },
            ),
        ]);
        let entries = fold_entries(&read, "device-1");
        assert!(matches!(
            &entries[1].parts[0],
            MessagePart::Tool {
                resolved: true,
                is_error: true,
                ..
            }
        ));
    }

    #[test]
    fn consecutive_text_merges_into_one_bubble() {
        let read = transcript(vec![
            (1_000, AgentEvent::UserMessage { text: "go".into() }),
            (
                1_100,
                AgentEvent::TextDelta {
                    text: "first".into(),
                },
            ),
            (
                1_200,
                AgentEvent::TextDelta {
                    text: "second".into(),
                },
            ),
        ]);
        let entries = fold_entries(&read, "device-1");
        assert_eq!(entries[1].parts.len(), 1);
        assert!(
            matches!(&entries[1].parts[0], MessagePart::Text { text, .. } if text == "first\n\nsecond")
        );
    }

    #[test]
    fn a_reply_with_no_prompt_before_it_still_imports() {
        let read = transcript(vec![(
            1_500,
            AgentEvent::TextDelta {
                text: "resumed reply".into(),
            },
        )]);
        let entries = fold_entries(&read, "device-1");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].role, MessageRole::Assistant);
    }
}
