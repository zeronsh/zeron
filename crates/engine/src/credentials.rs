//! Credential broker — Cloud runner only (docs/design/cloud-device.md,
//! "Engine changes"). Laptops never construct one: their agents use the
//! CLIs' own logins, unchanged.
//!
//! Right before a harness spawn the host asks the broker to fill
//! [`RunRequest::env`] (serde-skipped, so nothing here can reach the session
//! doc, the journal, or the wire):
//!
//! - **Codex**: a `codex` vault grant (ChatGPT tokens) becomes a managed
//!   `CODEX_HOME` at `{data_dir}/cloud/codex-home` whose `auth.json` holds the
//!   access token with an EMPTY refresh token (the vault is the only
//!   refresher; deleting the field instead makes codex reject the file).
//!   Else an `openai-key` grant becomes `{"OPENAI_API_KEY": key}` there.
//!   Else nothing — codex runs on whatever native login the sandbox has.
//!
//!   Why a file and not the app-server's `chatgptAuthTokens` login: codex
//!   0.159 does expose `account/login/start {type: "chatgptAuthTokens"}` and
//!   the `account/chatgptAuthTokens/refresh` server request, but both are
//!   marked "[UNSTABLE] FOR OPENAI INTERNAL USE ONLY - DO NOT USE" in its own
//!   schema. The file path needs no private API and is sufficient: ChatGPT
//!   access tokens live for days, every spawn re-grants (rewriting the file
//!   when the grant changed), the background refresher rewrites it ~10 min
//!   before expiry while a grant is cached, and a long-lived app-server that
//!   hits a 401 reloads `auth.json` from disk before attempting its own
//!   refresh ("Reloading auth for account …" in codex's auth recovery), so it
//!   picks up the rewritten token.
//! - **Claude Code** (only ever the unmodified CLI, only on the user's own
//!   Cloud device): (1) a `claude` vault grant (the user's subscription
//!   sign-in) is written as `claudeAiOauth` into the CLI's own
//!   `.credentials.json` (`$CLAUDE_CONFIG_DIR` or `~/.claude`; 0600, atomic,
//!   sibling keys such as `mcpOAuth` preserved) with an EMPTY refresh token —
//!   the vault owns the refresh token and this engine never sees one — and
//!   rewritten by the background refresher ~10 min before `expiresAt`;
//!   (2) else a native sandbox login (`claude auth login` wrote a real refresh
//!   token) is left alone; (3) else an `anthropic-key` grant (the user's own
//!   API key) becomes `ANTHROPIC_API_KEY`; else the run fails with Claude's
//!   own not-logged-in error. A native login the grant displaces is backed up
//!   (`{data_dir}/cloud/claude-native-credentials.json`) and restored when the
//!   grant goes away.
//! - **GitHub**: a `github` grant — a one-hour GitHub App installation token
//!   for the session repository (the user's own GitHub token never leaves the
//!   vault; the vault keeps a session on the login it started with, and a grant
//!   names that login as `account`) — becomes a git credential store at
//!   `{data_dir}/cloud/git-credentials` (0600) wired once via
//!   `git config --global credential.https://github.com.helper`, and
//!   `~/.config/gh/hosts.yml` for `gh` (only when absent or already
//!   Zeron-managed). Refreshed in the background ~10 min before expiry. Never
//!   `GH_TOKEN`/`GITHUB_TOKEN`: a run's environment is fixed at spawn, `gh`
//!   prefers it over hosts.yml, and the token expires within the hour, so a
//!   warm session's `gh` would start failing while `git push` kept working.
//!   Pushes and pull requests therefore act as the App.
//!
//! - **Every other agent** runs on the vault credentials
//!   [`zeron_proto::providers::cloud_credentials`] lists for it, asked for in
//!   one signed round trip (`POST /vault/{orgId}/grants`; one provider at a
//!   time from an edge that predates it). Within one account group the first
//!   credential granted is used (a sign-in before a key); separate groups
//!   are all used. Keys go in the run's environment (`CURSOR_API_KEY`,
//!   `XAI_API_KEY`, `META_API_KEY`, `OPENROUTER_API_KEY`, …). Sign-ins go
//!   where the agent's own CLI keeps them, never with a refresh token, and
//!   are rewritten ahead of expiry like Claude's:
//!   - Grok: an entry of `$GROK_HOME/auth.json` (`~/.grok`) keyed by its
//!     `{issuer}::{client_id}` scope, `refresh_token` empty, under grok's
//!     `auth.json.lock` flock. A login of the sandbox's own (a real refresh
//!     token) is left alone and used instead.
//!   - Pi's ChatGPT: the `openai-codex` entry of
//!     `$PI_CODING_AGENT_DIR/auth.json` (`~/.pi/agent`) from the `codex`
//!     grant Codex runs on (`refresh` empty), under Pi's proper-lockfile
//!     lock; Pi's other providers' entries are never touched.
//!   - Devin: `$XDG_DATA_HOME/devin/credentials.toml` (0600). A file the
//!     broker didn't write (its SHA-256 is kept beside the managed files) is
//!     the sandbox's own login and stays.
//!   - Pi's Claude: the `anthropic` entry beside it, from the `claude` grant
//!     Claude Code runs on.
//!   - Hermes: its ChatGPT (`codex`), Nous and xAI logins as records of
//!     `$HERMES_HOME/auth.json` (`providers.<id>`), under Hermes' own
//!     `auth.lock`, with a fresh `last_refresh` (Hermes adopts a record
//!     written under its lock only when it's newer than its own); its shared
//!     Nous store is pointed at an empty one of the machine's. The xAI token
//!     is replaced while more than 75 minutes remain: Hermes would start its
//!     own refresh an hour ahead.
//!   - Antigravity: on its Google login, `acp_token.json` holding the access
//!     token (refresh fields present but empty) and `auth.type = oauth-personal`,
//!     plus a loopback CCPA reverse proxy (`AGY_ACP_CCPA_BASE_URL`) that injects
//!     live vault tokens on every CCPA request (and retries once on 401) while
//!     writing a far-future `expiry` in `acp_token.json` so `agy_acp_server`'s
//!     in-memory credentials never expire mid-turn. On the key: `GEMINI_API_KEY`,
//!     and `auth.type = gemini-api-key` in its `settings.json` (only when absent
//!     or plain JSON).
//!   - Muse: its whole `auth.json` document as the vault holds it (refresh
//!     tokens empty), with `TBH_CREDENTIAL_BACKEND=file`; a file the broker
//!     didn't write (its SHA-256 is kept) is Muse's own and stays.
//!   - OpenCode: its sign-ins (ChatGPT, Copilot) go in one run variable,
//!     [`zeron_harness::opencode::CREDENTIALS_ENV`], which its harness
//!     installs through OpenCode's credential API on every server it boots
//!     (2.x keeps logins in its database), and never passes on.
//!
//!   A missing CLI is installed the first time a run needs it (once per
//!   agent; a failure is retried after a while). With no credential at all
//!   and no login of the sandbox's own, the run fails saying where to sign
//!   in or add a key.
//!
//! Grants are cached per provider until 5 min before expiry; a network failure
//! falls back to a cached, still-unexpired grant. A 4xx (`not_authorized`,
//! `not_found`, `needs_reconnect`, revoked, kill switch) means "not connected":
//! the cache entry and any file it backed are dropped.

mod antigravity_proxy;

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::{Duration, Instant};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64URL;
use serde::Deserialize;
use serde_json::{Value, json};

use zeron_proto::providers::{CloudDelivery, cloud_credentials};
use zeron_proto::{GithubRepo, HarnessId, RunRequest, VaultProvider};

use crate::http_error::describe_http_error;
use crate::runner::{EdgeErrorBody, RunnerIdentity, write_private_atomic};

/// The one error every "no GitHub grant" path reports. The
/// `github_not_connected:` prefix is the machine-readable code (RPC errors
/// carry only a message); the rest is user-facing.
pub const GITHUB_NOT_CONNECTED: &str =
    "github_not_connected: GitHub isn't connected for this device — connect it in Settings → Cloud";

/// Refusals that mean this session's own GitHub login can't serve it (the
/// vault's messages): it stays on that login, so a run says so rather than
/// working on with no way to push.
const GITHUB_SESSION_REFUSALS: [&str; 3] = [
    "github_account_needs_reconnect:",
    "github_account_disconnected:",
    "github_account_no_access:",
];

/// The words for the user, when `reason` is such a refusal.
pub fn github_session_refusal(reason: &str) -> Option<String> {
    GITHUB_SESSION_REFUSALS.iter().find_map(|prefix| {
        let at = reason.find(prefix)?;
        Some(reason[at + prefix.len()..].trim().to_owned())
    })
}

/// A Cloud run of a provider with no login anywhere on the device.
pub const CODEX_NOT_SIGNED_IN: &str = "Codex isn't signed in on Cloud. Sign in with ChatGPT, pick an \
     account or add an OpenAI API key in Settings → Providers → Cloud, then send again.";
pub const CLAUDE_NOT_SIGNED_IN: &str = "Claude Code isn't signed in on Cloud. Sign in with Claude, pick an \
     account or add an Anthropic API key in Settings → Providers → Cloud, then send again.";

/// A spawn uses a cached grant only while it has at least this much life left.
const GRANT_SLACK: Duration = Duration::from_secs(5 * 60);
/// The background refresher re-grants this far ahead of expiry.
const REFRESH_AHEAD: Duration = Duration::from_secs(10 * 60);
/// A "not connected" answer is reused this long before asking again.
const MISSING_TTL: Duration = Duration::from_secs(30);
/// A run re-asks the vault for a grant older than this, so switching the
/// account in Settings → Providers takes effect on the next turn (grants
/// otherwise live until shortly before they expire — hours).
const RUN_REVALIDATE: Duration = Duration::from_secs(30);
/// Background refresher cadence (wall-clock re-evaluation; sandboxes stop
/// and resume, which tokio's monotonic timers do not see).
const REFRESH_TICK: Duration = Duration::from_secs(60);
/// How often the refresher looks for a GitHub grant that isn't there yet.
const GITHUB_PROBE_EVERY: Duration = Duration::from_secs(5 * 60);
/// Offline fallback only for grants that are still valid this much longer.
const OFFLINE_MIN_VALIDITY: Duration = Duration::from_secs(30);
/// Antigravity reads its token once per connection and, past it, starts an
/// interactive browser sign-in: a run starts on one with at least this much
/// life (the vault refreshes Google tokens with less left).
const ANTIGRAVITY_MIN_LIFE: Duration = Duration::from_secs(50 * 60);
/// Hermes refreshes an xAI token an hour before it expires (when it lives
/// longer than 45 minutes) and can't — the refresh token is the vault's — so
/// the machine replaces it while more than this is left.
const XAI_OAUTH_AHEAD: Duration = Duration::from_secs(75 * 60);

/// The life a cached grant of `provider` must still have for a run to start
/// on it, else the vault is asked again.
fn run_min_validity(provider: VaultProvider) -> Duration {
    match provider {
        VaultProvider::Antigravity => ANTIGRAVITY_MIN_LIFE,
        VaultProvider::XaiOauth => XAI_OAUTH_AHEAD,
        _ => GRANT_SLACK,
    }
}

/// How far ahead of expiry the background refresher replaces `provider`'s
/// file (what the agent itself would otherwise try to refresh, and can't).
fn refresh_ahead(provider: VaultProvider) -> Duration {
    match provider {
        VaultProvider::Antigravity => ANTIGRAVITY_MIN_LIFE,
        VaultProvider::XaiOauth => XAI_OAUTH_AHEAD,
        _ => REFRESH_AHEAD,
    }
}
const GITHUB_MAX_PAGES: usize = 10;
const HOSTS_MARKER: &str =
    "# Managed by Zeron Cloud (GitHub grant) — delete this line to manage gh yourself.";

/// `{data_dir}/cloud/codex-home` — the managed `CODEX_HOME`.
pub fn managed_codex_home(data_dir: &Path) -> PathBuf {
    data_dir.join("cloud").join("codex-home")
}

/// `{data_dir}/cloud/git-credentials` — the git credential store.
pub fn git_credentials_path(data_dir: &Path) -> PathBuf {
    data_dir.join("cloud").join("git-credentials")
}

/// A vault grant (`POST /vault/{orgId}/grant` reply). `Debug` redacts secrets.
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Grant {
    #[serde(default)]
    pub provider: String,
    pub access_token: String,
    /// Unix ms.
    pub expires_at: i64,
    #[serde(default)]
    pub account_id: Option<String>,
    #[serde(default)]
    pub id_token: Option<String>,
    #[serde(default)]
    pub account: Option<String>,
    /// Claude: OAuth scopes of the access token.
    #[serde(default)]
    pub scopes: Option<Vec<String>>,
    /// Claude: `pro` / `max` / … as Claude Code records it.
    #[serde(default)]
    pub subscription_type: Option<String>,
    /// Non-secret fields the agent's credential file needs beside the token:
    /// Grok's entry identity and `scope`, Devin's server urls.
    #[serde(default)]
    pub settings: BTreeMap<String, String>,
    #[serde(default)]
    pub generation: i64,
    /// The vault account it comes from (an older vault names none).
    #[serde(default)]
    pub slot: Option<String>,
}

/// One login's part of a credential fingerprint
/// ([`CredentialBroker::credential`]): `provider@slot:generation`, the
/// account it comes from and its version (`provider:generation` from a vault
/// that names no account).
fn credential_part(provider: &str, grant: &Grant) -> String {
    match &grant.slot {
        Some(slot) => format!("{provider}@{slot}:{}", grant.generation),
        None => format!("{provider}:{}", grant.generation),
    }
}

/// The accounts a credential fingerprint was made with (`provider@slot`,
/// sorted, joined by `,`), without their versions: whose models a catalog
/// discovered with it are, the same after a token refresh, another after a
/// switch of account. `None` for one that names no account: a machine's own
/// login, none at all, or an older vault's grant.
pub fn credential_accounts(credential: &str) -> Option<String> {
    let mut accounts = credential
        .split(',')
        .map(|part| {
            let part = part.strip_suffix(":expiring").unwrap_or(part);
            let (account, generation) = part.rsplit_once(':')?;
            (account.contains('@') && generation.parse::<i64>().is_ok()).then_some(account)
        })
        .collect::<Option<Vec<_>>>()?;
    accounts.sort_unstable();
    (!accounts.is_empty()).then(|| accounts.join(","))
}

impl std::fmt::Debug for Grant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Grant")
            .field("provider", &self.provider)
            .field("expires_at", &self.expires_at)
            .field("account", &self.account)
            .field("generation", &self.generation)
            .finish_non_exhaustive()
    }
}

impl Grant {
    fn remaining_ms(&self) -> i64 {
        self.expires_at - crate::now_ms()
    }
}

/// What a grant request resolved to.
#[derive(Debug, Clone)]
pub enum GrantOutcome {
    Granted(Grant),
    /// The vault holds no usable connection of this provider for this device.
    NotConnected(String),
    /// The vault could not be reached and no cached grant is still valid.
    Unavailable(String),
}

enum Cached {
    Granted { grant: Grant, at: Instant },
    Missing { at: Instant, reason: String },
}

/// Filesystem + endpoint knobs (env-resolved in production, explicit in tests).
#[derive(Debug, Clone)]
pub struct BrokerConfig {
    /// Engine data dir (`{data_dir}/cloud/…` holds the managed files).
    pub data_dir: PathBuf,
    /// The sandbox user's home: `git config --global` and `~/.config/gh`.
    pub home: PathBuf,
    /// GitHub REST base (`https://api.github.com`).
    pub github_api: String,
    /// Claude Code's config dir (`$CLAUDE_CONFIG_DIR` or `~/.claude`) —
    /// holds `.credentials.json`.
    pub claude_config_dir: PathBuf,
    /// Claude Code's state file (`~/.claude.json`, or
    /// `$CLAUDE_CONFIG_DIR/.claude.json`).
    pub claude_config_file: PathBuf,
    /// Whether the engine's own environment already carries a Claude login
    /// the CLI inherits (`ANTHROPIC_API_KEY`, `CLAUDE_CODE_OAUTH_TOKEN`, …).
    pub claude_env_login: bool,
    /// Whether the engine's own environment carries an OpenAI / Codex API
    /// key the CLI inherits.
    pub codex_env_login: bool,
    /// The session repository as `owner/name`: `github` grants are minted
    /// for its owner's App installation. `None` = no GitHub grant at all.
    pub github_repo: Option<String>,
    /// Where the other agents keep their logins.
    pub agents: AgentPaths,
    /// Credential variables (a [`CloudDelivery::Env`] name) the engine's own
    /// environment already sets: the agents that read one have a login of
    /// the machine's own.
    pub env_logins: BTreeSet<String>,
    /// Install an agent's missing CLI before its first run (off in tests).
    pub install_missing: bool,
}

/// Where each agent beyond Claude Code and Codex keeps its login on this
/// machine (every path its CLI's own default unless its env var moves it).
#[derive(Debug, Clone)]
pub struct AgentPaths {
    /// `$GROK_HOME` (`~/.grok`): `auth.json`.
    pub grok_home: PathBuf,
    /// `$PI_CODING_AGENT_DIR` (`~/.pi/agent`): `auth.json`.
    pub pi_agent_dir: PathBuf,
    /// `$XDG_DATA_HOME/devin`: `credentials.toml`.
    pub devin_dir: PathBuf,
    /// Antigravity's `GEMINI_HOME`: `antigravity-acp/{settings,acp_token}.json`.
    pub antigravity_home: Option<PathBuf>,
    /// `$XDG_DATA_HOME/opencode`.
    pub opencode_dir: PathBuf,
    /// `$HERMES_HOME` (`~/.hermes`).
    pub hermes_home: PathBuf,
    /// `$XDG_CONFIG_HOME/muse`.
    pub muse_dir: PathBuf,
    /// Muse's login: `$MUSE_AUTH_PATH`, else `{muse_dir}/auth.json`.
    pub muse_auth_file: PathBuf,
    /// The Cursor SDK's saved login (`~/.cursor/sdk/auth.json`).
    pub cursor_sdk_auth: PathBuf,
}

impl AgentPaths {
    /// Every agent's default under `home`, no environment consulted.
    pub fn under(home: &Path) -> Self {
        let data = home.join(".local").join("share");
        Self {
            grok_home: home.join(".grok"),
            pi_agent_dir: home.join(".pi").join("agent"),
            devin_dir: data.join("devin"),
            antigravity_home: Some(home.join(".gemini")),
            opencode_dir: data.join("opencode"),
            hermes_home: home.join(".hermes"),
            muse_dir: home.join(".config").join("muse"),
            muse_auth_file: home.join(".config").join("muse").join("auth.json"),
            cursor_sdk_auth: home.join(".cursor").join("sdk").join("auth.json"),
        }
    }

    /// As the agents resolve them in this process's environment.
    pub fn detect(home: &Path) -> Self {
        let env = |name: &str| {
            std::env::var_os(name)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        };
        let data = env("XDG_DATA_HOME").unwrap_or_else(|| home.join(".local").join("share"));
        let config = env("XDG_CONFIG_HOME").unwrap_or_else(|| home.join(".config"));
        let defaults = Self::under(home);
        Self {
            grok_home: env("GROK_HOME").unwrap_or(defaults.grok_home),
            pi_agent_dir: env("PI_CODING_AGENT_DIR").unwrap_or(defaults.pi_agent_dir),
            devin_dir: data.join("devin"),
            antigravity_home: zeron_harness::acp::antigravity_home().ok(),
            opencode_dir: data.join("opencode"),
            hermes_home: env("HERMES_HOME").unwrap_or(defaults.hermes_home),
            muse_auth_file: env("MUSE_AUTH_PATH")
                .unwrap_or_else(|| config.join("muse").join("auth.json")),
            muse_dir: config.join("muse"),
            cursor_sdk_auth: defaults.cursor_sdk_auth,
        }
    }
}

/// Every credential variable any agent reads on Cloud.
fn credential_env_names() -> BTreeSet<&'static str> {
    HarnessId::ALL
        .iter()
        .flat_map(|h| cloud_credentials(*h))
        .filter_map(|c| match c.delivery {
            CloudDelivery::Env(names) => Some(names),
            CloudDelivery::File | CloudDelivery::Api => None,
        })
        .flatten()
        .copied()
        .collect()
}

