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

/// One subagent a session's agent spawned.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct SubagentRow {
    /// The spawn's tool part id.
    pub id: String,
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
        Self {
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
    /// The subagents this session's agent spawned, in transcript order
    /// (pull after `SessionChanged`).
    pub fn subagents(&self) -> Vec<SubagentRow> {
        zc::subagents::subagents(&self.inner.snapshot().transcript_messages())
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
