//! Pins: one chat runs on a saved account of its own while the live login
//! stays as it is, so other chats and the user's own terminals keep running
//! on it. Plugins decide *which* account (`zeron.pin_account`, and the
//! `run.preparing` hook that answers a chat's first run); the engine only
//! holds the table and delivers the credential.
//!
//! A pin lasts the chat's life: CLI sessions, resume and prompt caching all
//! assume stable credentials. A chat pinned to the live login (or to an
//! account that is gone) runs exactly as without a pin. Each pin is kept in
//! `{data_dir}/agent-accounts/pins.json`.
//!
//! | harness     | mechanism | how a pinned chat reaches its account |
//! |-------------|-----------|---------------------------------------|
//! | Claude Code | `env`     | `CLAUDE_CODE_OAUTH_TOKEN` = the saved login's access token |
//! | Muse        | `root`    | the account's own config root (`XDG_CONFIG_HOME`) |
//!
//! Every other harness reports `pinnable = false` until it has a mechanism.
//! A chat's run credential is a fingerprint of what it was started with, so a
//! session parked on another account (or a token Zeron has since refreshed)
//! is replaced, and resumed, before its next turn.
//!
//! **Claude Code.** The CLI neither refreshes nor writes a token it got from
//! the environment, so the live login can't be touched. Zeron owns the saved
//! login's refresh: a run starting with less than [`CLAUDE_REFRESH_MARGIN`]
//! left on the token refreshes the saved login first (the slot is the only
//! holder of its refresh token). Claude Code treats an environment token as
//! inference-only, so features that need the account's profile scope
//! (Remote Control) are unavailable in a pinned chat.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeron_proto::{AgentAccount, AgentAuthKind, HarnessId};

use super::muse::OWN_LOGIN;
use super::{AgentAccounts, MuseRunEnvironment, lock, now_ms, str_field, write_file_atomic};

/// The run credential of a chat that runs on the live login.
const LIVE: &str = "live";
/// A pinned Claude run starts with at least this much left on its token
/// (access tokens live ~8h; a turn running past expiry fails).
const CLAUDE_REFRESH_MARGIN: Duration = Duration::from_secs(10 * 60);
/// A chat's cleared pin is remembered this long, so a session parked on the
/// pinned account moves back to the live login.
const CLEARED_KEPT_MS: i64 = 30 * 24 * 60 * 60 * 1000;
/// `{data_dir}/agent-accounts/pins.json`.
pub(super) const PINS_FILE: &str = "pins.json";

/// What decides a chat's account for its first run (a plugin's
/// `run.preparing` answer): the account it names, if any.
pub type PinHook = Arc<dyn Fn(HarnessId, String) -> BoxFuture<'static, Option<String>> + Send + Sync>;

/// A chat running on a saved account.
#[derive(Debug, Clone, PartialEq)]
pub struct ChatPin {
    pub chat: String,
    pub harness: HarnessId,
    pub account: String,
    /// Epoch millis it was pinned.
    pub since: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
struct PinEntry {
    /// `None`: the pin was taken back, and the chat runs on the live login.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    account: Option<String>,
    since: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(super) struct PinState {
    /// chat id → harness → pin (a chat may change provider).
    #[serde(default)]
    chats: HashMap<String, HashMap<HarnessId, PinEntry>>,
}

impl PinState {
    pub(super) fn load(file: &std::path::Path) -> Self {
        std::fs::read_to_string(file)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
    }

