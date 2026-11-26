//! A session's companions over FFI: its subagents (read off the transcript)
//! and side chats (child chats: start a fresh one, or fork this one).
//! Terminals live in `crate::terminal`.

use std::sync::Arc;

use zeron_client as zc;

use super::CoreClient;
use super::session::SessionHandle;
use super::types::CoreResult;

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum SubagentState {
    Running,
    Done,
    Failed,
}

/// One subagent of a session: one its agent spawned in its own runtime, or a
/// chat it delegated to (`chat_id`), on any device and provider, opened,
/// stopped and answered as any chat is.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct SubagentRow {
    /// The spawn's tool part id; a delegated chat's id.
    pub id: String,
    /// A delegated chat: its id (open it with `open_session`; it stops and
    /// answers through its own handle).
    pub chat_id: Option<String>,
    /// A delegated chat's device, when it isn't the parent's.
    pub device_name: Option<String>,
    /// A delegated chat's wire harness (`codex`), for its mark.
    pub harness: Option<String>,
    /// A delegated chat asks the person something (its own handle's
    /// `open_input`).
    pub asks: bool,
    /// The child's transcript doc (`None` until the host binds one).
    pub doc_id: Option<String>,
    pub title: String,
    pub model: Option<String>,
    pub kind: Option<String>,
    pub state: SubagentState,
    /// Its latest output line while it works.
    pub tail: Option<String>,
    pub started_at_ms: i64,
}

impl From<zc::SubagentInfo> for SubagentRow {
    fn from(s: zc::SubagentInfo) -> Self {
        let (chat_id, device_name, harness, asks) = match s.origin {
            zc::SubagentOrigin::Native => (None, None, None, false),
            zc::SubagentOrigin::Delegated { chat_id, device_name, harness, asks, .. } => (Some(chat_id), device_name, harness, asks),
        };
        Self {
            chat_id,
            device_name,
            harness,
            asks,
            id: s.id,
            doc_id: s.doc_id,
            title: s.title,
            model: s.model,
            kind: s.kind,
            state: match s.state {
                zc::SubagentState::Running => SubagentState::Running,
                zc::SubagentState::Done => SubagentState::Done,
                zc::SubagentState::Failed => SubagentState::Failed,
            },
            tail: s.tail,
            started_at_ms: s.started_at_ms,
        }
    }
}

#[uniffi::export]
impl SessionHandle {
    /// This session's subagents: those its agent spawned, in transcript
    /// order, then the chats it delegated to, oldest first (pull after
    /// `SessionChanged` or `WorkspaceChanged`).
    pub fn subagents(&self) -> Vec<SubagentRow> {
        self.inner
            .subagents()
            .into_iter()
            .map(Into::into)
            .collect()
    }
}

#[uniffi::export]
impl CoreClient {
    /// A fresh side chat under `parent_id` (its host, folder and agent).
    /// Returns its id.
    pub fn create_side_chat(&self, parent_id: String) -> CoreResult<String> {
        Ok(self.client.create_side_chat(&parent_id)?)
    }

    /// Fork `source_id` through its latest completed reply into a side chat
    /// (the source's host copies the conversation). Returns the fork's id.
    pub async fn fork_side_chat(&self, source_id: String) -> CoreResult<String> {
        let client = self.client.clone();
        zc::runtime::run(async move { client.fork_side_chat(&source_id).await })
            .await
            .map_err(Into::into)
    }
}

/// Keeps a subagent transcript followed (see `TranscriptView::attach_subagent`).
pub(crate) struct SubagentFollow(#[allow(dead_code)] pub(crate) zc::SubagentWatch);

pub(crate) fn follow_subagent(
    client: &CoreClient,
    chat_id: &str,
    doc_id: &str,
    frozen: bool,
    sink: Arc<dyn zc::SubagentSink>,
) -> Option<SubagentFollow> {
    client
        .client
        .watch_subagent(chat_id, doc_id, frozen, sink)
        .ok()
        .map(SubagentFollow)
}
