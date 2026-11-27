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

/// Harnesses a Cloud device offers (wire `HarnessId` names): every built-in
/// agent. How each signs in there is [`crate::providers::cloud_credentials`].
/// Agents added from the ACP registry stay on the devices that added them.
pub const CLOUD_HARNESSES: &[&str] = &[
    "codex",
    "claude-code",
    "cursor",
    "devin",
    "grok",
    "hermes",
    "fx",
    "pi",
    "opencode",
    "antigravity",
    "muse",
    "kimi",
];

/// A Cloud session machine's own setup steps — started or woke, cloned the
/// project, checked out the session branch — shown as tool chips that open
/// the first answer after it boots: `ToolCall::Unknown { name, input }` whose
/// input carries this key (`"start"`, `"wake"`, `"update"`, `"clone"`, `"branch"`).
pub const CLOUD_SETUP_KEY: &str = "cloudSetup";

/// [`CloudSession::stage`] while its machine installs a release before answering.
pub const CLOUD_STAGE_UPDATING: &str = "updating";
/// `CloudSession::stage` while a wake takes far longer than normal (a resume
/// stuck at the provider, or a machine up with no engine yet): the edge kicks
/// it, and the apps say it's taking longer than usual.
pub const CLOUD_STAGE_SLOW: &str = "slow";

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

/// Names the states beyond the ones every shipped engine knows that this client
/// can parse, on its requests to the edge's session routes.
pub const CLOUD_STATES_HEADER: &str = "x-zeron-cloud-states";

/// What this build sends in [`CLOUD_STATES_HEADER`].
pub const CLOUD_STATES_UNDERSTOOD: &str = "data_expired";

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
    /// Session: the provider deleted the stopped machine's disk. The transcript
    /// is safe; `CloudSessionRetry` starts a fresh machine on the same repo and
    /// branch. The one state whose wire name has an underscore. An edge only
    /// sends it to a client naming it in [`CLOUD_STATES_HEADER`]: to an engine
    /// without this variant, it would fail the whole session list.
    #[serde(rename = "data_expired", alias = "dataExpired")]
    DataExpired,
    /// A state this build doesn't know yet (a newer edge).
    #[serde(other)]
    Unknown,
}

/// One size of Cloud machine, as the edge offers it. The edge's list is the
/// truth: clients show what it says and never assume a set of sizes.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudMachine {
    /// What's sent back to pick it (`small`, `default`, `large`, `xlarge`).
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub vcpu: f64,
    #[serde(default, rename = "memoryGB")]
    pub memory_gb: f64,
    /// What a machine of this size holds: the limit a wake is refused against.
    #[serde(default, rename = "diskGiB")]
    pub disk_gib: f64,
    /// Credits per minute relative to the standard machine (xlarge: 5.56).
    #[serde(default)]
    pub credit_multiplier: f64,
}

/// How full a session machine's disk is, against what its size holds.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudDisk {
    #[serde(default, rename = "usedGiB")]
    pub used_gib: f64,
    #[serde(default, rename = "capacityGiB")]
    pub capacity_gib: f64,
    /// `ok` · `warn` · `critical` (a string: a level this build doesn't know
    /// must not fail the whole list).
    #[serde(default)]
    pub level: String,
}

/// Why a session can't wake on the machine it has.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudWakeBlocked {
    /// `disk_too_small`.
    #[serde(default)]
    pub reason: String,
    #[serde(default, rename = "neededGiB", skip_serializing_if = "Option::is_none")]
    pub needed_gib: Option<f64>,
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
    /// Every machine size a user can pick.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub machines: Vec<CloudMachine>,
    /// The account's default size: what new sessions start on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine: Option<String>,
    /// The account's default reach: what new sessions start with. `None`
    /// from an edge that predates choosing (every session reaches devices).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reach: Option<CloudReach>,
}

