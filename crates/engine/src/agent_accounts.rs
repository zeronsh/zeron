//! AgentAccounts — the logins of every agent CLI on this device that has one
//! (feature-inventory §3.7 "Agent accounts"; port of zeron's
//! `agent-accounts.ts`).
//!
//! Grok, Devin, OpenCode, Pi and Hermes live in [`stores`] (credential
//! formats and detection), [`oauth`] (the sign-ins the engine drives itself)
//! and [`usage`] (their quota probes); each module documents per provider
//! what is supported and why. In short:
//!
//! | agent    | detect | switch | add account                    | usage |
//! |----------|--------|--------|--------------------------------|-------|
//! | Grok     | yes    | yes    | `grok login --device-auth`     | yes   |
//! | Devin    | yes    | yes    | ACP `authenticate` (loopback)  | yes   |
//! | OpenCode | yes    | yes    | ChatGPT loopback, Copilot code | yes   |
//! | Pi       | yes    | yes    | ChatGPT loopback               | yes   |
//! | Hermes   | yes    | no¹    | `hermes auth add` (device code) | yes  |
//!
//! ¹ Hermes keeps every account in its own credential pool and rotates
//! through it itself; zeron lists the pool and adds to it through Hermes'
//! own CLI, but never rewrites it.
//!
//! The original four providers each store exactly one live login:
//!
//! - **Claude Code** — credentials in `~/.claude/.credentials.json`
//!   (`$CLAUDE_CONFIG_DIR` relocates the dir) or, on macOS, the Keychain item
//!   `Claude Code-credentials`; the account identity (`oauthAccount`, `userID`)
//!   lives in `~/.claude.json`.
//! - **Codex** — `$CODEX_HOME/auth.json` (default `~/.codex`): a ChatGPT OAuth
//!   token set (identity inside the `id_token` JWT) or a raw API key.
//! - **Cursor** — `~/.cursor/sdk/auth.json`: the Cursor SDK's credential store
//!   (`StoredSdkCredentials`) holding the named, expiring user API key its
//!   browser login mints. Deliberately SEPARATE from `cursor-agent login`'s
//!   whole-account session tokens, which zeron never reads.
//! - **Antigravity** — its ACP server keeps one Google login per
//!   `GEMINI_HOME`: a token blob in the macOS Keychain (service `gemini`) or
//!   `antigravity-acp/acp_token.json`, plus the method in `settings.json`.
//!   The blob carries no identity and is never read — the login is listed
//!   from the method and the token's PRESENCE only, active and unswitchable.
//!
//! Claude-swap mechanics:
//!
//! 1. **Detect** the live login of each CLI and auto-snapshot it into a slot
//!    under `{data_dir}/agent-accounts/{harness}/{slotId}.json` — the current
//!    session is always backed up before any swap, and refreshed tokens stay
//!    current.
//! 2. **Swap** (`activate`): overwrite the CLI's credential store (and, for
//!    Claude, merge the identity back into `~/.claude.json`) with a saved slot.
//!    Claude's credential blob is overloaded: `claudeAiOauth` is per-account,
//!    but sibling keys such as `mcpOAuth` are machine-shared MCP/plugin tokens.
//!    Activate splices those live shared fields onto the target login so a
//!    switch does not force every MCP server to re-auth.
//! 3. **Add** (`start_login`…): drive an OAuth flow for a NEW account without
//!    touching the live one (unless it re-signs the live account in, or there
//!    is no live login) — the way each CLI signs in itself: the browser
//!    redirects to a loopback callback that finishes the login unattended.
//!    Claude runs the CLI's PKCE flow against our own `localhost:<port>/callback`
//!    (pasting the code is only the fallback when no port can be bound);
//!    Codex spawns `codex login` against a throwaway `CODEX_HOME` and polls
//!    until its loopback callback lands; Antigravity runs its server's
//!    `authenticate`. A login run for ANOTHER device (`requester`) publishes
//!    its callback port to [`zeron_preview::login`], so the requester can
//!    forward its own loopback to it over the P2P link.
//!
//! Usage probes: all three providers expose the rate-limit view their own CLIs render
//! (`/usage` in Claude Code, `/status` in Codex; Cursor's key has no quota view,
//! so the probe exchanges it for a dashboard session and reads the
//! `GetCurrentPeriodUsage` call the Cursor app itself makes). Usage is
//! stale-while-revalidate: every list serves each account's last good probe
//! (persisted to `agent-accounts/usage-cache.json`, so it survives restarts)
//! with its fetch time; only `force_usage` hits the network, probing all
//! accounts concurrently. The UI paints from a plain list, then forces one to
//! update in place. A failed probe keeps the last good windows, records why
//! (shown instead of "Usage unavailable"), and backs off — honouring
//! `Retry-After` — before that account is probed again. Only 401/403 counts
//! as a rejected token (and only then is a saved Claude slot refreshed).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as BASE64_URL;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use zeron_proto::{
    AgentAccount, AgentAccountWarning, AgentAccountsSnapshot, AgentAuthKind, AgentLoginMode,
    AgentLoginPoll, AgentLoginStart, AgentLoginStatus, AgentUsageWindow, HarnessId,
};

use crate::repos::home_dir;
use crate::{EngineError, new_id, now_ms};

mod oauth;
#[cfg(test)]
mod provider_tests;
mod stores;
mod usage;

// Claude Code's public OAuth client (the one the CLI itself uses for the manual
// "paste the code" flow — no secret involved, PKCE carries the proof).
const CLAUDE_CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
const CLAUDE_REDIRECT: &str = "https://console.anthropic.com/oauth/code/callback";
const CLAUDE_SCOPES: &str = "org:create_api_key user:profile user:inference";
const CLAUDE_TOKEN_URL: &str = "https://console.anthropic.com/v1/oauth/token";
const CLAUDE_PROFILE_URL: &str = "https://api.anthropic.com/api/oauth/profile";
const CLAUDE_USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
// Claude Code 2.1.x's automatic (loopback) login: the claude.ai authorize
// page, the token endpoint it exchanges at, the page it sends the browser to
// once the code is redeemed, and the scopes it asks for.
const CLAUDE_LOOPBACK_AUTHORIZE_URL: &str = "https://claude.com/cai/oauth/authorize";
const CLAUDE_LOOPBACK_TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";
const CLAUDE_LOOPBACK_SUCCESS_URL: &str =
    "https://platform.claude.com/oauth/code/success?app=claude-code";
const CLAUDE_LOOPBACK_SCOPES: &str = "org:create_api_key user:profile user:inference \
     user:sessions:claude_code user:mcp_servers user:file_upload user:plugins";
const CODEX_USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";
/// The Cursor dashboard's current-period usage RPC (Connect-style POST).
const CURSOR_CURRENT_PERIOD_USAGE: &str = "aiserver.v1.DashboardService/GetCurrentPeriodUsage";
const CURSOR_DEFAULT_BACKEND: &str = "https://api2.cursor.sh";

/// Claude Code stores these next to `claudeAiOauth` in the same credential
/// blob, but they are machine-shared (MCP server OAuth, plugin secrets) and
/// rotate independently of any account slot. On activate the live copies win.
const CLAUDE_SHARED_CREDENTIAL_KEYS: &[&str] = &[
    "mcpOAuth",
    "mcpOAuthClientConfig",
    "mcpXaaIdp",
    "mcpXaaIdpConfig",
    "pluginSecrets",
];

/// A forced list re-probes an account only when its last attempt is older
/// than this — the page's paint-then-refresh pair and Refresh mashing must
/// not multiply provider calls (Anthropic's usage endpoint 429s eagerly).
const FORCED_MIN_INTERVAL: Duration = Duration::from_secs(30);
/// How long a live Claude credential read is reused (see
/// [`AgentAccounts::read_claude_credentials_cached`]).
const CLAUDE_CREDENTIALS_TTL: Duration = Duration::from_secs(10);
/// An abandoned login flow (dialog dismissed without Cancel) is reaped past this.
const FLOW_TTL: Duration = Duration::from_secs(15 * 60);
const HTTP_TIMEOUT: Duration = Duration::from_secs(8);

/// A failed identity lookup is retried no sooner than this — an offline
/// device must not call the profile endpoint on every list.
const IDENTITY_RETRY: Duration = Duration::from_secs(5 * 60);

/// A remembered identity lookup (see `Inner::identities`).
#[derive(Clone)]
enum IdentityLookup {
    Known(String, SlotProfile),
    Failed(Instant),
}

/// Filesystem knobs — env-resolved in production ([`AgentAccountsConfig::detect`]),
/// explicit in tests.
#[derive(Debug, Clone)]
pub struct AgentAccountsConfig {
    /// Engine data dir; slots live under `{data_dir}/agent-accounts/`.
    pub data_dir: PathBuf,
    /// Claude config dir (`$CLAUDE_CONFIG_DIR` or `~/.claude`) — holds `.credentials.json`.
    pub claude_config_dir: PathBuf,
    /// Claude identity file (`~/.claude.json`, or `$CLAUDE_CONFIG_DIR/.claude.json`).
    pub claude_config_file: PathBuf,
    /// Codex home (`$CODEX_HOME` or `~/.codex`) — holds `auth.json`.
    pub codex_home: PathBuf,
    /// The Cursor SDK's credential store (`~/.cursor/sdk/auth.json`): the
    /// named, expiring API key minted by its browser login. SEPARATE from
    /// `cursor-agent login`'s session tokens — deliberately never read.
    pub cursor_sdk_auth_file: PathBuf,
    /// macOS Keychain service holding Claude Code's credentials, or `None`
    /// to use `.credentials.json` only (tests — a temp config must never
    /// read or write the real login). See [`claude_keychain_service`].
    pub claude_keychain_service: Option<String>,
    /// Antigravity's `GEMINI_HOME` (`None`: unresolvable — no Antigravity row).
    pub antigravity_home: Option<PathBuf>,
    /// Whether Antigravity's Keychain token counts (macOS production); tests
    /// look at the temp token files only.
    pub antigravity_keychain: bool,
    /// Grok's `GROK_HOME` (default `~/.grok`) — holds `auth.json`.
    pub grok_home: PathBuf,
    /// Devin's `credentials.toml` (`$XDG_DATA_HOME/devin/`, default
    /// `~/.local/share/devin/`).
    pub devin_credentials_file: PathBuf,
    /// OpenCode's `auth.json` (`$XDG_DATA_HOME/opencode/`, default
    /// `~/.local/share/opencode/`).
    pub opencode_auth_file: PathBuf,
    /// Pi's agent dir (`$PI_CODING_AGENT_DIR`, default `~/.pi/agent`) —
    /// holds `auth.json`.
    pub pi_agent_dir: PathBuf,
    /// Hermes' `HERMES_HOME` (default `~/.hermes`) — holds `auth.json`.
    pub hermes_home: PathBuf,
}

impl AgentAccountsConfig {
    /// Production resolution: `CLAUDE_CONFIG_DIR` relocates both the Claude config
    /// json and the credentials file; `CODEX_HOME` relocates the Codex auth file.
    pub fn detect(data_dir: &Path) -> Self {
        let env_dir = |name: &str| {
            std::env::var_os(name)
                .filter(|s| !s.is_empty())
                .map(PathBuf::from)
        };
        let claude_dir = env_dir("CLAUDE_CONFIG_DIR");
        let claude_config_file = match &claude_dir {
            Some(dir) => dir.join(".claude.json"),
            None => home_dir().join(".claude.json"),
        };
        Self {
            data_dir: data_dir.to_path_buf(),
            claude_keychain_service: cfg!(target_os = "macos")
                .then(|| claude_keychain_service(claude_dir.as_deref())),
            claude_config_dir: claude_dir.unwrap_or_else(|| home_dir().join(".claude")),
            claude_config_file,
            codex_home: env_dir("CODEX_HOME").unwrap_or_else(|| home_dir().join(".codex")),
            cursor_sdk_auth_file: home_dir().join(".cursor").join("sdk").join("auth.json"),
            antigravity_home: zeron_harness::acp::antigravity_home().ok(),
            antigravity_keychain: cfg!(target_os = "macos")
                && std::env::var_os("AGY_ACP_FORCE_FILE_STORAGE")
                    .is_none_or(|v| !matches!(v.to_str(), Some("1" | "true"))),
            grok_home: stores::default_grok_home(),
            devin_credentials_file: stores::default_devin_credentials_file(),
            opencode_auth_file: stores::default_opencode_auth_file(),
            pi_agent_dir: stores::default_pi_agent_dir(),
            hermes_home: stores::default_hermes_home(),
        }
    }

    /// Every provider pointed into `root` — never a real login or the
    /// Keychain. For tests.
    #[doc(hidden)]
    pub fn isolated(root: &Path) -> Self {
        Self {
            data_dir: root.join("data"),
            claude_config_dir: root.join("claude"),
            claude_config_file: root.join("claude.json"),
            codex_home: root.join("codex"),
            cursor_sdk_auth_file: root.join("cursor-sdk").join("auth.json"),
            claude_keychain_service: None,
            antigravity_home: Some(root.join("gemini")),
            antigravity_keychain: false,
            grok_home: root.join("grok"),
            devin_credentials_file: root.join("devin").join("credentials.toml"),
            opencode_auth_file: root.join("opencode").join("auth.json"),
            pi_agent_dir: root.join("pi"),
            hermes_home: root.join("hermes"),
        }
    }

    fn claude_creds_file(&self) -> PathBuf {
        self.claude_config_dir.join(".credentials.json")
    }

    fn codex_auth_file(&self) -> PathBuf {
        self.codex_home.join("auth.json")
    }

    fn root_dir(&self) -> PathBuf {
        self.data_dir.join("agent-accounts")
    }

    /// [`Self::root_dir`], created (or tightened) owner-only — it holds slot
    /// files and every throwaway sign-in home.
    fn private_root(&self) -> std::io::Result<PathBuf> {
        let root = self.root_dir();
        private_dir(&root)?;
        Ok(root)
    }

    fn usage_cache_file(&self) -> PathBuf {
        self.root_dir().join("usage-cache.json")
    }
}

/// `dir` created (parents as needed) and itself made owner-only — 0700 on
/// Unix, an existing looser dir included. Sign-in homes hold whatever a CLI
/// writes there (fresh tokens, in modes the CLI picks), so the directory is
/// the boundary that keeps other local users out.
fn private_dir(dir: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        if let Some(parent) = dir.parent() {
            std::fs::create_dir_all(parent)?;
        }
        match std::fs::DirBuilder::new().mode(0o700).create(dir) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists && dir.is_dir() => {}
            Err(err) => return Err(err),
        }
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(dir)
    }
}

/// Claude Code's Keychain service name (2.1.x `CL()`): `Claude Code-credentials`,
/// suffixed with the first 8 hex of sha256(config dir) when `CLAUDE_CONFIG_DIR`
/// relocates the config — each config dir is its own login.
fn claude_keychain_service(config_dir: Option<&Path>) -> String {
    match config_dir {
        None => "Claude Code-credentials".to_string(),
        Some(dir) => {
            let digest = Sha256::digest(dir.to_string_lossy().as_bytes());
            format!(
                "Claude Code-credentials-{}",
                &crate::repos::hex(&digest)[..8]
            )
        }
    }
}

// ── slot storage ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SlotProfile {
    email: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    organization: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    plan: Option<String>,
    auth_kind: AgentAuthKind,
}

/// One saved login (`{slotId}.json`), same field surface as zeron's slot files.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Slot {
    id: String,
    harness: HarnessId,
    /// The provider-side identity the slot is keyed by (account uuid/email).
    account_key: String,
    profile: SlotProfile,
    /// Claude: the `.credentials.json`/Keychain payload. Codex: `auth.json`.
    credentials: serde_json::Value,
    /// Claude only: `{oauthAccount, userID}` merged into `~/.claude.json` on swap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    claude_config: Option<serde_json::Value>,
    saved_at: i64,
    /// First time this account was saved — the STABLE sort key, so switching the
    /// active account (which re-snapshots and bumps `saved_at`) never reorders.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    created_at: Option<i64>,
    /// Agents that keep one login PER model provider in one store (OpenCode,
    /// Pi, Hermes): the store key this slot's `credentials` entry lives
    /// under (`openai`, `anthropic`, …). Swapping rewrites that entry only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    store_key: Option<String>,
}

/// A live detection result (before it's persisted into a slot).
#[derive(Debug, Clone)]
struct Detected {
    account_key: String,
    profile: SlotProfile,
    /// `None` ⇒ we know a login exists but couldn't read the secret.
    credentials: Option<serde_json::Value>,
    claude_config: Option<serde_json::Value>,
    /// See [`Slot::store_key`].
    store_key: Option<String>,
    /// `false` when the store carries no identity (Devin's bare API key):
    /// the profile is a placeholder, and a snapshot keeps whatever identity
    /// a usage probe already wrote into the slot.
    identity_known: bool,
}

impl Detected {
    /// A login whose store names who it is — the common case.
    fn known(account_key: String, profile: SlotProfile, credentials: serde_json::Value) -> Self {
        Self {
            account_key,
            profile,
            credentials: Some(credentials),
            claude_config: None,
            store_key: None,
            identity_known: true,
        }
    }

    fn keyed(mut self, store_key: &str) -> Self {
        self.store_key = Some(store_key.to_string());
        self
    }
}

// ── login flows ─────────────────────────────────────────────────────────────

enum LoginFlow {
    Claude {
        verifier: String,
        /// The OAuth `state`, random and separate from the PKCE verifier (a
        /// verifier in the authorize url would defeat PKCE).
        state: String,
        started_at: Instant,
    },
    /// A spawned login child polled to completion: `codex login` against a
    /// throwaway `CODEX_HOME`, the cursor shim's login mode minting into a
    /// throwaway store file, `grok login` against a throwaway `GROK_HOME`, or
    /// `hermes auth add` appending to Hermes' own pool.
    Spawned {
        harness: HarnessId,
        /// The login child; monitored (try_wait) + killable from cancel.
        child: Arc<Mutex<Option<zeron_harness::process::Child>>>,
        /// Throwaway dir, reclaimed on cancel/completion.
        home: PathBuf,
        /// What finishing looks like.
        completion: SpawnedCompletion,
        started_at: Instant,
        output: Arc<Mutex<String>>,
        /// `Some(code)` once the child exited (`None` code = killed by signal).
        exit: Arc<Mutex<Option<Option<i32>>>>,
        /// Finds the sign-in page in the child's output.
        scan_url: fn(&str) -> Option<String>,
        /// The device a remote login's callback is forwarded for.
        requester: Option<String>,
    },
    /// A sign-in the engine drives itself — Claude's and ChatGPT's loopback
    /// callbacks, GitHub's device code, Antigravity's and Devin's ACP
    /// `authenticate` — reporting the browser url and its outcome through
    /// `state`.
    Task {
        harness: HarnessId,
        started_at: Instant,
        state: Arc<Mutex<TaskLoginState>>,
        /// aborting drops the sign-in future, which kills its agent child.
        handle: tokio::task::JoinHandle<()>,
        /// A throwaway dir the sign-in writes into, reclaimed on cancel.
        home: Option<PathBuf>,
        /// A FIXED loopback port the sign-in holds (ChatGPT's 1455) — a new
        /// sign-in needing the same port supersedes this one.
        port: Option<u16>,
    },
}

/// How a [`LoginFlow::Spawned`] child signals success.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SpawnedCompletion {
    /// Its credential file appears under the throwaway home (`auth.json`);
    /// the child keeps running until then.
    CredentialFile,
    /// It exits 0 — the CLI saved the login into its own store (Hermes).
    ExitSuccess,
}

#[derive(Default)]
struct TaskLoginState {
    url: Option<String>,
    message: Option<String>,
    outcome: Option<Result<(), String>>,
    /// The device a remote login's callback is forwarded for.
    requester: Option<String>,
}

impl LoginFlow {
    fn started_at(&self) -> Instant {
        match self {
            LoginFlow::Claude { started_at, .. }
            | LoginFlow::Spawned { started_at, .. }
            | LoginFlow::Task { started_at, .. } => *started_at,
        }
    }
}

// ── service ─────────────────────────────────────────────────────────────────

/// One live usage probe: rate-limit windows plus the plan label the provider
/// reported alongside them (Codex's usage endpoint carries a live `plan_type`,
/// which supersedes the login-time JWT claim — plan changes show up here
/// without a re-login). Claude's usage endpoint has no plan field.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UsageSnapshot {
    windows: Vec<AgentUsageWindow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    plan_label: Option<String>,
}

/// Why a usage probe produced no windows. Drives the backoff and the reason
/// the UI shows instead of a bare "Usage unavailable". Persisted with the
/// usage cache so a relaunch neither forgets a Retry-After nor the reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
enum ProbeError {
    /// 401/403 — the token was rejected; the ONLY status worth a refresh.
    #[serde(rename_all = "camelCase")]
    Unauthorized { status: u16 },
    /// 429 — `retry_after_secs` from the `Retry-After` header when sent.
    #[serde(rename_all = "camelCase")]
    RateLimited { retry_after_secs: Option<u64> },
    /// Any other non-2xx: 5xx outages, a 404 from a moved endpoint, ….
    #[serde(rename_all = "camelCase")]
    Http {
        status: u16,
        retry_after_secs: Option<u64>,
    },
    /// Never reached the provider (DNS/connect/TLS) or it didn't answer in time.
    Network { timeout: bool },
    /// A 2xx whose body didn't carry the windows we parse (schema drift).
    Schema,
    /// The slot holds nothing probeable.
    #[serde(rename_all = "camelCase")]
    NoCredentials { why: NoCredentials },
    /// The credentials name a server (issuer, API or portal url) outside the
    /// provider's known hosts: nothing was sent.
    UntrustedEndpoint,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
enum NoCredentials {
    /// Codex API-key mode: no ChatGPT rate windows exist.
    ApiKey,
    /// The slot has no access token / key at all.
    Missing,
    /// Cursor's minted key is past its expiry.
    KeyExpired,
    /// The provider behind this login has no usage view zeron can read
    /// (a Hermes API key for some other vendor, Pi's Copilot session).
    Unsupported,
}

impl ProbeError {
    /// Short machine class for logs.
    fn class(&self) -> &'static str {
        match self {
            ProbeError::Unauthorized { .. } => "unauthorized",
            ProbeError::RateLimited { .. } => "rate-limited",
            ProbeError::Http { status, .. } if *status >= 500 => "server-error",
            ProbeError::Http { .. } => "http-error",
            ProbeError::Network { timeout: true } => "timeout",
            ProbeError::Network { .. } => "network",
            ProbeError::Schema => "schema",
            ProbeError::NoCredentials { .. } => "no-credentials",
            ProbeError::UntrustedEndpoint => "untrusted-endpoint",
        }
    }

    fn status(&self) -> Option<u16> {
        match self {
            ProbeError::Unauthorized { status } | ProbeError::Http { status, .. } => Some(*status),
            ProbeError::RateLimited { .. } => Some(429),
            _ => None,
        }
    }

