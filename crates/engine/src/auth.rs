//! Auth — the engine owns the WorkOS session for its device (feature-inventory §3.7,
//! ARCHITECTURE §5). Port of zeron's `apps/backend/src/auth.ts`.
//!
//! The engine is a public client: it builds the AuthKit authorize URL itself but
//! delegates the secret-bearing **code exchange** and **refresh** to the edge Worker
//! (`/auth/exchange`, `/auth/refresh` — the WorkOS API key lives only there).
//!
//! Two modes:
//! - **Dev** (no WorkOS client id configured, or the edge reports `auth: "dev"`): always
//!   signed in; the bearer IS the configured user id (current M2/M3 behavior).
//! - **WorkOS**: authorization-code flow. Headed devices use a loopback callback server
//!   on an ephemeral port; headless devices use the paste-code flow (the redirect is the
//!   edge's hosted `/auth/cli/callback` page, which shows `state.code` to paste back via
//!   stdin or the `CompleteSignIn` RPC). The refresh token is persisted 0600 in the data
//!   dir; access tokens are cached with dual-clock expiry (monotonic AND wall, whichever
//!   aged more — see [`AccessEntry`]) and refreshed on demand plus by a background loop,
//!   so the device-room relay and room clients always dial with a live `?token=`, even
//!   on the first redial after a laptop wakes from sleep. Org onboarding: an org-less session is `NeedsOrganization`; `SelectOrg`
//!   runs an org-scoped refresh and the state follows the returned token's `org_id`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::{Duration, Instant};

use futures::FutureExt;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::watch;

use crate::EngineError;
use crate::http_error::describe_http_error;
use zeron_rpc::TokenError;

const SIGN_IN_TTL: Duration = Duration::from_secs(15 * 60);
/// Refresh when the cached token has less than this much life left.
const TOKEN_SLACK: Duration = Duration::from_secs(30);
const HTTP_TIMEOUT: Duration = Duration::from_secs(15);
const REFRESH_RETRY_BASE: Duration = Duration::from_secs(1);
/// DNS can recover without an OS path event. Keep polling even at the cap.
const REFRESH_RETRY_CAP: Duration = Duration::from_secs(5);

type RefreshFlight =
    futures::future::Shared<futures::future::BoxFuture<'static, Result<Option<String>, String>>>;

#[derive(Default)]
struct RefreshRetry {
    generation: u64,
    failures: u32,
    failure: Option<RefreshFailure>,
}

struct RefreshFailure {
    message: String,
    at: tokio::time::Instant,
    wall: std::time::SystemTime,
    delay: Duration,
}

impl RefreshFailure {
    fn remaining(&self) -> Duration {
        let wall = self.wall.elapsed().unwrap_or(Duration::ZERO);
        self.delay.saturating_sub(self.at.elapsed().max(wall))
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

// ---------------------------------------------------------------------------
// Wire types (feature-inventory §2 AuthRpc)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthUser {
    pub id: String,
    pub email: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrgMembership {
    pub id: String,
    pub organization_id: String,
    pub name: String,
}

/// AuthStatus stream payload (`SignedOut | NeedsOrganization{user} |
/// SignedIn{user, orgId?}`). Serializes as the canonical [`zeron_proto::AuthState`]
/// wire shape (`{"state": "signedIn", …}`) so every client parses one form.
#[derive(Debug, Clone, PartialEq)]
pub enum AuthState {
    SignedOut,
    NeedsOrganization {
        user: AuthUser,
    },
    SignedIn {
        user: AuthUser,
        org_id: Option<String>,
    },
}

impl AuthState {
    pub fn is_signed_in(&self) -> bool {
        matches!(self, AuthState::SignedIn { .. })
    }

    pub fn org_id(&self) -> Option<&str> {
        match self {
            AuthState::SignedIn { org_id, .. } => org_id.as_deref(),
            _ => None,
        }
    }

    pub fn user(&self) -> Option<&AuthUser> {
        match self {
            AuthState::SignedIn { user, .. } | AuthState::NeedsOrganization { user } => Some(user),
            AuthState::SignedOut => None,
        }
    }

    /// The proto wire twin — the one shape the engine emits over AuthStatus.
    pub fn to_proto(&self) -> zeron_proto::AuthState {
        let profile = |user: &AuthUser| zeron_proto::UserProfile {
            id: user.id.clone(),
            email: user.email.clone(),
            name: user.name.clone(),
        };
        match self {
            AuthState::SignedOut => zeron_proto::AuthState::SignedOut,
            AuthState::NeedsOrganization { user } => zeron_proto::AuthState::NeedsOrganization {
                user: profile(user),
            },
            AuthState::SignedIn { user, org_id } => zeron_proto::AuthState::SignedIn {
                user: profile(user),
                org_id: org_id.clone(),
            },
        }
    }
}

impl Serialize for AuthState {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.to_proto().serialize(serializer)
    }
}

// ---------------------------------------------------------------------------
// Config + construction
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct AuthConfig {
    /// Edge base URL (`/auth/*` routes).
    pub edge_url: String,
    /// Data dir for the persisted session (`session.json`, 0600).
    pub data_dir: PathBuf,
    /// WorkOS client id; `None` = dev mode.
    pub workos_client_id: Option<String>,
    /// WorkOS API base (authorize URL host).
    pub workos_api_base: String,
    /// Dev-mode bearer/user id (mirrors the old `ZERON_EDGE_TOKEN` behavior).
    pub dev_user_id: String,
    /// Loopback callback port; `None` = ephemeral.
    pub callback_port: Option<u16>,
}

impl AuthConfig {
    pub fn new(edge_url: impl Into<String>, data_dir: impl Into<PathBuf>) -> Self {
        Self {
            edge_url: edge_url.into(),
            data_dir: data_dir.into(),
            workos_client_id: None,
            workos_api_base: "https://api.workos.com".into(),
            dev_user_id: "dev-user".into(),
            callback_port: None,
        }
    }
}

/// The persisted session (refresh token + user + last org scope).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredSession {
    refresh_token: String,
    user: AuthUser,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    org_id: Option<String>,
}