/// What a Cloud session reaches beyond its own machine. Only the person sets
/// it (Settings → Cloud, the composer): never an agent, never a machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CloudReach {
    /// The person's other devices, as a chat on one of them reaches them:
    /// their chats, and commands on them.
    #[default]
    Devices,
    /// Its own machine and the chats it delegates to, nothing else.
    Isolated,
}

impl CloudReach {
    pub fn as_str(self) -> &'static str {
        match self {
            CloudReach::Devices => "devices",
            CloudReach::Isolated => "isolated",
        }
    }
}

/// One Cloud session: a top-level chat run on Cloud and its sandbox.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
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
    /// The size it is on, or will come back on when picked asleep.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine: Option<String>,
    /// What it reaches beyond its machine. `None` from an older edge.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reach: Option<CloudReach>,
    /// Omitted until its engine first reports.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disk: Option<CloudDisk>,
    /// Waiting for the user to pick a size its disk fits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wake_blocked: Option<CloudWakeBlocked>,
    /// Unix ms the provider deletes its disk, while it sleeps.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<f64>,
    /// How far a waking machine has got, where the edge says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage: Option<String>,
    /// Unix ms the machine last found its chat store damaged and rebuilt it
    /// from the server copy of its chats. Older edges omit it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub store_recovered_at: Option<f64>,
}

/// `CloudSessions` reply.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
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

/// One monthly plan. A credit is a minute of a small machine.
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

/// Credentials the vault can hold. Logins (`codex`, `claude`, `grok`, …) are
/// one account's sign-in; `*-key` providers are the user's own API keys,
/// one vendor's each, used by every agent that reaches that vendor
/// ([`crate::providers::cloud_credentials`] says which agent takes which).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum VaultProvider {
    /// ChatGPT sign-in (a `codex login` token set). Codex runs on it, and so
    /// does Pi's ChatGPT provider: the vault is its only refresher.
    Codex,
    /// Claude subscription sign-in for Claude Code (uploaded from a laptop's
    /// sign-in through Claude's own OAuth flow, or a `claude setup-token`).
    /// Claude Code runs on it, and so does Pi's Claude provider.
    Claude,
    /// GitHub App user token (device flow).
    Github,
    /// The user's own Anthropic API key.
    AnthropicKey,
    /// The user's own OpenAI API key.
    OpenaiKey,
    /// The user's own OpenRouter API key.
    OpenrouterKey,
    /// The user's own Google Gemini API key.
    GeminiKey,
    /// The user's own xAI API key.
    XaiKey,
    /// The user's own DeepSeek API key.
    DeepseekKey,
    /// The user's own Vercel AI Gateway API key.
    AiGatewayKey,
    /// The user's own Meta API key (Muse Code's `META_API_KEY`).
    MetaKey,
    /// A Cursor user API key: minted by Cursor's own browser sign-in (named
    /// and expiring) or pasted from cursor.com/settings.
    CursorKey,
    /// A GitHub Copilot sign-in: the GitHub token of Copilot's editor app
    /// (device flow), which agents exchange for Copilot sessions themselves.
    Copilot,
    /// A Devin sign-in: the long-lived key (and its servers) Devin's own
    /// browser sign-in writes to `credentials.toml`.
    Devin,
    /// A Grok (xAI) sign-in: the OIDC token set `grok login` writes. The
    /// vault refreshes it; machines get its access token only.
    Grok,
    /// Hermes' xAI sign-in (`hermes auth add xai-oauth`): its own xAI OAuth
    /// grant (API scopes), refreshed by the vault at xAI.
    XaiOauth,
    /// Hermes' Nous Portal sign-in, refreshed by the vault at the portal.
    Nous,
    /// Antigravity's Google sign-in, refreshed by the vault at Google.
    Antigravity,
    /// Muse Code's Meta sign-in: its long-lived access token, used until it
    /// expires (Muse mints its model keys from it).
    Muse,
    /// A Kimi (Moonshot AI) sign-in: the token set `kimi login` writes,
    /// uploaded from a laptop's sign-in; machines get its access token only.
    Kimi,
    /// The user's own Moonshot AI API key (Kimi Code's `MOONSHOT_API_KEY`,
    /// also read as `KIMI_API_KEY`).
    MoonshotKey,
}

