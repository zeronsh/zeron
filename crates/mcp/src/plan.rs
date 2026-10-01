//! Plan mode for agents without a native plan tool (docs/plan-mode.md).
//!
//! Inside a Plan-mode chat the server carries one extra tool, `submit_plan`,
//! and its instructions tell the agent to use it. The engine puts the plan to
//! the user (the same question panel every device and the phones answer) and
//! the call returns once they've decided. A rejected plan comes back as an
//! error result the model reads and revises from.

use serde_json::{Value, json};

use crate::tools::{ToolDef, Tools};

pub(crate) const SUBMIT_PLAN: &str = "submit_plan";
/// Set by the engine on the server it hands a Plan-mode run.
pub(crate) const PLAN_MODE_ENV: &str = "ZERON_PLAN_MODE";

const DESCRIPTION: &str = "Present your plan to the user for approval. You are in plan mode: you \
can read and investigate, but edits and commands are refused until the user approves a plan. Call \
this once your plan is ready, with the whole plan as markdown (what you will change, in which \
files, in what order, and how you will check it). If the user wants changes the call returns an \
error saying what to change; revise the plan and call again.";

pub(crate) const INSTRUCTIONS: &str = "\n\n\
This chat is in plan mode. Investigate with read-only tools, then present your plan with \
`submit_plan` instead of changing anything. After the user approves it, say so and end your \
turn; the work continues in a new turn.";

pub(crate) fn def() -> ToolDef {
    ToolDef {
        name: SUBMIT_PLAN,
        description: DESCRIPTION,
        input_schema: json!({
            "type": "object",
            "properties": {
                "plan": {
                    "type": "string",
                    "description": "The plan, as markdown."
                }
            },
            "required": ["plan"],
            "additionalProperties": false
        }),
    }
}

impl Tools {
    /// `Ok` when the user approved the plan; `Err` (an error result the model
    /// reads) with what they want changed when they did not.
    pub(crate) async fn submit_plan(&self, args: Value) -> Result<Value, String> {
        let plan = args
            .get("plan")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|plan| !plan.is_empty())
            .ok_or("`plan` must be the plan's text")?;
        let reply = self
            .zeron
            .submit_plan(plan)
            .await
            .map_err(|e| e.to_string())?;
        if reply.approved {
            Ok(json!({ "approved": true, "mode": reply.mode, "message": reply.message }))
        } else {
            Err(reply.message)
        }
    }
}
