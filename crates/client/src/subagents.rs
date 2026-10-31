//! Subagents: the children an agent spawns (Claude's `Task`, Codex's
//! collaborators, …). Each spawn is a tool part on the parent's transcript —
//! `Agent[: <description>]`, stamped with the child doc it writes to — and the
//! child's own transcript is a separate doc on the parent's host
//! (`{chatId}--sub--{toolUseId}`), uploaded as a blob once it finishes.
//!
//! [`subagents`] lists a transcript's spawns; [`Client::watch_subagent`]
//! follows one child's transcript: the frozen blob for a finished child
//! (falling back to the doc — the upload is best effort), the live doc
//! (`WatchDocMessages`) for a running one.

use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use tokio_util::sync::CancellationToken;
use zeron_doc::parts::SubagentStatus;
use zeron_doc::{MessagePart, SessionMessageEntry, TranscriptUpdate, apply_transcript_frame};
use zeron_proto::ToolCall;
use zeron_rpc::methods;

use crate::client::{Client, ClientInner};
use crate::error::Result;

/// A spawn's lifecycle as the parent's transcript tells it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubagentState {
    Running,
    Done,
    Failed,
}

/// One subagent spawn on a transcript.
#[derive(Debug, Clone, PartialEq)]
pub struct SubagentInfo {
    /// The spawn's tool part id (stable across updates).
    pub id: String,
    /// The child's transcript doc, once the host has bound one.
    pub doc_id: Option<String>,
    /// What the agent asked it to do ("Explore the veil call sites").
    pub title: String,
    /// The model the spawn named, when it named one.
    pub model: Option<String>,
    /// The kind of agent, when the spawn named one (`Explore`, `reviewer`…).
    pub kind: Option<String>,
    pub state: SubagentState,
    /// Its latest output line while it works.
    pub tail: Option<String>,
    /// When the spawning message started (epoch ms).
    pub started_at_ms: i64,
    /// The parent entry holding the spawn.
    pub entry_id: String,
}

/// The spawns on `entries`, in transcript order.
pub fn subagents(entries: &[Arc<SessionMessageEntry>]) -> Vec<SubagentInfo> {
    let mut out = Vec::new();
    for entry in entries {
        for part in &entry.parts {
            let MessagePart::Tool {
                id,
                call,
                is_error,
                resolved,
                subagent_ref,
                subagent_status,
                subagent_tail,
                ..
            } = part
            else {
                continue;
            };
            if !call.is_subagent_spawn() {
                continue;
            }
            // The child's lifecycle is its own: the spawn call may resolve
            // while the child still runs (desktop `subagent_chip`).
            let state = match (subagent_ref, subagent_status) {
                (Some(_), Some(SubagentStatus::Running)) => SubagentState::Running,
                (Some(_), Some(SubagentStatus::Failed)) => SubagentState::Failed,
                (Some(_), Some(SubagentStatus::Done)) => SubagentState::Done,
                _ if *is_error => SubagentState::Failed,
                _ if !*resolved => SubagentState::Running,
                _ => SubagentState::Done,
            };
            let (_, detail) = zeron_proto::view::tool_chip_content(call);
            out.push(SubagentInfo {
                id: id.clone(),
                doc_id: subagent_ref.clone(),
                title: if detail.trim().is_empty() {
                    "Subagent".into()
                } else {
                    detail
                },
                model: call.subagent_model().map(str::to_owned),
                kind: spawn_kind(call),
                state,
                tail: subagent_tail.clone().filter(|t| !t.trim().is_empty()),
                started_at_ms: entry.created_at,
                entry_id: entry.id.clone(),
            });
        }
    }
    out
}

fn spawn_kind(call: &ToolCall) -> Option<String> {
    let input = match call {
        ToolCall::Unknown { input, .. } | ToolCall::Mcp { input, .. } => input.as_ref()?,
        _ => return None,
    };
    ["subagent_type", "agent_type", "agentType"]
        .iter()
        .find_map(|key| input.get(key).and_then(serde_json::Value::as_str))
        .map(str::trim)
        .filter(|kind| !kind.is_empty() && !kind.eq_ignore_ascii_case("general-purpose"))
        .map(str::to_owned)
}

/// The chat whose host holds `doc_id` (a nested child's doc is named after
/// the root chat too).
pub(crate) fn source_chat<'a>(doc_id: &'a str, chat_id: &'a str) -> &'a str {
    doc_id
        .split_once("--sub--")
        .map_or(chat_id, |(source, _)| source)
}

/// Receives a child's transcript, whole, every time it changes.
pub trait SubagentSink: Send + Sync {
    fn transcript(&self, entries: Vec<Arc<SessionMessageEntry>>);
    /// The host can't be reached right now (the stream retries).
    fn unavailable(&self, message: String);
}

/// Following one child; dropping it stops.
pub struct SubagentWatch {
    cancel: CancellationToken,
}

