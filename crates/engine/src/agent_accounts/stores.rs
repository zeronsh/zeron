//! Credential stores of the agents beyond Claude Code / Codex / Cursor /
//! Antigravity: where each CLI keeps its login, how zeron reads (and, where
//! it's safe, swaps) it, and the sign-ins that run through the CLI itself.
//!
//! - **Grok** — `$GROK_HOME/auth.json` (default `~/.grok`, 0600, guarded by
//!   an `auth.json.lock` flock): a map of `"{issuer}::{client_id}"` → OIDC
//!   token set (`key` = access token, `refresh_token`, `expires_at`, and
//!   identity: `user_id`, `email`, names, `subscription_tier`). One live
//!   login; the CLI re-reads the file on every call, so a swap is live at
//!   once. Swap = rewrite the file (under grok's lock). Add account = `grok
//!   login --device-auth` against a throwaway `GROK_HOME`: a device code
//!   needs no loopback, so a remote login works without a tunnel.
//! - **Devin** — `$XDG_DATA_HOME/devin/credentials.toml` (default
//!   `~/.local/share/devin/`; older CLIs used `~/Library/Application
//!   Support/devin/`, `%APPDATA%\devin`): a long-lived `windsurf_api_key`
//!   plus server urls, no identity and no refresh token. Swap = rewrite the
//!   file (running `devin acp` processes keep the key they started with).
//!   Add account = the ACP `authenticate` `devin-browser` method with a
//!   throwaway `XDG_DATA_HOME`: Devin's own PKCE loopback, whose
//!   `redirect_uri` port is reported for remote tunnelling. The identity is
//!   learned from the usage probe and written back into the slot.
//! - **OpenCode** — `$XDG_DATA_HOME/opencode/auth.json` (default
//!   `~/.local/share/opencode/`): one entry PER model provider (`{type:
//!   "oauth", access, refresh, expires, accountId?}` or `{type: "api",
//!   key}`), re-read on every request. zeron manages the OAuth entries of
//!   `openai` (ChatGPT) and `github-copilot`; a swap rewrites that one entry
//!   and leaves every other provider byte-for-byte. OpenCode has no lock of
//!   its own. API-key entries aren't accounts and are left alone.
//!   Anthropic OAuth was removed from OpenCode upstream and is not offered.
//! - **Pi** — `$PI_CODING_AGENT_DIR/auth.json` (default `~/.pi/agent`,
//!   0600, guarded by proper-lockfile's `auth.json.lock` DIRECTORY): the
//!   same per-provider shape (`openai-codex`, `anthropic`, `github-copilot`),
//!   hot-reloaded by running agents. Swap = rewrite one entry under that
//!   lock. Add account = ChatGPT only (zeron's own loopback, see
//!   [`super::oauth`]); Claude and Copilot logins stay with pi's `/login` —
//!   minting Claude Code OAuth for a third-party agent is not something
//!   zeron should do, and pi's Copilot login is an internal token exchange.
//! - **Hermes** — `$HERMES_HOME/auth.json` (default `~/.hermes`): Hermes
//!   keeps EVERY account itself, in `credential_pool[provider]`, and picks
//!   (fill-first by `priority`, or round-robin) and refreshes them on its
//!   own. A running Hermes writes its in-memory pool back over the file, so
//!   a zeron reorder would be silently undone — zeron lists the pool (the
//!   active provider's first entry is "in use"), probes usage, and adds
//!   accounts through `hermes auth add` (device code; Hermes appends to its
//!   own pool under its own lock). It never rewrites the pool.
//!
//! Opaque tokens (Pi's Claude login, Copilot tokens) carry no identity: a
//! live entry is matched to its slot by token, else identified ONCE per token
//! with a read-only profile call (`/api/oauth/profile`, GitHub `/user`). A
//! Copilot login on a self-hosted GitHub Enterprise Server (a host zeron
//! never sends the token to) gets a local identity instead — its host and a
//! SHA-256 fingerprint of the token — so it still switches; its usage is
//! skipped.

use std::collections::HashSet;

use super::*;

// ── paths ───────────────────────────────────────────────────────────────────

fn env_dir(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
}

fn xdg_data_home() -> PathBuf {
    env_dir("XDG_DATA_HOME").unwrap_or_else(|| home_dir().join(".local").join("share"))
}

pub(super) fn default_grok_home() -> PathBuf {
    env_dir("GROK_HOME").unwrap_or_else(|| home_dir().join(".grok"))
}

/// Devin 3000.x reads `$XDG_DATA_HOME/devin/credentials.toml`; older builds
/// kept it under the platform config dir. The first existing one wins, else
/// the current location (where a new login lands).
pub(super) fn default_devin_credentials_file() -> PathBuf {
    let primary = xdg_data_home().join("devin").join("credentials.toml");
    if primary.exists() {
        return primary;
    }
    let mut fallbacks = Vec::new();
    if cfg!(target_os = "macos") {
        fallbacks.push(
            home_dir()
                .join("Library")
                .join("Application Support")
                .join("devin")
                .join("credentials.toml"),
        );
    }
    if cfg!(windows) {
        fallbacks.push(
            env_dir("APPDATA")
                .unwrap_or_else(|| home_dir().join("AppData").join("Roaming"))
                .join("devin")
                .join("credentials.toml"),
        );
    }
    fallbacks
        .into_iter()
        .find(|file| file.exists())
        .unwrap_or(primary)
}

pub(super) fn default_opencode_auth_file() -> PathBuf {
    xdg_data_home().join("opencode").join("auth.json")
}

pub(super) fn default_pi_agent_dir() -> PathBuf {
    env_dir("PI_CODING_AGENT_DIR").unwrap_or_else(|| home_dir().join(".pi").join("agent"))
}

pub(super) fn default_hermes_home() -> PathBuf {
    if let Some(home) = env_dir("HERMES_HOME") {
        return home;
    }
    if cfg!(windows)
        && let Some(local) = env_dir("LOCALAPPDATA")
        && local.join("hermes").join("auth.json").exists()
    {
        return local.join("hermes");
    }
    home_dir().join(".hermes")
}

// ── naming ──────────────────────────────────────────────────────────────────

/// The CLI a user would run for `harness` (named in reasons and errors).
pub(super) fn cli_name(harness: HarnessId) -> &'static str {
    match harness {
        HarnessId::ClaudeCode => "claude",
        HarnessId::Codex => "codex",
        HarnessId::Cursor => "Cursor",
        HarnessId::Grok => "grok",
        HarnessId::Devin => "devin",
        HarnessId::Opencode => "opencode",
        HarnessId::Pi => "pi",
        HarnessId::Hermes => "hermes",
        HarnessId::Antigravity => "Antigravity",
        HarnessId::Mock => "mock",
    }
}

/// The vendor behind a per-provider store key, for usage reasons.
pub(super) fn upstream_vendor(store_key: &str) -> &'static str {
    match store_key {
        "openai" | "openai-codex" => "OpenAI",
        "anthropic" => "Anthropic",
        "github-copilot" | "copilot" => "GitHub",
        "nous" => "Nous",
        "xai-oauth" | "xai" => "xAI",
        _ => "the provider",
    }
}