    fn entry(&self, harness: HarnessId, chat: &str) -> Option<&PinEntry> {
        self.chats.get(chat)?.get(&harness)
    }
}

/// What a pinned chat's run gets.
pub(crate) struct PinRun {
    pub env: BTreeMap<String, String>,
    pub credential: String,
}

fn fingerprint(prefix: &str, secret: &str) -> String {
    let digest = Sha256::digest(secret.as_bytes());
    let hex: String = digest.iter().take(6).map(|b| format!("{b:02x}")).collect();
    format!("{prefix}:{hex}")
}

/// Whether `harness` has a per-chat mechanism at all.
fn pinnable(harness: HarnessId) -> bool {
    matches!(harness, HarnessId::ClaudeCode | HarnessId::Muse)
}

impl AgentAccounts {
    fn persist_pins(&self, state: &PinState) {
        let Ok(root) = self.inner.config.private_root() else { return };
        let Ok(bytes) = serde_json::to_vec_pretty(state) else { return };
        if let Err(err) = write_file_atomic(&root.join(PINS_FILE), &bytes, false) {
            tracing::warn!(error = %err, "could not save the chat pins");
        }
    }

    /// Where a plugin's `run.preparing` answer is asked (a chat's first run).
    pub fn set_pin_hook(&self, hook: PinHook) {
        *lock(&self.inner.pin_hook) = Some(hook);
    }

    /// How a chat can run on `account` beside the live login: `"env"` (a
    /// credential in its process), `"root"` (a config root of its own), or
    /// `None` (not on this device, or not this kind of login: an API key
    /// bills per token, so spreading over it saves nothing).
    pub fn pin_mechanism(&self, account: &AgentAccount) -> Option<&'static str> {
        if account.auth_kind == Some(AgentAuthKind::ApiKey) {
            return None;
        }
        match account.harness {
            HarnessId::ClaudeCode => self
                .read_slot(account.harness, &account.id)
                .is_some_and(|slot| slot.credentials.get("claudeAiOauth").is_some())
                .then_some("env"),
            HarnessId::Muse if account.id != OWN_LOGIN => {
                self.read_slot(account.harness, &account.id).is_some().then_some("root")
            }
            _ => None,
        }
    }

    /// Every chat pinned to an account.
    pub fn pins(&self) -> Vec<ChatPin> {
        let mut pins: Vec<ChatPin> = lock(&self.inner.pins)
            .chats
            .iter()
            .flat_map(|(chat, by_harness)| {
                by_harness.iter().filter_map(move |(harness, entry)| {
                    Some(ChatPin {
                        chat: chat.clone(),
                        harness: *harness,
                        account: entry.account.clone()?,
                        since: entry.since,
                    })
                })
            })
            .collect();
        pins.sort_by(|a, b| (a.since, &a.chat).cmp(&(b.since, &b.chat)));
        pins
    }

    /// Pin `chat` to `account` (or, with `None`, take its pin back). What the
    /// account is, and whether it can be pinned, is the caller's to have
    /// checked.
    pub fn set_pin(&self, harness: HarnessId, chat: &str, account: Option<&str>) {
        let now = now_ms();
        let mut state = lock(&self.inner.pins);
        let by_harness = state.chats.entry(chat.to_owned()).or_default();
        match account {
            Some(account) => {
                by_harness.insert(harness, PinEntry { account: Some(account.to_owned()), since: now });
            }
            None => {
                // Remembered, so a session parked on the account moves back.
                if by_harness.get(&harness).is_some_and(|e| e.account.is_some()) {
                    by_harness.insert(harness, PinEntry { account: None, since: now });
                }
            }
        }
        state.chats.retain(|_, by| {
            by.retain(|_, e| e.account.is_some() || now - e.since < CLEARED_KEPT_MS);
            !by.is_empty()
        });
        let snapshot = state.clone();
        drop(state);
        self.persist_pins(&snapshot);
    }

    /// The chat's pin, asking the plugins' `run.preparing` hook first when
    /// it has none yet.
    async fn pin_for_run(&self, harness: HarnessId, chat: &str) -> Option<String> {
        let existing = lock(&self.inner.pins).entry(harness, chat).cloned();
        if let Some(entry) = existing {
            return entry.account;
        }
        let hook = lock(&self.inner.pin_hook).clone()?;
        hook(harness, chat.to_owned()).await
    }

