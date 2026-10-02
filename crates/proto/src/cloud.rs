//! Cloud device + credential vault wire types (docs/design/cloud-device.md).
//!
//! Cloud is a place a session can run: the checkout picker offers it for any
//! project whose GitHub repository the Zeron GitHub App reaches. Each session
//! run there gets its own sandbox, which clones the repository, hosts an
//! ordinary engine as a hidden *session device*, sleeps when idle and wakes
//! when the chat is sent to. The account has one logical Cloud device id —
//! never a machine, never listed — that its provider connections are
//! authorized for. These types cover what a laptop needs to manage that: the
//! account and its sessions (owned by the edge's CloudAccount DO), the
//! vault-held provider connections, and the repositories Cloud reaches.

use serde::{Deserialize, Serialize};

/// `Device::platform` of the logical Cloud device and of every session
/// device's engine.
pub const CLOUD_PLATFORM: &str = "cloud";

/// Device ids minted by the edge for Cloud devices (logical and session)
/// carry this prefix.
pub const CLOUD_DEVICE_PREFIX: &str = "cloud-";

/// Capability the logical Cloud device's registry row carried (earlier
/// builds listed it). It has no engine: never dial or forward to it.
pub const CLOUD_ACCOUNT_CAPABILITY: &str = "cloud-account";

/// Capability on a session device's row: an engine in one session's sandbox.
/// Device lists hide these; the logical Cloud device stands for them.
pub const CLOUD_SESSION_CAPABILITY: &str = "cloud-session";

/// Harnesses a Cloud device offers in v1 (wire `HarnessId` names).
pub const CLOUD_HARNESSES: &[&str] = &["codex", "claude-code"];

/// A Cloud session machine's own setup steps — started or woke, cloned the
/// project, checked out the session branch — shown as tool chips that open
/// the first answer after it boots: `ToolCall::Unknown { name, input }` whose
/// input carries this key (`"start"`, `"wake"`, `"clone"`, `"branch"`).
pub const CLOUD_SETUP_KEY: &str = "cloudSetup";

/// The setup step a tool call records, if it is one ([`CLOUD_SETUP_KEY`]).
pub fn cloud_setup_step(call: &crate::ToolCall) -> Option<&str> {
    match call {
        crate::ToolCall::Unknown {
            input: Some(input), ..
        } => input.get(CLOUD_SETUP_KEY)?.as_str(),
        _ => None,
    }
}

pub fn is_cloud_device(device_id: &str, platform: &str) -> bool {
    platform == CLOUD_PLATFORM || device_id.starts_with(CLOUD_DEVICE_PREFIX)
}

/// Lifecycle state of the Cloud account (`Off` · `Ready` · `Deleting` ·
/// `Error`) or of one session's sandbox (any).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CloudState {
    /// Account: never enabled (or deleted). Session: no sandbox.
    #[default]
    Off,
    /// Session sandbox being created, the engine installed, the repo cloned.
    Provisioning,
    /// Session waking from sleep.
    Starting,
    /// Account enabled / session engine online.
    Ready,
    /// Session sandbox stopped; wakes when the chat is sent to.
    Sleeping,
    /// Session going to sleep.
    Stopping,
    /// Being deleted.
    Deleting,
    /// Last lifecycle step failed; see `error` / `failed_action`.
    Error,
}

/// `CloudStatus` / `CloudEnable` / `CloudDelete` reply: the account.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudStatus {
    pub state: CloudState,
    /// The logical Cloud device's id, once Cloud is enabled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<String>,
    /// User-facing description of the last failure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Which lifecycle action failed (`enable` · `wake` · `sleep` · `delete`),
    /// so a Retry can repeat exactly that action.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed_action: Option<String>,
    /// Unix ms of the most recent activity of any session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_active_at: Option<i64>,
    /// Sessions whose sandbox is currently running.
    #[serde(default)]
    pub awake_sessions: u32,
    /// Most sessions that may be awake at once; waking another puts the
    /// least recently active idle one to sleep.
    #[serde(default)]
    pub max_awake_sessions: u32,
    /// False when this account cannot use Cloud (edge not configured, signed
    /// out, local-only profile). The UI hides the enable button then.
    #[serde(default)]
    pub available: bool,
}

/// One Cloud session: a top-level chat run on Cloud and its sandbox.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudSession {
    pub chat_id: String,
    /// The session device hosting the chat (the chat row's `deviceId`).
    pub device_id: String,
    /// The project (on whichever device) the chat belongs to.
    pub space_id: String,
    /// The GitHub repository the machine cloned (`owner/name`) and where.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub state: CloudState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failed_action: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_active_at: Option<i64>,
    pub created_at: i64,
}

/// `CloudSessions` reply.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudSessions {
    #[serde(default)]
    pub sessions: Vec<CloudSession>,
    #[serde(default)]
    pub available: bool,
}