/// Access-token cache. Expiry ages the token's own lifetime (`exp - iat`) by
/// BOTH clocks, pessimistically. Monotonic alone (`Instant`) freezes across
/// system sleep (macOS `mach_absolute_time` and Linux `CLOCK_MONOTONIC` both
/// exclude suspend), so a laptop waking from hours of sleep presented a
/// wall-expired token that still read "fresh" — every room/relay redial got a
/// 401 with the same stale bearer and sync never recovered (user report).
/// Wall clock alone breaks under skewed device clocks (`exp` vs local time);
/// the elapsed-since-issue reading is skew-immune, and a BACKWARD wall step
/// (NTP correction) degrades harmlessly to the monotonic reading.
struct AccessEntry {
    token: String,
    ttl: Duration,
    got_at: Instant,
    got_wall: std::time::SystemTime,
}

impl AccessEntry {
    fn fresh(token: String) -> Self {
        let ttl = jwt_claims(&token)
            .and_then(|c| match (c.exp, c.iat) {
                (Some(exp), Some(iat)) if exp > iat => {
                    Some(Duration::from_secs((exp - iat) as u64))
                }
                _ => None,
            })
            .unwrap_or(Duration::from_secs(240));
        Self {
            token,
            ttl,
            got_at: Instant::now(),
            got_wall: std::time::SystemTime::now(),
        }
    }

    fn remaining(&self) -> Duration {
        let monotonic = self.got_at.elapsed();
        let wall = std::time::SystemTime::now()
            .duration_since(self.got_wall)
            .unwrap_or(Duration::ZERO);
        self.ttl.saturating_sub(monotonic.max(wall))
    }
}

struct AuthInner {
    config: AuthConfig,
    /// `Some(client_id)` = WorkOS mode; `None` = dev mode.
    workos: Option<String>,
    /// Whether construction loaded a parseable WorkOS session. This is an
    /// immutable startup fact: refresh or sign-out must not rewrite it.
    loaded_workos_session: bool,
    http: reqwest::Client,
    state_tx: watch::Sender<AuthState>,
    token_tx: watch::Sender<u64>,
    stored: Mutex<Option<StoredSession>>,
    access: Mutex<Option<AccessEntry>>,
    /// Pending OAuth states plus the cancellation generation that fences code
    /// exchanges already in flight when sign-out occurs.
    sign_in: Mutex<SignInLifecycle>,
    /// Single-flight refresh: WorkOS refresh tokens are single-use (rotated per
    /// exchange); two concurrent refreshes would race and could revoke the session.
    refresh_gate: tokio::sync::Mutex<()>,
    refresh_flight: Mutex<Option<RefreshFlight>>,
    /// Failed refreshes are shared across every HTTP/WS token consumer, not
    /// just delayed by the background loop. Resettable without waiting on HTTP.
    refresh_retry: Mutex<RefreshRetry>,
    retry_tx: watch::Sender<u64>,
    /// Loopback callback listener port, bound lazily on the first headed sign-in.
    loopback: tokio::sync::Mutex<Option<u16>>,
}

#[derive(Default)]
struct SignInLifecycle {
    generation: u64,
    pending: HashMap<String, Instant>,
}

/// The auth service — cheap to clone by `Arc`.
#[derive(Clone)]
pub struct Auth {
    inner: Arc<AuthInner>,
}

