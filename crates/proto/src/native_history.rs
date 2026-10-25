//! The conversation an agent's own CLI stores and reads back on its next
//! turn (Claude Code's session file, Codex's thread), as opposed to the
//! transcript Zeron mirrors from the run. Experimental transcript editing
//! reads and rewrites this history: an edit is only real once the agent
//! itself sees it.
use serde::{Deserialize, Serialize};

/// A chat's native history, newest edit applied.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeHistory {
    pub chat_id: String,
    /// Harness id (`claude-code`, `codex`).
    pub harness: String,
    /// The agent session the chat resumes next.
    pub session_id: String,
    /// Opaque fingerprint of the stored session. Edits name the revision they
    /// were made against and are refused once the session moved past it.
    pub revision: String,
    pub items: Vec<NativeHistoryItem>,
    /// The agent compacted earlier history: it now reads a summary in place
    /// of everything before it, so only what follows is listed.
    #[serde(default)]
    pub compacted: bool,
    /// An earlier edit can be undone (the pre-edit session is kept).
    #[serde(default)]
    pub can_undo: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NativeRole {
    User,
    Assistant,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeHistoryItem {
    /// Stable within one revision; edits reference it.
    pub id: String,
    pub role: NativeRole,
    /// Index of the user prompt this item answers (0-based), so a view can
    /// group items into turns.
    pub turn: u32,
    pub kind: NativeItemKind,
    /// The text can be replaced.
    pub editable: bool,
    /// The item can be dropped from the history.
    pub removable: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum NativeItemKind {
    Text {
        text: String,
    },
    /// Reasoning the model returned. Providers seal it: Claude signs its
    /// thinking (readable when stored, but any change breaks the
    /// signature), Codex stores it encrypted. `text` is whatever readable
    /// form is stored (thinking text or a summary), absent when only the
    /// sealed form exists.
    #[serde(rename_all = "camelCase")]
    Reasoning {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        seal: ReasoningSeal,
    },
    #[serde(rename_all = "camelCase")]
    ToolCall {
        name: String,
        /// One-line rendering of the call's input.
        summary: String,
    },
    #[serde(rename_all = "camelCase")]
    ToolResult {
        /// Leading part of the output.
        summary: String,
        is_error: bool,
    },
    /// The summary the agent wrote when it compacted earlier history.
    CompactionSummary {
        text: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ReasoningSeal {
    /// Readable, but signed: it can be removed, not changed.
    Signed,
    /// Only the provider can read it: it can be removed, not changed.
    Encrypted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase")]
pub enum NativeEdit {
    Replace { id: String, text: String },
    Remove { id: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetNativeHistoryParams {
    pub chat_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyNativeEditsParams {
    pub chat_id: String,
    pub revision: String,
    pub edits: Vec<NativeEdit>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UndoNativeEditParams {
    pub chat_id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_shapes() {
        let item = NativeHistoryItem {
            id: "a:1".into(),
            role: NativeRole::Assistant,
            turn: 0,
            kind: NativeItemKind::Reasoning {
                text: None,
                seal: ReasoningSeal::Encrypted,
            },
            editable: false,
            removable: true,
        };
        assert_eq!(
            serde_json::to_value(&item).unwrap(),
            serde_json::json!({
                "id": "a:1", "role": "assistant", "turn": 0,
                "kind": { "kind": "reasoning", "seal": "encrypted" },
                "editable": false, "removable": true,
            })
        );
        let edit: NativeEdit =
            serde_json::from_value(serde_json::json!({ "op": "replace", "id": "x", "text": "y" }))
                .unwrap();
        assert_eq!(
            edit,
            NativeEdit::Replace {
                id: "x".into(),
                text: "y".into()
            }
        );
    }
}
