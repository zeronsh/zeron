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

const CLOUD_SETUP_ENTRY_SUFFIX: &str = ".cloud-setup";

/// The assistant entry holding the setup steps of the machine that
/// `message_id` brought up. The sender's placeholder chip ("Starting a Cloud
/// machine") uses the same entry id and [`cloud_setup_part_id`]`(.., "machine")`,
/// so the machine's own entry replaces it in place.
pub fn cloud_setup_entry_id(message_id: &str) -> String {
    format!("{message_id}{CLOUD_SETUP_ENTRY_SUFFIX}")
}

/// The message a [`cloud_setup_entry_id`] belongs to.
pub fn cloud_setup_message_id(entry_id: &str) -> Option<&str> {
    entry_id.strip_suffix(CLOUD_SETUP_ENTRY_SUFFIX)
}

/// One setup chip of that entry: `slot` is `machine` (start or wake),
/// `clone` or `branch`.
pub fn cloud_setup_part_id(message_id: &str, slot: &str) -> String {
    format!("{message_id}.cloud-{slot}")
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
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
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
    /// Credits left — a minute of a small machine each. Sessions don't start
    /// or wake at zero or below. `None` from an edge that predates credits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credits: Option<f64>,
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

/// Credits used on one UTC day.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudUsageDay {
    /// `YYYY-MM-DD` (UTC).
    pub day: String,
    pub credits: f64,
}

/// `CloudUsage` reply: the signed-in user's Cloud credits — what's left, and
/// what was used per UTC day over `[from, to]`. A credit is a minute of a
/// small machine (larger machines use more per minute).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudUsage {
    /// `YYYY-MM-DD`, inclusive.
    #[serde(default)]
    pub from: String,
    #[serde(default)]
    pub to: String,
    /// Every day of the range, oldest first (zero when nothing ran).
    #[serde(default)]
    pub days: Vec<CloudUsageDay>,
    /// Credits used in the range.
    #[serde(default)]
    pub credits: f64,
    /// Credits left (negative when machines ran past zero).
    #[serde(default)]
    pub balance: f64,
    #[serde(default)]
    pub available: bool,
}

/// One monthly plan. A credit is a minute of the default machine.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BillingPlan {
    /// `base` · `pro` · `max`.
    pub id: String,
    pub name: String,
    pub price_cents: u32,
    pub monthly_credits: f64,
}

/// The plan the user is on.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BillingSubscription {
    #[serde(flatten)]
    pub plan: BillingPlan,
    /// Stripe status: `active`, `trialing`, `past_due`.
    pub status: String,
    /// Unix ms the period ends: renews, or lapses when canceling.
    pub period_end: i64,
    pub cancel_at_period_end: bool,
}

/// Where the credit balance stands.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BillingCredits {
    /// Plan credits left this period (they expire when it ends).
    #[serde(default)]
    pub monthly: f64,
    #[serde(default)]
    pub monthly_total: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub monthly_ends_at: Option<i64>,
    /// Credits that never expire.
    #[serde(default)]
    pub extra: f64,
    /// What sessions can spend now.
    #[serde(default)]
    pub total: f64,
}

/// The one-off credit pack.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BillingPack {
    pub price_cents: u32,
    pub credits: f64,
}

/// `BillingStatus` reply.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BillingStatus {
    /// Billing is configured on the server (purchases can be made).
    #[serde(default)]
    pub available: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<BillingSubscription>,
    #[serde(default)]
    pub credits: BillingCredits,
    #[serde(default)]
    pub plans: Vec<BillingPlan>,
    #[serde(default)]
    pub pack: BillingPack,
    /// A Stripe customer exists (the portal can open).
    #[serde(default)]
    pub has_customer: bool,
}