    /// How long to leave the provider alone after this failure. A server-sent
    /// Retry-After wins (clamped so a bogus value can't stall usage for a
    /// day, nor a 0 turn into hammering).
    fn backoff(&self) -> Duration {
        const MIN: u64 = 30;
        match self {
            ProbeError::RateLimited { retry_after_secs } => {
                Duration::from_secs(retry_after_secs.unwrap_or(5 * 60).clamp(MIN, 60 * 60))
            }
            ProbeError::Http {
                status,
                retry_after_secs,
            } if *status >= 500 => {
                Duration::from_secs(retry_after_secs.unwrap_or(60).clamp(MIN, 30 * 60))
            }
            // 404/400…: the request itself is wrong — retrying soon won't help.
            ProbeError::Http { .. } | ProbeError::Schema => Duration::from_secs(15 * 60),
            ProbeError::Network { .. } => Duration::from_secs(MIN),
            // Lifted early when the slot's credentials change (re-login, the
            // CLI refreshing its token) — see `UsageEntry::probe_due`.
            ProbeError::Unauthorized { .. }
            | ProbeError::NoCredentials { .. }
            | ProbeError::UntrustedEndpoint => Duration::from_secs(10 * 60),
        }
    }
}

/// Per-account usage state, persisted to `agent-accounts/usage-cache.json`
/// so the first list after launch paints the last known windows instantly.
/// All times are epoch millis (they must survive a restart).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UsageEntry {
    /// The last SUCCESSFUL probe — kept through later failures (a 429 must
    /// not blank meters that were right a minute ago).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    usage: Option<UsageSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    fetched_at: Option<i64>,
    /// The last probe's failure; cleared by the next success.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<ProbeError>,
    /// Last probe attempt, success or failure.
    #[serde(default)]
    checked_at: i64,
    /// No probe before this (Retry-After / backoff).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    retry_at: Option<i64>,
    /// Fingerprint of the credentials the last failure was against.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    credentials: Option<String>,
}

impl UsageEntry {
    /// Whether a forced list should probe this account now.
    fn probe_due(&self, credentials: &str, now: i64) -> bool {
        if now - self.checked_at < FORCED_MIN_INTERVAL.as_millis() as i64 {
            return false;
        }
        match (self.retry_at, &self.error) {
            (Some(at), Some(error)) if now < at => {
                // A token rejection (or a credential-less slot) is about THAT
                // credential: new credentials deserve a probe right away. A
                // rate limit or outage is not, so it holds regardless.
                matches!(
                    error,
                    ProbeError::Unauthorized { .. }
                        | ProbeError::NoCredentials { .. }
                        | ProbeError::UntrustedEndpoint
                ) && self.credentials.as_deref() != Some(credentials)
            }
            _ => true,
        }
    }

    fn record(&mut self, result: Result<UsageSnapshot, ProbeError>, credentials: String, now: i64) {
        self.checked_at = now;
        match result {
            Ok(usage) => {
                self.usage = Some(usage);
                self.fetched_at = Some(now);
                self.error = None;
                self.retry_at = None;
                self.credentials = None;
            }
            Err(error) => {
                self.retry_at = Some(now + error.backoff().as_millis() as i64);
                self.error = Some(error);
                self.credentials = Some(credentials);
            }
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct UsageCacheFile {
    #[serde(default)]
    entries: HashMap<String, UsageEntry>,
}

/// Network knobs — the real provider endpoints in production, a local mock
/// in tests. `allow_slot_refresh = false` keeps a probe strictly read-only
/// (the live diagnosis harness must never rotate a real refresh token).
#[derive(Debug, Clone)]
struct ProbeEndpoints {
    claude_usage: String,
    claude_token: String,
    /// Where the loopback login redeems its code (and reads the profile).
    claude_loopback_token: String,
    claude_profile: String,
    codex_usage: String,
    /// Grok's billing view (`/v1/billing?format=credits`).
    grok_usage: String,
    /// GitHub's REST root (`/user`, `/copilot_internal/user`).
    github_api: String,
    /// GitHub's OAuth root (`/login/device/code`, `/login/oauth/access_token`).
    github_login: String,
    /// OpenAI's OAuth root (`/oauth/authorize`, `/oauth/token`).
    openai_auth: String,
    /// The ChatGPT sign-in's loopback port — FIXED at 1455 by the client
    /// registration; tests bind any free port (0).
    openai_port: u16,
    /// The Nous portal (`/api/oauth/account`).
    nous_portal: String,
    allow_slot_refresh: bool,
    /// Test seam: admit `http://127.0.0.1` for credential-DEFINED endpoints
    /// (Grok issuer, Devin server, …) so mock servers can stand in. Always
    /// `false` in production — see [`stores::trusted_base`].
    allow_loopback_http: bool,
}

impl Default for ProbeEndpoints {
    fn default() -> Self {
        Self {
            claude_usage: CLAUDE_USAGE_URL.into(),
            claude_token: CLAUDE_TOKEN_URL.into(),
            claude_loopback_token: CLAUDE_LOOPBACK_TOKEN_URL.into(),
            claude_profile: CLAUDE_PROFILE_URL.into(),
            codex_usage: CODEX_USAGE_URL.into(),
            grok_usage: usage::GROK_USAGE_URL.into(),
            github_api: "https://api.github.com".into(),
            github_login: "https://github.com".into(),
            openai_auth: oauth::OPENAI_AUTH.into(),
            openai_port: oauth::OPENAI_LOOPBACK_PORT,
            nous_portal: usage::NOUS_PORTAL.into(),
            allow_slot_refresh: true,
            allow_loopback_http: false,
        }
    }
}

/// The live Claude credential read, cached briefly per identity (see
/// [`AgentAccounts::read_claude_credentials_cached`]).
struct CachedClaudeCredentials {
    account_key: String,
    read: (Option<serde_json::Value>, Option<String>),
    at: Instant,
}

struct Inner {
    config: AgentAccountsConfig,
    http: reqwest::Client,
    endpoints: ProbeEndpoints,
    /// Serializes operations that inspect and then mutate live credential
    /// stores and slots. Without this, a concurrent list can snapshot stale
    /// credentials over a freshly signed-in slot, or a switch can race a
    /// removal and cause the wrong live account to be signed out.
    ops: tokio::sync::Mutex<()>,
    flows: Mutex<HashMap<String, LoginFlow>>,
    /// `"{harness}:{accountKey}"` → usage state; mirrored to disk.
    usage: Mutex<HashMap<String, UsageEntry>>,
    /// Accounts with a usage probe in flight — overlapping forced lists (the
    /// page's paint-then-refresh, two open windows) share one probe.
    inflight_probes: Mutex<std::collections::HashSet<String>>,
    /// Slots with a token refresh in flight — a second refresh of the same
    /// (commonly single-use) refresh token would revoke the family.
    inflight_refreshes: Mutex<std::collections::HashSet<String>>,
    claude_credentials: Mutex<Option<CachedClaudeCredentials>>,
    /// Callback ports of logins run for another device (see module docs).
    callback_routes: zeron_preview::login::CallbackRoutes,
    /// Who an opaque live token belongs to (Pi's Claude login, a Copilot
    /// token), by token fingerprint — one profile call per token, not per
    /// list; a failed lookup waits [`IDENTITY_RETRY`] before the next. See
    /// [`stores`].
    identities: Mutex<HashMap<String, IdentityLookup>>,
    /// Test seam: fixed CLI binaries per agent instead of PATH resolution.
    cli_overrides: Mutex<HashMap<HarnessId, PathBuf>>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Releases single-flight markers on drop. RPC handlers are aborted when the
/// client cancels or disconnects, so a marker removed only after the await
/// would stay forever — and that account would never probe (or refresh)
/// again until restart.
struct InflightGuard<'a> {
    set: &'a Mutex<std::collections::HashSet<String>>,
    keys: Vec<String>,
}

impl Drop for InflightGuard<'_> {
    fn drop(&mut self) {
        let mut set = lock(self.set);
        for key in &self.keys {
            set.remove(key);
        }
    }
}

#[derive(Clone)]
pub struct AgentAccounts {
    inner: Arc<Inner>,
}

impl AgentAccounts {
    pub fn new(config: AgentAccountsConfig) -> Self {
        Self::with_endpoints(config, ProbeEndpoints::default(), Default::default())
    }

    /// [`Self::new`], publishing remote logins' callback ports to `routes`
    /// (the engine's P2P service, which serves them to the requester).
    pub fn with_callback_routes(
        config: AgentAccountsConfig,
        routes: zeron_preview::login::CallbackRoutes,
    ) -> Self {
        Self::with_endpoints(config, ProbeEndpoints::default(), routes)
    }

    fn with_endpoints(
        config: AgentAccountsConfig,
        endpoints: ProbeEndpoints,
        callback_routes: zeron_preview::login::CallbackRoutes,
    ) -> Self {
        // Startup sweep: a previous process that crashed mid-login leaves
        // `.login-<uuid>` throwaway CODEX_HOME dirs — each may hold live OAuth
        // tokens — with no owner to clean them. Reclaim them at boot.
        let root = config.root_dir();
        // A root from an older build may be group/world-readable: tighten it.
        if root.is_dir()
            && let Err(err) = private_dir(&root)
        {
            tracing::warn!(error = %err, "could not make the agent-accounts dir private");
        }
        if let Ok(entries) = std::fs::read_dir(&root) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with(".login-") {
                    let _ = std::fs::remove_dir_all(entry.path());
                }
            }
        }
        let http = reqwest::Client::builder()
            .timeout(HTTP_TIMEOUT)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        // Last known usage from the previous run — a missing or torn file is
        // just a cold cache.
        let usage = std::fs::read_to_string(config.usage_cache_file())
            .ok()
            .and_then(|raw| serde_json::from_str::<UsageCacheFile>(&raw).ok())
            .map(|file| file.entries)
            .unwrap_or_default();
        Self {
            inner: Arc::new(Inner {
                config,
                http,
                endpoints,
                ops: tokio::sync::Mutex::new(()),
                flows: Mutex::new(HashMap::new()),
                usage: Mutex::new(usage),
                inflight_probes: Mutex::new(std::collections::HashSet::new()),
                inflight_refreshes: Mutex::new(std::collections::HashSet::new()),
                claude_credentials: Mutex::new(None),
                callback_routes,
                identities: Mutex::new(HashMap::new()),
                cli_overrides: Mutex::new(HashMap::new()),
            }),
        }
    }

    /// Test seam: run `harness`'s sign-in with the CLI at `path` instead of
    /// the one PATH resolution finds.
    #[doc(hidden)]
    pub fn override_cli(&self, harness: HarnessId, path: impl Into<PathBuf>) {
        lock(&self.inner.cli_overrides).insert(harness, path.into());
    }

    /// The ACP harness for `harness`'s CLI, honouring [`Self::override_cli`].
    fn acp_harness(&self, harness: HarnessId) -> Option<zeron_harness::AcpHarness> {
        let acp = match harness {
            HarnessId::Grok => zeron_harness::AcpHarness::grok(),
            HarnessId::Devin => zeron_harness::AcpHarness::devin(),
            HarnessId::Hermes => zeron_harness::AcpHarness::hermes(),
            HarnessId::Antigravity => zeron_harness::AcpHarness::antigravity(),
            _ => return None,
        };
        Some(match lock(&self.inner.cli_overrides).get(&harness) {
            Some(path) => acp.with_executable(path.clone()),
            None => acp,
        })
    }

    // ── list ────────────────────────────────────────────────────────────────

    /// Detect the CLIs, auto-snapshot the live logins, and assemble the view.
    ///
    /// Usage is stale-while-revalidate: every list serves each account's last
    /// good probe (memory, seeded from disk) with its fetch time, so a plain
    /// list is offline-fast. `force_usage` additionally re-probes — all
    /// accounts concurrently — skipping any still inside a Retry-After /
    /// backoff window or probed in the last [`FORCED_MIN_INTERVAL`].
    pub async fn list(&self, force_usage: bool) -> Result<AgentAccountsSnapshot, EngineError> {
        let _ops = self.inner.ops.lock().await;
        self.list_locked(force_usage).await
    }

    /// Caller holds [`Inner::ops`].
    async fn list_locked(&self, force_usage: bool) -> Result<AgentAccountsSnapshot, EngineError> {
        let mut warnings: Vec<AgentAccountWarning> = Vec::new();
        // Per agent, the live logins' account keys — one for single-login
        // agents, one per model provider for OpenCode / Pi.
        let mut active_keys: HashMap<HarnessId, std::collections::HashSet<String>> = HashMap::new();
        // Live logins listed without a slot: credentials unreadable (Keychain
        // denied) or identity unresolvable (an opaque token whose profile
        // call failed). Active, never switchable.
        let mut unreadable: Vec<(HarnessId, Detected)> = Vec::new();
        let mut live = |harness: HarnessId, detected: &Detected| {
            active_keys
                .entry(harness)
                .or_default()
                .insert(detected.account_key.clone());
        };

        let (claude, claude_warning) = self.detect_claude().await;
        if let Some(message) = claude_warning {
            warnings.push(AgentAccountWarning {
                harness: HarnessId::ClaudeCode,
                message,
            });
        }
        if let Some(detected) = claude {
            live(HarnessId::ClaudeCode, &detected);
            if detected.credentials.is_some() {
                self.snapshot_detected(HarnessId::ClaudeCode, &detected)?;
            } else {
                unreadable.push((HarnessId::ClaudeCode, detected));
            }
        }
        self.migrate_codex_slot_keys()?;
        if let Some(detected) = self.detect_codex() {
            live(HarnessId::Codex, &detected);
            self.snapshot_detected(HarnessId::Codex, &detected)?;
        }
        if let Some(detected) = self.detect_cursor() {
            live(HarnessId::Cursor, &detected);
            self.snapshot_detected(HarnessId::Cursor, &detected)?;
            // The SDK's minted keys expire (90-day default) — an expired live
            // key fails every run with an auth error, so say so up front.
            if !self.cursor_live_usable() {
                warnings.push(AgentAccountWarning {
                    harness: HarnessId::Cursor,
                    message: "The connected Cursor login's API key has expired — connect again."
                        .into(),
                });
            }
        }
        let mut detected_more: Vec<(HarnessId, Detected)> = Vec::new();
        detected_more.extend(self.detect_grok().map(|d| (HarnessId::Grok, d)));
        detected_more.extend(self.detect_devin().map(|d| (HarnessId::Devin, d)));
        for harness in [HarnessId::Opencode, HarnessId::Pi] {
            let (resolved, unresolved) = self.detect_keyed(harness).await;
            detected_more.extend(resolved.into_iter().map(|d| (harness, d)));
            for detected in unresolved {
                live(harness, &detected);
                unreadable.push((harness, detected));
            }
        }
        for (harness, detected) in &detected_more {
            live(*harness, detected);
            self.snapshot_detected(*harness, detected)?;
        }
        if let Some(message) = stores::opencode_env_warning() {
            warnings.push(AgentAccountWarning {
                harness: HarnessId::Opencode,
                message,
            });
        }
        let antigravity = self.detect_antigravity().await;

        // Stable presentation order: provider, then slot creation order (never
        // active-first — switching must not reshuffle the cards). Hermes'
        // rows are its own credential pool, read live (never copied).
        let mut providers: Vec<(HarnessId, Vec<Slot>)> = [
            HarnessId::ClaudeCode,
            HarnessId::Codex,
            HarnessId::Cursor,
            HarnessId::Grok,
            HarnessId::Devin,
            HarnessId::Opencode,
            HarnessId::Pi,
        ]
        .into_iter()
        .map(|harness| (harness, self.read_slots(harness)))
        .collect();
        let (hermes, hermes_active) = self.hermes_slots();
        active_keys.insert(HarnessId::Hermes, hermes_active);
        providers.push((HarnessId::Hermes, hermes));
        if force_usage {
            let targets: Vec<(HarnessId, &Slot, bool)> = providers
                .iter()
                .flat_map(|(harness, slots)| {
                    let active = active_keys.get(harness);
                    slots.iter().map(move |slot| {
                        (
                            *harness,
                            slot,
                            active.is_some_and(|keys| keys.contains(&slot.account_key)),
                        )
                    })
                })
                .collect();
            self.refresh_usage(&targets).await;
            // A probe can teach a slot who it is (Devin's key file names no
            // one): show that in this very list.
            for (harness, slots) in providers.iter_mut() {
                if *harness == HarnessId::Devin {
                    *slots = self.read_slots(*harness);
                }
            }
        }
        let now = now_ms();
        let usage = lock(&self.inner.usage).clone();
        let mut accounts: Vec<AgentAccount> = Vec::new();
        for (harness, slots) in &providers {
            let harness = *harness;
            let active_set = active_keys.get(&harness);
            for slot in slots {
                let active = active_set.is_some_and(|keys| keys.contains(&slot.account_key));
                let entry = usage.get(&usage_key(harness, &slot.account_key));
                let snapshot = entry.and_then(|entry| entry.usage.as_ref());
                accounts.push(AgentAccount {
                    id: slot.id.clone(),
                    harness,
                    email: Some(slot.profile.email.clone()),
                    // A live plan from the usage probe (Codex `plan_type`)
                    // supersedes the login-time snapshot; fall back to the
                    // snapshot when no probe has succeeded yet.
                    plan_label: snapshot
                        .and_then(|usage| usage.plan_label.clone())
                        .or_else(|| slot.profile.plan.clone()),
                    active,
                    usage_windows: snapshot
                        .map(|usage| usage.windows.clone())
                        .unwrap_or_default(),
                    usage_fetched_at: entry.and_then(|entry| entry.fetched_at),
                    usage_error: entry.and_then(|entry| {
                        usage_error_message(
                            harness,
                            slot.store_key.as_deref(),
                            active,
                            entry.error.as_ref()?,
                            entry,
                            now,
                        )
                    }),
                    display_name: slot.profile.display_name.clone(),
                    organization: slot.profile.organization.clone(),
                    auth_kind: Some(slot.profile.auth_kind),
                    // Hermes' pool is Hermes' to order (see module docs).
                    switchable: harness != HarnessId::Hermes,
                    saved_at: (harness != HarnessId::Hermes).then_some(slot.saved_at),
                    provider: provider_group(harness, slot.store_key.as_deref()),
                });
            }
            // A live login whose credentials we couldn't read has no slot — still
            // show it (active, but not re-activatable until the Keychain relents).
            for (_, u) in unreadable.iter().filter(|(h, _)| *h == harness) {
                if slots.iter().any(|s| s.account_key == u.account_key) {
                    continue;
                }
                accounts.push(AgentAccount {
                    id: slot_id_for(harness, &u.account_key),
                    harness,
                    email: Some(u.profile.email.clone()),
                    plan_label: u.profile.plan.clone(),
                    active: true,
                    usage_windows: Vec::new(),
                    usage_fetched_at: None,
                    usage_error: None,
                    display_name: u.profile.display_name.clone(),
                    organization: u.profile.organization.clone(),
                    auth_kind: Some(u.profile.auth_kind),
                    switchable: false,
                    saved_at: None,
                    provider: provider_group(harness, u.store_key.as_deref()),
                });
            }
        }
        // Antigravity keeps exactly one login whose token zeron never reads:
        // one active, unswitchable row, and no usage (it has no quota view).
        if let Some(login) = antigravity {
            accounts.push(login.account());
        }
        Ok(AgentAccountsSnapshot { accounts, warnings })
    }

    // ── swap ────────────────────────────────────────────────────────────────

    /// Swap the CLI's live login to a saved slot. Detection runs first, so the
    /// CURRENT login is snapshotted into its slot before being overwritten (the
    /// claude-swap trick — a swap never strands the session it replaces).
    ///
    /// Refused for Hermes (and by [`Self::forget`]): Hermes owns its
    /// credential pool and rotates through it itself — a running Hermes
    /// would write its own order back over any change — so zeron keeps it
    /// read-only (see [`stores`]).
    pub async fn activate(
        &self,
        harness: HarnessId,
        account_id: &str,
    ) -> Result<AgentAccountsSnapshot, EngineError> {
        if harness == HarnessId::Hermes {
            return Err(EngineError::Other(
                "Hermes picks from its own credential pool — zeron doesn't reorder it. Use \
                 `hermes auth` to manage it."
                    .into(),
            ));
        }
        let _ops = self.inner.ops.lock().await;
        // The pre-swap snapshot must hold the CURRENT tokens (the CLI may have
        // rotated its refresh token seconds ago) — never a cached read.
        *lock(&self.inner.claude_credentials) = None;
        self.list_locked(false).await?;
        let slot = self
            .read_slots(harness)
            .into_iter()
            .find(|s| s.id == account_id)
            .ok_or_else(|| {
                EngineError::Other(
                    "That saved login no longer exists — refresh and try again.".into(),
                )
            })?;
        match harness {
            HarnessId::ClaudeCode => self.activate_claude(&slot).await?,
            HarnessId::Codex => self.activate_codex(&slot)?,
            HarnessId::Cursor => self.write_cursor_auth(&slot.credentials)?,
            HarnessId::Grok => {
                self.write_grok_entry(slot.store_key.as_deref(), &slot.credentials)?
            }
            HarnessId::Devin => self.write_devin_credentials(&slot.credentials)?,
            HarnessId::Opencode | HarnessId::Pi => {
                let key = slot.store_key.as_deref().ok_or_else(|| {
                    EngineError::Other("That saved login names no provider.".into())
                })?;
                self.write_keyed_entry(harness, key, Some(&slot.credentials))?;
            }
            other => {
                return Err(EngineError::Other(format!(
                    "agent accounts are not supported for {other:?}"
                )));
            }
        }
        *lock(&self.inner.claude_credentials) = None;
        self.list_locked(false).await
    }

    async fn activate_claude(&self, slot: &Slot) -> Result<(), EngineError> {
        // Slot owns the account login; live owns MCP/plugin OAuth that lives in
        // the same blob. A wholesale replace would restore stale (or empty)
        // mcpOAuth from the target snapshot and force every MCP to re-auth.
        let (live, _) = self.read_claude_credentials().await;
        let credentials = compose_claude_credentials(&slot.credentials, live.as_ref());
        self.write_claude_credentials(&credentials).await?;
        // Merge the identity back into ~/.claude.json — everything else (caches,
        // project history, onboarding flags, mcpServers) is left untouched, which
        // is all Claude Code needs to treat this as a fresh login.
        //
        // GUARD the merge: a parse failure on an EXISTING file means "don't touch
        // it", not "start fresh" — writing only our identity fields would destroy
        // the user's entire Claude config. Only a missing file may start from {}.
        let file = &self.inner.config.claude_config_file;
        let cfg = read_json(file);
        if cfg.is_none() && file.exists() {
            return Err(EngineError::Other(
                "~/.claude.json exists but could not be parsed — not switching to avoid wiping \
                 it. Fix or remove the file and try again."
                    .into(),
            ));
        }
        let mut merged = cfg.unwrap_or_else(|| serde_json::json!({}));
        let map = merged.as_object_mut().ok_or_else(|| {
            EngineError::Other("~/.claude.json is not a JSON object — not switching.".into())
        })?;
        let (oauth_account, user_id) = match &slot.claude_config {
            Some(cc) => (cc.get("oauthAccount").cloned(), cc.get("userID").cloned()),
            None => (None, None),
        };
        map.insert(
            "oauthAccount".into(),
            oauth_account.unwrap_or_else(|| {
                serde_json::json!({
                    "accountUuid": slot.account_key,
                    "emailAddress": slot.profile.email,
                    "organizationName": slot.profile.organization,
                    "displayName": slot.profile.display_name,
                })
            }),
        );
        match user_id.filter(|v| v.is_string()) {
            Some(user_id) => {
                map.insert("userID".into(), user_id);
            }
            None => {
                map.remove("userID");
            }
        }
        // Atomic: Claude Code rewrites this file frequently — a torn write from
        // our side must never be readable as "empty config".
        write_file_atomic(file, merged.to_string().as_bytes(), false)
    }

