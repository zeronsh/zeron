//! Account rotation through the local account proxy
//! ([`super::rotation::RotationMechanism::Proxy`], crate `zeron-account-proxy`).
//!
//! One proxy per engine, started at the first rotating run. Each rotating
//! chat gets one route per upstream its agent talks to ([`proxied_upstreams`])
//! — a random, unguessable URL that is the route's only credential — and
//! keeps it across process restarts, so a parked session never has to be
//! replaced for an account switch: the switch happens per request, inside
//! the proxy. The route's base URL rides the run environment
//! ([`zeron_harness::account_proxy`]) and the harness points its CLI at it.
//! An upstream is routed only while rotation is in effect for its provider,
//! the CLI's live login for it is a subscription login, and the user hasn't
//! configured an endpoint of their own for it ([`routing`]). The route is
//! visible to the agent's own tools (same user as the saved logins); the
//! proxy lets it reach the provider's inference endpoints only.
//!
//! The CLI keeps its own live login. For every request the proxy asks
//! [`AgentAccounts::proxy_lease`]: a chat on the live account (or a request
//! while rotation is off) goes out with the CLI's own credential, after
//! checking the CLI really holds that account (ChatGPT: its workspace id);
//! any other account's credential is swapped in. A refused own credential
//! goes back to the CLI as is, so its own refresh runs.
//!
//! Refresh ownership: a login this engine has seen live is never refreshed
//! here, not even after the live account changed (a process started on it
//! may still hold its single-use refresh token); its token is used until it
//! expires. Every other saved login is refreshed here shortly before it
//! expires, under the accounts lock and after re-reading the slot.
//!
//! What the proxy learns flows back: a usage limit or a refused token moves
//! the chat (the lease that retries the request records `previousAccountId`
//! and `movedReason`), and every successful response's rate-limit headers
//! refresh the account's cached usage.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use sha2::Digest as _;
use zeron_account_proxy as ap;
use zeron_harness::account_proxy as env;
use zeron_proto::{AgentUsageWindow, HarnessId};

use super::rotation::{HistoryAnswer, LIVE, ProxyLease};
use super::{
    AgentAccounts, CLAUDE_CLIENT_ID, InflightGuard, Inner, NoCredentials, ProbeError, Slot,
    UsageSnapshot, credentials_fingerprint, harness_slug, jwt_claims, lock, now_ms, probe_json,
    schema_error, str_field, usage_key,
};

/// A route nobody used for this long is dropped (a parked session that
/// wakes later is replaced and gets a new one, see
/// [`AgentAccounts::proxy_routes`]).
const ROUTE_IDLE: Duration = Duration::from_secs(24 * 60 * 60);
/// A saved login is refreshed when its token has less than this left.
const REFRESH_MARGIN: Duration = Duration::from_secs(5 * 60);
/// Rate-limit headers update an account's cached usage at most this often.
const USAGE_FEED_INTERVAL: Duration = Duration::from_secs(60);

/// The upstreams `harness`'s agent talks to through the proxy, with the
/// provider pool each draws its accounts from ([`super::rotation::rotation_pools`]).
pub(super) fn proxied_upstreams(
    harness: HarnessId,
) -> &'static [(ap::Provider, Option<&'static str>)] {
    match harness {
        HarnessId::Codex => &[(ap::Provider::ChatGpt, None)],
        HarnessId::Opencode => &[(ap::Provider::ChatGpt, Some("openai"))],
        HarnessId::Pi => &[
            (ap::Provider::ChatGpt, Some("openai-codex")),
            (ap::Provider::Anthropic, Some("anthropic")),
        ],
        HarnessId::Grok => &[(ap::Provider::Grok, None)],
        _ => &[],
    }
}

