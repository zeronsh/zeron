//! Account rotation: new chats of a harness spread across its saved
//! subscription logins instead of all running on the one live login.
//!
//! Per device, per harness, off by default (`rotation.json`). When on, a chat
//! is pinned to one account the first time a run of it starts (main, side
//! and worker chats alike) and keeps it for its lifetime: CLI sessions,
//! resume and prompt caching all assume stable credentials. Subagents a
//! harness runs inside its own process share their chat's account.
//!
//! **Selection** ([`pick`]): round-robin over the participating accounts in
//! the accounts page's stable order, from a persisted cursor (the account the
//! previous new chat got). An account is skipped while it is signed out (its
//! token was rejected) or exhausted (any usage window at
//! [`EXHAUSTED_FRACTION`] or more that hasn't reset yet), as long as another
//! account has headroom; when every usable account is exhausted the one with
//! the most headroom wins. Usage is the cached probe the accounts page and
//! the composer's ring keep fresh — no probe runs at spawn. Only subscription
//! logins take part (an API key bills per token: spreading over it saves
//! nothing), and rotation is in effect only with two or more usable ones.
//!
//! **Fallback**: a chat whose account was removed or signed out, or ran out
//! while another has headroom, moves to the next pick when its next run
//! starts (`previousAccountId` remembers where it came from).
//!
//! **Isolation**: the live login is never swapped — other chats and the
//! user's own terminals keep running on it. A chat pinned to the LIVE account
//! runs exactly as without rotation. Any other account reaches only its own
//! chat's process:
//!
//! - **Claude Code** gets `CLAUDE_CODE_OAUTH_TOKEN` = the saved login's access
//!   token. The config dir stays the user's own (settings, MCP servers,
//!   skills, plugins, hooks, `projects/` transcripts — resume just works) and
//!   the CLI neither refreshes nor writes a token it got from the
//!   environment, so the live login can't be touched. Zeron owns the
//!   refresh: a run starting with less than [`CLAUDE_REFRESH_MARGIN`] left
//!   on the token refreshes the saved login first (the slot is the only
//!   holder of its refresh token), and the run credential carries the
//!   token's fingerprint, so a session parked on the old token is replaced
//!   (and resumed) before its next turn. Claude Code treats an environment
//!   token as inference-only: features that need the account's profile
//!   scope (Remote Control) are unavailable in rotated chats.
//! - **Cursor** gets `CURSOR_API_KEY` = the saved login's minted user API
//!   key (the SDK takes it over its stored login; a key minted against
//!   another Cursor backend also gets `CURSOR_BACKEND_URL`). **Devin** gets
//!   `WINDSURF_API_KEY` = the saved key (it then owns the session's
//!   identity; a key bound to another Devin server also gets
//!   `WINDSURF_API_SERVER_URL`). Both are long-lived keys nobody refreshes,
//!   so no token is shared with the CLI and the run credential changes only
//!   with the account (or its key, after a re-login). A Cursor key past its
//!   expiry reads as signed out until the account connects again.
//!
//! | harness     | mechanism | how a rotated chat reaches its account |
//! |-------------|-----------|----------------------------------------|
//! | Claude Code | Env       | `CLAUDE_CODE_OAUTH_TOKEN` (engine refreshes the slot) |
//! | Cursor      | Env       | `CURSOR_API_KEY` (+ `CURSOR_BACKEND_URL`), expiry respected |
//! | Devin       | Env       | `WINDSURF_API_KEY` (+ `WINDSURF_API_SERVER_URL`) |
//! | Codex       | Proxy     | `openai_base_url` per thread → proxy route |
//! | Grok        | Proxy     | `GROK_CLI_CHAT_PROXY_BASE_URL` → proxy route |
//! | OpenCode    | Proxy     | ChatGPT logins only; Copilot doesn't rotate |
//! | Pi          | Proxy     | ChatGPT and Claude logins, each its own pool; Copilot doesn't rotate¹ |
//! | Hermes      | None      | rotates its own credential pool² |
//! | Antigravity | None      | one login, never saved as a slot³ |
//! | Muse        | None      | keeps its own per-account config roots ([`super::muse`]) |
//!
//! ¹ Pi keeps every provider's login in one `auth.json` per agent dir and
//! switches provider inside a running process, so a per-account
//! `PI_CODING_AGENT_DIR` would hand the process copies of the live ChatGPT
//! and Claude logins, whose single-use refresh tokens it would rotate away
//! from the live store; `COPILOT_GITHUB_TOKEN` loses to a stored login.
//! ² Zeron never writes Hermes' pool or config (a running Hermes writes its
//! own back), so its `credential_pool_strategies` stay the user's.
//! ³ Its token blob is never read: the login is listed from its method and
//! the token's presence, so there is no second account to pin a chat to.
//!
//! Codex, Grok, OpenCode (ChatGPT) and Pi (ChatGPT, Claude) rotate through
//! the engine's local auth-swapping account proxy ([`super::proxy`],
//! [`RotationMechanism::Proxy`]) instead: the CLI keeps its own live login,
//! its process is pointed at a per-chat proxy route, and the proxy asks
//! [`AgentAccounts::proxy_lease`] for every request, swapping the
//! credential of the chat's account in (a chat on the live account keeps the
//! CLI's own). Limits and rejections are reported back
//! ([`AgentAccounts::report_limit`], [`AgentAccounts::report_rejected`]), so
//! a chat that hits its account's limit fails over to the next account
//! mid-turn and stays there. Tokens of the saved logins that aren't live
//! are the engine's to refresh; the live login's stay the CLI's.
//!
//! OpenCode and Pi keep one login per model provider: rotation spans the
//! providers the proxy serves ([`rotation_pools`]), and a request draws only
//! from the pool of the upstream it goes to (Pi's ChatGPT and Claude
//! logins are separate rotations, each with its own round-robin cursor).
//!
//! Codex used to rotate by a per-account `CODEX_HOME` mirror under
//! `rotation-homes/`; those homes are retired at the first list (a token
//! Codex refreshed in one is saved back into its slot first).

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeron_proto::{
    AgentAccountChat, AgentAccountRotation, AgentAccountSkip, AgentAuthKind, AgentUsageWindow,
    HarnessId,
};

use super::stores::{DEVIN_SERVERS, HostPolicy, devin_api_key, trusted_base};
use super::{
    AgentAccounts, CURSOR_DEFAULT_BACKEND, CURSOR_DOMAINS, EngineError, ProbeError, Slot,
    UsageEntry, credentials_fingerprint, cursor_key_usable, lock, now_ms, parse_codex_auth,
    read_json, str_field, usage_key, write_file_atomic,
};

/// Where Codex's retired per-account homes lived (see module docs).
const HOMES_DIR: &str = "rotation-homes";
/// A usage window this full counts as exhausted (the binding limit is hit).
pub(crate) const EXHAUSTED_FRACTION: f32 = 0.95;
/// A rotated Claude run starts with at least this much left on its token
/// (access tokens live ~8h; a turn running past expiry fails).
pub(crate) const CLAUDE_REFRESH_MARGIN: Duration = Duration::from_secs(2 * 60 * 60);
/// Assignments of chats unused this long are dropped.
const ASSIGNMENT_TTL: Duration = Duration::from_secs(7 * 24 * 60 * 60);
/// The run credential of a chat that runs on the live login.
pub(crate) const LIVE: &str = "live";
/// How long an account that hit its limit is skipped when the provider
/// didn't say when the limit resets.
const LIMIT_COOLDOWN: Duration = Duration::from_secs(60 * 60);

