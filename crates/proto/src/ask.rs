//! Wire shapes of the child-ask primitive (`docs/goal-mode.md`, "Child ask").
//!
//! A *child ask* runs a short-lived hidden child chat and obtains a typed
//! result from it through a run-scoped MCP tool, `submit_result`. The MCP
//! server inside the child's harness fetches the ask's spec from the engine
//! ([`AskSpecInfo`]) to advertise the tool, and posts what the model submits
//! back ([`AskSubmitReply`] carries the engine's verdict on it).

use serde::{Deserialize, Serialize};

/// Environment variable naming the ask a `zeron mcp` process serves. Set only
/// on the MCP server injected into an ask's child chat; it switches the server
/// to the restricted ask toolset (`read_chat`, `get_chat`, `whoami`,
/// `submit_result`).
pub const ASK_ID_ENV: &str = "ZERON_ASK_ID";

/// One schema violation, located by a JSON-Pointer-style path
/// (`/items/2/name`; the empty path is the root).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SchemaViolation {
    pub path: String,
    pub message: String,
}

impl std::fmt::Display for SchemaViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.path.is_empty() {
            write!(f, "(root): {}", self.message)
        } else {
            write!(f, "{}: {}", self.path, self.message)
        }
    }
}

/// What the MCP server needs to advertise `submit_result` for one ask.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AskSpecInfo {
    pub ask_id: String,
    /// JSON Schema the submitted result must satisfy.
    pub result_schema: serde_json::Value,
    /// One line on what the result is, shown in the tool description.
    #[serde(default)]
    pub result_description: String,
    /// The ask's child may call `escalate` (workflow actors only).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub escalation: bool,
}

/// The engine's answer to a `submit_result` call. It is returned to the model
/// as the tool result, so `message` is written to be read by it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AskSubmitReply {
    pub accepted: bool,
    pub message: String,
    #[serde(default)]
    pub violations: Vec<SchemaViolation>,
    /// Repair rounds left after this submission (0 once exhausted).
    #[serde(default)]
    pub repairs_left: u32,
}

/// The actor-side `escalate` tool call (workflow actors only): ask the parent
/// agent a question mid-task. The first call carries `question`; while it is
/// unanswered the same call is repeated with `question_id` to keep waiting.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EscalateRequest {
    pub ask_id: String,
    #[serde(default)]
    pub question: Option<String>,
    #[serde(default)]
    pub context: Option<String>,
    #[serde(default)]
    pub question_id: Option<String>,
    /// How long this call may wait for an answer before reporting `pending`.
    #[serde(default)]
    pub max_wait_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EscalateStatus {
    Answered,
    /// Still waiting for the parent agent; call again with `question_id`.
    Pending,
    /// Not allowed (budget spent, empty question, not an escalating ask).
    Refused,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EscalateReply {
    pub status: EscalateStatus,
    #[serde(default)]
    pub question_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<String>,
    /// Read by the model as the tool result.
    pub message: String,
    /// Escalations this ask may still raise.
    #[serde(default)]
    pub left: u32,
}