/// The logins `hermes auth add` can run unattended (device codes).
pub(super) const HERMES_LOGINS: &[&str] = &["openai-codex", "nous"];

pub(super) fn unsupported_login(harness: HarnessId, provider: &str) -> EngineError {
    let reason = match (harness, provider) {
        (HarnessId::Pi, "anthropic") => {
            "Claude logins for Pi stay with pi's own /login — zeron only mints Claude Code \
             logins for Claude Code."
                .to_string()
        }
        _ => format!(
            "zeron can't add a {provider} login for {} — sign in with `{}` itself.",
            cli_name(harness),
            cli_name(harness)
        ),
    };
    EngineError::Other(reason)
}

/// OpenCode ignores `auth.json` while `OPENCODE_AUTH_CONTENT` is set.
pub(super) fn opencode_env_warning() -> Option<String> {
    std::env::var_os("OPENCODE_AUTH_CONTENT")
        .filter(|v| !v.is_empty())
        .map(|_| {
            "OPENCODE_AUTH_CONTENT is set, so OpenCode reads its logins from that variable — \
             switching accounts here won't change what it uses."
                .to_string()
        })
}

fn key_tail(secret: &str) -> String {
    let chars: Vec<char> = secret.chars().collect();
    chars[chars.len().saturating_sub(4)..].iter().collect()
}

fn hashed_key(secret: &str) -> String {
    let digest = Sha256::digest(secret.as_bytes());
    crate::repos::hex(&digest)[..12].to_string()
}

/// "SUBSCRIPTION_TIER_SUPER_GROK" / "super_grok" → "Super Grok".
fn title_words(raw: &str) -> Option<String> {
    let words: Vec<String> = raw
        .split(['_', '-', ' '])
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut chars = w.chars();
            match chars.next() {
                Some(first) => format!("{}{}", first.to_uppercase(), chars.as_str().to_lowercase()),
                None => String::new(),
            }
        })
        .collect();
    (!words.is_empty()).then(|| words.join(" "))
}

// ── Grok ────────────────────────────────────────────────────────────────────

/// `subscription_tier` → a plan chip ("SuperGrok Heavy"); `None` when absent.
pub(super) fn grok_plan(tier: Option<&str>) -> Option<String> {
    let tier = tier?;
    let stem = tier
        .strip_prefix("SUBSCRIPTION_TIER_")
        .or_else(|| tier.strip_prefix("TIER_"))
        .unwrap_or(tier);
    if matches!(
        stem.to_ascii_lowercase().as_str(),
        "" | "none" | "free" | "unspecified"
    ) {
        return None;
    }
    let label = title_words(stem)?;
    Some(label.replace("Super Grok", "SuperGrok"))
}

fn grok_identity(entry: &serde_json::Value) -> Option<String> {
    str_field(entry, "user_id")
        .or_else(|| str_field(entry, "principal_id"))
        .or_else(|| str_field(entry, "email"))
}

/// Grok's `auth.json` → the live login. The file can hold several issuers
/// (a legacy web login, an enterprise OIDC): prefer `auth.x.ai`, then an
/// entry that names its user. An identity-less entry is keyed by its stable
/// `issuer::client` map key — never the rotating access token.
pub(super) fn parse_grok_auth(auth: serde_json::Value) -> Option<Detected> {
    let (map_key, entry) = auth
        .as_object()?
        .iter()
        .filter(|(_, e)| str_field(e, "key").is_some())
        .max_by_key(|(map_key, e)| {
            let issuer = str_field(e, "oidc_issuer").unwrap_or_else(|| (*map_key).clone());
            (
                issuer.starts_with("https://auth.x.ai"),
                grok_identity(e).is_some(),
            )
        })?;
    let names: Vec<String> = ["first_name", "last_name"]
        .iter()
        .filter_map(|k| str_field(entry, k))
        .collect();
    let profile = SlotProfile {
        email: str_field(entry, "email")
            .or_else(|| grok_identity(entry))
            .unwrap_or_else(|| "Grok account".to_string()),
        display_name: (!names.is_empty()).then(|| names.join(" ")),
        organization: str_field(entry, "organization_name")
            .or_else(|| str_field(entry, "team_name")),
        plan: grok_plan(str_field(entry, "subscription_tier").as_deref()),
        auth_kind: AgentAuthKind::Oauth,
    };
    let account_key = grok_identity(entry).unwrap_or_else(|| format!("oidc:{map_key}"));
    // The slot holds THIS issuer's token set only, keyed by its map key: a
    // swap merges it back into the live file and leaves every other issuer's
    // entry (and its freshly refreshed tokens) alone.
    Some(Detected::known(account_key, profile, entry.clone()).keyed(map_key))
}

// ── Devin ───────────────────────────────────────────────────────────────────

/// `credentials.toml` as a JSON object, keys as the CLI wrote them (unknown
/// keys pass through, so a swap restores them).
pub(super) fn parse_devin_toml(text: &str) -> Option<serde_json::Value> {
    let table: toml::Table = toml::from_str(text).ok()?;
    let value = serde_json::to_value(table).ok()?;
    value
        .as_object()
        .is_some_and(|map| !map.is_empty())
        .then_some(value)
}

pub(super) fn devin_toml(credentials: &serde_json::Value) -> Result<String, EngineError> {
    toml::to_string(credentials)
        .map_err(|e| EngineError::Other(format!("serialize devin credentials: {e}")))
}

pub(super) fn devin_api_key(credentials: &serde_json::Value) -> Option<String> {
    str_field(credentials, "windsurf_api_key").or_else(|| str_field(credentials, "devin_api_key"))
}

/// The Devin login in `credentials`: keyed by a digest of its key (the file
/// holds no identity — the usage probe learns it).
pub(super) fn parse_devin_credentials(credentials: serde_json::Value) -> Option<Detected> {
    let key = devin_api_key(&credentials)?;
    let mut detected = Detected::known(
        format!("api-key:{}", hashed_key(&key)),
        SlotProfile {
            email: format!("Devin account ·…{}", key_tail(&key)),
            display_name: None,
            organization: None,
            plan: None,
            auth_kind: AgentAuthKind::Oauth,
        },
        credentials,
    );
    detected.identity_known = false;
    Some(detected)
}

// ── per-provider stores (OpenCode, Pi) ──────────────────────────────────────

/// The model provider behind a per-provider store entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Upstream {
    /// ChatGPT OAuth (the Codex client).
    OpenAi,
    /// Claude OAuth (the Claude Code client).
    Anthropic,
    /// GitHub Copilot (a GitHub OAuth token).
    Copilot,
}