/// How a rotated chat's process reaches its account.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RotationMechanism {
    /// Not rotatable on this device.
    None,
    /// Per-process isolation set at spawn (see module docs): the process
    /// holds its account's token for its lifetime.
    Env,
    /// The agent talks to a local auth-swapping proxy that asks
    /// [`AgentAccounts::proxy_lease`] for every request; the process keeps
    /// the CLI's own login and never sees another account's credential.
    Proxy,
}

/// The built-in mechanism of `harness` (see module docs);
/// [`AgentAccounts::set_rotation_mechanism`] overrides it per device.
pub fn default_rotation_mechanism(harness: HarnessId) -> RotationMechanism {
    match harness {
        // A non-Anthropic `ANTHROPIC_BASE_URL` puts Claude Code in a reduced
        // "custom endpoint" mode, so it keeps per-process tokens.
        HarnessId::ClaudeCode => RotationMechanism::Env,
        // Long-lived keys the CLI takes from its environment (see module docs).
        HarnessId::Cursor | HarnessId::Devin => RotationMechanism::Env,
        HarnessId::Codex | HarnessId::Grok | HarnessId::Opencode | HarnessId::Pi => {
            RotationMechanism::Proxy
        }
        _ => RotationMechanism::None,
    }
}

/// The provider stores a harness's rotation draws from, for agents that keep
/// one login per model provider (`None`: the harness's one store). Each is
/// rotated on its own (see module docs).
pub(crate) fn rotation_pools(harness: HarnessId) -> Option<&'static [&'static str]> {
    match harness {
        // ChatGPT only: Copilot tokens pick their own endpoint.
        HarnessId::Opencode => Some(&["openai"]),
        HarnessId::Pi => Some(&["openai-codex", "anthropic"]),
        _ => None,
    }
}

/// Whether `slot` takes part in `pool` (`None`: any of the harness's pools).
fn in_pool(harness: HarnessId, slot: &Slot, pool: Option<&str>) -> bool {
    let key = slot.store_key.as_deref();
    match (rotation_pools(harness), pool) {
        (_, Some(pool)) => key == Some(pool),
        (Some(pools), None) => key.is_some_and(|key| pools.contains(&key)),
        (None, None) => true,
    }
}

/// Harnesses whose accounts can rotate by default (see module docs).
pub fn rotation_supported(harness: HarnessId) -> bool {
    default_rotation_mechanism(harness) != RotationMechanism::None
}

/// One request's credentials for a chat's account
/// ([`AgentAccounts::lease_for_chat`]).
#[derive(Clone, PartialEq, Eq)]
pub struct Lease {
    pub account_id: String,
    /// The account's current access token (refreshed first when needed).
    pub access_token: String,
    /// Headers that authenticate a provider request as this account,
    /// `Authorization` included.
    pub headers: BTreeMap<String, String>,
}

impl std::fmt::Debug for Lease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Lease")
            .field("account_id", &self.account_id)
            .finish_non_exhaustive()
    }
}

// ── persisted state ─────────────────────────────────────────────────────────

/// `{data_dir}/agent-accounts/rotation.json`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct RotationState {
    #[serde(default)]
    harnesses: HashMap<HarnessId, HarnessRotation>,
    /// chat id → harness → assignment (a chat may change provider).
    #[serde(default)]
    chats: HashMap<String, HashMap<HarnessId, ChatAssignment>>,
    /// harness → account id → epoch millis until which the account is out
    /// of usage, as a provider reply said ([`AgentAccounts::report_limit`]).
    #[serde(default)]
    limited: HashMap<HarnessId, HashMap<String, i64>>,
    /// Per-device mechanism overrides (not persisted: whoever provides one
    /// installs it at startup).
    #[serde(skip)]
    mechanisms: HashMap<HarnessId, RotationMechanism>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HarnessRotation {
    #[serde(default)]
    enabled: bool,
    /// The account the previous new chat got; the next pick starts after it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cursor: Option<String>,
    /// [`Self::cursor`] per provider pool ([`rotation_pools`]).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pool_cursors: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ChatAssignment {
    account_id: String,
    assigned_at: i64,
    used_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    previous_account_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    moved_reason: Option<String>,
    /// The chat's history holds reasoning only this account accepts (it was
    /// moved back here, see [`AgentAccounts::history_recovery`]): it stays
    /// even while the account is out of usage.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    history_bound: bool,
    /// A harness whose logins rotate per provider pool ([`rotation_pools`]):
    /// the assignment in each pool. The fields above then mirror the most
    /// recently used one (what the UI shows for the chat).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pools: BTreeMap<String, ChatAssignment>,
}

impl ChatAssignment {
    fn new(account_id: String, now: i64) -> Self {
        Self {
            account_id,
            assigned_at: now,
            used_at: now,
            previous_account_id: None,
            moved_reason: None,
            history_bound: false,
            pools: BTreeMap::new(),
        }
    }

    /// Mirror the most recently used pool's assignment (see [`Self::pools`]).
    fn mirror_latest_pool(&mut self) {
        let Some(latest) = self.pools.values().max_by_key(|a| a.used_at).cloned() else {
            return;
        };
        let pools = std::mem::take(&mut self.pools);
        *self = Self { pools, ..latest };
    }
}

/// Assignments are persisted at most this often for a mere `used_at` bump
/// (every proxied request touches it).
const USED_AT_PERSIST: Duration = Duration::from_secs(60);

/// How a failover move's reason starts (see [`AgentAccounts::history_recovery`]).
const LIMIT_MOVE: &str = "Usage limit on ";

impl RotationState {
    pub(super) fn load(file: &Path) -> Self {
        let mut state: Self = std::fs::read_to_string(file)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default();
        state.prune(now_ms());
        state
    }

    fn enabled(&self, harness: HarnessId) -> bool {
        self.harnesses.get(&harness).is_some_and(|h| h.enabled)
    }

    fn mechanism(&self, harness: HarnessId) -> RotationMechanism {
        self.mechanisms
            .get(&harness)
            .copied()
            .unwrap_or_else(|| default_rotation_mechanism(harness))
    }

    fn limited_until(&self, harness: HarnessId, account_id: &str, now: i64) -> Option<i64> {
        self.limited
            .get(&harness)?
            .get(account_id)
            .copied()
            .filter(|until| *until > now)
    }

    fn cursor(&self, harness: HarnessId, pool: Option<&str>) -> Option<&str> {
        let h = self.harnesses.get(&harness)?;
        match pool {
            Some(pool) => h.pool_cursors.get(pool).map(String::as_str),
            None => h.cursor.as_deref(),
        }
    }

    fn set_cursor(&mut self, harness: HarnessId, pool: Option<&str>, account_id: &str) {
        let h = self.harnesses.entry(harness).or_default();
        match pool {
            Some(pool) => {
                h.pool_cursors
                    .insert(pool.to_string(), account_id.to_string());
            }
            None => h.cursor = Some(account_id.to_string()),
        }
    }

    /// `chat_id`'s assignment for `harness` (in provider `pool`, for a
    /// harness that rotates per pool).
    fn assignment(
        &self,
        chat_id: &str,
        harness: HarnessId,
        pool: Option<&str>,
    ) -> Option<&ChatAssignment> {
        let a = self.chats.get(chat_id)?.get(&harness)?;
        match pool {
            Some(pool) => a.pools.get(pool),
            None => Some(a),
        }
    }

