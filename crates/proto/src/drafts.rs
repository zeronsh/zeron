//! Prompt drafts are independent of conversations. Content lives outside the
//! registry; only immutable revision references and ordering travel in rows.
use crate::ChatConfig;
use serde::{Deserialize, Serialize};

pub const DRAFTS_CAPABILITY: &str = "prompt-drafts-v1";
pub const MAX_DRAFT_CONTENT_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_DRAFT_ASSET_BYTES: usize = 32 * 1024 * 1024;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftTarget {
    pub device_id: String,
    pub space_id: Option<String>,
    pub project_name: Option<String>,
    pub config: Option<ChatConfig>,
    pub branch: Option<String>,
    #[serde(default)]
    pub new_worktree: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftAttachment {
    pub id: String,
    pub name: String,
    /// SHA-256 of the raw bytes; immutable, shared across content revisions.
    pub blob: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub appshot: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftContent {
    pub prompt: String,
    pub target: DraftTarget,
    #[serde(default)]
    pub attachments: Vec<DraftAttachment>,
}
impl DraftContent {
    pub fn has_content(&self) -> bool {
        !self.prompt.trim().is_empty() || !self.attachments.is_empty()
    }
    pub fn preview(&self) -> String {
        let line = self
            .prompt
            .lines()
            .find(|s| !s.trim().is_empty())
            .unwrap_or("");
        if line.is_empty() {
            format!("{} attachments", self.attachments.len())
        } else {
            line.chars().take(180).collect()
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftAsset {
    pub blob: String,
    pub data: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveDraft {
    /// Persist the active canvas without listing it until navigation.
    #[serde(default)]
    pub deferred: bool,
    pub id: String,
    pub revision: String,
    pub base_revision: Option<String>,
    pub created_at: i64,
    pub content: DraftContent,
    #[serde(default)]
    pub assets: Vec<DraftAsset>,
}
impl SaveDraft {
    /// A visibility promotion must survive an ACK of the same content revision.
    pub fn publication_key(&self) -> String {
        if self.deferred {
            format!("editing-{}", self.revision)
        } else {
            self.revision.clone()
        }
    }
}
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftBundle {
    pub content: DraftContent,
    pub assets: Vec<DraftAsset>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptDraft {
    pub id: String,
    pub revision: String,
    pub base_revision: Option<String>,
    pub created_at: i64,
    pub preview: String,
    pub target: DraftTarget,
    pub order_key: String,
    #[serde(default)]
    pub conflict: bool,
    #[serde(default)]
    pub pending: bool,
}
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftsState {
    #[serde(default)]
    pub revision: u64,
    pub drafts: Vec<PromptDraft>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "camelCase")]
pub enum DraftChange {
    #[serde(rename_all = "camelCase")]
    Move {
        id: String,
        after: Option<String>,
        before: Option<String>,
    },
    Discard {
        id: String,
    },
    Consume {
        id: String,
        revision: String,
    },
}
pub fn valid_draft_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 128 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// Every draft asset RPC stays well below the IPC WebSocket frame limit.
pub const DRAFT_CHUNK_BYTES: usize = 1024 * 1024;
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DraftAssetChunk {
    pub blob: String,
    pub data: String,
    pub index: usize,
    pub total_bytes: usize,
}