/// The store keys zeron manages per agent.
pub(super) fn keyed_accounts(harness: HarnessId) -> &'static [(&'static str, Upstream)] {
    match harness {
        HarnessId::Opencode => &[
            ("openai", Upstream::OpenAi),
            ("github-copilot", Upstream::Copilot),
        ],
        HarnessId::Pi => &[
            ("openai-codex", Upstream::OpenAi),
            ("anthropic", Upstream::Anthropic),
            ("github-copilot", Upstream::Copilot),
        ],
        _ => &[],
    }
}

pub(super) fn upstream_of(harness: HarnessId, store_key: &str) -> Option<Upstream> {
    keyed_accounts(harness)
        .iter()
        .find(|(key, _)| *key == store_key)
        .map(|(_, upstream)| *upstream)
}

/// An OAuth entry zeron can use: `type: "oauth"` with an access token.
fn oauth_entry(entry: &serde_json::Value) -> bool {
    entry.get("type").and_then(|v| v.as_str()) == Some("oauth")
        && str_field(entry, "access").is_some()
}

/// A ChatGPT login entry's identity from its token claims: the access
/// token carries `https://api.openai.com/{auth,profile}` (an `id_token`,
/// when a fresh login still has one, carries `email` at top level).
pub(super) fn openai_detected(
    store_key: &str,
    entry: &serde_json::Value,
    id_token: Option<&str>,
) -> Option<Detected> {
    let access = jwt_claims(&str_field(entry, "access")?).unwrap_or_default();
    let id = id_token.and_then(jwt_claims).unwrap_or_default();
    let auth = access
        .get("https://api.openai.com/auth")
        .or_else(|| id.get("https://api.openai.com/auth"))
        .cloned()
        .unwrap_or_default();
    let email = access
        .get("https://api.openai.com/profile")
        .and_then(|p| str_field(p, "email"))
        .or_else(|| str_field(&id, "email"));
    let account_id =
        str_field(entry, "accountId").or_else(|| str_field(&auth, "chatgpt_account_id"));
    let identity = account_id.clone().or_else(|| email.clone())?;
    Some(
        Detected::known(
            format!("{store_key}:{identity}"),
            SlotProfile {
                email: email.unwrap_or_else(|| "ChatGPT account".to_string()),
                display_name: str_field(&id, "name"),
                organization: None,
                plan: codex_plan(str_field(&auth, "chatgpt_plan_type").as_deref())
                    .or_else(|| Some("ChatGPT".to_string())),
                auth_kind: AgentAuthKind::Oauth,
            },
            entry.clone(),
        )
        .keyed(store_key),
    )
}

/// The secret an opaque entry is matched by: Copilot's GitHub token lives
/// in `refresh` (both agents), Claude's rotating pair in `refresh`.
fn opaque_secret(entry: &serde_json::Value) -> Option<String> {
    str_field(entry, "refresh").or_else(|| str_field(entry, "access"))
}

/// A Copilot login on a GitHub Enterprise host zeron doesn't send tokens
/// to (self-hosted GHES): identified without the network — labelled by its
/// host, keyed by a SHA-256 fingerprint of its GitHub token (never the token
/// itself) — so it snapshots into a slot, switches and restores like any
/// other. Its usage stays skipped ([`copilot_api_base`] is `None`).
pub(super) fn local_enterprise_identity(
    store_key: &str,
    github_token: &str,
    entry: &serde_json::Value,
) -> (String, SlotProfile) {
    let host = str_field(entry, "enterpriseUrl").and_then(|raw| {
        let raw = raw.trim();
        let candidate = match raw.contains("://") {
            true => raw.to_string(),
            false => format!("https://{raw}"),
        };
        reqwest::Url::parse(&candidate)
            .ok()?
            .host_str()
            .map(str::to_ascii_lowercase)
    });
    let email = match host {
        Some(host) => format!("GitHub Enterprise account · {host}"),
        None => "GitHub Enterprise account".to_string(),
    };
    (
        format!("{store_key}:enterprise:{}", hashed_key(github_token)),
        SlotProfile {
            email,
            display_name: None,
            organization: None,
            plan: Some("GitHub Copilot".to_string()),
            auth_kind: AgentAuthKind::Oauth,
        },
    )
}

fn unresolved(store_key: &str, upstream: Upstream) -> Detected {
    let (email, plan) = match upstream {
        Upstream::Anthropic => ("Claude account", "Claude"),
        Upstream::Copilot => ("GitHub account", "GitHub Copilot"),
        Upstream::OpenAi => ("ChatGPT account", "ChatGPT"),
    };
    Detected {
        account_key: format!("{store_key}:unidentified"),
        profile: SlotProfile {
            email: email.to_string(),
            display_name: None,
            organization: None,
            plan: Some(plan.to_string()),
            auth_kind: AgentAuthKind::Oauth,
        },
        credentials: None,
        claude_config: None,
        store_key: Some(store_key.to_string()),
        identity_known: false,
    }
}

// ── locks ───────────────────────────────────────────────────────────────────

const LOCK_WAIT: Duration = Duration::from_secs(5);

/// An exclusive lock on a sidecar lock file — `flock` on unix,
/// `LockFileEx` on Windows (std's `File::try_lock`), the primitives the
/// Rust CLIs' own lock crates use. Grok's `auth.json.lock` is the CLI's
/// convention; OpenCode has none, so it gets a zeron-side one. Released
/// when dropped (the handle closes).
pub(super) struct FileLock(#[allow(dead_code)] std::fs::File);

impl FileLock {
    pub(super) fn acquire(path: &Path) -> Result<Self, EngineError> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let mut options = std::fs::OpenOptions::new();
        options.create(true).truncate(false).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(path)?;
        let deadline = Instant::now() + LOCK_WAIT;
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(Self(file)),
                Err(std::fs::TryLockError::WouldBlock) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(25));
                }
                Err(std::fs::TryLockError::WouldBlock) => {
                    return Err(EngineError::Other(format!(
                        "{} is locked by the agent — try again in a moment.",
                        path.display()
                    )));
                }
                Err(std::fs::TryLockError::Error(err)) => return Err(err.into()),
            }
        }
    }
}

/// proper-lockfile's lock: a `<file>.lock` DIRECTORY (Pi). A lock older than
/// proper-lockfile's own staleness window (10s, refreshed while held) is a
/// crashed holder's and is taken over.
pub(super) struct DirLock(PathBuf);

impl DirLock {
    pub(super) fn acquire(path: &Path) -> Result<Self, EngineError> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let deadline = Instant::now() + LOCK_WAIT;
        loop {
            match std::fs::create_dir(path) {
                Ok(()) => return Ok(Self(path.to_path_buf())),
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
                        return Err(EngineError::Other(format!(
                            "{} is locked by the agent — try again in a moment.",
                            path.display()
                        )));
                    }
                    std::thread::sleep(Duration::from_millis(25));
                }
                Err(err) => return Err(err.into()),
            }
        }
    }
}

impl Drop for DirLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir(&self.0);
    }
}

fn lock_path(file: &Path) -> PathBuf {
    let mut name = file.file_name().unwrap_or_default().to_os_string();
    name.push(".lock");
    file.with_file_name(name)
}