impl BrokerConfig {
    pub fn detect(data_dir: &Path) -> Self {
        let home = crate::repos::home_dir();
        let env = |name: &str| std::env::var(name).ok().filter(|v| !v.trim().is_empty());
        let claude_dir = env("CLAUDE_CONFIG_DIR").map(PathBuf::from);
        Self {
            agents: AgentPaths::detect(&home),
            env_logins: credential_env_names()
                .into_iter()
                .filter(|name| env(name).is_some())
                .map(str::to_owned)
                .collect(),
            install_missing: true,
            data_dir: data_dir.to_path_buf(),
            claude_config_file: claude_dir
                .as_ref()
                .map(|dir| dir.join(".claude.json"))
                .unwrap_or_else(|| home.join(".claude.json")),
            claude_config_dir: claude_dir.unwrap_or_else(|| home.join(".claude")),
            claude_env_login: [
                "ANTHROPIC_API_KEY",
                "ANTHROPIC_AUTH_TOKEN",
                "CLAUDE_CODE_OAUTH_TOKEN",
            ]
            .iter()
            .any(|name| env(name).is_some()),
            codex_env_login: ["OPENAI_API_KEY", "CODEX_API_KEY"]
                .iter()
                .any(|name| env(name).is_some()),
            github_api: env("ZERON_GITHUB_API_URL")
                .unwrap_or_else(|| "https://api.github.com".into()),
            github_repo: crate::runner::session()
                .and_then(crate::runner::CloudSession::github_repo),
            home,
        }
    }
}

#[derive(Default)]
struct FileState {
    git_helper_configured: bool,
    last_github_probe: Option<Instant>,
}

struct Inner {
    config: BrokerConfig,
    identity: Arc<RunnerIdentity>,
    tokens: Arc<dyn zeron_rpc::TokenSource>,
    http: reqwest::Client,
    cache: Mutex<HashMap<VaultProvider, Cached>>,
    /// One grant request in flight per device: the vault's replay fence
    /// rejects a `ts` that isn't above the last accepted one, so concurrent
    /// signed requests could race each other into `stale`.
    grant_gate: tokio::sync::Mutex<()>,
    /// Serializes writes of the managed files.
    files: tokio::sync::Mutex<FileState>,
    /// The user's git identity, looked up once per GitHub login.
    git_identity: tokio::sync::Mutex<Option<(String, GitIdentity)>>,
    refresher: Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// Whether this machine's Codex CLI can start ([`CredentialBroker::ensure_codex_starts`]).
    codex_check: tokio::sync::Mutex<CodexCheck>,
    /// Per agent: whether its CLI is installed ([`CredentialBroker::ensure_installed`]).
    installs: Mutex<HashMap<HarnessId, Arc<tokio::sync::Mutex<InstallCheck>>>>,
    /// Per account: when its usage was last reported, and what it said
    /// ([`CredentialBroker::report_usage`]).
    usage_reported: Mutex<HashMap<String, (Instant, String)>>,
    /// Lazy loopback CCPA auth reverse proxy for Antigravity vault runs.
    antigravity_proxy: Mutex<Option<antigravity_proxy::AntigravityProxy>>,
    /// Serializes lazy startup of [`Self::antigravity_proxy`].
    antigravity_proxy_init: tokio::sync::Mutex<()>,
}

/// An agent's install check: done once it passes; a failed install is
/// retried after [`INSTALL_RETRY`], and until then runs say why.
#[derive(Default)]
enum InstallCheck {
    #[default]
    Unchecked,
    Passed,
    FailedAt(Instant, String),
}

const INSTALL_RETRY: Duration = Duration::from_secs(10 * 60);

/// The Codex install check: done once it passes; a failed repair is retried
/// after [`CODEX_REPAIR_RETRY`] (a network blip mid-install must not leave
/// Codex broken until the machine restarts).
#[derive(Default)]
enum CodexCheck {
    #[default]
    Unchecked,
    Passed,
    FailedAt(Instant),
}

const CODEX_REPAIR_RETRY: Duration = Duration::from_secs(10 * 60);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The runner's credential broker. Cheap to clone.
#[derive(Clone)]
pub struct CredentialBroker {
    inner: Arc<Inner>,
}