    fn activate_codex(&self, slot: &Slot) -> Result<(), EngineError> {
        std::fs::create_dir_all(&self.inner.config.codex_home)?;
        let json = serde_json::to_string_pretty(&slot.credentials)
            .map_err(|e| EngineError::Other(format!("serialize codex auth: {e}")))?;
        write_file_atomic(&self.inner.config.codex_auth_file(), json.as_bytes(), true)
    }

    /// The live login's account key, from the identity alone (no secret read
    /// for Claude). `store_key` names the entry for agents that keep one
    /// login per model provider (OpenCode, Pi); an entry that is there but
    /// can't be identified has no key.
    async fn live_account_key(
        &self,
        harness: HarnessId,
        store_key: Option<&str>,
    ) -> Option<String> {
        match harness {
            HarnessId::ClaudeCode => {
                let cfg = read_json(&self.inner.config.claude_config_file)?;
                let oauth = cfg.get("oauthAccount")?;
                str_field(oauth, "accountUuid").or_else(|| str_field(oauth, "emailAddress"))
            }
            HarnessId::Codex => self.detect_codex().map(|d| d.account_key),
            HarnessId::Cursor => self.detect_cursor().map(|d| d.account_key),
            HarnessId::Grok => self.detect_grok().map(|d| d.account_key),
            HarnessId::Devin => self.detect_devin().map(|d| d.account_key),
            HarnessId::Opencode | HarnessId::Pi => self
                .detect_keyed_entry(harness, store_key?)
                .await
                .flatten()
                .map(|d| d.account_key),
            _ => None,
        }
    }

    /// Whether a per-provider agent has a live entry under `store_key` at
    /// all, identified or not — a live login zeron can't identify is still
    /// never replaced unasked.
    fn has_live_entry(&self, harness: HarnessId, store_key: Option<&str>) -> bool {
        match harness {
            HarnessId::Opencode | HarnessId::Pi => {
                store_key.is_none_or(|key| self.live_keyed_entry(harness, key).is_some())
            }
            _ => false,
        }
    }

    /// A fresh sign-in replaces the live login when it IS the live account
    /// (a re-login — otherwise the next list would snapshot the old, possibly
    /// revoked, live tokens straight back over the fresh slot) or when there
    /// is no live login at all (nothing to strand; "add" must mean "works").
    /// Any other live login stays untouched — switching remains explicit.
    async fn adopt_if_live(&self, slot: &Slot) -> Result<(), EngineError> {
        let store_key = slot.store_key.as_deref();
        let live = self.live_account_key(slot.harness, store_key).await;
        let usable = match slot.harness {
            HarnessId::Cursor => self.cursor_live_usable(),
            _ => live.is_some() || self.has_live_entry(slot.harness, store_key),
        };
        if usable && live.as_deref() != Some(slot.account_key.as_str()) {
            return Ok(());
        }
        match slot.harness {
            HarnessId::ClaudeCode => self.activate_claude(slot).await?,
            HarnessId::Codex => self.activate_codex(slot)?,
            HarnessId::Cursor => self.write_cursor_auth(&slot.credentials)?,
            HarnessId::Grok => self.write_grok_entry(store_key, &slot.credentials)?,
            HarnessId::Devin => self.write_devin_credentials(&slot.credentials)?,
            HarnessId::Opencode | HarnessId::Pi => {
                if let Some(key) = store_key {
                    self.write_keyed_entry(slot.harness, key, Some(&slot.credentials))?;
                }
            }
            _ => {}
        }
        *lock(&self.inner.claude_credentials) = None;
        Ok(())
    }

    /// Sign the CLI's live login out, so removing its slot sticks.
    async fn sign_out(
        &self,
        harness: HarnessId,
        store_key: Option<&str>,
        expected_account_key: &str,
    ) -> Result<(), EngineError> {
        if self.live_account_key(harness, store_key).await.as_deref() != Some(expected_account_key)
        {
            return Err(EngineError::Other(
                "The live login changed while it was being removed — refresh and try again.".into(),
            ));
        }
        match harness {
            HarnessId::ClaudeCode => {
                let (live, warning) = self.read_claude_credentials().await;
                if self.live_account_key(harness, None).await.as_deref()
                    != Some(expected_account_key)
                {
                    return Err(EngineError::Other(
                        "The live login changed while it was being removed — refresh and try again."
                            .into(),
                    ));
                }
                if let Some(mut credentials) = live {
                    // Only the account login goes — machine-shared MCP/plugin
                    // OAuth in the same blob stays.
                    if let Some(map) = credentials.as_object_mut() {
                        map.remove("claudeAiOauth");
                    }
                    self.write_claude_credentials(&credentials).await?;
                } else if let Some(warning) = warning {
                    return Err(EngineError::Other(format!(
                        "Couldn't sign Claude out: {warning}"
                    )));
                }
                let file = &self.inner.config.claude_config_file;
                if let Some(mut cfg) = read_json(file)
                    && let Some(map) = cfg.as_object_mut()
                    && map.remove("oauthAccount").is_some()
                {
                    write_file_atomic(file, cfg.to_string().as_bytes(), false)?;
                }
                *lock(&self.inner.claude_credentials) = None;
            }
            HarnessId::Codex => remove_if_exists(&self.inner.config.codex_auth_file())?,
            HarnessId::Cursor => remove_if_exists(&self.inner.config.cursor_sdk_auth_file)?,
            // Only this login's own entry goes: other issuers (Grok) and
            // other providers' logins and API keys (OpenCode, Pi) stay.
            HarnessId::Grok => {
                let map_key = self
                    .detect_grok()
                    .and_then(|d| d.store_key)
                    .ok_or_else(|| {
                        EngineError::Other("Couldn't find grok's live login to sign out.".into())
                    })?;
                self.remove_grok_entry(&map_key)?;
            }
            HarnessId::Devin => remove_if_exists(&self.inner.config.devin_credentials_file)?,
            HarnessId::Opencode | HarnessId::Pi => {
                let key = store_key
                    .ok_or_else(|| EngineError::Other("That login names no provider.".into()))?;
                self.write_keyed_entry(harness, key, None)?;
            }
            _ => {
                return Err(EngineError::Other(
                    "That's the live login — it can't be removed here.".into(),
                ));
            }
        }
        Ok(())
    }

    // ── forget ──────────────────────────────────────────────────────────────