/// How a JSON credential store is guarded while zeron rewrites one entry.
pub(super) enum StoreLock {
    /// A `flock`/`LockFileEx` on this sidecar file.
    File(PathBuf),
    /// proper-lockfile's lock directory.
    Dir(PathBuf),
}

/// Rounds of compare-before-write before a store that keeps changing wins.
const MERGE_ATTEMPTS: usize = 3;

/// Replace (or, with `entry` = `None`, remove) ONE entry of a JSON-object
/// credential store, keeping every other key as it is, under `lock` — then compare-before-write: the file is read
/// again right before the rename, and a change since the first read (an
/// agent that writes without taking the lock — OpenCode takes none — or a
/// CLI mid-refresh) restarts the cycle from the new contents instead of
/// clobbering it. The replacement is staged (written and synced to its temp
/// file) before that second read, so the unguarded window is only the
/// comparison plus the rename. RESIDUAL RISK: a writer that ignores the lock
/// and lands inside that window is overwritten; the entry it would have
/// changed is re-detected (and re-snapshotted) on the next list.
///
/// An existing store that doesn't parse is never overwritten — writing only
/// our entry would wipe the user's other logins.
pub(super) fn merge_json_entry(
    file: &Path,
    key: &str,
    entry: Option<&serde_json::Value>,
    lock: StoreLock,
) -> Result<(), EngineError> {
    // Held for its Drop (the unlock) only.
    #[allow(dead_code)]
    enum Guard {
        File(FileLock),
        Dir(DirLock),
    }
    let _guard = match lock {
        StoreLock::File(path) => Guard::File(FileLock::acquire(&path)?),
        StoreLock::Dir(path) => Guard::Dir(DirLock::acquire(&path)?),
    };
    let read = || match std::fs::read(file) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(EngineError::from(err)),
    };
    for _ in 0..MERGE_ATTEMPTS {
        let before = read()?;
        if before.is_none() && entry.is_none() {
            return Ok(()); // nothing to remove
        }
        let mut store = match &before {
            Some(bytes) => serde_json::from_slice::<serde_json::Value>(bytes)
                .ok()
                .filter(serde_json::Value::is_object)
                .ok_or_else(|| {
                    EngineError::Other(format!(
                        "{} exists but could not be parsed — not switching to avoid wiping it.",
                        file.display()
                    ))
                })?,
            None => serde_json::json!({}),
        };
        if let Some(map) = store.as_object_mut() {
            match entry {
                Some(entry) => {
                    map.insert(key.to_string(), entry.clone());
                }
                None => {
                    if map.remove(key).is_none() {
                        return Ok(()); // already gone
                    }
                }
            }
        }
        let json = serde_json::to_string_pretty(&store)
            .map_err(|e| EngineError::Other(format!("serialize {}: {e}", file.display())))?;
        if let Some(dir) = file.parent() {
            std::fs::create_dir_all(dir)?;
        }
        // Stage (create, write, fsync) BEFORE the final comparison, so the
        // unguarded window is just compare + rename, not the temp-file I/O.
        let staged = stage_file_atomic(file, json.as_bytes(), true)?;
        if read()? != before {
            drop(staged); // deletes the temp file
            std::thread::sleep(Duration::from_millis(50));
            continue;
        }
        staged
            .persist(file)
            .map_err(|e| EngineError::from(e.error))?;
        return Ok(());
    }
    Err(EngineError::Other(format!(
        "{} kept changing while zeron was switching — try again in a moment.",
        file.display()
    )))
}

// ── credential-defined endpoints ────────────────────────────────────────────

/// Which hosts a credential-supplied endpoint may name before zeron sends
/// that credential's secret there. A tampered or misconfigured store must
/// never route a refresh token or API key to an arbitrary server.
#[derive(Debug, Clone, Copy)]
pub(super) enum HostPolicy {
    /// The vendor's own domains (and their subdomains).
    Domains(&'static [&'static str]),
    /// GitHub Enterprise Cloud with data residency: a single-label subdomain
    /// of `ghe.com` (Copilot's `enterpriseUrl`). Self-hosted GHES hosts are
    /// deliberately NOT accepted — a credential field alone never decides
    /// where a GitHub token is sent. Such a login is identified locally
    /// instead (see [`local_enterprise_identity`]): it lists and switches,
    /// without usage.
    GheCom,
}

/// Grok OIDC issuers (`oidc_issuer`, refresh only).
pub(super) const GROK_ISSUERS: HostPolicy = HostPolicy::Domains(&["x.ai"]);
/// Devin / Windsurf / Codeium domains: its API servers and sign-in pages.
pub(super) const DEVIN_DOMAINS: &[&str] = &["codeium.com", "windsurf.com", "devin.ai"];
/// Devin / Windsurf API servers (`api_server_url`).
pub(super) const DEVIN_SERVERS: HostPolicy = HostPolicy::Domains(DEVIN_DOMAINS);
/// The Nous portal (`portal_base_url` in Hermes' pool).
pub(super) const NOUS_PORTALS: HostPolicy = HostPolicy::Domains(&["nousresearch.com"]);

/// `raw` as a base url safe to send a secret to: `https`, a DNS host the
/// policy allows, no userinfo, port, query or fragment, and no path beyond
/// `/`. A bare host (how OpenCode and Pi store `enterpriseUrl`) reads as
/// `https://host` under [`HostPolicy::GheCom`]. `allow_loopback`
/// additionally admits `http://127.0.0.1|localhost:<port>` — tests' mock
/// servers only, never production. `None` = don't send anything.
pub(super) fn trusted_base(
    raw: &str,
    policy: HostPolicy,
    allow_loopback: bool,
) -> Option<reqwest::Url> {
    let raw = raw.trim();
    let candidate = match (raw.contains("://"), policy) {
        (true, _) => raw.to_string(),
        (false, HostPolicy::GheCom) => format!("https://{}", raw.trim_end_matches('/')),
        (false, _) => return None,
    };
    let url = reqwest::Url::parse(&candidate).ok()?;
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || !matches!(url.path(), "" | "/")
    {
        return None;
    }
    if allow_loopback
        && url.scheme() == "http"
        && matches!(url.host_str(), Some("127.0.0.1" | "localhost"))
    {
        return Some(url);
    }
    if url.scheme() != "https" || url.port().is_some() {
        return None;
    }
    // `domain()` is `None` for IP literals: a secret never goes to a bare IP.
    let host = url.domain()?.to_ascii_lowercase();
    let allowed = match policy {
        HostPolicy::Domains(domains) => host_in(&host, domains),
        HostPolicy::GheCom => host
            .strip_suffix(".ghe.com")
            .is_some_and(|tenant| !tenant.is_empty() && !tenant.contains('.')),
    };
    allowed.then_some(url)
}

/// `host` is one of `domains` or a subdomain of one — on a label boundary,
/// so `evildevin.ai` and `devin.ai.evil.example` are not `devin.ai`.
pub(super) fn host_in(host: &str, domains: &[&str]) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    domains
        .iter()
        .any(|d| host == *d || host.ends_with(&format!(".{d}")))
}