/// Metered machine time of one sandbox within a month, as last reconciled
/// against the provider's meter (billable seconds already carry the machine
/// type's multiplier).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudSandboxUsage {
    /// Sandbox provider (`boat`, …).
    #[serde(default)]
    pub provider: String,
    pub sandbox_id: String,
    /// The session (chat) this sandbox ran, when it was a session sandbox.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chat_id: Option<String>,
    /// `small` · `default` · `large`.
    pub sandbox_type: String,
    pub seconds: u64,
    /// Provider list price for `seconds`, USD.
    pub dollars: f64,
    /// Whether the sandbox was running when last reconciled.
    pub running: bool,
    /// Unix ms of the reconciliation that produced these numbers.
    pub reconciled_at: i64,
}

/// `CloudUsage` reply: one user's metered Cloud usage for a UTC month.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudUsage {
    /// `YYYY-MM` (UTC).
    pub month: String,
    pub seconds: u64,
    pub dollars: f64,
    #[serde(default)]
    pub sandboxes: Vec<CloudSandboxUsage>,
    /// True once the month has closed and every sandbox was reconciled
    /// against the provider after month end — the billable figure.
    #[serde(default)]
    pub closed: bool,
    #[serde(default)]
    pub available: bool,
}

/// Credentials the vault can hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum VaultProvider {
    /// ChatGPT sign-in for Codex (uploaded from a laptop's `codex login`).
    Codex,
    /// Claude subscription sign-in for Claude Code (uploaded from a laptop's
    /// sign-in through Claude's own OAuth flow, or a `claude setup-token`).
    Claude,
    /// GitHub App user token (device flow).
    Github,
    /// The user's own Anthropic API key (Claude Code).
    AnthropicKey,
    /// The user's own OpenAI API key (Codex).
    OpenaiKey,
}

impl VaultProvider {
    pub const ALL: [VaultProvider; 5] = [
        VaultProvider::Codex,
        VaultProvider::Claude,
        VaultProvider::Github,
        VaultProvider::AnthropicKey,
        VaultProvider::OpenaiKey,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            VaultProvider::Codex => "codex",
            VaultProvider::Claude => "claude",
            VaultProvider::Github => "github",
            VaultProvider::AnthropicKey => "anthropic-key",
            VaultProvider::OpenaiKey => "openai-key",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum VaultConnectionStatus {
    #[default]
    Connected,
    /// The provider rejected the stored grant; reconnect from a laptop.
    NeedsReconnect,
}

/// One provider connection held by the vault.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultConnection {
    pub provider: VaultProvider,
    pub status: VaultConnectionStatus,
    /// Device ids allowed to request grants for this connection.
    #[serde(default)]
    pub authorized_devices: Vec<String>,
    /// Display label: account email / GitHub login / key suffix.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    pub updated_at: i64,
}

/// A device enrolled with the vault.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultDevice {
    pub device_id: String,
    /// `laptop` | `cloud`.
    pub kind: String,
    pub enrolled_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked_at: Option<i64>,
    /// A Cloud session machine's account device (the logical Cloud device).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
}

/// `VaultStatus` reply.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultStatus {
    #[serde(default)]
    pub connections: Vec<VaultConnection>,
    #[serde(default)]
    pub devices: Vec<VaultDevice>,
    /// False when the account can't reach the vault (signed out, local
    /// profile, vault not deployed).
    #[serde(default)]
    pub available: bool,
    /// Where the user installs the Zeron GitHub App on accounts and
    /// repositories — Cloud sessions reach only those.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub github_install_url: Option<String>,
}

/// `GithubConnectStart` reply: show `user_code`, open `verification_uri`,
/// then call `GithubConnectPoll {flowId}` every `interval_secs`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubDeviceFlow {
    pub flow_id: String,
    pub user_code: String,
    pub verification_uri: String,
    pub interval_secs: u64,
    pub expires_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GithubConnectState {
    /// Keep polling.
    Pending,
    /// Stored in the vault.
    Connected,
    /// Denied, expired, or failed; start again.
    Failed,
}

/// `GithubConnectPoll` reply.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubConnectProgress {
    pub state: GithubConnectState,
    /// GitHub login once connected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// One repository from `ListGithubRepos`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubRepo {
    /// `owner/name`.
    pub full_name: String,
    pub clone_url: String,
    pub default_branch: String,
    pub private: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Unix ms of the last push, for sorting.
    #[serde(default)]
    pub pushed_at: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_wire_names_match_the_vault() {
        for provider in VaultProvider::ALL {
            let wire = serde_json::to_value(provider).unwrap();
            assert_eq!(wire, serde_json::Value::String(provider.as_str().into()));
        }
    }

    #[test]
    fn status_defaults_tolerate_missing_fields() {
        let status: CloudStatus = serde_json::from_str(r#"{"state":"sleeping"}"#).unwrap();
        assert_eq!(status.state, CloudState::Sleeping);
        assert!(!status.available);
    }
}
