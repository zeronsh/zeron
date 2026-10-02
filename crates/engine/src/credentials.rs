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
//!   for the installation on the session repository's owner (the user's own
//!   GitHub token never leaves the vault) — becomes `GH_TOKEN` +
//!   `GITHUB_TOKEN`, a git credential store at
//!   `{data_dir}/cloud/git-credentials` (0600) wired once via
//!   `git config --global credential.https://github.com.helper`, and
//!   `~/.config/gh/hosts.yml` for interactive terminals (only when absent or
//!   already Zeron-managed). Refreshed in the background ~10 min before expiry.
//!   Pushes and pull requests therefore act as the App.
//!
//! Grants are cached per provider until 5 min before expiry; a network failure
//! falls back to a cached, still-unexpired grant. A 4xx (`not_authorized`,
//! `not_found`, `needs_reconnect`, revoked, kill switch) means "not connected":
//! the cache entry and any file it backed are dropped.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::{Duration, Instant};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64URL;
use serde::Deserialize;
use serde_json::{Value, json};

use zeron_proto::{GithubRepo, HarnessId, RunRequest, VaultProvider};

use crate::http_error::describe_http_error;
use crate::runner::{EdgeErrorBody, RunnerIdentity, write_private_atomic};

/// The one error every "no GitHub grant" path reports. The
/// `github_not_connected:` prefix is the machine-readable code (RPC errors
/// carry only a message); the rest is user-facing.
pub const GITHUB_NOT_CONNECTED: &str =
    "github_not_connected: GitHub isn't connected for this device — connect it in Settings → Cloud";

/// A Cloud run of a provider with no login anywhere on the device.
pub const CODEX_NOT_SIGNED_IN: &str = "Codex isn't signed in on Cloud. Sign in with ChatGPT or add an \
     OpenAI API key in Settings → Cloud, then send again.";
pub const CLAUDE_NOT_SIGNED_IN: &str = "Claude Code isn't signed in on Cloud. Sign in with Claude or \
     add an Anthropic API key in Settings → Cloud, then send again.";

/// A spawn uses a cached grant only while it has at least this much life left.
const GRANT_SLACK: Duration = Duration::from_secs(5 * 60);
/// The background refresher re-grants this far ahead of expiry.
const REFRESH_AHEAD: Duration = Duration::from_secs(10 * 60);
/// A "not connected" answer is reused this long before asking again.
const MISSING_TTL: Duration = Duration::from_secs(30);
/// Background refresher cadence (wall-clock re-evaluation; sandboxes stop
/// and resume, which tokio's monotonic timers do not see).
const REFRESH_TICK: Duration = Duration::from_secs(60);
/// How often the refresher looks for a GitHub grant that isn't there yet.
const GITHUB_PROBE_EVERY: Duration = Duration::from_secs(5 * 60);
/// Offline fallback only for grants that are still valid this much longer.
const OFFLINE_MIN_VALIDITY: Duration = Duration::from_secs(30);
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
    #[serde(default)]
    pub generation: i64,
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
    Granted(Grant),
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
}

impl BrokerConfig {
    pub fn detect(data_dir: &Path) -> Self {
        let home = crate::repos::home_dir();
        let env = |name: &str| std::env::var(name).ok().filter(|v| !v.trim().is_empty());
        let claude_dir = env("CLAUDE_CONFIG_DIR").map(PathBuf::from);
        Self {
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
    refresher: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

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
                refresher: Mutex::new(None),
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
            loop {
                let Some(inner) = weak.upgrade() else { return };
                CredentialBroker { inner }.refresh_due().await;
                tokio::time::sleep(REFRESH_TICK).await;
            }
        }));
    }

    pub fn shutdown(&self) {
        if let Some(task) = lock(&self.inner.refresher).take() {
            task.abort();
        }
    }

    // -- grants -------------------------------------------------------------

    /// A grant valid for at least 5 more minutes (cached or fresh).
    pub async fn grant(&self, provider: VaultProvider) -> GrantOutcome {
        self.grant_within(provider, GRANT_SLACK).await
    }

    async fn grant_within(&self, provider: VaultProvider, min_validity: Duration) -> GrantOutcome {
        if let Some(hit) = self.cached(provider, min_validity) {
            return hit;
        }
        let _gate = self.inner.grant_gate.lock().await;
        if let Some(hit) = self.cached(provider, min_validity) {
            return hit;
        }
        match self.fetch(provider).await {
            GrantOutcome::Granted(grant) => {
                tracing::info!(
                    provider = provider.as_str(),
                    generation = grant.generation,
                    expires_in_s = grant.remaining_ms() / 1000,
                    "credentials: vault grant"
                );
                lock(&self.inner.cache).insert(provider, Cached::Granted(grant.clone()));
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
                if let Some(Cached::Granted(grant)) = lock(&self.inner.cache).get(&provider)
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

    fn cached(&self, provider: VaultProvider, min_validity: Duration) -> Option<GrantOutcome> {
        match lock(&self.inner.cache).get(&provider)? {
            Cached::Granted(grant) if grant.remaining_ms() > min_validity.as_millis() as i64 => {
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
            Cached::Granted(grant) => Some(grant.clone()),
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
        if let Some(token) = self.github_token().await {
            request.env.insert("GH_TOKEN".into(), token.clone());
            request.env.insert("GITHUB_TOKEN".into(), token);
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
            _ => {}
        }
        Ok(())
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
        match self.grant(VaultProvider::Github).await {
            GrantOutcome::Granted(grant) => {
                self.sync_github_files(&grant).await;
                Some(grant.access_token)
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
                Err(reason)
            }
            GrantOutcome::Unavailable(reason) => Err(reason),
        }
    }

    /// The managed `CODEX_HOME` when a Codex credential is available.
    pub async fn codex_home(&self) -> Option<PathBuf> {
        let home = managed_codex_home(&self.inner.config.data_dir);
        let auth_file = home.join("auth.json");
        match self.grant(VaultProvider::Codex).await {
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
        match self.grant(VaultProvider::OpenaiKey).await {
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
        match self.grant(VaultProvider::Claude).await {
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
        match self.grant(VaultProvider::AnthropicKey).await {
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
                .grant_within(VaultProvider::Github, REFRESH_AHEAD)
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
                .grant_within(VaultProvider::Claude, REFRESH_AHEAD)
                .await
            {
                GrantOutcome::Granted(grant) => {
                    self.write_claude_credentials(&grant).await;
                }
                GrantOutcome::NotConnected(_) => self.clear_claude_credentials().await,
                GrantOutcome::Unavailable(_) => {}
            }
        }
        if let Some(grant) = self.cached_grant(VaultProvider::Codex)
            && grant.remaining_ms() <= REFRESH_AHEAD.as_millis() as i64
        {
            let home = managed_codex_home(&self.inner.config.data_dir);
            match self.grant_within(VaultProvider::Codex, REFRESH_AHEAD).await {
                GrantOutcome::Granted(grant) => {
                    self.write_codex_auth(
                        &home,
                        codex_chatgpt_auth_json(&grant, chrono::Utc::now()),
                    )
                    .await;
                }
                GrantOutcome::NotConnected(_) => {
                    let _files = self.inner.files.lock().await;
                    let _ = std::fs::remove_file(home.join("auth.json"));
                }
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
    async fn prepare(&self, harness: HarnessId, request: &mut RunRequest) -> Result<(), String> {
        CredentialBroker::prepare(self, harness, request).await
    }
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
            generation: 3,
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