/// `BillingCheckout` / `BillingPortal` reply: the page to open.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BillingLink {
    pub url: String,
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
    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.as_str() == value)
    }

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

    /// Whether this provider's API key may name its own base URL. Only
    /// `anthropic-key`: Claude Code then sends its requests to
    /// `ANTHROPIC_BASE_URL` (an LLM gateway or proxy) instead of Anthropic's
    /// API.
    pub fn takes_base_url(self) -> bool {
        self == VaultProvider::AnthropicKey
    }

    /// The base URL typed beside this provider's API key, as the vault keeps
    /// it: trimmed, without trailing slashes; `Ok(None)` when left empty.
    /// Claude Code appends `/v1/messages` itself, so a trailing `/v1` would
    /// double it. Errors are user-facing; the vault checks the same rules
    /// (`edge/vault/src/providers/keys.ts`).
    pub fn api_base_url(self, raw: &str) -> Result<Option<String>, &'static str> {
        let raw = raw.trim();
        if raw.is_empty() {
            return Ok(None);
        }
        if !self.takes_base_url() {
            return Err("Only an Anthropic API key takes a base URL.");
        }
        if raw.len() > 512 || !raw.bytes().all(|b| b.is_ascii_graphic()) {
            return Err("That doesn't look like a URL.");
        }
        if !raw
            .get(..8)
            .is_some_and(|scheme| scheme.eq_ignore_ascii_case("https://"))
        {
            return Err("Use an https:// base URL — the key is sent with every request.");
        }
        let url = raw.trim_end_matches('/');
        let rest = url.get(8..).unwrap_or_default();
        let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
        if authority.is_empty() {
            return Err("That base URL has no host.");
        }
        if authority.contains('@') {
            return Err("Leave credentials out of the base URL.");
        }
        if rest.contains(['?', '#']) {
            return Err("A base URL can't have a query or a fragment.");
        }
        let path = &rest[authority.len()..];
        if path.len() >= 3 && path[path.len() - 3..].eq_ignore_ascii_case("/v1") {
            return Err("Leave /v1 off the base URL — Claude Code adds /v1/messages itself.");
        }
        Ok(Some(url.to_string()))
    }

    /// The base URL field's verdict as the user types it — every app's
    /// API-key form shows this under the field, so a refusal reads the same
    /// on the desktop and the phone as from [`Self::api_base_url`].
    pub fn check_base_url(self, raw: &str) -> BaseUrlCheck {
        let raw = raw.trim();
        if raw.is_empty() {
            return BaseUrlCheck::Empty;
        }
        // "h", "http", "https:/" are on their way to a URL: calling them wrong
        // would flash red at every keystroke of the scheme.
        if "https://".starts_with(&raw.to_ascii_lowercase()) {
            return BaseUrlCheck::Typing;
        }
        match self.api_base_url(raw) {
            Ok(Some(url)) => BaseUrlCheck::Ready(url),
            Ok(None) => BaseUrlCheck::Empty,
            Err(why) => BaseUrlCheck::Refused(why),
        }
    }
}

/// What Claude Code appends to `ANTHROPIC_BASE_URL` for every request.
pub const ANTHROPIC_MESSAGES_PATH: &str = "/v1/messages";

/// A base URL field, read as it is typed ([`VaultProvider::check_base_url`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BaseUrlCheck {
    /// Nothing typed.
    Empty,
    /// Part of `https://` so far: no verdict until there is a host.
    Typing,
    /// Refused, in [`VaultProvider::api_base_url`]'s words.
    Refused(&'static str),
    /// The base URL as the vault will keep it.
    Ready(String),
}

/// The host of a base URL as [`VaultProvider::api_base_url`] keeps it, the
/// way the vault's label will name it (`new URL(…).host`: lowercase, the
/// default port dropped). Pure.
pub fn base_url_host(url: &str) -> String {
    let rest = url.get("https://".len()..).unwrap_or_default();
    let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
    host.strip_suffix(":443")
        .unwrap_or(host)
        .to_ascii_lowercase()
}

/// The last four characters a stored key is labelled by (`…81e5`), once
/// there is enough of a key for the vault to take it (8 characters). Pure.
pub fn api_key_suffix(key: &str) -> Option<String> {
    let chars: Vec<char> = key.trim().chars().collect();
    (chars.len() >= 8).then(|| {
        let last: String = chars[chars.len() - 4..].iter().collect();
        format!("…{last}")
    })
}

/// The vault's label for a stored API key, split into the key's last four
/// characters (`…abcd`) and the host of the base URL it names, if any — the
/// vault writes `…abcd · llm-gateway.example.com`
/// (`edge/vault/src/providers/keys.ts`). Never secret material.
pub fn api_key_label(label: &str) -> (&str, Option<&str>) {
    match label.split_once(" · ") {
        Some((suffix, host)) if !host.is_empty() => (suffix, Some(host)),
        _ => (label, None),
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
    /// How many logins the provider holds.
    #[serde(default)]
    pub accounts: u32,
    /// Whether one of them is active (grants come from it).
    #[serde(default)]
    pub has_active: bool,
}

/// The `targetDeviceId` the Providers page uses for Cloud: account calls
/// (list, switch, forget, sign in) aimed at it are answered by this device's
/// engine from the vault — Cloud's logins live there, not on a machine.
pub const CLOUD_ACCOUNTS_DEVICE: &str = "cloud";

/// One stored login of a vault provider (`edge/vault/src/api.ts`
/// `VaultAccountView`). A provider holds any number; grants come from the
/// active one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultAccount {
    pub slot: String,
    pub status: VaultConnectionStatus,
    pub active: bool,
    /// `oauth` · `setup-token` · `api-key` · `github`.
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub organization: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
    #[serde(default)]
    pub authorized_devices: Vec<String>,
    #[serde(default)]
    pub created_at: i64,
    #[serde(default)]
    pub updated_at: i64,
}