impl CredentialBroker {
    pub fn new(
        config: BrokerConfig,
        identity: Arc<RunnerIdentity>,
        tokens: Arc<dyn zeron_rpc::TokenSource>,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                config,
                identity,
                tokens,
                http: crate::runner::http_client(),
                cache: Mutex::new(HashMap::new()),
                grant_gate: tokio::sync::Mutex::new(()),
                files: tokio::sync::Mutex::new(FileState::default()),
                git_identity: tokio::sync::Mutex::new(None),
                refresher: Mutex::new(None),
                codex_check: tokio::sync::Mutex::new(CodexCheck::Unchecked),
                installs: Mutex::new(HashMap::new()),
                usage_reported: Mutex::new(HashMap::new()),
                antigravity_proxy: Mutex::new(None),
                antigravity_proxy_init: tokio::sync::Mutex::new(()),
            }),
        }
    }

    pub fn config(&self) -> &BrokerConfig {
        &self.inner.config
    }

    /// Start the background refresher (idempotent): primes the GitHub grant
    /// so terminals can `git push` right after boot, then keeps the GitHub
    /// files and any cached Codex grant fresh ahead of expiry.
    pub fn start(&self) {
        let mut slot = lock(&self.inner.refresher);
        if slot.is_some() || tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        let weak: Weak<Inner> = Arc::downgrade(&self.inner);
        *slot = Some(tokio::spawn(async move {
            let mut first = true;
            loop {
                let Some(inner) = weak.upgrade() else { return };
                let broker = CredentialBroker { inner };
                if std::mem::take(&mut first) {
                    tokio::join!(broker.refresh_due(), broker.prime_for_first_run());
                } else {
                    broker.refresh_due().await;
                }
                drop(broker);
                tokio::time::sleep(REFRESH_TICK).await;
            }
        }));
    }

    /// What the first run after a boot asks for: fetched now, beside the
    /// engine's boot, so a machine woken by a message doesn't make that run
    /// wait on one round trip after another. A grant it finds cached within
    /// [`RUN_REVALIDATE`] is used as it is.
    async fn prime_for_first_run(&self) {
        if let Some(grant) = self.github_grant().await {
            let _ = self.git_identity(&grant).await;
        }
        for provider in [VaultProvider::Claude, VaultProvider::Codex] {
            let granted = self.grant_for_run(provider).await;
            if let (VaultProvider::Codex, GrantOutcome::Granted(grant)) = (provider, granted) {
                self.report_codex_usage(&grant).await;
            }
        }
    }

    /// ChatGPT's usage, read here with this machine's grant (ChatGPT answers
    /// a machine, never the vault's Cloudflare servers) and reported as its
    /// account's ([`Self::report_usage`]): a device's Cloud card shows it
    /// before any ChatGPT run.
    async fn report_codex_usage(&self, grant: &Grant) {
        let reply = self
            .inner
            .http
            .get(CODEX_USAGE_URL)
            .bearer_auth(&grant.access_token)
            .header("chatgpt-account-id", grant.account_id.as_deref().unwrap_or_default())
            .timeout(Duration::from_secs(15))
            .send()
            .await;
        let body = match reply {
            Ok(res) if res.status().is_success() => res.json::<Value>().await.ok(),
            _ => None,
        };
        let Some((windows, plan)) = body
            .as_ref()
            .and_then(|body| crate::agent_accounts::usage_windows_from_reply(HarnessId::Codex, body))
        else {
            tracing::debug!("ChatGPT usage unreadable here");
            return;
        };
        self.report_usage(&credential_part("codex", grant), windows, plan).await;
    }

    pub fn shutdown(&self) {
        if let Some(task) = lock(&self.inner.refresher).take() {
            task.abort();
        }
        let _ = lock(&self.inner.antigravity_proxy).take();
    }

    fn has_antigravity_proxy(&self) -> bool {
        lock(&self.inner.antigravity_proxy).is_some()
    }

    async fn ensure_antigravity_proxy(&self, fixed_upstream: Option<String>) -> Option<String> {
        if let Some(proxy) = lock(&self.inner.antigravity_proxy).as_ref() {
            return Some(proxy.base_url().to_owned());
        }
        let _guard = self.inner.antigravity_proxy_init.lock().await;
        if let Some(proxy) = lock(&self.inner.antigravity_proxy).as_ref() {
            return Some(proxy.base_url().to_owned());
        }
        match antigravity_proxy::AntigravityProxy::start(
            Arc::downgrade(&self.inner),
            fixed_upstream,
        )
        .await
        {
            Ok(proxy) => {
                let url = proxy.base_url().to_owned();
                *lock(&self.inner.antigravity_proxy) = Some(proxy);
                Some(url)
            }
            Err(error) => {
                tracing::warn!(
                    %error,
                    "credentials: failed to start Antigravity CCPA auth proxy; falling back to static token"
                );
                None
            }
        }
    }

    /// Fetch a live Antigravity access token for the loopback CCPA proxy.
    /// Uses [`GRANT_SLACK`] (5m) rather than [`ANTIGRAVITY_MIN_LIFE`] (50m) so
    /// ongoing requests reuse valid cached tokens and transparently refresh
    /// ahead of real Google expiry. When `force_refresh` is true (after an
    /// upstream 401), bypasses the cache once to ask the vault directly.
    async fn antigravity_proxy_grant(&self, force_refresh: bool) -> GrantOutcome {
        if !force_refresh {
            return self
                .grant_within(VaultProvider::Antigravity, GRANT_SLACK, Some(RUN_REVALIDATE))
                .await;
        }
        let _gate = self.inner.grant_gate.lock().await;
        let fetched = self.fetch(VaultProvider::Antigravity).await;
        self.record(VaultProvider::Antigravity, fetched)
    }

    // -- grants -------------------------------------------------------------

    /// A grant valid for at least 5 more minutes (cached or fresh).
    pub async fn grant(&self, provider: VaultProvider) -> GrantOutcome {
        self.grant_within(provider, GRANT_SLACK, None).await
    }

    /// [`Self::grant`] for a run about to start: a cached grant older than
    /// [`RUN_REVALIDATE`] is checked with the vault first, so the run uses
    /// the provider's active account as it is now.
    async fn grant_for_run(&self, provider: VaultProvider) -> GrantOutcome {
        self.grant_within(provider, run_min_validity(provider), Some(RUN_REVALIDATE))
            .await
    }

    async fn grant_within(
        &self,
        provider: VaultProvider,
        min_validity: Duration,
        max_age: Option<Duration>,
    ) -> GrantOutcome {
        if let Some(hit) = self.cached(provider, min_validity, max_age) {
            return hit;
        }
        let _gate = self.inner.grant_gate.lock().await;
        if let Some(hit) = self.cached(provider, min_validity, max_age) {
            return hit;
        }
        let fetched = self.fetch(provider).await;
        self.record(provider, fetched)
    }

    /// Grants for a run of an agent that takes several providers: each one
    /// cached within [`RUN_REVALIDATE`] as it is, the rest in ONE signed
    /// request (`POST /vault/{orgId}/grants`), else one at a time from an
    /// edge that predates it. Never `github`.
    async fn grants_for_run(
        &self,
        providers: &[VaultProvider],
    ) -> HashMap<VaultProvider, GrantOutcome> {
        let mut out = HashMap::new();
        let mut wanted: Vec<VaultProvider> = Vec::new();
        let hit = |p| self.cached(p, run_min_validity(p), Some(RUN_REVALIDATE));
        for &provider in providers {
            if provider == VaultProvider::Github || wanted.contains(&provider) {
                continue;
            }
            match hit(provider) {
                Some(outcome) => {
                    out.insert(provider, outcome);
                }
                None => wanted.push(provider),
            }
        }
        if wanted.len() <= 1 {
            for provider in wanted {
                out.insert(provider, self.grant_for_run(provider).await);
            }
            return out;
        }
        let _gate = self.inner.grant_gate.lock().await;
        wanted.retain(|&provider| match hit(provider) {
            Some(outcome) => {
                out.insert(provider, outcome);
                false
            }
            None => true,
        });
        if wanted.is_empty() {
            return out;
        }
        let fetched = match self.fetch_many(&wanted).await {
            Some(fetched) => fetched,
            // An edge without the batch route: one at a time (the gate is
            // held, so the timestamps stay ordered).
            None => {
                let mut one_by_one = HashMap::new();
                for &provider in &wanted {
                    one_by_one.insert(provider, self.fetch(provider).await);
                }
                one_by_one
            }
        };
        for provider in wanted {
            let outcome = fetched.get(&provider).cloned().unwrap_or_else(|| {
                GrantOutcome::Unavailable(format!(
                    "the vault answered without {}",
                    provider.as_str()
                ))
            });
            out.insert(provider, self.record(provider, outcome));
        }
        out
    }

    /// `POST /vault/{orgId}/grants` for `providers`. `None` when the edge
    /// doesn't have the route (404): the caller asks one at a time.
    async fn fetch_many(
        &self,
        providers: &[VaultProvider],
    ) -> Option<HashMap<VaultProvider, GrantOutcome>> {
        let all = |outcome: GrantOutcome| {
            Some(
                providers
                    .iter()
                    .map(|p| (*p, outcome.clone()))
                    .collect::<HashMap<_, _>>(),
            )
        };
        let bearer = match self.inner.tokens.token().await {
            Ok(token) => token,
            Err(err) => return all(GrantOutcome::Unavailable(format!("no runner token: {err}"))),
        };
        let identity = &self.inner.identity;
        let url = format!("{}/vault/{}/grants", identity.edge_url(), identity.org_id());
        let names: Vec<&str> = providers.iter().map(|p| p.as_str()).collect();
        for attempt in 0..2 {
            let body = identity.grants_request(&names);
            let res = match self
                .inner
                .http
                .post(&url)
                .bearer_auth(&bearer)
                .json(&body)
                .send()
                .await
            {
                Ok(res) => res,
                Err(err) => return all(GrantOutcome::Unavailable(describe_http_error(err))),
            };
            let status = res.status().as_u16();
            if res.status().is_success() {
                return match res.json::<GrantsReply>().await {
                    Ok(reply) => Some(reply.outcomes(providers)),
                    Err(err) => all(GrantOutcome::Unavailable(format!(
                        "malformed grants: {}",
                        describe_http_error(err)
                    ))),
                };
            }
            if status == 404 {
                tracing::info!("credentials: the edge has no batch grants; asking one at a time");
                return None;
            }
            let body = EdgeErrorBody::read(res).await;
            if body.error == "stale" && attempt == 0 {
                continue;
            }
            return all(classify_grant_failure(status, &body));
        }
        all(GrantOutcome::Unavailable(
            "vault grant timestamp refused".into(),
        ))
    }

    /// Cache what the vault answered for `provider` (a grant, or that it has
    /// none), falling back to a still-valid cached grant when it couldn't be
    /// reached.
    fn record(&self, provider: VaultProvider, fetched: GrantOutcome) -> GrantOutcome {
        match fetched {
            GrantOutcome::Granted(grant) => {
                tracing::info!(
                    provider = provider.as_str(),
                    generation = grant.generation,
                    expires_in_s = grant.remaining_ms() / 1000,
                    "credentials: vault grant"
                );
                lock(&self.inner.cache).insert(
                    provider,
                    Cached::Granted {
                        grant: grant.clone(),
                        at: Instant::now(),
                    },
                );
                GrantOutcome::Granted(grant)
            }
            GrantOutcome::NotConnected(reason) => {
                tracing::info!(provider = provider.as_str(), %reason, "credentials: provider not connected");
                lock(&self.inner.cache).insert(
                    provider,
                    Cached::Missing {
                        at: Instant::now(),
                        reason: reason.clone(),
                    },
                );
                GrantOutcome::NotConnected(reason)
            }
            GrantOutcome::Unavailable(reason) => {
                // Offline fallback: a cached grant that is still valid keeps
                // working while the vault is unreachable.
                if let Some(Cached::Granted { grant, .. }) = lock(&self.inner.cache).get(&provider)
                    && grant.remaining_ms() > OFFLINE_MIN_VALIDITY.as_millis() as i64
                {
                    tracing::warn!(provider = provider.as_str(), %reason,
                        "credentials: vault unreachable; using the cached grant");
                    return GrantOutcome::Granted(grant.clone());
                }
                tracing::warn!(provider = provider.as_str(), %reason, "credentials: vault unreachable");
                GrantOutcome::Unavailable(reason)
            }
        }
    }

    fn cached(
        &self,
        provider: VaultProvider,
        min_validity: Duration,
        max_age: Option<Duration>,
    ) -> Option<GrantOutcome> {
        match lock(&self.inner.cache).get(&provider)? {
            Cached::Granted { grant, at }
                if grant.remaining_ms() > min_validity.as_millis() as i64
                    && max_age.is_none_or(|max| at.elapsed() < max) =>
            {
                Some(GrantOutcome::Granted(grant.clone()))
            }
            Cached::Missing { at, reason } if at.elapsed() < MISSING_TTL => {
                Some(GrantOutcome::NotConnected(reason.clone()))
            }
            _ => None,
        }
    }

    fn cached_grant(&self, provider: VaultProvider) -> Option<Grant> {
        match lock(&self.inner.cache).get(&provider)? {
            Cached::Granted { grant, .. } => Some(grant.clone()),
            Cached::Missing { .. } => None,
        }
    }

    async fn fetch(&self, provider: VaultProvider) -> GrantOutcome {
        // A GitHub grant is for the session repository's owner: without one
        // there is nothing to ask for.
        let repo = match provider {
            VaultProvider::Github => match self.inner.config.github_repo.as_deref() {
                Some(repo) => Some(repo),
                None => {
                    return GrantOutcome::NotConnected(
                        "this Cloud device has no GitHub repository".into(),
                    );
                }
            },
            _ => None,
        };
        let bearer = match self.inner.tokens.token().await {
            Ok(token) => token,
            Err(err) => return GrantOutcome::Unavailable(format!("no runner token: {err}")),
        };
        let identity = &self.inner.identity;
        let url = format!("{}/vault/{}/grant", identity.edge_url(), identity.org_id());
        for attempt in 0..2 {
            let body = identity.grant_request(provider.as_str(), repo);
            let res = match self
                .inner
                .http
                .post(&url)
                .bearer_auth(&bearer)
                .json(&body)
                .send()
                .await
            {
                Ok(res) => res,
                Err(err) => return GrantOutcome::Unavailable(describe_http_error(err)),
            };
            let status = res.status().as_u16();
            if res.status().is_success() {
                return match res.json::<Grant>().await {
                    Ok(grant) if !grant.access_token.is_empty() => GrantOutcome::Granted(grant),
                    Ok(_) => GrantOutcome::Unavailable("the vault returned an empty grant".into()),
                    Err(err) => GrantOutcome::Unavailable(format!(
                        "malformed grant: {}",
                        describe_http_error(err)
                    )),
                };
            }
            let body = EdgeErrorBody::read(res).await;
            // Replay fence (per device, across providers): our clock lagged
            // the last accepted ts (e.g. a restart). One retry, fresh ts.
            if body.error == "stale" && attempt == 0 {
                continue;
            }
            return classify_grant_failure(status, &body);
        }
        GrantOutcome::Unavailable("vault grant timestamp refused".into())
    }

    // -- run environment ----------------------------------------------------

    /// Fill `request.env` for a spawn of `harness` on this Cloud device.
    /// `Err` when the provider has no login here at all: the run fails at
    /// once, saying where to sign in, instead of the CLI retrying without
    /// credentials.
    pub async fn prepare(
        &self,
        harness: HarnessId,
        request: &mut RunRequest,
    ) -> Result<(), String> {
        // GitHub: the credential store and hosts.yml (refreshed in place),
        // never the run's environment (fixed at spawn, outlived by the token).
        // The user's identity does go in the environment: it outranks any
        // git config, so an agent that sets one of its own (the App's bot,
        // which its token belongs to) still commits as the user. Asked again
        // before a run, so a login disconnected since the last one stops it.
        match self.grant_for_run(VaultProvider::Github).await {
            GrantOutcome::Granted(grant) => {
                self.sync_github_files(&grant).await;
                if let Some(identity) = self.git_identity(&grant).await {
                    for (key, value) in identity.env() {
                        request.env.insert(key.into(), value);
                    }
                }
            }
            GrantOutcome::NotConnected(reason) => {
                self.clear_github_files().await;
                if let Some(message) = github_session_refusal(&reason) {
                    return Err(message);
                }
            }
            GrantOutcome::Unavailable(_) => {}
        }
        if harness == HarnessId::Codex {
            self.ensure_codex_starts().await;
        }
        match harness {
            HarnessId::Codex => match self.codex_home().await {
                Some(home) => {
                    request
                        .env
                        .insert("CODEX_HOME".into(), home.to_string_lossy().into_owned());
                }
                None if self.codex_native_login() => {}
                None => return Err(CODEX_NOT_SIGNED_IN.into()),
            },
            HarnessId::ClaudeCode => match self.prepare_claude().await {
                Some(key) => {
                    request.env.insert("ANTHROPIC_API_KEY".into(), key);
                }
                None if self.claude_has_login() => {}
                None => return Err(CLAUDE_NOT_SIGNED_IN.into()),
            },
            other if !cloud_credentials(other).is_empty() => {
                self.ensure_installed(other).await?;
                self.prepare_agent(other, request).await?;
            }
            _ => {}
        }
        Ok(())
    }

    /// Hand `harness` the credentials it runs on (see the module docs): per
    /// account group the first one granted, a key in the environment, a
    /// sign-in in the agent's own file. `Err` when it has none at all and no
    /// login of the machine's own.
    async fn prepare_agent(
        &self,
        harness: HarnessId,
        request: &mut RunRequest,
    ) -> Result<(), String> {
        let plan = self.plan(harness).await;
        let mut handoffs: Vec<Value> = Vec::new();
        for (credential, grant) in &plan.use_grants {
            match credential.delivery {
                CloudDelivery::Env(names) => {
                    for name in names {
                        request
                            .env
                            .insert((*name).into(), grant.access_token.clone());
                    }
                }
                CloudDelivery::File => {
                    if credential.vault == VaultProvider::GeminiKey {
                        request
                            .env
                            .insert("GEMINI_API_KEY".into(), grant.access_token.clone());
                    }
                    if harness == HarnessId::Antigravity
                        && credential.vault == VaultProvider::Antigravity
                    {
                        let caller_upstream = request
                            .env
                            .get(antigravity_proxy::CLOUD_CODE_URL_ENV)
                            .or_else(|| request.env.get(antigravity_proxy::CCPA_BASE_URL_ENV))
                            .cloned()
                            .filter(|u| !u.trim().is_empty());
                        if let Some(proxy_url) =
                            self.ensure_antigravity_proxy(caller_upstream).await
                        {
                            request
                                .env
                                .insert(antigravity_proxy::CCPA_BASE_URL_ENV.into(), proxy_url.clone());
                            request
                                .env
                                .insert(antigravity_proxy::CLOUD_CODE_URL_ENV.into(), proxy_url);
                        }
                        request.env.entry("SSH_CONNECTION".into()).or_insert_with(|| {
                            zeron_harness::acp::ANTIGRAVITY_SSH_FILE_STORAGE_VALUE.into()
                        });
                    }
                    self.write_agent_file(harness, credential.vault, grant)
                        .await;
                }
                CloudDelivery::Api => {
                    if let Some(value) = opencode_value(credential.vault, grant) {
                        handoffs.push(json!({
                            "integrationID": credential.group,
                            "value": value,
                        }));
                    }
                }
            }
        }
        // Every OpenCode run says what Cloud hands it (none = remove ours):
        // its harness installs them through OpenCode's credential API.
        if cloud_credentials(harness)
            .iter()
            .any(|c| c.delivery == CloudDelivery::Api)
        {
            request.env.insert(
                zeron_harness::opencode::CREDENTIALS_ENV.into(),
                Value::Array(handoffs).to_string(),
            );
        }
        match harness {
            // Hermes merges a shared Nous store over its own: an empty one
            // of the machine's, so a stale copy never wins over the vault's.
            HarnessId::Hermes => {
                let shared = hermes_shared_dir(&self.inner.config.data_dir);
                if private_dir(&shared).is_ok() {
                    request.env.insert(
                        "HERMES_SHARED_AUTH_DIR".into(),
                        shared.to_string_lossy().into_owned(),
                    );
                }
            }
            // Muse's login is the file the broker writes, never a keyring.
            HarnessId::Muse => {
                request
                    .env
                    .insert("TBH_CREDENTIAL_BACKEND".into(), "file".into());
            }
            _ => {}
        }
        for vault in &plan.cleared_files {
            self.clear_agent_file(harness, *vault).await;
        }
        if plan.use_grants.is_empty() && !plan.unavailable && !self.agent_native_login(harness) {
            return Err(not_signed_in(harness));
        }
        Ok(())
    }

    /// What a run of `harness` uses now: per group, the first credential
    /// the vault grants (a group a login of the machine's own fills counts as
    /// filled); which managed files lost their grant; and whether the vault
    /// couldn't be reached for any of them.
    async fn plan(&self, harness: HarnessId) -> AgentPlan {
        let credentials = cloud_credentials(harness);
        let providers: Vec<VaultProvider> = credentials.iter().map(|c| c.vault).collect();
        let outcomes = self.grants_for_run(&providers).await;
        let mut plan = AgentPlan::default();
        let mut filled: HashSet<&str> = HashSet::new();
        for credential in credentials {
            match outcomes.get(&credential.vault) {
                Some(GrantOutcome::Granted(grant)) => {
                    if filled.contains(credential.group) {
                        continue;
                    }
                    if credential.delivery == CloudDelivery::File
                        && self.native_file_login(harness, credential.vault, grant)
                    {
                        // The machine's own login wins; it's not ours to replace.
                        filled.insert(credential.group);
                        plan.native = true;
                        continue;
                    }
                    filled.insert(credential.group);
                    plan.use_grants.push((*credential, grant.clone()));
                }
                Some(GrantOutcome::NotConnected(_)) => {
                    // (An OpenCode login the vault stopped granting is
                    // removed by its harness: it's no longer handed over.)
                    if credential.delivery == CloudDelivery::File {
                        plan.cleared_files.push(credential.vault);
                    }
                }
                Some(GrantOutcome::Unavailable(_)) | None => plan.unavailable = true,
            }
        }
        plan
    }

    /// Which credential a run of `harness` would use now, in [`Self::prepare`]'s
    /// order: the vault login, the sandbox's own, the vault API key. Keyed by
    /// the grant's generation, so another account (or a refreshed token) reads
    /// as different. `None` while the vault can't be reached: unknown is not
    /// a change.
    pub async fn credential(&self, harness: HarnessId) -> Option<String> {
        let (login, key, native) = match harness {
            HarnessId::Codex => (
                VaultProvider::Codex,
                VaultProvider::OpenaiKey,
                self.codex_native_login(),
            ),
            HarnessId::ClaudeCode => (
                VaultProvider::Claude,
                VaultProvider::AnthropicKey,
                self.claude_native_login(),
            ),
            other if !cloud_credentials(other).is_empty() => {
                let plan = self.plan(other).await;
                let mut used: Vec<String> = plan
                    .use_grants
                    .iter()
                    .map(|(c, g)| {
                        let mut id = credential_part(c.vault.as_str(), g);
                        // Antigravity reads its token once per connection: a
                        // session must not outlive it, so one whose token is
                        // nearing its end reads as another credential and is
                        // replaced at its next turn.
                        if c.vault == VaultProvider::Antigravity
                            && g.remaining_ms() < ANTIGRAVITY_MIN_LIFE.as_millis() as i64
                        {
                            id.push_str(":expiring");
                        }
                        id
                    })
                    .collect();
                if !used.is_empty() {
                    used.sort();
                    return Some(used.join(","));
                }
                if plan.unavailable {
                    return None;
                }
                return Some(
                    if plan.native || self.agent_native_login(other) {
                        "native"
                    } else {
                        "none"
                    }
                    .into(),
                );
            }
            _ => return None,
        };
        let generation = |grant: Grant| credential_part(&grant.provider, &grant);
        match self.grant_for_run(login).await {
            GrantOutcome::Granted(grant) => return Some(generation(grant)),
            GrantOutcome::Unavailable(_) => return None,
            GrantOutcome::NotConnected(_) => {}
        }
        if native {
            return Some("native".into());
        }
        match self.grant_for_run(key).await {
            GrantOutcome::Granted(grant) => Some(generation(grant)),
            GrantOutcome::NotConnected(_) => Some("none".into()),
            GrantOutcome::Unavailable(_) => None,
        }
    }

    /// Before the first Codex run: a machine image whose global npm Codex
    /// lacks its native binary (an optional dependency) fails every spawn,
    /// so reinstall it once. Later runs wait on a repair in progress.
    async fn ensure_codex_starts(&self) {
        let mut check = self.inner.codex_check.lock().await;
        match *check {
            CodexCheck::Passed => return,
            CodexCheck::FailedAt(at) if at.elapsed() < CODEX_REPAIR_RETRY => return,
            _ => {}
        }
        *check = match zeron_harness::codex::npm_repair::repair().await {
            Ok(zeron_harness::codex::npm_repair::Repair::NotNeeded) => CodexCheck::Passed,
            Ok(zeron_harness::codex::npm_repair::Repair::Repaired { version }) => {
                tracing::info!(%version, "reinstalled Codex with its native binary");
                CodexCheck::Passed
            }
            Err(error) => {
                tracing::warn!(%error, "Codex is missing its native binary and the reinstall failed");
                CodexCheck::FailedAt(Instant::now())
            }
        };
    }

    /// A Codex login the sandbox has on its own (`codex login` inside it, or
    /// an API key in the engine's environment).
    fn codex_native_login(&self) -> bool {
        self.inner
            .config
            .home
            .join(".codex")
            .join("auth.json")
            .exists()
            || self.inner.config.codex_env_login
    }

    /// Whether Claude Code has anything to sign in with: the vault's login in
    /// its credential file, or its own.
    fn claude_has_login(&self) -> bool {
        self.claude_native_login()
            || read_json(&self.claude_credentials_file()).is_some_and(|value| {
                value["claudeAiOauth"]["accessToken"]
                    .as_str()
                    .is_some_and(|token| !token.is_empty())
            })
    }

    /// The GitHub token (files synced), or `None` when not connected/unreachable.
    pub async fn github_token(&self) -> Option<String> {
        self.github_grant().await.map(|grant| grant.access_token)
    }

    /// The GitHub grant, its files synced (or cleared when not connected).
    async fn github_grant(&self) -> Option<Grant> {
        match self.grant(VaultProvider::Github).await {
            GrantOutcome::Granted(grant) => {
                self.sync_github_files(&grant).await;
                Some(grant)
            }
            GrantOutcome::NotConnected(_) => {
                self.clear_github_files().await;
                None
            }
            GrantOutcome::Unavailable(_) => None,
        }
    }

    /// Make sure git can authenticate to github.com before a clone (no-op
    /// without a grant: public repositories still clone). `Err` carries why
    /// there is no grant (e.g. the App isn't installed on the owner), for a
    /// clone that then fails.
    pub async fn ensure_git_credentials(&self) -> Result<(), String> {
        match self.grant(VaultProvider::Github).await {
            GrantOutcome::Granted(grant) => {
                self.sync_github_files(&grant).await;
                Ok(())
            }
            GrantOutcome::NotConnected(reason) => {
                self.clear_github_files().await;
                Err(github_session_refusal(&reason).unwrap_or(reason))
            }
            GrantOutcome::Unavailable(reason) => Err(reason),
        }
    }

    /// The managed `CODEX_HOME` when a Codex credential is available.
    pub async fn codex_home(&self) -> Option<PathBuf> {
        let home = managed_codex_home(&self.inner.config.data_dir);
        let auth_file = home.join("auth.json");
        match self.grant_for_run(VaultProvider::Codex).await {
            GrantOutcome::Granted(grant) => {
                return self
                    .write_codex_auth(&home, codex_chatgpt_auth_json(&grant, chrono::Utc::now()))
                    .await
                    .then_some(home);
            }
            // Offline with nothing cached: the last written file may still
            // hold a valid token; codex reports its own error otherwise.
            GrantOutcome::Unavailable(_) => {
                return auth_file.exists().then_some(home);
            }
            GrantOutcome::NotConnected(_) => {}
        }
        match self.grant_for_run(VaultProvider::OpenaiKey).await {
            GrantOutcome::Granted(grant) => self
                .write_codex_auth(&home, codex_api_key_auth_json(&grant.access_token))
                .await
                .then_some(home),
            GrantOutcome::Unavailable(_) => auth_file.exists().then_some(home),
            GrantOutcome::NotConnected(_) => {
                // Revoked/disconnected: never leave a stale credential behind.
                let _files = self.inner.files.lock().await;
                if auth_file.exists() {
                    let _ = std::fs::remove_file(&auth_file);
                    tracing::info!("credentials: removed the managed Codex login (no grant)");
                }
                None
            }
        }
    }

    /// Resolve Claude Code's credential for a spawn (see the module docs).
    /// Returns the `ANTHROPIC_API_KEY` to inject, if that is the resolution;
    /// a subscription grant goes to the CLI's own credential file instead.
    pub async fn prepare_claude(&self) -> Option<String> {
        match self.grant_for_run(VaultProvider::Claude).await {
            GrantOutcome::Granted(grant) => {
                if self.write_claude_credentials(&grant).await {
                    return None;
                }
            }
            GrantOutcome::NotConnected(_) => self.clear_claude_credentials().await,
            // Keep whatever is on disk (the last managed token may still be
            // valid); fall through to the other sources.
            GrantOutcome::Unavailable(_) => {}
        }
        if self.claude_native_login() {
            return None;
        }
        match self.grant_for_run(VaultProvider::AnthropicKey).await {
            GrantOutcome::Granted(grant) => Some(grant.access_token),
            _ => None,
        }
    }

    fn claude_credentials_file(&self) -> PathBuf {
        claude_credentials_file(&self.inner.config)
    }

    fn claude_native_backup(&self) -> PathBuf {
        claude_native_backup(&self.inner.config)
    }

    /// The unmodified CLI's own login (`claude auth login` inside the sandbox
    /// writes `.credentials.json` with a real refresh token), or an inherited
    /// env login. A vault-managed login (empty refresh token) is not native.
    fn claude_native_login(&self) -> bool {
        self.inner.config.claude_env_login
            || read_json(&self.claude_credentials_file())
                .is_some_and(|value| is_native_claude_oauth(&value["claudeAiOauth"]))
    }

    /// Write the grant as `claudeAiOauth` (empty refresh token), preserving
    /// sibling keys; back up a native login it displaces. Returns whether the
    /// file now holds the grant.
    async fn write_claude_credentials(&self, grant: &Grant) -> bool {
        let _files = self.inner.files.lock().await;
        let path = self.claude_credentials_file();
        let mut value = match read_json_guarded(&path) {
            Ok(value) => value.unwrap_or_else(|| json!({})),
            Err(()) => {
                tracing::warn!(path = %path.display(),
                    "credentials: Claude credentials file is unreadable; not touching it");
                return false;
            }
        };
        let Some(map) = value.as_object_mut() else {
            return false;
        };
        let desired = claude_oauth_json(grant);
        if map.get("claudeAiOauth") == Some(&desired) {
            return true;
        }
        if let Some(native) = map.get("claudeAiOauth")
            && is_native_claude_oauth(native)
        {
            let backup = self.claude_native_backup();
            if !backup.exists() {
                match serde_json::to_vec(native)
                    .map_err(|err| crate::EngineError::Other(err.to_string()))
                    .and_then(|bytes| write_private_atomic(&backup, &bytes))
                {
                    Ok(()) => tracing::warn!(
                        "credentials: a vault Claude grant replaces this sandbox's own Claude \
                         login; the native login was backed up and returns if the grant goes away"
                    ),
                    Err(err) => {
                        tracing::warn!(error = %err,
                            "credentials: cannot back up the native Claude login; leaving it in place");
                        return false;
                    }
                }
            }
        }
        map.insert("claudeAiOauth".into(), desired);
        let written = private_dir(&self.inner.config.claude_config_dir)
            .map_err(crate::EngineError::from)
            .and_then(|()| {
                serde_json::to_vec_pretty(&value)
                    .map_err(|err| crate::EngineError::Other(err.to_string()))
            })
            .and_then(|bytes| write_private_atomic(&path, &bytes));
        match written {
            Ok(()) => {
                tracing::info!(
                    expires_in_s = grant.remaining_ms() / 1000,
                    "credentials: wrote the vault Claude login"
                );
                ensure_claude_onboarded(&self.inner.config.claude_config_file);
                true
            }
            Err(err) => {
                tracing::warn!(error = %err, "credentials: failed to write the Claude login");
                false
            }
        }
    }

    /// The `claude` grant went away: drop a vault-managed `claudeAiOauth`
    /// (restoring a backed-up native login), never a native one.
    async fn clear_claude_credentials(&self) {
        let _files = self.inner.files.lock().await;
        clear_managed_claude(&self.inner.config);
    }

    /// Write `auth.json` (0600, atomic) into the managed home when its
    /// credential differs from `desired`; seeds `config.toml` once. Returns
    /// whether the home is usable.
    async fn write_codex_auth(&self, home: &Path, desired: Value) -> bool {
        let _files = self.inner.files.lock().await;
        if let Err(err) = private_dir(home) {
            tracing::warn!(error = %err, "credentials: cannot create the managed CODEX_HOME");
            return false;
        }
        seed_codex_config(
            home,
            &self.inner.config.home.join(".codex").join("config.toml"),
        );
        let path = home.join("auth.json");
        let current = std::fs::read(&path)
            .ok()
            .and_then(|raw| serde_json::from_slice::<Value>(&raw).ok());
        if current
            .as_ref()
            .is_some_and(|current| same_codex_credential(current, &desired))
        {
            return true;
        }
        let bytes = match serde_json::to_vec_pretty(&desired) {
            Ok(bytes) => bytes,
            Err(_) => return false,
        };
        match write_private_atomic(&path, &bytes) {
            Ok(()) => {
                tracing::info!("credentials: wrote the managed Codex login");
                true
            }
            Err(err) => {
                tracing::warn!(error = %err, "credentials: failed to write the managed Codex login");
                false
            }
        }
    }

    // -- GitHub files -------------------------------------------------------

    async fn sync_github_files(&self, grant: &Grant) {
        let mut files = self.inner.files.lock().await;
        let config = &self.inner.config;
        let store = git_credentials_path(&config.data_dir);
        let line = format!("https://x-access-token:{}@github.com\n", grant.access_token);
        // Rewritten when it differs (a new token, or git's own `erase` after
        // a rejected push emptied the store).
        if std::fs::read_to_string(&store).ok().as_deref() != Some(line.as_str())
            && let Err(err) = store
                .parent()
                .map_or(Ok(()), private_dir)
                .map_err(crate::EngineError::from)
                .and_then(|()| write_private_atomic(&store, line.as_bytes()))
        {
            tracing::warn!(error = %err, "credentials: failed to write git credentials");
        }
        if !files.git_helper_configured {
            files.git_helper_configured = configure_git_helper(&config.home, &store).await;
        }
        write_gh_hosts(&config.home, grant);
    }

    /// The user's git identity from the grant's GitHub login: their name and
    /// GitHub noreply address (`{id}+{login}@users.noreply.github.com`,
    /// which ties a commit to their account whatever their email settings).
    /// Looked up once per login; `None` until GitHub answers. Also stamped as
    /// the machine's global git identity for the user's own terminals, unless
    /// they set their own.
    async fn git_identity(&self, grant: &Grant) -> Option<GitIdentity> {
        let login = grant
            .account
            .as_deref()
            .map(str::trim)
            .filter(|login| is_github_login(login))?;
        let mut cached = self.inner.git_identity.lock().await;
        if let Some((known, identity)) = cached.as_ref()
            && known == login
        {
            return Some(identity.clone());
        }
        let api = self.inner.config.github_api.trim_end_matches('/');
        let user = match github_get(
            &self.inner.http,
            &format!("{api}/users/{login}"),
            &grant.access_token,
        )
        .await
        {
            Ok(user) => user,
            Err(err) => {
                let error = match err {
                    GithubError::Unauthorized => "unauthorized".to_string(),
                    GithubError::Other(error) => error,
                };
                tracing::warn!(%error, login, "credentials: GitHub user lookup failed");
                return None;
            }
        };
        let identity = GitIdentity::from_user(login, &user)?;
        configure_git_identity(&self.inner.config.home, &identity).await;
        *cached = Some((login.to_string(), identity.clone()));
        Some(identity)
    }

    async fn clear_github_files(&self) {
        let _files = self.inner.files.lock().await;
        let store = git_credentials_path(&self.inner.config.data_dir);
        if store.exists() {
            let _ = std::fs::remove_file(&store);
            tracing::info!("credentials: removed git credentials (GitHub not connected)");
        }
        let hosts = gh_hosts_path(&self.inner.config.home);
        if is_managed_hosts(&hosts) {
            let _ = std::fs::remove_file(&hosts);
        }
    }

    /// One background pass (the refresher runs it every minute): GitHub
    /// (probed every few minutes when absent, re-granted ~10 min before
    /// expiry) plus any cached Claude / Codex grant nearing expiry.
    pub async fn refresh_due(&self) {
        let github_due = match self.cached_grant(VaultProvider::Github) {
            Some(grant) => grant.remaining_ms() <= REFRESH_AHEAD.as_millis() as i64,
            None => {
                let mut files = self.inner.files.lock().await;
                let due = files
                    .last_github_probe
                    .is_none_or(|at| at.elapsed() >= GITHUB_PROBE_EVERY);
                if due {
                    files.last_github_probe = Some(Instant::now());
                }
                due
            }
        };
        if github_due {
            match self
                .grant_within(VaultProvider::Github, REFRESH_AHEAD, None)
                .await
            {
                GrantOutcome::Granted(grant) => self.sync_github_files(&grant).await,
                GrantOutcome::NotConnected(_) => self.clear_github_files().await,
                GrantOutcome::Unavailable(_) => {}
            }
        }
        if let Some(grant) = self.cached_grant(VaultProvider::Claude)
            && grant.remaining_ms() <= REFRESH_AHEAD.as_millis() as i64
        {
            // Claude Code access tokens live hours, not days: keep the file
            // ahead of expiry for long-lived sessions and terminals.
            match self
                .grant_within(VaultProvider::Claude, REFRESH_AHEAD, None)
                .await
            {
                GrantOutcome::Granted(grant) => {
                    self.write_claude_credentials(&grant).await;
                    // Pi's Claude runs on the same login: its entry too,
                    // when the broker wrote it.
                    if pi_entry_managed(&self.inner.config.agents.pi_agent_dir, PI_CLAUDE_ENTRY) {
                        self.write_agent_file(HarnessId::Pi, VaultProvider::Claude, &grant)
                            .await;
                    }
                }
                GrantOutcome::NotConnected(_) => {
                    self.clear_claude_credentials().await;
                    self.clear_agent_file(HarnessId::Pi, VaultProvider::Claude)
                        .await;
                }
                GrantOutcome::Unavailable(_) => {}
            }
        }
        if let Some(grant) = self.cached_grant(VaultProvider::Codex)
            && grant.remaining_ms() <= REFRESH_AHEAD.as_millis() as i64
        {
            let home = managed_codex_home(&self.inner.config.data_dir);
            match self
                .grant_within(VaultProvider::Codex, REFRESH_AHEAD, None)
                .await
            {
                GrantOutcome::Granted(grant) => {
                    self.write_codex_auth(
                        &home,
                        codex_chatgpt_auth_json(&grant, chrono::Utc::now()),
                    )
                    .await;
                    // Pi's ChatGPT runs on the same login: keep its entry
                    // fresh too, when the broker wrote it.
                    if pi_codex_managed(&self.inner.config.agents.pi_agent_dir) {
                        self.write_agent_file(HarnessId::Pi, VaultProvider::Codex, &grant)
                            .await;
                    }
                    // And Hermes' ChatGPT, the same way.
                    if hermes_record_managed(&self.inner.config.agents.hermes_home, VaultProvider::Codex) {
                        self.write_agent_file(HarnessId::Hermes, VaultProvider::Codex, &grant)
                            .await;
                    }
                }
                GrantOutcome::NotConnected(_) => {
                    {
                        let _files = self.inner.files.lock().await;
                        let _ = std::fs::remove_file(home.join("auth.json"));
                    }
                    self.clear_agent_file(HarnessId::Pi, VaultProvider::Codex)
                        .await;
                    self.clear_agent_file(HarnessId::Hermes, VaultProvider::Codex)
                        .await;
                }
                GrantOutcome::Unavailable(_) => {}
            }
        }
        if let Some(grant) = self.cached_grant(VaultProvider::Grok)
            && grant.remaining_ms() <= REFRESH_AHEAD.as_millis() as i64
        {
            match self
                .grant_within(VaultProvider::Grok, REFRESH_AHEAD, None)
                .await
            {
                GrantOutcome::Granted(grant) => {
                    if !self.native_file_login(HarnessId::Grok, VaultProvider::Grok, &grant) {
                        self.write_agent_file(HarnessId::Grok, VaultProvider::Grok, &grant)
                            .await;
                    }
                }
                GrantOutcome::NotConnected(_) => {
                    self.clear_agent_file(HarnessId::Grok, VaultProvider::Grok)
                        .await;
                }
                GrantOutcome::Unavailable(_) => {}
            }
        }
        // Logins the agent itself would refresh ahead of expiry, and can't
        // (no refresh token): replaced before it tries.
        for (harness, provider) in [
            (HarnessId::Hermes, VaultProvider::XaiOauth),
            (HarnessId::Hermes, VaultProvider::Nous),
            (HarnessId::Antigravity, VaultProvider::Antigravity),
        ] {
            let ahead = refresh_ahead(provider);
            let Some(grant) = self.cached_grant(provider) else {
                continue;
            };
            if grant.remaining_ms() > ahead.as_millis() as i64 {
                continue;
            }
            match self.grant_within(provider, ahead, None).await {
                GrantOutcome::Granted(grant) => {
                    if !self.native_file_login(harness, provider, &grant) {
                        self.write_agent_file(harness, provider, &grant).await;
                    }
                }
                GrantOutcome::NotConnected(_) => self.clear_agent_file(harness, provider).await,
                GrantOutcome::Unavailable(_) => {}
            }
        }
    }

    // -- GitHub repositories ------------------------------------------------

    /// `ListGithubRepos`: repositories the device's GitHub grant (an App
    /// installation token) can see — the installation's repositories —
    /// filtered by `query` (case-insensitive substring of `owner/name`),
    /// newest push first.
    pub async fn list_github_repos(&self, query: Option<&str>) -> Result<Vec<GithubRepo>, String> {
        let token = match self.grant(VaultProvider::Github).await {
            GrantOutcome::Granted(grant) => {
                self.sync_github_files(&grant).await;
                grant.access_token
            }
            GrantOutcome::NotConnected(_) => return Err(GITHUB_NOT_CONNECTED.into()),
            GrantOutcome::Unavailable(reason) => {
                return Err(format!("Couldn't reach the credential vault: {reason}"));
            }
        };
        let repos =
            match fetch_github_repos(&self.inner.http, &self.inner.config.github_api, &token).await
            {
                Ok(repos) => repos,
                Err(GithubError::Unauthorized) => {
                    // The grant is dead on GitHub's side: drop it so the next
                    // call re-grants instead of reusing it until expiry.
                    lock(&self.inner.cache).remove(&VaultProvider::Github);
                    return Err(
                    "GitHub rejected this device's token — reconnect GitHub in Settings → Cloud"
                        .into(),
                );
                }
                Err(GithubError::Other(message)) => return Err(message),
            };
        Ok(filter_and_sort_repos(repos, query))
    }
}