    fn assignment_mut(
        &mut self,
        chat_id: &str,
        harness: HarnessId,
        pool: Option<&str>,
    ) -> Option<&mut ChatAssignment> {
        let a = self.chats.get_mut(chat_id)?.get_mut(&harness)?;
        match pool {
            Some(pool) => a.pools.get_mut(pool),
            None => Some(a),
        }
    }

    fn set_assignment(
        &mut self,
        chat_id: &str,
        harness: HarnessId,
        pool: Option<&str>,
        assignment: ChatAssignment,
    ) {
        let chat = self.chats.entry(chat_id.to_string()).or_default();
        match pool {
            None => {
                chat.insert(harness, assignment);
            }
            Some(pool) => {
                let entry = chat.entry(harness).or_insert_with(|| assignment.clone());
                entry.pools.insert(pool.to_string(), assignment);
                entry.mirror_latest_pool();
            }
        }
    }

    /// Re-mirror a pooled assignment after one of its pools changed.
    fn refresh_mirror(&mut self, chat_id: &str, harness: HarnessId, pool: Option<&str>) {
        if pool.is_some()
            && let Some(a) = self
                .chats
                .get_mut(chat_id)
                .and_then(|c| c.get_mut(&harness))
        {
            a.mirror_latest_pool();
        }
    }

    /// Drop `chat_id`'s assignment for `harness` (in `pool` only, for a
    /// harness that rotates per pool: the others stay).
    fn release(&mut self, chat_id: &str, harness: HarnessId, pool: Option<&str>) -> bool {
        let Some(chat) = self.chats.get_mut(chat_id) else {
            return false;
        };
        let removed = match pool {
            None => chat.remove(&harness).is_some(),
            Some(pool) => match chat.get_mut(&harness) {
                Some(a) => {
                    let removed = a.pools.remove(pool).is_some();
                    if a.pools.is_empty() {
                        chat.remove(&harness);
                    } else {
                        a.mirror_latest_pool();
                    }
                    removed
                }
                None => false,
            },
        };
        if chat.is_empty() {
            self.chats.remove(chat_id);
        }
        removed
    }

    /// Drop every chat's assignment to `harness` (rotation turned off).
    fn release_harness(&mut self, harness: HarnessId) {
        for chat in self.chats.values_mut() {
            chat.remove(&harness);
        }
        self.chats.retain(|_, chat| !chat.is_empty());
    }

    /// Drop every assignment to a forgotten account.
    pub(super) fn release_account(&mut self, harness: HarnessId, account_id: &str) {
        for chat in self.chats.values_mut() {
            let Some(a) = chat.get_mut(&harness) else {
                continue;
            };
            if a.pools.is_empty() {
                if a.account_id == account_id {
                    chat.remove(&harness);
                }
            } else {
                a.pools.retain(|_, p| p.account_id != account_id);
                if a.pools.is_empty() {
                    chat.remove(&harness);
                } else {
                    a.mirror_latest_pool();
                }
            }
        }
        self.chats.retain(|_, chat| !chat.is_empty());
        if let Some(h) = self.harnesses.get_mut(&harness) {
            if h.cursor.as_deref() == Some(account_id) {
                h.cursor = None;
            }
            h.pool_cursors.retain(|_, id| id != account_id);
        }
        if let Some(limited) = self.limited.get_mut(&harness) {
            limited.remove(account_id);
        }
    }

    fn prune(&mut self, now: i64) {
        let horizon = now - ASSIGNMENT_TTL.as_millis() as i64;
        for chat in self.chats.values_mut() {
            chat.retain(|_, a| {
                if a.pools.is_empty() {
                    return a.used_at >= horizon;
                }
                a.pools.retain(|_, p| p.used_at >= horizon);
                a.mirror_latest_pool();
                !a.pools.is_empty()
            });
        }
        self.chats.retain(|_, chat| !chat.is_empty());
        for limited in self.limited.values_mut() {
            limited.retain(|_, until| *until > now);
        }
        self.limited.retain(|_, limited| !limited.is_empty());
    }
}

// ── selection ───────────────────────────────────────────────────────────────

/// One participating account, as selection sees it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Candidate {
    pub id: String,
    /// False while its token is known rejected (or it holds none).
    pub usable: bool,
    /// The binding limit: the fullest usage window that hasn't reset yet
    /// (`None`: no usage known — treated as headroom).
    pub used: Option<f32>,
}

impl Candidate {
    fn exhausted(&self) -> bool {
        self.used.is_some_and(|used| used >= EXHAUSTED_FRACTION)
    }
}

/// The fullest window of `windows` still running at `now` (a window whose
/// reset time has passed is empty again, whatever the stale probe said).
pub(crate) fn binding_usage(windows: &[AgentUsageWindow], now: i64) -> Option<f32> {
    windows
        .iter()
        .filter(|w| w.resets_at.is_none_or(|at| at.timestamp_millis() > now))
        .map(|w| w.used_fraction.clamp(0.0, 1.0))
        .reduce(f32::max)
}

/// When the exhausted windows of `windows` have all reset (`None`: unknown).
fn exhausted_until(windows: &[AgentUsageWindow], now: i64) -> Option<i64> {
    windows
        .iter()
        .filter(|w| w.used_fraction >= EXHAUSTED_FRACTION)
        .filter_map(|w| w.resets_at.map(|at| at.timestamp_millis()))
        .filter(|at| *at > now)
        .max()
}

/// The rotation rule (see module docs): the index of the account the next
/// chat gets, `None` without a usable one.
pub(crate) fn pick(candidates: &[Candidate], cursor: Option<&str>) -> Option<usize> {
    let start = cursor
        .and_then(|id| candidates.iter().position(|c| c.id == id))
        .map_or(0, |at| at + 1);
    let order: Vec<usize> = (0..candidates.len())
        .map(|i| (start + i) % candidates.len())
        .filter(|&i| candidates[i].usable)
        .collect();
    order
        .iter()
        .copied()
        .find(|&i| !candidates[i].exhausted())
        // All exhausted: the most headroom; round-robin order breaks ties.
        .or_else(|| {
            order.iter().copied().min_by(|&a, &b| {
                let used = |i: usize| candidates[i].used.unwrap_or(0.0);
                used(a).total_cmp(&used(b))
            })
        })
}

/// [`AgentAccounts::assign`]'s answer.
pub(super) enum Assigned {
    /// Rotation isn't in effect: the chat runs on the live login.
    Off,
    Slot(Box<Slot>),
    /// In effect, but every account is excluded or unusable.
    NoneLeft,
}

/// [`AgentAccounts::history_recovery`]'s answer.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum HistoryAnswer {
    PassThrough,
    Retry,
    LimitReached {
        resets_at: Option<chrono::DateTime<chrono::Utc>>,
    },
}

/// What one proxied request of a chat goes out with
/// ([`AgentAccounts::proxy_lease`]).
#[derive(Clone, PartialEq, Eq)]
pub(crate) enum ProxyLease {
    /// The CLI's own login, as sent: the chat runs on the live account
    /// (`Some`: its slot, for reports and failover) or rotation isn't in
    /// effect (`None`).
    Live { account_id: Option<String> },
    /// The chat's account: the headers that authenticate as it.
    Account {
        account_id: String,
        headers: BTreeMap<String, String>,
    },
    /// Every account was excluded for this request.
    NoneLeft,
}