/// `url` is an `https` page on one of `domains` (or a subdomain), with no
/// userinfo and no explicit port: a sign-in page a CLI printed that is safe
/// to open on the requesting device.
pub(super) fn trusted_page(url: &str, domains: &[&str]) -> bool {
    reqwest::Url::parse(url).is_ok_and(|parsed| {
        parsed.scheme() == "https"
            && parsed.username().is_empty()
            && parsed.password().is_none()
            && parsed.port().is_none()
            // `domain()` is `None` for IP literals.
            && parsed.domain().is_some_and(|host| host_in(host, domains))
    })
}

/// The REST root for a Copilot login: GitHub's, or `https://api.<tenant>.ghe.com`
/// for a GitHub Enterprise Cloud (`*.ghe.com`) `enterpriseUrl`. Any other
/// enterprise host — self-hosted GHES included, whose REST root would be
/// `https://<host>/api/v3` — is `None`: zeron skips identity/usage probes
/// rather than send the token to a host the credential file alone names
/// (the login gets a local identity, [`local_enterprise_identity`]).
pub(super) fn copilot_api_base(
    entry: &serde_json::Value,
    github_api: &str,
    allow_loopback: bool,
) -> Option<String> {
    match str_field(entry, "enterpriseUrl") {
        None => Some(github_api.trim_end_matches('/').to_string()),
        Some(raw) => {
            let url = trusted_base(&raw, HostPolicy::GheCom, allow_loopback)?;
            Some(match url.scheme() {
                "https" => format!("https://api.{}", url.host_str()?),
                _ => url.as_str().trim_end_matches('/').to_string(),
            })
        }
    }
}

// ── service ─────────────────────────────────────────────────────────────────

impl AgentAccounts {
    fn grok_auth_file(&self) -> PathBuf {
        self.inner.config.grok_home.join("auth.json")
    }

    pub(super) fn detect_grok(&self) -> Option<Detected> {
        read_json(&self.grok_auth_file()).and_then(parse_grok_auth)
    }

    pub(super) fn detect_devin(&self) -> Option<Detected> {
        let text = std::fs::read_to_string(&self.inner.config.devin_credentials_file).ok()?;
        parse_devin_toml(&text).and_then(parse_devin_credentials)
    }

    /// Put one issuer's token set (a Grok slot) into the live `auth.json`
    /// under the CLI's own `auth.json.lock`, leaving other issuers' entries
    /// untouched.
    pub(super) fn write_grok_entry(
        &self,
        map_key: Option<&str>,
        entry: &serde_json::Value,
    ) -> Result<(), EngineError> {
        let map_key = map_key
            .ok_or_else(|| EngineError::Other("That saved Grok login names no issuer.".into()))?;
        let file = self.grok_auth_file();
        merge_json_entry(
            &file,
            map_key,
            Some(entry),
            StoreLock::File(lock_path(&file)),
        )
    }

    /// Sign grok out of one issuer's login: drop that entry from the live
    /// `auth.json` (under grok's lock), leaving any other issuer's alone.
    pub(super) fn remove_grok_entry(&self, map_key: &str) -> Result<(), EngineError> {
        let file = self.grok_auth_file();
        merge_json_entry(&file, map_key, None, StoreLock::File(lock_path(&file)))
    }

    pub(super) fn write_devin_credentials(
        &self,
        credentials: &serde_json::Value,
    ) -> Result<(), EngineError> {
        let file = &self.inner.config.devin_credentials_file;
        if let Some(dir) = file.parent() {
            std::fs::create_dir_all(dir)?;
        }
        write_file_atomic(file, devin_toml(credentials)?.as_bytes(), true)
    }

    fn keyed_file(&self, harness: HarnessId) -> PathBuf {
        match harness {
            HarnessId::Pi => self.inner.config.pi_agent_dir.join("auth.json"),
            _ => self.inner.config.opencode_auth_file.clone(),
        }
    }

    /// The live entry under `store_key`, if it's an OAuth login.
    pub(super) fn live_keyed_entry(
        &self,
        harness: HarnessId,
        store_key: &str,
    ) -> Option<serde_json::Value> {
        read_json(&self.keyed_file(harness))?
            .get(store_key)
            .filter(|entry| oauth_entry(entry))
            .cloned()
    }

    /// Replace ONE provider's entry in OpenCode's / Pi's store, keeping every
    /// other key as it is (see [`merge_json_entry`]). Pi: under its own
    /// proper-lockfile lock. OpenCode takes no lock of its own, so zeron
    /// serialises its writers on a sidecar lock and relies on
    /// compare-before-write against OpenCode itself.
    pub(super) fn write_keyed_entry(
        &self,
        harness: HarnessId,
        store_key: &str,
        entry: Option<&serde_json::Value>,
    ) -> Result<(), EngineError> {
        let file = self.keyed_file(harness);
        let lock = match harness {
            HarnessId::Pi => StoreLock::Dir(lock_path(&file)),
            _ => StoreLock::File(file.with_file_name("auth.json.zeron-lock")),
        };
        merge_json_entry(&file, store_key, entry, lock)
    }

    /// The live per-provider logins of OpenCode / Pi: `(identified,
    /// unidentified)`. An unidentified login (an opaque token whose profile
    /// call failed) is still listed, active and unswitchable.
    pub(super) async fn detect_keyed(&self, harness: HarnessId) -> (Vec<Detected>, Vec<Detected>) {
        let mut resolved = Vec::new();
        let mut unidentified = Vec::new();
        let Some(store) = read_json(&self.keyed_file(harness)) else {
            return (resolved, unidentified);
        };
        for &(store_key, upstream) in keyed_accounts(harness) {
            let Some(entry) = store.get(store_key).filter(|e| oauth_entry(e)) else {
                continue;
            };
            match self
                .identify_entry(harness, store_key, upstream, entry)
                .await
            {
                Some(detected) => resolved.push(detected),
                None => unidentified.push(unresolved(store_key, upstream)),
            }
        }
        (resolved, unidentified)
    }

    /// The live login under ONE store key: `Some(None)` when an OAuth entry
    /// is there but couldn't be identified, `None` when there is none.
    pub(super) async fn detect_keyed_entry(
        &self,
        harness: HarnessId,
        store_key: &str,
    ) -> Option<Option<Detected>> {
        let upstream = upstream_of(harness, store_key)?;
        let entry = self.live_keyed_entry(harness, store_key)?;
        Some(
            self.identify_entry(harness, store_key, upstream, &entry)
                .await,
        )
    }

    async fn identify_entry(
        &self,
        harness: HarnessId,
        store_key: &str,
        upstream: Upstream,
        entry: &serde_json::Value,
    ) -> Option<Detected> {
        match upstream {
            Upstream::OpenAi => openai_detected(store_key, entry, None),
            Upstream::Anthropic | Upstream::Copilot => {
                self.identify_opaque(harness, store_key, upstream, entry)
                    .await
            }
        }
    }