impl Auth {
    /// Build from config: dev mode unless a WorkOS client id is configured.
    pub fn new(config: AuthConfig) -> Self {
        let workos = config
            .workos_client_id
            .clone()
            .filter(|s| !s.trim().is_empty());
        let session_file = config.data_dir.join("session.json");
        let stored: Option<StoredSession> = if workos.is_some() {
            std::fs::read_to_string(&session_file)
                .ok()
                .and_then(|raw| serde_json::from_str(&raw).ok())
        } else {
            None
        };
        let initial = match (&workos, &stored) {
            (None, _) => AuthState::SignedIn {
                user: AuthUser {
                    id: config.dev_user_id.clone(),
                    email: config.dev_user_id.clone(),
                    name: None,
                },
                org_id: None,
            },
            (Some(_), Some(session)) => state_for(session.user.clone(), session.org_id.clone()),
            (Some(_), None) => AuthState::SignedOut,
        };
        let loaded_workos_session = workos.is_some() && stored.is_some();
        let (state_tx, _) = watch::channel(initial);
        let (token_tx, _) = watch::channel(0);
        let (retry_tx, _) = watch::channel(0);
        let http = reqwest::Client::builder()
            .timeout(HTTP_TIMEOUT)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            inner: Arc::new(AuthInner {
                config,
                workos,
                loaded_workos_session,
                http,
                state_tx,
                token_tx,
                stored: Mutex::new(stored),
                access: Mutex::new(None),
                sign_in: Mutex::new(SignInLifecycle::default()),
                refresh_gate: tokio::sync::Mutex::new(()),
                refresh_flight: Mutex::new(None),
                refresh_retry: Mutex::new(RefreshRetry::default()),
                retry_tx,
                loopback: tokio::sync::Mutex::new(None),
            }),
        }
    }

    /// Like [`Auth::new`], but additionally probes `{edge}/health`: an edge running in
    /// dev auth mode forces dev mode even when a client id is configured (matching the
    /// edge's "bearer = user id" verification).
    pub async fn detect(mut config: AuthConfig) -> Self {
        if config.workos_client_id.is_some() {
            #[derive(Deserialize)]
            struct Health {
                auth: Option<String>,
            }
            let url = format!("{}/health", config.edge_url.trim_end_matches('/'));
            let probe = async {
                reqwest::Client::new()
                    .get(&url)
                    .timeout(Duration::from_secs(3))
                    .send()
                    .await
                    .ok()?
                    .json::<Health>()
                    .await
                    .ok()
            };
            if let Some(health) = probe.await
                && health.auth.as_deref() == Some("dev")
            {
                tracing::info!("auth: edge is in dev mode — using dev bearer");
                config.workos_client_id = None;
            }
        }
        Self::new(config)
    }

    pub fn workos_enabled(&self) -> bool {
        self.inner.workos.is_some()
    }

    /// True when construction loaded a parseable persisted WorkOS session.
    /// The value stays true even if a later refresh revokes that session.
    pub fn loaded_workos_session(&self) -> bool {
        self.inner.loaded_workos_session
    }

    /// Live auth status (current value + changes).
    pub fn watch_state(&self) -> watch::Receiver<AuthState> {
        self.inner.state_tx.subscribe()
    }

    pub fn state(&self) -> AuthState {
        self.inner.state_tx.borrow().clone()
    }

    /// The signed-in user id — the identity that scopes workspace rooms
    /// (`ws3/{orgId}/{userId}`) and local storage (`orgs/{org}/{user}/`).
    /// Dev mode mirrors the edge's dev-bearer parsing (`user@org` → `user`,
    /// a bare token IS the user id). `None` = signed out (WorkOS only).
    pub fn user_id(&self) -> Option<String> {
        if self.inner.workos.is_none() {
            let dev = &self.inner.config.dev_user_id;
            return Some(dev.split('@').next().unwrap_or(dev).to_string());
        }
        self.state().user().map(|u| u.id.clone())
    }

    /// Current bearer. Network failures preserve the session and return
    /// `TemporarilyUnavailable`; only absent/revoked credentials are `SignedOut`.
    /// Dev mode: the configured user id. WorkOS: cached access token, refreshed when
    /// it has under 30s left.
    pub async fn access_token(&self) -> Result<String, TokenError> {
        if self.inner.workos.is_none() {
            return Ok(self.inner.config.dev_user_id.clone());
        }
        if let Some(entry) = &*lock(&self.inner.access)
            && entry.remaining() > TOKEN_SLACK
        {
            return Ok(entry.token.clone());
        }
        let result = self.refresh(None).await;
        // Sign-in/out may replace the session while a shared request is in
        // flight. Report the current credentials, never a discarded response's
        // `None` as a revocation of the replacement session.
        let _session = lock(&self.inner.sign_in);
        if lock(&self.inner.stored).is_none() {
            return Err(TokenError::SignedOut);
        }
        if let Some(entry) = &*lock(&self.inner.access)
            && entry.remaining() > TOKEN_SLACK
        {
            return Ok(entry.token.clone());
        }
        result
            .map_err(|e| TokenError::TemporarilyUnavailable(e.to_string()))?
            .ok_or_else(|| {
                TokenError::TemporarilyUnavailable("session changed during refresh; retry".into())
            })
    }

    /// Allow one fresh attempt after a connectivity hint or an explicit Retry.
    /// Consumers still serialize on the refresh gate and share its outcome.
    pub fn retry_refresh(&self) {
        let mut retry = lock(&self.inner.refresh_retry);
        retry.generation = retry.generation.wrapping_add(1);
        retry.failures = 0;
        retry.failure = None;
        self.inner.retry_tx.send_replace(retry.generation);
    }

    /// Sleep-until-near-expiry refresh loop so long-lived dials (relay, rooms) always
    /// have a live token to present on reconnect. No-op task in dev mode.
    pub fn spawn_refresh_loop(&self) -> tokio::task::JoinHandle<()> {
        let auth = self.clone();
        tokio::spawn(async move {
            if auth.inner.workos.is_none() {
                return;
            }
            let mut state_rx = auth.watch_state();
            let mut wake = zeron_sync::wake::subscribe();
            let mut online = zeron_sync::wake::subscribe_online();
            let mut retry_rx = auth.inner.retry_tx.subscribe();
            loop {
                if !state_rx.borrow().is_signed_in() {
                    if state_rx.changed().await.is_err() {
                        return;
                    }
                    continue;
                }
                let remaining = lock(&auth.inner.access)
                    .as_ref()
                    .map(AccessEntry::remaining)
                    .unwrap_or(Duration::ZERO);
                let wait = remaining.saturating_sub(TOKEN_SLACK);
                if wait > Duration::ZERO {
                    // Re-evaluate at least once a minute rather than parking
                    // on one long timer: tokio timers ride the monotonic
                    // clock, which excludes system suspend — a laptop waking
                    // from sleep would otherwise wait the WHOLE original
                    // duration again before noticing the (wall-expired) token.
                    let wait = wait.min(Duration::from_secs(60));
                    tokio::select! {
                        _ = tokio::time::sleep(wait) => { continue; }
                        changed = state_rx.changed() => {
                            if changed.is_err() { return; }
                            continue;
                        }
                        // Wake: the cached token is almost certainly
                        // wall-expired — refresh NOW so the reconnecting
                        // rooms/relays dial with live credentials instead of
                        // discovering staleness one 401 at a time.
                        _ = wake.recv() => {}
                    }
                }
                if auth.refresh(None).await.is_err() {
                    // A failed refresh is usually the network, not WorkOS —
                    // retry the moment connectivity returns (online bus)
                    // instead of always waiting out the full pause.
                    while online.try_recv().is_ok() {}
                    let wait = lock(&auth.inner.refresh_retry)
                        .failure
                        .as_ref()
                        .map(RefreshFailure::remaining)
                        .unwrap_or(Duration::ZERO);
                    tokio::select! {
                        _ = tokio::time::sleep(wait) => {}
                        _ = wake.recv() => { auth.retry_refresh(); }
                        _ = online.recv() => { auth.retry_refresh(); }
                        _ = retry_rx.changed() => {}
                    }
                }
            }
        })
    }

    // -- sign-in flows ------------------------------------------------------

    /// Begin a headed sign-in: returns the AuthKit authorize URL redirecting to our
    /// loopback callback server (bound lazily on an ephemeral port).
    pub async fn start_sign_in(&self) -> Result<String, EngineError> {
        if self.inner.workos.is_none() {
            return Ok(String::new()); // dev mode: nothing to do (TS parity)
        }
        let port = self.ensure_loopback().await?;
        Ok(self.begin_sign_in(&format!("http://127.0.0.1:{port}/callback")))
    }

    /// Begin a headless sign-in: the redirect is the edge's hosted paste-code page —
    /// nothing ever redirects to this machine, so the browser can be anywhere.
    pub fn start_headless_sign_in(&self) -> String {
        if self.inner.workos.is_none() {
            return String::new();
        }
        let edge = self.inner.config.edge_url.trim_end_matches('/');
        self.begin_sign_in(&format!("{edge}/auth/cli/callback"))
    }

    /// Finish a headless sign-in with the pasted `state.code` string. The state half
    /// must match a sign-in started HERE (same CSRF discipline as the loopback flow).
    pub async fn complete_sign_in(&self, pasted: &str) -> Result<(), EngineError> {
        if self.inner.workos.is_none() {
            return Ok(());
        }
        let trimmed = pasted.trim();
        let (state, code) = trimmed.split_once('.').unwrap_or(("", ""));
        if state.is_empty() || code.is_empty() {
            return Err(EngineError::Other(
                "invalid or expired sign-in code — start sign-in again and paste the full code"
                    .into(),
            ));
        }
        let Some(generation) = self.take_pending(state) else {
            return Err(EngineError::Other(
                "invalid or expired sign-in code — start sign-in again and paste the full code"
                    .into(),
            ));
        };
        let result = self.exchange_code(code).await?;
        self.finish_sign_in(result, generation)
    }

    pub fn sign_out(&self) {
        let mut sign_in = lock(&self.inner.sign_in);
        self.clear_session(&mut sign_in);
    }

    fn clear_session(&self, sign_in: &mut SignInLifecycle) {
        sign_in.generation = sign_in.generation.wrapping_add(1);
        sign_in.pending.clear();
        *lock(&self.inner.stored) = None;
        *lock(&self.inner.access) = None;
        self.retry_refresh();
        self.persist::<&StoredSession>(None);
        self.inner.state_tx.send_replace(AuthState::SignedOut);
        self.inner
            .token_tx
            .send_modify(|epoch| *epoch = epoch.wrapping_add(1));
    }

    // -- organizations ------------------------------------------------------

    pub async fn list_orgs(&self) -> Result<Vec<OrgMembership>, EngineError> {
        if self.inner.workos.is_none() {
            return Ok(Vec::new());
        }
        #[derive(Deserialize)]
        struct Orgs {
            #[serde(default)]
            orgs: Vec<OrgMembership>,
        }
        let body: Orgs = self
            .authed_json(reqwest::Method::GET, "/auth/orgs", None)
            .await?;
        Ok(body.orgs)
    }

    /// Create an org (the edge makes us its first admin member) and scope to it.
    pub async fn create_org(&self, name: &str) -> Result<(), EngineError> {
        if self.inner.workos.is_none() {
            return Ok(());
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Created {
            organization_id: String,
        }
        let created: Created = self
            .authed_json(
                reqwest::Method::POST,
                "/auth/orgs",
                Some(serde_json::json!({ "name": name })),
            )
            .await?;
        self.select_org(&created.organization_id).await
    }

    /// Scope the session to an org: one refresh with `organizationId`; the state follows
    /// the returned token's `org_id` claim.
    pub async fn select_org(&self, organization_id: &str) -> Result<(), EngineError> {
        if self.inner.workos.is_none() {
            return Ok(());
        }
        let token = self.refresh(Some(organization_id)).await?;
        let scoped = token
            .as_deref()
            .and_then(jwt_claims)
            .and_then(|c| c.org_id)
            .is_some_and(|org| org == organization_id);
        if !scoped {
            return Err(EngineError::Other(
                "could not switch to that workspace — you may no longer be a member".into(),
            ));
        }
        Ok(())
    }

    // -- internals ----------------------------------------------------------

    fn begin_sign_in(&self, redirect_uri: &str) -> String {
        let state = uuid::Uuid::new_v4().to_string();
        {
            let mut sign_in = lock(&self.inner.sign_in);
            let cutoff = Instant::now();
            sign_in
                .pending
                .retain(|_, at| cutoff.duration_since(*at) < SIGN_IN_TTL);
            sign_in.pending.insert(state.clone(), cutoff);
        }
        let client_id = self.inner.workos.clone().unwrap_or_default();
        format!(
            "{}/user_management/authorize?response_type=code&client_id={}&redirect_uri={}&provider=authkit&state={}",
            self.inner.config.workos_api_base.trim_end_matches('/'),
            url_encode(&client_id),
            url_encode(redirect_uri),
            state
        )
    }

    /// Consume a pending sign-in state and capture its cancellation generation.
    /// `None` means unknown/expired (CSRF check).
    fn take_pending(&self, state: &str) -> Option<u64> {
        let mut sign_in = lock(&self.inner.sign_in);
        let now = Instant::now();
        sign_in
            .pending
            .retain(|_, at| now.duration_since(*at) < SIGN_IN_TTL);
        sign_in.pending.remove(state)?;
        Some(sign_in.generation)
    }

    async fn exchange_code(&self, code: &str) -> Result<SignInResult, EngineError> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct WireUser {
            id: String,
            email: String,
            #[serde(default)]
            first_name: Option<String>,
            #[serde(default)]
            last_name: Option<String>,
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Exchange {
            user: WireUser,
            access_token: String,
            refresh_token: String,
        }
        let url = format!(
            "{}/auth/exchange",
            self.inner.config.edge_url.trim_end_matches('/')
        );
        let res = self
            .inner
            .http
            .post(&url)
            .json(&serde_json::json!({ "code": code }))
            .send()
            .await
            .map_err(|e| {
                EngineError::Other(format!(
                    "the edge is unreachable: {}",
                    describe_http_error(e)
                ))
            })?;
        if !res.status().is_success() {
            return Err(EngineError::Other(format!(
                "sign-in failed during token exchange ({}) — the code may have expired; start again",
                res.status().as_u16()
            )));
        }
        let body: Exchange = res.json().await.map_err(|e| {
            EngineError::Other(format!(
                "malformed exchange response: {}",
                describe_http_error(e)
            ))
        })?;
        let name = [body.user.first_name, body.user.last_name]
            .into_iter()
            .flatten()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        Ok(SignInResult {
            user: AuthUser {
                id: body.user.id,
                email: body.user.email,
                name: (!name.is_empty()).then_some(name),
            },
            access_token: body.access_token,
            refresh_token: body.refresh_token,
        })
    }

    fn finish_sign_in(&self, result: SignInResult, generation: u64) -> Result<(), EngineError> {
        // Serialize the final commit with sign-out. A callback can consume its
        // OAuth state and spend time exchanging the code; if cancellation wins
        // during that await, its old generation must never restore credentials.
        let sign_in = lock(&self.inner.sign_in);
        if sign_in.generation != generation {
            return Err(EngineError::Other(
                "sign-in was canceled — start again from Zeron".into(),
            ));
        }
        let org_id = jwt_claims(&result.access_token).and_then(|c| c.org_id);
        *lock(&self.inner.access) = Some(AccessEntry::fresh(result.access_token));
        let session = StoredSession {
            refresh_token: result.refresh_token,
            user: result.user.clone(),
            org_id: org_id.clone(),
        };
        self.persist(Some(&session));
        *lock(&self.inner.stored) = Some(session);
        self.retry_refresh();
        tracing::info!(email = %result.user.email, org = org_id.as_deref().unwrap_or("<none>"),
            "auth: signed in");
        self.inner
            .state_tx
            .send_replace(state_for(result.user, org_id));
        self.inner
            .token_tx
            .send_modify(|epoch| *epoch = epoch.wrapping_add(1));
        Ok(())
    }

    /// Refresh the session (single-flight). `organization_id` migrates the WorkOS
    /// session to that org; routine refreshes keep the current scope. Returns the new
    /// access token, `None` when signed out / the refresh could not run.
    async fn refresh(&self, organization_id: Option<&str>) -> Result<Option<String>, EngineError> {
        if organization_id.is_some() {
            return self.refresh_serialized(organization_id).await;
        }
        let flight = {
            let mut active = lock(&self.inner.refresh_flight);
            if let Some(flight) = active.as_ref() {
                flight.clone()
            } else {
                let auth = self.clone();
                // A room's connect timeout must not cancel a rotating refresh
                // token exchange that sibling rooms are also waiting for.
                let task = tokio::spawn(async move {
                    let result = auth
                        .refresh_serialized(None)
                        .await
                        .map_err(|e| e.to_string());
                    *lock(&auth.inner.refresh_flight) = None;
                    result
                });
                let flight = async move {
                    task.await
                        .map_err(|e| format!("refresh task stopped: {e}"))?
                }
                .boxed()
                .shared();
                *active = Some(flight.clone());
                flight
            }
        };
        flight.await.map_err(EngineError::Other)
    }

    async fn refresh_serialized(
        &self,
        organization_id: Option<&str>,
    ) -> Result<Option<String>, EngineError> {
        let _gate = self.inner.refresh_gate.lock().await;
        // Re-check under the gate: the refresh we queued behind may have done the work.
        if organization_id.is_none()
            && let Some(entry) = &*lock(&self.inner.access)
            && entry.remaining() > TOKEN_SLACK
        {
            return Ok(Some(entry.token.clone()));
        }
        let generation = {
            let retry = lock(&self.inner.refresh_retry);
            // An explicit org switch must not reuse another scope's error.
            if organization_id.is_none()
                && let Some(failure) = &retry.failure
                && !failure.remaining().is_zero()
            {
                return Err(EngineError::Other(failure.message.clone()));
            }
            retry.generation
        };
        let result = self.refresh_locked(organization_id).await;
        let mut retry = lock(&self.inner.refresh_retry);
        // A Retry/sign-in/sign-out during the request invalidates its cooldown.
        if retry.generation == generation {
            match &result {
                Err(err) if organization_id.is_none() => {
                    retry.failures = retry.failures.saturating_add(1);
                    let backoff =
                        REFRESH_RETRY_BASE.saturating_mul(1 << (retry.failures - 1).min(8));
                    let jitter =
                        Duration::from_millis(u64::from(uuid::Uuid::new_v4().as_bytes()[0]));
                    let delay = (backoff + jitter).min(REFRESH_RETRY_CAP);
                    tracing::warn!(error = %err, retry_ms = delay.as_millis() as u64,
                        "auth: refresh failed; cooling down");
                    retry.failure = Some(RefreshFailure {
                        message: err.to_string(),
                        at: tokio::time::Instant::now(),
                        wall: std::time::SystemTime::now(),
                        delay,
                    });
                }
                Ok(_) => {
                    retry.failures = 0;
                    retry.failure = None;
                }
                Err(_) => {}
            }
        }
        result
    }

    /// Called only while holding the gate, including org-scoped refreshes.
    async fn refresh_locked(
        &self,
        organization_id: Option<&str>,
    ) -> Result<Option<String>, EngineError> {
        let (generation, refresh_token) = {
            let sign_in = lock(&self.inner.sign_in);
            let Some(refresh_token) = lock(&self.inner.stored)
                .as_ref()
                .map(|s| s.refresh_token.clone())
            else {
                return Ok(None);
            };
            (sign_in.generation, refresh_token)
        };
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct RefreshBody<'a> {
            refresh_token: &'a str,
            #[serde(skip_serializing_if = "Option::is_none")]
            organization_id: Option<&'a str>,
        }
        let url = format!(
            "{}/auth/refresh",
            self.inner.config.edge_url.trim_end_matches('/')
        );
        let res = self
            .inner
            .http
            .post(&url)
            .json(&RefreshBody {
                refresh_token: &refresh_token,
                organization_id,
            })
            .send()
            .await;
        let res = match res {
            Ok(res) => res,
            Err(err) => {
                // Network failure is transient: keep the session, but surface the
                // error so the background loop applies its retry delay.
                return Err(EngineError::Other(format!(
                    "could not reach the edge during refresh: {}",
                    describe_http_error(err)
                )));
            }
        };
        let status = res.status().as_u16();
        if (400..500).contains(&status) && !matches!(status, 408 | 429) && organization_id.is_none()
        {
            let mut sign_in = lock(&self.inner.sign_in);
            if sign_in.generation != generation
                || lock(&self.inner.stored)
                    .as_ref()
                    .is_none_or(|s| s.refresh_token != refresh_token)
            {
                return Ok(None);
            }
            // Timeouts/rate limits are retryable. Other 4xx responses mean a
            // rejected refresh (revoked session,
            // deleted user) — it can NEVER succeed again. Degrade to SignedOut so every
            // downstream retry loop quiets down. (Org-switch refreshes are exempt: a 4xx
            // there means "not a member", not a dead session.)
            tracing::warn!(
                status,
                "auth: refresh rejected — session revoked; signing out"
            );
            self.clear_session(&mut sign_in);
            return Ok(None);
        }
        if !res.status().is_success() {
            return Err(EngineError::Other(format!("refresh failed ({status})")));
        }
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Tokens {
            access_token: String,
            refresh_token: String,
        }
        let tokens: Tokens = res.json().await.map_err(|e| {
            EngineError::Other(format!(
                "malformed refresh response: {}",
                describe_http_error(e)
            ))
        })?;
        let sign_in = lock(&self.inner.sign_in);
        if sign_in.generation != generation
            || lock(&self.inner.stored)
                .as_ref()
                .is_none_or(|s| s.refresh_token != refresh_token)
        {
            return Ok(None);
        }
        let org_id = jwt_claims(&tokens.access_token).and_then(|c| c.org_id);
        let entry = AccessEntry::fresh(tokens.access_token.clone());
        tracing::info!(ttl_s = entry.ttl.as_secs(), "auth: access token refreshed");
        *lock(&self.inner.access) = Some(entry);
        let (user, org_changed) = {
            let mut stored = lock(&self.inner.stored);
            match stored.as_mut() {
                Some(session) => {
                    let changed = session.org_id != org_id;
                    session.refresh_token = tokens.refresh_token;
                    session.org_id = org_id.clone();
                    (session.user.clone(), changed)
                }
                None => return Ok(None), // signed out mid-refresh
            }
        };
        self.persist(lock(&self.inner.stored).as_ref());
        if org_changed {
            self.inner.state_tx.send_replace(state_for(user, org_id));
        }
        self.inner
            .token_tx
            .send_modify(|epoch| *epoch = epoch.wrapping_add(1));
        Ok(Some(tokens.access_token))
    }

    fn session_file(&self) -> PathBuf {
        self.inner.config.data_dir.join("session.json")
    }

    /// Persist (0600) or remove the stored session. Never panics: a disk error degrades
    /// to a logged warning, not a crash mid-refresh.
    fn persist<S: std::borrow::Borrow<StoredSession>>(&self, session: Option<S>) {
        let path = self.session_file();
        let outcome = match session {
            Some(session) => serde_json::to_vec(session.borrow())
                .map_err(std::io::Error::other)
                .and_then(|bytes| write_private(&path, &bytes)),
            None => match std::fs::remove_file(&path) {
                Err(err) if err.kind() != std::io::ErrorKind::NotFound => Err(err),
                _ => Ok(()),
            },
        };
        if let Err(err) = outcome {
            tracing::warn!(error = %err, "auth: failed to persist session");
        }
    }

    async fn authed_json<T: serde::de::DeserializeOwned>(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<T, EngineError> {
        let token = self.access_token().await?;
        let url = format!(
            "{}{}",
            self.inner.config.edge_url.trim_end_matches('/'),
            path
        );
        let mut req = self.inner.http.request(method, &url).bearer_auth(token);
        if let Some(body) = body {
            req = req.json(&body);
        }
        let res = req.send().await.map_err(|e| {
            EngineError::Other(format!(
                "the edge is unreachable: {}",
                describe_http_error(e)
            ))
        })?;
        if !res.status().is_success() {
            return Err(EngineError::Other(format!(
                "workspace request failed ({})",
                res.status().as_u16()
            )));
        }
        res.json::<T>().await.map_err(|e| {
            EngineError::Other(format!("malformed response: {}", describe_http_error(e)))
        })
    }

    // -- loopback callback server ------------------------------------------

    /// Bind the loopback callback listener (idempotent); returns its port.
    async fn ensure_loopback(&self) -> Result<u16, EngineError> {
        let mut slot = self.inner.loopback.lock().await;
        if let Some(port) = *slot {
            return Ok(port);
        }
        let requested = self.inner.config.callback_port.unwrap_or(0);
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", requested))
            .await
            .map_err(|e| EngineError::Other(format!("sign-in callback bind failed: {e}")))?;
        let port = listener
            .local_addr()
            .map_err(|e| EngineError::Other(format!("sign-in callback addr: {e}")))?
            .port();
        *slot = Some(port);
        let weak = Arc::downgrade(&self.inner);
        tokio::spawn(loopback_loop(listener, weak));
        tracing::info!(port, "auth: sign-in callback listening");
        Ok(port)
    }
}

