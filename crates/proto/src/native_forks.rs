//! Exact provider boundaries. Public RPCs reference Zeron messages, never these IDs.
use serde::{Deserialize, Serialize};

use crate::HarnessId;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum NativeForkBoundary {
    AppServerTurn { turn_id: String },
    ClaudeMessage { uuid: String },
    OpenCodeReply { assistant_message_id: String },
    PiEntry { entry_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", try_from = "UncheckedPoint")]
pub struct NativeForkPoint {
    pub format_version: u32,
    pub harness: HarnessId,
    pub source_device_id: String,
    pub source_session_id: String,
    pub cwd: String,
    pub boundary: NativeForkBoundary,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UncheckedPoint {
    format_version: u32,
    harness: HarnessId,
    source_device_id: String,
    source_session_id: String,
    cwd: String,
    boundary: NativeForkBoundary,
}

impl TryFrom<UncheckedPoint> for NativeForkPoint {
    type Error = String;
    fn try_from(raw: UncheckedPoint) -> Result<Self, Self::Error> {
        let point = Self {
            format_version: raw.format_version,
            harness: raw.harness,
            source_device_id: raw.source_device_id,
            source_session_id: raw.source_session_id,
            cwd: raw.cwd,
            boundary: raw.boundary,
        };
        point.validate()?;
        Ok(point)
    }
}

impl NativeForkPoint {
    pub fn validate(&self) -> Result<(), String> {
        let (harness, id) = match &self.boundary {
            NativeForkBoundary::AppServerTurn { turn_id } => (HarnessId::Codex, turn_id),
            NativeForkBoundary::ClaudeMessage { uuid } => (HarnessId::ClaudeCode, uuid),
            NativeForkBoundary::OpenCodeReply {
                assistant_message_id,
            } => (HarnessId::Opencode, assistant_message_id),
            NativeForkBoundary::PiEntry { entry_id } => (HarnessId::Pi, entry_id),
        };
        if self.format_version != 1
            || (self.harness != harness && self.harness != HarnessId::Mock)
            || [
                id,
                &self.source_device_id,
                &self.source_session_id,
                &self.cwd,
            ]
            .iter()
            .any(|value| value.trim().is_empty())
        {
            return Err("Invalid native fork point".into());
        }
        Ok(())
    }
}

pub fn native_fork_provider(harness: HarnessId) -> bool {
    matches!(
        harness,
        HarnessId::Codex | HarnessId::ClaudeCode | HarnessId::Opencode | HarnessId::Pi
    )
}

/// Set by the host from durable history metadata, never trusted from the viewer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ResumePolicy {
    #[default]
    AllowFresh,
    RequireExisting,
}

impl ResumePolicy {
    pub fn is_default(&self) -> bool {
        *self == Self::AllowFresh
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeForkAvailability {
    pub available: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl NativeForkAvailability {
    pub fn available() -> Self {
        Self {
            available: true,
            reason: None,
        }
    }
    pub fn unavailable(reason: impl Into<String>) -> Self {
        Self {
            available: false,
            reason: Some(reason.into()),
        }
    }
}

/// Visual destination only; both modes preserve the same native fork contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NativeForkDestination {
    #[default]
    SideChat,
    MainConversation,
}

impl NativeForkDestination {
    fn is_side_chat(&self) -> bool {
        *self == Self::SideChat
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ForkMessageSideChatRequest {
    pub request_id: String,
    pub chat_id: String,
    pub source_chat_id: String,
    pub source_message_id: String,
    #[serde(default, skip_serializing_if = "NativeForkDestination::is_side_chat")]
    pub destination: NativeForkDestination,
    #[serde(default)]
    pub parent_chat_id: Option<String>,
    pub target_device_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeForkAvailabilityRequest {
    pub source_chat_id: String,
    pub message_ids: Vec<String>,
    pub target_device_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeForkResult {
    pub session_id: String,
    pub cwd: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_fork_points_round_trip_and_reject_mismatched_boundaries() {
        for (harness, boundary) in [
            (
                HarnessId::Codex,
                NativeForkBoundary::AppServerTurn {
                    turn_id: "t1".into(),
                },
            ),
            (
                HarnessId::ClaudeCode,
                NativeForkBoundary::ClaudeMessage { uuid: "m1".into() },
            ),
            (
                HarnessId::Opencode,
                NativeForkBoundary::OpenCodeReply {
                    assistant_message_id: "m1".into(),
                },
            ),
            (
                HarnessId::Pi,
                NativeForkBoundary::PiEntry {
                    entry_id: "entry-1".into(),
                },
            ),
        ] {
            let point = NativeForkPoint {
                format_version: 1,
                harness,
                source_device_id: "device".into(),
                source_session_id: "session".into(),
                cwd: "/project".into(),
                boundary,
            };
            let mut value = serde_json::to_value(&point).unwrap();
            assert_eq!(
                serde_json::from_value::<NativeForkPoint>(value.clone()).unwrap(),
                point
            );
            value["harness"] = serde_json::json!("cursor");
            assert!(serde_json::from_value::<NativeForkPoint>(value).is_err());
        }
    }

    #[test]
    fn old_run_requests_keep_permissive_resume_without_new_wire_fields() {
        let value = serde_json::json!({"prompt":"hello", "model":null, "reasoning":null,
            "cwd":"/project", "sandbox":"read-only", "resume":null});
        let request: crate::RunRequest = serde_json::from_value(value).unwrap();
        assert_eq!(request.resume_policy, ResumePolicy::AllowFresh);
        assert!(
            serde_json::to_value(request)
                .unwrap()
                .get("resumePolicy")
                .is_none()
        );
    }

    #[test]
    fn native_fork_destination_preserves_old_side_chat_requests() {
        let value = serde_json::json!({"requestId":"r", "chatId":"c", "sourceChatId":"s", "sourceMessageId":"m", "targetDeviceId":"d"});
        let mut request: ForkMessageSideChatRequest =
            serde_json::from_value(value.clone()).unwrap();
        assert_eq!(request.destination, NativeForkDestination::SideChat);
        assert!(
            serde_json::to_value(&request)
                .unwrap()
                .get("destination")
                .is_none()
        );
        request.destination = NativeForkDestination::MainConversation;
        let wire = serde_json::to_value(&request).unwrap();
        assert_eq!(wire["destination"], "mainConversation");
        assert_eq!(
            serde_json::from_value::<ForkMessageSideChatRequest>(wire).unwrap(),
            request
        );
        let mut invalid = value;
        invalid["destination"] = serde_json::json!("unknown");
        assert!(serde_json::from_value::<ForkMessageSideChatRequest>(invalid).is_err());
    }

    #[test]
    fn native_rpc_does_not_accept_provider_ids() {
        let value = serde_json::json!({"requestId":"r", "chatId":"c", "sourceChatId":"s", "sourceMessageId":"m", "targetDeviceId":"d", "sourceSessionId":"injected"});
        assert!(serde_json::from_value::<ForkMessageSideChatRequest>(value).is_err());
    }
}