/// The run-environment variable that carries a route to `provider`.
fn route_var(provider: ap::Provider) -> Option<&'static str> {
    match provider {
        ap::Provider::ChatGpt => Some(env::CHATGPT),
        ap::Provider::Anthropic => Some(env::ANTHROPIC),
        ap::Provider::Grok => Some(env::GROK),
        ap::Provider::Muse => None,
    }
}

fn parse_harness(slug: &str) -> Option<HarnessId> {
    HarnessId::ALL
        .into_iter()
        .find(|harness| harness_slug(*harness) == slug)
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct RouteKey {
    chat_id: String,
    harness: HarnessId,
    provider: ap::Provider,
}

struct ChatRoute {
    route: ap::Route,
    /// Changes when a route is minted anew, so a process holding an older
    /// route's URL reads as needing replacement.
    generation: u64,
    used: Instant,
}

/// The engine's proxy and its routes ([`super::Inner::proxy`]).
#[derive(Default)]
pub(super) struct ProxyState {
    proxy: tokio::sync::OnceCell<ap::AccountProxy>,
    routes: Mutex<HashMap<RouteKey, ChatRoute>>,
    generations: AtomicU64,
    /// When each account's usage was last fed from response headers.
    usage_fed: Mutex<HashMap<String, Instant>>,
    /// Test seam: fake upstreams instead of the providers.
    upstreams: Mutex<HashMap<ap::Provider, String>>,
    /// Logins ([`usage_key`]) this engine has seen live. A process it
    /// started may still hold such a login's single-use refresh token after
    /// the live account changed, so the engine never refreshes one of these
    /// slots itself (see [`AgentAccounts::fresh_proxy_token`]).
    ever_live: Mutex<HashSet<String>>,
}

/// A chat's routes for one run ([`AgentAccounts::proxy_routes`]).
pub(super) struct ChatRoutes {
    /// Route variables for the run environment.
    pub env: BTreeMap<String, String>,
    /// The run credential: stable while the routes are.
    pub credential: String,
}

impl AgentAccounts {
    fn from_inner(inner: Arc<Inner>) -> Self {
        Self { inner }
    }

    /// Test seam: send `provider`'s proxied traffic to `base` (a local fake
    /// server) instead. Takes effect when the proxy starts.
    #[doc(hidden)]
    pub fn set_proxy_upstream(&self, provider: ap::Provider, base: impl Into<String>) {
        lock(&self.inner.proxy.upstreams).insert(provider, base.into());
    }

    /// The running proxy, started on first use.
    async fn account_proxy(&self) -> Option<ap::AccountProxy> {
        let state = &self.inner.proxy;
        let started = state
            .proxy
            .get_or_try_init(|| async {
                let upstreams = lock(&state.upstreams).clone();
                let config = ap::ProxyConfig {
                    // A test's fake upstreams are on loopback.
                    system_proxy: upstreams.is_empty(),
                    upstream_overrides: upstreams,
                    ..Default::default()
                };
                let source = Arc::new(Source(Arc::downgrade(&self.inner)));
                let proxy = ap::AccountProxy::start(source, config).await?;
                spawn_event_log(proxy.subscribe());
                Ok::<_, std::io::Error>(proxy)
            })
            .await;
        match started {
            Ok(proxy) => Some(proxy.clone()),
            Err(err) => {
                tracing::warn!(error = %err, "account proxy failed to start; rotating chats run on the live login");
                None
            }
        }
    }

    /// The routes of `chat_id`'s `harness` run, `None` when it talks to
    /// the providers directly on the CLI's own login. A provider is routed
    /// only while rotation is in effect for it (on, with two usable
    /// accounts), the CLI's live login for it is a subscription one, and the
    /// user hasn't pointed it at an endpoint of their own (`cwd`: the run's
    /// project, for project config). Routes are minted on first use, then
    /// reused across process restarts. The run credential names what is
    /// routed and the live accounts, so the process is replaced when either
    /// changes (rotation comes into or out of effect, the user switches the
    /// live account).
    pub(super) async fn proxy_routes(
        &self,
        harness: HarnessId,
        chat_id: &str,
        cwd: Option<&str>,
    ) -> Option<ChatRoutes> {
        let upstreams = proxied_upstreams(harness);
        if upstreams.is_empty() || !self.rotation_enabled(harness) {
            return None;
        }
        let mut routed = Vec::new();
        let mut live_keys = Vec::new();
        for &(provider, pool) in upstreams {
            let in_effect = {
                let this = self.clone();
                let pool = pool.map(String::from);
                tokio::task::spawn_blocking(move || {
                    this.rotation_in_effect(harness, pool.as_deref())
                })
                .await
                .unwrap_or(false)
            };
            if in_effect
                && self.live_login_is_subscription(harness, pool).await
                && !self.custom_endpoint(harness, pool, cwd)
            {
                routed.push((provider, pool));
                live_keys.push(self.proxy_live_key(harness, pool).await.unwrap_or_default());
            }
        }
        if routed.is_empty() {
            return None;
        }
        let proxy = self.account_proxy().await?;
        let state = &self.inner.proxy;
        let mut env = BTreeMap::new();
        let mut generations = Vec::new();
        {
            let mut routes = lock(&state.routes);
            let now = Instant::now();
            routes.retain(|_, r| now.duration_since(r.used) < ROUTE_IDLE);
            for &(provider, _) in &routed {
                let Some(var) = route_var(provider) else {
                    continue;
                };
                let key = RouteKey {
                    chat_id: chat_id.to_string(),
                    harness,
                    provider,
                };
                let entry = routes.entry(key).or_insert_with(|| ChatRoute {
                    route: proxy.register(ap::RouteInfo {
                        chat_id: chat_id.to_string(),
                        harness: harness_slug(harness).to_string(),
                        provider,
                    }),
                    generation: state.generations.fetch_add(1, Ordering::Relaxed),
                    used: now,
                });
                entry.used = now;
                env.insert(var.to_string(), entry.route.base_url().to_string());
                generations.push(format!("{}{}", provider.as_str(), entry.generation));
            }
        }
        // A chat gets its account when its run starts, as with per-process
        // rotation (and the session card can name it). An agent that talks
        // to several providers (Pi) gets one per provider at its first
        // request there.
        if let ([(_, pool)], 1) = (upstreams, routed.len()) {
            let this = self.clone();
            let (chat, pool) = (chat_id.to_string(), pool.map(String::from));
            let _ = tokio::task::spawn_blocking(move || {
                this.assign(harness, &chat, &[], pool.as_deref())
            })
            .await;
        }
        let live = crate::repos::hex(&sha2::Sha256::digest(live_keys.join("\n").as_bytes()))[..12]
            .to_string();
        Some(ChatRoutes {
            env,
            credential: format!("proxy:{chat_id}:{}:{live}", generations.join("-")),
        })
    }

    /// The account key of the CLI's live login for `pool`'s provider. For
    /// OpenCode 2.x it comes from its database ([`Self::opencode_live_entry`]).
    /// Every login seen live is remembered as never to be refreshed here.
    pub(super) async fn proxy_live_key(
        &self,
        harness: HarnessId,
        pool: Option<&str>,
    ) -> Option<String> {
        let key = match (harness, pool) {
            (HarnessId::Opencode, Some(pool)) => self
                .opencode_live_entry(pool)
                .and_then(|entry| super::stores::openai_detected(pool, &entry, None))
                .map(|d| d.account_key),
            _ => self.live_account_key(harness, pool).await,
        };
        if let Some(key) = &key {
            lock(&self.inner.proxy.ever_live).insert(usage_key(harness, key));
        }
        key
    }

    /// Drop `chat_id`'s routes (archived or deleted): its process, if one
    /// still runs, gets 404s and is replaced before its next turn.
    pub fn release_chat_routes(&self, chat_id: &str) {
        lock(&self.inner.proxy.routes).retain(|key, _| key.chat_id != chat_id);
    }

    fn touch_route(&self, key: &RouteKey) {
        if let Some(route) = lock(&self.inner.proxy.routes).get_mut(key) {
            route.used = Instant::now();
        }
    }

    /// The headers that authenticate one request as `slot`'s account (never
    /// the live one: that request carries the CLI's own login).
    pub(super) async fn proxy_headers(
        &self,
        slot: &Slot,
    ) -> Result<BTreeMap<String, String>, ProbeError> {
        let access = self.fresh_proxy_token(slot).await?;
        let mut headers =
            BTreeMap::from([("authorization".to_string(), format!("Bearer {access}"))]);
        if let Some(account) = chatgpt_account_id(slot) {
            headers.insert("chatgpt-account-id".into(), account);
        }
        Ok(headers)
    }

    /// `slot`'s access token, refreshed first when it has less than
    /// [`REFRESH_MARGIN`] left. A refused refresh marks the account signed
    /// out (rotation skips it until it signs in again).
    async fn fresh_proxy_token(&self, slot: &Slot) -> Result<String, ProbeError> {
        let missing = ProbeError::NoCredentials {
            why: NoCredentials::Missing,
        };
        let fresh = |expires: Option<i64>| {
            // Unknown expiry: use it; a refusal reports it.
            expires.is_none_or(|at| at - now_ms() > REFRESH_MARGIN.as_millis() as i64)
        };
        let current = token(slot).ok_or(missing.clone())?;
        if fresh(current.1) {
            return Ok(current.0);
        }
        // Serialized with switches and lists, re-read under the lock: a
        // switch must never install a refresh token this rotates away, and a
        // concurrent refresh may already have stored the result.
        let _ops = self.inner.ops.lock().await;
        let slot = self
            .read_slot(slot.harness, &slot.id)
            .ok_or(missing.clone())?;
        let current = token(&slot).ok_or(missing.clone())?;
        if fresh(current.1) {
            return Ok(current.0);
        }
        // Never a login this engine has seen live: the CLI owns its refresh
        // token, and a process started on it may still hold it after a
        // switch. Its current token is used until it expires.
        self.proxy_live_key(slot.harness, slot.store_key.as_deref())
            .await;
        let was_live =
            lock(&self.inner.proxy.ever_live).contains(&usage_key(slot.harness, &slot.account_key));
        if !self.inner.endpoints.allow_slot_refresh || was_live {
            return Ok(current.0);
        }
        let refreshed = match (slot.harness, slot.store_key.as_deref()) {
            (HarnessId::Grok, _) => self.refresh_grok_slot(&slot).await,
            (_, Some("anthropic")) => self.refresh_anthropic_entry(&slot).await,
            _ => self.refresh_chatgpt_slot(&slot).await,
        };
        match refreshed {
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
            Err(error) => match current.1 {
                Some(at) if at > now_ms() => Ok(current.0),
                None => Ok(current.0),
                _ => Err(error),
            },
        }
    }

    /// Refresh a saved ChatGPT login (a Codex `auth.json`, or an OpenCode /
    /// Pi entry) the way the Codex CLI does, and store the rotated tokens in
    /// the slot. `Ok(None)`: a refresh of this slot is already in flight.
    async fn refresh_chatgpt_slot(&self, slot: &Slot) -> Result<Option<String>, ProbeError> {
        if !lock(&self.inner.inflight_refreshes).insert(slot.id.clone()) {
            return Ok(None);
        }
        let _release = InflightGuard {
            set: &self.inner.inflight_refreshes,
            keys: vec![slot.id.clone()],
        };
        let missing = ProbeError::NoCredentials {
            why: NoCredentials::Missing,
        };
        let codex = slot.harness == HarnessId::Codex;
        let refresh_token = match codex {
            true => slot
                .credentials
                .get("tokens")
                .and_then(|t| str_field(t, "refresh_token")),
            false => str_field(&slot.credentials, "refresh"),
        }
        .ok_or(missing)?;
        let body = probe_json(
            "chatgpt",
            "refresh",
            self.inner
                .http
                .post(format!(
                    "{}/oauth/token",
                    self.inner.endpoints.openai_auth.trim_end_matches('/')
                ))
                .json(&serde_json::json!({
                    "grant_type": "refresh_token",
                    "client_id": super::oauth::OPENAI_CLIENT_ID,
                    "refresh_token": refresh_token,
                })),
        )
        .await
        .map_err(|error| match error {
            // invalid_grant / revoked: the saved login is dead.
            ProbeError::Http { status: 400, .. } => ProbeError::Unauthorized { status: 400 },
            other => other,
        })?;
        let access =
            str_field(&body, "access_token").ok_or_else(|| schema_error("chatgpt", &body))?;
        let rotated = str_field(&body, "refresh_token").unwrap_or(refresh_token);
        let mut credentials = slot.credentials.clone();
        if codex {
            if let Some(tokens) = credentials
                .get_mut("tokens")
                .and_then(|t| t.as_object_mut())
            {
                tokens.insert("access_token".into(), access.clone().into());
                tokens.insert("refresh_token".into(), rotated.into());
                if let Some(id_token) = str_field(&body, "id_token") {
                    tokens.insert("id_token".into(), id_token.into());
                }
            }
            if let Some(auth) = credentials.as_object_mut() {
                auth.insert(
                    "last_refresh".into(),
                    chrono::Utc::now().to_rfc3339().into(),
                );
            }
        } else if let Some(entry) = credentials.as_object_mut() {
            let expires = body
                .get("expires_in")
                .and_then(|v| v.as_i64())
                .map(|secs| now_ms() + secs * 1000)
                .or_else(|| jwt_expiry(&access))
                .unwrap_or_else(|| now_ms() + 3_600_000);
            entry.insert("access".into(), access.clone().into());
            entry.insert("refresh".into(), rotated.into());
            entry.insert("expires".into(), expires.into());
        }
        self.store_refreshed(slot, credentials);
        Ok(Some(access))
    }

    /// Refresh a saved Claude subscription entry of Pi (`{access, refresh,
    /// expires}`) at Claude's token endpoint. `Ok(None)`: already in flight.
    async fn refresh_anthropic_entry(&self, slot: &Slot) -> Result<Option<String>, ProbeError> {
        if !lock(&self.inner.inflight_refreshes).insert(slot.id.clone()) {
            return Ok(None);
        }
        let _release = InflightGuard {
            set: &self.inner.inflight_refreshes,
            keys: vec![slot.id.clone()],
        };
        let refresh_token =
            str_field(&slot.credentials, "refresh").ok_or(ProbeError::NoCredentials {
                why: NoCredentials::Missing,
            })?;
        let body = probe_json(
            "anthropic",
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
            ProbeError::Http { status: 400, .. } => ProbeError::Unauthorized { status: 400 },
            other => other,
        })?;
        let access =
            str_field(&body, "access_token").ok_or_else(|| schema_error("anthropic", &body))?;
        let expires_in = body
            .get("expires_in")
            .and_then(|v| v.as_i64())
            .unwrap_or(3600);
        let mut credentials = slot.credentials.clone();
        if let Some(entry) = credentials.as_object_mut() {
            entry.insert("access".into(), access.clone().into());
            entry.insert(
                "refresh".into(),
                str_field(&body, "refresh_token")
                    .unwrap_or(refresh_token)
                    .into(),
            );
            entry.insert("expires".into(), (now_ms() + expires_in * 1000).into());
        }
        self.store_refreshed(slot, credentials);
        Ok(Some(access))
    }

    fn store_refreshed(&self, slot: &Slot, credentials: serde_json::Value) {
        let mut refreshed = slot.clone();
        refreshed.credentials = credentials;
        refreshed.saved_at = now_ms();
        match self.write_slot(&refreshed) {
            Ok(()) => tracing::info!(slot = %slot.id, "rotated login refreshed"),
            Err(err) => {
                tracing::warn!(slot = %slot.id, error = %err, "refreshed slot write failed")
            }
        }
    }

    /// A successful proxied response's rate-limit headers, as the account's
    /// cached usage (throttled; the plan label of the last probe is kept).
    fn record_proxy_usage(
        &self,
        harness: HarnessId,
        account_id: &str,
        limits: &ap::RateLimitSnapshot,
    ) {
        let windows = usage_windows(limits);
        if windows.is_empty() {
            return;
        }
        {
            let mut fed = lock(&self.inner.proxy.usage_fed);
            let now = Instant::now();
            if fed
                .get(account_id)
                .is_some_and(|at| now.duration_since(*at) < USAGE_FEED_INTERVAL)
            {
                return;
            }
            fed.insert(account_id.to_string(), now);
        }
        let Some(slot) = self.read_slot(harness, account_id) else {
            return;
        };
        let mut usage = lock(&self.inner.usage);
        let entry = usage
            .entry(usage_key(harness, &slot.account_key))
            .or_default();
        let plan_label = entry.usage.as_ref().and_then(|u| u.plan_label.clone());
        entry.record(
            Ok(UsageSnapshot {
                windows,
                plan_label,
            }),
            credentials_fingerprint(&slot.credentials),
            now_ms(),
        );
        self.persist_usage(&usage);
    }
}