/// What a run of an agent beyond Claude Code / Codex uses ([`CredentialBroker::plan`]).
#[derive(Default)]
struct AgentPlan {
    /// The credentials handed over, one per account group.
    use_grants: Vec<(zeron_proto::providers::CloudCredential, Grant)>,
    /// File credentials the vault no longer grants: their managed files go.
    cleared_files: Vec<VaultProvider>,
    /// A group was filled by a login of the machine's own.
    native: bool,
    /// The vault couldn't be reached for at least one credential.
    unavailable: bool,
}

impl CredentialBroker {
    /// Install `harness`'s CLI before its first run on this machine, once
    /// (concurrent runs wait on the same install). Claude Code and Codex come
    /// with the machine; Cursor runs on an SDK the harness installs itself.
    async fn ensure_installed(&self, harness: HarnessId) -> Result<(), String> {
        if !self.inner.config.install_missing
            || matches!(
                harness,
                HarnessId::ClaudeCode | HarnessId::Codex | HarnessId::Cursor | HarnessId::Mock
            )
        {
            return Ok(());
        }
        let slot = lock(&self.inner.installs)
            .entry(harness)
            .or_default()
            .clone();
        let mut check = slot.lock().await;
        match &*check {
            InstallCheck::Passed => return Ok(()),
            InstallCheck::FailedAt(at, why) if at.elapsed() < INSTALL_RETRY => {
                return Err(why.clone());
            }
            _ => {}
        }
        let name = zeron_proto::providers::provider_name(harness);
        if zeron_harness::install::installed(harness) {
            *check = InstallCheck::Passed;
            return Ok(());
        }
        if !zeron_harness::install::can_install(harness) {
            let why = format!(
                "{name} isn't installed on this Cloud machine, and it has no installer here."
            );
            *check = InstallCheck::FailedAt(Instant::now(), why.clone());
            return Err(why);
        }
        tracing::info!(
            harness = harness.as_str(),
            "credentials: installing a missing agent CLI"
        );
        let started = Instant::now();
        match zeron_harness::install::install_harness(
            harness,
            zeron_harness::CancellationToken::new(),
        )
        .await
        {
            Ok(()) => {
                tracing::info!(
                    harness = harness.as_str(),
                    elapsed_s = started.elapsed().as_secs(),
                    "credentials: installed the agent CLI"
                );
                *check = InstallCheck::Passed;
                Ok(())
            }
            Err(error) => {
                tracing::warn!(harness = harness.as_str(), %error, "credentials: installing the agent CLI failed");
                let why = format!("Couldn't install {name} on this Cloud machine: {error}");
                *check = InstallCheck::FailedAt(Instant::now(), why.clone());
                Err(why)
            }
        }
    }

    /// Write the file a sign-in of `vault` goes to for `harness`.
    async fn write_agent_file(&self, harness: HarnessId, vault: VaultProvider, grant: &Grant) {
        let _files = self.inner.files.lock().await;
        let agents = self.inner.config.agents.clone();
        let record = devin_record(&self.inner.config.data_dir);
        let grant = grant.clone();
        let muse_record = muse_record(&self.inner.config.data_dir);
        let antigravity_proxied = self.has_antigravity_proxy();
        let written = tokio::task::spawn_blocking(move || match (harness, vault) {
            (HarnessId::Grok, VaultProvider::Grok) => write_grok_entry(&agents.grok_home, &grant),
            (HarnessId::Pi, VaultProvider::Codex) => write_pi_codex(&agents.pi_agent_dir, &grant),
            (HarnessId::Pi, VaultProvider::Claude) => write_pi_entry(
                &agents.pi_agent_dir,
                PI_CLAUDE_ENTRY,
                pi_claude_entry(&grant),
            ),
            (HarnessId::Hermes, VaultProvider::Codex | VaultProvider::Nous | VaultProvider::XaiOauth) => {
                write_hermes(&agents.hermes_home, vault, &grant, chrono::Utc::now())
            }
            (HarnessId::Antigravity, VaultProvider::Antigravity) => match &agents.antigravity_home {
                Some(home) => write_antigravity_login(home, &grant, antigravity_proxied),
                None => Ok(()),
            },
            (HarnessId::Muse, VaultProvider::Muse) => {
                write_muse(&agents.muse_auth_file, &muse_record, &grant)
            }
            (HarnessId::Devin, VaultProvider::Devin) => {
                write_devin(&agents.devin_dir, &record, &grant)
            }
            (HarnessId::Antigravity, VaultProvider::GeminiKey) => match &agents.antigravity_home {
                Some(home) => write_antigravity_key_method(home),
                None => Ok(()),
            },
            _ => Ok(()),
        })
        .await
        .unwrap_or_else(|join| Err(join.to_string()));
        if let Err(error) = written {
            tracing::warn!(harness = harness.as_str(), vault = vault.as_str(), %error,
                "credentials: failed to write the agent's login");
        }
    }

    /// The vault no longer grants `vault`: drop what the broker wrote for it.
    async fn clear_agent_file(&self, harness: HarnessId, vault: VaultProvider) {
        let _files = self.inner.files.lock().await;
        let agents = self.inner.config.agents.clone();
        let record = devin_record(&self.inner.config.data_dir);
        let muse_record = muse_record(&self.inner.config.data_dir);
        let _ = tokio::task::spawn_blocking(move || match (harness, vault) {
            (HarnessId::Grok, VaultProvider::Grok) => clear_grok_managed(&agents.grok_home),
            (HarnessId::Pi, VaultProvider::Codex) => clear_pi_codex(&agents.pi_agent_dir),
            (HarnessId::Pi, VaultProvider::Claude) => {
                clear_pi_entry(&agents.pi_agent_dir, PI_CLAUDE_ENTRY)
            }
            (HarnessId::Hermes, VaultProvider::Codex | VaultProvider::Nous | VaultProvider::XaiOauth) => {
                clear_hermes(&agents.hermes_home, vault)
            }
            (HarnessId::Antigravity, VaultProvider::Antigravity) => {
                if let Some(home) = &agents.antigravity_home {
                    clear_antigravity_login(home);
                }
            }
            (HarnessId::Muse, VaultProvider::Muse) => clear_muse(&agents.muse_auth_file, &muse_record),
            (HarnessId::Devin, VaultProvider::Devin) => clear_devin(&agents.devin_dir, &record),
            _ => {}
        })
        .await;
    }

    /// Whether a login of the machine's own already fills what a `vault`
    /// grant would write for `harness` (it stays, and is used).
    fn native_file_login(&self, harness: HarnessId, vault: VaultProvider, grant: &Grant) -> bool {
        let agents = &self.inner.config.agents;
        match (harness, vault) {
            (HarnessId::Grok, VaultProvider::Grok) => {
                grok_scope(grant).is_some_and(|scope| grok_native(&agents.grok_home, Some(&scope)))
            }
            (HarnessId::Pi, VaultProvider::Codex) => pi_codex_native(&agents.pi_agent_dir),
            (HarnessId::Pi, VaultProvider::Claude) => {
                pi_entry_native(&agents.pi_agent_dir, PI_CLAUDE_ENTRY)
            }
            (HarnessId::Hermes, VaultProvider::Codex | VaultProvider::Nous | VaultProvider::XaiOauth) => {
                hermes_record_native(&agents.hermes_home, vault)
            }
            (HarnessId::Antigravity, VaultProvider::Antigravity) => agents
                .antigravity_home
                .as_deref()
                .is_some_and(antigravity_token_exists),
            (HarnessId::Muse, VaultProvider::Muse) => muse_native(
                &agents.muse_auth_file,
                &muse_record(&self.inner.config.data_dir),
            ),
            (HarnessId::Devin, VaultProvider::Devin) => devin_native(
                &agents.devin_dir,
                &devin_record(&self.inner.config.data_dir),
            ),
            (HarnessId::Antigravity, VaultProvider::GeminiKey) => agents
                .antigravity_home
                .as_deref()
                .is_some_and(antigravity_token_exists),
            _ => false,
        }
    }

    /// Whether `harness` has a login of the machine's own (its CLI's own
    /// sign-in, or a key in the engine's environment). Conservative: where
    /// that can't be told, yes (the agent then reports its own error).
    fn agent_native_login(&self, harness: HarnessId) -> bool {
        let config = &self.inner.config;
        let in_env = cloud_credentials(harness).iter().any(|c| match c.delivery {
            CloudDelivery::Env(names) => names.iter().any(|n| config.env_logins.contains(*n)),
            CloudDelivery::File | CloudDelivery::Api => false,
        });
        if in_env {
            return true;
        }
        let agents = &config.agents;
        match harness {
            HarnessId::Cursor => agents.cursor_sdk_auth.is_file(),
            HarnessId::Devin => devin_native(&agents.devin_dir, &devin_record(&config.data_dir)),
            HarnessId::Grok => grok_native(&agents.grok_home, None),
            HarnessId::Pi => pi_native(&agents.pi_agent_dir),
            HarnessId::Opencode => {
                agents.opencode_dir.join("auth.json").exists()
                    || agents.opencode_dir.join("opencode.db").exists()
            }
            HarnessId::Hermes => {
                hermes_native(&agents.hermes_home) || agents.hermes_home.join(".env").exists()
            }
            HarnessId::Antigravity => agents
                .antigravity_home
                .as_deref()
                .is_some_and(antigravity_token_exists),
            HarnessId::Muse => muse_native(&agents.muse_auth_file, &muse_record(&config.data_dir)),
            _ => true,
        }
    }
}

/// The run's error for `harness` with no credential anywhere: where to
/// sign in, or which key to add.
pub fn not_signed_in(harness: HarnessId) -> String {
    use zeron_proto::providers::{cloud_key_providers, cloud_login_options, provider_name};
    let name = provider_name(harness);
    let logins: Vec<&str> = cloud_login_options(harness)
        .iter()
        .map(|o| o.label)
        .collect();
    let keys: Vec<&str> = cloud_key_providers(harness)
        .into_iter()
        .filter_map(VaultProvider::key_vendor)
        .collect();
    let article = |word: &str| {
        if word.starts_with(['A', 'E', 'I', 'O', 'U', 'x']) {
            "an"
        } else {
            "a"
        }
    };
    let key = match keys.as_slice() {
        [] => None,
        [one] => Some(format!("add {} {one} API key", article(one))),
        [a, b] => Some(format!("add {} {a} or {b} API key", article(a))),
        [a, b, c] => Some(format!("add {} {a}, {b} or {c} API key", article(a))),
        _ => Some("add an API key for one of its providers".to_string()),
    };
    // "A", "A or B", "A, B or C".
    let either = |words: &[&str]| match words {
        [] => String::new(),
        [one] => (*one).to_string(),
        [rest @ .., last] => format!("{} or {last}", rest.join(", ")),
    };
    let action = match (logins.is_empty(), key) {
        (false, Some(key)) if logins.len() == 1 => format!("Sign in with {} or {key}", logins[0]),
        (false, Some(key)) => format!("Sign in with {}, or {key},", either(&logins)),
        (false, None) => format!("Sign in with {}", either(&logins)),
        (true, Some(key)) => {
            let mut key = key;
            key.replace_range(..1, "A");
            key
        }
        (true, None) => "Connect it".to_string(),
    };
    format!(
        "{name} isn't signed in on Cloud. {action} in Settings → Providers → Cloud, then send again."
    )
}

// ---------------------------------------------------------------------------
// Other agents' credential files
// ---------------------------------------------------------------------------

const STORE_LOCK_WAIT: Duration = Duration::from_secs(5);

/// How an agent guards its JSON credential store: grok's `auth.json.lock`
/// flock, or proper-lockfile's `auth.json.lock` directory (Pi). The same
/// conventions the agents' own CLIs take (agent_accounts' stores lock them
/// alike).
enum StoreGuard {
    Flock(#[allow(dead_code)] std::fs::File),
    Dir(PathBuf),
}

impl Drop for StoreGuard {
    fn drop(&mut self) {
        if let StoreGuard::Dir(path) = self {
            let _ = std::fs::remove_dir(path);
        }
    }
}

fn lock_file_of(file: &Path) -> PathBuf {
    let mut name = file.file_name().unwrap_or_default().to_os_string();
    name.push(".lock");
    file.with_file_name(name)
}

fn flock(path: &Path) -> Result<StoreGuard, String> {
    if let Some(dir) = path.parent() {
        private_dir(dir).map_err(|e| e.to_string())?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.create(true).truncate(false).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(path).map_err(|e| e.to_string())?;
    let deadline = Instant::now() + STORE_LOCK_WAIT;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(StoreGuard::Flock(file)),
            Err(std::fs::TryLockError::WouldBlock) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(std::fs::TryLockError::WouldBlock) => {
                return Err(format!("{} is locked by the agent", path.display()));
            }
            Err(std::fs::TryLockError::Error(err)) => return Err(err.to_string()),
        }
    }
}

/// proper-lockfile's lock: a directory; one older than its staleness window
/// is a crashed holder's and is taken over.
fn dir_lock(path: &Path) -> Result<StoreGuard, String> {
    if let Some(dir) = path.parent() {
        private_dir(dir).map_err(|e| e.to_string())?;
    }
    let deadline = Instant::now() + STORE_LOCK_WAIT;
    loop {
        match std::fs::create_dir(path) {
            Ok(()) => return Ok(StoreGuard::Dir(path.to_path_buf())),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                let stale = std::fs::metadata(path)
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|at| at.elapsed().ok())
                    .is_some_and(|age| age > Duration::from_secs(30));
                if stale {
                    let _ = std::fs::remove_dir(path);
                    continue;
                }
                if Instant::now() > deadline {
                    return Err(format!("{} is locked by the agent", path.display()));
                }
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(err) => return Err(err.to_string()),
        }
    }
}

/// Change a JSON-object credential store under its lock: `change` edits
/// the map and says whether it changed anything; only then is it written
/// (0600, atomic). A store that exists but isn't a JSON object is never
/// touched — it may hold the user's other logins.
fn update_json_store(
    file: &Path,
    guard: impl FnOnce(&Path) -> Result<StoreGuard, String>,
    change: impl FnOnce(&mut serde_json::Map<String, Value>) -> bool,
) -> Result<(), String> {
    update_json_store_at(file, &lock_file_of(file), guard, change)
}