/// A login's identity sent with its upload (display + de-duplication only).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultAccountProfile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub organization: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<String>,
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
    fn only_an_anthropic_key_takes_a_base_url_and_it_is_kept_as_typed() {
        let anthropic = |raw| VaultProvider::AnthropicKey.api_base_url(raw);
        assert_eq!(anthropic("   "), Ok(None), "empty is Anthropic's own API");
        assert_eq!(
            anthropic(" https://llm-gateway.example.com/ "),
            Ok(Some("https://llm-gateway.example.com".into()))
        );
        assert_eq!(
            anthropic("HTTPS://gateway.ai.cloudflare.com/v1/acct/gw/anthropic//"),
            Ok(Some(
                "HTTPS://gateway.ai.cloudflare.com/v1/acct/gw/anthropic".into()
            ))
        );
        assert_eq!(
            anthropic("https://[::1]:8443/proxy"),
            Ok(Some("https://[::1]:8443/proxy".into()))
        );
        for (raw, why) in [
            ("http://llm-gateway.example.com", "https://"),
            ("llm-gateway.example.com", "https://"),
            ("https://", "no host"),
            ("https:///v1/messages", "no host"),
            ("https://user:secret@gw.example.com", "credentials"),
            ("https://gw.example.com/?key=1", "query"),
            ("https://gw.example.com/#top", "query"),
            ("https://gw.example.com/v1", "/v1"),
            ("https://gw.example.com/anthropic/V1/", "/v1"),
            ("https://gw.example.com/a b", "look like a URL"),
            ("https://gw.exämple.com", "look like a URL"),
        ] {
            let error = anthropic(raw).unwrap_err();
            assert!(error.contains(why), "{raw:?}: {error}");
        }
        assert!(anthropic(&format!("https://gw.example.com/{}", "a".repeat(512))).is_err());
        assert_eq!(VaultProvider::OpenaiKey.api_base_url(""), Ok(None));
        assert!(
            VaultProvider::OpenaiKey
                .api_base_url("https://api.openai.com")
                .is_err()
        );
        assert!(VaultProvider::AnthropicKey.takes_base_url());
        assert!(!VaultProvider::Claude.takes_base_url());
    }

    #[test]
    fn an_api_key_label_names_its_gateway_host() {
        assert_eq!(
            api_key_label("…81e5 · llm-gateway.example.com:8443"),
            ("…81e5", Some("llm-gateway.example.com:8443"))
        );
        assert_eq!(api_key_label("…81e5"), ("…81e5", None));
        assert_eq!(api_key_label("…81e5 · "), ("…81e5 · ", None));
    }

    #[test]
    fn a_base_url_is_judged_only_once_it_has_a_host() {
        let check = |raw| VaultProvider::AnthropicKey.check_base_url(raw);
        assert_eq!(check("  "), BaseUrlCheck::Empty);
        for typing in ["h", "HTTP", "https:/", "https://"] {
            assert_eq!(check(typing), BaseUrlCheck::Typing, "{typing:?}");
        }
        assert_eq!(
            check(" https://llm-gateway.example.com/ "),
            BaseUrlCheck::Ready("https://llm-gateway.example.com".into())
        );
        let BaseUrlCheck::Refused(why) = check("http://llm-gateway.example.com") else {
            panic!("http is refused");
        };
        assert!(why.contains("https://"), "{why}");
        assert!(matches!(
            check("https://llm-gateway.example.com/v1"),
            BaseUrlCheck::Refused(why) if why.contains("/v1")
        ));
        assert!(matches!(
            VaultProvider::OpenaiKey.check_base_url("https://api.openai.com"),
            BaseUrlCheck::Refused(_)
        ));
    }

    #[test]
    fn a_gateway_is_named_by_its_host_as_the_vault_labels_it() {
        assert_eq!(
            base_url_host("https://llm-gateway.example.com"),
            "llm-gateway.example.com"
        );
        assert_eq!(
            base_url_host("https://gateway.ai.cloudflare.com/v1/acct/gw/anthropic"),
            "gateway.ai.cloudflare.com"
        );
        assert_eq!(base_url_host("https://[::1]:8443/proxy"), "[::1]:8443");
        assert_eq!(
            base_url_host("HTTPS://LLM-Gateway.Example.com:443/a"),
            "llm-gateway.example.com"
        );
    }

    #[test]
    fn a_key_is_echoed_by_the_characters_the_vault_labels_it_by() {
        assert_eq!(api_key_suffix("sk-ant"), None, "too short to store");
        assert_eq!(
            api_key_suffix(" sk-gateway-7d1f4c0a2b9e81e5\n").as_deref(),
            Some("…81e5")
        );
    }

    #[test]
    fn status_defaults_tolerate_missing_fields() {
        let status: CloudStatus = serde_json::from_str(r#"{"state":"sleeping"}"#).unwrap();
        assert_eq!(status.state, CloudState::Sleeping);
        assert!(!status.available);
    }
}