/// `slot`'s access token and its expiry (epoch ms, when known).
fn token(slot: &Slot) -> Option<(String, Option<i64>)> {
    let c = &slot.credentials;
    match slot.harness {
        HarnessId::Codex => {
            let access = str_field(c.get("tokens")?, "access_token")?;
            let expires = jwt_expiry(&access);
            Some((access, expires))
        }
        HarnessId::Grok => {
            let expires = str_field(c, "expires_at")
                .and_then(|at| chrono::DateTime::parse_from_rfc3339(&at).ok())
                .map(|at| at.timestamp_millis());
            Some((str_field(c, "key")?, expires))
        }
        _ => Some((
            str_field(c, "access")?,
            c.get("expires")
                .and_then(|v| v.as_i64())
                .filter(|ms| *ms > 0),
        )),
    }
}

/// A JWT's `exp`, as epoch millis.
fn jwt_expiry(token: &str) -> Option<i64> {
    jwt_claims(token)?
        .get("exp")
        .and_then(|v| v.as_i64())
        .map(|secs| secs * 1000)
}

/// Whether the CLI's request (`cli_identity`: the ChatGPT workspace its
/// credential acts in) comes from `slot`'s login. Unknown on either side:
/// assume so (Claude and Grok tokens carry no identity the proxy can read
/// cheaply; the run credential already replaces a process after a switch).
pub(super) fn cli_holds(slot: &Slot, cli_identity: Option<&str>) -> bool {
    match (cli_identity, chatgpt_account_id(slot)) {
        (Some(cli), Some(own)) => cli == own,
        _ => true,
    }
}