impl std::fmt::Debug for ProxyLease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Live { account_id } => f
                .debug_struct("Live")
                .field("account_id", account_id)
                .finish(),
            Self::Account { account_id, .. } => f
                .debug_struct("Account")
                .field("account_id", account_id)
                .finish_non_exhaustive(),
            Self::NoneLeft => f.write_str("NoneLeft"),
        }
    }
}

/// What a rotated run of a chat gets.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RotationRun {
    pub account_id: String,
    /// Environment for the agent process (empty: the live login).
    pub env: BTreeMap<String, String>,
    /// The run credential ([`crate::registry::RunEnvironment::credential`]).
    pub credential: String,
}

fn short_hash(secret: &str) -> String {
    crate::repos::hex(&Sha256::digest(secret.as_bytes()))[..12].to_string()
}

/// A Codex `auth.json`'s `last_refresh`, as epoch millis.
fn codex_refreshed_at(auth: &serde_json::Value) -> Option<i64> {
    let at = auth.get("last_refresh")?.as_str()?;
    chrono::DateTime::parse_from_rfc3339(at)
        .ok()
        .map(|at| at.timestamp_millis())
}

/// Whether Codex login `a` is a newer refresh of the same tokens than `b`.
fn codex_is_newer(a: &serde_json::Value, b: &serde_json::Value) -> bool {
    let refresh = |auth: &serde_json::Value| {
        auth.get("tokens")
            .and_then(|t| str_field(t, "refresh_token"))
    };
    if refresh(a) == refresh(b) && a.get("tokens") == b.get("tokens") {
        return false;
    }
    match (codex_refreshed_at(a), codex_refreshed_at(b)) {
        (Some(a), Some(b)) => a > b,
        (Some(_), None) => true,
        _ => false,
    }
}

impl AgentAccounts {
    fn persist_rotation(&self, state: &RotationState) {
        let written = serde_json::to_vec_pretty(state)
            .map_err(|e| EngineError::Other(e.to_string()))
            .and_then(|json| {
                self.inner.config.private_root()?;
                write_file_atomic(&self.inner.config.rotation_file(), &json, true)
            });
        if let Err(err) = written {
            tracing::warn!(error = %err, "account rotation state write failed");
        }
    }

    /// How rotated chats of `harness` reach their account on this device
    /// ([`RotationMechanism::None`] on a Cloud device, whose logins belong
    /// to its credential broker).
    pub fn rotation_mechanism(&self, harness: HarnessId) -> RotationMechanism {
        if crate::runner::is_cloud_platform() {
            return RotationMechanism::None;
        }
        lock(&self.inner.rotation).mechanism(harness)
    }

    /// Override `harness`'s rotation mechanism on this device (the account
    /// proxy installs [`RotationMechanism::Proxy`] for what it serves).
    pub fn set_rotation_mechanism(&self, harness: HarnessId, mechanism: RotationMechanism) {
        lock(&self.inner.rotation)
            .mechanisms
            .insert(harness, mechanism);
    }

    fn rotation_available(&self, harness: HarnessId) -> bool {
        self.rotation_mechanism(harness) != RotationMechanism::None
    }

    /// Turn rotation for `harness` on or off. Off releases every chat
    /// pinned to an account: from their next turn they run on the live
    /// login again.
    pub async fn set_rotation(
        &self,
        harness: HarnessId,
        enabled: bool,
    ) -> Result<zeron_proto::AgentAccountsSnapshot, EngineError> {
        if enabled && !self.rotation_available(harness) {
            return Err(EngineError::Other(
                "Account rotation isn't available for this agent on this device.".into(),
            ));
        }
        {
            let mut state = lock(&self.inner.rotation);
            state.harnesses.entry(harness).or_default().enabled = enabled;
            if !enabled {
                state.release_harness(harness);
            }
            self.persist_rotation(&state);
        }
        tracing::info!(harness = ?harness, enabled, "account rotation set");
        self.list(false).await
    }

    /// `harness`'s participating accounts in rotation order: saved
    /// subscription logins holding a token, with what selection needs.
    /// `state` supplies reported limits; `exclude`d accounts count as
    /// unusable. `pool` narrows to one provider store ([`rotation_pools`]).
    fn rotation_candidates(
        &self,
        harness: HarnessId,
        state: &RotationState,
        exclude: &[String],
        pool: Option<&str>,
    ) -> Vec<(Slot, Candidate)> {
        let usage = lock(&self.inner.usage).clone();
        let now = now_ms();
        self.read_slots(harness)
            .into_iter()
            .filter(|slot| slot.profile.auth_kind == AgentAuthKind::Oauth)
            .filter(|slot| in_pool(harness, slot, pool))
            .filter(|slot| holds_token(harness, &slot.credentials))
            .map(|slot| {
                let entry = usage.get(&usage_key(harness, &slot.account_key));
                let probed = entry
                    .and_then(|entry| entry.usage.as_ref())
                    .and_then(|usage| binding_usage(&usage.windows, now));
                let candidate = Candidate {
                    id: slot.id.clone(),
                    usable: !exclude.contains(&slot.id)
                        && !key_expired(harness, &slot.credentials)
                        && entry.is_none_or(|entry| !rejected(entry, &slot.credentials)),
                    // A limit a provider reply reported outranks the probe.
                    used: match state.limited_until(harness, &slot.id, now) {
                        Some(_) => Some(1.0),
                        None => probed,
                    },
                };
                (slot, candidate)
            })
            .collect()
    }

    /// The account `chat_id` runs `harness` on: its pinned one, or the next
    /// pick for a chat new to rotation (or whose account fell out or is
    /// `exclude`d). `None` while rotation isn't in effect — the chat runs on
    /// the live login — or when no account is left.
    fn rotation_assign(
        &self,
        harness: HarnessId,
        chat_id: &str,
        exclude: &[String],
    ) -> Option<Slot> {
        match self.assign(harness, chat_id, exclude, None) {
            Assigned::Slot(slot) => Some(*slot),
            Assigned::Off | Assigned::NoneLeft => None,
        }
    }

    /// Whether rotation is switched on for `harness` on this device (in
    /// effect or not).
    /// Whether rotation is in effect for `harness`'s `pool`: on, with two
    /// usable accounts there.
    pub(super) fn rotation_in_effect(&self, harness: HarnessId, pool: Option<&str>) -> bool {
        if !self.rotation_enabled(harness) {
            return false;
        }
        let state = lock(&self.inner.rotation);
        self.rotation_candidates(harness, &state, &[], pool)
            .iter()
            .filter(|(_, c)| c.usable)
            .count()
            >= 2
    }

    pub(super) fn rotation_enabled(&self, harness: HarnessId) -> bool {
        self.rotation_available(harness) && lock(&self.inner.rotation).enabled(harness)
    }