struct SignInResult {
    user: AuthUser,
    access_token: String,
    refresh_token: String,
}

fn state_for(user: AuthUser, org_id: Option<String>) -> AuthState {
    // Every user must belong to an organization before the product opens up; an org-less
    // session is `NeedsOrganization`, which the UI gates on.
    match org_id {
        Some(org_id) => AuthState::SignedIn {
            user,
            org_id: Some(org_id),
        },
        None => AuthState::NeedsOrganization { user },
    }
}

/// The relay/room token seam: `Auth` IS a [`zeron_rpc::TokenSource`], so the host relay
/// and link cache always dial with a fresh bearer after refreshes.
#[async_trait::async_trait]
impl zeron_rpc::TokenSource for Auth {
    async fn token(&self) -> Result<String, TokenError> {
        if self.inner.workos.is_some() && !self.state().is_signed_in() {
            return Err(TokenError::SignedOut);
        }
        self.access_token().await
    }

    fn subscribe(&self) -> Option<watch::Receiver<u64>> {
        Some(self.inner.token_tx.subscribe())
    }
}

// ---------------------------------------------------------------------------
// Loopback HTTP (hand-rolled: no HTTP server dependency in the engine)
// ---------------------------------------------------------------------------

async fn loopback_loop(listener: tokio::net::TcpListener, inner: Weak<AuthInner>) {
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            break;
        };
        let Some(inner) = inner.upgrade() else { break };
        tokio::spawn(async move {
            if let Err(err) = handle_loopback_conn(stream, Auth { inner }).await {
                tracing::debug!(error = %err, "auth: callback connection failed");
            }
        });
    }
}