/// The ChatGPT workspace the CLI's request acts in: its
/// `ChatGPT-Account-ID`, else the claim in its bearer token.
fn cli_chatgpt_identity(inbound: &reqwest::header::HeaderMap) -> Option<String> {
    let header = |name: &str| {
        inbound
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(String::from)
    };
    header("chatgpt-account-id").or_else(|| {
        let bearer = header("authorization")?;
        jwt_claims(bearer.strip_prefix("Bearer ")?)?
            .get("https://api.openai.com/auth")
            .and_then(|auth| str_field(auth, "chatgpt_account_id"))
    })
}

/// The ChatGPT workspace a ChatGPT login acts in (`ChatGPT-Account-ID`).
fn chatgpt_account_id(slot: &Slot) -> Option<String> {
    let c = &slot.credentials;
    let claim = |jwt: Option<String>| {
        jwt.as_deref().and_then(jwt_claims).and_then(|claims| {
            claims
                .get("https://api.openai.com/auth")
                .and_then(|auth| str_field(auth, "chatgpt_account_id"))
        })
    };
    match (slot.harness, slot.store_key.as_deref()) {
        (HarnessId::Codex, _) => {
            let tokens = c.get("tokens")?;
            str_field(tokens, "account_id").or_else(|| claim(str_field(tokens, "id_token")))
        }
        (HarnessId::Opencode | HarnessId::Pi, Some("openai" | "openai-codex")) => {
            str_field(c, "accountId").or_else(|| claim(str_field(c, "access")))
        }
        _ => None,
    }
}