    /// [`Self::rotation_assign`] within one provider `pool`, telling "not in
    /// effect" from "every account excluded".
    pub(super) fn assign(
        &self,
        harness: HarnessId,
        chat_id: &str,
        exclude: &[String],
        pool: Option<&str>,
    ) -> Assigned {
        let mechanism = self.rotation_mechanism(harness);
        let mut state = lock(&self.inner.rotation);
        if mechanism == RotationMechanism::None || !state.enabled(harness) {
            if state.release(chat_id, harness, pool) {
                self.persist_rotation(&state);
            }
            return Assigned::Off;
        }
        // In effect only with two usable accounts — exclusions don't count:
        // a failover with one account left still moves to it.
        let all = self.rotation_candidates(harness, &state, &[], pool);
        if all.iter().filter(|(_, c)| c.usable).count() < 2 {
            if state.release(chat_id, harness, pool) {
                self.persist_rotation(&state);
            }
            return Assigned::Off;
        }
        let candidates = self.rotation_candidates(harness, &state, exclude, pool);
        let now = now_ms();
        let only: Vec<Candidate> = candidates.iter().map(|(_, c)| c.clone()).collect();
        let current = state.assignment(chat_id, harness, pool);
        let pinned_id = current.map(|a| a.account_id.clone());
        let bound = current.is_some_and(|a| a.history_bound);
        let pinned = pinned_id
            .as_deref()
            .and_then(|id| candidates.iter().find(|(slot, _)| slot.id == id));
        if bound
            && let Some((slot, _)) = pinned
            && exclude.contains(&slot.id)
        {
            // Bound to the account its history needs, which just failed
            // (its limit): stay, and let its own answer reach the CLI.
            return Assigned::NoneLeft;
        }
        let keep = pinned.filter(|(_, c)| {
            // Out of usage while another account has room: move on (unless
            // the chat's history only works there).
            c.usable
                && (bound || !(c.exhausted() && only.iter().any(|o| o.usable && !o.exhausted())))
        });
        let mut changed = false;
        let slot = match keep {
            Some((slot, _)) => {
                let slot = slot.clone();
                if let Some(a) = state.assignment_mut(chat_id, harness, pool)
                    && now - a.used_at >= USED_AT_PERSIST.as_millis() as i64
                {
                    a.used_at = now;
                    changed = true;
                }
                state.refresh_mirror(chat_id, harness, pool);
                slot
            }
            None => {
                let Some(at) = pick(&only, state.cursor(harness, pool)) else {
                    return Assigned::NoneLeft;
                };
                let slot = candidates[at].0.clone();
                // Pinned to a login of another provider pool (a Pi chat that
                // changed model provider): that login still exists.
                let elsewhere = pinned.is_none()
                    && pinned_id
                        .as_deref()
                        .is_some_and(|id| self.read_slot(harness, id).is_some());
                let moved_reason = pinned_id.as_ref().map(|_| {
                    let to = &slot.profile.email;
                    match pinned {
                        None if elsewhere => format!("Changed model provider; moved to {to}"),
                        None => format!("Its account was removed; moved to {to}"),
                        // A reported limit counts the account exhausted.
                        Some((old, c)) if c.exhausted() => {
                            format!("{LIMIT_MOVE}{}, moved to {to}", old.profile.email)
                        }
                        Some((old, _)) => {
                            format!("{} was signed out, moved to {to}", old.profile.email)
                        }
                    }
                });
                match (&pinned_id, &moved_reason) {
                    (Some(from), Some(reason)) => {
                        tracing::info!(chat = %chat_id, harness = ?harness, %from,
                            to = %slot.id, %reason, "rotated chat moved to another account");
                    }
                    _ => tracing::info!(chat = %chat_id, harness = ?harness, account = %slot.id,
                        "rotated chat assigned an account"),
                }
                // Only a new chat advances the round-robin; a move doesn't
                // skip the next new chat past an account.
                if pinned_id.is_none() {
                    state.set_cursor(harness, pool, &slot.id);
                }
                let mut assignment = ChatAssignment::new(slot.id.clone(), now);
                assignment.previous_account_id = pinned_id.clone();
                assignment.moved_reason = moved_reason;
                state.set_assignment(chat_id, harness, pool, assignment);
                changed = true;
                slot
            }
        };
        if changed {
            state.prune(now);
            self.persist_rotation(&state);
        }
        Assigned::Slot(Box::new(slot))
    }

    /// The credentials one request of `chat_id`'s `harness` run should carry
    /// (the account proxy's per-request lookup): the chat's pinned account,
    /// or the next pick for a chat new to rotation. `exclude` lists accounts
    /// this request already failed on (a limit reply): a pinned account in it
    /// moves the chat for good. `None`: rotation isn't in effect — forward
    /// the request on the live login.
    /// Kept for the rotation tests; the proxy asks [`Self::proxy_lease`].
    #[cfg(test)]
    pub(crate) async fn lease_for_chat(
        &self,
        chat_id: &str,
        harness: HarnessId,
        exclude: &[String],
    ) -> Option<Lease> {
        for _ in 0..2 {
            let slot = self.rotation_assign(harness, chat_id, exclude)?;
            let live = self.live_account_key(harness, None).await;
            let is_live = live.as_deref() == Some(slot.account_key.as_str());
            let token = match harness {
                // The live login's token is the CLI's to refresh: read it
                // where the CLI keeps it.
                HarnessId::ClaudeCode if is_live => {
                    let (credentials, _) =
                        self.read_claude_credentials_cached(&slot.account_key).await;
                    credentials
                        .as_ref()
                        .and_then(|c| c.get("claudeAiOauth"))
                        .and_then(|oauth| str_field(oauth, "accessToken"))
                        .ok_or(ProbeError::NoCredentials {
                            why: super::NoCredentials::Missing,
                        })
                }
                HarnessId::ClaudeCode => self.fresh_claude_token(&slot).await,
                HarnessId::Codex => {
                    let credentials = match is_live {
                        true => self.detect_codex().and_then(|d| d.credentials),
                        false => Some(slot.credentials.clone()),
                    };
                    credentials
                        .as_ref()
                        .and_then(|c| c.get("tokens"))
                        .and_then(|t| str_field(t, "access_token"))
                        .ok_or(ProbeError::NoCredentials {
                            why: super::NoCredentials::Missing,
                        })
                }
                _ => return None,
            };
            let access_token = match token {
                Ok(token) => token,
                Err(ProbeError::Unauthorized { .. }) => continue,
                Err(error) => {
                    tracing::warn!(slot = %slot.id, class = error.class(),
                        "rotated login unavailable for a lease");
                    return None;
                }
            };
            let mut headers =
                BTreeMap::from([("authorization".into(), format!("Bearer {access_token}"))]);
            if harness == HarnessId::Codex
                && let Some(account) = slot
                    .credentials
                    .get("tokens")
                    .and_then(|t| str_field(t, "account_id"))
            {
                headers.insert("chatgpt-account-id".into(), account);
            }
            return Some(Lease {
                account_id: slot.id,
                access_token,
                headers,
            });
        }
        None
    }

    /// The credentials one proxied request of `chat_id`'s `harness` run goes
    /// out with, drawn from provider `pool` ([`rotation_pools`]): the chat's
    /// pinned account (or the next pick for a chat new to rotation), as
    /// swapped-in headers, or the CLI's own login when that account is the
    /// live one or rotation isn't in effect. `exclude` lists accounts this
    /// request already failed on: a pinned account in it moves the chat for
    /// good. The live login's token is never touched (the CLI refreshes it);
    /// other logins are refreshed here when they are about to expire.
    pub(crate) async fn proxy_lease(
        &self,
        chat_id: &str,
        harness: HarnessId,
        pool: Option<&str>,
        exclude: &[String],
        cli_identity: Option<&str>,
    ) -> ProxyLease {
        // A refused token takes its account out; the chat gets the next.
        for _ in 0..2 {
            // Selection reads slot files under a std lock: off the async
            // workers.
            let assigned = {
                let this = self.clone();
                let (chat, exclude) = (chat_id.to_string(), exclude.to_vec());
                let pool = pool.map(String::from);
                tokio::task::spawn_blocking(move || {
                    this.assign(harness, &chat, &exclude, pool.as_deref())
                })
                .await
            };
            let slot = match assigned {
                Ok(Assigned::Off) | Err(_) => return ProxyLease::Live { account_id: None },
                Ok(Assigned::NoneLeft) => return ProxyLease::NoneLeft,
                Ok(Assigned::Slot(slot)) => *slot,
            };
            let live = self.proxy_live_key(harness, pool).await;
            // The chat's account is the live one, and the CLI really holds
            // it (a process started before a live switch holds the old one
            // until it is replaced): the CLI's own login goes out.
            if live.as_deref() == Some(slot.account_key.as_str())
                && super::proxy::cli_holds(&slot, cli_identity)
            {
                return ProxyLease::Live {
                    account_id: Some(slot.id),
                };
            }
            match self.proxy_headers(&slot).await {
                Ok(headers) => {
                    return ProxyLease::Account {
                        account_id: slot.id,
                        headers,
                    };
                }
                Err(ProbeError::Unauthorized { .. }) => continue,
                Err(error) => {
                    tracing::warn!(slot = %slot.id, class = error.class(),
                        "rotated login unavailable for a proxied request; using the live login");
                    return ProxyLease::Live { account_id: None };
                }
            }
        }
        ProxyLease::NoneLeft
    }