    /// Who an opaque live entry is: the slot already holding this exact
    /// token, else the remembered answer for it, else one read-only profile
    /// call (cached per token, so a list doesn't re-ask).
    async fn identify_opaque(
        &self,
        harness: HarnessId,
        store_key: &str,
        upstream: Upstream,
        entry: &serde_json::Value,
    ) -> Option<Detected> {
        let secret = opaque_secret(entry)?;
        if let Some(slot) = self.read_slots(harness).into_iter().find(|slot| {
            slot.store_key.as_deref() == Some(store_key)
                && opaque_secret(&slot.credentials).as_deref() == Some(secret.as_str())
        }) {
            return Some(
                Detected::known(slot.account_key, slot.profile, entry.clone()).keyed(store_key),
            );
        }
        // A GitHub Enterprise host zeron won't send the token to (self-hosted
        // GHES): no profile call — a local identity keeps it switchable.
        if upstream == Upstream::Copilot
            && copilot_api_base(
                entry,
                &self.inner.endpoints.github_api,
                self.inner.endpoints.allow_loopback_http,
            )
            .is_none()
        {
            let (account_key, profile) = local_enterprise_identity(store_key, &secret, entry);
            return Some(Detected::known(account_key, profile, entry.clone()).keyed(store_key));
        }
        let fingerprint = format!("{store_key}:{}", hashed_key(&secret));
        let cached = lock(&self.inner.identities).get(&fingerprint).cloned();
        let (account_key, profile) = match cached {
            Some(IdentityLookup::Known(key, profile)) => (key, profile),
            Some(IdentityLookup::Failed(at)) if at.elapsed() < IDENTITY_RETRY => return None,
            _ => {
                let known = match upstream {
                    Upstream::Anthropic => match str_field(entry, "access") {
                        Some(access) => self.anthropic_identity(store_key, &access).await,
                        None => None,
                    },
                    Upstream::Copilot => self.github_identity(store_key, &secret, entry).await,
                    Upstream::OpenAi => None,
                };
                let lookup = match &known {
                    Some((key, profile)) => IdentityLookup::Known(key.clone(), profile.clone()),
                    None => IdentityLookup::Failed(Instant::now()),
                };
                lock(&self.inner.identities).insert(fingerprint, lookup);
                known?
            }
        };
        Some(Detected::known(account_key, profile, entry.clone()).keyed(store_key))
    }

    /// Claude's profile for a live access token (`user:profile` scope).
    async fn anthropic_identity(
        &self,
        store_key: &str,
        access_token: &str,
    ) -> Option<(String, SlotProfile)> {
        let profile: serde_json::Value = self
            .inner
            .http
            .get(&self.inner.endpoints.claude_profile)
            .bearer_auth(access_token)
            .header("anthropic-beta", "oauth-2025-04-20")
            .send()
            .await
            .ok()
            .filter(|res| res.status().is_success())?
            .json()
            .await
            .ok()?;
        let account = profile.get("account")?;
        let org = profile.get("organization").cloned().unwrap_or_default();
        let email = str_field(account, "email_address")?;
        let uuid = str_field(account, "uuid").unwrap_or_else(|| email.clone());
        Some((
            format!("{store_key}:{uuid}"),
            SlotProfile {
                email,
                display_name: str_field(account, "display_name")
                    .or_else(|| str_field(account, "full_name")),
                organization: str_field(&org, "name"),
                plan: claude_plan(
                    str_field(&org, "organization_type").as_deref(),
                    str_field(&org, "rate_limit_tier").as_deref(),
                )
                .map(|plan| format!("Claude {plan}"))
                .or_else(|| Some("Claude".to_string())),
                auth_kind: AgentAuthKind::Oauth,
            },
        ))
    }

    /// The GitHub user behind a Copilot login's GitHub token.
    pub(super) async fn github_identity(
        &self,
        store_key: &str,
        github_token: &str,
        entry: &serde_json::Value,
    ) -> Option<(String, SlotProfile)> {
        // A GitHub Enterprise host is validated before the token goes there.
        let base = copilot_api_base(
            entry,
            &self.inner.endpoints.github_api,
            self.inner.endpoints.allow_loopback_http,
        )?;
        let user: serde_json::Value = self
            .inner
            .http
            .get(format!("{base}/user"))
            .header("Authorization", format!("token {github_token}"))
            .header("Accept", "application/vnd.github+json")
            .header("User-Agent", "zeron")
            .send()
            .await
            .ok()
            .filter(|res| res.status().is_success())?
            .json()
            .await
            .ok()?;
        let id = user.get("id").and_then(|v| {
            v.as_i64()
                .map(|n| n.to_string())
                .or_else(|| str_field(&user, "id"))
        })?;
        let login = str_field(&user, "login")?;
        Some((
            format!("{store_key}:{id}"),
            SlotProfile {
                email: str_field(&user, "email").unwrap_or_else(|| login.clone()),
                display_name: str_field(&user, "name").or(Some(login)),
                organization: str_field(entry, "enterpriseUrl"),
                plan: Some("GitHub Copilot".to_string()),
                auth_kind: AgentAuthKind::Oauth,
            },
        ))
    }

    /// Persist a fresh login as a slot, then let it take over the live login
    /// where [`AgentAccounts::adopt_if_live`] says so: no live login yet
    /// ("Connect" means runs work afterwards), or a re-login of the live
    /// account (else the next list would snapshot the old, possibly revoked,
    /// live tokens straight back over the fresh slot). Any other live login
    /// is left alone — switching stays explicit. Under [`Inner::ops`], so a
    /// concurrent list can't land between the two writes.
    pub(super) async fn save_new_login(
        &self,
        harness: HarnessId,
        detected: &Detected,
    ) -> Result<(), EngineError> {
        let _ops = self.inner.ops.lock().await;
        self.snapshot_detected(harness, detected)?;
        let id = slot_id_for(harness, &detected.account_key);
        if let Some(slot) = self.read_slot(harness, &id) {
            self.adopt_if_live(&slot).await?;
        }
        Ok(())
    }

    // ── Hermes ──────────────────────────────────────────────────────────────

