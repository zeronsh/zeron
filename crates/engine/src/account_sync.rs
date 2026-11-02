//! Synced subscriptions: a device's Claude Code and Codex logins, shared with
//! the user's other devices and with Cloud through the credential vault
//! (`edge/vault`, docs/design/cloud-device.md "Synced subscriptions").
//!
//! The vault already holds Cloud's logins and is their only refresher; Cloud
//! machines get short-lived access tokens ("grants") signed for by their
//! device key. A laptop that turns sync on joins the same scheme:
//!
//! 1. **Enroll** a device key of its own (`{data_dir}/account-sync/device-key`,
//!    Ed25519, 0600) with the vault as a `laptop`.
//! 2. **Upload** every Claude Code / Codex subscription login it holds (the
//!    live one and every saved slot) with `authorizedDevices: ["*"]` — every
//!    device the user enrolled, Cloud included — and `activate: false`, so
//!    Cloud's account only changes when Cloud has none yet. Each uploaded
//!    slot is deleted: from now on the vault alone refreshes the login (two
//!    refreshers of one rotating refresh token sign each other out — the
//!    "random sign-outs" of copying credential files between machines).
//! 3. **Install** the login this device uses as a grant: the CLI's own store
//!    (`~/.claude/.credentials.json` or the macOS Keychain, `~/.codex/auth.json`)
//!    gets the access token with an EMPTY refresh token, exactly like a Cloud
//!    machine. A background task re-grants ahead of expiry, so the CLI keeps
//!    working — in Zeron and in a terminal alike — while the engine runs.
//!
//! Each device picks its own login (`state.active`, per agent): a grant names
//! the vault slot it is for. A login the user signs in to later — through
//! Zeron (captured straight into the vault) or with the CLI itself (a real
//! refresh token appears in the live store) — is uploaded the same way, so
//! every login on a synced device is everyone's.
//!
//! Turning sync off keeps the vault's logins (other devices and Cloud keep
//! using them); this device keeps its current access token until it expires.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64URL;
use ring::signature::{Ed25519KeyPair, KeyPair};
use serde::{Deserialize, Serialize};
use zeron_proto::{
    AccountSyncStatus, AgentAccount, AgentAccountWarning, AgentAccountsSnapshot, AgentAuthKind,
    HarnessId, VaultAccount, VaultAccountProfile, VaultConnectionStatus, VaultProvider,
};

use crate::EngineError;
use crate::agent_accounts::{AgentAccounts, CaptureSink, Handoff, LocalLogin, SYNCED_HARNESSES};
use crate::cloud_client::CloudClient;
use crate::credentials::{Grant, GrantOutcome};

/// Background cadence: re-grant tokens due soon, adopt new local logins.
const TICK: Duration = Duration::from_secs(60);
/// The background task re-grants this far ahead of a token's expiry.
const REFRESH_AHEAD_MS: i64 = 10 * 60_000;
/// A run re-grants first when the token has less than this left.
const RUN_SLACK_MS: i64 = 5 * 60_000;

const DIR: &str = "account-sync";
const STATE_FILE: &str = "state.json";
const KEY_FILE: &str = "device-key";

pub(crate) const NOT_SIGNED_IN: &str =
    "Sign in to your Zeron account (Settings → Account) to sync logins across devices.";

/// What [`AccountSync`] needs from the runtime, read fresh each time (the
/// session can change while the engine runs): the vault client acting as
/// the signed-in user, and that user's id (signed into every grant request).
pub(crate) struct SyncContext {
    pub cloud: CloudClient,
    pub user_id: String,
}

pub(crate) type ContextFn = Arc<dyn Fn() -> Option<SyncContext> + Send + Sync>;

