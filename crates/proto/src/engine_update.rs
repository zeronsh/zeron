//! Engine self-update lifecycle, as reported by the device that owns the
//! installation. Advertised by [`crate::capabilities::ENGINE_UPDATES_V1`];
//! older engines only expose the version facts of `UpdateStatus`.
//!
//! The engine owns every accepted operation: closing a window or losing the
//! relay never cancels it, and a client that reconnects reads the same
//! operation back instead of replaying a destructive request.

use serde::{Deserialize, Serialize};

/// Everything a client needs to render one engine installation's row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineUpdateState {
    /// The version of the process answering this stream.
    pub running_version: String,
    /// The version on disk, when the installation layout reveals it. Differs
    /// from `running_version` when a newer binary awaits a restart.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub installed_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_version: Option<String>,
    /// Epoch ms of the last successful release check.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checked_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check_error: Option<String>,
    pub support: EngineUpdateSupport,
    /// The current or most recent operation. Survives the engine restart that
    /// finishes it, so a reconnecting client can confirm the outcome.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation: Option<EngineUpdateOperation>,
}

impl EngineUpdateState {
    /// A newer release exists and nothing is installing it yet.
    pub fn update_available(&self) -> bool {
        let newer = self
            .latest_version
            .as_deref()
            .is_some_and(|latest| version_newer(latest, &self.running_version));
        newer
            && self
                .operation
                .as_ref()
                .is_none_or(|op| op.phase.is_terminal())
    }

    /// A newer binary is installed but this process still runs the old one.
    pub fn restart_pending(&self) -> bool {
        self.installed_version
            .as_deref()
            .is_some_and(|installed| version_newer(installed, &self.running_version))
    }
}

/// What this installation lets a remote client do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum EngineUpdateSupport {
    /// Symlink-managed install run by its service manager: the engine can
    /// stage, apply and restart itself.
    Managed,
    /// Managed install without a supervisor (a hand-started `zeron headless`):
    /// the engine can install, but someone must restart it.
    ManualRestart { reason: String },
    /// Another owner replaces this binary (the desktop app, a package manager,
    /// a source build). The reason names that owner.
    Unsupported { reason: String },
    /// A support level this client does not know yet.
    #[serde(other)]
    Unknown,
}

impl EngineUpdateSupport {
    pub fn can_install(&self) -> bool {
        matches!(self, Self::Managed | Self::ManualRestart { .. })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineUpdateOperation {
    pub id: String,
    /// The client's idempotency key. A retried start with the same key
    /// returns this operation rather than starting another.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    pub from_version: String,
    /// Known once the release feed has been read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_version: Option<String>,
    pub phase: EngineUpdatePhase,
    pub started_at: i64,
    pub updated_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EngineUpdatePhase {
    /// Downloading and verifying the release. Cancellable.
    Staging,
    /// Staged; waiting for runs, terminals and agent updates to finish.
    /// Cancellable.
    WaitingForIdle,
    /// New work is refused and the staged release is being swapped in. The
    /// irreversible boundary: cancellation is refused from here on.
    Applying,
    /// Installed; the service manager is restarting the engine. Clients see
    /// the stream end and should report Reconnecting until it returns.
    Restarting,
    /// Installed, but this process still runs the old version.
    RestartRequired,
    /// The restarted engine runs the target version.
    Updated,
    Failed,
    Cancelled,
    #[serde(other)]
    Unknown,
}

impl EngineUpdatePhase {
    /// No further transition happens without a new request (or a restart,
    /// for `RestartRequired`).
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::RestartRequired | Self::Updated | Self::Failed | Self::Cancelled | Self::Unknown
        )
    }

    pub fn cancellable(self) -> bool {
        matches!(self, Self::Staging | Self::WaitingForIdle)
    }
}

/// Acknowledgement of `StartEngineUpdate`: the operation the engine now owns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineUpdateAck {
    pub operation_id: String,
    pub state: EngineUpdateState,
}

fn version_newer(latest: &str, current: &str) -> bool {
    match (
        crate::version_triple(latest),
        crate::version_triple(current),
    ) {
        (Some(latest), Some(current)) => latest > current,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> EngineUpdateState {
        EngineUpdateState {
            running_version: "0.2.90".into(),
            installed_version: Some("0.2.90".into()),
            latest_version: Some("0.2.97".into()),
            checked_at: Some(1),
            check_error: None,
            support: EngineUpdateSupport::Managed,
            operation: None,
        }
    }

    #[test]
    fn wire_names_are_stable_and_camel_case() {
        let mut state = state();
        state.operation = Some(EngineUpdateOperation {
            id: "op-1".into(),
            request_id: Some("req-1".into()),
            from_version: "0.2.90".into(),
            target_version: Some("0.2.97".into()),
            phase: EngineUpdatePhase::WaitingForIdle,
            started_at: 1,
            updated_at: 2,
            error: None,
        });
        let json = serde_json::to_value(&state).unwrap();
        assert_eq!(json["runningVersion"], "0.2.90");
        assert_eq!(json["support"], serde_json::json!({ "kind": "managed" }));
        assert_eq!(json["operation"]["phase"], "waiting-for-idle");
        assert_eq!(json["operation"]["targetVersion"], "0.2.97");
        assert_eq!(
            serde_json::from_value::<EngineUpdateState>(json).unwrap(),
            state
        );
    }

    #[test]
    fn unknown_support_and_phases_degrade_instead_of_failing() {
        let state: EngineUpdateState = serde_json::from_value(serde_json::json!({
            "runningVersion": "0.3.0",
            "support": { "kind": "fleet-managed", "policy": "x" },
            "operation": {
                "id": "op", "fromVersion": "0.3.0", "phase": "rolling-back",
                "startedAt": 1, "updatedAt": 1
            }
        }))
        .unwrap();
        assert_eq!(state.support, EngineUpdateSupport::Unknown);
        assert!(!state.support.can_install());
        let phase = state.operation.unwrap().phase;
        assert_eq!(phase, EngineUpdatePhase::Unknown);
        assert!(phase.is_terminal() && !phase.cancellable());
    }

    #[test]
    fn availability_and_restart_come_from_versions_not_flags() {
        let mut state = state();
        assert!(state.update_available());
        assert!(!state.restart_pending());
        state.installed_version = Some("0.2.97".into());
        assert!(state.restart_pending());
        state.operation = Some(EngineUpdateOperation {
            id: "op".into(),
            request_id: None,
            from_version: "0.2.90".into(),
            target_version: Some("0.2.97".into()),
            phase: EngineUpdatePhase::Staging,
            started_at: 1,
            updated_at: 1,
            error: None,
        });
        assert!(!state.update_available(), "already being installed");
    }
}