impl Drop for SubagentWatch {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

impl Client {
    /// Follow `doc_id`, spawned in `chat_id` (a chat, or a parent child's doc
    /// for a nested spawn). `frozen`: the child is finished.
    pub fn watch_subagent(
        &self,
        chat_id: &str,
        doc_id: &str,
        frozen: bool,
        sink: Arc<dyn SubagentSink>,
    ) -> Result<SubagentWatch> {
        let source = source_chat(doc_id, chat_id).to_owned();
        let device_id = self.chat_host(&source)?;
        let cancel = CancellationToken::new();
        crate::runtime::shared().spawn(follow(
            Arc::downgrade(&self.inner),
            device_id,
            source,
            doc_id.to_owned(),
            frozen,
            sink,
            cancel.clone(),
        ));
        Ok(SubagentWatch { cancel })
    }
}

async fn follow(
    weak: std::sync::Weak<ClientInner>,
    device_id: String,
    source: String,
    doc_id: String,
    frozen: bool,
    sink: Arc<dyn SubagentSink>,
    cancel: CancellationToken,
) {
    if frozen {
        let Some(inner) = weak.upgrade() else { return };
        let fetch = inner.host_rpc(
            &device_id,
            methods::FETCH_TOOL_BLOB,
            json!({ "blobRef": format!("{source}/{doc_id}") }),
        );
        let reply = tokio::select! {
            _ = cancel.cancelled() => return,
            reply = fetch => reply,
        };
        drop(inner);
        let entries = reply.ok().and_then(|value| {
            serde_json::from_str::<Vec<SessionMessageEntry>>(value.get("text")?.as_str()?).ok()
        });
        if let Some(entries) = entries {
            sink.transcript(entries.into_iter().map(Arc::new).collect());
            return;
        }
    }
    let mut backoff = Duration::from_millis(500);
    loop {
        let Some(inner) = weak.upgrade() else { return };
        let subscribe = inner.host_subscribe(
            &device_id,
            methods::WATCH_DOC_MESSAGES,
            json!({ "chatId": doc_id }),
        );
        let stream = tokio::select! {
            _ = cancel.cancelled() => return,
            stream = subscribe => stream,
        };
        drop(inner);
        match stream {
            Ok(mut stream) => {
                let mut rows: Vec<SessionMessageEntry> = Vec::new();
                // Entries that didn't change keep their `Arc`, so the layout
                // engine re-measures only what moved.
                let mut shared: Vec<Arc<SessionMessageEntry>> = Vec::new();
                let mut desynced = false;
                loop {
                    let item = tokio::select! {
                        _ = cancel.cancelled() => return,
                        item = stream.recv() => item,
                    };
                    let Some(item) = item else { break };
                    let applied = serde_json::from_value::<TranscriptUpdate>(item)
                        .ok()
                        .is_some_and(|update| {
                            apply_transcript_frame(&mut rows, update.frame).is_ok()
                        });
                    if !applied {
                        desynced = true;
                        break;
                    }
                    backoff = Duration::from_millis(500);
                    shared = rows
                        .iter()
                        .map(|entry| {
                            shared
                                .iter()
                                .find(|old| old.id == entry.id && ***old == *entry)
                                .cloned()
                                .unwrap_or_else(|| Arc::new(entry.clone()))
                        })
                        .collect();
                    sink.transcript(shared.clone());
                }
                // A frame that didn't apply means a full replay next: not
                // every half second (the desktop waits 2s too).
                if desynced {
                    backoff = backoff.max(Duration::from_secs(2));
                }
            }
            Err(err) => sink.unavailable(err.to_string()),
        }
        tokio::select! {
            _ = cancel.cancelled() => return,
            _ = tokio::time::sleep(backoff) => {}
        }
        backoff = (backoff * 2).min(Duration::from_secs(5));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zeron_doc::{MessageRole, MessageStatus};

    fn spawn(id: &str, name: &str, status: Option<SubagentStatus>, resolved: bool) -> MessagePart {
        MessagePart::Tool {
            id: id.into(),
            call: ToolCall::Unknown {
                name: name.into(),
                input: Some(json!({ "model": "claude-haiku-5", "subagent_type": "Explore" })),
            },
            is_error: false,
            resolved,
            output: None,
            diff: None,
            output_ref: None,
            output_bytes: None,
            diff_ref: None,
            diff_stats: None,
            subagent_ref: status.map(|_| format!("chat--sub--{id}")),
            subagent_status: status,
            subagent_tail: Some("Reading veil.rs".into()),
        }
    }

    #[test]
    fn lists_spawns_with_their_own_lifecycle() {
        let entry = Arc::new(SessionMessageEntry {
            id: "m1".into(),
            role: MessageRole::Assistant,
            parts: vec![
                spawn(
                    "a",
                    "Agent: Explore call sites",
                    Some(SubagentStatus::Running),
                    true,
                ),
                spawn("b", "Agent: Review tests", Some(SubagentStatus::Done), true),
                spawn("c", "Agent", None, false),
                MessagePart::Text {
                    id: "t".into(),
                    text: "hi".into(),
                },
            ],
            created_at: 7,
            device_id: "mac".into(),
            status: Some(MessageStatus::Complete),
            continuation_of: None,
            duration_ms: None,
        });
        let list = subagents(&[entry]);
        assert_eq!(list.len(), 3);
        assert_eq!(list[0].title, "Explore call sites");
        // Resolved spawn call, child still running: running.
        assert_eq!(list[0].state, SubagentState::Running);
        assert_eq!(list[0].doc_id.as_deref(), Some("chat--sub--a"));
        assert_eq!(list[0].model.as_deref(), Some("claude-haiku-5"));
        assert_eq!(list[0].kind.as_deref(), Some("Explore"));
        assert_eq!(list[1].state, SubagentState::Done);
        assert_eq!(list[2].title, "Subagent");
        assert_eq!(list[2].state, SubagentState::Running);
        assert_eq!(list[2].started_at_ms, 7);
    }

    #[test]
    fn nested_docs_resolve_to_the_root_chat() {
        assert_eq!(source_chat("chat-1--sub--t9", "chat-1--sub--t1"), "chat-1");
        assert_eq!(source_chat("plain", "chat-2"), "chat-2");
    }
}