/// [`update_json_store`] under a lock file the agent names itself (Hermes'
/// `auth.lock`, beside its `auth.json`).
fn update_json_store_at(
    file: &Path,
    lock: &Path,
    guard: impl FnOnce(&Path) -> Result<StoreGuard, String>,
    change: impl FnOnce(&mut serde_json::Map<String, Value>) -> bool,
) -> Result<(), String> {
    let _guard = guard(lock)?;
    let mut map = match read_json_guarded(file) {
        Ok(Some(Value::Object(map))) => map,
        Ok(_) => serde_json::Map::new(),
        Err(()) => {
            return Err(format!(
                "{} isn't a JSON object; left alone",
                file.display()
            ));
        }
    };
    if !change(&mut map) {
        return Ok(());
    }
    if let Some(dir) = file.parent() {
        private_dir(dir).map_err(|e| e.to_string())?;
    }
    let bytes = serde_json::to_vec_pretty(&Value::Object(map)).map_err(|e| e.to_string())?;
    write_private_atomic(file, &bytes).map_err(|e| e.to_string())
}

/// A token field that is there and not empty: a login whose CLI refreshes
/// it itself (the broker writes its own with an empty one).
fn has_refresh(entry: &Value, field: &str) -> bool {
    entry[field].as_str().is_some_and(|t| !t.trim().is_empty())
}

/// An entry the broker wrote: the refresh field there and EMPTY (a login
/// with no refresh field at all is some other kind of the agent's own).
fn is_managed(entry: &Value, field: &str) -> bool {
    entry[field].as_str() == Some("")
}

fn rfc3339_ms(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .unwrap_or_default()
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

// -- Grok ---------------------------------------------------------------------

/// The `{issuer}::{client_id}` key a Grok grant's entry lives under.
fn grok_scope(grant: &Grant) -> Option<String> {
    if let Some(scope) = grant.settings.get("scope").filter(|s| s.contains("::")) {
        return Some(scope.clone());
    }
    let issuer = grant.settings.get("oidc_issuer")?;
    let client = grant.settings.get("oidc_client_id")?;
    Some(format!("{issuer}::{client}"))
}

/// The `auth.json` entry for a Grok grant: its identity fields, the access
/// token as `key`, an EMPTY `refresh_token` (the vault is the refresher).
fn grok_entry(grant: &Grant) -> Value {
    let mut entry = serde_json::Map::new();
    for (key, value) in &grant.settings {
        if !matches!(
            key.as_str(),
            "scope" | "key" | "refresh_token" | "expires_at"
        ) {
            entry.insert(key.clone(), json!(value));
        }
    }
    entry.insert("key".into(), json!(grant.access_token));
    entry.insert("refresh_token".into(), json!(""));
    entry.insert("expires_at".into(), json!(rfc3339_ms(grant.expires_at)));
    Value::Object(entry)
}

fn grok_auth_file(grok_home: &Path) -> PathBuf {
    grok_home.join("auth.json")
}

/// Whether grok has a login of its own (for `scope`, or any).
fn grok_native(grok_home: &Path, scope: Option<&str>) -> bool {
    let Some(Value::Object(map)) = read_json(&grok_auth_file(grok_home)) else {
        return false;
    };
    map.iter()
        .filter(|(key, _)| scope.is_none_or(|s| s == key.as_str()))
        .any(|(_, entry)| has_refresh(entry, "refresh_token"))
}

/// Put the grant's entry in grok's `auth.json`; other issuers' entries stay,
/// and an earlier managed entry under another scope goes (one Cloud login).
fn write_grok_entry(grok_home: &Path, grant: &Grant) -> Result<(), String> {
    let scope = grok_scope(grant).ok_or("the Grok grant names no scope")?;
    let desired = grok_entry(grant);
    update_json_store(&grok_auth_file(grok_home), flock, |map| {
        if map
            .get(&scope)
            .is_some_and(|e| !is_managed(e, "refresh_token"))
        {
            return false;
        }
        let stale: Vec<String> = map
            .iter()
            .filter(|(key, e)| **key != scope && is_managed(e, "refresh_token"))
            .map(|(key, _)| key.clone())
            .collect();
        let mut changed = !stale.is_empty();
        for key in stale {
            map.remove(&key);
        }
        if map.get(&scope) != Some(&desired) {
            map.insert(scope.clone(), desired);
            changed = true;
        }
        changed
    })
}

/// Drop every entry the broker wrote (an empty refresh token).
fn clear_grok_managed(grok_home: &Path) {
    let file = grok_auth_file(grok_home);
    if !file.exists() {
        return;
    }
    let result = update_json_store(&file, flock, |map| {
        let before = map.len();
        map.retain(|_, e| !is_managed(e, "refresh_token"));
        map.len() != before
    });
    if let Err(error) = result {
        tracing::warn!(%error, "credentials: failed to clear the managed Grok login");
    }
}

// -- Pi -----------------------------------------------------------------------

const PI_CODEX_ENTRY: &str = "openai-codex";

fn pi_auth_file(pi_dir: &Path) -> PathBuf {
    pi_dir.join("auth.json")
}

/// Pi's ChatGPT entry from a `codex` grant: `refresh` EMPTY.
fn pi_codex_entry(grant: &Grant) -> Value {
    let account_id = grant.account_id.clone().or_else(|| {
        jwt_payload(&grant.access_token)?["https://api.openai.com/auth"]["chatgpt_account_id"]
            .as_str()
            .map(str::to_owned)
    });
    let mut entry = json!({
        "type": "oauth",
        "access": grant.access_token,
        "refresh": "",
        "expires": grant.expires_at,
    });
    if let Some(account_id) = account_id {
        entry["accountId"] = json!(account_id);
    }
    entry
}

fn pi_codex_native(pi_dir: &Path) -> bool {
    pi_entry_native(pi_dir, PI_CODEX_ENTRY)
}

fn pi_codex_managed(pi_dir: &Path) -> bool {
    pi_entry_managed(pi_dir, PI_CODEX_ENTRY)
}

/// Pi's Claude entry (`anthropic`): Pi's own Claude Pro/Max login shape,
/// from the `claude` grant Claude Code runs on, `refresh` EMPTY.
const PI_CLAUDE_ENTRY: &str = "anthropic";

fn pi_claude_entry(grant: &Grant) -> Value {
    json!({
        "type": "oauth",
        "access": grant.access_token,
        "refresh": "",
        "expires": grant.expires_at,
    })
}

/// Pi's own login under `name` (a real refresh token).
fn pi_entry_native(pi_dir: &Path, name: &str) -> bool {
    read_json(&pi_auth_file(pi_dir)).is_some_and(|v| has_refresh(&v[name], "refresh"))
}

/// The broker's own entry under `name` (an empty refresh token).
fn pi_entry_managed(pi_dir: &Path, name: &str) -> bool {
    read_json(&pi_auth_file(pi_dir)).is_some_and(|v| is_managed(&v[name], "refresh"))
}

/// Any login of Pi's own: an entry the broker didn't write.
fn pi_native(pi_dir: &Path) -> bool {
    let Some(Value::Object(map)) = read_json(&pi_auth_file(pi_dir)) else {
        return false;
    };
    map.values().any(|entry| !is_managed(entry, "refresh"))
}

fn write_pi_codex(pi_dir: &Path, grant: &Grant) -> Result<(), String> {
    write_pi_entry(pi_dir, PI_CODEX_ENTRY, pi_codex_entry(grant))
}

/// Put `desired` under `name` in Pi's `auth.json`, under Pi's lock; Pi's own
/// login there, and every other entry, stays.
fn write_pi_entry(pi_dir: &Path, name: &str, desired: Value) -> Result<(), String> {
    update_json_store(&pi_auth_file(pi_dir), dir_lock, |map| {
        match map.get(name) {
            Some(entry) if !is_managed(entry, "refresh") => false,
            Some(entry) if *entry == desired => false,
            _ => {
                map.insert(name.into(), desired);
                true
            }
        }
    })
}

fn clear_pi_codex(pi_dir: &Path) {
    clear_pi_entry(pi_dir, PI_CODEX_ENTRY);
}

fn clear_pi_entry(pi_dir: &Path, name: &str) {
    let file = pi_auth_file(pi_dir);
    if !file.exists() {
        return;
    }
    let result = update_json_store(&file, dir_lock, |map| {
        let managed = map.get(name).is_some_and(|e| is_managed(e, "refresh"));
        if managed {
            map.remove(name);
        }
        managed
    });
    if let Err(error) = result {
        tracing::warn!(%error, entry = name, "credentials: failed to clear a managed Pi login");
    }
}

// -- Hermes -------------------------------------------------------------------

fn hermes_auth_file(home: &Path) -> PathBuf {
    home.join("auth.json")
}

/// `$HERMES_HOME/auth.lock`: the lock every Hermes writer takes.
fn hermes_lock(home: &Path) -> PathBuf {
    home.join("auth.lock")
}

/// `{data_dir}/cloud/hermes-shared`: Hermes' shared Nous store on Cloud,
/// kept empty (the vault's login is the one).
fn hermes_shared_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("cloud").join("hermes-shared")
}

/// Hermes' provider id for a vault login.
fn hermes_id(vault: VaultProvider) -> Option<&'static str> {
    match vault {
        VaultProvider::Codex => Some("openai-codex"),
        VaultProvider::Nous => Some("nous"),
        VaultProvider::XaiOauth => Some("xai-oauth"),
        _ => None,
    }
}

/// A provider record's refresh token: Nous keeps it at the top, ChatGPT and
/// xAI under `tokens`.
fn hermes_refresh(record: &Value) -> Option<&str> {
    record["tokens"]["refresh_token"]
        .as_str()
        .or_else(|| record["refresh_token"].as_str())
}

fn hermes_access(record: &Value) -> Option<&str> {
    record["tokens"]["access_token"]
        .as_str()
        .or_else(|| record["access_token"].as_str())
}

fn hermes_record_of(home: &Path, vault: VaultProvider) -> Option<Value> {
    let id = hermes_id(vault)?;
    let auth = read_json(&hermes_auth_file(home))?;
    let record = auth["providers"][id].clone();
    record.is_object().then_some(record)
}

/// Hermes' own login for `vault`'s provider (a real refresh token).
fn hermes_record_native(home: &Path, vault: VaultProvider) -> bool {
    hermes_record_of(home, vault)
        .is_some_and(|r| hermes_refresh(&r).is_some_and(|t| !t.trim().is_empty()))
}

/// The broker's record for `vault`'s provider (an empty refresh token).
fn hermes_record_managed(home: &Path, vault: VaultProvider) -> bool {
    hermes_record_of(home, vault).is_some_and(|r| hermes_refresh(&r) == Some(""))
}

/// Any login of Hermes' own: a provider record with a refresh token, or a
/// pool entry that isn't one the broker's records (or keys) became.
fn hermes_native(home: &Path) -> bool {
    let Some(auth) = read_json(&hermes_auth_file(home)) else {
        return false;
    };
    let records = auth["providers"]
        .as_object()
        .is_some_and(|p| p.values().any(|r| hermes_refresh(r).is_some_and(|t| !t.trim().is_empty())));
    let pool = auth["credential_pool"].as_object().is_some_and(|pool| {
        pool.values().flat_map(|rows| rows.as_array().into_iter().flatten()).any(|row| {
            row["refresh_token"].as_str().is_some_and(|t| !t.trim().is_empty())
                || (row["auth_type"] == "api_key"
                    && !row["source"].as_str().is_some_and(|s| s.starts_with("env")))
        })
    });
    records || pool
}