    /// What a run of `chat` needs to reach its account (`None`: the live
    /// login, as without a pin). `Err`: the account can't be used and the
    /// run can't start (the words say what to do).
    pub(crate) async fn pin_run(&self, harness: HarnessId, account: &str) -> Result<Option<PinRun>, String> {
        match harness {
            HarnessId::ClaudeCode => {
                let Some(slot) = self.read_slot(harness, account) else { return Ok(None) };
                // The live login runs as it always did, and is never
                // refreshed here: the CLI holds its refresh token.
                if !self.claude_slot_is_not_live(&slot).await {
                    return Ok(None);
                }
                let oauth = slot.credentials.get("claudeAiOauth").cloned().unwrap_or_default();
                let who = slot.profile.email.clone();
                let signed_out = || format!("{who} needs signing in again (Settings → Accounts), or pin this chat to another account");
                let mut token = str_field(&oauth, "accessToken").ok_or_else(signed_out)?;
                let expires = oauth.get("expiresAt").and_then(|v| v.as_i64()).unwrap_or(0);
                if expires - now_ms() < CLAUDE_REFRESH_MARGIN.as_millis() as i64 {
                    match self.refresh_claude_slot(&slot).await {
                        Ok(Some(fresh)) => token = fresh,
                        // Another refresh of this slot is in flight: its
                        // result is on disk by the next run.
                        Ok(None) => {}
                        Err(_) => return Err(signed_out()),
                    }
                }
                Ok(Some(PinRun {
                    credential: fingerprint("claude", &token),
                    env: BTreeMap::from([("CLAUDE_CODE_OAUTH_TOKEN".to_owned(), token)]),
                }))
            }
            HarnessId::Muse => {
                if self.read_slot(harness, account).is_none() {
                    return Ok(None);
                }
                let env = self.muse_env(account).map_err(|e| e.to_string())?;
                Ok(Some(PinRun { env, credential: account.to_owned() }))
            }
            _ => Ok(None),
        }
    }
}

/// The agent accounts' spawn-time environment: a pinned chat's account, and
/// Muse's zeron-kept account ([`MuseRunEnvironment`]) for the chats that
/// aren't pinned.
pub struct PinRunEnvironment(pub AgentAccounts);

#[async_trait::async_trait]
impl crate::registry::RunEnvironment for PinRunEnvironment {
    async fn prepare(
        &self,
        harness: HarnessId,
        chat_id: Option<&str>,
        request: &mut zeron_proto::RunRequest,
    ) -> Result<(), String> {
        if pinnable(harness)
            && let Some(chat) = chat_id
            && let Some(account) = self.0.pin_for_run(harness, chat).await
            && let Some(run) = self.0.pin_run(harness, &account).await?
        {
            // Never over what another provider (the Cloud broker) set.
            for (key, value) in run.env {
                request.env.entry(key).or_insert(value);
            }
            return Ok(());
        }
        MuseRunEnvironment(self.0.clone()).prepare(harness, chat_id, request).await
    }

    async fn credential(&self, harness: HarnessId, chat_id: Option<&str>) -> Option<String> {
        let pinned = chat_id
            .filter(|_| pinnable(harness))
            .and_then(|chat| lock(&self.0.inner.pins).entry(harness, chat).cloned());
        if let Some(entry) = pinned {
            // A pin taken back reads as a change too, so a parked session
            // moves to the live login before its next turn.
            let run = match entry.account {
                Some(account) => self.0.pin_run(harness, &account).await.ok().flatten(),
                None => None,
            };
            if let Some(run) = run {
                return Some(run.credential);
            }
            if harness != HarnessId::Muse {
                return Some(LIVE.to_owned());
            }
        }
        MuseRunEnvironment(self.0.clone()).credential(harness, chat_id).await
    }

    async fn record_usage(
        &self,
        harness: HarnessId,
        credential: &str,
        windows: Vec<zeron_proto::AgentUsageWindow>,
        plan_label: Option<String>,
    ) {
        MuseRunEnvironment(self.0.clone())
            .record_usage(harness, credential, windows, plan_label)
            .await;
    }
}

#[cfg(test)]
mod tests;