/// The meters a response's rate-limit headers fill, labelled like the usage
/// probes' (Claude's 5-hour "Session" and "Week"; Codex windows by span).
fn usage_windows(limits: &ap::RateLimitSnapshot) -> Vec<AgentUsageWindow> {
    limits
        .windows
        .iter()
        .filter_map(|w| {
            let used = w.used_percent?;
            let label = match w.name.as_str() {
                "five_hour" => "Session".to_string(),
                "seven_day" => "Week".to_string(),
                _ => zeron_proto::plan_usage::codex_window_label(w.window_minutes? as i64 * 60)
                    .to_string(),
            };
            Some(AgentUsageWindow {
                label,
                used_fraction: (used / 100.0).clamp(0.0, 1.0) as f32,
                resets_at: w.resets_at,
            })
        })
        .collect()
}

fn spawn_event_log(mut events: tokio::sync::broadcast::Receiver<ap::ProxyEvent>) {
    tokio::spawn(async move {
        loop {
            match events.recv().await {
                Ok(ap::ProxyEvent::AccountSwitched {
                    route,
                    from,
                    to,
                    reason,
                }) => tracing::info!(chat = %route.chat_id, harness = %route.harness,
                    from = ?from, to = %to, reason = ?reason, "proxied chat switched accounts"),
                Ok(ap::ProxyEvent::Exhausted { route, tried }) => tracing::info!(
                    chat = %route.chat_id, harness = %route.harness, tried = tried.len(),
                    "every account of a proxied chat is out; the provider's error went through"),
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
            }
        }
    });
}