    pub async fn forget(
        &self,
        harness: HarnessId,
        account_id: &str,
    ) -> Result<AgentAccountsSnapshot, EngineError> {
        // Reject anything that isn't a slot id (16 lowercase hex) BEFORE touching
        // the filesystem: `account_id` is a raw RPC string that becomes a path,
        // so a crafted id (`../../…`) must never reach `remove_file`.
        if account_id.len() != 16
            || !account_id
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(EngineError::Other("Unknown account.".into()));
        }
        if harness == HarnessId::Hermes {
            // Hermes' rows are its own pool, not zeron slots.
            return Err(EngineError::Other(
                "Hermes keeps this login in its own credential pool — remove it with \
                 `hermes auth remove`."
                    .into(),
            ));
        }
        let _ops = self.inner.ops.lock().await;
        let snapshot = self.list_locked(false).await?;
        let row = snapshot
            .accounts
            .iter()
            .find(|a| a.harness == harness && a.id == account_id);
        // Per-provider agents (OpenCode, Pi): the store entry this row is.
        let store_key = row.and_then(|a| a.provider.clone());
        if row.is_some_and(|a| a.active) {
            // Removing the live login signs the CLI out — dropping only the
            // slot would re-detect (and re-snapshot) it on the next list, and
            // the only account must stay removable.
            let expected = self
                .live_account_key(harness, store_key.as_deref())
                .await
                .ok_or_else(|| {
                    EngineError::Other(
                    "The live login changed while it was being removed — refresh and try again."
                        .into(),
                )
                })?;
            if slot_id_for(harness, &expected) != account_id {
                return Err(EngineError::Other(
                    "The live login changed while it was being removed — refresh and try again."
                        .into(),
                ));
            }
            self.sign_out(harness, store_key.as_deref(), &expected)
                .await?;
        }
        let file = self.slots_dir(harness)?.join(format!("{account_id}.json"));
        if file.exists() {
            std::fs::remove_file(&file)?;
        }
        self.list_locked(false).await
    }

    // ── add-account OAuth flows ─────────────────────────────────────────────

    pub async fn start_login(&self, harness: HarnessId) -> Result<AgentLoginStart, EngineError> {
        self.start_login_for(harness, None).await
    }

    /// [`Self::start_login`] on behalf of `requester` — another device, whose
    /// browser finishes the sign-in. The login's loopback callback port is
    /// published for that device alone, for as long as the login runs.
    pub async fn start_login_for(
        &self,
        harness: HarnessId,
        requester: Option<&str>,
    ) -> Result<AgentLoginStart, EngineError> {
        self.start_login_with(harness, None, requester).await
    }

    /// [`Self::start_login_for`] for one model provider of an agent that
    /// keeps a login per provider (`provider`: OpenCode's `openai` /
    /// `github-copilot`, Pi's `openai-codex`, Hermes' `openai-codex` /
    /// `nous`); `None` picks the agent's default.
    pub async fn start_login_with(
        &self,
        harness: HarnessId,
        provider: Option<&str>,
        requester: Option<&str>,
    ) -> Result<AgentLoginStart, EngineError> {
        self.sweep_flows();
        let provider = provider.filter(|p| !p.is_empty());
        let mut start = match harness {
            HarnessId::ClaudeCode => {
                // A new start supersedes any earlier Claude flow and its
                // loopback listener (the others reap the same way).
                self.reap_spawned_flows(HarnessId::ClaudeCode);
                self.start_claude_login().await
            }
            HarnessId::Codex => self.start_codex_login(requester).await?,
            HarnessId::Cursor => self.start_cursor_login().await?,
            HarnessId::Antigravity => self.start_antigravity_login(requester),
            HarnessId::Grok => self.start_grok_login(requester).await?,
            HarnessId::Devin => self.start_devin_login(requester)?,
            HarnessId::Opencode => match provider.unwrap_or("openai") {
                "openai" => {
                    self.start_openai_login(HarnessId::Opencode, "openai")
                        .await?
                }
                "github-copilot" => self.start_copilot_login(HarnessId::Opencode).await?,
                other => return Err(stores::unsupported_login(harness, other)),
            },
            HarnessId::Pi => match provider.unwrap_or("openai-codex") {
                "openai-codex" => {
                    self.start_openai_login(HarnessId::Pi, "openai-codex")
                        .await?
                }
                other => return Err(stores::unsupported_login(harness, other)),
            },
            HarnessId::Hermes => {
                let provider = provider.unwrap_or("openai-codex");
                if !stores::HERMES_LOGINS.contains(&provider) {
                    return Err(stores::unsupported_login(harness, provider));
                }
                self.start_hermes_login(provider).await?
            }
            other => {
                return Err(EngineError::Other(format!(
                    "agent logins are not supported for {other:?}"
                )));
            }
        };
        if start.callback_port.is_none() {
            start.callback_port = loopback_port(&start.url);
        }
        if let (Some(requester), Some(port)) = (requester, start.callback_port) {
            self.inner
                .callback_routes
                .register(&start.login_id, port, requester, FLOW_TTL);
        }
        Ok(start)
    }

    /// Claude: the CLI's own automatic login — PKCE against a loopback
    /// `localhost:<port>/callback` we serve, finishing when the browser lands
    /// there. Pasting the code is the fallback when no port can be bound.
    async fn start_claude_login(&self) -> AgentLoginStart {
        match tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await {
            Ok(listener) => match listener.local_addr() {
                Ok(address) => self.start_claude_loopback_login(listener, address.port()),
                Err(error) => {
                    tracing::warn!(%error, "Claude login callback has no port; pasting the code instead");
                    self.start_claude_paste_login()
                }
            },
            Err(error) => {
                tracing::warn!(%error, "no loopback port for the Claude login; pasting the code instead");
                self.start_claude_paste_login()
            }
        }
    }

    fn start_claude_loopback_login(
        &self,
        listener: tokio::net::TcpListener,
        port: u16,
    ) -> AgentLoginStart {
        let login_id = new_id();
        let (verifier, challenge) = pkce_pair();
        // 32 random bytes, like the CLI's own `state` — Anthropic's authorize
        // page rejects a shorter one as "Invalid request format".
        let state = random_url_token();
        let redirect = format!("http://localhost:{port}/callback");
        let url = format!(
            "{CLAUDE_LOOPBACK_AUTHORIZE_URL}?code=true&client_id={CLAUDE_CLIENT_ID}\
             &response_type=code&redirect_uri={}&scope={}&code_challenge={challenge}\
             &code_challenge_method=S256&state={state}",
            urlencode(&redirect),
            urlencode(CLAUDE_LOOPBACK_SCOPES),
        );
        let task_state = Arc::new(Mutex::new(TaskLoginState {
            url: Some(url.clone()),
            ..Default::default()
        }));
        let this = self.clone();
        let outcome_state = task_state.clone();
        let handle = tokio::spawn(async move {
            let outcome = this
                .finish_claude_loopback_login(listener, &state, &verifier, &redirect)
                .await;
            lock(&outcome_state).outcome = Some(outcome.map_err(|e| e.to_string()));
        });
        lock(&self.inner.flows).insert(
            login_id.clone(),
            LoginFlow::Task {
                harness: HarnessId::ClaudeCode,
                started_at: Instant::now(),
                state: task_state,
                handle,
                home: None,
                port: None,
            },
        );
        AgentLoginStart {
            login_id,
            url,
            mode: AgentLoginMode::Browser,
            callback_port: Some(port),
        }
    }

    /// Wait for the browser on the loopback callback, redeem its code, and
    /// answer the browser: the CLI's success page, or why it failed.
    async fn finish_claude_loopback_login(
        &self,
        listener: tokio::net::TcpListener,
        state: &str,
        verifier: &str,
        redirect: &str,
    ) -> Result<(), EngineError> {
        use tokio::io::AsyncWriteExt as _;
        let (callback, mut browser) = await_claude_callback(&listener, state).await;
        drop(listener);
        let result = match callback {
            Ok(code) => {
                self.redeem_claude_code(
                    &self.inner.endpoints.claude_loopback_token,
                    &code,
                    state,
                    verifier,
                    redirect,
                    CLAUDE_LOOPBACK_SCOPES,
                )
                .await
            }
            Err(message) => Err(EngineError::Other(message)),
        };
        let response = match &result {
            Ok(()) => http_response(
                "302 Found",
                &[("Location", CLAUDE_LOOPBACK_SUCCESS_URL)],
                "",
            ),
            Err(error) => http_response(
                "400 Bad Request",
                &[("Content-Type", "text/html; charset=utf-8")],
                &format!(
                    "<!doctype html><title>Sign-in failed</title><p>{}</p>\
                     <p>Return to Zeron to try again.</p>",
                    html_escape(&zeron_harness::redact::redact_output(&error.to_string()))
                ),
            ),
        };
        let _ = browser.write_all(response.as_bytes()).await;
        let _ = browser.shutdown().await;
        result
    }

    /// The paste-code fallback: Anthropic's manual redirect shows the code
    /// for the user to paste back ([`Self::complete_login`]).
    fn start_claude_paste_login(&self) -> AgentLoginStart {
        let login_id = new_id();
        let (verifier, challenge) = pkce_pair();
        let state = random_url_token();
        let url = format!(
            "https://claude.ai/oauth/authorize?code=true&client_id={CLAUDE_CLIENT_ID}\
             &response_type=code&redirect_uri={}&scope={}&code_challenge={challenge}\
             &code_challenge_method=S256&state={state}",
            urlencode(CLAUDE_REDIRECT),
            urlencode(CLAUDE_SCOPES),
        );
        lock(&self.inner.flows).insert(
            login_id.clone(),
            LoginFlow::Claude {
                verifier,
                state,
                started_at: Instant::now(),
            },
        );
        AgentLoginStart {
            login_id,
            url,
            mode: AgentLoginMode::PasteCode,
            callback_port: None,
        }
    }

    /// Supersede — and reap — any pending spawned flow for `harness` (codex:
    /// `codex login` binds a fixed loopback OAuth port, so a lingering flow
    /// makes every retry exit on EADDRINUSE; cursor: one flow is simply the
    /// sane state).
    fn reap_spawned_flows(&self, harness: HarnessId) {
        let stale: Vec<String> = lock(&self.inner.flows)
            .iter()
            .filter(|(_, f)| {
                matches!(
                    f,
                    LoginFlow::Spawned { harness: h, .. } | LoginFlow::Task { harness: h, .. }
                        if *h == harness
                )
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in stale {
            self.cancel_login(&id);
        }
    }

    /// Cancel every pending flow holding the fixed loopback `port` — the
    /// ChatGPT sign-ins (codex's, OpenCode's, Pi's) all need 1455, and a
    /// lingering one makes every retry fail to bind.
    fn reap_port_flows(&self, port: u16) {
        let stale: Vec<String> = lock(&self.inner.flows)
            .iter()
            .filter(|(_, f)| match f {
                LoginFlow::Task { port: Some(p), .. } => *p == port,
                LoginFlow::Spawned {
                    harness: HarnessId::Codex,
                    ..
                } => port == oauth::OPENAI_LOOPBACK_PORT,
                _ => false,
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in stale {
            self.cancel_login(&id);
        }
    }

    /// Spawn a CLI login child, register it as a [`LoginFlow::Spawned`], and
    /// wait briefly for the sign-in page it prints. `home` is the flow's
    /// throwaway dir — reclaimed on any failure here, and on cancel/finish.
    #[allow(clippy::too_many_arguments)]
    async fn spawn_login_child(
        &self,
        login_id: String,
        harness: HarnessId,
        mut command: zeron_harness::process::Command,
        home: PathBuf,
        completion: SpawnedCompletion,
        scan_url: fn(&str) -> Option<String>,
        requester: Option<&str>,
    ) -> Result<AgentLoginStart, EngineError> {
        command
            .stdin(zeron_harness::process::Stdio::null())
            .stdout(zeron_harness::process::Stdio::piped())
            .stderr(zeron_harness::process::Stdio::piped());
        let child = match command.spawn() {
            Ok(child) => child,
            Err(err) => {
                let _ = std::fs::remove_dir_all(&home);
                let cli = stores::cli_name(harness);
                return Err(EngineError::Other(
                    if err.kind() == std::io::ErrorKind::NotFound {
                        format!("The `{cli}` CLI was not found on this device — install it first.")
                    } else {
                        format!("Could not start the {cli} sign-in: {err}")
                    },
                ));
            }
        };
        let (child, output, exit) = wire_login_child(child);
        lock(&self.inner.flows).insert(
            login_id.clone(),
            LoginFlow::Spawned {
                harness,
                child,
                home,
                completion,
                started_at: Instant::now(),
                output: output.clone(),
                exit: exit.clone(),
                scan_url,
                requester: requester.map(str::to_string),
            },
        );
        let url = await_login_url(&output, &exit, scan_url).await;
        Ok(AgentLoginStart {
            login_id,
            url,
            mode: AgentLoginMode::Browser,
            callback_port: None,
        })
    }

    async fn start_codex_login(
        &self,
        requester: Option<&str>,
    ) -> Result<AgentLoginStart, EngineError> {
        self.reap_spawned_flows(HarnessId::Codex);
        // `codex login` binds the same fixed port as the ChatGPT sign-ins.
        self.reap_port_flows(oauth::OPENAI_LOOPBACK_PORT);
        let login_id = new_id();
        // A throwaway CODEX_HOME isolates the new login completely — the live
        // ~/.codex session is never touched until the user explicitly switches.
        let home = self.login_home(&login_id)?;
        // Resolve through the harness itself (`CODEX_EXECUTABLE`, PATH, the
        // login-shell snapshot, install dirs — the Windows npm payload
        // included) and compose the same child PATH a chat run gets, so
        // account login never diverges from what the harness can launch.
        let mut command = match zeron_harness::codex::login_command(&home) {
            Ok(command) => command,
            Err(err) => {
                let _ = std::fs::remove_dir_all(&home);
                return Err(EngineError::Other(match err {
                    zeron_harness::HarnessError::NotInstalled(hint) => {
                        format!(
                            "The `codex` CLI was not found on this device — install it first. ({hint})"
                        )
                    }
                    other => format!("Could not resolve the codex CLI for login: {other}"),
                }));
            }
        };
        command
            .stdin(zeron_harness::process::Stdio::null())
            .stdout(zeron_harness::process::Stdio::piped())
            .stderr(zeron_harness::process::Stdio::piped());
        // The CLI opens the authorization tab itself (via the `webbrowser`
        // crate) AND the app opens the page when this start reply lands —
        // users got TWO identical auth.openai.com tabs. `webbrowser` prefers
        // $BROWSER over xdg-open, so a no-op script there keeps the CLI's
        // open quiet; a failed open is advisory to `codex login` (it prints
        // the URL and keeps serving the loopback callback either way).
        #[cfg(unix)]
        if let Some(noop_browser) = ensure_noop_browser(&self.inner.config.root_dir()) {
            command.env("BROWSER", noop_browser);
        }
        // codex prints the authorize URL (to stderr as of 0.142 — scan both
        // streams); grab it so the app can open the single authorization tab
        // (the CLI's own browser-open is suppressed via BROWSER above).
        self.spawn_login_child(
            login_id,
            HarnessId::Codex,
            command,
            home,
            SpawnedCompletion::CredentialFile,
            scan_openai_url,
            requester,
        )
        .await
    }

    /// Antigravity: its ACP server's own Google sign-in. The start replies at
    /// once — a first sign-in may download a large server — and polls carry
    /// the browser url once the server prints it. A server that already holds
    /// a valid token answers `authenticate` without any browser at all, which
    /// is success: the poll reports done and the list shows the login.
    fn start_antigravity_login(&self, requester: Option<&str>) -> AgentLoginStart {
        self.reap_spawned_flows(HarnessId::Antigravity);
        let login_id = new_id();
        let state = Arc::new(Mutex::new(TaskLoginState {
            requester: requester.map(str::to_string),
            ..Default::default()
        }));
        #[cfg(unix)]
        let browser = {
            self.inner
                .config
                .private_root()
                .ok()
                .and_then(|root| ensure_noop_browser(&root))
        };
        #[cfg(not(unix))]
        let browser = None;
        let home = self.inner.config.antigravity_home.clone();
        let keychain = self.inner.config.antigravity_keychain;
        let task_state = state.clone();
        let handle = tokio::spawn(async move {
            let progress_state = task_state.clone();
            let mut outcome = zeron_harness::AcpHarness::antigravity()
                .sign_in(browser, move |progress| match progress {
                    zeron_harness::acp::SignInProgress::OpenBrowser(url) => {
                        lock(&progress_state).url = Some(url);
                    }
                })
                .await
                .map_err(|e| e.to_string());
            // A success the list can't show would drop the user back at
            // "Connect" with no word why — say where the login went missing.
            if outcome.is_ok()
                && let Some(home) = &home
                && detect_antigravity_login(home, keychain).await.is_none()
            {
                outcome = Err(format!(
                    "Antigravity reported a successful sign-in, but no login was saved in {}.",
                    home.join("antigravity-acp").display()
                ));
            }
            lock(&task_state).outcome = Some(outcome);
        });
        lock(&self.inner.flows).insert(
            login_id.clone(),
            LoginFlow::Task {
                harness: HarnessId::Antigravity,
                started_at: Instant::now(),
                state,
                handle,
                home: None,
                port: None,
            },
        );
        AgentLoginStart {
            login_id,
            url: String::new(),
            mode: AgentLoginMode::Browser,
            callback_port: None,
        }
    }

    /// Cursor: the SDK's own PKCE browser flow, driven through the zeron shim
    /// in login mode. The minted key lands in a throwaway store file (never
    /// the live `~/.cursor/sdk/auth.json`), then snapshots into a slot on
    /// poll — mirroring codex's throwaway `CODEX_HOME`.
    async fn start_cursor_login(&self) -> Result<AgentLoginStart, EngineError> {
        self.reap_spawned_flows(HarnessId::Cursor);
        let login_id = new_id();
        let home = self.login_home(&login_id)?;
        let cmd = zeron_harness::cursor::login_command(&home.join("auth.json"))
            .await
            .map_err(|e| {
                let _ = std::fs::remove_dir_all(&home);
                EngineError::Other(format!("Could not start the Cursor login: {e}"))
            })?;
        self.spawn_login_child(
            login_id,
            HarnessId::Cursor,
            cmd,
            home,
            SpawnedCompletion::CredentialFile,
            scan_cursor_url,
            None,
        )
        .await
    }

    /// Exchange the pasted `code#state` for tokens and save the account as a
    /// slot (see [`Self::adopt_if_live`] for when it also becomes live).
    pub async fn complete_login(
        &self,
        login_id: &str,
        code: &str,
    ) -> Result<AgentAccountsSnapshot, EngineError> {
        let (verifier, expected_state) = match lock(&self.inner.flows).get(login_id) {
            Some(LoginFlow::Claude {
                verifier, state, ..
            }) => (verifier.clone(), state.clone()),
            _ => {
                return Err(EngineError::Other(
                    "This sign-in attempt expired — start again.".into(),
                ));
            }
        };
        let (auth_code, state) = match code.trim().split_once('#') {
            Some((c, s)) => (c.to_string(), s.to_string()),
            None => (code.trim().to_string(), expected_state.clone()),
        };
        if state != expected_state {
            return Err(EngineError::Other(
                "That code belongs to a different sign-in — start again.".into(),
            ));
        }
        if auth_code.is_empty() {
            return Err(EngineError::Other(
                "That code looks empty — paste the whole code.".into(),
            ));
        }
        self.redeem_claude_code(
            CLAUDE_TOKEN_URL,
            &auth_code,
            &state,
            &verifier,
            CLAUDE_REDIRECT,
            CLAUDE_SCOPES,
        )
        .await?;
        self.remove_flow(login_id);
        self.list(false).await
    }

    /// Redeem a Claude authorization code at `token_url` and save the account
    /// as a slot (see [`Self::adopt_if_live`] for when it also becomes live).
    /// `redirect` must be the one the authorize url named.
    async fn redeem_claude_code(
        &self,
        token_url: &str,
        auth_code: &str,
        state: &str,
        verifier: &str,
        redirect: &str,
        default_scopes: &str,
    ) -> Result<(), EngineError> {
        let token = self
            .inner
            .http
            .post(token_url)
            .json(&serde_json::json!({
                "grant_type": "authorization_code",
                "code": auth_code,
                "state": state,
                "client_id": CLAUDE_CLIENT_ID,
                "redirect_uri": redirect,
                "code_verifier": verifier,
            }))
            .timeout(Duration::from_secs(15))
            .send()
            .await
            .map_err(|e| EngineError::Other(format!("token exchange failed: {e}")))?;
        if !token.status().is_success() {
            let status = token.status();
            // Never echo the body: it can carry token material on odd errors,
            // and the loopback page shows this message to the browser.
            return Err(EngineError::Other(format!(
                "Anthropic rejected the code ({status}) — try again."
            )));
        }
        let token: serde_json::Value = token
            .json()
            .await
            .map_err(|e| EngineError::Other(format!("token exchange returned junk: {e}")))?;

        let access_token = str_field(&token, "access_token");
        let refresh_token = str_field(&token, "refresh_token");
        let expires_in = token
            .get("expires_in")
            .and_then(|v| v.as_i64())
            .unwrap_or(3600);
        let (Some(access_token), Some(refresh_token)) = (access_token, refresh_token) else {
            return Err(EngineError::Other(
                "Anthropic returned no usable tokens — try signing in again.".into(),
            ));
        };

        // Best-effort profile fetch — fills in the plan/org the way Claude Code does.
        let profile: Option<serde_json::Value> = match self
            .inner
            .http
            .get(&self.inner.endpoints.claude_profile)
            .bearer_auth(&access_token)
            .header("anthropic-beta", "oauth-2025-04-20")
            .send()
            .await
        {
            Ok(res) if res.status().is_success() => res.json().await.ok(),
            _ => None,
        };
        let empty = serde_json::json!({});
        let p_account = profile
            .as_ref()
            .and_then(|p| p.get("account"))
            .unwrap_or(&empty);
        let p_org = profile
            .as_ref()
            .and_then(|p| p.get("organization"))
            .unwrap_or(&empty);
        let t_account = token.get("account").unwrap_or(&empty);
        let t_org = token.get("organization").unwrap_or(&empty);

        let email = str_field(p_account, "email_address")
            .or_else(|| str_field(t_account, "email_address"))
            .ok_or_else(|| {
                EngineError::Other("Could not identify the signed-in account.".into())
            })?;
        let account_uuid = str_field(p_account, "uuid")
            .or_else(|| str_field(t_account, "uuid"))
            .unwrap_or_else(|| email.clone());
        let org_name = str_field(p_org, "name").or_else(|| str_field(t_org, "name"));
        let org_type = str_field(p_org, "organization_type");
        let rate_tier = str_field(p_org, "rate_limit_tier");
        let display_name =
            str_field(p_account, "display_name").or_else(|| str_field(p_account, "full_name"));
        let subscription_type = match org_type.as_deref() {
            Some("claude_max") => Some("max"),
            Some("claude_pro") => Some("pro"),
            Some("claude_team") => Some("team"),
            Some("claude_enterprise") => Some("enterprise"),
            _ => None,
        };

        let scopes: Vec<String> = str_field(&token, "scope")
            .unwrap_or_else(|| default_scopes.to_string())
            .split(' ')
            .map(str::to_string)
            .collect();
        let mut oauth = serde_json::json!({
            "accessToken": access_token,
            "refreshToken": refresh_token,
            "expiresAt": now_ms() + expires_in * 1000,
            "scopes": scopes,
        });
        if let (Some(sub), Some(map)) = (subscription_type, oauth.as_object_mut()) {
            map.insert("subscriptionType".into(), serde_json::json!(sub));
        }
        let mut oauth_account = serde_json::json!({
            "accountUuid": account_uuid,
            "emailAddress": email,
            "organizationUuid": str_field(p_org, "uuid").or_else(|| str_field(t_org, "uuid")),
            "organizationName": org_name,
            "displayName": display_name,
        });
        if let Some(map) = oauth_account.as_object_mut() {
            if let Some(t) = &org_type {
                map.insert("organizationType".into(), serde_json::json!(t));
            }
            if let Some(t) = &rate_tier {
                map.insert("organizationRateLimitTier".into(), serde_json::json!(t));
            }
        }

        let slot = Slot {
            id: slot_id_for(HarnessId::ClaudeCode, &account_uuid),
            harness: HarnessId::ClaudeCode,
            account_key: account_uuid.clone(),
            profile: SlotProfile {
                email,
                display_name,
                organization: org_name,
                plan: claude_plan(org_type.as_deref(), rate_tier.as_deref()),
                auth_kind: AgentAuthKind::Oauth,
            },
            credentials: serde_json::json!({ "claudeAiOauth": oauth }),
            claude_config: Some(serde_json::json!({ "oauthAccount": oauth_account })),
            saved_at: now_ms(),
            created_at: None,
            store_key: None,
        };
        let _ops = self.inner.ops.lock().await;
        self.write_slot(&slot)?;
        self.adopt_if_live(&slot).await
    }

    pub async fn poll_login(&self, login_id: &str) -> Result<AgentLoginPoll, EngineError> {
        self.sweep_flows();
        if let Some(poll) = self.poll_task_login(login_id) {
            return Ok(poll);
        }
        let (harness, home, completion, exit, output, scan_url, requester) =
            match lock(&self.inner.flows).get(login_id) {
                None => {
                    return Err(EngineError::Other(
                        "This sign-in attempt expired — start again.".into(),
                    ));
                }
                Some(LoginFlow::Claude { .. }) => {
                    return Ok(AgentLoginPoll {
                        status: AgentLoginStatus::Pending,
                        message: None,
                        url: None,
                        callback_port: None,
                    });
                }
                Some(LoginFlow::Task { .. }) => unreachable!("task logins poll above"),
                Some(LoginFlow::Spawned {
                    harness,
                    home,
                    completion,
                    exit,
                    output,
                    scan_url,
                    requester,
                    ..
                }) => (
                    *harness,
                    home.clone(),
                    *completion,
                    exit.clone(),
                    output.clone(),
                    *scan_url,
                    requester.clone(),
                ),
            };
        let exited = *lock(&exit);
        let detected = match completion {
            SpawnedCompletion::CredentialFile => {
                read_json(&home.join("auth.json")).and_then(|auth| match harness {
                    HarnessId::Codex => parse_codex_auth(auth),
                    HarnessId::Cursor => parse_cursor_auth(auth),
                    HarnessId::Grok => stores::parse_grok_auth(auth),
                    _ => None,
                })
            }
            SpawnedCompletion::ExitSuccess => None,
        };
        if let Some(detected) = detected {
            let _ops = self.inner.ops.lock().await;
            self.snapshot_detected(harness, &detected)?;
            // "Connect" semantics: with no (usable) live login, or a re-login
            // of the live account, the fresh login becomes the live one.
            let id = slot_id_for(harness, &detected.account_key);
            if let Some(slot) = self.read_slots(harness).into_iter().find(|s| s.id == id) {
                self.adopt_if_live(&slot).await?;
            }
            self.cancel_login(login_id);
            return Ok(AgentLoginPoll {
                status: AgentLoginStatus::Done,
                message: None,
                url: None,
                callback_port: None,
            });
        }
        if completion == SpawnedCompletion::ExitSuccess && exited == Some(Some(0)) {
            self.cancel_login(login_id);
            return Ok(AgentLoginPoll {
                status: AgentLoginStatus::Done,
                message: None,
                url: None,
                callback_port: None,
            });
        }
        if let Some(code) = exited {
            self.cancel_login(login_id);
            let message = if code == Some(0) {
                "The sign-in finished without credentials.".to_string()
            } else {
                let output = lock(&output);
                // The cursor shim reports failures as a JSONL fatal frame;
                // the CLIs print plain text. Surface the human part.
                scan_shim_fatal(&output).unwrap_or_else(|| {
                    strip_ansi(&output)
                        .trim()
                        .lines()
                        .last()
                        .unwrap_or("sign-in failed")
                        .to_string()
                })
            };
            // A CLI's last words can carry an authorize url, a device code or
            // worse — never hand them to the UI (or a log) raw.
            return Ok(AgentLoginPoll {
                status: AgentLoginStatus::Error,
                message: Some(zeron_harness::redact::redact_output(&message)),
                url: None,
                callback_port: None,
            });
        }
        // Still waiting: re-report the sign-in page (a page printed after
        // the start's short wait only reaches the app here) and any device
        // code the user has to type in.
        let (url, message) = {
            let output = lock(&output);
            // Only the device-code CLIs print a code; a loopback login's
            // output (codex's authorize url) is never scanned for one.
            let code = matches!(harness, HarnessId::Grok | HarnessId::Hermes)
                .then(|| scan_device_code(&output))
                .flatten();
            (scan_url(&output), code)
        };
        let callback_port = url.as_deref().and_then(loopback_port);
        if let (Some(requester), Some(port)) = (&requester, callback_port)
            && !self.inner.callback_routes.is_registered(login_id)
        {
            self.inner
                .callback_routes
                .register(login_id, port, requester, FLOW_TTL);
        }
        Ok(AgentLoginPoll {
            status: AgentLoginStatus::Pending,
            message: message.map(|code| format!("Enter the code {code} when asked.")),
            url,
            callback_port,
        })
    }

    /// Poll an engine-driven sign-in; `None` when `login_id` isn't one. A
    /// page first learned here (Antigravity's) publishes its callback port
    /// for a remote requester before the poll hands the url out.
    fn poll_task_login(&self, login_id: &str) -> Option<AgentLoginPoll> {
        let state = match lock(&self.inner.flows).get(login_id) {
            Some(LoginFlow::Task { state, .. }) => state.clone(),
            _ => return None,
        };
        let poll = {
            let state = lock(&state);
            match &state.outcome {
                None => {
                    let callback_port = state.url.as_deref().and_then(loopback_port);
                    if let (Some(requester), Some(port)) = (&state.requester, callback_port)
                        && !self.inner.callback_routes.is_registered(login_id)
                    {
                        self.inner
                            .callback_routes
                            .register(login_id, port, requester, FLOW_TTL);
                    }
                    return Some(AgentLoginPoll {
                        status: AgentLoginStatus::Pending,
                        message: state.message.clone(),
                        url: state.url.clone(),
                        callback_port,
                    });
                }
                Some(Ok(())) => AgentLoginPoll {
                    status: AgentLoginStatus::Done,
                    message: None,
                    url: None,
                    callback_port: None,
                },
                Some(Err(message)) => AgentLoginPoll {
                    status: AgentLoginStatus::Error,
                    message: Some(zeron_harness::redact::redact_output(message)),
                    url: None,
                    callback_port: None,
                },
            }
        };
        self.remove_flow(login_id);
        Some(poll)
    }

    /// Drop a flow's bookkeeping, and with it any callback route it published.
    fn remove_flow(&self, login_id: &str) -> Option<LoginFlow> {
        self.inner.callback_routes.remove(login_id);
        lock(&self.inner.flows).remove(login_id)
    }

    /// Drop a flow: kill a pending login child (`codex login` holds the fixed
    /// loopback OAuth port; the cursor shim polls Cursor's backend) and
    /// reclaim its throwaway home dir. Idempotent.
    pub fn cancel_login(&self, login_id: &str) {
        let flow = self.remove_flow(login_id);
        match flow {
            Some(LoginFlow::Spawned { child, home, .. }) => {
                if let Some(c) = lock(&child).as_mut() {
                    let _ = c.start_kill();
                }
                let _ = std::fs::remove_dir_all(&home);
            }
            Some(LoginFlow::Task { handle, home, .. }) => {
                handle.abort();
                if let Some(home) = home {
                    let _ = std::fs::remove_dir_all(&home);
                }
            }
            _ => {}
        }
    }

    /// Engine shutdown: kill any in-flight login child so an orphan `codex login`
    /// can't survive the restart and brick the next attempt.
    pub fn shutdown(&self) {
        let ids: Vec<String> = lock(&self.inner.flows).keys().cloned().collect();
        for id in ids {
            self.cancel_login(&id);
        }
    }

    /// Lazy TTL sweep (zeron uses a background fiber; native reaps on the next
    /// accounts call — same bound, no standing task).
    fn sweep_flows(&self) {
        let stale: Vec<String> = lock(&self.inner.flows)
            .iter()
            .filter(|(_, f)| f.started_at().elapsed() > FLOW_TTL)
            .map(|(id, _)| id.clone())
            .collect();
        for id in stale {
            self.cancel_login(&id);
        }
    }

    // ── detection ───────────────────────────────────────────────────────────

    async fn detect_claude(&self) -> (Option<Detected>, Option<String>) {
        let cfg = read_json(&self.inner.config.claude_config_file);
        let Some(oauth) = cfg.as_ref().and_then(|c| c.get("oauthAccount")).cloned() else {
            return (None, None);
        };
        let Some(email) = str_field(&oauth, "emailAddress") else {
            return (None, None);
        };
        let account_key = str_field(&oauth, "accountUuid").unwrap_or_else(|| email.clone());
        let (credentials, warning) = self.read_claude_credentials_cached(&account_key).await;
        let user_id = cfg.as_ref().and_then(|c| c.get("userID")).cloned();
        let mut claude_config = serde_json::json!({ "oauthAccount": oauth });
        if let (Some(uid), Some(map)) = (user_id, claude_config.as_object_mut())
            && uid.is_string()
        {
            map.insert("userID".into(), uid);
        }
        (
            Some(Detected {
                account_key,
                profile: SlotProfile {
                    email,
                    display_name: str_field(&oauth, "displayName"),
                    organization: str_field(&oauth, "organizationName"),
                    plan: claude_plan(
                        str_field(&oauth, "organizationType").as_deref(),
                        str_field(&oauth, "organizationRateLimitTier").as_deref(),
                    ),
                    auth_kind: AgentAuthKind::Oauth,
                },
                credentials,
                claude_config: Some(claude_config),
                store_key: None,
                identity_known: true,
            }),
            warning,
        )
    }

    async fn detect_antigravity(&self) -> Option<AntigravityLogin> {
        let home = self.inner.config.antigravity_home.as_deref()?;
        detect_antigravity_login(home, self.inner.config.antigravity_keychain).await
    }

    fn detect_codex(&self) -> Option<Detected> {
        read_json(&self.inner.config.codex_auth_file()).and_then(parse_codex_auth)
    }

    /// Codex slots used to be keyed only by `chatgpt_account_id`. Every seat
    /// in a Team workspace shares that id, so move legacy slots to the
    /// user-plus-workspace key returned by [`parse_codex_auth`]. Caller holds
    /// [`Inner::ops`].
    fn migrate_codex_slot_keys(&self) -> Result<(), EngineError> {
        for slot in self.read_slots(HarnessId::Codex) {
            let Some(detected) = parse_codex_auth(slot.credentials.clone()) else {
                continue;
            };
            if detected.account_key == slot.account_key {
                continue;
            }
            let dir = self.slots_dir(HarnessId::Codex)?;
            let id = slot_id_for(HarnessId::Codex, &detected.account_key);
            // If both forms exist, the new-key slot was written after the
            // upgrade and is the authoritative copy.
            if !dir.join(format!("{id}.json")).exists() {
                let mut moved = slot.clone();
                moved.id = id;
                moved.account_key = detected.account_key;
                moved.created_at = Some(slot.created_at.unwrap_or(slot.saved_at));
                self.write_slot(&moved)?;
            }
            remove_if_exists(&dir.join(format!("{}.json", slot.id)))?;
        }
        Ok(())
    }

    fn detect_cursor(&self) -> Option<Detected> {
        read_json(&self.inner.config.cursor_sdk_auth_file).and_then(parse_cursor_auth)
    }

    /// A live cursor login that runs can actually use: present, parseable,
    /// and not past the minted key's expiry.
    fn cursor_live_usable(&self) -> bool {
        read_json(&self.inner.config.cursor_sdk_auth_file)
            .is_some_and(|auth| cursor_key_usable(&auth))
    }

    fn write_cursor_auth(&self, credentials: &serde_json::Value) -> Result<(), EngineError> {
        let file = &self.inner.config.cursor_sdk_auth_file;
        if let Some(dir) = file.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let json = serde_json::to_string_pretty(credentials)
            .map_err(|e| EngineError::Other(format!("serialize cursor auth: {e}")))?;
        write_file_atomic(file, json.as_bytes(), true)
    }

    /// Persist a detected login into its slot (refreshing stored tokens).
    fn snapshot_detected(&self, harness: HarnessId, d: &Detected) -> Result<(), EngineError> {
        let Some(credentials) = &d.credentials else {
            return Ok(());
        };
        let id = slot_id_for(harness, &d.account_key);
        // A store without identity (Devin's bare key) must not overwrite the
        // identity a usage probe already wrote into the slot.
        let profile = match d.identity_known {
            true => d.profile.clone(),
            false => self
                .read_slot(harness, &id)
                .map(|existing| existing.profile)
                .unwrap_or_else(|| d.profile.clone()),
        };
        self.write_slot(&Slot {
            id,
            harness,
            account_key: d.account_key.clone(),
            profile,
            credentials: credentials.clone(),
            claude_config: d.claude_config.clone(),
            saved_at: now_ms(),
            created_at: None,
            store_key: d.store_key.clone(),
        })
    }

    /// One saved slot by id (`None` when missing or unparseable).
    fn read_slot(&self, harness: HarnessId, id: &str) -> Option<Slot> {
        let file = self.slots_dir(harness).ok()?.join(format!("{id}.json"));
        serde_json::from_str(&std::fs::read_to_string(file).ok()?).ok()
    }

    // ── Claude credential store (Keychain on macOS, file elsewhere) ─────────

    /// [`Self::read_claude_credentials`], reused for [`CLAUDE_CREDENTIALS_TTL`]
    /// while the live identity (`account_key`, from `~/.claude.json`) is
    /// unchanged. The page lists twice per open (paint, then refresh), and
    /// each Keychain read spawns `security` twice. Keying on the identity
    /// means a re-login is never paired with the previous login's secret;
    /// activate drops the cache so a swap always snapshots fresh tokens.
    async fn read_claude_credentials_cached(
        &self,
        account_key: &str,
    ) -> (Option<serde_json::Value>, Option<String>) {
        if let Some(cached) = lock(&self.inner.claude_credentials).as_ref()
            && cached.account_key == account_key
            && cached.at.elapsed() < CLAUDE_CREDENTIALS_TTL
        {
            return cached.read.clone();
        }
        let read = self.read_claude_credentials().await;
        *lock(&self.inner.claude_credentials) = Some(CachedClaudeCredentials {
            account_key: account_key.to_string(),
            read: read.clone(),
            at: Instant::now(),
        });
        read
    }

    /// Read the live Claude credentials. `None` payload + warning ⇒ we know a
    /// login exists but couldn't read the secret (Keychain denied us).
    ///
    /// Same precedence as Claude Code itself (2.1.x secure storage =
    /// keychain-with-plaintext-fallback): the Keychain FIRST, the
    /// `.credentials.json` file only when the Keychain holds nothing. The
    /// file is a fallback Claude Code leaves behind (it is only deleted when
    /// a write migrates an EMPTY Keychain), so a stale one routinely sits
    /// next to the live Keychain login — reading it first captured tokens
    /// that expired days ago, and every usage probe for the active account
    /// came back 401 ("Usage unavailable").
    async fn read_claude_credentials(&self) -> (Option<serde_json::Value>, Option<String>) {
        #[cfg(target_os = "macos")]
        if let Some(service) = &self.inner.config.claude_keychain_service {
            let (creds, warning) = keychain::read_credentials(service).await;
            if creds.is_some() {
                return (creds, None);
            }
            // Denied/unparseable Keychain: Claude Code falls back to the
            // file too, but keep the warning — the file may be stale.
            return (read_json(&self.inner.config.claude_creds_file()), warning);
        }
        (read_json(&self.inner.config.claude_creds_file()), None)
    }

    async fn write_claude_credentials(
        &self,
        credentials: &serde_json::Value,
    ) -> Result<(), EngineError> {
        let json = credentials.to_string();
        #[cfg(target_os = "macos")]
        if let Some(service) = &self.inner.config.claude_keychain_service {
            // claude-swap's primitive: update the Keychain item in place —
            // wherever Claude Code will READ it (see `read_claude_credentials`):
            // the Keychain whenever it holds an item or no file exists; the
            // file only for a file-only (Keychain-less) login.
            if keychain::has_item(service).await || !self.inner.config.claude_creds_file().exists()
            {
                return keychain::write_credentials(service, &json).await;
            }
        }
        std::fs::create_dir_all(&self.inner.config.claude_config_dir)?;
        // Atomic + owner-only from birth — live tokens.
        write_file_atomic(
            &self.inner.config.claude_creds_file(),
            json.as_bytes(),
            true,
        )
    }

    // ── slot files ──────────────────────────────────────────────────────────

    fn slots_dir(&self, harness: HarnessId) -> Result<PathBuf, EngineError> {
        let dir = self
            .inner
            .config
            .private_root()?
            .join(harness_slug(harness));
        private_dir(&dir)?;
        Ok(dir)
    }

    fn read_slots(&self, harness: HarnessId) -> Vec<Slot> {
        let Ok(dir) = self.slots_dir(harness) else {
            return Vec::new();
        };
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return Vec::new();
        };
        let mut slots: Vec<Slot> = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            // One malformed slot file must skip THAT slot, not brick the page.
            if let Some(slot) = std::fs::read_to_string(&path)
                .ok()
                .and_then(|raw| serde_json::from_str::<Slot>(&raw).ok())
            {
                slots.push(slot);
            }
        }
        // Creation order — stable across switches (saved_at churns on every
        // auto-snapshot; created_at never does). Slot id breaks creation-time
        // ties: two logins saved in the same millisecond otherwise land in
        // read_dir order, which is filesystem-arbitrary for UUID-named files
        // and reshuffles the page between restarts (issue #161).
        slots.sort_by(|a, b| {
            (a.created_at.unwrap_or(a.saved_at), &a.id)
                .cmp(&(b.created_at.unwrap_or(b.saved_at), &b.id))
        });
        slots
    }

    fn write_slot(&self, slot: &Slot) -> Result<(), EngineError> {
        let file = self
            .slots_dir(slot.harness)?
            .join(format!("{}.json", slot.id));
        let existing: Option<Slot> = std::fs::read_to_string(&file)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok());
        let mut full = slot.clone();
        full.created_at = existing
            .and_then(|e| e.created_at.or(Some(e.saved_at)))
            .or(slot.created_at)
            .or_else(|| {
                // A brand-new slot: stamp it strictly after every sibling, so
                // two logins inside the same millisecond still list in the
                // order they were saved (creation order is the page's sort
                // key; ms-resolution ties otherwise fall to read_dir order).
                let floor = self
                    .read_slots(slot.harness)
                    .iter()
                    .map(|s| s.created_at.unwrap_or(s.saved_at))
                    .max()
                    .map(|newest| newest + 1)
                    .unwrap_or(slot.saved_at);
                Some(floor.max(slot.saved_at))
            });
        let json = serde_json::to_string_pretty(&full)
            .map_err(|e| EngineError::Other(format!("serialize slot: {e}")))?;
        // Atomic + 0600 from birth: tokens must never be world-readable, and a
        // crash mid-write must never leave torn JSON.
        write_file_atomic(&file, json.as_bytes(), true)
    }

    // ── remaining usage ─────────────────────────────────────────────────────

    /// Probe every due account concurrently and fold the outcomes into the
    /// usage cache (then disk). Accounts inside a backoff window, probed a
    /// moment ago, or already being probed by an overlapping list are left
    /// alone — they keep serving their last known usage.
    async fn refresh_usage(&self, targets: &[(HarnessId, &Slot, bool)]) {
        let now = now_ms();
        let mut probes = Vec::new();
        let mut claimed = Vec::new();
        {
            let usage = lock(&self.inner.usage);
            let mut inflight = lock(&self.inner.inflight_probes);
            for &(harness, slot, active) in targets {
                let key = usage_key(harness, &slot.account_key);
                let credentials = credentials_fingerprint(&slot.credentials);
                if let Some(entry) = usage.get(&key)
                    && !entry.probe_due(&credentials, now)
                {
                    tracing::debug!(
                        provider = harness_slug(harness),
                        slot = %slot.id,
                        retry_at = ?entry.retry_at,
                        "usage probe skipped (backoff or fresh)"
                    );
                    continue;
                }
                if !inflight.insert(key.clone()) {
                    continue;
                }
                claimed.push(key.clone());
                probes.push(async move {
                    let result = self.probe_usage(harness, slot, active).await;
                    (key, credentials, result)
                });
            }
        }
        let _release = InflightGuard {
            set: &self.inner.inflight_probes,
            keys: claimed,
        };
        if probes.is_empty() {
            return;
        }
        let results = futures::future::join_all(probes).await;
        let now = now_ms();
        let mut usage = lock(&self.inner.usage);
        for (key, credentials, result) in results {
            usage
                .entry(key)
                .or_default()
                .record(result, credentials, now);
        }
        // Drop entries for accounts that no longer have a slot (forgotten).
        let live: std::collections::HashSet<String> = targets
            .iter()
            .map(|(harness, slot, _)| usage_key(*harness, &slot.account_key))
            .collect();
        usage.retain(|key, _| live.contains(key));
        // Persist under the lock: overlapping lists must not interleave
        // writes of the same file.
        let file = UsageCacheFile {
            entries: usage.clone(),
        };
        let persisted = serde_json::to_vec_pretty(&file)
            .map_err(|e| EngineError::Other(e.to_string()))
            .and_then(|json| {
                self.inner.config.private_root()?;
                write_file_atomic(&self.inner.config.usage_cache_file(), &json, true)
            });
        if let Err(err) = persisted {
            tracing::warn!(error = %err, "agent usage cache write failed");
        }
    }

    /// One account's probe. Never touches the cache (see [`Self::refresh_usage`]).
    async fn probe_usage(
        &self,
        harness: HarnessId,
        slot: &Slot,
        is_active: bool,
    ) -> Result<UsageSnapshot, ProbeError> {
        let result = match harness {
            HarnessId::ClaudeCode => self.claude_usage(slot, is_active).await,
            HarnessId::Codex => self.codex_usage(slot).await,
            HarnessId::Cursor => self.cursor_usage(slot).await,
            HarnessId::Grok => self.grok_usage(slot, is_active).await,
            HarnessId::Devin => self.devin_usage(slot).await,
            HarnessId::Opencode | HarnessId::Pi | HarnessId::Hermes => {
                self.keyed_usage(harness, slot).await
            }
            _ => Err(ProbeError::NoCredentials {
                why: NoCredentials::Missing,
            }),
        };
        if let Err(error) = &result {
            // Expected, quiet outcomes (API-key logins have no windows).
            if !matches!(error, ProbeError::NoCredentials { .. }) {
                tracing::warn!(
                    provider = harness_slug(harness),
                    slot = %slot.id,
                    active = is_active,
                    class = error.class(),
                    status = ?error.status(),
                    backoff_s = error.backoff().as_secs(),
                    "agent usage unavailable"
                );
            }
        }
        result
    }

    async fn claude_usage(
        &self,
        slot: &Slot,
        is_active: bool,
    ) -> Result<UsageSnapshot, ProbeError> {
        let missing = ProbeError::NoCredentials {
            why: NoCredentials::Missing,
        };
        let oauth = slot
            .credentials
            .get("claudeAiOauth")
            .ok_or(missing.clone())?;
        let access_token = str_field(oauth, "accessToken").ok_or(missing)?;
        match self.claude_usage_request(&access_token).await {
            // The stored expiry metadata can lie (a Claude Code regression
            // wrote `expiresAt: 0` for fresh logins), so probe with the
            // stored token first and rotate the slot-owned pair only after
            // the endpoint actually REJECTED it (401/403) — a 429 or 5xx says
            // nothing about the token, and a refresh there would burn a
            // single-use refresh token for nothing. The active login is never
            // rotated: the running CLI may hold its single-use refresh token.
            Err(ProbeError::Unauthorized { status })
                if !is_active && self.inner.endpoints.allow_slot_refresh =>
            {
                match self.refresh_claude_slot(slot).await? {
                    Some(fresh) => self.claude_usage_request(&fresh).await,
                    // Refresh in flight elsewhere (single-flight) — report
                    // the original rejection; the next probe sees the result.
                    None => Err(ProbeError::Unauthorized { status }),
                }
            }
            result => result,
        }
    }

    /// One usage probe: GET the endpoint and parse windows.
    async fn claude_usage_request(&self, access_token: &str) -> Result<UsageSnapshot, ProbeError> {
        let body = probe_json(
            "claude-code",
            "usage",
            self.inner
                .http
                .get(&self.inner.endpoints.claude_usage)
                .bearer_auth(access_token)
                .header("anthropic-beta", "oauth-2025-04-20")
                .header("Content-Type", "application/json"),
        )
        .await?;
        claude_usage_windows(&body).ok_or_else(|| schema_error("claude-code", &body))
    }

    async fn codex_usage(&self, slot: &Slot) -> Result<UsageSnapshot, ProbeError> {
        // api-key mode has no ChatGPT rate windows.
        let Some(tokens) = slot.credentials.get("tokens") else {
            return Err(ProbeError::NoCredentials {
                why: if str_field(&slot.credentials, "OPENAI_API_KEY").is_some() {
                    NoCredentials::ApiKey
                } else {
                    NoCredentials::Missing
                },
            });
        };
        let access_token = str_field(tokens, "access_token").ok_or(ProbeError::NoCredentials {
            why: NoCredentials::Missing,
        })?;
        let body = probe_json(
            "codex",
            "usage",
            self.inner
                .http
                .get(&self.inner.endpoints.codex_usage)
                .bearer_auth(&access_token)
                .header(
                    "chatgpt-account-id",
                    str_field(tokens, "account_id").unwrap_or_default(),
                ),
        )
        .await?;
        codex_usage_snapshot(&body).ok_or_else(|| schema_error("codex", &body))
    }

    async fn cursor_usage(&self, slot: &Slot) -> Result<UsageSnapshot, ProbeError> {
        // The SDK key tracks identity/expiry but has no quota view — the
        // account numbers live on the dashboard API the Cursor app itself
        // calls, reachable with a session minted from the key.
        let api_key = str_field(&slot.credentials, "apiKey").ok_or(ProbeError::NoCredentials {
            why: NoCredentials::Missing,
        })?;
        if !cursor_key_usable(&slot.credentials) {
            return Err(ProbeError::NoCredentials {
                why: NoCredentials::KeyExpired,
            });
        }
        let backend = str_field(&slot.credentials, "backendUrl")
            .unwrap_or_else(|| CURSOR_DEFAULT_BACKEND.to_string())
            .trim_end_matches('/')
            .to_string();
        let session = probe_json(
            "cursor",
            "exchange",
            self.inner
                .http
                .post(format!("{backend}/auth/exchange_user_api_key"))
                .bearer_auth(api_key)
                .json(&serde_json::json!({})),
        )
        .await?;
        let access_token =
            str_field(&session, "accessToken").ok_or_else(|| schema_error("cursor", &session))?;
        let body = probe_json(
            "cursor",
            "usage",
            self.inner
                .http
                .post(format!("{backend}/{CURSOR_CURRENT_PERIOD_USAGE}"))
                .bearer_auth(&access_token)
                .header("Connect-Protocol-Version", "1")
                .json(&serde_json::json!({})),
        )
        .await?;
        cursor_usage_window(&body)
            .map(|window| UsageSnapshot {
                windows: vec![window],
                plan_label: None,
            })
            .ok_or_else(|| schema_error("cursor", &body))
    }

    /// Refresh a saved Claude slot's expired access token so its usage stays
    /// queryable. NEVER called for the active login. Single-flight per slot:
    /// OAuth refresh tokens are commonly single-use, and a concurrent second
    /// POST of the same one would revoke the family and brick the slot.
    /// `Ok(None)` = another refresh of this slot is already in flight.
    async fn refresh_claude_slot(&self, slot: &Slot) -> Result<Option<String>, ProbeError> {
        if !lock(&self.inner.inflight_refreshes).insert(slot.id.clone()) {
            return Ok(None);
        }
        let _release = InflightGuard {
            set: &self.inner.inflight_refreshes,
            keys: vec![slot.id.clone()],
        };
        self.refresh_claude_slot_once(slot).await.map(Some)
    }

    async fn refresh_claude_slot_once(&self, slot: &Slot) -> Result<String, ProbeError> {
        let missing = ProbeError::NoCredentials {
            why: NoCredentials::Missing,
        };
        let oauth = slot
            .credentials
            .get("claudeAiOauth")
            .ok_or(missing.clone())?
            .clone();
        let refresh_token = str_field(&oauth, "refreshToken").ok_or(missing)?;
        let body = probe_json(
            "claude-code",
            "refresh",
            self.inner
                .http
                .post(&self.inner.endpoints.claude_token)
                .json(&serde_json::json!({
                    "grant_type": "refresh_token",
                    "refresh_token": refresh_token,
                    "client_id": CLAUDE_CLIENT_ID,
                })),
        )
        .await
        .map_err(|error| match error {
            // A refused refresh (400 invalid_grant: revoked/used token) means
            // this login is dead — surface it as a rejection ("Sign in again").
            ProbeError::Http { status, .. } if status == 400 => ProbeError::Unauthorized { status },
            other => other,
        })?;
        let access_token =
            str_field(&body, "access_token").ok_or_else(|| schema_error("claude-code", &body))?;
        let expires_in = body
            .get("expires_in")
            .and_then(|v| v.as_i64())
            .unwrap_or(3600);
        let mut updated = oauth;
        if let Some(map) = updated.as_object_mut() {
            map.insert("accessToken".into(), serde_json::json!(access_token));
            map.insert(
                "refreshToken".into(),
                serde_json::json!(str_field(&body, "refresh_token").unwrap_or(refresh_token)),
            );
            map.insert(
                "expiresAt".into(),
                serde_json::json!(now_ms() + expires_in * 1000),
            );
        }
        let mut refreshed = slot.clone();
        // Keep sibling keys (mcpOAuth, pluginSecrets, …) — rewriting the blob
        // as oauth-only would drop them, and a later activate of this slot
        // would have nothing to merge if live credentials were also empty.
        refreshed.credentials = with_claude_ai_oauth(&slot.credentials, updated);
        refreshed.saved_at = now_ms();
        if let Err(err) = self.write_slot(&refreshed) {
            tracing::warn!(slot = %slot.id, error = %err, "refreshed slot write failed");
        }
        Ok(access_token)
    }
}

