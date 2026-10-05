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

/// Environment variable carrying the ask's submission secret, set beside
/// [`ASK_ID_ENV`]. The engine keeps it in memory only (never in the synced
/// doc) and accepts `SubmitAskResult` only with it, so knowing a chat and ask
/// id is not enough to answer for the verifier.
pub const ASK_TOKEN_ENV: &str = "ZERON_ASK_TOKEN";

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