/// The engine as the proxy's credential source. Weak: the proxy lives inside
/// the accounts it asks.
struct Source(Weak<Inner>);

impl Source {
    fn accounts(&self) -> Option<AgentAccounts> {
        self.0.upgrade().map(AgentAccounts::from_inner)
    }
}

#[async_trait::async_trait]
impl ap::CredentialSource for Source {
    async fn lease(
        &self,
        route: &ap::RouteInfo,
        inbound: &reqwest::header::HeaderMap,
        exclude: &[ap::AccountId],
    ) -> Result<ap::Lease, ap::LeaseError> {
        let accounts = self
            .accounts()
            .ok_or_else(|| ap::LeaseError::Failed("the engine is stopping".into()))?;
        let harness = parse_harness(&route.harness).ok_or(ap::LeaseError::NoAccount)?;
        let pool = pool_of(harness, route.provider).ok_or(ap::LeaseError::NoAccount)?;
        accounts.touch_route(&RouteKey {
            chat_id: route.chat_id.clone(),
            harness,
            provider: route.provider,
        });
        let exclude: Vec<String> = exclude.iter().map(|a| a.as_str().to_string()).collect();
        let cli_identity = match route.provider {
            ap::Provider::ChatGpt => cli_chatgpt_identity(inbound),
            _ => None,
        };
        match accounts
            .proxy_lease(
                &route.chat_id,
                harness,
                pool,
                &exclude,
                cli_identity.as_deref(),
            )
            .await
        {
            ProxyLease::Live { account_id } => Ok(ap::Lease::passthrough(
                account_id.unwrap_or_else(|| LIVE.to_string()),
            )),
            ProxyLease::Account {
                account_id,
                headers,
            } => {
                let mut lease = ap::Lease::new(account_id);
                for (name, value) in headers {
                    let name = reqwest::header::HeaderName::from_bytes(name.as_bytes())
                        .map_err(|_| ap::LeaseError::Failed("bad header name".into()))?;
                    lease = lease
                        .header(name, &value)
                        .map_err(|_| ap::LeaseError::Failed("bad header value".into()))?;
                }
                Ok(lease)
            }
            ProxyLease::NoneLeft => Err(ap::LeaseError::NoAccount),
        }
    }

