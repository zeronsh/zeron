//! User-facing proposals carried by native tools, without a new wire variant.

use crate::ToolCall;
use serde::Serialize;
use serde_json::{Value, json};

pub const MAX_PLAN_BYTES: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct ProposedPlan<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<&'a str>,
    #[serde(rename = "plan")]
    pub body: &'a str,
}

impl<'a> ProposedPlan<'a> {
    pub fn new(title: Option<&'a str>, body: &'a str) -> Option<Self> {
        let plan = Self {
            title: title.filter(|title| !title.trim().is_empty()),
            body,
        };
        if body.trim().is_empty()
            || body.len() > MAX_PLAN_BYTES
            || plan.title.is_some_and(|title| title.len() > MAX_PLAN_BYTES)
            || serde_json::to_vec(&plan).ok()?.len() > MAX_PLAN_BYTES
        {
            return None;
        }
        Some(plan)
    }

    /// Only these display fields may cross the tool-input privacy boundary.
    pub fn display_input(self) -> Value {
        let mut input = json!({"plan": self.body});
        if let Some(title) = self.title {
            input["title"] = title.into();
        }
        input
    }

    pub fn markdown(self) -> String {
        match self.title {
            Some(title) => format!(
                "## {}\n\n{}",
                title.lines().collect::<Vec<_>>().join(" "),
                self.body
            ),
            None => self.body.into(),
        }
    }
}

pub fn proposed_plan(call: &ToolCall) -> Option<ProposedPlan<'_>> {
    let ToolCall::Unknown {
        name,
        input: Some(input),
    } = call
    else {
        return None;
    };
    if name != "ExitSpecMode" {
        return None;
    }
    let title = match input.get("title") {
        None | Some(Value::Null) => None,
        Some(value) => Some(value.as_str()?),
    };
    ProposedPlan::new(title, input.get("plan")?.as_str()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_plans_keep_the_complete_markdown_and_only_display_fields() {
        let body = "1. Inspect\n\n```rust\n  work();\n```\n";
        let call = ToolCall::Unknown {
            name: "ExitSpecMode".into(),
            input: Some(json!({"title":"A plan", "plan":body, "private":"not for display"})),
        };
        let plan = proposed_plan(&call).unwrap();
        assert_eq!(plan.body, body);
        assert_eq!(plan.markdown(), format!("## A plan\n\n{body}"));
        assert_eq!(plan.display_input(), json!({"title":"A plan", "plan":body}));
        let wire = serde_json::to_value(&call).unwrap();
        assert_eq!(wire["kind"], "unknown");
        assert_eq!(serde_json::from_value::<ToolCall>(wire).unwrap(), call);
    }

    #[test]
    fn other_tools_and_invalid_or_oversized_proposals_are_not_plans() {
        for (name, input) in [
            ("Other", json!({"plan":"hello"})),
            ("ExitPlanMode", json!({"plan":"hello"})),
            ("ExitSpecMode", json!({})),
            ("ExitSpecMode", json!({"plan":" \n"})),
            ("ExitSpecMode", json!({"plan":7})),
            ("ExitSpecMode", json!({"plan":"hello", "title":7})),
            ("ExitSpecMode", json!({"plan":"x".repeat(MAX_PLAN_BYTES)})),
            (
                "ExitSpecMode",
                json!({"plan":"\u{0000}".repeat(MAX_PLAN_BYTES / 2)}),
            ),
        ] {
            assert!(
                proposed_plan(&ToolCall::Unknown {
                    name: name.into(),
                    input: Some(input)
                })
                .is_none()
            );
        }
        assert!(
            proposed_plan(&ToolCall::Mcp {
                server: "other".into(),
                tool: "ExitSpecMode".into(),
                input: Some(json!({"plan":"hello"}))
            })
            .is_none()
        );
        assert_eq!(
            ProposedPlan::new(None, "  keep whitespace\n")
                .unwrap()
                .markdown(),
            "  keep whitespace\n"
        );
    }
}