// ── macOS Keychain (documented here; compiled only on macOS) ────────────────
//
// Claude Code stores its credentials in the login Keychain under the service
// `Claude Code-credentials` (suffixed per `CLAUDE_CONFIG_DIR`, see
// `claude_keychain_service`), account = the current username. Reads use
// `security find-generic-password` — two-step (existence probe needs no
// authorization, then `-w` for the secret) so a user denial is distinguishable
// from "not logged in". Writes use `add-generic-password -U` (update in place).
// Every call is bounded at 15s: an unanswered Keychain consent dialog blocks
// `security` INDEFINITELY, and this runs on every list.
#[cfg(target_os = "macos")]
mod keychain {
    use super::*;

    const EXEC_TIMEOUT: Duration = Duration::from_secs(15);

    async fn exec(args: &[&str]) -> (bool, String, String) {
        // Absolute path: a PATH-planted `security` must never see secrets.
        let run = tokio::process::Command::new("/usr/bin/security")
            .args(args)
            .stdin(std::process::Stdio::null())
            .output();
        match tokio::time::timeout(EXEC_TIMEOUT, run).await {
            Ok(Ok(out)) => (
                out.status.success(),
                String::from_utf8_lossy(&out.stdout).to_string(),
                String::from_utf8_lossy(&out.stderr).to_string(),
            ),
            _ => (false, String::new(), "security timed out".into()),
        }
    }

    fn account() -> String {
        std::env::var("USER").unwrap_or_else(|_| "unknown".into())
    }

    /// Whether the item exists (metadata only — needs no authorization).
    pub(super) async fn has_item(service: &str) -> bool {
        exec(&["find-generic-password", "-s", service]).await.0
    }

    /// [`has_item`] for one account of a shared service (metadata only).
    pub(super) async fn has_account_item(service: &str, account: &str) -> bool {
        exec(&["find-generic-password", "-s", service, "-a", account])
            .await
            .0
    }

    pub(super) async fn read_credentials(
        service: &str,
    ) -> (Option<serde_json::Value>, Option<String>) {
        if !has_item(service).await {
            return (None, None);
        }
        let (ok, stdout, _) = exec(&[
            "find-generic-password",
            "-a",
            &account(),
            "-s",
            service,
            "-w",
        ])
        .await;
        if !ok {
            return (
                None,
                Some(
                    "A Claude Code login exists, but macOS Keychain denied access to it — \
                     approve the prompt (choose “Always Allow”) and refresh to enable switching."
                        .into(),
                ),
            );
        }
        match serde_json::from_str(stdout.trim()) {
            Ok(creds) => (Some(creds), None),
            Err(_) => (
                None,
                Some("The Claude Code Keychain entry could not be parsed.".into()),
            ),
        }
    }

    pub(super) async fn write_credentials(service: &str, json: &str) -> Result<(), EngineError> {
        let (ok, _, stderr) = exec(&[
            "add-generic-password",
            "-U",
            "-a",
            &account(),
            "-s",
            service,
            "-w",
            json,
        ])
        .await;
        if ok {
            Ok(())
        } else {
            Err(EngineError::Other(format!(
                "Keychain write failed: {}",
                if stderr.trim().is_empty() {
                    "unknown error"
                } else {
                    stderr.trim()
                }
            )))
        }
    }
}

// ── Antigravity ─────────────────────────────────────────────────────────────

/// Antigravity's live login, as far as zeron can see it without its secret.
#[derive(Debug, Clone, PartialEq, Eq)]
struct AntigravityLogin {
    /// The canonical auth method (`oauth-personal`, `oauth-business`, …).
    method: String,
}

impl AntigravityLogin {
    /// The account row. The token blob holds no identity (the server looks
    /// the email up per session and keeps it in memory), so the row names
    /// the kind of login instead of an address.
    fn account(&self) -> AgentAccount {
        let (label, plan, kind) = match self.method.as_str() {
            "oauth-personal" => ("Google account", None, AgentAuthKind::Oauth),
            "oauth-business" => (
                "Google account",
                Some("Gemini Enterprise"),
                AgentAuthKind::Oauth,
            ),
            "gemini-api-key" => ("Gemini API key", None, AgentAuthKind::ApiKey),
            "agent-platform" => (
                "Google Cloud",
                Some("Agent Platform"),
                AgentAuthKind::ApiKey,
            ),
            other => (other, None, AgentAuthKind::Oauth),
        };
        AgentAccount {
            id: slot_id_for(HarnessId::Antigravity, &self.method),
            harness: HarnessId::Antigravity,
            email: None,
            plan_label: plan.map(str::to_string),
            active: true,
            usage_windows: Vec::new(),
            usage_fetched_at: None,
            usage_error: None,
            display_name: Some(label.to_string()),
            organization: None,
            auth_kind: Some(kind),
            switchable: false,
            saved_at: None,
            provider: None,
        }
    }
}

/// Detect Antigravity's login under `home` without reading any secret: the
/// method its server saves on every successful sign-in (`settings.json`
/// `auth.type`), and for Google sign-ins the PRESENCE of the token it stores —
/// a Keychain item (`gemini` / `antigravity-acp[-business]`, metadata only)
/// or its file fallback. Key-based methods keep the key in the environment,
/// so the saved method is all there is. A first run saves no method; its
/// default is the personal Google sign-in.
async fn detect_antigravity_login(home: &Path, keychain: bool) -> Option<AntigravityLogin> {
    let method = zeron_harness::acp::antigravity_saved_auth_method(home)
        .unwrap_or_else(|| "oauth-personal".into());
    let token = match method.as_str() {
        "oauth-personal" => Some(("acp_token.json", "antigravity-acp")),
        "oauth-business" => Some(("acp_business_token.json", "antigravity-acp-business")),
        _ => None,
    };
    if let Some((file, account)) = token
        && !home.join("antigravity-acp").join(file).is_file()
        && !(keychain && antigravity_keychain_item(account).await)
    {
        return None;
    }
    Some(AntigravityLogin { method })
}

#[cfg(target_os = "macos")]
async fn antigravity_keychain_item(account: &str) -> bool {
    keychain::has_account_item("gemini", account).await
}

#[cfg(not(target_os = "macos"))]
async fn antigravity_keychain_item(_account: &str) -> bool {
    false
}

// ── helpers ─────────────────────────────────────────────────────────────────

/// The upstream group a row belongs to (`AgentAccount::provider`): only
/// agents that keep one login PER model provider have one. (Grok slots also
/// carry a store key — the issuer entry they swap — but one Grok login is
/// live at a time, so its rows form a single group.)
fn provider_group(harness: HarnessId, store_key: Option<&str>) -> Option<String> {
    matches!(
        harness,
        HarnessId::Opencode | HarnessId::Pi | HarnessId::Hermes
    )
    .then(|| store_key.map(str::to_string))
    .flatten()
}

fn harness_slug(harness: HarnessId) -> &'static str {
    match harness {
        HarnessId::ClaudeCode => "claude-code",
        HarnessId::Codex => "codex",
        HarnessId::Cursor => "cursor",
        HarnessId::Devin => "devin",
        HarnessId::Grok => "grok",
        HarnessId::Hermes => "hermes",
        HarnessId::Pi => "pi",
        HarnessId::Opencode => "opencode",
        HarnessId::Antigravity => "antigravity",
        HarnessId::Mock => "mock",
    }
}

fn read_json(file: &Path) -> Option<serde_json::Value> {
    let raw = std::fs::read_to_string(file).ok()?;
    serde_json::from_str(&raw)
        .ok()
        .filter(serde_json::Value::is_object)
}