    /// The provider refused a request of `chat_id` on `account_id` because
    /// its history holds reasoning another account produced. When a usage
    /// limit moved the chat here, it goes back to the account it came from
    /// for good (its history only works there): try it again there if that
    /// account has room, or stop the way a usage limit does if it hasn't.
    /// Only a failover move is undone, and only once (the chat stays bound
    /// to its history's account afterwards), so this never loops.
    pub(super) fn history_recovery(
        &self,
        harness: HarnessId,
        chat_id: &str,
        pool: Option<&str>,
        account_id: &str,
    ) -> HistoryAnswer {
        let mut state = lock(&self.inner.rotation);
        let Some(current) = state.assignment(chat_id, harness, pool).cloned() else {
            return HistoryAnswer::PassThrough;
        };
        let moved_by_limit = current
            .moved_reason
            .as_deref()
            .is_some_and(|reason| reason.starts_with(LIMIT_MOVE));
        let Some(previous) = current.previous_account_id.clone().filter(|_| {
            current.account_id == account_id && moved_by_limit && !current.history_bound
        }) else {
            return HistoryAnswer::PassThrough;
        };
        let candidates = self.rotation_candidates(harness, &state, &[], pool);
        let Some((owner, candidate)) = candidates.iter().find(|(slot, _)| slot.id == previous)
        else {
            return HistoryAnswer::PassThrough;
        };
        if !candidate.usable {
            // Signed out: the history can't be served anywhere now.
            return HistoryAnswer::PassThrough;
        }
        let now = now_ms();
        let mut back = ChatAssignment::new(previous.clone(), now);
        back.previous_account_id = Some(current.account_id.clone());
        back.moved_reason = Some(format!(
            "Moved back to {}: this chat's history is tied to it",
            owner.profile.email
        ));
        back.history_bound = true;
        state.set_assignment(chat_id, harness, pool, back);
        self.persist_rotation(&state);
        tracing::info!(chat = %chat_id, harness = ?harness, from = %account_id, to = %previous,
            "chat moved back to the account its history belongs to");
        if !candidate.exhausted() {
            return HistoryAnswer::Retry;
        }
        let usage = lock(&self.inner.usage).clone();
        let probed = usage
            .get(&usage_key(harness, &owner.account_key))
            .and_then(|entry| entry.usage.as_ref())
            .and_then(|usage| exhausted_until(&usage.windows, now));
        HistoryAnswer::LimitReached {
            resets_at: state
                .limited_until(harness, &previous, now)
                .or(probed)
                .and_then(chrono::DateTime::from_timestamp_millis),
        }
    }

    /// A provider reply said `account_id` is out of usage until `resets_at`
    /// (unknown: an hour). Rotation skips it while another account has room,
    /// and chats pinned to it move at their next lease or run start.
    pub fn report_limit(
        &self,
        harness: HarnessId,
        account_id: &str,
        resets_at: Option<chrono::DateTime<chrono::Utc>>,
    ) {
        let now = now_ms();
        let until = resets_at
            .map(|at| at.timestamp_millis())
            .filter(|at| *at > now)
            .unwrap_or(now + LIMIT_COOLDOWN.as_millis() as i64);
        let mut state = lock(&self.inner.rotation);
        state
            .limited
            .entry(harness)
            .or_default()
            .insert(account_id.to_string(), until);
        self.persist_rotation(&state);
        tracing::info!(harness = ?harness, account = %account_id, until, "account reported out of usage");
    }

    /// A provider refused `account_id`'s token: it is signed out for
    /// rotation until its credentials change (a re-login or refresh).
    pub fn report_rejected(&self, harness: HarnessId, account_id: &str) {
        let Some(slot) = self.read_slot(harness, account_id) else {
            return;
        };
        self.mark_rejected(&slot, ProbeError::Unauthorized { status: 401 });
    }

    pub(super) fn mark_rejected(&self, slot: &Slot, error: ProbeError) {
        tracing::warn!(slot = %slot.id, "rotated login was refused; skipping it");
        let mut usage = lock(&self.inner.usage);
        usage
            .entry(usage_key(slot.harness, &slot.account_key))
            .or_default()
            .record(
                Err(error),
                credentials_fingerprint(&slot.credentials),
                now_ms(),
            );
        self.persist_usage(&usage);
    }

    /// What a run of `chat_id` on `harness` gets under rotation by
    /// per-process isolation (Claude Code, Cursor, Devin). `None`: not in
    /// effect for it (or its harness rotates through the proxy).
    pub(crate) async fn rotation_run(
        &self,
        harness: HarnessId,
        chat_id: &str,
    ) -> Option<RotationRun> {
        if self.rotation_mechanism(harness) != RotationMechanism::Env {
            return None;
        }
        // A rejected token takes its account out; the chat gets the next.
        for _ in 0..2 {
            let slot = self.rotation_assign(harness, chat_id, &[])?;
            let live = self.live_account_key(harness, None).await;
            if live.as_deref() == Some(slot.account_key.as_str()) {
                return Some(RotationRun {
                    account_id: slot.id.clone(),
                    env: BTreeMap::new(),
                    credential: format!("rotation:{}:{LIVE}", slot.id),
                });
            }
            match harness {
                HarnessId::ClaudeCode => match self.fresh_claude_token(&slot).await {
                    Ok(token) => {
                        return Some(RotationRun {
                            account_id: slot.id.clone(),
                            credential: format!("rotation:{}:{}", slot.id, short_hash(&token)),
                            env: BTreeMap::from([("CLAUDE_CODE_OAUTH_TOKEN".into(), token)]),
                        });
                    }
                    Err(ProbeError::Unauthorized { .. }) => continue,
                    Err(error) => {
                        tracing::warn!(slot = %slot.id, class = error.class(),
                            "rotated Claude login unavailable; running on the live login");
                        return None;
                    }
                },
                // A saved key, as is: nothing to refresh, and the credential
                // changes only with the account (or its key, after a re-login).
                HarnessId::Cursor | HarnessId::Devin => {
                    let Some((key, env)) = key_run_env(&slot) else {
                        tracing::warn!(slot = %slot.id,
                            "rotated login holds no usable key; running on the live login");
                        return None;
                    };
                    return Some(RotationRun {
                        account_id: slot.id.clone(),
                        credential: format!("rotation:{}:{}", slot.id, short_hash(&key)),
                        env,
                    });
                }
                _ => return None,
            }
        }
        None
    }