    fn report(&self, route: &ap::RouteInfo, account: &ap::AccountId, outcome: ap::Outcome) {
        // The CLI's own login while rotation is off: nothing to record.
        if account.as_str() == LIVE {
            return;
        }
        let (Some(accounts), Some(harness)) = (self.accounts(), parse_harness(&route.harness))
        else {
            return;
        };
        let account = account.as_str().to_string();
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        match outcome {
            // Slot and usage files: off the async workers.
            ap::Outcome::Ok { rate_limits } => {
                runtime.spawn_blocking(move || {
                    accounts.record_proxy_usage(harness, &account, &rate_limits)
                });
            }
            ap::Outcome::LimitReached { resets_at, .. } => {
                runtime.spawn_blocking(move || accounts.report_limit(harness, &account, resets_at));
            }
            // A swapped credential was refused (the CLI's own never is
            // reported); still, never mark the live login signed out.
            ap::Outcome::Rejected { .. } => {
                let pool = pool_of(harness, route.provider).flatten();
                runtime.spawn(async move {
                    let live = accounts.proxy_live_key(harness, pool).await;
                    let slot = accounts.read_slot(harness, &account);
                    if slot.is_some_and(|slot| Some(slot.account_key) != live) {
                        accounts.report_rejected(harness, &account);
                    }
                });
            }
            ap::Outcome::IncompatibleHistory { status } => tracing::warn!(
                chat = %route.chat_id, account = %account, status,
                "the provider refused reasoning another account produced"
            ),
        }
    }