fn str_field(value: &serde_json::Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Decode a JWT payload without verifying — we only mine identity claims from a
/// token the user's own CLI already trusts.
fn jwt_claims(jwt: &str) -> Option<serde_json::Value> {
    let payload = jwt.split('.').nth(1)?;
    let bytes = BASE64_URL
        .decode(payload)
        .or_else(|_| BASE64.decode(payload))
        .ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn slot_id_for(harness: HarnessId, account_key: &str) -> String {
    let digest = Sha256::digest(format!("{}:{account_key}", harness_slug(harness)).as_bytes());
    crate::repos::hex(&digest)[..16].to_string()
}

/// Pretty plan label from Claude's org type + rate-limit tier ("Max 20×").
fn claude_plan(org_type: Option<&str>, tier: Option<&str>) -> Option<String> {
    let base = match org_type {
        Some("claude_max") => "Max",
        Some("claude_pro") => "Pro",
        Some("claude_team") => "Team",
        Some("claude_enterprise") => "Enterprise",
        _ => return None,
    };
    // "…_20x" style tiers carry a multiplier suffix.
    let mult = tier.and_then(|t| {
        let stem = t.strip_suffix('x')?;
        let digits: String = stem
            .chars()
            .rev()
            .take_while(char::is_ascii_digit)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        let preceded = stem.len() > digits.len()
            && stem.as_bytes().get(stem.len() - digits.len() - 1) == Some(&b'_');
        (!digits.is_empty() && preceded).then_some(digits)
    });
    Some(match mult {
        Some(mult) => format!("{base} {mult}×"),
        None => base.to_string(),
    })
}

fn codex_plan(plan: Option<&str>) -> Option<String> {
    let plan = plan?;
    let mut chars = plan.chars();
    let first = chars.next()?;
    Some(format!(
        "ChatGPT {}{}",
        first.to_uppercase(),
        chars.as_str()
    ))
}

/// Meter label for a Codex rate-limit window from its `limit_window_seconds`:
/// the free tier's window is a 30-day month (2_592_000s), Plus runs a 5-hour
/// primary (~18_000s) with a weekly secondary (604_800s). A bare "> 1 day =
/// week" rule mislabeled the monthly window "Week"; thresholds in seconds
/// leave the middle gaps to the nearest label rather than guessing a plan.
fn codex_window_label(span_seconds: i64) -> &'static str {
    const DAY: i64 = 86_400;
    if span_seconds >= 28 * DAY {
        "Month"
    } else if span_seconds >= 5 * DAY {
        "Week"
    } else {
        "Session"
    }
}

/// Compose a target Claude login with the machine's current shared fields.
///
/// `claudeAiOauth` (and any other slot-owned sibling, including
/// `trustedDeviceToken`) come from `target`. Allowlisted shared keys come
/// from `live`, presence and absence alike — a key the live blob no longer
/// holds is not resurrected from the slot. When there is no live JSON object
/// (or the target is not a Claude OAuth blob), `target` is returned unchanged.
fn compose_claude_credentials(
    target: &serde_json::Value,
    live: Option<&serde_json::Value>,
) -> serde_json::Value {
    let Some(live_obj) = live.and_then(|v| v.as_object()) else {
        return target.clone();
    };
    let Some(target_obj) = target.as_object() else {
        return target.clone();
    };
    if !target_obj.contains_key("claudeAiOauth") {
        return target.clone();
    }
    let unrecognized: Vec<&str> = live_obj
        .keys()
        .map(String::as_str)
        .filter(|key| {
            *key != "claudeAiOauth"
                && *key != "trustedDeviceToken"
                && !CLAUDE_SHARED_CREDENTIAL_KEYS.contains(key)
        })
        .collect();
    if !unrecognized.is_empty() {
        tracing::debug!(
            keys = ?unrecognized,
            "live Claude credentials have sibling keys we don't recognize; treating them as slot-owned"
        );
    }
    let mut composed = serde_json::Map::new();
    for (key, value) in target_obj {
        if !CLAUDE_SHARED_CREDENTIAL_KEYS.contains(&key.as_str()) {
            composed.insert(key.clone(), value.clone());
        }
    }
    for key in CLAUDE_SHARED_CREDENTIAL_KEYS {
        if let Some(value) = live_obj.get(*key) {
            composed.insert((*key).to_string(), value.clone());
        }
    }
    serde_json::Value::Object(composed)
}

/// Replace `claudeAiOauth` without dropping sibling keys on the same blob.
fn with_claude_ai_oauth(
    credentials: &serde_json::Value,
    oauth: serde_json::Value,
) -> serde_json::Value {
    let mut creds = credentials.clone();
    match creds.as_object_mut() {
        Some(map) => {
            map.insert("claudeAiOauth".into(), oauth);
            creds
        }
        None => serde_json::json!({ "claudeAiOauth": oauth }),
    }
}

/// Parse a codex `auth.json` (the live one or a fresh login's).
fn parse_codex_auth(auth: serde_json::Value) -> Option<Detected> {
    if let Some(id_token) = auth
        .get("tokens")
        .and_then(|t| t.get("id_token"))
        .and_then(|v| v.as_str())
    {
        let claims = jwt_claims(id_token).unwrap_or_else(|| serde_json::json!({}));
        let oa = claims
            .get("https://api.openai.com/auth")
            .cloned()
            .unwrap_or_default();
        let email = str_field(&claims, "email")?;
        // Team seats share a workspace id. Include the user id so one
        // teammate's login cannot collide with, or be mistaken for, another's.
        let workspace = str_field(&oa, "chatgpt_account_id");
        let user = str_field(&oa, "chatgpt_user_id").or_else(|| str_field(&oa, "user_id"));
        let account_key = match (user, workspace) {
            (Some(user), Some(workspace)) => format!("{user}::{workspace}"),
            (None, Some(workspace)) => workspace,
            _ => email.clone(),
        };
        return Some(Detected {
            account_key,
            profile: SlotProfile {
                email,
                display_name: str_field(&claims, "name"),
                organization: None,
                plan: codex_plan(str_field(&oa, "chatgpt_plan_type").as_deref()),
                auth_kind: AgentAuthKind::Oauth,
            },
            credentials: Some(auth),
            claude_config: None,
            store_key: None,
            identity_known: true,
        });
    }
    let api_key = str_field(&auth, "OPENAI_API_KEY")?;
    let digest = Sha256::digest(api_key.as_bytes());
    let tail: String = api_key
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    Some(Detected {
        account_key: format!("api-key:{}", &crate::repos::hex(&digest)[..12]),
        profile: SlotProfile {
            email: format!("API key ·…{tail}"),
            display_name: None,
            organization: None,
            plan: Some("API key".into()),
            auth_kind: AgentAuthKind::ApiKey,
        },
        credentials: Some(auth),
        claude_config: None,
        store_key: None,
        identity_known: true,
    })
}

/// ISO string (Claude) or unix seconds (Codex) → timestamp.
/// The Cursor SDK's credential store (`StoredSdkCredentials`, version 1):
/// the named user API key its browser login minted, plus identity/expiry.
fn parse_cursor_auth(auth: serde_json::Value) -> Option<Detected> {
    let api_key = str_field(&auth, "apiKey")?;
    let email = str_field(&auth, "email");
    let account_key = email.clone().unwrap_or_else(|| {
        let digest = Sha256::digest(api_key.as_bytes());
        format!("api-key:{}", &crate::repos::hex(&digest)[..12])
    });
    let expires_at = auth.get("apiKeyExpiresAtMs").and_then(|v| v.as_i64());
    // The key's expiry doubles as the plan chip — with 90-day keys it is the
    // one fact worth showing on the card.
    let plan = expires_at.and_then(|ms| {
        let when = DateTime::<Utc>::from_timestamp_millis(ms)?;
        Some(if ms < now_ms() {
            "Key expired".to_string()
        } else {
            format!("Key expires {}", when.format("%b %-d"))
        })
    });
    Some(Detected {
        account_key,
        profile: SlotProfile {
            email: email.unwrap_or_else(|| {
                let tail: String = api_key
                    .chars()
                    .skip(api_key.len().saturating_sub(4))
                    .collect();
                format!("API key ·…{tail}")
            }),
            display_name: None,
            organization: None,
            plan,
            auth_kind: AgentAuthKind::Oauth,
        },
        credentials: Some(auth),
        claude_config: None,
        store_key: None,
        identity_known: true,
    })
}

/// Present, parseable, and unexpired — what a run can actually use.
fn cursor_key_usable(auth: &serde_json::Value) -> bool {
    str_field(auth, "apiKey").is_some()
        && auth
            .get("apiKeyExpiresAtMs")
            .and_then(|v| v.as_i64())
            .is_none_or(|ms| ms > now_ms())
}

fn parse_when(value: Option<&serde_json::Value>) -> Option<DateTime<Utc>> {
    match value? {
        serde_json::Value::Number(n) => DateTime::<Utc>::from_timestamp(n.as_i64()?, 0),
        serde_json::Value::String(s) => DateTime::parse_from_rfc3339(s)
            .ok()
            .map(|t| t.with_timezone(&Utc)),
        _ => None,
    }
}

fn usage_key(harness: HarnessId, account_key: &str) -> String {
    format!("{}:{account_key}", harness_slug(harness))
}

/// A short digest of a slot's credential blob — tells "same token that was
/// rejected" from "the CLI refreshed / the user re-logged in". Never the
/// secret itself: this lands in the usage cache file.
fn credentials_fingerprint(credentials: &serde_json::Value) -> String {
    crate::repos::hex(&Sha256::digest(credentials.to_string().as_bytes()))[..16].to_string()
}

/// Send one probe request and decode its JSON body, classifying every
/// failure. Logs provider/step/status/class/Retry-After — never the request
/// (bearer tokens) or the response body (it can echo account details).
async fn probe_json(
    provider: &'static str,
    step: &'static str,
    request: reqwest::RequestBuilder,
) -> Result<serde_json::Value, ProbeError> {
    let response = match request.send().await {
        Ok(response) => response,
        Err(err) => {
            let error = ProbeError::Network {
                timeout: err.is_timeout(),
            };
            tracing::warn!(
                provider,
                step,
                class = error.class(),
                connect = err.is_connect(),
                "agent usage probe failed"
            );
            return Err(error);
        }
    };
    let status = response.status();
    if !status.is_success() {
        let retry_after_secs = retry_after_secs(response.headers(), Utc::now());
        let error = classify_status(status.as_u16(), retry_after_secs);
        tracing::warn!(
            provider,
            step,
            status = status.as_u16(),
            class = error.class(),
            retry_after_s = ?retry_after_secs,
            "agent usage probe failed"
        );
        return Err(error);
    }
    response.json().await.map_err(|_| {
        tracing::warn!(
            provider,
            step,
            class = "schema",
            "agent usage probe: body is not JSON"
        );
        ProbeError::Schema
    })
}

fn classify_status(status: u16, retry_after_secs: Option<u64>) -> ProbeError {
    match status {
        401 | 403 => ProbeError::Unauthorized { status },
        429 => ProbeError::RateLimited { retry_after_secs },
        _ => ProbeError::Http {
            status,
            retry_after_secs,
        },
    }
}

/// `Retry-After` as delay-seconds or an HTTP-date (RFC 9110 §10.2.3).
fn retry_after_secs(headers: &reqwest::header::HeaderMap, now: DateTime<Utc>) -> Option<u64> {
    let value = headers
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim();
    if let Ok(secs) = value.parse::<u64>() {
        return Some(secs);
    }
    let at = DateTime::parse_from_rfc2822(value).ok()?;
    Some(
        at.with_timezone(&Utc)
            .signed_duration_since(now)
            .num_seconds()
            .max(0) as u64,
    )
}

/// A 2xx without the fields we parse: log the top-level KEYS (never values)
/// so schema drift is diagnosable from the log alone.
fn schema_error(provider: &'static str, body: &serde_json::Value) -> ProbeError {
    let keys: Vec<&str> = body
        .as_object()
        .map(|map| map.keys().map(String::as_str).collect())
        .unwrap_or_default();
    tracing::warn!(
        provider,
        class = "schema",
        ?keys,
        "agent usage probe: unexpected response shape"
    );
    ProbeError::Schema
}

/// The UI's reason line for an account whose last probe failed. `None` for
/// failures that need no explanation beyond the missing meters.
fn usage_error_message(
    harness: HarnessId,
    store_key: Option<&str>,
    active: bool,
    error: &ProbeError,
    entry: &UsageEntry,
    now: i64,
) -> Option<String> {
    let provider = match (harness, store_key) {
        (HarnessId::ClaudeCode, _) => "Anthropic",
        (HarnessId::Codex, _) => "OpenAI",
        (HarnessId::Cursor, _) => "Cursor",
        (HarnessId::Grok, _) => "xAI",
        (HarnessId::Devin, _) => "Devin",
        (_, Some(key)) => stores::upstream_vendor(key),
        _ => "the provider",
    };
    let retry = entry
        .retry_at
        .filter(|at| *at > now)
        .map(|at| format!(" — retrying in {}", short_duration(at - now)))
        .unwrap_or_default();
    Some(match error {
        ProbeError::RateLimited { .. } => format!("Rate limited by {provider}{retry}"),
        // Devin's key never expires on its own: a rejection means it was
        // revoked, whichever login it is.
        ProbeError::Unauthorized { .. } if harness == HarnessId::Devin => {
            "Signed out — sign in again".to_string()
        }
        ProbeError::Unauthorized { .. } if active => {
            // The CLI owns (and refreshes) the live token; it just hasn't yet.
            format!(
                "Session expired — it refreshes the next time {} runs",
                stores::cli_name(harness)
            )
        }
        // Codex-style slots aren't refreshed in the background (their
        // refresh tokens rotate); the CLI refreshes a switched-to login on
        // its next run. Hermes refreshes its own pool.
        ProbeError::Unauthorized { .. }
            if matches!(
                harness,
                HarnessId::Codex | HarnessId::Opencode | HarnessId::Pi
            ) =>
        {
            "Session expired — switch to it to refresh".to_string()
        }
        ProbeError::Unauthorized { .. } if harness == HarnessId::Hermes => {
            "Session expired — it refreshes the next time hermes uses it".to_string()
        }
        ProbeError::Unauthorized { .. } => "Signed out — sign in again".to_string(),
        ProbeError::Http { status, .. } if *status >= 500 => {
            format!("{provider} is having trouble ({status}){retry}")
        }
        ProbeError::Http { status, .. } => format!("Usage check failed ({status})"),
        ProbeError::Network { timeout: true } => format!("{provider} didn't respond{retry}"),
        ProbeError::Network { .. } => format!("Couldn't reach {provider}{retry}"),
        ProbeError::Schema => "Usage format changed — update zeron".to_string(),
        ProbeError::NoCredentials {
            why: NoCredentials::ApiKey,
        } => "API keys have no plan usage".to_string(),
        ProbeError::NoCredentials {
            why: NoCredentials::KeyExpired,
        } => "API key expired — connect again".to_string(),
        ProbeError::NoCredentials {
            why: NoCredentials::Missing,
        } => return None,
        ProbeError::NoCredentials {
            why: NoCredentials::Unsupported,
        } => "No usage view for this login".to_string(),
        ProbeError::UntrustedEndpoint => {
            "Usage skipped — this login names a server zeron doesn't recognize".to_string()
        }
    })
}

/// "45s" / "2m" / "3h" — the coarse countdown the reason line needs.
fn short_duration(ms: i64) -> String {
    let secs = (ms + 999) / 1000;
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", (secs + 59) / 60)
    } else {
        format!("{}h", (secs + 3599) / 3600)
    }
}

/// Codex `/wham/usage`: primary/secondary windows + the live plan.
fn codex_usage_snapshot(body: &serde_json::Value) -> Option<UsageSnapshot> {
    let rl = body.get("rate_limit")?;
    let mut windows = Vec::new();
    for key in ["primary_window", "secondary_window"] {
        if let Some(w) = rl.get(key)
            && let Some(used) = w.get("used_percent").and_then(|v| v.as_f64())
        {
            let span = w
                .get("limit_window_seconds")
                .and_then(|v| v.as_i64())
                .unwrap_or(0);
            windows.push(AgentUsageWindow {
                label: codex_window_label(span).to_string(),
                used_fraction: (used / 100.0) as f32,
                resets_at: parse_when(w.get("reset_at")),
            });
        }
    }
    if windows.is_empty() {
        return None;
    }
    // Live plan ("free"/"plus"/"pro"…) — beats the login-time JWT claim,
    // so a plan change shows up on the next forced refresh without a
    // re-login.
    let plan_label = codex_plan(str_field(body, "plan_type").as_deref());
    Some(UsageSnapshot {
        windows,
        plan_label,
    })
}

/// Windows from Claude's `/api/oauth/usage`: the 5-hour session and weekly
/// buckets, each a 0-100 `utilization` with an RFC3339 `resets_at`.
fn claude_usage_windows(body: &serde_json::Value) -> Option<UsageSnapshot> {
    let mut windows = Vec::new();
    for (key, label) in [("five_hour", "Session"), ("seven_day", "Week")] {
        if let Some(w) = body.get(key)
            && let Some(utilization) = w.get("utilization").and_then(|v| v.as_f64())
        {
            windows.push(AgentUsageWindow {
                label: label.to_string(),
                used_fraction: (utilization / 100.0) as f32,
                resets_at: parse_when(w.get("resets_at")),
            });
        }
    }
    (!windows.is_empty()).then_some(UsageSnapshot {
        windows,
        plan_label: None,
    })
}

/// The billing-cycle window from Cursor's `GetCurrentPeriodUsage`. The blended
/// percent is derived from spend/limit in cents: the payload's own
/// `totalPercentUsed` disagrees with the number Cursor's UI narrates ("You've
/// used 72% of your included usage" against `totalSpend`/`limit`, not the
/// precomputed 11.5). proto3 JSON omits zero-valued fields, so an absent
/// `limit` means unusable, not 0% — a synthetic 0% would render a healthy bar
/// for an account whose usage nobody knows.
fn cursor_usage_window(body: &serde_json::Value) -> Option<AgentUsageWindow> {
    let plan = body.get("planUsage")?;
    let limit = plan.get("limit").and_then(|v| v.as_f64())?;
    if limit <= 0.0 {
        return None;
    }
    let used = plan
        .get("totalSpend")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0);
    Some(AgentUsageWindow {
        label: "Month".to_string(),
        used_fraction: (used / limit) as f32,
        resets_at: json_ms(body.get("billingCycleEnd")),
    })
}

/// Unix-millis timestamp arriving as a JSON number or proto3 int64 string.
fn json_ms(value: Option<&serde_json::Value>) -> Option<DateTime<Utc>> {
    let ms = match value? {
        serde_json::Value::Number(n) => n.as_i64()?,
        serde_json::Value::String(s) => s.parse::<i64>().ok()?,
        _ => return None,
    };
    DateTime::<Utc>::from_timestamp_millis(ms)
}

fn scan_openai_url(output: &str) -> Option<String> {
    scan_https_url(output, &["auth.openai.com"])
}

/// Terminal escape sequences (colours, cursor moves) removed — the CLIs
/// colour their sign-in urls and codes.
fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        match chars.peek() {
            // CSI: parameters/intermediates up to a final byte in @..~.
            Some('[') => {
                chars.next();
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
            // OSC (hyperlinks): up to BEL or ESC \.
            Some(']') => {
                chars.next();
                while let Some(c) = chars.next() {
                    if c == '\u{7}' {
                        break;
                    }
                    if c == '\u{1b}' {
                        chars.next();
                        break;
                    }
                }
            }
            _ => {
                chars.next();
            }
        }
    }
    out
}

/// The first `https://` url in `output` (escapes stripped) whose host is
/// one of `hosts` or a subdomain of one (on a label boundary), with no
/// userinfo or explicit port — see [`stores::trusted_page`].
fn scan_https_url(output: &str, hosts: &[&str]) -> Option<String> {
    let text = strip_ansi(output);
    let mut rest = text.as_str();
    while let Some(start) = rest.find("https://") {
        let candidate = &rest[start..];
        let end = candidate
            .find(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '<' | '>'))
            .unwrap_or(candidate.len());
        let url = candidate[..end].trim_end_matches(['.', ',', ';', ')', ']']);
        if stores::trusted_page(url, hosts) {
            return Some(url.to_string());
        }
        rest = &candidate[end.max(1)..];
    }
    None
}

/// A device-flow user code a CLI printed for the user to type in: `Code:
/// ABCD-1234` / `enter code: ABCD-1234` on one line, or a line saying to
/// enter the code followed by the code alone. `None` for anything else — a
/// loopback login's output has no code.
fn scan_device_code(output: &str) -> Option<String> {
    let text = strip_ansi(output);
    let is_code = |token: &str| {
        let token = token.trim();
        (4..=16).contains(&token.len())
            && token.chars().any(|c| c.is_ascii_digit() || c == '-')
            && token
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '-')
            && !token.starts_with('-')
    };
    let lines: Vec<&str> = text.lines().collect();
    for (ix, line) in lines.iter().enumerate() {
        let lower = line.to_ascii_lowercase();
        let Some(at) = lower.rfind("code") else {
            continue;
        };
        let after = line[at + 4..].trim_start_matches([':', ' ', '\t']);
        if let Some(token) = after.split_whitespace().next()
            && is_code(token)
        {
            return Some(token.to_string());
        }
        if lower.contains("enter") || lower.trim_end().ends_with("code:") {
            if let Some(next) = lines[ix + 1..].iter().find(|l| !l.trim().is_empty())
                && is_code(next)
            {
                return Some(next.trim().to_string());
            }
        }
    }
    None
}

/// Path of the no-op "browser" script `start_codex_login` hands the CLI via
/// `BROWSER` so `codex login` doesn't open a second authorization tab (the
/// app opens the one tab). Unix only — `webbrowser` only consults `BROWSER`
/// on unix; elsewhere the CLI's own open is left as-is.
#[cfg(unix)]
fn ensure_noop_browser(root: &Path) -> Option<PathBuf> {
    const SCRIPT: &str = "#!/bin/sh\nexit 0\n";
    let path = root.join(".noop-browser");
    if std::fs::read_to_string(&path).ok().as_deref() != Some(SCRIPT) {
        std::fs::write(&path, SCRIPT).ok()?;
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).ok()?;
    }
    Some(path)
}

/// A "browser" that records the url it was asked to open into
/// `$ZERON_LOGIN_URL_FILE` instead of opening it — so the app opens the
/// one tab (on the requesting device, for a remote login) even when the CLI
/// never prints its sign-in url. Unix only, like [`ensure_noop_browser`].
#[cfg(unix)]
fn ensure_recording_browser(root: &Path) -> Option<PathBuf> {
    // `umask 077`: should the file not exist yet, it's still owner-only
    // (the sign-in pre-creates it 0600).
    const SCRIPT: &str = "#!/bin/sh\numask 077\n[ -n \"$ZERON_LOGIN_URL_FILE\" ] && \
                          printf '%s\\n' \"$1\" >> \"$ZERON_LOGIN_URL_FILE\"\nexit 0\n";
    let path = root.join(".record-browser");
    if std::fs::read_to_string(&path).ok().as_deref() != Some(SCRIPT) {
        std::fs::write(&path, SCRIPT).ok()?;
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).ok()?;
    }
    Some(path)
}

/// First JSONL frame with the given `ev` in a cursor-shim output accumulator.
fn scan_shim_event(output: &str, ev: &str) -> Option<serde_json::Value> {
    output.lines().find_map(|line| {
        let v: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
        (v.get("ev").and_then(|e| e.as_str()) == Some(ev)).then_some(v)
    })
}