    /// The saved Claude login's access token, refreshed first when it has
    /// less than [`CLAUDE_REFRESH_MARGIN`] left. A refused refresh marks the
    /// account signed out (rotation skips it until it signs in again).
    async fn fresh_claude_token(&self, slot: &Slot) -> Result<String, ProbeError> {
        let missing = ProbeError::NoCredentials {
            why: super::NoCredentials::Missing,
        };
        let token = |slot: &Slot| -> Option<(String, Option<i64>)> {
            let oauth = slot.credentials.get("claudeAiOauth")?;
            let expires = oauth
                .get("expiresAt")
                .and_then(|v| v.as_i64())
                .filter(|ms| *ms > 0);
            Some((str_field(oauth, "accessToken")?, expires))
        };
        let fresh = |expires: Option<i64>| {
            expires.is_some_and(|at| at - now_ms() > CLAUDE_REFRESH_MARGIN.as_millis() as i64)
        };
        if let Some((access, expires)) = token(slot)
            && fresh(expires)
        {
            return Ok(access);
        }
        // Serialized with switches and lists: a switch must never install a
        // refresh token this rotates away underneath it.
        let _ops = self.inner.ops.lock().await;
        let slot = self
            .read_slot(slot.harness, &slot.id)
            .ok_or(missing.clone())?;
        let current = token(&slot);
        if let Some((access, expires)) = &current
            && fresh(*expires)
        {
            return Ok(access.clone());
        }
        if !self.inner.endpoints.allow_slot_refresh || !self.claude_slot_is_not_live(&slot).await {
            return current.map(|(access, _)| access).ok_or(missing);
        }
        match self.refresh_claude_slot(&slot).await {
            Ok(Some(access)) => Ok(access),
            // Refreshed elsewhere meanwhile: the slot holds the result.
            Ok(None) => self
                .read_slot(slot.harness, &slot.id)
                .and_then(|slot| token(&slot))
                .map(|(access, _)| access)
                .ok_or(missing),
            Err(error @ ProbeError::Unauthorized { .. }) => {
                self.mark_rejected(&slot, error.clone());
                Err(error)
            }
            // Offline / 5xx: a token that hasn't expired yet still works.
            Err(error) => match current {
                Some((access, expires)) if expires.is_none_or(|at| at > now_ms()) => Ok(access),
                _ => Err(error),
            },
        }
    }

    /// Retire the per-account Codex homes an earlier build rotated with (see
    /// module docs): copy a login Codex refreshed inside one back into its
    /// slot, then shred them (caller holds [`super::Inner::ops`]). The live
    /// account's slot follows the live store instead, never a home.
    pub(super) fn retire_rotation_homes(&self) {
        let Ok(root) = self.inner.config.private_root() else {
            return;
        };
        let codex = root
            .join(HOMES_DIR)
            .join(super::harness_slug(HarnessId::Codex));
        if !codex.is_dir() {
            return;
        }
        let live = self.detect_codex().map(|d| d.account_key);
        let live_file = self.inner.config.codex_auth_file();
        for slot in self.read_slots(HarnessId::Codex) {
            let Some(home) = read_json(&codex.join(&slot.id).join("auth.json")) else {
                continue;
            };
            let same_account =
                parse_codex_auth(home.clone()).is_some_and(|d| d.account_key == slot.account_key);
            if same_account && live.as_deref() == Some(slot.account_key.as_str()) {
                // The account is live now: its store is the CLI's. Only a
                // strictly newer refresh from the home (whose rotation made
                // the store's refresh token stale) replaces it.
                let Some(store) = read_json(&live_file) else {
                    continue;
                };
                if codex_is_newer(&home, &store) {
                    let json = serde_json::to_string_pretty(&home).unwrap_or_default();
                    if let Err(err) = write_file_atomic(&live_file, json.as_bytes(), true) {
                        tracing::warn!(error = %err, "retired Codex home: live write-back failed");
                        return;
                    }
                    let mut updated = slot.clone();
                    updated.credentials = home;
                    updated.saved_at = now_ms();
                    let _ = self.write_slot(&updated);
                    tracing::info!(slot = %slot.id, "rotated Codex login saved back into the live store");
                }
                continue;
            }
            if same_account && codex_is_newer(&home, &slot.credentials) {
                let mut updated = slot.clone();
                updated.credentials = home;
                updated.saved_at = now_ms();
                match self.write_slot(&updated) {
                    Ok(()) => tracing::info!(slot = %slot.id, "rotated Codex login saved back"),
                    Err(err) => {
                        // Keep the home: it holds the only copy of that token.
                        tracing::warn!(slot = %slot.id, error = %err,
                            "rotated Codex login write-back failed");
                        return;
                    }
                }
            }
        }
        super::shred_dir(&root.join(HOMES_DIR));
        tracing::info!("retired the per-account Codex rotation homes");
    }

    /// A forgotten account leaves rotation: its chats move on at their next
    /// run (or request).
    pub(super) fn forget_rotation(&self, harness: HarnessId, account_id: &str) {
        let mut state = lock(&self.inner.rotation);
        state.release_account(harness, account_id);
        self.persist_rotation(&state);
    }

    /// Why each skipped participant is skipped, and until when.
    fn rotation_skips(
        &self,
        harness: HarnessId,
        state: &RotationState,
        pairs: &[(Slot, Candidate)],
    ) -> Vec<AgentAccountSkip> {
        let now = now_ms();
        let usage = lock(&self.inner.usage).clone();
        pairs
            .iter()
            .filter_map(|(slot, candidate)| {
                if !candidate.usable {
                    return Some(AgentAccountSkip {
                        account_id: slot.id.clone(),
                        reason: "Signed out".into(),
                        until: None,
                    });
                }
                if !candidate.exhausted() {
                    return None;
                }
                let probed = usage
                    .get(&usage_key(harness, &slot.account_key))
                    .and_then(|entry| entry.usage.as_ref())
                    .and_then(|usage| exhausted_until(&usage.windows, now));
                Some(AgentAccountSkip {
                    account_id: slot.id.clone(),
                    reason: "Usage limit".into(),
                    until: state.limited_until(harness, &slot.id, now).or(probed),
                })
            })
            .collect()
    }

    /// The snapshot's rotation view for `harnesses` (no side effects).
    pub(super) fn rotation_view(&self, harnesses: &[HarnessId]) -> Vec<AgentAccountRotation> {
        let state = lock(&self.inner.rotation).clone();
        let running = self.running_chats();
        let mut view = Vec::new();
        for &harness in harnesses {
            let supported = self.rotation_available(harness);
            let enabled = supported && state.enabled(harness);
            let pairs = if supported {
                self.rotation_candidates(harness, &state, &[], None)
            } else {
                Vec::new()
            };
            let skipped = self.rotation_skips(harness, &state, &pairs);
            let candidates: Vec<Candidate> = pairs.into_iter().map(|(_, c)| c).collect();
            let usable = candidates.iter().filter(|c| c.usable).count();
            let active = enabled && usable >= 2;
            let note = if crate::runner::is_cloud_platform() {
                Some("Cloud machines use the accounts connected in Settings → Cloud.".to_string())
            } else if !supported {
                Some(unsupported_note(harness).to_string())
            } else if usable < 2 {
                Some("Needs at least two signed-in subscription accounts.".to_string())
            } else {
                None
            };
            let mut chats: Vec<AgentAccountChat> = if active {
                state
                    .chats
                    .iter()
                    .filter_map(|(chat_id, by_harness)| {
                        let a = by_harness.get(&harness)?;
                        Some(AgentAccountChat {
                            chat_id: chat_id.clone(),
                            account_id: a.account_id.clone(),
                            assigned_at: a.assigned_at,
                            used_at: a.used_at,
                            previous_account_id: a.previous_account_id.clone(),
                            moved_reason: a.moved_reason.clone(),
                            running: running.contains(chat_id),
                        })
                    })
                    .collect()
            } else {
                Vec::new()
            };
            chats.sort_by(|a, b| b.used_at.cmp(&a.used_at).then(a.chat_id.cmp(&b.chat_id)));
            view.push(AgentAccountRotation {
                harness,
                supported,
                enabled,
                active,
                next_account_id: active
                    .then(|| pick(&candidates, state.cursor(harness, None)))
                    .flatten()
                    .map(|at| candidates[at].id.clone()),
                accounts: candidates.into_iter().map(|c| c.id).collect(),
                chats,
                note,
                skipped,
            });
        }
        view
    }
}

