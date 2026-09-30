//! Workspace lifecycle types shared by the engine and its clients.

use serde::{Deserialize, Serialize};

/// Protocol features are advertised explicitly because personal/integration
/// builds may share a semver with upstream while exposing a different RPC and
/// document surface.
pub mod capabilities {
    /// The host decodes durable composer references at the harness boundary.
    pub const COMPOSER_REFERENCES_V1: &str = "composer-references-v1";
    pub const MESSAGE_QUEUE_V1: &str = "message-queue-v1";
    pub const MESSAGE_QUEUE_ACTIONS_V1: &str = "message-queue-actions-v1";
    pub const MESSAGE_QUEUE_ATTACHMENTS_V1: &str = "message-queue-attachments-v1";
    pub const MESSAGE_QUEUE_CLEAN_ATTACHMENT_TEXT_V1: &str =
        "message-queue-clean-attachment-text-v1";
    pub const MESSAGE_QUEUE_EDIT_LEASE_V1: &str = "message-queue-edit-lease-v1";
    pub const HARNESS_UPDATES_V1: &str = "harness-updates-v1";
    /// The engine can replace itself in place (exec handoff) without stopping
    /// running agents or terminals (`HandoffEngine`). Deliberately NOT in
    /// [`CURRENT`]: it is advertised only by the headless IPC owner that
    /// serves that method, never by an embedded engine that shares `EngineRpc`.
    pub const HANDOFF_V1: &str = "handoff-v1";

    /// This engine is the host a headed app started for itself (its process
    /// carries `ZERON_ENGINE_HOST=app`): quitting that app stops it, and an
    /// update swap of the window leaves it running. Not in [`CURRENT`]; a
    /// service or hand-started engine never has it.
    pub const APP_HOSTED: &str = "app-hosted";

    pub const CURRENT: &[&str] = &[
        COMPOSER_REFERENCES_V1,
        MESSAGE_QUEUE_V1,
        MESSAGE_QUEUE_ACTIONS_V1,
        MESSAGE_QUEUE_ATTACHMENTS_V1,
        MESSAGE_QUEUE_CLEAN_ATTACHMENT_TEXT_V1,
        MESSAGE_QUEUE_EDIT_LEASE_V1,
        HARNESS_UPDATES_V1,
    ];

    pub fn current() -> Vec<String> {
        CURRENT.iter().map(|value| (*value).to_string()).collect()
    }
}

/// The fixed data boundary selected when an engine runtime is assembled.
///
/// Authentication can change while a runtime is alive, but its workspace scope
/// cannot. Switching scopes requires assembling a new runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WorkspaceScope {
    Local,
    Synced,
    Development,
}

/// Stable information about the engine runtime reached by a client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineInfo {
    pub device_id: String,
    pub workspace_scope: WorkspaceScope,
    /// SDK selected by the owning engine, absent on older versions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor_sdk_version: Option<String>,
    /// Supported protocol/document features. Missing on older engines.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<String>,
    /// Build version of the owning engine, for diagnostics and UI skew
    /// display only (never a compatibility gate — see `capabilities`).
    /// Absent on older engines.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

impl EngineInfo {
    pub fn supports(&self, capability: &str) -> bool {
        self.capabilities.iter().any(|value| value == capability)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_scope_uses_wire_safe_names() {
        for (scope, encoded) in [
            (WorkspaceScope::Local, "\"local\""),
            (WorkspaceScope::Synced, "\"synced\""),
            (WorkspaceScope::Development, "\"development\""),
        ] {
            assert_eq!(serde_json::to_string(&scope).unwrap(), encoded);
            assert_eq!(
                serde_json::from_str::<WorkspaceScope>(encoded).unwrap(),
                scope
            );
        }
    }

    #[test]
    fn engine_info_uses_camel_case_fields() {
        let info = EngineInfo {
            device_id: "device-1".into(),
            workspace_scope: WorkspaceScope::Local,
            cursor_sdk_version: Some("1.0.31".into()),
            capabilities: capabilities::current(),
            version: None,
        };
        assert_eq!(
            serde_json::to_value(&info).unwrap(),
            serde_json::json!({
                "deviceId": "device-1",
                "workspaceScope": "local",
                "cursorSdkVersion": "1.0.31",
                "capabilities": [
                    "composer-references-v1",
                    "message-queue-v1",
                    "message-queue-actions-v1",
                    "message-queue-attachments-v1",
                    "message-queue-clean-attachment-text-v1",
                    "message-queue-edit-lease-v1",
                    "harness-updates-v1"
                ],
            })
        );
    }

    #[test]
    fn engine_info_without_a_version_still_parses() {
        let old: EngineInfo = serde_json::from_value(serde_json::json!({
            "deviceId": "d",
            "workspaceScope": "local"
        }))
        .unwrap();
        assert_eq!(old.version, None);
        let new = EngineInfo {
            version: Some("0.3.0".into()),
            ..old
        };
        let round: EngineInfo =
            serde_json::from_str(&serde_json::to_string(&new).unwrap()).unwrap();
        assert_eq!(round.version.as_deref(), Some("0.3.0"));
        // An engine that does not report a version leaves the key out entirely.
        let bare = EngineInfo {
            version: None,
            ..new
        };
        assert!(
            serde_json::to_value(&bare)
                .unwrap()
                .get("version")
                .is_none()
        );
    }

    #[test]
    fn handoff_capability_is_advertised_only_by_the_headless_ipc_owner() {
        // The headless IPC owner adds it to `EngineInfo` because only it serves
        // `HandoffEngine`; an embedded engine sharing `EngineRpc` must not.
        assert_eq!(capabilities::HANDOFF_V1, "handoff-v1");
        assert!(!capabilities::CURRENT.contains(&capabilities::HANDOFF_V1));
    }

    #[test]
    fn old_engine_info_defaults_to_no_capabilities() {
        let info: EngineInfo = serde_json::from_value(serde_json::json!({
            "deviceId": "old",
            "workspaceScope": "synced"
        }))
        .unwrap();
        assert!(info.capabilities.is_empty());
        assert!(info.cursor_sdk_version.is_none());
        assert!(!info.supports(capabilities::MESSAGE_QUEUE_V1));
        assert!(!info.supports(capabilities::COMPOSER_REFERENCES_V1));
    }
}
