//! The ask profile: what `zeron mcp` serves inside a child ask's chat.
//!
//! Besides the read tools it carries one run-scoped tool, `submit_result`,
//! whose input schema *is* the ask's result schema (fetched from the engine).
//! A rejected submission comes back as an error result listing the violations,
//! which is how the model learns what to repair.

use serde_json::{Value, json};

use crate::tools::{ToolDef, Tools};

pub(crate) const SUBMIT_RESULT: &str = "submit_result";

/// The read-only tools an ask's chat keeps. `read_chat` is how a verifier
/// reads the transcript of the chat it judges.
pub(crate) const ASK_READ_TOOLS: [&str; 3] = ["whoami", "get_chat", "read_chat"];

const DESCRIPTION: &str = "Deliver your final structured result for this request. Call it exactly \
once, when you are done: its arguments ARE the result and must match this tool's input schema. \
If the call is rejected, the error lists what to fix; fix it and call again.";

/// The advertised `submit_result`.
#[derive(Debug, Clone)]
pub(crate) struct AskTool {
    schema: Value,
    /// The ask's schema is not an object, so the result travels under
    /// `result` (MCP tool inputs must be objects).
    wrapped: bool,
}

impl AskTool {
    pub(crate) fn from_spec(spec: &zeron_proto::AskSpecInfo) -> Self {
        let mut schema = spec.result_schema.clone();
        if !spec.result_description.is_empty()
            && let Some(object) = schema.as_object_mut()
        {
            object
                .entry("description")
                .or_insert_with(|| Value::String(spec.result_description.clone()));
        }
        let is_object = schema.get("type").and_then(Value::as_str) == Some("object");
        if is_object {
            Self {
                schema,
                wrapped: false,
            }
        } else {
            Self {
                schema: json!({
                    "type": "object",
                    "properties": { "result": schema },
                    "required": ["result"],
                    "additionalProperties": false,
                }),
                wrapped: true,
            }
        }
    }

    /// Used when the engine cannot be reached to read the spec: the tool is
    /// still offered (the engine validates the submission regardless).
    pub(crate) fn permissive() -> Self {
        Self {
            schema: json!({ "type": "object", "additionalProperties": true }),
            wrapped: false,
        }
    }

    pub(crate) fn def(&self) -> ToolDef {
        ToolDef {
            name: SUBMIT_RESULT,
            description: DESCRIPTION,
            input_schema: self.schema.clone(),
        }
    }

    fn unwrap(&self, args: Value) -> Value {
        if self.wrapped {
            args.get("result").cloned().unwrap_or(Value::Null)
        } else {
            args
        }
    }
}

impl Tools {
    pub(crate) async fn ask_tool(&self) -> &AskTool {
        self.ask_tool
            .get_or_init(|| async {
                match self.zeron.ask_spec().await {
                    Ok(spec) => AskTool::from_spec(&spec),
                    Err(error) => {
                        tracing::warn!(%error, "could not read the ask spec; offering a permissive submit_result");
                        AskTool::permissive()
                    }
                }
            })
            .await
    }

    /// `Ok` when the engine accepted the result; `Err` (an error result the
    /// model reads) with the violations when it did not.
    pub(crate) async fn submit_result(&self, args: Value) -> Result<Value, String> {
        let tool = self.ask_tool().await;
        let reply = self
            .zeron
            .submit_ask_result(tool.unwrap(args))
            .await
            .map_err(|e| e.to_string())?;
        if reply.accepted {
            Ok(json!({ "accepted": true, "message": reply.message }))
        } else {
            Err(reply.message)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(schema: Value) -> zeron_proto::AskSpecInfo {
        zeron_proto::AskSpecInfo {
            ask_id: "a".into(),
            result_schema: schema,
            result_description: "the verdict".into(),
        }
    }

    #[test]
    fn object_schemas_are_advertised_as_is_with_the_description() {
        let tool = AskTool::from_spec(&spec(json!({
            "type": "object", "properties": {"passed": {"type": "boolean"}}
        })));
        let def = tool.def();
        assert_eq!(def.name, "submit_result");
        assert_eq!(def.input_schema["properties"]["passed"]["type"], "boolean");
        assert_eq!(def.input_schema["description"], "the verdict");
        assert_eq!(
            tool.unwrap(json!({"passed": true})),
            json!({"passed": true})
        );
    }

    #[test]
    fn non_object_schemas_travel_under_result() {
        let tool = AskTool::from_spec(&spec(json!({"type": "array", "items": {"type": "string"}})));
        let def = tool.def();
        assert_eq!(def.input_schema["type"], "object");
        assert_eq!(def.input_schema["properties"]["result"]["type"], "array");
        assert_eq!(tool.unwrap(json!({"result": ["a"]})), json!(["a"]));
    }
}