fn iso_utc(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .unwrap_or_default()
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// Hermes' provider record for a vault grant: the access token, an EMPTY
/// refresh token, and a fresh `last_refresh` (Hermes adopts a record written
/// under its lock only when it's newer than the one it holds).
fn hermes_record(vault: VaultProvider, grant: &Grant, now: chrono::DateTime<chrono::Utc>) -> Option<Value> {
    let now_iso = now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let expires_in = ((grant.expires_at - now.timestamp_millis()) / 1000).max(0);
    let setting = |key: &str| grant.settings.get(key).filter(|v| !v.is_empty()).cloned();
    Some(match vault {
        VaultProvider::Codex => {
            let mut tokens = json!({
                "access_token": grant.access_token,
                "refresh_token": "",
            });
            if let Some(id_token) = grant.id_token.as_ref().filter(|t| !t.is_empty()) {
                tokens["id_token"] = json!(id_token);
            }
            if let Some(account) = grant.account_id.as_ref() {
                tokens["account_id"] = json!(account);
            }
            json!({ "tokens": tokens, "last_refresh": now_iso, "auth_mode": "chatgpt" })
        }
        VaultProvider::XaiOauth => {
            let mut tokens = json!({
                "access_token": grant.access_token,
                "refresh_token": "",
                "expires_in": expires_in,
                "token_type": setting("token_type").unwrap_or_else(|| "Bearer".into()),
            });
            if let Some(id_token) = setting("id_token") {
                tokens["id_token"] = json!(id_token);
            }
            let mut record = json!({
                "tokens": tokens,
                "last_refresh": now_iso,
                "auth_mode": "oauth_device_code",
            });
            if let Some(endpoint) = setting("token_endpoint") {
                record["discovery"] = json!({ "token_endpoint": endpoint });
            }
            record
        }
        VaultProvider::Nous => {
            let mut record = serde_json::Map::new();
            for (key, value) in &grant.settings {
                if !matches!(
                    key.as_str(),
                    "access_token" | "refresh_token" | "expires_at" | "expires_in" | "obtained_at"
                ) {
                    record.insert(key.clone(), json!(value));
                }
            }
            record.insert("access_token".into(), json!(grant.access_token));
            record.insert("refresh_token".into(), json!(""));
            record.insert("expires_at".into(), json!(iso_utc(grant.expires_at)));
            record.insert("expires_in".into(), json!(expires_in));
            record.insert("obtained_at".into(), json!(now_iso));
            record
                .entry("token_type")
                .or_insert_with(|| json!("Bearer"));
            Value::Object(record)
        }
        _ => return None,
    })
}

/// Put the grant's record in Hermes' `auth.json` under `auth.lock`: Hermes'
/// own login for the provider, and every other provider and pool entry,
/// stay. Unchanged when it already holds this access token.
fn write_hermes(
    home: &Path,
    vault: VaultProvider,
    grant: &Grant,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<(), String> {
    let id = hermes_id(vault).ok_or("not a Hermes login")?;
    let record = hermes_record(vault, grant, now).ok_or("not a Hermes login")?;
    let now_iso = now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    update_json_store_at(&hermes_auth_file(home), &hermes_lock(home), flock, |map| {
        let mut changed = false;
        let current = map.get("providers").and_then(|p| p.get(id));
        let native = current.is_some_and(|r| hermes_refresh(r).is_some_and(|t| !t.trim().is_empty()));
        let fresh = current.is_some_and(|r| hermes_access(r) == Some(grant.access_token.as_str()));
        if !native && !fresh {
            let providers = map
                .entry("providers")
                .or_insert_with(|| json!({}));
            if !providers.is_object() {
                *providers = json!({});
            }
            providers[id] = record;
            changed = true;
        }
        // Hermes runs on its `active_provider` (what `hermes model` and
        // `hermes auth add` set): without one it reports no provider
        // connected, whatever logins it holds. One that names nothing
        // Hermes holds is no choice either.
        let held = |name: &str| map.get("providers").and_then(|p| p.get(name)).is_some();
        let chosen = map
            .get("active_provider")
            .and_then(Value::as_str)
            .is_some_and(|name| held(name));
        if !chosen {
            map.insert("active_provider".into(), json!(id));
            changed = true;
        }
        if changed {
            map.entry("version").or_insert_with(|| json!(1));
            map.insert("updated_at".into(), json!(now_iso));
        }
        changed
    })
}

/// Drop the broker's record for `vault`'s provider (never Hermes' own).
fn clear_hermes(home: &Path, vault: VaultProvider) {
    let Some(id) = hermes_id(vault) else { return };
    let file = hermes_auth_file(home);
    if !file.exists() {
        return;
    }
    let result = update_json_store_at(&file, &hermes_lock(home), flock, |map| {
        let Some(providers) = map.get_mut("providers").and_then(Value::as_object_mut) else {
            return false;
        };
        let managed = providers
            .get(id)
            .is_some_and(|r| hermes_refresh(r) == Some(""));
        if managed {
            providers.remove(id);
            if map.get("active_provider").and_then(Value::as_str) == Some(id) {
                map.remove("active_provider");
            }
        }
        managed
    });
    if let Err(error) = result {
        tracing::warn!(%error, provider = id, "credentials: failed to clear a managed Hermes login");
    }
}

// -- Devin --------------------------------------------------------------------

fn devin_file(devin_dir: &Path) -> PathBuf {
    devin_dir.join("credentials.toml")
}

/// `{data_dir}/cloud/devin-credentials.sha256`: what the broker last wrote.
fn devin_record(data_dir: &Path) -> PathBuf {
    data_dir.join("cloud").join("devin-credentials.sha256")
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest as _;
    format!("{:x}", sha2::Sha256::digest(bytes))
}

/// The file is there and isn't what the broker wrote: Devin's own login.
fn devin_native(devin_dir: &Path, record: &Path) -> bool {
    let Ok(bytes) = std::fs::read(devin_file(devin_dir)) else {
        return false;
    };
    std::fs::read_to_string(record)
        .ok()
        .as_deref()
        .map(str::trim)
        != Some(sha256_hex(&bytes).as_str())
}

/// `credentials.toml` for a Devin grant: the key as `windsurf_api_key` and
/// the grant's other (non-secret) fields as they were.
fn devin_toml(grant: &Grant) -> Result<String, String> {
    let mut table = toml::Table::new();
    for (key, value) in &grant.settings {
        if !matches!(key.as_str(), "windsurf_api_key" | "devin_api_key") {
            table.insert(key.clone(), toml::Value::String(value.clone()));
        }
    }
    table.insert(
        "windsurf_api_key".into(),
        toml::Value::String(grant.access_token.clone()),
    );
    toml::to_string(&table).map_err(|e| e.to_string())
}

fn write_devin(devin_dir: &Path, record: &Path, grant: &Grant) -> Result<(), String> {
    if devin_native(devin_dir, record) {
        return Ok(());
    }
    let text = devin_toml(grant)?;
    let file = devin_file(devin_dir);
    if std::fs::read(&file).ok().as_deref() == Some(text.as_bytes()) {
        return Ok(());
    }
    private_dir(devin_dir).map_err(|e| e.to_string())?;
    if let Some(dir) = record.parent() {
        private_dir(dir).map_err(|e| e.to_string())?;
    }
    // The record first: a crash between the two leaves a file that reads as
    // ours (the old one is then replaced on the next run), never the reverse.
    write_private_atomic(record, sha256_hex(text.as_bytes()).as_bytes())
        .map_err(|e| e.to_string())?;
    write_private_atomic(&file, text.as_bytes()).map_err(|e| e.to_string())
}

/// Remove the file only when it's still exactly what the broker wrote.
fn clear_devin(devin_dir: &Path, record: &Path) {
    let file = devin_file(devin_dir);
    if file.exists() && !devin_native(devin_dir, record) {
        let _ = std::fs::remove_file(&file);
        tracing::info!("credentials: removed the managed Devin login");
    }
    if !file.exists() {
        let _ = std::fs::remove_file(record);
    }
}

// -- Antigravity --------------------------------------------------------------

fn antigravity_token_file(home: &Path) -> PathBuf {
    home.join("antigravity-acp").join("acp_token.json")
}

fn antigravity_oauth_token_file(home: &Path) -> PathBuf {
    zeron_harness::acp::antigravity_oauth_token_file(home)
}

fn has_nested_refresh(doc: &Value) -> bool {
    doc.get("token")
        .is_some_and(|token| has_refresh(token, "refresh_token"))
}

fn is_managed_oauth_token(doc: &Value) -> bool {
    let Some(token) = doc.get("token").filter(|v| v.is_object()) else {
        return false;
    };
    !has_refresh(token, "refresh_token")
        && token
            .get("access_token")
            .and_then(Value::as_str)
            .is_some()
}

/// Antigravity's own Google login: either token file holding a refresh token
/// (the broker writes its own with an empty/absent one), or a business sign-in.
fn antigravity_token_exists(home: &Path) -> bool {
    let business = home.join("antigravity-acp").join("acp_business_token.json");
    business.is_file()
        || read_json(&antigravity_token_file(home))
            .is_some_and(|token| has_refresh(&token, "refresh_token"))
        || read_json(&antigravity_oauth_token_file(home))
            .is_some_and(|token| has_nested_refresh(&token))
}

/// Make Antigravity sign in with `GEMINI_API_KEY`.
fn write_antigravity_key_method(home: &Path) -> Result<(), String> {
    write_antigravity_method(home, "gemini-api-key")
}

/// Legacy `acp_token.json` for an `antigravity` grant: the access token and its
/// expiry (naive UTC, as google-auth reads it), the client and project, and
/// the keys a refresh would need there but EMPTY (google-auth requires them
/// present; the vault refreshes). When `proxied` is true, writes a far-future
/// expiry so `agy_acp_server`'s in-memory `Credentials.valid` never trips
/// mid-turn while the loopback proxy injects fresh vault tokens on every call.
fn antigravity_token_json(grant: &Grant, proxied: bool) -> Value {
    let expiry = if proxied {
        antigravity_proxy::PROXIED_TOKEN_EXPIRY.to_owned()
    } else {
        chrono::DateTime::from_timestamp_millis(grant.expires_at)
            .unwrap_or_default()
            .format("%Y-%m-%dT%H:%M:%S")
            .to_string()
    };
    let scopes: Vec<&str> = grant
        .settings
        .get("scopes")
        .map(|s| s.split_whitespace().collect())
        .unwrap_or_default();
    json!({
        "token": grant.access_token,
        "expiry": expiry,
        "refresh_token": "",
        "client_id": grant.settings.get("client_id").cloned().unwrap_or_default(),
        "client_secret": "",
        "scopes": scopes,
        "project_id": grant.settings.get("project_id").cloned().unwrap_or_default(),
    })
}

/// Unified CLI `antigravity-oauth-token` for an `antigravity` grant: access
/// token + RFC3339 UTC expiry and `"auth_method": "consumer"`, with NO refresh
/// token (the vault refreshes, and when `proxied` is true the far-future expiry
/// keeps the CLI from attempting an interactive login mid-turn).
fn antigravity_oauth_token_json(grant: &Grant, proxied: bool) -> Value {
    let expiry = if proxied {
        antigravity_proxy::PROXIED_TOKEN_EXPIRY_RFC3339.to_owned()
    } else {
        chrono::DateTime::from_timestamp_millis(grant.expires_at)
            .unwrap_or_default()
            .format("%Y-%m-%dT%H:%M:%SZ")
            .to_string()
    };
    json!({
        "token": {
            "access_token": grant.access_token,
            "token_type": "Bearer",
            "expiry": expiry,
        },
        "auth_method": "consumer",
    })
}

/// Antigravity on the vault's Google login: writes both the unified CLI's
/// `antigravity-oauth-token` and legacy `acp_token.json` (unless one of
/// Antigravity's own is there) and the personal sign-in method.
fn write_antigravity_login(home: &Path, grant: &Grant, proxied: bool) -> Result<(), String> {
    if antigravity_token_exists(home) {
        return Ok(());
    }
    for (file, desired) in [
        (
            antigravity_oauth_token_file(home),
            antigravity_oauth_token_json(grant, proxied),
        ),
        (
            antigravity_token_file(home),
            antigravity_token_json(grant, proxied),
        ),
    ] {
        if read_json(&file).as_ref() != Some(&desired) {
            if let Some(dir) = file.parent() {
                private_dir(dir).map_err(|e| e.to_string())?;
            }
            let bytes = serde_json::to_vec_pretty(&desired).map_err(|e| e.to_string())?;
            write_private_atomic(&file, &bytes).map_err(|e| e.to_string())?;
        }
    }
    write_antigravity_method(home, "oauth-personal")
}

/// Drop the broker's token files (never one of Antigravity's own).
fn clear_antigravity_login(home: &Path) {
    let mut removed = false;
    let legacy = antigravity_token_file(home);
    if read_json(&legacy).is_some_and(|token| is_managed(&token, "refresh_token")) {
        let _ = std::fs::remove_file(&legacy);
        removed = true;
    }
    let oauth = antigravity_oauth_token_file(home);
    if read_json(&oauth).is_some_and(|token| is_managed_oauth_token(&token)) {
        let _ = std::fs::remove_file(&oauth);
        removed = true;
    }
    if removed {
        tracing::info!("credentials: removed the managed Antigravity login");
    }
}

/// `auth.type` in Antigravity's `settings.json`, set when the file is absent
/// or plain JSON (anything else is the user's, and left alone).
fn write_antigravity_method(home: &Path, method: &str) -> Result<(), String> {
    let file = home.join("antigravity-acp").join("settings.json");
    let mut value = match read_json_guarded(&file) {
        Ok(Some(value)) => value,
        Ok(None) => json!({}),
        Err(()) => return Ok(()),
    };
    if value["auth"]["type"] == json!(method) {
        return Ok(());
    }
    if !value["auth"].is_object() {
        value["auth"] = json!({});
    }
    value["auth"]["type"] = json!(method);
    if let Some(dir) = file.parent() {
        private_dir(dir).map_err(|e| e.to_string())?;
    }
    let bytes = serde_json::to_vec_pretty(&value).map_err(|e| e.to_string())?;
    write_private_atomic(&file, &bytes).map_err(|e| e.to_string())
}

// -- Muse ---------------------------------------------------------------------

/// `{data_dir}/cloud/muse-auth.sha256`: what the broker last wrote.
fn muse_record(data_dir: &Path) -> PathBuf {
    data_dir.join("cloud").join("muse-auth.sha256")
}

/// A file at `file` the broker didn't write (`record` holds the SHA-256 of
/// what it did): the agent's own login.
fn foreign_file(file: &Path, record: &Path) -> bool {
    let Ok(bytes) = std::fs::read(file) else {
        return false;
    };
    std::fs::read_to_string(record)
        .ok()
        .as_deref()
        .map(str::trim)
        != Some(sha256_hex(&bytes).as_str())
}

fn muse_native(file: &Path, record: &Path) -> bool {
    foreign_file(file, record)
}

/// Every `refresh_token` string in `value`, emptied (the vault already did;
/// the machine never writes one either way).
fn blank_refresh_tokens(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (key, field) in map.iter_mut() {
                if key == "refresh_token" && field.is_string() {
                    *field = json!("");
                } else {
                    blank_refresh_tokens(field);
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(blank_refresh_tokens),
        _ => {}
    }
}

/// Muse's `auth.json` for a `muse` grant: the document the vault holds,
/// refresh tokens emptied.
fn muse_document(grant: &Grant) -> Result<String, String> {
    let raw = grant
        .settings
        .get("document")
        .ok_or("the Muse grant carries no login document")?;
    let mut document: Value = serde_json::from_str(raw).map_err(|e| e.to_string())?;
    if !document.is_object() {
        return Err("the Muse login document isn't a JSON object".into());
    }
    blank_refresh_tokens(&mut document);
    serde_json::to_string_pretty(&document).map_err(|e| e.to_string())
}

fn write_muse(file: &Path, record: &Path, grant: &Grant) -> Result<(), String> {
    if muse_native(file, record) {
        return Ok(());
    }
    let text = muse_document(grant)?;
    if std::fs::read(file).ok().as_deref() == Some(text.as_bytes()) {
        return Ok(());
    }
    if let Some(dir) = file.parent() {
        private_dir(dir).map_err(|e| e.to_string())?;
    }
    if let Some(dir) = record.parent() {
        private_dir(dir).map_err(|e| e.to_string())?;
    }
    // The record first, as Devin's.
    write_private_atomic(record, sha256_hex(text.as_bytes()).as_bytes())
        .map_err(|e| e.to_string())?;
    write_private_atomic(file, text.as_bytes()).map_err(|e| e.to_string())
}

fn clear_muse(file: &Path, record: &Path) {
    if file.exists() && !muse_native(file, record) {
        let _ = std::fs::remove_file(file);
        tracing::info!("credentials: removed the managed Muse login");
    }
    if !file.exists() {
        let _ = std::fs::remove_file(record);
    }
}

// -- OpenCode -----------------------------------------------------------------

/// OpenCode's `Credential.Value` for a vault login it takes through its
/// credential API (its harness adds the sign-in method): ChatGPT with an
/// EMPTY refresh token; Copilot as OpenCode keeps it — the GitHub token
/// (which doesn't rotate) in both fields, exchanged by OpenCode itself.
fn opencode_value(vault: VaultProvider, grant: &Grant) -> Option<Value> {
    match vault {
        VaultProvider::Codex => {
            let mut value = json!({
                "type": "oauth",
                "refresh": "",
                "access": grant.access_token,
                "expires": grant.expires_at,
            });
            let account = grant.account_id.clone().or_else(|| {
                jwt_payload(&grant.access_token)?["https://api.openai.com/auth"]
                    ["chatgpt_account_id"]
                    .as_str()
                    .map(str::to_owned)
            });
            if let Some(account) = account {
                value["metadata"] = json!({ "accountID": account });
            }
            Some(value)
        }
        VaultProvider::Copilot => Some(json!({
            "type": "oauth",
            "refresh": grant.access_token,
            "access": grant.access_token,
            "expires": 0,
        })),
        _ => None,
    }
}

/// `POST /vault/{orgId}/grants` reply.
#[derive(Deserialize)]
struct GrantsReply {
    #[serde(default)]
    grants: Vec<Grant>,
    #[serde(default)]
    missing: Vec<MissingGrant>,
}

#[derive(Deserialize)]
struct MissingGrant {
    provider: String,
    #[serde(default)]
    error: String,
    #[serde(default)]
    message: String,
}

impl GrantsReply {
    /// One outcome per asked provider: its grant, why it has none, or (named
    /// nowhere in the reply) unavailable.
    fn outcomes(self, providers: &[VaultProvider]) -> HashMap<VaultProvider, GrantOutcome> {
        let mut out = HashMap::new();
        for grant in self.grants {
            if let Some(provider) = VaultProvider::parse(&grant.provider)
                .filter(|p| providers.contains(p))
                .filter(|_| !grant.access_token.is_empty())
            {
                out.insert(provider, GrantOutcome::Granted(grant));
            }
        }
        for missing in self.missing {
            let Some(provider) = VaultProvider::parse(&missing.provider) else {
                continue;
            };
            // The status a single grant would have answered with.
            let status = match missing.error.as_str() {
                "upstream" => 502,
                "unavailable" => 503,
                _ => 403,
            };
            let body = EdgeErrorBody {
                error: missing.error,
                message: missing.message,
            };
            out.entry(provider)
                .or_insert_with(|| classify_grant_failure(status, &body));
        }
        out
    }
}

/// Vault failure → outcome. Only transient failures (502 `upstream`, 503
/// `unavailable`, 408/429, and the runner bearer itself being refused) keep a
/// cached grant alive; 403 (`device_unknown`/`device_revoked`/
/// `not_authorized`/`forbidden`/`disabled`), 409 `needs_reconnect`, a bad
/// signature, and either kill switch (`disabled`, even as a 503) drop it.
fn classify_grant_failure(status: u16, body: &EdgeErrorBody) -> GrantOutcome {
    let detail = body.describe();
    let error = body.error.as_str();
    if error == "disabled" || error == "bad_signature" {
        return GrantOutcome::NotConnected(detail);
    }
    if status >= 500 || matches!(status, 401 | 408 | 429) {
        return GrantOutcome::Unavailable(format!("vault grant failed ({status}): {detail}"));
    }
    GrantOutcome::NotConnected(detail)
}

#[async_trait::async_trait]
impl crate::registry::RunEnvironment for CredentialBroker {
    async fn prepare(
        &self,
        harness: HarnessId,
        _chat_id: Option<&str>,
        request: &mut RunRequest,
    ) -> Result<(), String> {
        CredentialBroker::prepare(self, harness, request).await
    }

    async fn credential(&self, harness: HarnessId, _chat_id: Option<&str>) -> Option<String> {
        CredentialBroker::credential(self, harness).await
    }

    async fn record_usage(
        &self,
        _harness: HarnessId,
        credential: &str,
        windows: Vec<zeron_proto::AgentUsageWindow>,
        plan_label: Option<String>,
    ) {
        CredentialBroker::report_usage(self, credential, windows, plan_label).await;
    }
}

/// ChatGPT's usage view (`/wham/usage`), as Codex reads it.
const CODEX_USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";

/// How soon the same account's usage is reported again: the same figures
/// within a minute, or any at all within ten seconds, aren't.
const USAGE_REPORT_SAME: Duration = Duration::from_secs(60);
const USAGE_REPORT_GAP: Duration = Duration::from_secs(10);

impl CredentialBroker {
    /// A run's subscription usage (`AgentEvent::PlanUsage`), kept on the edge
    /// as its account's (`POST /runner/usage`): where a device's Cloud card
    /// reads it when the vault can't ask the provider itself (ChatGPT turns
    /// Cloudflare's servers away). Only a run on one account's subscription
    /// (`credential_accounts`); best effort.
    pub async fn report_usage(
        &self,
        credential: &str,
        windows: Vec<zeron_proto::AgentUsageWindow>,
        plan_label: Option<String>,
    ) {
        let Some(body) = usage_report(credential, &windows, plan_label.as_deref()) else {
            return;
        };
        let account = body["accounts"].as_str().unwrap_or_default().to_owned();
        let said = body.to_string();
        {
            let mut reported = self.inner.usage_reported.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some((at, last)) = reported.get(&account)
                && (at.elapsed() < USAGE_REPORT_GAP || (*last == said && at.elapsed() < USAGE_REPORT_SAME))
            {
                return;
            }
            reported.insert(account, (Instant::now(), said));
        }
        let Ok(bearer) = self.inner.tokens.token().await else {
            return;
        };
        let url = format!("{}/runner/usage", self.inner.identity.edge_url());
        match self.inner.http.post(&url).bearer_auth(bearer).json(&body).send().await {
            Ok(res) if res.status().is_success() || res.status().as_u16() == 404 => {}
            Ok(res) => tracing::warn!(status = res.status().as_u16(), "usage report refused"),
            Err(err) => tracing::warn!(error = %describe_http_error(err), "usage report failed"),
        }
    }
}

/// What [`CredentialBroker::report_usage`] sends: `{accounts, windows,
/// planLabel?}`, for a run on exactly one account's subscription.
fn usage_report(
    credential: &str,
    windows: &[zeron_proto::AgentUsageWindow],
    plan_label: Option<&str>,
) -> Option<Value> {
    let account = credential_accounts(credential).filter(|a| !a.contains(','))?;
    if windows.is_empty() {
        return None;
    }
    let mut body = serde_json::json!({ "accounts": account, "windows": windows });
    if let Some(plan) = plan_label.filter(|p| !p.is_empty()) {
        body["planLabel"] = Value::String(plan.to_owned());
    }
    Some(body)
}

// ---------------------------------------------------------------------------
// Codex auth.json
// ---------------------------------------------------------------------------

/// The `auth.json` codex reads for a ChatGPT login. `refresh_token` MUST be
/// present and empty: the vault is the only refresher, and codex rejects a
/// token set without the field.
pub fn codex_chatgpt_auth_json(grant: &Grant, now: chrono::DateTime<chrono::Utc>) -> Value {
    let access_claims = jwt_payload(&grant.access_token).unwrap_or(Value::Null);
    let account_id = grant.account_id.clone().or_else(|| {
        access_claims["https://api.openai.com/auth"]["chatgpt_account_id"]
            .as_str()
            .map(str::to_owned)
    });
    let id_token = grant
        .id_token
        .clone()
        .filter(|token| !token.trim().is_empty())
        .unwrap_or_else(|| synthesize_id_token(&access_claims, account_id.as_deref(), grant));
    json!({
        "OPENAI_API_KEY": null,
        "tokens": {
            "id_token": id_token,
            "access_token": grant.access_token,
            "refresh_token": "",
            "account_id": account_id,
        },
        "last_refresh": now.to_rfc3339_opts(chrono::SecondsFormat::Micros, true),
    })
}

/// `auth.json` for the user's own OpenAI API key.
pub fn codex_api_key_auth_json(key: &str) -> Value {
    json!({ "OPENAI_API_KEY": key })
}

/// Same credential (ignoring `last_refresh`, which we restamp).
fn same_codex_credential(current: &Value, desired: &Value) -> bool {
    current.get("OPENAI_API_KEY") == desired.get("OPENAI_API_KEY")
        && current.get("tokens") == desired.get("tokens")
}

/// codex parses `id_token` (unverified) for the account's email, plan and
/// workspace; an empty one reads as signed out. When the vault has none, mint
/// an unsigned stand-in from the access token's own claims.
fn synthesize_id_token(access_claims: &Value, account_id: Option<&str>, grant: &Grant) -> String {
    let mut auth = access_claims["https://api.openai.com/auth"].clone();
    if !auth.is_object() {
        auth = json!({});
    }
    if let Some(account_id) = account_id {
        auth["chatgpt_account_id"] = json!(account_id);
    }
    let email = grant
        .account
        .clone()
        .filter(|account| account.contains('@'))
        .or_else(|| {
            access_claims["https://api.openai.com/profile"]["email"]
                .as_str()
                .map(str::to_owned)
        });
    let payload = json!({ "email": email, "https://api.openai.com/auth": auth });
    format!(
        "{}.{}.unsigned",
        B64URL.encode(br#"{"alg":"none","typ":"JWT"}"#),
        B64URL.encode(payload.to_string().as_bytes())
    )
}

fn jwt_payload(token: &str) -> Option<Value> {
    let payload = token.split('.').nth(1)?;
    let bytes = B64URL.decode(payload.trim_end_matches('=')).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Seed the managed home's `config.toml` once: a copy of the user's
/// `~/.codex/config.toml` when present, pinned to file-based credentials.
fn seed_codex_config(home: &Path, user_config: &Path) {
    let path = home.join("config.toml");
    if path.exists() {
        return;
    }
    let user = std::fs::read_to_string(user_config).unwrap_or_default();
    let mut seeded = String::new();
    if !user.contains("cli_auth_credentials_store") {
        // Top-level key first (before any [table]): the managed auth.json is
        // the credential, never a keyring entry.
        seeded.push_str("cli_auth_credentials_store = \"file\"\n");
    }
    seeded.push_str(&user);
    if let Err(err) = std::fs::write(&path, seeded) {
        tracing::warn!(error = %err, "credentials: failed to seed the managed codex config.toml");
    }
}

// ---------------------------------------------------------------------------
// Claude Code credentials
// ---------------------------------------------------------------------------

fn claude_credentials_file(config: &BrokerConfig) -> PathBuf {
    config.claude_config_dir.join(".credentials.json")
}

fn claude_native_backup(config: &BrokerConfig) -> PathBuf {
    config
        .data_dir
        .join("cloud")
        .join("claude-native-credentials.json")
}

/// Drop a vault-managed `claudeAiOauth` (empty refresh token), restoring a
/// backed-up native login if one was displaced; a native login is untouched.
fn clear_managed_claude(config: &BrokerConfig) {
    let path = claude_credentials_file(config);
    let Ok(Some(mut value)) = read_json_guarded(&path) else {
        return;
    };
    let Some(map) = value.as_object_mut() else {
        return;
    };
    let managed = map
        .get("claudeAiOauth")
        .is_some_and(|oauth| oauth.is_object() && !is_native_claude_oauth(oauth));
    if !managed {
        return;
    }
    let backup = claude_native_backup(config);
    match read_json(&backup) {
        Some(native) => {
            map.insert("claudeAiOauth".into(), native);
        }
        None => {
            map.remove("claudeAiOauth");
        }
    }
    let result = serde_json::to_vec_pretty(&value)
        .map_err(|err| crate::EngineError::Other(err.to_string()))
        .and_then(|bytes| write_private_atomic(&path, &bytes));
    match result {
        Ok(()) => {
            let _ = std::fs::remove_file(&backup);
            tracing::info!("credentials: removed the vault Claude login");
        }
        Err(err) => tracing::warn!(error = %err, "credentials: failed to clear the Claude login"),
    }
}

/// Scrub every credential file a broker manages — a copied (forked /
/// template) runner identity is being discarded. User files, repositories,
/// and a native Claude login (real refresh token) are left alone.
pub fn discard_managed_credentials(config: &BrokerConfig) {
    let codex = managed_codex_home(&config.data_dir);
    if codex.exists() {
        match std::fs::remove_dir_all(&codex) {
            Ok(()) => tracing::info!("credentials: discarded a previous managed CODEX_HOME"),
            Err(err) => {
                tracing::warn!(error = %err, "credentials: failed to remove the managed CODEX_HOME")
            }
        }
    }
    if std::fs::remove_file(git_credentials_path(&config.data_dir)).is_ok() {
        tracing::info!("credentials: discarded previous git credentials");
    }
    let hosts = gh_hosts_path(&config.home);
    if is_managed_hosts(&hosts) {
        let _ = std::fs::remove_file(&hosts);
    }
    clear_managed_claude(config);
    let agents = &config.agents;
    clear_grok_managed(&agents.grok_home);
    clear_pi_codex(&agents.pi_agent_dir);
    clear_pi_entry(&agents.pi_agent_dir, PI_CLAUDE_ENTRY);
    clear_devin(&agents.devin_dir, &devin_record(&config.data_dir));
    for vault in [VaultProvider::Codex, VaultProvider::Nous, VaultProvider::XaiOauth] {
        clear_hermes(&agents.hermes_home, vault);
    }
    if let Some(home) = &agents.antigravity_home {
        clear_antigravity_login(home);
    }
    clear_muse(&agents.muse_auth_file, &muse_record(&config.data_dir));
}

/// `claudeAiOauth` for a vault grant. `refreshToken` is present and EMPTY:
/// the refresh token never leaves the vault.
pub fn claude_oauth_json(grant: &Grant) -> Value {
    json!({
        "accessToken": grant.access_token,
        "refreshToken": "",
        "expiresAt": grant.expires_at,
        "scopes": grant.scopes.clone().unwrap_or_default(),
        "subscriptionType": grant.subscription_type,
    })
}

/// A login written by the CLI itself carries a real refresh token.
fn is_native_claude_oauth(oauth: &Value) -> bool {
    oauth["refreshToken"]
        .as_str()
        .is_some_and(|token| !token.trim().is_empty())
}

fn read_json(path: &Path) -> Option<Value> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

/// `Ok(None)` = missing; `Err` = present but not a JSON object (never
/// overwrite a file we can't parse — it may hold the user's own state).
fn read_json_guarded(path: &Path) -> Result<Option<Value>, ()> {
    match std::fs::read(path) {
        Ok(raw) => match serde_json::from_slice::<Value>(&raw) {
            Ok(value) if value.is_object() => Ok(Some(value)),
            _ => Err(()),
        },
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(()),
    }
}

/// Claude Code's first-run flow (theme picker, login prompt) must not block a
/// headless sandbox: mark onboarding complete in its state file, touching
/// nothing else (the same merge discipline as an account switch).
fn ensure_claude_onboarded(file: &Path) {
    let Ok(existing) = read_json_guarded(file) else {
        return;
    };
    let mut value = existing.unwrap_or_else(|| json!({}));
    let Some(map) = value.as_object_mut() else {
        return;
    };
    if map.get("hasCompletedOnboarding") == Some(&Value::Bool(true)) {
        return;
    }
    map.insert("hasCompletedOnboarding".into(), Value::Bool(true));
    if let Err(err) = serde_json::to_vec_pretty(&value)
        .map_err(|err| crate::EngineError::Other(err.to_string()))
        .and_then(|bytes| write_private_atomic(file, &bytes))
    {
        tracing::warn!(error = %err, "credentials: failed to mark Claude Code onboarded");
    }
}

// ---------------------------------------------------------------------------
// git + gh
// ---------------------------------------------------------------------------

/// `git config --global credential.https://github.com.helper "store --file=…"`
/// (only when it differs). `HOME` is pinned so the "global" config is the
/// broker's home. Returns whether the helper is in place.
/// Wire the store as git's helper for github.com. An empty helper comes
/// first: it resets the helper list, so a helper from the system config
/// (Apple git's `osxkeychain`, a distro's libsecret or cache) never also
/// stores — or prompts to store — the short-lived App token.
async fn configure_git_helper(home: &Path, store: &Path) -> bool {
    let key = "credential.https://github.com.helper";
    let value = format!("store --file=\"{}\"", store.display());
    let git = |args: &[&str]| {
        let mut cmd = tokio::process::Command::new("git");
        cmd.args(args)
            .env("HOME", home)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        cmd
    };
    let wanted = format!("\n{value}\n");
    if let Ok(out) = git(&["config", "--global", "--get-all", key])
        .output()
        .await
        && out.status.success()
        && String::from_utf8_lossy(&out.stdout) == wanted
    {
        return true;
    }
    let run = |args: Vec<String>| async move {
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        git(&args).output().await
    };
    let steps = [
        vec![
            "config".into(),
            "--global".into(),
            "--replace-all".into(),
            key.into(),
            String::new(),
        ],
        vec![
            "config".into(),
            "--global".into(),
            "--add".into(),
            key.into(),
            value.clone(),
        ],
    ];
    for step in steps {
        match run(step).await {
            Ok(out) if out.status.success() => {}
            Ok(out) => {
                tracing::warn!(stderr = %String::from_utf8_lossy(&out.stderr).trim(),
                    "credentials: git config failed");
                return false;
            }
            Err(err) => {
                tracing::warn!(error = %err, "credentials: git is not runnable");
                return false;
            }
        }
    }
    tracing::info!("credentials: git credential helper configured for github.com");
    true
}

fn gh_hosts_path(home: &Path) -> PathBuf {
    home.join(".config").join("gh").join("hosts.yml")
}

fn is_managed_hosts(path: &Path) -> bool {
    std::fs::read_to_string(path)
        .ok()
        .is_some_and(|raw| raw.lines().next() == Some(HOSTS_MARKER))
}

/// `~/.config/gh/hosts.yml` for interactive terminals — written only when
/// absent or already Zeron-managed (a user's own `gh auth login` wins).
fn write_gh_hosts(home: &Path, grant: &Grant) {
    let path = gh_hosts_path(home);
    if path.exists() && !is_managed_hosts(&path) {
        return;
    }
    let user = grant
        .account
        .clone()
        .filter(|login| !login.trim().is_empty() && !login.contains(['\n', ':', '"']))
        .unwrap_or_else(|| "x-access-token".into());
    let token = &grant.access_token;
    let body = format!(
        "{HOSTS_MARKER}\ngithub.com:\n    users:\n        {user}:\n            oauth_token: {token}\n    git_protocol: https\n    oauth_token: {token}\n    user: {user}\n"
    );
    if std::fs::read_to_string(&path).ok().as_deref() == Some(body.as_str()) {
        return;
    }
    if let Some(dir) = path.parent()
        && let Err(err) = std::fs::create_dir_all(dir)
    {
        tracing::warn!(error = %err, "credentials: cannot create the gh config dir");
        return;
    }
    if let Err(err) = write_private_atomic(&path, body.as_bytes()) {
        tracing::warn!(error = %err, "credentials: failed to write gh hosts.yml");
    }
}

/// Who a Cloud machine's commits are by: the signed-in GitHub user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GitIdentity {
    pub name: String,
    pub email: String,
}

impl GitIdentity {
    /// From `GET /users/{login}`: the profile name (else the login) and the
    /// noreply address keyed by the account id.
    fn from_user(login: &str, user: &Value) -> Option<Self> {
        let id = user["id"].as_u64()?;
        let login = user["login"]
            .as_str()
            .filter(|l| is_github_login(l))
            .unwrap_or(login);
        let name = user["name"]
            .as_str()
            .map(str::trim)
            .filter(|name| !name.is_empty() && !name.contains(['\n', '<', '>']))
            .unwrap_or(login);
        Some(Self {
            name: name.to_string(),
            email: format!("{id}+{login}@users.noreply.github.com"),
        })
    }

    fn env(&self) -> [(&'static str, String); 4] {
        [
            ("GIT_AUTHOR_NAME", self.name.clone()),
            ("GIT_AUTHOR_EMAIL", self.email.clone()),
            ("GIT_COMMITTER_NAME", self.name.clone()),
            ("GIT_COMMITTER_EMAIL", self.email.clone()),
        ]
    }
}

fn is_github_login(login: &str) -> bool {
    !login.is_empty()
        && login.len() <= 39
        && login.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// The machine's global git identity, for the user's own terminals: set
/// when there is none, or when it's a bot's (an agent that configured the
/// App it pushes as). An identity the user chose stays.
async fn configure_git_identity(home: &Path, identity: &GitIdentity) {
    // `git config --global` writes $XDG_CONFIG_HOME/git/config when that file
    // exists. An XDG_CONFIG_HOME outside `home` belongs to another home (this
    // process's own, when `home` is a sandbox or a test directory), so `home`'s is
    // used instead; otherwise the identity lands in that other user's config.
    // When `home` is this process's own, its XDG_CONFIG_HOME is the user's
    // wherever it lives: replacing it would hide an identity kept there, and
    // ~/.gitconfig (which wins over it) would get the machine's.
    let own_home = std::env::var_os("HOME").is_some_and(|own| Path::new(&own) == home);
    let xdg_config = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .filter(|dir| own_home || dir.starts_with(home))
        .unwrap_or_else(|| home.join(".config"));
    let git = |args: &[&str]| {
        let mut cmd = tokio::process::Command::new("git");
        cmd.args(args)
            .env("HOME", home)
            .env("XDG_CONFIG_HOME", &xdg_config)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null());
        cmd
    };
    let current = match git(&["config", "--global", "user.email"]).output().await {
        Ok(out) => String::from_utf8_lossy(&out.stdout).trim().to_string(),
        Err(_) => return,
    };
    if !current.is_empty() && !current.contains("[bot]") {
        return;
    }
    for (key, value) in [
        ("user.name", &identity.name),
        ("user.email", &identity.email),
    ] {
        if let Err(err) = git(&["config", "--global", key, value]).output().await {
            tracing::warn!(error = %err, "credentials: setting the git identity failed");
            return;
        }
    }
}

/// `dir` created and made owner-only (0700 on Unix).
fn private_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// GitHub REST
// ---------------------------------------------------------------------------

enum GithubError {
    Unauthorized,
    Other(String),
}

async fn github_get(http: &reqwest::Client, url: &str, token: &str) -> Result<Value, GithubError> {
    let res = http
        .get(url)
        .bearer_auth(token)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .send()
        .await
        .map_err(|err| {
            GithubError::Other(format!(
                "GitHub is unreachable: {}",
                describe_http_error(err)
            ))
        })?;
    let status = res.status().as_u16();
    if status == 401 {
        return Err(GithubError::Unauthorized);
    }
    if !res.status().is_success() {
        return Err(GithubError::Other(format!(
            "GitHub request failed ({status})"
        )));
    }
    res.json::<Value>().await.map_err(|err| {
        GithubError::Other(format!(
            "malformed GitHub reply: {}",
            describe_http_error(err)
        ))
    })
}

/// Paginate `{url}&page=N` (100 per page) collecting `items(reply)`.
async fn github_pages(
    http: &reqwest::Client,
    url: &str,
    token: &str,
    items: impl Fn(&Value) -> Vec<Value>,
) -> Result<Vec<Value>, GithubError> {
    let mut out = Vec::new();
    for page in 1..=GITHUB_MAX_PAGES {
        let reply = github_get(http, &format!("{url}&page={page}"), token).await?;
        let batch = items(&reply);
        let full = batch.len() >= 100;
        out.extend(batch);
        if !full {
            break;
        }
    }
    Ok(out)
}

/// An installation token sees exactly its installation's repositories.
async fn fetch_github_repos(
    http: &reqwest::Client,
    api: &str,
    token: &str,
) -> Result<Vec<GithubRepo>, GithubError> {
    let api = api.trim_end_matches('/');
    let raw = github_pages(
        http,
        &format!("{api}/installation/repositories?per_page=100"),
        token,
        |reply| {
            reply["repositories"]
                .as_array()
                .cloned()
                .unwrap_or_default()
        },
    )
    .await?;
    Ok(raw.iter().filter_map(parse_github_repo).collect())
}

fn parse_github_repo(value: &Value) -> Option<GithubRepo> {
    let full_name = value["full_name"].as_str()?.to_owned();
    Some(GithubRepo {
        clone_url: value["clone_url"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| format!("https://github.com/{full_name}.git")),
        default_branch: value["default_branch"]
            .as_str()
            .unwrap_or("main")
            .to_owned(),
        private: value["private"].as_bool().unwrap_or(false),
        description: value["description"]
            .as_str()
            .filter(|d| !d.trim().is_empty())
            .map(str::to_owned),
        pushed_at: value["pushed_at"]
            .as_str()
            .and_then(|at| chrono::DateTime::parse_from_rfc3339(at).ok())
            .map(|at| at.timestamp_millis())
            .unwrap_or(0),
        full_name,
        ..Default::default()
    })
}

/// Dedupe by `owner/name`, keep `query` matches (case-insensitive substring),
/// newest push first.
pub fn filter_and_sort_repos(repos: Vec<GithubRepo>, query: Option<&str>) -> Vec<GithubRepo> {
    let needle = query
        .map(|q| q.trim().to_lowercase())
        .filter(|q| !q.is_empty());
    let mut seen = std::collections::HashSet::new();
    let mut out: Vec<GithubRepo> = repos
        .into_iter()
        .filter(|repo| seen.insert(repo.full_name.to_lowercase()))
        .filter(|repo| {
            needle
                .as_deref()
                .is_none_or(|needle| repo.full_name.to_lowercase().contains(needle))
        })
        .collect();
    out.sort_by(|a, b| {
        b.pushed_at
            .cmp(&a.pushed_at)
            .then_with(|| a.full_name.cmp(&b.full_name))
    });
    out
}

#[cfg(test)]
mod tests {
    use super::{GitIdentity, configure_git_identity};

    #[tokio::test]
    async fn the_machine_identity_never_overrides_the_users_own() {
        let home = tempfile::tempdir().unwrap();
        let git = |args: &[&str]| {
            // XDG_CONFIG_HOME too: with the developer's set, `--global` is their real config.
            let out = std::process::Command::new("git")
                .args(args)
                .env("HOME", home.path())
                .env("XDG_CONFIG_HOME", home.path().join(".config"))
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .output()
                .unwrap();
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        };
        let user = GitIdentity {
            name: "Octo Cat".into(),
            email: "583231+octo@users.noreply.github.com".into(),
        };
        // A bot identity an agent configured is replaced.
        git(&[
            "config",
            "--global",
            "user.email",
            "346184242+zeronsh[bot]@users.noreply.github.com",
        ]);
        configure_git_identity(home.path(), &user).await;
        assert_eq!(git(&["config", "--global", "user.email"]), user.email);
        // One the user chose stays.
        git(&["config", "--global", "user.email", "me@example.com"]);
        configure_git_identity(home.path(), &user).await;
        assert_eq!(git(&["config", "--global", "user.email"]), "me@example.com");
    }

    #[test]
    fn identity_comes_from_the_github_profile() {
        let user = serde_json::json!({"login": "octo", "id": 7, "name": "  "});
        let identity = GitIdentity::from_user("octo", &user).unwrap();
        assert_eq!(
            identity.name, "octo",
            "a blank name falls back to the login"
        );
        assert_eq!(identity.email, "7+octo@users.noreply.github.com");
        assert!(GitIdentity::from_user("octo", &serde_json::json!({"login": "octo"})).is_none());
    }
    use super::*;

    fn grant(access_token: &str) -> Grant {
        Grant {
            provider: "codex".into(),
            access_token: access_token.into(),
            expires_at: crate::now_ms() + 3_600_000,
            account_id: Some("acct_1".into()),
            id_token: Some("h.p.s".into()),
            account: Some("u@example.com".into()),
            scopes: None,
            subscription_type: None,
            settings: BTreeMap::new(),
            generation: 3,
            slot: Some("s1".into()),
        }
    }

    /// A run's usage goes as its one account's; a run on several (Pi on a
    /// ChatGPT and a Claude login) or none says nothing.
    #[test]
    fn a_runs_usage_is_its_one_accounts() {
        let windows = vec![zeron_proto::AgentUsageWindow { label: "5h".into(), used_fraction: 0.25, resets_at: None }];
        let body = usage_report("codex@c1:7", &windows, Some("ChatGPT Pro")).unwrap();
        assert_eq!(body["accounts"], "codex@c1");
        assert_eq!(body["windows"][0]["usedFraction"], 0.25);
        assert_eq!(body["planLabel"], "ChatGPT Pro");
        assert!(usage_report("codex@c1:7", &[], None).is_none());
        assert!(usage_report("claude@a1:2,codex@c1:7", &windows, None).is_none());
        assert!(usage_report("native", &windows, None).is_none());
        assert!(usage_report("codex:7", &windows, None).is_none(), "an older vault names no account");
    }

    /// A fingerprint names each login's account and version; the accounts
    /// alone say whose catalog it is: the same across a token refresh (or
    /// Antigravity's nearing expiry), another after a switch of account, and
    /// none where it names no account.
    #[test]
    fn a_fingerprints_accounts_drop_their_versions() {
        let mut g = grant("at");
        assert_eq!(credential_part("codex", &g), "codex@s1:3");
        g.slot = None;
        assert_eq!(credential_part("codex", &g), "codex:3", "an older vault names no account");
        assert_eq!(credential_accounts("pi-key@k2:4,codex@s1:7").as_deref(), Some("codex@s1,pi-key@k2"));
        assert_eq!(credential_accounts("antigravity@a:9:expiring").as_deref(), Some("antigravity@a"));
        assert_eq!(credential_accounts("codex@s1:8"), credential_accounts("codex@s1:3"));
        assert_ne!(credential_accounts("codex@s2:3"), credential_accounts("codex@s1:3"));
        for unnamed in ["codex:3", "codex@s1:3,claude:2", "native", "none", ""] {
            assert_eq!(credential_accounts(unnamed), None, "{unnamed}");
        }
    }

    #[test]
    fn codex_auth_json_has_an_empty_refresh_token_and_no_api_key() {
        let now = chrono::DateTime::parse_from_rfc3339("2026-10-01T12:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let value = codex_chatgpt_auth_json(&grant("at-1"), now);
        assert_eq!(value["OPENAI_API_KEY"], Value::Null);
        assert!(value.as_object().unwrap().contains_key("OPENAI_API_KEY"));
        let tokens = value["tokens"].as_object().unwrap();
        assert_eq!(tokens["access_token"], "at-1");
        assert_eq!(tokens["id_token"], "h.p.s");
        assert_eq!(tokens["account_id"], "acct_1");
        // Present AND empty — deleting the field breaks codex.
        assert_eq!(tokens.get("refresh_token"), Some(&json!("")));
        assert!(
            value["last_refresh"]
                .as_str()
                .unwrap()
                .starts_with("2026-10-01T12:00:00")
        );
        assert_eq!(
            codex_api_key_auth_json("sk-x"),
            json!({ "OPENAI_API_KEY": "sk-x" })
        );
    }

    #[test]
    fn missing_id_token_is_synthesized_from_the_access_token_claims() {
        let claims = json!({
            "https://api.openai.com/auth": {"chatgpt_account_id": "acct_9", "chatgpt_plan_type": "pro"},
            "https://api.openai.com/profile": {"email": "p@example.com"},
        });
        let access = format!("e30.{}.sig", B64URL.encode(claims.to_string().as_bytes()));
        let mut g = grant(&access);
        g.id_token = None;
        g.account_id = None;
        g.account = None;
        let value = codex_chatgpt_auth_json(&g, chrono::Utc::now());
        assert_eq!(value["tokens"]["account_id"], "acct_9");
        let id = jwt_payload(value["tokens"]["id_token"].as_str().unwrap()).unwrap();
        assert_eq!(id["email"], "p@example.com");
        assert_eq!(
            id["https://api.openai.com/auth"]["chatgpt_plan_type"],
            "pro"
        );
        assert_eq!(
            id["https://api.openai.com/auth"]["chatgpt_account_id"],
            "acct_9"
        );
    }

    #[test]
    fn credential_comparison_ignores_last_refresh() {
        let a = codex_chatgpt_auth_json(&grant("at-1"), chrono::Utc::now());
        let mut b = a.clone();
        b["last_refresh"] = json!("1999-01-01T00:00:00Z");
        assert!(same_codex_credential(&a, &b));
        let c = codex_chatgpt_auth_json(&grant("at-2"), chrono::Utc::now());
        assert!(!same_codex_credential(&a, &c));
    }

    #[test]
    fn vault_failures_classify_per_the_contract() {
        let body = |error: &str| EdgeErrorBody {
            error: error.into(),
            message: String::new(),
        };
        let not_connected = |status, error: &str| {
            matches!(
                classify_grant_failure(status, &body(error)),
                GrantOutcome::NotConnected(_)
            )
        };
        let unavailable = |status, error: &str| {
            matches!(
                classify_grant_failure(status, &body(error)),
                GrantOutcome::Unavailable(_)
            )
        };
        for error in [
            "device_unknown",
            "device_revoked",
            "not_authorized",
            "forbidden",
            "disabled",
        ] {
            assert!(not_connected(403, error), "{error}");
        }
        assert!(not_connected(409, "needs_reconnect"));
        assert!(not_connected(404, "not_found"));
        assert!(not_connected(401, "bad_signature"));
        // The global kill switch drops cached grants too.
        assert!(not_connected(503, "disabled"));
        assert!(unavailable(502, "upstream"));
        assert!(unavailable(503, "unavailable"));
        assert!(unavailable(429, ""));
        assert!(unavailable(401, "stale"));
        assert!(unavailable(401, "unauthorized"));
    }

    #[test]
    fn a_batch_reply_says_per_provider_what_a_single_grant_would() {
        let reply: GrantsReply = serde_json::from_value(json!({
            "grants": [
                {"provider": "xai-key", "accessToken": "k", "expiresAt": 1, "generation": 2},
                {"provider": "openrouter-key", "accessToken": "", "expiresAt": 1},
                {"provider": "not-asked", "accessToken": "x", "expiresAt": 1}
            ],
            "missing": [
                {"provider": "gemini-key", "error": "upstream", "message": "down"},
                {"provider": "deepseek-key", "error": "not_authorized"},
                {"provider": "some-later-provider", "error": "not_found"}
            ]
        }))
        .unwrap();
        let out = reply.outcomes(&[
            VaultProvider::XaiKey,
            VaultProvider::OpenrouterKey,
            VaultProvider::GeminiKey,
            VaultProvider::DeepseekKey,
        ]);
        assert!(
            matches!(out.get(&VaultProvider::XaiKey), Some(GrantOutcome::Granted(g)) if g.generation == 2)
        );
        assert!(
            !out.contains_key(&VaultProvider::OpenrouterKey),
            "an empty grant is no grant"
        );
        assert!(matches!(
            out.get(&VaultProvider::GeminiKey),
            Some(GrantOutcome::Unavailable(_))
        ));
        assert!(matches!(
            out.get(&VaultProvider::DeepseekKey),
            Some(GrantOutcome::NotConnected(_))
        ));
        assert_eq!(out.len(), 3);
    }

    #[test]
    fn agent_files_carry_the_token_never_a_refresh_token() {
        let mut g = grant("tok");
        g.settings = BTreeMap::from([
            ("oidc_issuer".to_string(), "https://auth.x.ai".to_string()),
            ("oidc_client_id".to_string(), "c1".to_string()),
            ("refresh_token".to_string(), "must-not-leak".to_string()),
        ]);
        assert_eq!(grok_scope(&g).as_deref(), Some("https://auth.x.ai::c1"));
        let entry = grok_entry(&g);
        assert_eq!(entry["key"], "tok");
        assert_eq!(entry["refresh_token"], "");
        g.settings = BTreeMap::from([
            (
                "api_server_url".to_string(),
                "https://server.codeium.com".to_string(),
            ),
            ("devin_api_key".to_string(), "old".to_string()),
        ]);
        let toml: toml::Table = devin_toml(&g).unwrap().parse().unwrap();
        assert_eq!(toml["windsurf_api_key"].as_str(), Some("tok"));
        assert!(toml.get("devin_api_key").is_none());
        let pi = pi_codex_entry(&g);
        assert_eq!(pi["refresh"], "");
        assert_eq!(pi["accountId"], "acct_1");
        assert_eq!(pi["expires"], g.expires_at);
    }

    #[test]
    fn pis_claude_entry_runs_on_the_claude_login_and_its_own_stays() {
        let dir = tempfile::tempdir().unwrap();
        let mut g = grant("claude-access");
        g.provider = "claude".into();
        write_pi_entry(dir.path(), PI_CLAUDE_ENTRY, pi_claude_entry(&g)).unwrap();
        let auth = read_json(&dir.path().join("auth.json")).unwrap();
        assert_eq!(
            auth["anthropic"],
            json!({"type": "oauth", "access": "claude-access", "refresh": "", "expires": g.expires_at})
        );
        assert!(pi_entry_managed(dir.path(), PI_CLAUDE_ENTRY));
        assert!(!pi_native(dir.path()), "only the broker's entry is there");
        // Pi's own Claude login (a real refresh token) is never replaced.
        std::fs::write(
            dir.path().join("auth.json"),
            r#"{"anthropic":{"type":"oauth","access":"own","refresh":"rt","expires":1}}"#,
        )
        .unwrap();
        assert!(pi_entry_native(dir.path(), PI_CLAUDE_ENTRY));
        write_pi_entry(dir.path(), PI_CLAUDE_ENTRY, pi_claude_entry(&g)).unwrap();
        clear_pi_entry(dir.path(), PI_CLAUDE_ENTRY);
        let auth = read_json(&dir.path().join("auth.json")).unwrap();
        assert_eq!(auth["anthropic"]["access"], "own");
        assert!(pi_native(dir.path()));
        // The lock directory is gone once written.
        assert!(!dir.path().join("auth.json.lock").exists());
    }

    #[test]
    fn hermes_records_carry_no_refresh_token_and_a_fresh_last_refresh() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        std::fs::write(
            home.join("auth.json"),
            r#"{"version":1,"active_provider":"nous","providers":{"anthropic":{"tokens":{"access_token":"own","refresh_token":"own-rt"}}},"credential_pool":{"openrouter":[{"id":"x","auth_type":"api_key","source":"env:OPENROUTER_API_KEY"}]}}"#,
        )
        .unwrap();
        let now = chrono::Utc::now();
        // ChatGPT, from the codex grant.
        write_hermes(home, VaultProvider::Codex, &grant("chatgpt-at"), now).unwrap();
        // xAI: lifetime, endpoint and id token from the grant's settings.
        let mut xai = grant("xai-at");
        xai.settings = BTreeMap::from([
            ("token_endpoint".to_string(), "https://auth.x.ai/oauth2/token".to_string()),
            ("id_token".to_string(), "i.d.t".to_string()),
        ]);
        write_hermes(home, VaultProvider::XaiOauth, &xai, now).unwrap();
        // Nous: the record's non-token fields from the settings.
        let mut nous = grant("nous-at");
        nous.settings = BTreeMap::from([
            ("portal_base_url".to_string(), "https://portal.nousresearch.com".to_string()),
            ("inference_base_url".to_string(), "https://inference-api.nousresearch.com/v1".to_string()),
            ("client_id".to_string(), "hermes-cli".to_string()),
            ("scope".to_string(), "inference:invoke".to_string()),
            ("refresh_token".to_string(), "must-not-leak".to_string()),
        ]);
        write_hermes(home, VaultProvider::Nous, &nous, now).unwrap();
        let auth = read_json(&home.join("auth.json")).unwrap();
        let providers = &auth["providers"];
        let iso = now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        assert_eq!(providers["openai-codex"]["tokens"]["access_token"], "chatgpt-at");
        assert_eq!(providers["openai-codex"]["tokens"]["refresh_token"], "");
        assert_eq!(providers["openai-codex"]["tokens"]["account_id"], "acct_1");
        assert_eq!(providers["openai-codex"]["last_refresh"], iso);
        assert_eq!(providers["openai-codex"]["auth_mode"], "chatgpt");
        assert_eq!(providers["xai-oauth"]["tokens"]["refresh_token"], "");
        assert_eq!(providers["xai-oauth"]["tokens"]["id_token"], "i.d.t");
        assert_eq!(
            providers["xai-oauth"]["discovery"]["token_endpoint"],
            "https://auth.x.ai/oauth2/token"
        );
        assert!(providers["xai-oauth"]["tokens"]["expires_in"].as_i64().unwrap() > 3000);
        assert_eq!(providers["nous"]["access_token"], "nous-at");
        assert_eq!(providers["nous"]["refresh_token"], "");
        assert_eq!(providers["nous"]["client_id"], "hermes-cli");
        assert_eq!(providers["nous"]["portal_base_url"], "https://portal.nousresearch.com");
        assert!(!auth.to_string().contains("must-not-leak"));
        // Everything of Hermes' own stays.
        assert_eq!(providers["anthropic"]["tokens"]["refresh_token"], "own-rt");
        // Its active provider named nothing it held: the first login written
        // is the one Hermes runs on (without one it says none is connected).
        assert_eq!(auth["active_provider"], "openai-codex");
        assert_eq!(auth["credential_pool"]["openrouter"][0]["id"], "x");
        assert_eq!(auth["updated_at"], iso);
        assert!(hermes_record_managed(home, VaultProvider::Nous));
        // The same token again: no write (Hermes keeps what it adopted).
        let before = std::fs::read(home.join("auth.json")).unwrap();
        write_hermes(home, VaultProvider::Codex, &grant("chatgpt-at"), now + chrono::Duration::minutes(5)).unwrap();
        assert_eq!(std::fs::read(home.join("auth.json")).unwrap(), before);
        // A native record (a real refresh token) is never replaced, and
        // counts as Hermes' own login.
        assert!(hermes_native(home));
        clear_hermes(home, VaultProvider::Codex);
        clear_hermes(home, VaultProvider::XaiOauth);
        clear_hermes(home, VaultProvider::Nous);
        let auth = read_json(&home.join("auth.json")).unwrap();
        assert!(auth["providers"].get("openai-codex").is_none());
        assert!(auth["providers"].get("nous").is_none());
        assert!(auth.get("active_provider").is_none(), "it named a login now gone");
        assert_eq!(auth["providers"]["anthropic"]["tokens"]["access_token"], "own");
        // Written under Hermes' own lock file.
        assert!(home.join("auth.lock").exists());
    }

    #[test]
    fn a_provider_hermes_was_set_to_stays_its_choice() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("auth.json"),
            r#"{"active_provider":"anthropic","providers":{"anthropic":{"tokens":{"access_token":"own","refresh_token":"own-rt"}}}}"#,
        )
        .unwrap();
        write_hermes(dir.path(), VaultProvider::Codex, &grant("chatgpt-at"), chrono::Utc::now()).unwrap();
        let auth = read_json(&dir.path().join("auth.json")).unwrap();
        assert_eq!(auth["active_provider"], "anthropic");
        assert_eq!(auth["providers"]["openai-codex"]["tokens"]["access_token"], "chatgpt-at");
    }

    #[test]
    fn an_env_key_hermes_pooled_isnt_its_own_login() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("auth.json"),
            r#"{"credential_pool":{"openrouter":[{"auth_type":"api_key","source":"env:OPENROUTER_API_KEY"}]}}"#,
        )
        .unwrap();
        assert!(!hermes_native(dir.path()));
        std::fs::write(
            dir.path().join("auth.json"),
            r#"{"credential_pool":{"openrouter":[{"auth_type":"api_key","source":"manual"}]}}"#,
        )
        .unwrap();
        assert!(hermes_native(dir.path()));
    }

    #[test]
    fn antigravity_runs_on_an_access_token_file_with_empty_refresh_fields() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let mut g = grant("ya29.token");
        g.expires_at = chrono::DateTime::parse_from_rfc3339("2026-11-18T13:04:05Z")
            .unwrap()
            .timestamp_millis();
        g.settings = BTreeMap::from([
            ("client_id".to_string(), "1071.apps.googleusercontent.com".to_string()),
            ("project_id".to_string(), "proj-1".to_string()),
            ("scopes".to_string(), "https://www.googleapis.com/auth/cloud-platform openid".to_string()),
        ]);
        write_antigravity_login(home, &g, false).unwrap();
        let token = read_json(&antigravity_token_file(home)).unwrap();
        assert_eq!(
            token,
            json!({
                "token": "ya29.token",
                "expiry": "2026-11-18T13:04:05",
                "refresh_token": "",
                "client_id": "1071.apps.googleusercontent.com",
                "client_secret": "",
                "scopes": ["https://www.googleapis.com/auth/cloud-platform", "openid"],
                "project_id": "proj-1",
            })
        );
        let oauth_token = read_json(&antigravity_oauth_token_file(home)).unwrap();
        assert_eq!(
            oauth_token,
            json!({
                "token": {
                    "access_token": "ya29.token",
                    "token_type": "Bearer",
                    "expiry": "2026-11-18T13:04:05Z",
                },
                "auth_method": "consumer",
            })
        );
        write_antigravity_login(home, &g, true).unwrap();
        let proxied_token = read_json(&antigravity_token_file(home)).unwrap();
        assert_eq!(proxied_token["expiry"], antigravity_proxy::PROXIED_TOKEN_EXPIRY);
        assert_eq!(proxied_token["token"], "ya29.token");
        assert_eq!(proxied_token["refresh_token"], "");
        let proxied_oauth = read_json(&antigravity_oauth_token_file(home)).unwrap();
        assert_eq!(
            proxied_oauth["token"]["expiry"],
            antigravity_proxy::PROXIED_TOKEN_EXPIRY_RFC3339
        );
        assert_eq!(proxied_oauth["token"]["access_token"], "ya29.token");
        assert_eq!(proxied_oauth["auth_method"], "consumer");
        assert!(proxied_oauth["token"].get("refresh_token").is_none());
        let settings = read_json(&home.join("antigravity-acp/settings.json")).unwrap();
        assert_eq!(settings["auth"]["type"], "oauth-personal");
        // The broker's files aren't Antigravity's own login.
        assert!(!antigravity_token_exists(home));
        clear_antigravity_login(home);
        assert!(!antigravity_token_file(home).exists());
        assert!(!antigravity_oauth_token_file(home).exists());
        // Antigravity's own legacy file (with a refresh token) stays, and is used.
        std::fs::write(antigravity_token_file(home), r#"{"refresh_token":"1//own"}"#).unwrap();
        assert!(antigravity_token_exists(home));
        write_antigravity_login(home, &g, true).unwrap();
        clear_antigravity_login(home);
        assert_eq!(read_json(&antigravity_token_file(home)).unwrap()["refresh_token"], "1//own");
        // Antigravity's own unified CLI file (with nested refresh_token) also stays.
        std::fs::remove_file(antigravity_token_file(home)).unwrap();
        std::fs::write(
            antigravity_oauth_token_file(home),
            r#"{"token":{"access_token":"ya29.own","refresh_token":"1//own-cli"},"auth_method":"consumer"}"#,
        )
        .unwrap();
        assert!(antigravity_token_exists(home));
        write_antigravity_login(home, &g, true).unwrap();
        clear_antigravity_login(home);
        assert_eq!(
            read_json(&antigravity_oauth_token_file(home)).unwrap()["token"]["refresh_token"],
            "1//own-cli"
        );
    }

    #[test]
    fn muse_gets_its_document_without_refresh_tokens_and_keeps_its_own() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("config/muse/auth.json");
        let record = dir.path().join("data/cloud/muse-auth.sha256");
        let mut g = grant("meta-at");
        g.settings = BTreeMap::from([(
            "document".to_string(),
            json!({"schema_version": 3, "providers": {"meta": {
                "mechanism": "oauth", "access_token": "meta-at", "refresh_token": "leak", "expires_at": 1_900_000_000
            }}})
            .to_string(),
        )]);
        write_muse(&file, &record, &g).unwrap();
        let written = read_json(&file).unwrap();
        assert_eq!(written["providers"]["meta"]["access_token"], "meta-at");
        assert_eq!(written["providers"]["meta"]["refresh_token"], "");
        assert_eq!(written["schema_version"], 3);
        assert!(!muse_native(&file, &record));
        clear_muse(&file, &record);
        assert!(!file.exists() && !record.exists());
        // A login Muse wrote itself stays.
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, r#"{"providers":{"meta":{"access_token":"own"}}}"#).unwrap();
        assert!(muse_native(&file, &record));
        write_muse(&file, &record, &g).unwrap();
        clear_muse(&file, &record);
        assert_eq!(read_json(&file).unwrap()["providers"]["meta"]["access_token"], "own");
        g.settings.insert("document".into(), "[1]".into());
        assert!(muse_document(&g).is_err());
    }

    #[test]
    fn opencode_takes_chatgpt_with_no_refresh_token_and_copilot_as_it_keeps_it() {
        let chatgpt = opencode_value(VaultProvider::Codex, &grant("at")).unwrap();
        assert_eq!(chatgpt["type"], "oauth");
        assert_eq!(chatgpt["refresh"], "");
        assert_eq!(chatgpt["access"], "at");
        assert_eq!(chatgpt["metadata"]["accountID"], "acct_1");
        let copilot = opencode_value(VaultProvider::Copilot, &grant("gho_token")).unwrap();
        assert_eq!(
            copilot,
            json!({"type": "oauth", "refresh": "gho_token", "access": "gho_token", "expires": 0})
        );
        assert!(opencode_value(VaultProvider::Grok, &grant("x")).is_none());
    }

    #[test]
    fn grants_that_must_outlive_an_agents_own_refresh_are_asked_for_ahead() {
        assert_eq!(run_min_validity(VaultProvider::Antigravity), ANTIGRAVITY_MIN_LIFE);
        assert_eq!(run_min_validity(VaultProvider::XaiOauth), XAI_OAUTH_AHEAD);
        assert_eq!(run_min_validity(VaultProvider::Codex), GRANT_SLACK);
        assert!(refresh_ahead(VaultProvider::XaiOauth) > Duration::from_secs(3600));
        assert_eq!(refresh_ahead(VaultProvider::Grok), REFRESH_AHEAD);
    }

    #[test]
    fn an_antigravity_settings_file_that_isnt_json_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("antigravity-acp/settings.json");
        write_antigravity_key_method(dir.path()).unwrap();
        assert_eq!(read_json(&file).unwrap()["auth"]["type"], "gemini-api-key");
        std::fs::write(&file, "{ // hjson\n auth: {type: oauth-personal} }").unwrap();
        write_antigravity_key_method(dir.path()).unwrap();
        assert!(std::fs::read_to_string(&file).unwrap().contains("hjson"));
    }

    #[test]
    fn grant_debug_never_prints_secrets() {
        let printed = format!("{:?}", grant("super-secret-access"));
        assert!(!printed.contains("super-secret-access"));
        assert!(!printed.contains("h.p.s"));
    }

    #[test]
    fn repos_dedupe_filter_and_sort_newest_first() {
        let repo = |name: &str, pushed_at: i64| GithubRepo {
            full_name: name.into(),
            clone_url: format!("https://github.com/{name}.git"),
            default_branch: "main".into(),
            private: false,
            description: None,
            pushed_at,
            ..Default::default()
        };
        let repos = vec![
            repo("acme/old", 1),
            repo("acme/New-App", 30),
            repo("other/thing", 20),
            repo("acme/new-app", 30),
        ];
        let all = filter_and_sort_repos(repos.clone(), None);
        assert_eq!(
            all.iter().map(|r| r.full_name.as_str()).collect::<Vec<_>>(),
            ["acme/New-App", "other/thing", "acme/old"]
        );
        let filtered = filter_and_sort_repos(repos, Some(" ACME/ "));
        assert_eq!(
            filtered
                .iter()
                .map(|r| r.full_name.as_str())
                .collect::<Vec<_>>(),
            ["acme/New-App", "acme/old"]
        );
    }

    #[test]
    fn github_repo_parsing_tolerates_missing_fields() {
        let parsed = parse_github_repo(&json!({
            "full_name": "acme/app",
            "private": true,
            "description": "",
            "pushed_at": "2026-09-01T00:00:00Z",
        }))
        .unwrap();
        assert_eq!(parsed.clone_url, "https://github.com/acme/app.git");
        assert_eq!(parsed.default_branch, "main");
        assert!(parsed.private);
        assert_eq!(parsed.description, None);
        assert!(parsed.pushed_at > 0);
        assert!(parse_github_repo(&json!({"name": "x"})).is_none());
    }

    #[test]
    fn config_seed_pins_file_credentials_and_keeps_the_users_config() {
        let dir = tempfile::tempdir().unwrap();
        let user = dir.path().join("user-config.toml");
        std::fs::write(
            &user,
            "model = \"gpt-6\"\n[mcp_servers.x]\ncommand = \"y\"\n",
        )
        .unwrap();
        let home = dir.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        seed_codex_config(&home, &user);
        let seeded = std::fs::read_to_string(home.join("config.toml")).unwrap();
        assert!(seeded.starts_with("cli_auth_credentials_store = \"file\"\n"));
        assert!(seeded.contains("[mcp_servers.x]"));
        assert!(seeded.parse::<toml::Table>().is_ok(), "{seeded}");
        // Kept afterwards (never clobbered).
        std::fs::write(home.join("config.toml"), "model = \"mine\"\n").unwrap();
        seed_codex_config(&home, &user);
        assert_eq!(
            std::fs::read_to_string(home.join("config.toml")).unwrap(),
            "model = \"mine\"\n"
        );
    }
}