impl VaultProvider {
    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.as_str() == value)
    }

    pub const ALL: [VaultProvider; 21] = [
        VaultProvider::Codex,
        VaultProvider::Claude,
        VaultProvider::Github,
        VaultProvider::AnthropicKey,
        VaultProvider::OpenaiKey,
        VaultProvider::OpenrouterKey,
        VaultProvider::GeminiKey,
        VaultProvider::XaiKey,
        VaultProvider::DeepseekKey,
        VaultProvider::AiGatewayKey,
        VaultProvider::MetaKey,
        VaultProvider::CursorKey,
        VaultProvider::Copilot,
        VaultProvider::Devin,
        VaultProvider::Grok,
        VaultProvider::XaiOauth,
        VaultProvider::Nous,
        VaultProvider::Antigravity,
        VaultProvider::Muse,
        VaultProvider::Kimi,
        VaultProvider::MoonshotKey,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            VaultProvider::Codex => "codex",
            VaultProvider::Claude => "claude",
            VaultProvider::Github => "github",
            VaultProvider::AnthropicKey => "anthropic-key",
            VaultProvider::OpenaiKey => "openai-key",
            VaultProvider::OpenrouterKey => "openrouter-key",
            VaultProvider::GeminiKey => "gemini-key",
            VaultProvider::XaiKey => "xai-key",
            VaultProvider::DeepseekKey => "deepseek-key",
            VaultProvider::AiGatewayKey => "ai-gateway-key",
            VaultProvider::MetaKey => "meta-key",
            VaultProvider::CursorKey => "cursor-key",
            VaultProvider::Copilot => "copilot",
            VaultProvider::Devin => "devin",
            VaultProvider::Grok => "grok",
            VaultProvider::XaiOauth => "xai-oauth",
            VaultProvider::Nous => "nous",
            VaultProvider::Antigravity => "antigravity",
            VaultProvider::Muse => "muse",
            VaultProvider::Kimi => "kimi",
            VaultProvider::MoonshotKey => "moonshot-key",
        }
    }

    /// A pasted API key (`{key}` material), as opposed to a sign-in.
    pub fn is_api_key(self) -> bool {
        self.as_str().ends_with("-key")
    }

    /// Who issues the key, for a pasted API key: "OpenRouter".
    pub fn key_vendor(self) -> Option<&'static str> {
        Some(match self {
            VaultProvider::AnthropicKey => "Anthropic",
            VaultProvider::OpenaiKey => "OpenAI",
            VaultProvider::OpenrouterKey => "OpenRouter",
            VaultProvider::GeminiKey => "Google Gemini",
            VaultProvider::XaiKey => "xAI",
            VaultProvider::DeepseekKey => "DeepSeek",
            VaultProvider::AiGatewayKey => "Vercel AI Gateway",
            VaultProvider::MetaKey => "Meta",
            VaultProvider::MoonshotKey => "Moonshot AI",
            VaultProvider::CursorKey => "Cursor",
            _ => return None,
        })
    }

    /// The sign-in a machine prefers over this API key, for the agent whose
    /// card both sit on: saving the key as the one to use steps that login
    /// aside, as on a laptop (Claude Code, Codex and Grok run a login first).
    pub fn login_it_replaces(self) -> Option<VaultProvider> {
        match self {
            VaultProvider::AnthropicKey => Some(VaultProvider::Claude),
            VaultProvider::OpenaiKey => Some(VaultProvider::Codex),
            VaultProvider::XaiKey => Some(VaultProvider::Grok),
            VaultProvider::GeminiKey => Some(VaultProvider::Antigravity),
            VaultProvider::MetaKey => Some(VaultProvider::Muse),
            VaultProvider::MoonshotKey => Some(VaultProvider::Kimi),
            _ => None,
        }
    }
}