    async fn incompatible_history(
        &self,
        route: &ap::RouteInfo,
        account: &ap::AccountId,
    ) -> ap::HistoryRecovery {
        let (Some(accounts), Some(harness)) = (self.accounts(), parse_harness(&route.harness))
        else {
            return ap::HistoryRecovery::PassThrough;
        };
        let Some(pool) = pool_of(harness, route.provider) else {
            return ap::HistoryRecovery::PassThrough;
        };
        let (chat, account, pool) = (
            route.chat_id.clone(),
            account.as_str().to_string(),
            pool.map(String::from),
        );
        let answer = tokio::task::spawn_blocking(move || {
            accounts.history_recovery(harness, &chat, pool.as_deref(), &account)
        })
        .await
        .unwrap_or(HistoryAnswer::PassThrough);
        match answer {
            HistoryAnswer::PassThrough => ap::HistoryRecovery::PassThrough,
            HistoryAnswer::Retry => ap::HistoryRecovery::Retry,
            HistoryAnswer::LimitReached { resets_at } => ap::HistoryRecovery::LimitReached {
                resets_at,
                window: None,
            },
        }
    }
}

/// The provider pool a route to `provider` of `harness` draws from (`None`:
/// not a proxied upstream of it).
fn pool_of(harness: HarnessId, provider: ap::Provider) -> Option<Option<&'static str>> {
    proxied_upstreams(harness)
        .iter()
        .find(|(p, _)| *p == provider)
        .map(|(_, pool)| *pool)
}

mod routing;

#[cfg(test)]
mod tests;