    /// Hermes' credential pool as rows (never persisted as slots — the pool
    /// is Hermes' own), plus the account keys in use: the active provider's
    /// first entry by priority (fill-first, Hermes' default strategy).
    pub(super) fn hermes_slots(&self) -> (Vec<Slot>, HashSet<String>) {
        let mut slots = Vec::new();
        let mut active = HashSet::new();
        let Some(auth) = read_json(&self.inner.config.hermes_home.join("auth.json")) else {
            return (slots, active);
        };
        let Some(pool) = auth.get("credential_pool").and_then(|p| p.as_object()) else {
            return (slots, active);
        };
        let active_provider = str_field(&auth, "active_provider");
        let mut providers: Vec<&String> = pool.keys().collect();
        // The provider in use first, then the rest alphabetically — stable
        // for as long as the active provider is.
        providers.sort_by_key(|p| (Some(p.as_str()) != active_provider.as_deref(), p.as_str()));
        for provider in providers {
            let Some(entries) = pool.get(provider).and_then(|e| e.as_array()) else {
                continue;
            };
            let mut entries: Vec<&serde_json::Value> = entries
                .iter()
                .filter(|e| str_field(e, "id").is_some())
                .collect();
            entries.sort_by_key(|e| {
                e.get("priority")
                    .and_then(|p| p.as_i64())
                    .unwrap_or(i64::MAX)
            });
            for (ix, entry) in entries.iter().enumerate() {
                let id = str_field(entry, "id").unwrap_or_default();
                let account_key = format!("{provider}:{id}");
                if ix == 0 && active_provider.as_deref() == Some(provider.as_str()) {
                    active.insert(account_key.clone());
                }
                slots.push(Slot {
                    id: slot_id_for(HarnessId::Hermes, &account_key),
                    harness: HarnessId::Hermes,
                    account_key,
                    profile: hermes_profile(provider, entry, ix),
                    credentials: (*entry).clone(),
                    claude_config: None,
                    saved_at: 0,
                    created_at: None,
                    store_key: Some(provider.clone()),
                });
            }
        }
        (slots, active)
    }

    // ── sign-ins through the CLI ────────────────────────────────────────────

    /// A throwaway dir for one sign-in (`.login-<id>`, swept at startup),
    /// owner-only inside an owner-only root before any CLI is spawned.
    pub(super) fn login_home(&self, login_id: &str) -> Result<PathBuf, EngineError> {
        let home = self
            .inner
            .config
            .private_root()?
            .join(format!(".login-{login_id}"));
        private_dir(&home)?;
        Ok(home)
    }

    /// Devin's throwaway sign-in: its XDG data / config / cache homes (all
    /// owner-only) and the recording browser's `browser-url` file, created
    /// empty and 0600 before Devin starts.
    pub(super) fn devin_login_dirs(&self, login_id: &str) -> Result<DevinLoginDirs, EngineError> {
        let home = self.login_home(login_id)?;
        let dirs = DevinLoginDirs {
            data: home.join("data"),
            config: home.join("config"),
            cache: home.join("cache"),
            url_file: home.join("browser-url"),
            home,
        };
        for dir in [&dirs.data, &dirs.config, &dirs.cache] {
            private_dir(dir)?;
        }
        let mut url_file = std::fs::OpenOptions::new();
        url_file.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            url_file.mode(0o600);
        }
        url_file.open(&dirs.url_file)?;
        Ok(dirs)
    }

    async fn cli_command(
        &self,
        harness: HarnessId,
        args: &[&str],
    ) -> Result<zeron_harness::process::Command, EngineError> {
        let acp = self
            .acp_harness(harness)
            .ok_or_else(|| EngineError::Other(format!("{harness:?} has no CLI sign-in")))?;
        acp.cli_command(args).await.map_err(|err| {
            EngineError::Other(match err {
                zeron_harness::HarnessError::NotInstalled(hint) => format!(
                    "The `{}` CLI was not found on this device — install it first. ({hint})",
                    cli_name(harness)
                ),
                other => format!("Could not resolve the {} CLI: {other}", cli_name(harness)),
            })
        })
    }

    /// Grok: `grok login --device-auth` into a throwaway `GROK_HOME`. It
    /// prints a verification url + code; the login lands as that home's
    /// `auth.json`, which the poll snapshots.
    pub(super) async fn start_grok_login(
        &self,
        requester: Option<&str>,
    ) -> Result<AgentLoginStart, EngineError> {
        self.reap_spawned_flows(HarnessId::Grok);
        let login_id = new_id();
        let home = self.login_home(&login_id)?;
        let mut command = match self
            .cli_command(
                HarnessId::Grok,
                &["--no-auto-update", "login", "--device-auth"],
            )
            .await
        {
            Ok(command) => command,
            Err(err) => {
                let _ = std::fs::remove_dir_all(&home);
                return Err(err);
            }
        };
        command.env("GROK_HOME", &home);
        #[cfg(unix)]
        if let Some(noop) = ensure_noop_browser(&self.inner.config.root_dir()) {
            command.env("BROWSER", noop);
        }
        self.spawn_login_child(
            login_id,
            HarnessId::Grok,
            command,
            home,
            SpawnedCompletion::CredentialFile,
            scan_grok_url,
            requester,
        )
        .await
    }

    /// Hermes: `hermes auth add <provider> --type oauth --no-browser` — a
    /// device code, appended to Hermes' own pool under its own lock (it
    /// never becomes the one in use unless the pool was empty). The shared
    /// Nous store is pointed at the throwaway dir so Hermes doesn't offer to
    /// re-import the login already there instead of adding a new one.
    pub(super) async fn start_hermes_login(
        &self,
        provider: &str,
    ) -> Result<AgentLoginStart, EngineError> {
        self.reap_spawned_flows(HarnessId::Hermes);
        let login_id = new_id();
        let home = self.login_home(&login_id)?;
        let mut command = match self
            .cli_command(
                HarnessId::Hermes,
                &["auth", "add", provider, "--type", "oauth", "--no-browser"],
            )
            .await
        {
            Ok(command) => command,
            Err(err) => {
                let _ = std::fs::remove_dir_all(&home);
                return Err(err);
            }
        };
        command
            .env("HERMES_HOME", &self.inner.config.hermes_home)
            .env("HERMES_SHARED_AUTH_DIR", home.join("shared"));
        self.spawn_login_child(
            login_id,
            HarnessId::Hermes,
            command,
            home,
            SpawnedCompletion::ExitSuccess,
            scan_hermes_url,
            None,
        )
        .await
    }

    /// Devin: its ACP server's `devin-browser` sign-in with a throwaway
    /// `XDG_DATA_HOME`, so the new key lands there and never replaces the
    /// live one. Devin opens the browser itself: a recording `$BROWSER`
    /// hands the url to the app instead (where Devin honours `$BROWSER`),
    /// and its log is scanned as a fallback — either way the url's
    /// `redirect_uri` port is reported so a remote login can tunnel it.
    pub(super) fn start_devin_login(
        &self,
        requester: Option<&str>,
    ) -> Result<AgentLoginStart, EngineError> {
        self.reap_spawned_flows(HarnessId::Devin);
        let acp = self
            .acp_harness(HarnessId::Devin)
            .ok_or_else(|| EngineError::Other("Devin has no sign-in".into()))?;
        let login_id = new_id();
        let DevinLoginDirs {
            home,
            data,
            config,
            cache,
            url_file,
        } = self.devin_login_dirs(&login_id)?;
        #[cfg(unix)]
        let browser = ensure_recording_browser(&self.inner.config.root_dir());
        #[cfg(not(unix))]
        let browser = None;
        let options = zeron_harness::acp::SignInOptions {
            browser,
            method: Some("devin-browser".into()),
            env: vec![
                ("XDG_DATA_HOME".into(), data.clone().into()),
                ("XDG_CONFIG_HOME".into(), config.into()),
                ("XDG_CACHE_HOME".into(), cache.into()),
                ("ZERON_LOGIN_URL_FILE".into(), url_file.clone().into()),
            ],
            url_filter: Some(devin_login_url),
        };
        let state = Arc::new(Mutex::new(TaskLoginState {
            requester: requester.map(str::to_string),
            ..Default::default()
        }));
        let this = self.clone();
        let task_state = state.clone();
        let task_home = home.clone();
        let handle = tokio::spawn(async move {
            // Where Devin opened (or logged) its page — the recording
            // browser's file, else its log — until the sign-in ends.
            let watch_state = task_state.clone();
            let logs = data.join("devin");
            let watcher = tokio::spawn(async move {
                loop {
                    if lock(&watch_state).url.is_some() {
                        break;
                    }
                    if let Some(url) = recorded_devin_url(&url_file, &logs) {
                        lock(&watch_state).url = Some(url);
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(250)).await;
                }
            });
            let progress_state = task_state.clone();
            let signed_in = acp
                .sign_in_with(options, move |progress| match progress {
                    zeron_harness::acp::SignInProgress::OpenBrowser(url) => {
                        let mut state = lock(&progress_state);
                        if state.url.is_none() {
                            state.url = Some(url);
                        }
                    }
                })
                .await
                .map_err(|e| e.to_string());
            watcher.abort();
            let detected = signed_in.and_then(|()| {
                let file = data.join("devin").join("credentials.toml");
                std::fs::read_to_string(&file)
                    .ok()
                    .and_then(|text| parse_devin_toml(&text))
                    .and_then(parse_devin_credentials)
                    .ok_or_else(|| {
                        "Devin reported a successful sign-in, but saved no credentials.".to_string()
                    })
            });
            let outcome = match detected {
                Ok(detected) => this
                    .save_new_login(HarnessId::Devin, &detected)
                    .await
                    .map_err(|e| e.to_string()),
                Err(message) => Err(message),
            };
            let _ = std::fs::remove_dir_all(&task_home);
            lock(&task_state).outcome = Some(outcome);
        });
        lock(&self.inner.flows).insert(
            login_id.clone(),
            LoginFlow::Task {
                harness: HarnessId::Devin,
                started_at: Instant::now(),
                state,
                handle,
                home: Some(home),
                port: None,
            },
        );
        Ok(AgentLoginStart {
            login_id,
            url: String::new(),
            mode: AgentLoginMode::Browser,
            callback_port: None,
        })
    }
}

