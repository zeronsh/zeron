//! Claude CLI stream-json wire frames (stdout JSONL + stdin lines).
//!
//! Tolerant by construction: every field defaults, unknown frame types map to
//! [`Frame::Other`], so a newer CLI never breaks parsing — we only read the
//! fields the normalizer needs (spec: docs/research/harness.md).

use serde::Deserialize;
use serde_json::{Value, json};

/// One parsed stdout line.
#[derive(Debug)]
pub(crate) enum Frame {
    System(SystemFrame),
    StreamEvent(StreamEventFrame),
    Assistant(MessageFrame),
    User(MessageFrame),
    RateLimit(RateLimitFrame),
    Result(ResultFrame),
    ControlRequest(ControlRequestFrame),
    /// The CLI's answer to one of our control requests.
    ControlResponse(ControlResponseFrame),
    /// Where the CLI has taken one of our stdin user messages.
    CommandLifecycle(CommandLifecycleFrame),
    /// control_cancel_request / anything unknown.
    Other,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct ControlResponseFrame {
    #[serde(default)]
    pub response: ControlResponseBody,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct ControlResponseBody {
    #[serde(default)]
    pub request_id: String,
}

/// `{"type":"command_lifecycle","command_uuid":…,"state":…}` — emitted for
/// every stdin user message carrying a `uuid` (verified live against CLI
/// 2.1.286): `queued` when read, `started` when a turn takes it up (a `next`
/// steer starts mid-turn, at the step boundary it folds into), then
/// `completed` or `cancelled` (its turn was aborted by a later `now` steer
/// or an interrupt). Background-task wake turns carry no command at all.
#[derive(Debug, Default, Deserialize)]
pub(crate) struct CommandLifecycleFrame {
    #[serde(default)]
    pub command_uuid: String,
    #[serde(default)]
    pub state: String,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct SystemFrame {
    #[serde(default)]
    pub subtype: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub tools: Vec<String>,
    #[serde(default)]
    pub cwd: String,
    #[serde(default)]
    pub session_id: String,
    /// `task_notification`: the spawning Agent tool's id, or the latest
    /// SendMessage id after a resume. Stop notifications may omit it.
    #[serde(default, alias = "toolUseId")]
    pub tool_use_id: Option<String>,
    /// `task_notification` terminal status (`completed`/`failed`/`killed`…).
    #[serde(default)]
    pub status: Option<String>,
    /// `task_started` / `task_notification`: stable agent/task id
    /// (`SendMessage`'s `to:` address), used to retain spawn identity.
    #[serde(default, alias = "taskId")]
    pub task_id: Option<String>,
    /// `task_started`: present only for AGENT tasks (a subagent spawning),
    /// absent on subagent-owned background shell tasks.
    #[serde(default)]
    pub subagent_type: Option<String>,
    /// `background_tasks_changed`: every background task the session holds
    /// now (agents and shells) — the authoritative set, not a delta.
    #[serde(default)]
    pub tasks: Option<Vec<Value>>,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct StreamEventFrame {
    #[serde(default)]
    pub parent_tool_use_id: Option<String>,
    #[serde(default)]
    pub event: StreamEventBody,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct StreamEventBody {
    #[serde(rename = "type", default)]
    pub kind: String,
    #[serde(default)]
    pub delta: Delta,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct Delta {
    #[serde(rename = "type", default)]
    pub kind: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub thinking: String,
}

/// An `assistant` or `user` frame (an Anthropic API message envelope).
#[derive(Debug, Default, Deserialize)]
pub(crate) struct MessageFrame {
    #[serde(default)]
    pub uuid: Option<String>,
    #[serde(default)]
    pub parent_tool_use_id: Option<String>,
    #[serde(default)]
    pub message: MessageBody,
    /// Terse assistant-level error code (`rate_limit`, `billing_error`, …).
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct MessageBody {
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub usage: Option<Value>,
    /// Either a plain string or an array of content blocks.
    #[serde(default)]
    pub content: Value,
}

impl MessageBody {
    pub fn blocks(&self) -> impl Iterator<Item = ContentBlock> + '_ {
        self.content
            .as_array()
            .map(|a| a.as_slice())
            .unwrap_or_default()
            .iter()
            .filter_map(|b| serde_json::from_value(b.clone()).ok())
    }
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct ContentBlock {
    #[serde(rename = "type", default)]
    pub kind: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub input: Value,
    #[serde(default)]
    pub tool_use_id: String,
    #[serde(default)]
    pub is_error: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct RateLimitFrame {
    #[serde(default)]
    pub rate_limit_info: RateLimitInfo,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct RateLimitInfo {
    #[serde(default)]
    pub status: String,
    #[serde(rename = "rateLimitType", default)]
    pub rate_limit_type: Option<String>,
    /// `allowed`/`allowed_warning` when the account may spend provisioned
    /// overage past a rejected window.
    #[serde(rename = "overageStatus", default)]
    pub overage_status: Option<String>,
    #[serde(rename = "isUsingOverage", default)]
    pub is_using_overage: bool,
    #[serde(rename = "overageInUse", default)]
    pub overage_in_use: bool,
}

impl RateLimitInfo {
    /// A rejected window that actually blocks the turn: an account spending
    /// provisioned overage keeps running despite the reject.
    pub fn blocks(&self) -> bool {
        self.status == "rejected"
            && !matches!(
                self.overage_status.as_deref(),
                Some("allowed" | "allowed_warning")
            )
            && !self.is_using_overage
            && !self.overage_in_use
    }
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct ResultFrame {
    #[serde(default, rename = "modelUsage")]
    pub model_usage: std::collections::BTreeMap<String, Value>,
    #[serde(default)]
    pub subtype: String,
    #[serde(default)]
    pub result: Option<String>,
    #[serde(default)]
    pub errors: Vec<Value>,
    #[serde(default)]
    pub usage: UsageBody,
    #[serde(default)]
    pub session_id: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct UsageBody {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
}

/// A CLI→client control request (`can_use_tool` is the one we act on).
#[derive(Debug, Default, Deserialize)]
pub(crate) struct ControlRequestFrame {
    #[serde(default)]
    pub request_id: String,
    #[serde(default)]
    pub request: ControlRequestBody,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct ControlRequestBody {
    #[serde(default)]
    pub subtype: String,
    #[serde(default)]
    pub tool_name: String,
    #[serde(default)]
    pub input: Value,
}

/// Parse one stdout JSONL line. `Err` = not JSON; unknown types = `Other`.
pub(crate) fn parse_frame(line: &str) -> Result<Frame, serde_json::Error> {
    let value: Value = serde_json::from_str(line)?;
    let kind = value.get("type").and_then(Value::as_str).unwrap_or("");
    let frame = match kind {
        "system" => Frame::System(serde_json::from_value(value)?),
        "stream_event" => Frame::StreamEvent(serde_json::from_value(value)?),
        "assistant" => Frame::Assistant(serde_json::from_value(value)?),
        "user" => Frame::User(serde_json::from_value(value)?),
        "rate_limit_event" => Frame::RateLimit(serde_json::from_value(value)?),
        "result" => Frame::Result(serde_json::from_value(value)?),
        "control_request" => Frame::ControlRequest(serde_json::from_value(value)?),
        "control_response" => Frame::ControlResponse(serde_json::from_value(value)?),
        "command_lifecycle" => Frame::CommandLifecycle(serde_json::from_value(value)?),
        _ => Frame::Other,
    };
    Ok(frame)
}

/// A stdin user turn: `{"type":"user","message":{...},"parent_tool_use_id":null}`.
/// Steering = another such line mid-run (consumed at a step boundary).
#[cfg(test)]
pub(crate) fn user_message_line(text: &str) -> String {
    json!({
        "type": "user",
        "message": { "role": "user", "content": text },
        "parent_tool_use_id": null,
    })
    .to_string()
}

/// Fold this input into the next model step without aborting tools or tasks.
/// A steer line. `immediate` → `priority: "now"`: streaming text/thinking
/// stops at once and the steer is answered next (verified against CLI
/// 2.1.280). But `now` also aborts an in-flight MCP tool call ("The tool call
/// was interrupted before a result was received"), so while any tool is open
/// the steer goes as `next`: the tool finishes and the steer lands right after
/// its result, in the same turn. An interrupted turn still emits a `result`.
#[cfg(test)]
pub(crate) fn steer_message_line(text: &str, id: &str, immediate: bool) -> String {
    steer_message_line_with_images(text, id, immediate, &[])
}

/// [`steer_message_line`] carrying inline images ahead of the text, in the
/// same block shape as [`user_message_line_with_images`].
pub(crate) fn steer_message_line_with_images(
    text: &str,
    id: &str,
    immediate: bool,
    images: &[ImageBlock],
) -> String {
    let content = if images.is_empty() {
        Value::String(text.to_owned())
    } else {
        Value::Array(content_blocks(text, images))
    };
    serde_json::json!({"type":"user", "uuid":id, "priority": if immediate { "now" } else { "next" },
        "message":{"role":"user","content":content}, "parent_tool_use_id":null})
    .to_string()
}

/// One inline image for a stdin user turn (Anthropic base64 image source).
pub(crate) struct ImageBlock {
    /// One of the API-supported media types (png/jpeg/gif/webp).
    pub media_type: String,
    /// Raw base64 (no data-URL prefix).
    pub data: String,
}

/// A stdin user turn whose content is an array of blocks: the attached images
/// first, then the text — the standard Anthropic image+text message shape
/// (verified against the real CLI: `--input-format stream-json` accepts image
/// content blocks in user frames). Empty `images` degrades to the plain line.
/// The run's opening prompt, tagged with `id` so its `command_lifecycle`
/// frames can be told apart from a steer's. Images precede the text, as in
/// [`user_message_line_with_images`].
pub(crate) fn prompt_line(text: &str, id: &str, images: &[ImageBlock]) -> String {
    let content = if images.is_empty() {
        Value::String(text.to_owned())
    } else {
        Value::Array(content_blocks(text, images))
    };
    json!({
        "type": "user",
        "uuid": id,
        "message": { "role": "user", "content": content },
        "parent_tool_use_id": null,
    })
    .to_string()
}

#[cfg(test)]
pub(crate) fn user_message_line_with_images(text: &str, images: &[ImageBlock]) -> String {
    if images.is_empty() {
        return user_message_line(text);
    }
    json!({
        "type": "user",
        "message": { "role": "user", "content": content_blocks(text, images) },
        "parent_tool_use_id": null,
    })
    .to_string()
}

fn content_blocks(text: &str, images: &[ImageBlock]) -> Vec<Value> {
    let mut blocks: Vec<Value> = images
        .iter()
        .map(|img| {
            json!({
                "type": "image",
                "source": {
                    "type": "base64",
                    "media_type": img.media_type,
                    "data": img.data,
                },
            })
        })
        .collect();
    blocks.push(json!({ "type": "text", "text": text }));
    blocks
}

/// The run's opening control request. `perTaskStopAffordance` tells the CLI
/// this host stops tasks individually, so an interrupt ends the turn and
/// spares background agents — without it the CLI stops every background
/// agent on interrupt (verified live against 2.1.285; background shells
/// survive either way).
pub(crate) fn initialize_request_line(request_id: &str) -> String {
    json!({
        "type": "control_request",
        "request_id": request_id,
        "request": { "subtype": "initialize", "perTaskStopAffordance": true },
    })
    .to_string()
}

/// Switch the live process's model (`None` = the user's default model).
pub(crate) fn set_model_request_line(request_id: &str, model: Option<&str>) -> String {
    json!({
        "type": "control_request",
        "request_id": request_id,
        "request": { "subtype": "set_model", "model": model },
    })
    .to_string()
}

/// Replace the live process's flag-layer settings (effort, fast mode, …).
pub(crate) fn apply_flag_settings_line(request_id: &str, settings: Value) -> String {
    json!({
        "type": "control_request",
        "request_id": request_id,
        "request": { "subtype": "apply_flag_settings", "settings": settings },
    })
    .to_string()
}

/// Stop the in-flight turn only. `cancel_queued` drops steers the CLI has
/// queued but not yet consumed: a stopped turn never continues into them.
pub(crate) fn stop_turn_request_line(request_id: &str) -> String {
    json!({
        "type": "control_request",
        "request_id": request_id,
        "request": { "subtype": "interrupt", "cancel_queued": true },
    })
    .to_string()
}

/// Success reply to a CLI control request (`can_use_tool` allow/deny payloads).
pub(crate) fn control_response_line(request_id: &str, response: Value) -> String {
    json!({
        "type": "control_response",
        "response": {
            "subtype": "success",
            "request_id": request_id,
            "response": response,
        },
    })
    .to_string()
}

/// `can_use_tool` allow payload with the (possibly updated) tool input.
pub(crate) fn allow_response(updated_input: Value) -> Value {
    json!({ "behavior": "allow", "updatedInput": updated_input })
}

/// Client→CLI interrupt control request.
pub(crate) fn interrupt_request_line(request_id: &str) -> String {
    json!({
        "type": "control_request",
        "request_id": request_id,
        "request": { "subtype": "interrupt" },
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steer_priority_follows_tool_state() {
        let now: serde_json::Value =
            serde_json::from_str(&steer_message_line("hi", "u1", true)).unwrap();
        let next: serde_json::Value =
            serde_json::from_str(&steer_message_line("hi", "u2", false)).unwrap();
        assert_eq!(now["priority"], "now");
        assert_eq!(next["priority"], "next");
        assert_eq!(next["uuid"], "u2");
    }

    #[test]
    fn parses_known_and_unknown_frames() {
        let init = r#"{"type":"system","subtype":"init","model":"m","tools":["Bash"],"cwd":"/x","session_id":"s1"}"#;
        match parse_frame(init).expect("parses") {
            Frame::System(f) => {
                assert_eq!(f.subtype, "init");
                assert_eq!(f.session_id, "s1");
            }
            other => panic!("unexpected frame: {other:?}"),
        }
        assert!(matches!(
            parse_frame(r#"{"type":"mystery_frame"}"#).expect("parses"),
            Frame::Other
        ));
        assert!(parse_frame("not json").is_err());
    }

    #[test]
    fn user_line_shape_matches_protocol() {
        let line = user_message_line("hi");
        let v: Value = serde_json::from_str(&line).expect("json");
        assert_eq!(v["type"], "user");
        assert_eq!(v["message"]["content"], "hi");
        assert!(v["parent_tool_use_id"].is_null());
    }

    #[test]
    fn user_line_with_images_is_blocks_then_text() {
        let line = user_message_line_with_images(
            "what is this?",
            &[ImageBlock {
                media_type: "image/png".into(),
                data: "QUJD".into(),
            }],
        );
        let v: Value = serde_json::from_str(&line).expect("json");
        assert_eq!(v["type"], "user");
        let content = v["message"]["content"].as_array().expect("array content");
        assert_eq!(content.len(), 2);
        assert_eq!(content[0]["type"], "image");
        assert_eq!(content[0]["source"]["type"], "base64");
        assert_eq!(content[0]["source"]["media_type"], "image/png");
        assert_eq!(content[0]["source"]["data"], "QUJD");
        assert_eq!(content[1]["type"], "text");
        assert_eq!(content[1]["text"], "what is this?");
        // No images ⇒ identical to the plain string line.
        assert_eq!(
            user_message_line_with_images("hi", &[]),
            user_message_line("hi")
        );
    }
}
