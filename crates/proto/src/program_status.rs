//! Program Status Protocol (OSC 7501) data, shared by the engine and viewports.
//! Terminal reports describe program activity; they never complete a native
//! harness turn or authorize answering a permission request.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProgramState {
    Idle,
    Working,
    Done,
    Blocked,
    Error,
}

impl ProgramState {
    /// All records participate: an idle/done root cannot hide a live child.
    pub fn attention_rank(self) -> u8 {
        match self {
            Self::Blocked => 0,
            Self::Working => 1,
            Self::Error => 2,
            Self::Done => 3,
            Self::Idle => 4,
        }
    }

    pub fn active(self) -> bool {
        matches!(self, Self::Working | Self::Blocked)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProgramInputKind {
    Permission,
    Question,
    Auth,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgramStatus {
    /// Empty for the terminal's root record; slash-separated for child records.
    pub id: String,
    pub state: ProgramState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<ProgramInputKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// The most urgent program record across a chat's managed terminals. Full
/// record sets stay engine-local and bounded; only this small projection syncs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalActivity {
    pub terminal_id: String,
    pub report: ProgramStatus,
    /// Actual report transition, not the engine's liveness heartbeat. This is
    /// compared with the chat's seen marker to preserve unread completions.
    pub updated_at: DateTime<Utc>,
}