/// See [`AgentAccounts::devin_login_dirs`].
pub(super) struct DevinLoginDirs {
    pub(super) home: PathBuf,
    pub(super) data: PathBuf,
    pub(super) config: PathBuf,
    pub(super) cache: PathBuf,
    pub(super) url_file: PathBuf,
}

fn hermes_profile(provider: &str, entry: &serde_json::Value, ix: usize) -> SlotProfile {
    let access = str_field(entry, "access_token")
        .and_then(|t| jwt_claims(&t))
        .unwrap_or_default();
    let plan = match provider {
        "openai-codex" => access
            .get("https://api.openai.com/auth")
            .and_then(|auth| codex_plan(str_field(auth, "chatgpt_plan_type").as_deref()))
            .or_else(|| Some("ChatGPT".to_string())),
        "nous" => Some("Nous Portal".to_string()),
        "anthropic" => Some("Claude".to_string()),
        "copilot" => Some("GitHub Copilot".to_string()),
        "xai-oauth" => Some("xAI".to_string()),
        "openrouter" => Some("OpenRouter".to_string()),
        other => title_words(other),
    };
    let api_key = str_field(entry, "auth_type").as_deref() == Some("api_key");
    SlotProfile {
        email: str_field(entry, "label")
            .or_else(|| {
                access
                    .get("https://api.openai.com/profile")
                    .and_then(|p| str_field(p, "email"))
            })
            .or_else(|| str_field(&access, "email"))
            .unwrap_or_else(|| format!("{provider} #{}", ix + 1)),
        display_name: None,
        organization: None,
        plan,
        auth_kind: if api_key {
            AgentAuthKind::ApiKey
        } else {
            AgentAuthKind::Oauth
        },
    }
}

pub(super) fn scan_grok_url(output: &str) -> Option<String> {
    scan_https_url(output, &["x.ai", "grok.com"])
}

pub(super) fn scan_hermes_url(output: &str) -> Option<String> {
    scan_https_url(output, &["nousresearch.com", "openai.com"])
}

/// Devin's sign-in page — its CLI-auth page (`/auth/cli/…`) or the Windsurf
/// editor sign-in (`/editor/signin`) — on a Devin / Windsurf / Codeium
/// host over https, with no userinfo or explicit port. A `redirect_uri`, when
/// present, must be a loopback `http` callback. This url is opened on the
/// requesting device, so a CLI (or a log line) naming any other page is
/// ignored.
pub(super) fn devin_login_url(raw: &str) -> bool {
    let raw = raw.trim();
    if !trusted_page(raw, DEVIN_DOMAINS) {
        return false;
    }
    let Ok(url) = reqwest::Url::parse(raw) else {
        return false;
    };
    let path = url.path();
    let sign_in_page = path.starts_with("/auth/cli/")
        || path == "/editor/signin"
        || path.starts_with("/editor/signin/");
    sign_in_page
        && url
            .query_pairs()
            .filter(|(key, _)| key == "redirect_uri")
            .all(|(_, redirect)| loopback_redirect(&redirect))
}

/// An `http://localhost|127.0.0.1|[::1]:<port>/…` callback, no userinfo.
fn loopback_redirect(raw: &str) -> bool {
    reqwest::Url::parse(raw).is_ok_and(|url| {
        url.scheme() == "http"
            && url.username().is_empty()
            && url.password().is_none()
            && url.port().is_some()
            && matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))
    })
}

/// The page a Devin sign-in opened: the recording browser's file, else the
/// first sign-in url in the logs under its throwaway data dir.
pub(super) fn recorded_devin_url(url_file: &Path, logs: &Path) -> Option<String> {
    if let Ok(text) = std::fs::read_to_string(url_file)
        && let Some(url) = text.lines().map(str::trim).find(|l| devin_login_url(l))
    {
        return Some(url.to_string());
    }
    let mut stack = vec![logs.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().and_then(|e| e.to_str()) == Some("log")
                && let Ok(text) = std::fs::read_to_string(&path)
            {
                let mut rest = text.as_str();
                while let Some(start) = rest.find("https://") {
                    let candidate = &rest[start..];
                    let end = candidate
                        .find(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '<' | '>'))
                        .unwrap_or(candidate.len());
                    let url = &candidate[..end];
                    if devin_login_url(url) {
                        return Some(url.to_string());
                    }
                    rest = &candidate[end.max(1)..];
                }
            }
        }
    }
    None
}