/// The vault provider holding `harness`'s subscription logins.
fn provider_of(harness: HarnessId) -> Option<VaultProvider> {
    match harness {
        HarnessId::ClaudeCode => Some(VaultProvider::Claude),
        HarnessId::Codex => Some(VaultProvider::Codex),
        _ => None,
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct State {
    #[serde(default)]
    enabled: bool,
    /// The vault slot each agent uses on THIS device.
    #[serde(default)]
    active: HashMap<HarnessId, String>,
    /// Claude: `{oauthAccount, userID}` per vault slot — the identity
    /// written into `~/.claude.json` when that login is installed. Never
    /// secret.
    #[serde(default)]
    identities: HashMap<String, serde_json::Value>,
    /// The user this device's key is enrolled for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    enrolled_for: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    synced_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

/// The grant installed in an agent's live store.
#[derive(Debug, Clone)]
struct Installed {
    slot: String,
    expires_at: i64,
}

struct Inner {
    dir: PathBuf,
    device_id: String,
    accounts: AgentAccounts,
    context: ContextFn,
    state: Mutex<State>,
    key: Mutex<Option<Arc<Ed25519KeyPair>>>,
    last_ts: Mutex<i64>,
    installed: Mutex<HashMap<HarnessId, Installed>>,
    /// Serializes enable/disable, uploads, switches and re-grants.
    ops: tokio::sync::Mutex<()>,
    /// One signed grant request at a time (the vault's replay fence wants
    /// strictly increasing timestamps per device).
    grant_gate: tokio::sync::Mutex<()>,
    task: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

/// Cheap to clone.
#[derive(Clone)]
pub struct AccountSync {
    inner: Arc<Inner>,
}

impl AccountSync {
    pub(crate) fn new(
        data_dir: &Path,
        device_id: &str,
        accounts: AgentAccounts,
        context: ContextFn,
    ) -> Self {
        let dir = data_dir.join(DIR);
        let state = std::fs::read(dir.join(STATE_FILE))
            .ok()
            .and_then(|raw| serde_json::from_slice::<State>(&raw).ok())
            .unwrap_or_default();
        Self {
            inner: Arc::new(Inner {
                dir,
                device_id: device_id.to_string(),
                accounts,
                context,
                state: Mutex::new(state),
                key: Mutex::new(None),
                last_ts: Mutex::new(0),
                installed: Mutex::new(HashMap::new()),
                ops: tokio::sync::Mutex::new(()),
                grant_gate: tokio::sync::Mutex::new(()),
                task: Mutex::new(None),
            }),
        }
    }

    /// Start the background task (idempotent; no-op outside a runtime).
    pub fn start(&self) {
        let mut task = lock(&self.inner.task);
        if task.is_some() || tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        let weak = Arc::downgrade(&self.inner);
        *task = Some(tokio::spawn(async move {
            loop {
                let Some(inner) = weak.upgrade() else { return };
                AccountSync { inner }.tick().await;
                tokio::time::sleep(TICK).await;
            }
        }));
    }

    pub fn shutdown(&self) {
        if let Some(task) = lock(&self.inner.task).take() {
            task.abort();
        }
    }

    pub fn is_enabled(&self) -> bool {
        lock(&self.inner.state).enabled
    }

    fn active_slot(&self, harness: HarnessId) -> Option<String> {
        let state = lock(&self.inner.state);
        state
            .enabled
            .then(|| state.active.get(&harness).cloned())
            .flatten()
    }

    pub fn status(&self) -> AccountSyncStatus {
        let state = lock(&self.inner.state).clone();
        let available = (self.inner.context)().is_some();
        AccountSyncStatus {
            enabled: state.enabled,
            available,
            message: if available {
                state.message
            } else {
                Some(NOT_SIGNED_IN.into())
            },
            synced_at: state.synced_at,
        }
    }

    // ── state ───────────────────────────────────────────────────────────────

    fn update(&self, change: impl FnOnce(&mut State)) {
        let snapshot = {
            let mut state = lock(&self.inner.state);
            change(&mut state);
            state.clone()
        };
        if let Err(err) = self.persist(&snapshot) {
            tracing::warn!(error = %err, "account sync: couldn't save its state");
        }
    }

    fn persist(&self, state: &State) -> Result<(), EngineError> {
        private_dir(&self.inner.dir)?;
        let bytes = serde_json::to_vec_pretty(state)
            .map_err(|e| EngineError::Other(format!("serialize sync state: {e}")))?;
        crate::runner::write_private_atomic(&self.inner.dir.join(STATE_FILE), &bytes)
    }

    fn note(&self, message: Option<String>) {
        if lock(&self.inner.state).message != message {
            self.update(|state| state.message = message);
        }
    }

    // ── on / off ────────────────────────────────────────────────────────────

    /// Turn sync on (enroll, upload, install) or off.
    pub async fn set_enabled(&self, enabled: bool) -> Result<AccountSyncStatus, EngineError> {
        let _ops = self.inner.ops.lock().await;
        if !enabled {
            self.update(|state| {
                state.enabled = false;
                state.active.clear();
                state.message = None;
            });
            lock(&self.inner.installed).clear();
            // Grant-installed logins can't refresh: don't leave them listed
            // as this device's saved accounts. The live one keeps working
            // until its token expires.
            for harness in SYNCED_HARNESSES {
                if let Err(err) = self.inner.accounts.drop_grant_slots(harness) {
                    tracing::warn!(error = %err, ?harness, "account sync: couldn't tidy grant slots");
                }
            }
            return Ok(self.status());
        }
        let ctx = (self.inner.context)().ok_or_else(|| EngineError::Other(NOT_SIGNED_IN.into()))?;
        self.enroll(&ctx, true).await?;
        // Logins Cloud (or another device) already holds become every
        // device's, so this one can use them too.
        for harness in SYNCED_HARNESSES {
            let Some(provider) = provider_of(harness) else {
                continue;
            };
            if ctx
                .cloud
                .vault_accounts(provider)
                .await
                .is_ok_and(|a| !a.is_empty())
            {
                ctx.cloud
                    .authorize(provider, vec![zeron_proto::VAULT_ALL_DEVICES.into()])
                    .await?;
            }
        }
        self.update(|state| {
            state.enabled = true;
            state.message = None;
        });
        let result = self.sync_all(&ctx).await;
        self.note(result.as_ref().err().map(ToString::to_string));
        result?;
        Ok(self.status())
    }

    /// Upload this device's own logins and install the one it uses, for
    /// both agents. Caller holds `ops`.
    async fn sync_all(&self, ctx: &SyncContext) -> Result<(), EngineError> {
        let mut failures = Vec::new();
        for harness in SYNCED_HARNESSES {
            if let Err(err) = self.sync_harness(ctx, harness).await {
                tracing::warn!(error = %err, ?harness, "account sync failed");
                failures.push(format!("{}: {err}", harness_name(harness)));
            }
        }
        if failures.is_empty() {
            self.update(|state| state.synced_at = Some(crate::now_ms()));
            Ok(())
        } else {
            Err(EngineError::Other(failures.join(" · ")))
        }
    }

    async fn sync_harness(&self, ctx: &SyncContext, harness: HarnessId) -> Result<(), EngineError> {
        let provider =
            provider_of(harness).ok_or_else(|| EngineError::Other("unsupported".into()))?;
        let accounts = &self.inner.accounts;
        let hold = accounts.handoff(harness).await?;
        let logins = accounts.syncable_logins(&hold).await?;
        // What the vault already holds, to tell a saved slot that is merely
        // an old copy of a synced login from one it doesn't have.
        let stored = if logins.iter().any(|login| !login.live) {
            ctx.cloud.vault_accounts(provider).await?
        } else {
            Vec::new()
        };
        let mut live_slot = None;
        let mut failures = Vec::new();
        for login in logins {
            if login.live {
                // From here the CLI can't refresh: the vault is about to own
                // the refresh token, and no later failure may leave two
                // refreshers. Undone if the upload doesn't happen.
                accounts.neuter_live(&hold).await?;
                match self.upload(ctx, provider, &login).await {
                    Ok(slot) => live_slot = Some(slot),
                    Err(err) => {
                        accounts.restore_live(&hold, &login.credential).await?;
                        failures.push(err.to_string());
                        continue;
                    }
                }
            } else if stored
                .iter()
                .any(|account| same_login(account, &login) && account.status == VaultConnectionStatus::Connected)
            {
                // A saved copy of a login the vault already refreshes: its
                // refresh token is likely spent, and uploading it would
                // replace the working one. Just drop it.
            } else if let Err(err) = self.upload(ctx, provider, &login).await {
                failures.push(err.to_string());
                continue;
            }
            if let Some(id) = &login.slot_id {
                accounts.drop_slots(harness, std::slice::from_ref(id))?;
            }
        }
        let current = self.active_slot(harness);
        let target = match live_slot {
            // The CLI's own login was just uploaded: keep using it (even if
            // the install below fails, the next pass retries it).
            Some(slot) => {
                self.update(|state| {
                    state.active.insert(harness, slot.clone());
                });
                Some(slot)
            }
            None if current.is_some() => current,
            // Nothing picked yet: a device with no login of its own (or
            // only a grant) takes the account Cloud uses, else the first.
            None => {
                let api_key = harness == HarnessId::Codex && accounts.live_codex_api_key();
                if api_key {
                    None
                } else {
                    let stored = ctx.cloud.vault_accounts(provider).await?;
                    pick_default(&stored)
                }
            }
        };
        if let Some(slot) = target {
            match self.install(ctx, &hold, harness, &slot).await {
                Ok(()) => self.update(|state| {
                    state.active.insert(harness, slot);
                }),
                Err(err) => failures.push(err.to_string()),
            }
        }
        if failures.is_empty() {
            Ok(())
        } else {
            Err(EngineError::Other(failures.join("; ")))
        }
    }

    /// Upload one login; answers the vault slot it landed in.
    async fn upload(
        &self,
        ctx: &SyncContext,
        provider: VaultProvider,
        login: &LocalLogin,
    ) -> Result<String, EngineError> {
        let material = match login.harness {
            HarnessId::ClaudeCode => serde_json::json!({ "claudeAiOauth": login.credential }),
            _ => serde_json::json!({ "authJson": login.credential }),
        };
        let profile = VaultAccountProfile {
            email: Some(login.email.clone()),
            display_name: login.display_name.clone(),
            organization: login.organization.clone(),
            plan: login.plan.clone(),
        };
        let slot = ctx
            .cloud
            .put_synced_login(provider, material, Some(profile))
            .await?;
        if let Some(identity) = login.claude_config.clone() {
            self.update(|state| {
                state.identities.insert(slot.clone(), identity);
            });
        }
        tracing::info!(provider = provider.as_str(), %slot, "account sync: uploaded a login");
        Ok(slot)
    }

    // ── grants ──────────────────────────────────────────────────────────────

    fn key(&self, fresh: bool) -> Result<Arc<Ed25519KeyPair>, EngineError> {
        let mut slot = lock(&self.inner.key);
        if !fresh && let Some(key) = slot.as_ref() {
            return Ok(key.clone());
        }
        let path = self.inner.dir.join(KEY_FILE);
        let pkcs8 = match std::fs::read(&path) {
            Ok(bytes) if !fresh => bytes,
            _ => {
                private_dir(&self.inner.dir)?;
                let rng = ring::rand::SystemRandom::new();
                let pkcs8 = Ed25519KeyPair::generate_pkcs8(&rng)
                    .map_err(|e| EngineError::Other(format!("generate device key: {e}")))?;
                crate::runner::write_private_atomic(&path, pkcs8.as_ref())?;
                pkcs8.as_ref().to_vec()
            }
        };
        let key = Arc::new(
            Ed25519KeyPair::from_pkcs8(&pkcs8)
                .map_err(|e| EngineError::Other(format!("invalid device key: {e}")))?,
        );
        *slot = Some(key.clone());
        Ok(key)
    }

    /// Enroll this device's key for the signed-in user (once per user;
    /// `force` re-enrolls). A key the user revoked is replaced by a new one
    /// only when they turn sync on again (`force`).
    async fn enroll(&self, ctx: &SyncContext, force: bool) -> Result<(), EngineError> {
        if !force && lock(&self.inner.state).enrolled_for.as_deref() == Some(ctx.user_id.as_str()) {
            return Ok(());
        }
        let key = self.key(false)?;
        let public = B64URL.encode(key.public_key().as_ref());
        match ctx
            .cloud
            .vault_enroll_laptop(&self.inner.device_id, &public)
            .await
        {
            Ok(_) => {}
            Err(err) if force && err.to_string().contains("revoked") => {
                let key = self.key(true)?;
                let public = B64URL.encode(key.public_key().as_ref());
                ctx.cloud
                    .vault_enroll_laptop(&self.inner.device_id, &public)
                    .await?;
            }
            Err(err) => return Err(err),
        }
        let user = ctx.user_id.clone();
        self.update(|state| state.enrolled_for = Some(user));
        Ok(())
    }

    fn next_ts(&self) -> i64 {
        let mut last = lock(&self.inner.last_ts);
        let ts = crate::now_ms().max(*last + 1);
        *last = ts;
        ts
    }

    /// A grant of `slot` (a login of `provider`) for this device.
    async fn grant(&self, ctx: &SyncContext, provider: VaultProvider, slot: &str) -> GrantOutcome {
        let _gate = self.inner.grant_gate.lock().await;
        if let Err(err) = self.enroll(ctx, false).await {
            return GrantOutcome::Unavailable(err.to_string());
        }
        for attempt in 0..3 {
            let key = match self.key(false) {
                Ok(key) => key,
                Err(err) => return GrantOutcome::Unavailable(err.to_string()),
            };
            let ts = self.next_ts();
            let message = crate::runner::RunnerIdentity::grant_message(
                &ctx.user_id,
                &self.inner.device_id,
                provider.as_str(),
                ts,
            );
            let body = serde_json::json!({
                "provider": provider.as_str(),
                "deviceId": self.inner.device_id,
                "ts": ts,
                "sig": B64URL.encode(key.sign(message.as_bytes()).as_ref()),
                "slot": slot,
            });
            match ctx.cloud.vault_grant(body).await {
                GrantOutcome::Granted(grant) if grant.slot.as_deref() != Some(slot) => {
                    // A vault from before per-device logins hands out its
                    // active account whatever was asked: never install the
                    // wrong account.
                    return GrantOutcome::Unavailable(
                        "Zeron's servers don't support synced logins yet — try again later.".into(),
                    );
                }
                GrantOutcome::NotConnected(reason)
                    if attempt < 2 && reason.starts_with("stale") =>
                {
                    continue;
                }
                GrantOutcome::NotConnected(reason)
                    if attempt < 2 && reason.starts_with("device_unknown") =>
                {
                    // The vault forgot this device (or another user signed
                    // in here): enroll again and retry.
                    self.update(|state| state.enrolled_for = None);
                    if let Err(err) = self.enroll(ctx, false).await {
                        return GrantOutcome::Unavailable(err.to_string());
                    }
                    continue;
                }
                other => return other,
            }
        }
        GrantOutcome::Unavailable(
            "Zeron's servers refused this device's clock — check that the date and time are \
             set automatically."
                .into(),
        )
    }

    /// Grant `slot` and make it `harness`'s live login on this device.
    async fn install(
        &self,
        ctx: &SyncContext,
        hold: &Handoff<'_>,
        harness: HarnessId,
        slot: &str,
    ) -> Result<(), EngineError> {
        let provider =
            provider_of(harness).ok_or_else(|| EngineError::Other("unsupported".into()))?;
        let grant = match self.grant(ctx, provider, slot).await {
            GrantOutcome::Granted(grant) => grant,
            GrantOutcome::NotConnected(reason) => {
                return Err(EngineError::Other(not_connected(harness, &reason)));
            }
            GrantOutcome::Unavailable(reason) => return Err(EngineError::Other(reason)),
        };
        // Checked under the hold, after the grant round trip: a sign-in that
        // landed meanwhile (a terminal `/login`) is the only copy of that
        // login — the next pass uploads it instead.
        if self.inner.accounts.live_has_refresh_token(hold).await {
            return Err(EngineError::Other(format!(
                "A new {} login appeared on this device; it's synced on the next pass.",
                harness_name(harness)
            )));
        }
        self.write_grant(ctx, hold, harness, slot, &grant).await?;
        lock(&self.inner.installed).insert(
            harness,
            Installed {
                slot: slot.to_string(),
                expires_at: grant.expires_at,
            },
        );
        tracing::info!(?harness, %slot, generation = grant.generation, "account sync: installed a grant");
        Ok(())
    }

    async fn write_grant(
        &self,
        ctx: &SyncContext,
        hold: &Handoff<'_>,
        harness: HarnessId,
        slot: &str,
        grant: &Grant,
    ) -> Result<(), EngineError> {
        let accounts = &self.inner.accounts;
        match harness {
            HarnessId::Codex => accounts.install_codex_grant(
                hold,
                &crate::credentials::codex_chatgpt_auth_json(grant, chrono::Utc::now()),
            ),
            HarnessId::ClaudeCode => {
                let known = lock(&self.inner.state).identities.get(slot).cloned();
                let identity = match known {
                    Some(identity) => identity,
                    None => match accounts.claude_identity(&grant.access_token).await {
                        Some(identity) => {
                            let stored = identity.clone();
                            self.update(|state| {
                                state.identities.insert(slot.to_string(), stored);
                            });
                            identity
                        }
                        // Not remembered: the real profile is fetched again
                        // on the next install.
                        None => fallback_claude_identity(ctx, slot).await,
                    },
                };
                accounts
                    .install_claude_grant(
                        hold,
                        crate::credentials::claude_oauth_json(grant),
                        &identity,
                    )
                    .await
            }
            _ => Ok(()),
        }
    }

    /// Re-grant `harness`'s login when its token has less than `slack_ms`
    /// left (or none is known to be installed).
    async fn ensure_fresh(
        &self,
        ctx: &SyncContext,
        harness: HarnessId,
        slack_ms: i64,
    ) -> Result<(), EngineError> {
        let Some(slot) = self.active_slot(harness) else {
            return Ok(());
        };
        let due = match lock(&self.inner.installed).get(&harness) {
            Some(installed) if installed.slot == slot => {
                installed.expires_at - crate::now_ms() < slack_ms
            }
            _ => true,
        };
        if !due {
            return Ok(());
        }
        let accounts = &self.inner.accounts;
        // The user switched Codex to an API key outside Zeron: stop
        // installing synced logins over it.
        if harness == HarnessId::Codex && accounts.live_codex_api_key() {
            self.release(harness);
            return Ok(());
        }
        // The user signed in by themselves meanwhile (a real refresh token
        // in the live store): upload that login first — never overwrite it.
        if accounts
            .live_login(harness)
            .await
            .is_some_and(|(_, _, refreshable)| refreshable)
        {
            return self.sync_harness(ctx, harness).await;
        }
        let hold = accounts.handoff(harness).await?;
        match self.install(ctx, &hold, harness, &slot).await {
            Ok(()) => Ok(()),
            Err(err) => {
                if err.to_string().contains(GONE) {
                    // Removed from the vault (from another device): stop
                    // using it here.
                    drop(hold);
                    self.update(|state| {
                        state.active.remove(&harness);
                    });
                    lock(&self.inner.installed).remove(&harness);
                    accounts.sign_out_grant(harness).await?;
                }
                Err(err)
            }
        }
    }

    /// The background pass: adopt new local logins, keep tokens fresh.
    async fn tick(&self) {
        if !self.is_enabled() {
            return;
        }
        let _ops = self.inner.ops.lock().await;
        let Some(ctx) = (self.inner.context)() else {
            self.note(Some(NOT_SIGNED_IN.into()));
            return;
        };
        let mut failures = Vec::new();
        for harness in SYNCED_HARNESSES {
            let accounts = &self.inner.accounts;
            // A login the vault doesn't hold yet: the CLI's own sign-in, or
            // a saved slot that still refreshes.
            let new_login = accounts
                .live_login(harness)
                .await
                .is_some_and(|(_, _, refreshable)| refreshable)
                || accounts.has_syncable_slots(harness);
            let result = if new_login {
                self.sync_harness(&ctx, harness).await
            } else {
                self.ensure_fresh(&ctx, harness, REFRESH_AHEAD_MS).await
            };
            if let Err(err) = result {
                tracing::warn!(error = %err, ?harness, "account sync: background pass failed");
                failures.push(format!("{}: {err}", harness_name(harness)));
            }
        }
        if failures.is_empty() {
            self.note(None);
        } else {
            self.note(Some(failures.join(" · ")));
        }
    }

    // ── Providers page ──────────────────────────────────────────────────────

    /// This device's accounts as the Providers page shows them while sync is
    /// on: its Claude Code / Codex subscription rows are the vault's logins
    /// (ids `"{provider}:{slot}"`); API keys and every other agent stay local.
    pub async fn list(
        &self,
        local: AgentAccountsSnapshot,
        force_usage: bool,
    ) -> AgentAccountsSnapshot {
        if !self.is_enabled() {
            return local;
        }
        let Some(ctx) = (self.inner.context)() else {
            let mut local = local;
            local.warnings.push(AgentAccountWarning {
                harness: HarnessId::ClaudeCode,
                message: NOT_SIGNED_IN.into(),
            });
            return local;
        };
        let mut warnings = local.warnings;
        let mut synced: HashMap<HarnessId, Vec<AgentAccount>> = HashMap::new();
        for harness in SYNCED_HARNESSES {
            let active = self.active_slot(harness);
            match crate::cloud_accounts::synced_rows(
                &ctx.cloud,
                harness,
                active.as_deref(),
                force_usage,
            )
            .await
            {
                Ok(rows) => {
                    synced.insert(harness, rows);
                }
                Err(message) => warnings.push(AgentAccountWarning {
                    harness,
                    message: format!("Synced logins unavailable: {message}"),
                }),
            }
        }
        let mut accounts = Vec::new();
        let mut placed = std::collections::HashSet::new();
        for account in local.accounts {
            let harness = account.harness;
            if let Some(rows) = synced.get_mut(&harness) {
                if placed.insert(harness) {
                    accounts.append(rows);
                }
                // Local subscription rows are the vault's now (or grant
                // snapshots of them); API keys stay.
                if account.auth_kind != Some(AgentAuthKind::ApiKey) {
                    continue;
                }
                let mut account = account;
                // A synced login in use outranks a key that isn't.
                if self.active_slot(harness).is_some() {
                    account.active = false;
                }
                accounts.push(account);
                continue;
            }
            accounts.push(account);
        }
        for harness in SYNCED_HARNESSES {
            if placed.insert(harness)
                && let Some(rows) = synced.remove(&harness)
            {
                accounts.extend(rows);
            }
        }
        AgentAccountsSnapshot { accounts, warnings }
    }

    /// Whether an account call for `harness` / `account_id` is a synced
    /// login's (answered here, not by the device's own slots).
    pub fn handles(&self, harness: HarnessId, account_id: &str) -> bool {
        self.is_enabled() && provider_of(harness).is_some() && account_id.contains(':')
    }

    /// Use a synced login on this device.
    pub async fn activate(&self, harness: HarnessId, account_id: &str) -> Result<(), EngineError> {
        let slot = synced_slot(harness, account_id)?;
        let _ops = self.inner.ops.lock().await;
        let ctx = (self.inner.context)().ok_or_else(|| EngineError::Other(NOT_SIGNED_IN.into()))?;
        // Upload a login the CLI holds that the vault doesn't yet: the
        // switch must not strand it.
        self.hand_over_live(&ctx, harness).await;
        let hold = self.inner.accounts.handoff(harness).await?;
        self.install(&ctx, &hold, harness, &slot).await?;
        self.update(|state| {
            state.active.insert(harness, slot);
        });
        Ok(())
    }

    /// Before a switch: upload the CLI's own login if the vault doesn't hold
    /// it yet. A failure is logged — the switch itself then refuses to
    /// overwrite that login (see [`Self::install`]).
    async fn hand_over_live(&self, ctx: &SyncContext, harness: HarnessId) {
        if self
            .inner
            .accounts
            .live_login(harness)
            .await
            .is_some_and(|(_, _, refreshable)| refreshable)
            && let Err(err) = self.sync_harness(ctx, harness).await
        {
            tracing::warn!(error = %err, ?harness, "account sync: couldn't upload the live login first");
        }
    }

    /// A local (non-synced) row was switched to — an API key: stop
    /// installing synced grants over it.
    pub fn release(&self, harness: HarnessId) {
        if self.active_slot(harness).is_some() {
            self.update(|state| {
                state.active.remove(&harness);
            });
            lock(&self.inner.installed).remove(&harness);
        }
    }

    /// Remove a synced login from the vault — every device and Cloud.
    pub async fn forget(&self, harness: HarnessId, account_id: &str) -> Result<(), EngineError> {
        let slot = synced_slot(harness, account_id)?;
        let provider =
            provider_of(harness).ok_or_else(|| EngineError::Other("unsupported".into()))?;
        let _ops = self.inner.ops.lock().await;
        let ctx = (self.inner.context)().ok_or_else(|| EngineError::Other(NOT_SIGNED_IN.into()))?;
        ctx.cloud.vault_forget(provider, &slot).await?;
        crate::cloud_accounts::forget_usage(provider, &slot);
        let was_active = self.active_slot(harness).as_deref() == Some(slot.as_str());
        self.update(|state| {
            state.identities.remove(&slot);
            if was_active {
                state.active.remove(&harness);
            }
        });
        if was_active {
            lock(&self.inner.installed).remove(&harness);
            self.inner.accounts.sign_out_grant(harness).await?;
        }
        Ok(())
    }

    /// Where a sign-in on a synced device goes: the vault (every device's),
    /// then this device uses it.
    pub fn login_sink(&self, harness: HarnessId) -> CaptureSink {
        let sync = self.clone();
        Arc::new(move |captured| {
            let sync = sync.clone();
            Box::pin(async move {
                sync.adopt_captured(harness, captured)
                    .await
                    .map_err(|e| e.to_string())
            })
        })
    }

    async fn adopt_captured(
        &self,
        harness: HarnessId,
        captured: serde_json::Value,
    ) -> Result<(), EngineError> {
        let provider =
            provider_of(harness).ok_or_else(|| EngineError::Other("unsupported".into()))?;
        let _ops = self.inner.ops.lock().await;
        let ctx = (self.inner.context)().ok_or_else(|| EngineError::Other(NOT_SIGNED_IN.into()))?;
        let (credential, profile) = crate::cloud_client::split_capture(captured);
        let material = match harness {
            HarnessId::ClaudeCode => serde_json::json!({ "claudeAiOauth": credential }),
            _ => serde_json::json!({ "authJson": credential }),
        };
        let slot = ctx
            .cloud
            .put_synced_login(provider, material, profile)
            .await?;
        self.hand_over_live(&ctx, harness).await;
        let hold = self.inner.accounts.handoff(harness).await?;
        self.install(&ctx, &hold, harness, &slot).await?;
        self.update(|state| {
            state.active.insert(harness, slot);
            state.synced_at = Some(crate::now_ms());
        });
        Ok(())
    }
}

/// The marker [`not_connected`] puts in a login the vault no longer has.
const GONE: &str = "was removed from your synced logins";

fn not_connected(harness: HarnessId, reason: &str) -> String {
    if reason.starts_with("not_found") {
        format!("This {} login {GONE}.", harness_name(harness))
    } else if reason.starts_with("needs_reconnect") {
        format!(
            "This {} login stopped working — sign in to it again.",
            harness_name(harness)
        )
    } else if reason.starts_with("device_revoked") {
        "This device was removed from your synced devices — turn sync on again to add it back."
            .into()
    } else {
        format!(
            "Couldn't use the synced {} login: {reason}",
            harness_name(harness)
        )
    }
}

fn harness_name(harness: HarnessId) -> &'static str {
    match harness {
        HarnessId::ClaudeCode => "Claude Code",
        HarnessId::Codex => "Codex",
        _ => "agent",
    }
}

/// `"{provider}:{slot}"` naming one of `harness`'s subscription logins.
fn synced_slot(harness: HarnessId, account_id: &str) -> Result<String, EngineError> {
    let (provider, slot) = crate::cloud_accounts::account_ref(harness, account_id)
        .map_err(|e| EngineError::Other(e.to_string()))?;
    if Some(provider) != provider_of(harness) {
        return Err(EngineError::Other(
            "API keys stored for Cloud aren't synced to devices.".into(),
        ));
    }
    Ok(slot.to_string())
}

/// Whether a vault account is the same login as a local one, by the vault's
/// own identity rule (Claude: email + organization; Codex: email).
fn same_login(account: &VaultAccount, login: &LocalLogin) -> bool {
    let email = account.email.as_deref().or(account.account.as_deref());
    let org = |o: Option<&str>| o.unwrap_or_default().to_lowercase();
    email.is_some_and(|email| email.eq_ignore_ascii_case(&login.email))
        && (login.harness != HarnessId::ClaudeCode
            || org(account.organization.as_deref()) == org(login.organization.as_deref()))
}

/// The login a device with none picks: the one Cloud uses, else the first
/// working one.
fn pick_default(stored: &[VaultAccount]) -> Option<String> {
    let usable = |a: &&VaultAccount| a.status == VaultConnectionStatus::Connected;
    stored
        .iter()
        .filter(usable)
        .find(|a| a.active)
        .or_else(|| stored.iter().find(usable))
        .map(|a| a.slot.clone())
}

/// `{oauthAccount}` from the vault's own view of a login, when Claude's
/// profile endpoint can't be read.
async fn fallback_claude_identity(ctx: &SyncContext, slot: &str) -> serde_json::Value {
    let account = ctx
        .cloud
        .vault_accounts(VaultProvider::Claude)
        .await
        .ok()
        .and_then(|accounts| accounts.into_iter().find(|a| a.slot == slot));
    let email = account
        .as_ref()
        .and_then(|a| a.email.clone().or_else(|| a.account.clone()))
        .unwrap_or_default();
    serde_json::json!({
        "oauthAccount": {
            "accountUuid": email,
            "emailAddress": email,
            "organizationName": account.as_ref().and_then(|a| a.organization.clone()),
            "displayName": account.as_ref().and_then(|a| a.display_name.clone()),
        }
    })
}

fn private_dir(dir: &Path) -> Result<(), EngineError> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

#[async_trait::async_trait]
impl crate::registry::RunEnvironment for AccountSync {
    /// A run of Claude Code / Codex on a synced device starts with a token
    /// that has a few minutes left at least. A failure is logged, not fatal:
    /// the CLI then reports its own sign-in error.
    async fn prepare(
        &self,
        harness: HarnessId,
        _request: &mut zeron_proto::RunRequest,
    ) -> Result<(), String> {
        if self.active_slot(harness).is_none() {
            return Ok(());
        }
        let _ops = self.inner.ops.lock().await;
        let Some(ctx) = (self.inner.context)() else {
            return Ok(());
        };
        if let Err(err) = self.ensure_fresh(&ctx, harness, RUN_SLACK_MS).await {
            tracing::warn!(error = %err, ?harness, "account sync: couldn't refresh before a run");
        }
        Ok(())
    }

    /// Switching the synced login used here applies from the next turn.
    async fn credential(&self, harness: HarnessId) -> Option<String> {
        self.active_slot(harness)
            .map(|slot| format!("synced:{slot}"))
    }
}

#[cfg(test)]
mod scenarios;

#[cfg(test)]
mod tests {
    use super::*;

    fn vault(slot: &str, active: bool, status: VaultConnectionStatus) -> VaultAccount {
        VaultAccount {
            slot: slot.into(),
            status,
            active,
            kind: "oauth".into(),
            account: Some(format!("{slot}@x.com")),
            email: Some(format!("{slot}@x.com")),
            display_name: None,
            organization: None,
            plan: None,
            authorized_devices: vec!["*".into()],
            created_at: 1,
            updated_at: 2,
        }
    }

    #[test]
    fn a_device_without_a_login_takes_clouds_then_the_first_working_one() {
        use VaultConnectionStatus::{Connected, NeedsReconnect};
        assert_eq!(pick_default(&[]), None);
        assert_eq!(
            pick_default(&[vault("a", false, Connected), vault("b", true, Connected)]),
            Some("b".into())
        );
        assert_eq!(
            pick_default(&[
                vault("a", true, NeedsReconnect),
                vault("b", false, Connected)
            ]),
            Some("b".into())
        );
        assert_eq!(pick_default(&[vault("a", true, NeedsReconnect)]), None);
    }

    #[test]
    fn synced_ids_name_subscription_logins_only() {
        assert_eq!(
            synced_slot(HarnessId::ClaudeCode, "claude:s1").unwrap(),
            "s1"
        );
        assert_eq!(synced_slot(HarnessId::Codex, "codex:s2").unwrap(), "s2");
        assert!(synced_slot(HarnessId::Codex, "openai-key:k1").is_err());
        assert!(synced_slot(HarnessId::Codex, "claude:s1").is_err());
    }

    #[test]
    fn vault_refusals_read_as_what_to_do() {
        assert!(not_connected(HarnessId::Codex, "not_found: no such codex account").contains(GONE));
        assert!(
            not_connected(HarnessId::ClaudeCode, "needs_reconnect: x")
                .contains("sign in to it again")
        );
        assert!(
            not_connected(HarnessId::Codex, "device_revoked: x").contains("turn sync on again")
        );
    }

    #[test]
    fn state_round_trips_and_ignores_unknown_fields() {
        let mut state = State {
            enabled: true,
            ..Default::default()
        };
        state.active.insert(HarnessId::ClaudeCode, "s1".into());
        let json = serde_json::to_value(&state).unwrap();
        let back: State = serde_json::from_value(json.clone()).unwrap();
        assert!(back.enabled);
        assert_eq!(
            back.active.get(&HarnessId::ClaudeCode).map(String::as_str),
            Some("s1")
        );
        let mut extra = json;
        extra["future"] = serde_json::json!(1);
        assert!(serde_json::from_value::<State>(extra).is_ok());
    }

    /// The whole on-device path against a mock vault: enable uploads the
    /// live Codex login (slot deleted, grant installed with an empty
    /// refresh token), and a switch installs another slot's grant.
    #[tokio::test]
    async fn enabling_uploads_the_live_login_and_installs_its_grant() {
        let server = mock_vault::start().await;
        let root = tempfile::tempdir().unwrap();
        let config = crate::agent_accounts::AgentAccountsConfig::isolated(root.path());
        std::fs::create_dir_all(&config.codex_home).unwrap();
        std::fs::write(
            config.codex_home.join("auth.json"),
            mock_vault::codex_auth("me@x.com", "rt-mine").to_string(),
        )
        .unwrap();
        let accounts = AgentAccounts::new(config.clone());
        let url = server.url.clone();
        let context: ContextFn = Arc::new(move || {
            Some(SyncContext {
                cloud: CloudClient::new(
                    url.clone(),
                    "org1",
                    Arc::new(mock_vault::Token) as Arc<dyn zeron_rpc::TokenSource>,
                ),
                user_id: "user1".into(),
            })
        });
        let sync = AccountSync::new(
            &root.path().join("data"),
            "laptop-1",
            accounts.clone(),
            context,
        );
        let status = sync.set_enabled(true).await.unwrap();
        assert!(status.enabled, "{status:?}");
        assert!(status.message.is_none(), "{status:?}");

        // The vault got the login with the real refresh token, for every
        // device, without taking Cloud's active account.
        let uploads = server.uploads();
        let codex = uploads.iter().find(|u| u["provider"] == "codex").unwrap();
        assert_eq!(codex["body"]["authorizedDevices"], serde_json::json!(["*"]));
        assert_eq!(codex["body"]["activate"], false);
        assert_eq!(
            codex["body"]["material"]["authJson"]["tokens"]["refresh_token"],
            "rt-mine"
        );
        // The device enrolled, and asked for that slot by name.
        assert_eq!(server.enrolled(), vec!["laptop-1".to_string()]);
        let grant = server
            .grants()
            .into_iter()
            .find(|g| g["provider"] == "codex")
            .unwrap();
        assert_eq!(grant["slot"], "codex-1");
        assert_eq!(grant["deviceId"], "laptop-1");
        // Live store: the grant, refreshable only by the vault.
        let live: serde_json::Value =
            serde_json::from_slice(&std::fs::read(config.codex_home.join("auth.json")).unwrap())
                .unwrap();
        assert_eq!(live["tokens"]["access_token"], "granted-codex-1");
        assert_eq!(live["tokens"]["refresh_token"], "");
        assert_eq!(
            sync.active_slot(HarnessId::Codex).as_deref(),
            Some("codex-1")
        );
        assert_eq!(
            crate::registry::RunEnvironment::credential(&sync, HarnessId::Codex).await,
            Some("synced:codex-1".into())
        );

        // The page lists the vault's logins in place of local slots.
        let local = accounts.list(false).await.unwrap();
        let listed = sync.list(local, false).await;
        let codex_rows: Vec<_> = listed
            .accounts
            .iter()
            .filter(|a| a.harness == HarnessId::Codex)
            .collect();
        assert_eq!(codex_rows.len(), 1, "{listed:?}");
        assert_eq!(codex_rows[0].id, "codex:codex-1");
        assert!(codex_rows[0].active);

        // Switching to another synced login installs its grant.
        server.add_account("codex", "codex-2", "other@x.com");
        sync.activate(HarnessId::Codex, "codex:codex-2")
            .await
            .unwrap();
        let live: serde_json::Value =
            serde_json::from_slice(&std::fs::read(config.codex_home.join("auth.json")).unwrap())
                .unwrap();
        assert_eq!(live["tokens"]["access_token"], "granted-codex-2");

        // Off: this device stops installing grants; the vault keeps logins.
        let status = sync.set_enabled(false).await.unwrap();
        assert!(!status.enabled);
        assert_eq!(sync.active_slot(HarnessId::Codex), None);
        assert_eq!(
            crate::registry::RunEnvironment::credential(&sync, HarnessId::Codex).await,
            None
        );
        sync.shutdown();
    }

    /// A tiny stand-in for the edge's `/vault/{org}/…` routes (raw
    /// HTTP/1.1, one request per connection).
    mod mock_vault {
        use std::sync::{Arc, Mutex};

        use serde_json::{Value, json};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        pub struct Token;

        #[async_trait::async_trait]
        impl zeron_rpc::TokenSource for Token {
            async fn token(&self) -> Result<String, zeron_rpc::TokenError> {
                Ok("bearer".into())
            }
        }

        pub fn codex_auth(email: &str, refresh: &str) -> Value {
            use base64::Engine as _;
            let enc =
                |v: &Value| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(v.to_string());
            let claims = json!({
                "email": email,
                "https://api.openai.com/auth": {
                    "chatgpt_account_id": "ws-1",
                    "chatgpt_user_id": format!("user-{email}"),
                    "chatgpt_plan_type": "pro",
                },
            });
            json!({
                "OPENAI_API_KEY": null,
                "tokens": {
                    "id_token": format!("{}.{}.sig", enc(&json!({"alg": "none"})), enc(&claims)),
                    "access_token": format!("at-{email}"),
                    "refresh_token": refresh,
                    "account_id": "ws-1",
                },
                "last_refresh": "2026-11-01T00:00:00Z",
            })
        }

        #[derive(Default)]
        struct Vault {
            uploads: Vec<Value>,
            grants: Vec<Value>,
            enrolled: Vec<String>,
            accounts: Vec<(String, Value)>,
        }

        #[derive(Clone)]
        pub struct Server {
            pub url: String,
            vault: Arc<Mutex<Vault>>,
        }

        impl Server {
            pub fn uploads(&self) -> Vec<Value> {
                self.vault.lock().unwrap().uploads.clone()
            }
            pub fn grants(&self) -> Vec<Value> {
                self.vault.lock().unwrap().grants.clone()
            }
            pub fn enrolled(&self) -> Vec<String> {
                self.vault.lock().unwrap().enrolled.clone()
            }
            pub fn add_account(&self, provider: &str, slot: &str, email: &str) {
                self.vault
                    .lock()
                    .unwrap()
                    .accounts
                    .push((provider.into(), account(slot, email, false)));
            }
        }

        fn account(slot: &str, email: &str, active: bool) -> Value {
            json!({
                "slot": slot, "status": "connected", "active": active, "kind": "oauth",
                "account": email, "email": email, "authorizedDevices": ["*"],
                "createdAt": 1, "updatedAt": crate::now_ms(),
            })
        }

        fn status() -> Value {
            json!({ "connections": [], "devices": [], "available": true })
        }

        fn route(vault: &Mutex<Vault>, method: &str, path: &str, body: Value) -> (u16, Value) {
            let parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();
            let mut v = vault.lock().unwrap();
            match (method, parts.as_slice()) {
                ("PUT", ["vault", _, "credentials", provider]) => {
                    let provider = provider.to_string();
                    let n = v.accounts.iter().filter(|(p, _)| *p == provider).count() + 1;
                    let email = body["profile"]["email"].as_str().unwrap_or("x").to_string();
                    v.accounts.push((
                        provider.clone(),
                        account(&format!("{provider}-{n}"), &email, n == 1),
                    ));
                    v.uploads
                        .push(json!({ "provider": provider, "body": body }));
                    (200, status())
                }
                ("PATCH", ["vault", _, "credentials", _]) => (200, status()),
                ("GET", ["vault", _, "accounts", provider]) => {
                    let list: Vec<Value> = v
                        .accounts
                        .iter()
                        .filter(|(p, _)| p == provider)
                        .map(|(_, a)| a.clone())
                        .collect();
                    (200, json!({ "accounts": list }))
                }
                ("POST", ["vault", _, "devices", "enroll"]) => {
                    let id = body["deviceId"].as_str().unwrap_or_default().to_string();
                    v.enrolled.push(id);
                    (200, status())
                }
                ("POST", ["vault", _, "grant"]) => {
                    v.grants.push(body.clone());
                    let slot = body["slot"].as_str().unwrap_or_default().to_string();
                    (
                        200,
                        json!({
                            "provider": body["provider"],
                            "accessToken": format!("granted-{slot}"),
                            "expiresAt": crate::now_ms() + 3_600_000,
                            "generation": 1,
                            "slot": slot,
                        }),
                    )
                }
                _ => (404, json!({ "error": "not_found" })),
            }
        }

        pub async fn start() -> Server {
            let vault = Arc::new(Mutex::new(Vault::default()));
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            let shared = vault.clone();
            tokio::spawn(async move {
                while let Ok((mut stream, _)) = listener.accept().await {
                    let vault = shared.clone();
                    tokio::spawn(async move {
                        let mut raw = Vec::new();
                        let mut chunk = [0u8; 4096];
                        let header_end = loop {
                            let Ok(n) = stream.read(&mut chunk).await else {
                                return;
                            };
                            if n == 0 {
                                return;
                            }
                            raw.extend_from_slice(&chunk[..n]);
                            if let Some(at) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                                break at + 4;
                            }
                        };
                        let head = String::from_utf8_lossy(&raw[..header_end]).to_string();
                        let mut lines = head.lines();
                        let mut request_line = lines.next().unwrap_or("").split_whitespace();
                        let method = request_line.next().unwrap_or("").to_string();
                        let target = request_line.next().unwrap_or("").to_string();
                        let length = lines
                            .filter_map(|line| line.split_once(':'))
                            .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                            .and_then(|(_, value)| value.trim().parse::<usize>().ok())
                            .unwrap_or(0);
                        while raw.len() < header_end + length {
                            let Ok(n) = stream.read(&mut chunk).await else {
                                return;
                            };
                            if n == 0 {
                                break;
                            }
                            raw.extend_from_slice(&chunk[..n]);
                        }
                        let body =
                            serde_json::from_slice(&raw[header_end..]).unwrap_or(Value::Null);
                        let path = target.split('?').next().unwrap_or("").to_string();
                        let (status, reply) = route(&vault, &method, &path, body);
                        let reply = reply.to_string();
                        let response = format!(
                            "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\n\
                             content-length: {}\r\nconnection: close\r\n\r\n{reply}",
                            reply.len()
                        );
                        let _ = stream.write_all(response.as_bytes()).await;
                        let _ = stream.shutdown().await;
                    });
                }
            });
            Server { url, vault }
        }
    }
}
