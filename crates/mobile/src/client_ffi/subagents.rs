//! A chat's subagents over FFI: the desktop Explorer's Subagents section as
//! data. Derivation, order and grouping live in `zeron_client::subagents`;
//! paging and which groups are open stay with the platform (view state).

use std::sync::Arc;

use zeron_client as zc;

use super::session::SessionHandle;
use super::types::CoreResult;
use super::CoreClient;

/// Where a subagent's lifecycle stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum SubagentState {
    Running,
    /// Spawned, no status stamped yet.
    Pending,
    Completed,
    Failed,
}

impl From<zc::SubagentState> for SubagentState {
    fn from(s: zc::SubagentState) -> Self {
        match s {
            zc::SubagentState::Running => Self::Running,
            zc::SubagentState::Pending => Self::Pending,
            zc::SubagentState::Completed => Self::Completed,
            zc::SubagentState::Failed => Self::Failed,
        }
    }
}

/// One subagent of a chat.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SubagentView {
    /// Its transcript: `CoreClient.open_subagent(chat, doc_id)`.
    pub doc_id: String,
    pub spawn_id: String,
    /// The bare task ("verify the marker pipeline").
    pub title: String,
    /// The spawn chip's detail line.
    pub description: String,
    /// `subagent_type` ("Explore", "general-purpose"…), when the spawn said.
    pub agent_type: Option<String>,
    /// The model the spawn named; `None` = the chat's model.
    pub model: Option<String>,
    pub state: SubagentState,
    pub started_at_ms: i64,
    /// Latest turn that spawned or steered it (the desktop's "updated").
    pub updated_at_ms: i64,
    /// What it reported back to the parent, else its last line.
    pub summary: Option<String>,
    pub spawn_failed: bool,
}

impl From<&zc::SubagentItem> for SubagentView {
    fn from(i: &zc::SubagentItem) -> Self {
        Self {
            doc_id: i.doc_id.clone(),
            spawn_id: i.spawn_id.clone(),
            title: i.title.clone(),
            description: i.description.clone(),
            agent_type: i.agent_type.clone(),
            model: i.model.clone(),
            state: i.state.into(),
            started_at_ms: i.started_at_ms,
            updated_at_ms: i.updated_at_ms,
            summary: i.summary.clone(),
            spawn_failed: i.spawn_failed,
        }
    }
}

/// The lists the Subagents panel draws, in order.
#[derive(Debug, Clone, Default, PartialEq, Eq, uniffi::Record)]
pub struct SubagentGroups {
    /// Running (longest-running first), then not yet stamped.
    pub active: Vec<SubagentView>,
    /// Newest first.
    pub completed: Vec<SubagentView>,
    /// Newest first.
    pub failed: Vec<SubagentView>,
    /// Subagents streaming right now.
    pub running: u32,
    /// The chat's harness: subagents run inside it.
    pub harness: Option<String>,
    pub harness_label: Option<String>,
    /// The chat's model (a spawn without its own inherits it).
    pub model_label: Option<String>,
}

impl SubagentGroups {
    fn from_groups(g: &zc::SubagentGroups, row: Option<&zc::SessionRow>) -> Self {
        let views = |rows: &[zc::SubagentItem]| rows.iter().map(SubagentView::from).collect();
        Self {
            active: views(&g.active),
            completed: views(&g.completed),
            failed: views(&g.failed),
            running: u32::try_from(g.running()).unwrap_or(u32::MAX),
            harness: row.and_then(|r| r.harness.clone()),
            harness_label: row.and_then(|r| r.harness_label.clone()),
            model_label: row.and_then(|r| r.model_label.clone()),
        }
    }
}

#[uniffi::export]
impl CoreClient {
    /// The subagents of `chat_id`, read from its open session's transcript
    /// (empty until the session is open and hydrated).
    pub fn subagents(&self, chat_id: String) -> SubagentGroups {
        let Some(session) = self.client.session(&chat_id) else {
            return SubagentGroups::default();
        };
        let groups = zc::SubagentGroups::from_entries(&session.snapshot().transcript_messages());
        let workspace = self.client.workspace();
        SubagentGroups::from_groups(&groups, workspace.session(&chat_id).map(|r| &**r))
    }

    /// Open one subagent's transcript read-only (`doc_id` from a
    /// [`SubagentView`] or a spawn chip). Attach a `TranscriptView` with the
    /// same `doc_id`.
    pub fn open_subagent(
        &self,
        parent_chat_id: String,
        doc_id: String,
    ) -> CoreResult<Arc<SessionHandle>> {
        Ok(SessionHandle::new(
            self.client.open_subagent(&parent_chat_id, &doc_id)?,
        ))
    }
}

/// A running count as pills and badges draw it: exact to 99, then "99+".
#[uniffi::export]
pub fn running_count_label(count: u32) -> String {
    zc::subagents::running_count_label(count)
}

/// Rows a finished list shows before "Show more", and how many each press
/// adds.
#[uniffi::export]
pub fn subagent_page_rows() -> u32 {
    zc::subagents::SUBAGENT_PAGE_ROWS as u32
}

/// The link a spawn card in the transcript carries (`LinkHit.url`), so a tap
/// opens the subagent: `zeron-subagent:{docId}`.
pub(crate) const SUBAGENT_LINK_SCHEME: &str = "zeron-subagent:";

/// The subagent doc a transcript link opens, if it is a spawn card's link.
#[uniffi::export]
pub fn subagent_link_doc(url: String) -> Option<String> {
    url.strip_prefix(SUBAGENT_LINK_SCHEME)
        .filter(|doc| zc::subagents::is_subagent_doc(doc))
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_name_subagent_docs_only() {
        assert_eq!(
            subagent_link_doc("zeron-subagent:chat--sub--a".into()).as_deref(),
            Some("chat--sub--a")
        );
        assert_eq!(subagent_link_doc("zeron-subagent:chat".into()), None);
        assert_eq!(subagent_link_doc("https://x.dev".into()), None);
    }

    #[test]
    fn counts_cap_like_the_desktop_pill() {
        assert_eq!(running_count_label(7), "7");
        assert_eq!(running_count_label(120), "99+");
        assert_eq!(subagent_page_rows(), 10);
    }
}