async fn handle_loopback_conn(
    mut stream: tokio::net::TcpStream,
    auth: Auth,
) -> Result<(), std::io::Error> {
    // Read the request head (bounded; we only need the request line).
    let mut buf = Vec::with_capacity(2048);
    let mut chunk = [0u8; 1024];
    loop {
        let n = tokio::time::timeout(Duration::from_secs(10), stream.read(&mut chunk))
            .await
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "header read"))??;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") || buf.len() > 16 * 1024 {
            break;
        }
    }
    let head = String::from_utf8_lossy(&buf);
    let request_line = head.lines().next().unwrap_or_default();
    let target = request_line.split_whitespace().nth(1).unwrap_or("");
    let (path, query) = target.split_once('?').unwrap_or((target, ""));

    let (status, body) = if path != "/callback" {
        ("404 Not Found", page("Not found."))
    } else {
        let params: HashMap<String, String> = query
            .split('&')
            .filter_map(|kv| kv.split_once('='))
            .map(|(k, v)| (k.to_string(), url_decode(v)))
            .collect();
        let code = params.get("code");
        let state = params.get("state");
        let invalid_callback = || {
            (
                "400 Bad Request",
                page("Invalid or expired sign-in link. Start again from Zeron."),
            )
        };
        match (code, state) {
            (Some(code), Some(state)) => match auth.take_pending(state) {
                Some(generation) => match auth.exchange_code(code).await {
                    Ok(result) => match auth.finish_sign_in(result, generation) {
                        Ok(()) => (
                            "200 OK",
                            page("Signed in. You can close this tab and return to Zeron."),
                        ),
                        Err(err) => {
                            tracing::info!(error = %err, "auth: discarded canceled callback exchange");
                            (
                                "409 Conflict",
                                page(
                                    "This sign-in was canceled. Start again from Zeron if you still want to enable sync.",
                                ),
                            )
                        }
                    },
                    Err(err) => {
                        tracing::warn!(error = %err, "auth: loopback code exchange failed");
                        (
                            "502 Bad Gateway",
                            page("Sign-in failed during token exchange — check the Zeron logs."),
                        )
                    }
                },
                None => invalid_callback(),
            },
            _ => invalid_callback(),
        }
    };
    let response = format!(
        "HTTP/1.1 {status}\r\ncontent-type: text/html; charset=utf-8\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes()).await?;
    stream.shutdown().await
}