/// Whether `credentials` hold a token a rotated run could use.
fn holds_token(harness: HarnessId, credentials: &serde_json::Value) -> bool {
    match harness {
        HarnessId::ClaudeCode => credentials.get("claudeAiOauth").is_some_and(|oauth| {
            str_field(oauth, "accessToken").is_some() || str_field(oauth, "refreshToken").is_some()
        }),
        HarnessId::Codex => credentials
            .get("tokens")
            .is_some_and(|tokens| str_field(tokens, "access_token").is_some()),
        HarnessId::Grok => str_field(credentials, "key").is_some(),
        HarnessId::Opencode | HarnessId::Pi => str_field(credentials, "access").is_some(),
        HarnessId::Cursor => str_field(credentials, "apiKey").is_some(),
        HarnessId::Devin => devin_api_key(credentials).is_some(),
        _ => !credentials.is_null(),
    }
}

/// Whether `credentials` hold a key past its expiry (Cursor's minted keys
/// expire; nothing refreshes them, so the account reads signed out until
/// it connects again).
fn key_expired(harness: HarnessId, credentials: &serde_json::Value) -> bool {
    harness == HarnessId::Cursor && !cursor_key_usable(credentials)
}

/// The environment that runs `slot`'s agent on its own saved key, and that
/// key: Cursor's SDK takes `CURSOR_API_KEY` over its stored login, Devin
/// takes `WINDSURF_API_KEY` over `credentials.toml`. A key bound to another
/// server goes with that server's url, and only to the vendor's own hosts.
/// `None`: no usable key.
fn key_run_env(slot: &Slot) -> Option<(String, BTreeMap<String, String>)> {
    let credentials = &slot.credentials;
    let (key, var, server, server_var) = match slot.harness {
        HarnessId::Cursor if cursor_key_usable(credentials) => {
            let backend = str_field(credentials, "backendUrl")
                .filter(|url| url.trim_end_matches('/') != CURSOR_DEFAULT_BACKEND)
                .and_then(|url| trusted_base(&url, HostPolicy::Domains(CURSOR_DOMAINS), false));
            (
                str_field(credentials, "apiKey")?,
                "CURSOR_API_KEY",
                backend,
                "CURSOR_BACKEND_URL",
            )
        }
        HarnessId::Devin => (
            devin_api_key(credentials)?,
            "WINDSURF_API_KEY",
            str_field(credentials, "api_server_url")
                .and_then(|url| trusted_base(&url, DEVIN_SERVERS, false)),
            "WINDSURF_API_SERVER_URL",
        ),
        _ => return None,
    };
    let mut env = BTreeMap::from([(var.to_string(), key.clone())]);
    if let Some(server) = server {
        env.insert(
            server_var.to_string(),
            server.as_str().trim_end_matches('/').to_string(),
        );
    }
    Some((key, env))
}

/// Why `harness` can't rotate, for the toggle's caption.
fn unsupported_note(harness: HarnessId) -> &'static str {
    match harness {
        HarnessId::Hermes => "Hermes rotates its saved accounts itself.",
        HarnessId::Antigravity => {
            "Antigravity keeps one Google login, and Zeron can't save a second one to take \
             turns with."
        }
        _ => "Rotation isn't available for this agent yet.",
    }
}

/// Whether the account's last probe found THESE credentials rejected (a
/// re-login or refresh since makes it usable again).
fn rejected(entry: &UsageEntry, credentials: &serde_json::Value) -> bool {
    matches!(
        entry.error,
        Some(ProbeError::Unauthorized { .. } | ProbeError::NoCredentials { .. })
    ) && entry.credentials.as_deref() == Some(credentials_fingerprint(credentials).as_str())
}

// ── spawn-time environment ──────────────────────────────────────────────────

/// The agent accounts' spawn-time environment: Muse's zeron-kept accounts
/// ([`super::MuseRunEnvironment`]) and account rotation.
pub struct AccountsRunEnvironment(pub AgentAccounts);

#[async_trait::async_trait]
impl crate::registry::RunEnvironment for AccountsRunEnvironment {
    async fn prepare(
        &self,
        harness: HarnessId,
        chat_id: Option<&str>,
        request: &mut zeron_proto::RunRequest,
    ) -> Result<(), String> {
        if harness == HarnessId::Muse {
            return super::MuseRunEnvironment(self.0.clone())
                .prepare(harness, chat_id, request)
                .await;
        }
        let Some(chat_id) = chat_id else {
            return Ok(());
        };
        let env = match self.0.rotation_mechanism(harness) {
            RotationMechanism::Env => self
                .0
                .rotation_run(harness, chat_id)
                .await
                .map(|run| run.env),
            RotationMechanism::Proxy => self
                .0
                .proxy_routes(harness, chat_id, Some(&request.cwd))
                .await
                .map(|routes| routes.env),
            RotationMechanism::None => None,
        };
        // Never over what another provider (the Cloud broker) set.
        for (key, value) in env.unwrap_or_default() {
            request.env.entry(key).or_insert(value);
        }
        Ok(())
    }

    async fn credential(&self, harness: HarnessId, chat_id: Option<&str>) -> Option<String> {
        if harness == HarnessId::Muse {
            return super::MuseRunEnvironment(self.0.clone())
                .credential(harness, chat_id)
                .await;
        }
        let chat_id = chat_id?;
        // A chat leaving rotation (turned off, its last other account gone)
        // reads as a change too, so its parked session moves to the live
        // login before its next turn.
        match self.0.rotation_mechanism(harness) {
            RotationMechanism::Env => Some(match self.0.rotation_run(harness, chat_id).await {
                Some(run) => run.credential,
                None => LIVE.to_string(),
            }),
            // A proxied chat swaps accounts per request, so its process is
            // replaced only when it gains or loses its route.
            RotationMechanism::Proxy => {
                Some(match self.0.proxy_routes(harness, chat_id, None).await {
                    Some(routes) => routes.credential,
                    None => LIVE.to_string(),
                })
            }
            RotationMechanism::None => None,
        }
    }

    async fn record_usage(
        &self,
        harness: HarnessId,
        credential: &str,
        windows: Vec<AgentUsageWindow>,
        plan_label: Option<String>,
    ) {
        super::MuseRunEnvironment(self.0.clone())
            .record_usage(harness, credential, windows, plan_label)
            .await;
    }
}

#[cfg(test)]
mod tests;