/// Request header naming the vault providers a client understands
/// (comma-separated wire names). The edge lists only those in `VaultStatus`;
/// a client that sends none (built before this header) sees the original
/// five, so a provider added later never fails its whole status read.
pub const VAULT_PROVIDERS_HEADER: &str = "x-zeron-vault-providers";

/// [`VAULT_PROVIDERS_HEADER`]'s value for this build.
pub fn vault_providers_understood() -> String {
    VaultProvider::ALL
        .iter()
        .map(|p| p.as_str())
        .collect::<Vec<_>>()
        .join(",")
}

/// `connections`, keeping only providers this build knows: an edge newer
/// than the client may list more, and one unknown entry must not fail the
/// whole status.
fn known_connections<'de, D>(deserializer: D) -> Result<Vec<VaultConnection>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = Vec::<serde_json::Value>::deserialize(deserializer)?;
    Ok(raw
        .into_iter()
        .filter_map(|value| serde_json::from_value(value).ok())
        .collect())
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
    /// GitHub logins: their picture on github.com.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar_url: Option<String>,
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
    #[serde(default, deserialize_with = "known_connections")]
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
    /// Connected, but as a login already connected: nothing was added (GitHub
    /// approves as whoever the browser is signed in as).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub already_connected: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// One repository from `ListGithubRepos`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
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
    /// The GitHub App installation that gives access to it. Access comes from
    /// here; logins say who may use it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub installation_id: Option<i64>,
    /// The connected login a new session on it would use.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    /// Every connected login that reaches it, the one above first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub accounts: Vec<String>,
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
    fn a_provider_this_build_doesnt_know_never_fails_the_status() {
        let status: VaultStatus = serde_json::from_value(serde_json::json!({
            "available": true,
            "connections": [
                {"provider": "codex", "status": "connected", "updatedAt": 1},
                {"provider": "some-later-key", "status": "connected", "updatedAt": 2},
                {"provider": "grok", "status": "needsReconnect", "updatedAt": 3},
            ],
        }))
        .unwrap();
        let providers: Vec<_> = status.connections.iter().map(|c| c.provider).collect();
        assert_eq!(providers, vec![VaultProvider::Codex, VaultProvider::Grok]);
        assert!(
            vault_providers_understood()
                .split(',')
                .all(|p| VaultProvider::parse(p).is_some())
        );
    }

    #[test]
    fn keys_and_logins_say_what_they_are() {
        for provider in VaultProvider::ALL {
            assert_eq!(
                provider.is_api_key(),
                provider.key_vendor().is_some(),
                "{provider:?}"
            );
        }
        assert_eq!(
            VaultProvider::XaiKey.login_it_replaces(),
            Some(VaultProvider::Grok)
        );
        assert_eq!(VaultProvider::OpenrouterKey.login_it_replaces(), None);
    }

    #[test]
    fn status_defaults_tolerate_missing_fields() {
        // What an edge adds later never fails the list it's in.
        let unknown: CloudState = serde_json::from_str(r#""hibernating""#).unwrap();
        assert_eq!(unknown, CloudState::Unknown);
        let expired: CloudState = serde_json::from_str(r#""data_expired""#).unwrap();
        assert_eq!(expired, CloudState::DataExpired);
        // What this build names is what it parses.
        for state in CLOUD_STATES_UNDERSTOOD.split(',') {
            let parsed: CloudState = serde_json::from_value(state.trim().into()).unwrap();
            assert_ne!(parsed, CloudState::Unknown, "{state}");
        }
        let status: CloudStatus = serde_json::from_str(r#"{"state":"sleeping"}"#).unwrap();
        assert_eq!(status.state, CloudState::Sleeping);
        assert!(!status.available);
    }
}