fn page(message: &str) -> String {
    format!("<html><body style='font-family:sans-serif;padding:2rem'>{message}</body></html>")
}

// ---------------------------------------------------------------------------
// Small utilities (JWT claims, base64url, URL encoding, 0600 writes)
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
struct JwtClaims {
    #[serde(default)]
    exp: Option<i64>,
    #[serde(default)]
    iat: Option<i64>,
    #[serde(default)]
    org_id: Option<String>,
}

/// Decode (without verifying — the edge verifies) the JWT payload claims. Total: a
/// malformed token yields `None`, never a panic.
fn jwt_claims(token: &str) -> Option<JwtClaims> {
    let payload = token.split('.').nth(1)?;
    let bytes = base64url_decode(payload)?;
    serde_json::from_slice(&bytes).ok()
}

fn base64url_decode(input: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(input.len() * 3 / 4 + 3);
    let mut acc: u32 = 0;
    let mut bits = 0u32;
    for byte in input.bytes() {
        let value = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'-' | b'+' => 62,
            b'_' | b'/' => 63,
            b'=' => continue,
            _ => return None,
        };
        acc = (acc << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

fn url_encode(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

fn url_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
                match hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                    Some(byte) => {
                        out.push(byte);
                        i += 3;
                    }
                    None => {
                        out.push(b'%');
                        i += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            byte => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Write a file readable only by the owner (0600). On non-unix targets a plain write.
fn write_private(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        // An existing file keeps its old mode through OpenOptions — enforce 0600 anyway.
        file.set_permissions(std::os::unix::fs::PermissionsExt::from_mode(0o600))?;
        file.write_all(bytes)
    }
    #[cfg(not(unix))]
    {
        std::fs::write(path, bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn concurrent_dns_failure_is_shared_with_http_and_room_consumers() {
        use crate::http_error::test_support::FailingDns;
        use std::sync::atomic::Ordering;

        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("session.json"),
            r#"{"refreshToken":"refresh-secret","user":{"id":"user_1","email":"u@example.com"},"orgId":"org_1"}"#,
        ).unwrap();
        let mut config = AuthConfig::new("https://edge.invalid", dir.path());
        config.workos_client_id = Some("client_test".into());
        let mut auth = Auth::new(config);
        let dns = Arc::new(FailingDns::default());
        Arc::get_mut(&mut auth.inner).unwrap().http = dns.client();

        let barrier = Arc::new(tokio::sync::Barrier::new(20));
        let mut tasks = tokio::task::JoinSet::new();
        for _ in 0..20 {
            let auth = auth.clone();
            let barrier = barrier.clone();
            tasks.spawn(async move {
                barrier.wait().await;
                auth.access_token().await
            });
        }
        let mut failures = Vec::new();
        while let Some(result) = tasks.join_next().await {
            match result.unwrap() {
                Err(TokenError::TemporarilyUnavailable(reason)) => failures.push(reason),
                other => panic!("DNS failure misclassified: {other:?}"),
            }
        }
        let reason = &failures[0];
        assert!(reason.contains("injected DNS lookup failure"), "{reason}");
        assert!(!reason.contains("refresh-secret"));
        assert!(failures.iter().all(|failure| failure == reason));

        let edge = crate::EdgeConfig::new("https://edge.invalid", Arc::new(auth.clone()));
        assert!(matches!(
            edge.room_url("/registry/org_1/ws").url().await,
            Err(zeron_sync::SyncError::TemporarilyUnavailable(message)) if &message == reason
        ));
        assert!(matches!(
            auth.list_orgs().await,
            Err(EngineError::Token(TokenError::TemporarilyUnavailable(message))) if &message == reason
        ));
        assert_eq!(
            dns.calls.load(Ordering::SeqCst),
            1,
            "cooldown must cover all consumers"
        );
        assert!(auth.state().is_signed_in());
        assert!(dir.path().join("session.json").exists());
    }

    #[test]
    fn base64url_round_trips_jwt_payload() {
        let payload = br#"{"exp":100,"iat":40,"org_id":"org_1"}"#;
        // Standard base64url without padding (as JWTs use).
        let encoded = {
            const ALPHABET: &[u8] =
                b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
            let mut out = String::new();
            for chunk in payload.chunks(3) {
                let b = [
                    chunk[0],
                    *chunk.get(1).unwrap_or(&0),
                    *chunk.get(2).unwrap_or(&0),
                ];
                let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
                out.push(ALPHABET[(n >> 18) as usize & 63] as char);
                out.push(ALPHABET[(n >> 12) as usize & 63] as char);
                if chunk.len() > 1 {
                    out.push(ALPHABET[(n >> 6) as usize & 63] as char);
                }
                if chunk.len() > 2 {
                    out.push(ALPHABET[n as usize & 63] as char);
                }
            }
            out
        };
        assert_eq!(
            base64url_decode(&encoded).as_deref(),
            Some(payload.as_slice())
        );
        let token = format!("h.{encoded}.sig");
        let claims = jwt_claims(&token).expect("claims decode");
        assert_eq!(claims.exp, Some(100));
        assert_eq!(claims.iat, Some(40));
        assert_eq!(claims.org_id.as_deref(), Some("org_1"));
    }

    #[test]
    fn url_coding_round_trips() {
        let raw = "http://127.0.0.1:1234/callback?x=a b&y=%";
        assert_eq!(url_decode(&url_encode(raw)), raw);
        assert_eq!(url_encode("a b"), "a%20b");
    }

    #[test]
    fn auth_state_serializes_as_proto_shape() {
        let user = AuthUser {
            id: "u1".into(),
            email: "u@x".into(),
            name: None,
        };
        let signed_in = AuthState::SignedIn {
            user: user.clone(),
            org_id: Some("org_1".into()),
        };
        let value = serde_json::to_value(&signed_in).expect("json");
        assert_eq!(
            value,
            serde_json::json!({
                "state": "signedIn",
                "user": {"id": "u1", "email": "u@x", "name": null},
                "orgId": "org_1",
            })
        );
        // The proto type itself round-trips the emitted value.
        let parsed: zeron_proto::AuthState = serde_json::from_value(value).expect("proto parse");
        assert!(matches!(parsed, zeron_proto::AuthState::SignedIn { .. }));
        assert_eq!(
            serde_json::to_value(AuthState::SignedOut).expect("json"),
            serde_json::json!({"state": "signedOut"})
        );
        assert_eq!(
            serde_json::to_value(AuthState::NeedsOrganization { user }).expect("json"),
            serde_json::json!({
                "state": "needsOrganization",
                "user": {"id": "u1", "email": "u@x", "name": null},
            })
        );
    }
}

#[async_trait::async_trait]
impl zeron_preview::signaling::TokenSource for Auth {
    async fn token(&self) -> anyhow::Result<String> {
        Ok(self.access_token().await?)
    }
}