/// The Cursor SDK's sign-in page from the shim's `auth-url` frame — only a
/// Cursor https page (it's opened on the requesting device).
fn scan_cursor_url(output: &str) -> Option<String> {
    str_field(&scan_shim_event(output, "auth-url")?, "url")
        .filter(|url| stores::trusted_page(url, CURSOR_DOMAINS))
}

/// Where the Cursor SDK's browser sign-in lives.
const CURSOR_DOMAINS: &[&str] = &["cursor.com", "cursor.sh"];

fn scan_shim_fatal(output: &str) -> Option<String> {
    str_field(&scan_shim_event(output, "fatal")?, "message")
}

type LoginChildHandles = (
    Arc<Mutex<Option<zeron_harness::process::Child>>>,
    Arc<Mutex<String>>,
    Arc<Mutex<Option<Option<i32>>>>,
);

/// Wire a spawned login child: both pipes accumulate into one output buffer
/// (the URL can land on either stream), and a monitor polls `try_wait` so the
/// child is reaped without owning it — the cancel path needs concurrent kill
/// access.
fn wire_login_child(mut child: zeron_harness::process::Child) -> LoginChildHandles {
    let output = Arc::new(Mutex::new(String::new()));
    for pipe in [
        child
            .stdout
            .take()
            .map(|s| Box::new(s) as Box<dyn tokio::io::AsyncRead + Send + Unpin>),
        child
            .stderr
            .take()
            .map(|s| Box::new(s) as Box<dyn tokio::io::AsyncRead + Send + Unpin>),
    ]
    .into_iter()
    .flatten()
    {
        let sink = output.clone();
        tokio::spawn(async move {
            use tokio::io::AsyncReadExt;
            let mut pipe = pipe;
            let mut buf = [0u8; 4096];
            while let Ok(n) = pipe.read(&mut buf).await {
                if n == 0 {
                    break;
                }
                lock(&sink).push_str(&String::from_utf8_lossy(&buf[..n]));
            }
        });
    }
    let child = Arc::new(Mutex::new(Some(child)));
    let exit: Arc<Mutex<Option<Option<i32>>>> = Arc::new(Mutex::new(None));
    {
        let child = child.clone();
        let exit = exit.clone();
        tokio::spawn(async move {
            loop {
                {
                    let mut slot = lock(&child);
                    match slot.as_mut().map(|c| c.try_wait()) {
                        None => break,
                        Some(Ok(Some(status))) => {
                            *lock(&exit) = Some(status.code());
                            *slot = None;
                            break;
                        }
                        Some(Ok(None)) => {}
                        Some(Err(_)) => {
                            *lock(&exit) = Some(None);
                            *slot = None;
                            break;
                        }
                    }
                }
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        });
    }
    (child, output, exit)
}

/// Wait briefly for the login child to print its authorize URL (empty when it
/// exits or stays silent past the deadline — the flow still completes via
/// poll; the UI just can't offer an open-browser button).
async fn await_login_url(
    output: &Arc<Mutex<String>>,
    exit: &Arc<Mutex<Option<Option<i32>>>>,
    scan: fn(&str) -> Option<String>,
) -> String {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(url) = scan(&lock(output)) {
            break url;
        }
        if lock(exit).is_some() || Instant::now() > deadline {
            break String::new();
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// 32 random bytes (two v4 uuids — OS randomness), base64url without
/// padding (43 chars): the size Claude Code uses for both its PKCE verifier
/// and its OAuth `state`.
fn random_url_token() -> String {
    let raw: Vec<u8> = uuid::Uuid::new_v4()
        .as_bytes()
        .iter()
        .chain(uuid::Uuid::new_v4().as_bytes())
        .copied()
        .collect();
    BASE64_URL.encode(&raw)
}

/// PKCE: a verifier of 32 random bytes and its S256 challenge.
fn pkce_pair() -> (String, String) {
    let verifier = random_url_token();
    let challenge = BASE64_URL.encode(Sha256::digest(verifier.as_bytes()));
    (verifier, challenge)
}

/// The loopback port an authorize url's `redirect_uri` lands on — where the
/// login's CLI (or our own listener) waits for the browser. `None` for flows
/// that don't redirect to this machine.
/// Whether a remote login's reported callback `port` may be forwarded on this
/// device: never a privileged port, and only the one its authorize `url`
/// actually redirects to — a buggy or hostile peer can't make us bind (and
/// receive local traffic on) an arbitrary loopback port.
pub(crate) fn tunnel_port_allowed(port: u16, url: Option<&str>) -> bool {
    port >= 1024
        && url
            .and_then(loopback_port)
            .is_some_and(|redirect| redirect == port)
}

pub(crate) fn loopback_port(url: &str) -> Option<u16> {
    let url = reqwest::Url::parse(url).ok()?;
    let redirect = url
        .query_pairs()
        .find(|(key, _)| key == "redirect_uri")
        .map(|(_, value)| value.into_owned())?;
    let redirect = reqwest::Url::parse(&redirect).ok()?;
    let loopback = matches!(
        redirect.host_str(),
        Some("localhost" | "127.0.0.1" | "[::1]")
    );
    (redirect.scheme() == "http" && loopback)
        .then(|| redirect.port())
        .flatten()
}

/// Serve Claude's loopback redirect until a `/callback` request carries our
/// `state`: its `code`, or why the provider refused. Strays — a favicon, a
/// request with anyone else's state — are answered and ignored, so a stray
/// can neither finish nor kill the login. Returns the browser's socket, to
/// be answered once the code is redeemed.
async fn await_claude_callback(
    listener: &tokio::net::TcpListener,
    state: &str,
) -> (Result<String, String>, tokio::net::TcpStream) {
    await_loopback_callback(listener, "/callback", state, "Claude").await
}

/// [`await_claude_callback`] for any loopback redirect: serve `path` until a
/// request carries our `state`; `who` names the provider in a refusal.
async fn await_loopback_callback(
    listener: &tokio::net::TcpListener,
    path: &str,
    state: &str,
    who: &str,
) -> (Result<String, String>, tokio::net::TcpStream) {
    use tokio::io::AsyncWriteExt as _;
    loop {
        let Ok((mut socket, _)) = listener.accept().await else {
            tokio::time::sleep(Duration::from_millis(100)).await;
            continue;
        };
        let Some(target) = read_request_target(&mut socket).await else {
            continue;
        };
        let url = reqwest::Url::parse(&format!("http://localhost{target}")).ok();
        let Some(url) = url.filter(|url| url.path() == path) else {
            let _ = socket
                .write_all(http_response("404 Not Found", &[], "").as_bytes())
                .await;
            continue;
        };
        let param = |name: &str| {
            url.query_pairs()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.into_owned())
        };
        if param("state").as_deref() != Some(state) {
            let _ = socket
                .write_all(http_response("400 Bad Request", &[], "Unknown sign-in.").as_bytes())
                .await;
            continue;
        }
        if let Some(code) = param("code").filter(|code| !code.is_empty()) {
            return (Ok(code), socket);
        }
        let reason = param("error_description")
            .or_else(|| param("error"))
            .unwrap_or_else(|| "no authorization code came back".into());
        return (Err(format!("{who} sign-in failed: {reason}")), socket);
    }
}

/// The request target of a browser's `GET` (headers read and discarded).
async fn read_request_target(socket: &mut tokio::net::TcpStream) -> Option<String> {
    use tokio::io::AsyncReadExt as _;
    let mut head = Vec::new();
    let mut chunk = [0u8; 2048];
    let read = async {
        while !head.windows(4).any(|w| w == b"\r\n\r\n") && head.len() < 16 * 1024 {
            let n = socket.read(&mut chunk).await.ok()?;
            if n == 0 {
                break;
            }
            head.extend_from_slice(&chunk[..n]);
        }
        Some(())
    };
    tokio::time::timeout(Duration::from_secs(10), read)
        .await
        .ok()??;
    let line = String::from_utf8_lossy(&head);
    let mut parts = line.lines().next()?.split_whitespace();
    (parts.next()? == "GET").then_some(())?;
    parts
        .next()
        .filter(|target| target.starts_with('/'))
        .map(str::to_string)
}

fn http_response(status: &str, headers: &[(&str, &str)], body: &str) -> String {
    let mut response = format!("HTTP/1.1 {status}\r\n");
    for (name, value) in headers {
        response.push_str(&format!("{name}: {value}\r\n"));
    }
    response.push_str(&format!(
        "Content-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n\r\n{body}",
        body.len()
    ));
    response
}

fn html_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Minimal percent-encoding for OAuth query params (matches `encodeURIComponent`
/// for the constant inputs used here).
fn urlencode(input: &str) -> String {
    let mut out = String::with_capacity(input.len() * 3);
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'_'
            | b'.'
            | b'!'
            | b'~'
            | b'*'
            | b'\''
            | b'('
            | b')' => out.push(byte as char),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn remove_if_exists(file: &Path) -> Result<(), EngineError> {
    match std::fs::remove_file(file) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
        _ => Ok(()),
    }
}

/// Atomic write via a same-dir temp file + rename; `secret` = 0600 from birth.
///
/// The temp file has a random name and is created exclusively (`O_CREAT |
/// O_EXCL`, which never follows or reuses a pre-planted path or symlink),
/// gets its permissions set on the open handle before any byte is written,
/// is fsynced, then renamed over `file` — replacing a symlink there rather
/// than writing through it. A crash leaves at worst an owner-only temp file
/// and never a torn target.
fn write_file_atomic(file: &Path, bytes: &[u8], secret: bool) -> Result<(), EngineError> {
    stage_file_atomic(file, bytes, secret)?
        .persist(file)
        .map_err(|e| EngineError::from(e.error))?;
    Ok(())
}

/// The first half of [`write_file_atomic`]: `bytes` written and synced to a
/// fresh, exclusive, same-directory temp file with the final permissions,
/// ready to `persist` over `file`. Callers that must re-check the target
/// right before replacing it (see `stores::merge_json_entry`) stage first, so
/// only the comparison and the rename remain between their check and the
/// swap. Dropping the returned file deletes it.
fn stage_file_atomic(
    file: &Path,
    bytes: &[u8],
    secret: bool,
) -> Result<tempfile::NamedTempFile, EngineError> {
    use std::io::Write;
    let dir = file
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let name = file
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut tmp = tempfile::Builder::new()
        .prefix(&format!(".{name}."))
        .suffix(".tmp")
        .tempfile_in(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // Secrets: owner-only. Anything else keeps the target's mode (a
        // config file the user made group-readable stays so).
        let mode = if secret {
            0o600
        } else {
            std::fs::metadata(file)
                .map(|m| m.permissions().mode() & 0o777)
                .unwrap_or(0o644)
        };
        tmp.as_file()
            .set_permissions(std::fs::Permissions::from_mode(mode))?;
    }
    #[cfg(not(unix))]
    let _ = secret;
    tmp.write_all(bytes)?;
    tmp.as_file().sync_all()?;
    Ok(tmp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_labels() {
        assert_eq!(
            claude_plan(Some("claude_max"), Some("default_claude_max_20x")).as_deref(),
            Some("Max 20×")
        );
        assert_eq!(
            claude_plan(Some("claude_pro"), None).as_deref(),
            Some("Pro")
        );
        assert_eq!(
            claude_plan(Some("claude_team"), Some("weird")).as_deref(),
            Some("Team")
        );
        assert_eq!(claude_plan(Some("free"), None), None);
        assert_eq!(codex_plan(Some("plus")).as_deref(), Some("ChatGPT Plus"));
        assert_eq!(codex_plan(Some("free")).as_deref(), Some("ChatGPT Free"));
        assert_eq!(codex_plan(None), None);
    }

    #[test]
    fn codex_window_labels_track_the_window_span() {
        // Codex free tier: one 30-day window (observed live:
        // limit_window_seconds = 2_592_000) — NOT a week.
        assert_eq!(codex_window_label(2_592_000), "Month");
        // Plus: 5-hour primary + weekly secondary.
        assert_eq!(codex_window_label(18_000), "Session");
        assert_eq!(codex_window_label(604_800), "Week");
        // Unknown/absent span falls back to the shortest label.
        assert_eq!(codex_window_label(0), "Session");
    }

    #[test]
    fn claude_usage_windows_map_buckets_and_percent() {
        let body = serde_json::json!({
            "five_hour": { "utilization": 6.0, "resets_at": "2026-04-08T18:59:59Z" },
            "seven_day": { "utilization": 35.0, "resets_at": "2026-04-14T16:59:59Z" },
            "extra_usage": { "is_enabled": true },
        });
        let snapshot = claude_usage_windows(&body).expect("windows");
        assert_eq!(snapshot.plan_label, None);
        let labels: Vec<_> = snapshot.windows.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(labels, ["Session", "Week"]);
        assert!((snapshot.windows[0].used_fraction - 0.06).abs() < 1e-6);
        assert!((snapshot.windows[1].used_fraction - 0.35).abs() < 1e-6);
        assert_eq!(
            snapshot.windows[1].resets_at,
            Some(
                "2026-04-14T16:59:59Z"
                    .parse::<chrono::DateTime<Utc>>()
                    .unwrap()
            )
        );
    }

    #[test]
    fn claude_usage_windows_none_without_any_bucket() {
        // A 200 with only `extra_usage` (no rate windows) is not a snapshot —
        // the UI then says "Usage unavailable" instead of showing nothing.
        assert!(claude_usage_windows(&serde_json::json!({ "extra_usage": {} })).is_none());
        assert!(claude_usage_windows(&serde_json::json!({ "five_hour": {} })).is_none());
    }

    #[test]
    fn cursor_usage_window_derives_percent_from_spend_not_total_percent() {
        // Real payload flavor (observed shape): proto3 JSON with string int64
        // cycle bounds. `totalPercentUsed` (11.53) contradicts the derived
        // 28846/40000 = 72.1% that Cursor's own UI narrates — derive.
        let body = serde_json::json!({
            "billingCycleStart": "1768399334000",
            "billingCycleEnd": "1771077734000",
            "planUsage": {
                "totalSpend": 28846,
                "includedSpend": 23222,
                "bonusSpend": 5624,
                "remaining": 11154,
                "limit": 40000,
                "totalPercentUsed": 11.5384,
            },
            "displayMessage": "You've used 72% of your included usage",
        });
        let window = cursor_usage_window(&body).expect("window");
        assert_eq!(window.label, "Month");
        assert!((window.used_fraction - 0.72115).abs() < 1e-5);
        assert_eq!(
            window.resets_at,
            Some(chrono::DateTime::<Utc>::from_timestamp_millis(1_771_077_734_000).unwrap())
        );
    }

    #[test]
    fn cursor_usage_window_absent_limit_is_unusable_not_zero() {
        // proto3 JSON omits zero-valued fields: an account with no usage-based
        // spend simply lacks `limit` — never render a synthetic 0% bar.
        assert!(cursor_usage_window(&serde_json::json!({ "planUsage": {} })).is_none());
        assert!(cursor_usage_window(&serde_json::json!({ "planUsage": { "limit": 0 } })).is_none());
    }

    #[test]
    fn cursor_usage_window_no_spend_is_zero_percent() {
        let window = cursor_usage_window(&serde_json::json!({
            "billingCycleEnd": 1771077734000i64,
            "planUsage": { "limit": 40000 },
        }))
        .expect("window");
        assert_eq!(window.used_fraction, 0.0);
    }

    #[test]
    fn cursor_auth_parses_and_gates_on_expiry() {
        // The SDK's StoredSdkCredentials shape (credential-store.d.ts, 1.0.28).
        let live = serde_json::json!({
            "version": 1,
            "backendUrl": "https://api2.cursor.sh",
            "apiKey": "key_abc123",
            "apiKeyExpiresAtMs": now_ms() + 86_400_000,
            "email": "dev@example.com",
            "createdAtMs": now_ms() - 1000,
        });
        let detected = parse_cursor_auth(live.clone()).expect("parses");
        assert_eq!(detected.account_key, "dev@example.com");
        assert_eq!(detected.profile.email, "dev@example.com");
        assert_eq!(detected.profile.auth_kind, AgentAuthKind::Oauth);
        assert!(
            detected
                .profile
                .plan
                .as_deref()
                .unwrap()
                .starts_with("Key expires")
        );
        assert!(cursor_key_usable(&live));

        let expired = serde_json::json!({
            "version": 1,
            "apiKey": "key_abc123",
            "apiKeyExpiresAtMs": now_ms() - 1000,
        });
        let detected = parse_cursor_auth(expired.clone()).expect("expired still detects");
        assert_eq!(detected.profile.plan.as_deref(), Some("Key expired"));
        // No email → keyed (and labeled) off the key itself, codex-api-key style.
        assert!(detected.account_key.starts_with("api-key:"));
        assert!(!cursor_key_usable(&expired));

        // No expiry field = never expires.
        assert!(cursor_key_usable(&serde_json::json!({"apiKey": "k"})));
        assert!(parse_cursor_auth(serde_json::json!({"version": 1})).is_none());
    }

    #[test]
    fn cursor_shim_output_scan() {
        let output = concat!(
            "npm warn something unrelated\n",
            "{\"ev\":\"auth-url\",\"url\":\"https://cursor.com/loginDeepControl?challenge=x\"}\n",
        );
        assert_eq!(
            scan_cursor_url(output).as_deref(),
            Some("https://cursor.com/loginDeepControl?challenge=x")
        );
        assert_eq!(scan_cursor_url("no frames here"), None);
        // Only a Cursor https page is ever opened on the requesting device.
        for url in [
            "https://evil.example/loginDeepControl?challenge=x",
            "https://cursor.com.evil.example/loginDeepControl",
            "http://cursor.com/loginDeepControl",
            "https://user@cursor.com/loginDeepControl",
            "https://cursor.com:8443/loginDeepControl",
            "javascript:alert(1)",
        ] {
            let frame = format!("{}\n", serde_json::json!({ "ev": "auth-url", "url": url }));
            assert_eq!(scan_cursor_url(&frame), None, "{url}");
        }
        assert!(
            scan_cursor_url(
                "{\"ev\":\"auth-url\",\"url\":\"https://www.cursor.com/loginDeepControl?c=1\"}\n"
            )
            .is_some()
        );
        assert_eq!(
            scan_shim_fatal("{\"ev\":\"fatal\",\"message\":\"cursor login failed: boom\"}\n")
                .as_deref(),
            Some("cursor login failed: boom")
        );
    }

    #[test]
    fn openai_url_scan() {
        assert_eq!(
            scan_openai_url("open https://auth.openai.com/authorize?x=1 in your browser\n")
                .as_deref(),
            Some("https://auth.openai.com/authorize?x=1")
        );
        assert_eq!(scan_openai_url("nothing here"), None);
        for output in [
            "open https://auth.openai.com.evil.example/authorize?x=1",
            "open https://auth.openai.com@evil.example/authorize?x=1",
            "open https://auth.openai.com:8443/authorize?x=1",
            "open http://auth.openai.com/authorize?x=1",
        ] {
            assert_eq!(scan_openai_url(output), None, "{output}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn noop_browser_script_is_stable_and_executable() {
        let root = tempfile::tempdir().expect("tempdir");
        let path = ensure_noop_browser(root.path()).expect("script");
        assert!(path.ends_with(".noop-browser"));
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "#!/bin/sh\nexit 0\n"
        );
        {
            use std::os::unix::fs::PermissionsExt;
            assert!(path.metadata().unwrap().permissions().mode() & 0o111 != 0);
        }
        // A second ensure is idempotent (same path, same content).
        assert_eq!(ensure_noop_browser(root.path()), Some(path));
    }

    #[test]
    fn urlencode_matches_encode_uri_component() {
        assert_eq!(
            urlencode("org:create_api_key user:profile"),
            "org%3Acreate_api_key%20user%3Aprofile"
        );
        assert_eq!(urlencode("https://a/b"), "https%3A%2F%2Fa%2Fb");
    }

    #[test]
    fn compose_claude_credentials_live_mcp_wins_over_stale_slot() {
        let target = serde_json::json!({
            "claudeAiOauth": { "accessToken": "alice" },
            "trustedDeviceToken": "alice-device",
            "mcpOAuth": { "github": { "accessToken": "stale" } },
            "pluginSecrets": { "old": true },
        });
        let live = serde_json::json!({
            "claudeAiOauth": { "accessToken": "bob" },
            "trustedDeviceToken": "bob-device",
            "mcpOAuth": { "github": { "accessToken": "live" } },
            "pluginSecrets": { "live": true },
        });
        let composed = compose_claude_credentials(&target, Some(&live));
        assert_eq!(composed["claudeAiOauth"]["accessToken"], "alice");
        assert_eq!(composed["trustedDeviceToken"], "alice-device");
        assert_eq!(composed["mcpOAuth"]["github"]["accessToken"], "live");
        assert_eq!(composed["pluginSecrets"]["live"], true);
        assert!(composed["pluginSecrets"].get("old").is_none());
    }

    #[test]
    fn compose_claude_credentials_does_not_resurrect_absent_live_mcp() {
        let target = serde_json::json!({
            "claudeAiOauth": { "accessToken": "alice" },
            "mcpOAuth": { "github": { "accessToken": "stale" } },
        });
        let live = serde_json::json!({
            "claudeAiOauth": { "accessToken": "bob" },
        });
        let composed = compose_claude_credentials(&target, Some(&live));
        assert_eq!(composed["claudeAiOauth"]["accessToken"], "alice");
        assert!(composed.get("mcpOAuth").is_none());
    }

    #[test]
    fn compose_claude_credentials_passthrough_without_live_or_oauth() {
        let target = serde_json::json!({
            "claudeAiOauth": { "accessToken": "alice" },
            "mcpOAuth": { "github": { "accessToken": "slot" } },
        });
        assert_eq!(compose_claude_credentials(&target, None), target);

        let api_key = serde_json::json!({ "apiKey": "sk-x" });
        let live = serde_json::json!({
            "claudeAiOauth": { "accessToken": "bob" },
            "mcpOAuth": { "github": { "accessToken": "live" } },
        });
        assert_eq!(
            compose_claude_credentials(&api_key, Some(&live)),
            api_key,
            "opaque/API-key shapes activate verbatim"
        );
    }

    #[test]
    fn with_claude_ai_oauth_keeps_sibling_keys() {
        let creds = serde_json::json!({
            "claudeAiOauth": { "accessToken": "old" },
            "mcpOAuth": { "github": { "accessToken": "keep" } },
        });
        let updated = with_claude_ai_oauth(&creds, serde_json::json!({ "accessToken": "new" }));
        assert_eq!(updated["claudeAiOauth"]["accessToken"], "new");
        assert_eq!(updated["mcpOAuth"]["github"]["accessToken"], "keep");
    }
}

#[cfg(test)]
mod probe_tests {
    use super::*;

    fn snapshot() -> UsageSnapshot {
        UsageSnapshot {
            windows: vec![AgentUsageWindow {
                label: "5h".into(),
                used_fraction: 0.4,
                resets_at: None,
            }],
            plan_label: None,
        }
    }

    #[test]
    fn remote_login_tunnels_only_forward_the_redirect_port() {
        let url = "https://auth.openai.com/oauth/authorize?client_id=x\
                   &redirect_uri=http%3A%2F%2Flocalhost%3A1455%2Fauth%2Fcallback";
        assert!(tunnel_port_allowed(1455, Some(url)));
        // A port the authorize url doesn't redirect to, a privileged port,
        // or no url at all: refused.
        assert!(!tunnel_port_allowed(22, Some(url)));
        assert!(!tunnel_port_allowed(8080, Some(url)));
        assert!(!tunnel_port_allowed(1455, None));
        let privileged = "https://x/authorize?redirect_uri=http%3A%2F%2Flocalhost%3A80%2Fcb";
        assert!(!tunnel_port_allowed(80, Some(privileged)));
    }

    #[tokio::test]
    async fn a_pasted_code_with_another_logins_state_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let accounts = AgentAccounts::new(AgentAccountsConfig {
            data_dir: dir.path().to_path_buf(),
            claude_config_dir: dir.path().join("claude"),
            claude_config_file: dir.path().join("claude.json"),
            codex_home: dir.path().join("codex"),
            cursor_sdk_auth_file: dir.path().join("cursor.json"),
            claude_keychain_service: None,
            antigravity_home: None,
            antigravity_keychain: false,
            ..AgentAccountsConfig::isolated(dir.path())
        });
        let start = accounts.start_claude_paste_login();
        assert!(start.url.contains("state="));
        // The verifier never rides the authorize url.
        let verifier = match lock(&accounts.inner.flows).get(&start.login_id) {
            Some(LoginFlow::Claude {
                verifier, state, ..
            }) => {
                assert_ne!(verifier, state);
                verifier.clone()
            }
            _ => panic!("paste flow registered"),
        };
        assert!(!start.url.contains(&verifier));
        let error = accounts
            .complete_login(&start.login_id, "code#not-the-state")
            .await
            .unwrap_err();
        assert!(error.to_string().contains("different sign-in"), "{error}");
    }

    #[test]
    fn only_401_and_403_count_as_a_rejected_token() {
        assert!(matches!(
            classify_status(401, None),
            ProbeError::Unauthorized { .. }
        ));
        assert!(matches!(
            classify_status(403, None),
            ProbeError::Unauthorized { .. }
        ));
        assert!(matches!(
            classify_status(429, Some(90)),
            ProbeError::RateLimited {
                retry_after_secs: Some(90)
            }
        ));
        assert!(matches!(
            classify_status(503, None),
            ProbeError::Http { status: 503, .. }
        ));
    }

    #[test]
    fn rate_limit_backoff_honours_retry_after_within_bounds() {
        let limited = |secs| ProbeError::RateLimited {
            retry_after_secs: secs,
        };
        assert_eq!(limited(Some(120)).backoff(), Duration::from_secs(120));
        // Clamped: a 0 must not turn into hammering, a day must not stall usage.
        assert_eq!(limited(Some(0)).backoff(), Duration::from_secs(30));
        assert_eq!(limited(Some(86_400)).backoff(), Duration::from_secs(3600));
        assert_eq!(limited(None).backoff(), Duration::from_secs(300));
    }

    #[test]
    fn retry_after_parses_seconds_and_http_dates() {
        let now = Utc::now();
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(reqwest::header::RETRY_AFTER, "45".parse().unwrap());
        assert_eq!(retry_after_secs(&headers, now), Some(45));
        let at = (now + chrono::TimeDelta::seconds(120)).to_rfc2822();
        headers.insert(reqwest::header::RETRY_AFTER, at.parse().unwrap());
        let parsed = retry_after_secs(&headers, now).unwrap();
        assert!((119..=120).contains(&parsed));
    }

    #[test]
    fn a_failure_keeps_the_last_good_usage_and_backs_off() {
        let mut entry = UsageEntry::default();
        entry.record(Ok(snapshot()), "creds".into(), 1_000);
        assert!(entry.error.is_none());
        let later = 1_000 + FORCED_MIN_INTERVAL.as_millis() as i64 + 1;
        entry.record(
            Err(ProbeError::RateLimited {
                retry_after_secs: Some(120),
            }),
            "creds".into(),
            later,
        );
        // The meters that were right a minute ago survive the 429.
        assert_eq!(entry.usage, Some(snapshot()));
        assert_eq!(entry.fetched_at, Some(1_000));
        // Inside the Retry-After window nothing re-probes — not even with
        // fresh credentials, since a rate limit isn't about the token.
        let inside = later + 60_000;
        assert!(!entry.probe_due("creds", inside));
        assert!(!entry.probe_due("new-creds", inside));
        assert!(entry.probe_due("creds", later + 121_000));
    }

    #[test]
    fn a_rejected_token_reprobes_once_the_credentials_change() {
        let mut entry = UsageEntry::default();
        entry.record(
            Err(ProbeError::Unauthorized { status: 401 }),
            "old".into(),
            1_000,
        );
        let soon = 1_000 + FORCED_MIN_INTERVAL.as_millis() as i64 + 1;
        assert!(!entry.probe_due("old", soon));
        assert!(entry.probe_due("refreshed", soon));
    }

    #[test]
    fn usage_errors_read_as_reasons() {
        let mut entry = UsageEntry::default();
        entry.record(
            Err(ProbeError::RateLimited {
                retry_after_secs: Some(120),
            }),
            "c".into(),
            0,
        );
        let message = |harness, active, error: &ProbeError| {
            usage_error_message(harness, None, active, error, &entry, 0)
        };
        assert_eq!(
            message(
                HarnessId::ClaudeCode,
                true,
                &ProbeError::RateLimited {
                    retry_after_secs: Some(120)
                }
            )
            .as_deref(),
            Some("Rate limited by Anthropic — retrying in 2m")
        );
        assert_eq!(
            message(
                HarnessId::ClaudeCode,
                false,
                &ProbeError::Unauthorized { status: 401 }
            )
            .as_deref(),
            Some("Signed out — sign in again")
        );
        assert_eq!(
            message(
                HarnessId::Codex,
                false,
                &ProbeError::NoCredentials {
                    why: NoCredentials::Missing
                }
            ),
            None
        );
    }

    #[test]
    fn the_usage_cache_file_round_trips() {
        let mut entry = UsageEntry::default();
        entry.record(Ok(snapshot()), "c".into(), 5);
        let file = UsageCacheFile {
            entries: HashMap::from([("claude-code:acct".to_string(), entry)]),
        };
        let json = serde_json::to_string(&file).unwrap();
        let back: UsageCacheFile = serde_json::from_str(&json).unwrap();
        let entry = &back.entries["claude-code:acct"];
        assert_eq!(entry.usage, Some(snapshot()));
        assert_eq!(entry.fetched_at, Some(5));
        // The fingerprint is a digest, never the credential blob.
        assert!(!json.contains("accessToken"));
    }
}

#[cfg(test)]
mod login_tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// Temp homes for every provider; never the real logins or Keychain.
    fn config(root: &Path) -> AgentAccountsConfig {
        AgentAccountsConfig {
            data_dir: root.join("data"),
            claude_config_dir: root.join("claude"),
            claude_config_file: root.join("claude.json"),
            codex_home: root.join("codex"),
            cursor_sdk_auth_file: root.join("cursor-sdk").join("auth.json"),
            claude_keychain_service: None,
            antigravity_home: Some(root.join("gemini")),
            antigravity_keychain: false,
            ..AgentAccountsConfig::isolated(root)
        }
    }

    fn write(path: &Path, contents: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }

    #[test]
    fn loopback_port_reads_only_local_http_redirects() {
        assert_eq!(
            loopback_port(
                "https://auth.openai.com/oauth/authorize?client_id=x\
                 &redirect_uri=http%3A%2F%2Flocalhost%3A1455%2Fauth%2Fcallback"
            ),
            Some(1455)
        );
        assert_eq!(
            loopback_port(
                "https://accounts.google.com/o/oauth2/auth?redirect_uri=http://127.0.0.1:51234/"
            ),
            Some(51234)
        );
        // Anthropic's manual page, a remote host, no redirect at all.
        assert_eq!(
            loopback_port(
                "https://claude.ai/oauth/authorize?redirect_uri=https%3A%2F%2Fconsole.anthropic.com%2Foauth%2Fcode%2Fcallback"
            ),
            None
        );
        assert_eq!(
            loopback_port("https://x.test/?redirect_uri=http://example.com:80/cb"),
            None
        );
        assert_eq!(loopback_port("https://cursor.com/loginDeepControl"), None);
        assert_eq!(loopback_port(""), None);
    }

    #[test]
    fn the_paste_code_fallback_keeps_anthropics_manual_redirect() {
        let tmp = tempfile::tempdir().unwrap();
        let accounts = AgentAccounts::new(config(tmp.path()));
        let start = accounts.start_claude_paste_login();
        assert_eq!(start.mode, AgentLoginMode::PasteCode);
        assert_eq!(start.callback_port, None);
        assert!(
            start
                .url
                .contains("redirect_uri=https%3A%2F%2Fconsole.anthropic.com")
        );
    }

    #[tokio::test]
    async fn antigravity_login_is_detected_from_settings_and_token_presence() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("gemini");
        let acp = home.join("antigravity-acp");
        // Never signed in.
        assert_eq!(detect_antigravity_login(&home, false).await, None);
        // A first sign-in: the consumer token, no saved method yet.
        write(&acp.join("acp_token.json"), "{}");
        let login = detect_antigravity_login(&home, false).await.unwrap();
        assert_eq!(login.method, "oauth-personal");
        let row = login.account();
        assert_eq!(row.harness, HarnessId::Antigravity);
        assert_eq!(row.display_name.as_deref(), Some("Google account"));
        assert_eq!(row.email, None);
        assert!(row.active && !row.switchable);
        assert!(row.usage_windows.is_empty() && row.usage_error.is_none());
        // Gemini Enterprise keeps its own token file.
        write(
            &acp.join("settings.json"),
            r#"{"auth": {"type": "oauth-business"}}"#,
        );
        assert_eq!(detect_antigravity_login(&home, false).await, None);
        write(&acp.join("acp_business_token.json"), "{}");
        let business = detect_antigravity_login(&home, false).await.unwrap();
        assert_eq!(
            business.account().plan_label.as_deref(),
            Some("Gemini Enterprise")
        );
        // Key-based methods: the saved method is the login.
        write(
            &acp.join("settings.json"),
            r#"{"auth": {"type": "gemini-api-key"}}"#,
        );
        let key = detect_antigravity_login(&home, false).await.unwrap();
        assert_eq!(
            key.account().display_name.as_deref(),
            Some("Gemini API key")
        );
        assert_eq!(key.account().auth_kind, Some(AgentAuthKind::ApiKey));
        // Signed out: the method stays, the Google token is gone.
        write(
            &acp.join("settings.json"),
            r#"{"auth": {"type": "oauth-personal"}}"#,
        );
        std::fs::remove_file(acp.join("acp_token.json")).unwrap();
        assert_eq!(detect_antigravity_login(&home, false).await, None);
    }

    #[tokio::test]
    async fn the_list_shows_the_live_antigravity_login_as_one_active_row() {
        let tmp = tempfile::tempdir().unwrap();
        let config = config(tmp.path());
        let accounts = AgentAccounts::new(config.clone());
        let listed = |snapshot: &AgentAccountsSnapshot| {
            snapshot
                .accounts
                .iter()
                .filter(|a| a.harness == HarnessId::Antigravity)
                .cloned()
                .collect::<Vec<_>>()
        };
        assert!(listed(&accounts.list(false).await.unwrap()).is_empty());
        write(
            &config
                .antigravity_home
                .as_ref()
                .unwrap()
                .join("antigravity-acp/acp_token.json"),
            "{}",
        );
        let rows = listed(&accounts.list(true).await.unwrap());
        assert_eq!(rows.len(), 1);
        assert!(rows[0].active && !rows[0].switchable);
        // The live login can't be forgotten out from under the agent.
        assert!(
            accounts
                .forget(HarnessId::Antigravity, &rows[0].id)
                .await
                .is_err()
        );
    }

    /// A stand-in for Anthropic's token endpoint: accepts one code and
    /// replies with a token set naming the account.
    async fn token_server() -> (String, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let url = format!("http://{}/token", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let mut request = vec![0u8; 16 * 1024];
                let n = socket.read(&mut request).await.unwrap_or(0);
                let request = String::from_utf8_lossy(&request[..n]).to_string();
                let leaky = request.contains("\"code\":\"leaky-code\"");
                let body = if request.contains("\"code\":\"good-code\"")
                    && request.contains("\"redirect_uri\":\"http://localhost:")
                {
                    serde_json::json!({
                        "access_token": "access",
                        "refresh_token": "refresh",
                        "expires_in": 3600,
                        "account": { "email_address": "new@example.com", "uuid": "acct-new" },
                    })
                    .to_string()
                } else if leaky {
                    // An odd error that echoes token material.
                    r#"{"error":"invalid_grant","access_token":"LEAKED-SECRET-123"}"#.into()
                } else {
                    String::new()
                };
                let status = if body.is_empty() || leaky {
                    "400 Bad Request"
                } else {
                    "200 OK"
                };
                let _ = socket
                    .write_all(
                        http_response(status, &[("Content-Type", "application/json")], &body)
                            .as_bytes(),
                    )
                    .await;
            }
        });
        (url, task)
    }

    async fn browser_get(port: u16, target: &str) -> String {
        let mut socket = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        socket
            .write_all(
                format!("GET {target} HTTP/1.1\r\nHost: localhost:{port}\r\n\r\n").as_bytes(),
            )
            .await
            .unwrap();
        let mut response = String::new();
        let _ = socket.read_to_string(&mut response).await;
        response
    }

    async fn poll_until_settled(accounts: &AgentAccounts, login_id: &str) -> AgentLoginPoll {
        for _ in 0..100 {
            let poll = accounts.poll_login(login_id).await.unwrap();
            if poll.status != AgentLoginStatus::Pending {
                return poll;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("login never settled");
    }

    #[tokio::test]
    async fn claude_loopback_login_finishes_when_the_browser_lands() {
        let tmp = tempfile::tempdir().unwrap();
        let (token_url, _server) = token_server().await;
        let endpoints = ProbeEndpoints {
            claude_loopback_token: token_url,
            claude_profile: "http://127.0.0.1:9/profile".into(),
            ..Default::default()
        };
        let accounts =
            AgentAccounts::with_endpoints(config(tmp.path()), endpoints, Default::default());
        let start = accounts.start_login(HarnessId::ClaudeCode).await.unwrap();
        assert_eq!(start.mode, AgentLoginMode::Browser);
        let port = start.callback_port.unwrap();
        let state = reqwest::Url::parse(&start.url)
            .unwrap()
            .query_pairs()
            .find(|(k, _)| k == "state")
            .unwrap()
            .1
            .into_owned();

        // Strays neither finish nor kill the login.
        assert!(
            browser_get(port, "/favicon.ico")
                .await
                .starts_with("HTTP/1.1 404")
        );
        assert!(
            browser_get(port, "/callback?code=good-code&state=someone-else")
                .await
                .starts_with("HTTP/1.1 400")
        );
        let poll = accounts.poll_login(&start.login_id).await.unwrap();
        assert_eq!(poll.status, AgentLoginStatus::Pending);

        // The real redirect: redeemed, saved, and the browser sent on to the
        // CLI's success page.
        let response = browser_get(port, &format!("/callback?code=good-code&state={state}")).await;
        assert!(response.starts_with("HTTP/1.1 302"), "{response}");
        assert!(response.contains(CLAUDE_LOOPBACK_SUCCESS_URL));
        let poll = poll_until_settled(&accounts, &start.login_id).await;
        assert_eq!(poll.status, AgentLoginStatus::Done, "{:?}", poll.message);
        // No live login to strand: the new account goes live at once.
        let snapshot = accounts.list(false).await.unwrap();
        assert!(
            snapshot
                .accounts
                .iter()
                .any(|a| a.harness == HarnessId::ClaudeCode
                    && a.email.as_deref() == Some("new@example.com")
                    && a.active)
        );
    }

    fn write_live_claude(root: &Path, uuid: &str, email: &str, token: &str) {
        let config = config(root);
        write(
            &config.claude_config_file,
            &serde_json::json!({
                "oauthAccount": { "accountUuid": uuid, "emailAddress": email },
                "projects": { "/keep/me": {} },
            })
            .to_string(),
        );
        write(
            &config.claude_creds_file(),
            &serde_json::json!({
                "claudeAiOauth": { "accessToken": token, "refreshToken": "rt", "expiresAt": 1 },
                "mcpOAuth": { "server": "keep" },
            })
            .to_string(),
        );
    }

    async fn sign_in_new_account(accounts: &AgentAccounts) {
        let start = accounts.start_login(HarnessId::ClaudeCode).await.unwrap();
        let port = start.callback_port.unwrap();
        let state = reqwest::Url::parse(&start.url)
            .unwrap()
            .query_pairs()
            .find(|(k, _)| k == "state")
            .unwrap()
            .1
            .into_owned();
        browser_get(port, &format!("/callback?code=good-code&state={state}")).await;
        let poll = poll_until_settled(accounts, &start.login_id).await;
        assert_eq!(poll.status, AgentLoginStatus::Done, "{:?}", poll.message);
    }

    fn loopback_accounts(root: &Path, token_url: String) -> AgentAccounts {
        let endpoints = ProbeEndpoints {
            claude_loopback_token: token_url,
            claude_profile: "http://127.0.0.1:9/profile".into(),
            ..Default::default()
        };
        AgentAccounts::with_endpoints(config(root), endpoints, Default::default())
    }

    #[tokio::test]
    async fn re_signing_in_the_live_account_replaces_its_dead_tokens() {
        let tmp = tempfile::tempdir().unwrap();
        let (token_url, _server) = token_server().await;
        let accounts = loopback_accounts(tmp.path(), token_url);
        write_live_claude(tmp.path(), "acct-new", "new@example.com", "dead");
        accounts.list(false).await.unwrap();

        sign_in_new_account(&accounts).await;

        // The fresh tokens are live (not clobbered by a re-snapshot of the
        // dead ones), shared MCP OAuth survives, and so does the slot.
        let creds = read_json(&config(tmp.path()).claude_creds_file()).unwrap();
        assert_eq!(creds["claudeAiOauth"]["accessToken"], "access");
        assert_eq!(creds["mcpOAuth"]["server"], "keep");
        let snapshot = accounts.list(false).await.unwrap();
        let rows: Vec<_> = snapshot
            .accounts
            .iter()
            .filter(|a| a.harness == HarnessId::ClaudeCode)
            .collect();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].active);
        let slot = accounts
            .read_slots(HarnessId::ClaudeCode)
            .into_iter()
            .next()
            .unwrap();
        assert_eq!(slot.credentials["claudeAiOauth"]["accessToken"], "access");
    }

    #[tokio::test]
    async fn signing_in_another_account_leaves_the_live_login_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let (token_url, _server) = token_server().await;
        let accounts = loopback_accounts(tmp.path(), token_url);
        write_live_claude(tmp.path(), "acct-bob", "bob@example.com", "bob-token");
        accounts.list(false).await.unwrap();

        sign_in_new_account(&accounts).await;

        let creds = read_json(&config(tmp.path()).claude_creds_file()).unwrap();
        assert_eq!(creds["claudeAiOauth"]["accessToken"], "bob-token");
        let snapshot = accounts.list(false).await.unwrap();
        assert!(
            snapshot
                .accounts
                .iter()
                .any(|a| a.email.as_deref() == Some("new@example.com") && !a.active)
        );
        assert!(
            snapshot
                .accounts
                .iter()
                .any(|a| a.email.as_deref() == Some("bob@example.com") && a.active)
        );
    }

    /// A failed exchange reports its status only — never the provider's
    /// body — on the browser page and in the poll.
    #[tokio::test]
    async fn a_failed_claude_exchange_never_echoes_the_response_body() {
        let tmp = tempfile::tempdir().unwrap();
        let (token_url, _server) = token_server().await;
        let endpoints = ProbeEndpoints {
            claude_loopback_token: token_url,
            claude_profile: "http://127.0.0.1:9/profile".into(),
            ..Default::default()
        };
        let accounts =
            AgentAccounts::with_endpoints(config(tmp.path()), endpoints, Default::default());
        let start = accounts.start_login(HarnessId::ClaudeCode).await.unwrap();
        let port = start.callback_port.unwrap();
        let state = reqwest::Url::parse(&start.url)
            .unwrap()
            .query_pairs()
            .find(|(k, _)| k == "state")
            .unwrap()
            .1
            .into_owned();
        let page = browser_get(port, &format!("/callback?code=leaky-code&state={state}")).await;
        assert!(page.starts_with("HTTP/1.1 400"), "{page}");
        assert!(!page.contains("LEAKED"), "{page}");
        assert!(!page.contains("invalid_grant"), "{page}");
        assert!(
            page.contains("Anthropic rejected the code (400 Bad Request) — try again."),
            "{page}"
        );
        let poll = poll_until_settled(&accounts, &start.login_id).await;
        assert_eq!(poll.status, AgentLoginStatus::Error);
        let message = poll.message.unwrap();
        assert_eq!(
            message,
            "Anthropic rejected the code (400 Bad Request) — try again."
        );
    }

    #[tokio::test]
    async fn a_refused_claude_callback_is_an_error_not_a_silent_reset() {
        let tmp = tempfile::tempdir().unwrap();
        let accounts = AgentAccounts::new(config(tmp.path()));
        let start = accounts.start_login(HarnessId::ClaudeCode).await.unwrap();
        let port = start.callback_port.unwrap();
        let state = reqwest::Url::parse(&start.url)
            .unwrap()
            .query_pairs()
            .find(|(k, _)| k == "state")
            .unwrap()
            .1
            .into_owned();
        let response = browser_get(
            port,
            &format!(
                "/callback?error=access_denied&error_description=User%20declined&state={state}"
            ),
        )
        .await;
        assert!(response.starts_with("HTTP/1.1 400"), "{response}");
        let poll = poll_until_settled(&accounts, &start.login_id).await;
        assert_eq!(poll.status, AgentLoginStatus::Error);
        assert!(poll.message.unwrap().contains("User declined"));
    }

    #[tokio::test]
    async fn a_login_for_another_device_publishes_its_callback_until_it_ends() {
        let tmp = tempfile::tempdir().unwrap();
        let routes = zeron_preview::login::CallbackRoutes::default();
        let accounts = AgentAccounts::with_callback_routes(config(tmp.path()), routes.clone());
        // A local login publishes nothing.
        let local = accounts.start_login(HarnessId::ClaudeCode).await.unwrap();
        assert!(!routes.is_registered(&local.login_id));
        accounts.cancel_login(&local.login_id);
        // A login started for device-a serves its port to device-a only.
        let remote = accounts
            .start_login_for(HarnessId::ClaudeCode, Some("device-a"))
            .await
            .unwrap();
        assert!(remote.callback_port.is_some());
        assert!(routes.is_registered(&remote.login_id));
        accounts.cancel_login(&remote.login_id);
        assert!(!routes.is_registered(&remote.login_id));
    }
}
