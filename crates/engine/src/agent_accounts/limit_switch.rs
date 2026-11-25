//! A chat whose account hit its usage limit, running meanwhile on another
//! account (its orchestrator's `CrewPolicy::on_limit`; the engine side is
//! `crate::account_switch`). The accounts side is a pin ([`super::pins`])
//! plus a record of what the chat ran on before, so the switch back after
//! the reset restores exactly that. Records are kept in
//! `{data_dir}/agent-accounts/limit-switches.json`, so a switch survives a
//! restart.
//!
//! Which account takes over ([`pick_switch_account`]): a connected saved
//! login of the harness that a chat can run on beside the live login, whose
//! usage was probed recently and is below 100% in every window (a window
//! whose reset has passed counts as empty); the one with the most headroom.

use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};
use zeron_proto::{AgentAccount, AgentAuthKind, ChatConfig, HarnessId};

use super::{AgentAccounts, lock, now_ms, write_file_atomic};

/// `{data_dir}/agent-accounts/limit-switches.json`.
pub(super) const SWITCHES_FILE: &str = "limit-switches.json";
/// A record still there this long after its switch belongs to a chat that
/// is gone: dropped.
const KEPT_MS: i64 = 30 * 24 * 60 * 60 * 1000;
/// Usage probed at most this long ago is fresh enough to switch on.
pub const FRESH_USAGE_MS: i64 = 10 * 60 * 1000;

/// A chat running on another account while its own is limited.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LimitSwitch {
    /// What it runs on now: the harness and the account (slot id).
    pub harness: HarnessId,
    pub account: String,
    /// That account, as the person reads it.
    pub label: String,
    /// What it ran on before: its harness, and its pin there (`None`: the
    /// machine's login).
    pub from_harness: HarnessId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_account: Option<String>,
    /// The account whose limit started it (kept out of every pick until
    /// the switch back).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limited_account: Option<String>,
    /// Another harness only: the chat's settings before, and the ones the
    /// switch set (put back only while they are still what it set).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_config: Option<ChatConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<ChatConfig>,
    /// Epoch millis of the first switch.
    pub at: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(super) struct SwitchState {
    #[serde(default)]
    chats: HashMap<String, LimitSwitch>,
}

impl SwitchState {
    pub(super) fn load(file: &Path) -> Self {
        let mut state: Self = std::fs::read_to_string(file)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default();
        let now = now_ms();
        state.chats.retain(|_, s| now - s.at < KEPT_MS);
        state
    }
}

/// What the account `a` has left now (1.0 = all of it), when a limited
/// chat may go to it: a fresh probe (at most [`FRESH_USAGE_MS`] old) whose
/// every window is below 100% or has reset since. `None`: unknown, signed
/// out, or full.
pub fn headroom(a: &AgentAccount, now: i64) -> Option<f32> {
    if a.usage_error.as_deref().is_some_and(signed_out) {
        return None;
    }
    let fetched = a.usage_fetched_at?;
    if now - fetched > FRESH_USAGE_MS || a.usage_windows.is_empty() {
        return None;
    }
    let mut used = 0f32;
    for w in &a.usage_windows {
        let reset = w.resets_at.is_some_and(|at| at.timestamp_millis() <= now);
        let fraction = if reset { 0.0 } else { w.used_fraction.max(0.0) };
        if fraction >= 1.0 {
            return None;
        }
        used = used.max(fraction);
    }
    Some(1.0 - used)
}

fn signed_out(error: &str) -> bool {
    zeron_proto::run_errors::provider_problem(error) == Some(zeron_proto::run_errors::ProviderProblem::SignedOut)
        || error.to_ascii_lowercase().contains("sign in")
}

/// The account of `harness` a limited chat continues on: switchable, a
/// subscription login (never an API key, never one its provider rejected), one `runnable` says a chat can
/// run on, not in `exclude`, with quota now ([`headroom`]); the most
/// headroom wins, the page's order breaking ties.
pub fn pick_switch_account<'a>(
    accounts: &'a [AgentAccount],
    harness: HarnessId,
    exclude: &[&str],
    now: i64,
    runnable: impl Fn(&AgentAccount) -> bool,
) -> Option<&'a AgentAccount> {
    accounts
        .iter()
        .filter(|a| a.harness == harness && a.switchable && !exclude.contains(&a.id.as_str()))
        .filter(|a| a.auth_kind != Some(AgentAuthKind::ApiKey))
        // (Never one its provider rejected.)
        .filter(|a| !a.invalid)
        .filter_map(|a| Some((a, headroom(a, now)?)))
        .filter(|(a, _)| runnable(a))
        .fold(None, |best: Option<(&AgentAccount, f32)>, (a, room)| match best {
            Some((_, best_room)) if best_room >= room => best,
            _ => Some((a, room)),
        })
        .map(|(a, _)| a)
}

impl AgentAccounts {
    fn persist_limit_switches(&self, state: &SwitchState) {
        let Ok(root) = self.inner.config.private_root() else { return };
        let Ok(bytes) = serde_json::to_vec_pretty(state) else { return };
        if let Err(err) = write_file_atomic(&root.join(SWITCHES_FILE), &bytes, false) {
            tracing::warn!(error = %err, "could not save the chats' account switches");
        }
    }

    /// `chat`'s switch to another account, while it lasts.
    pub fn limit_switch(&self, chat: &str) -> Option<LimitSwitch> {
        lock(&self.inner.limit_switches).chats.get(chat).cloned()
    }

    /// Run `chat` on `account` (of `harness`) from its next run: its pin
    /// there, and a record of what it ran on before (`from_harness`, its
    /// pin, the limited account, and for another harness its settings
    /// before and after). A chat switched already keeps its first record's
    /// "before".
    #[allow(clippy::too_many_arguments)]
    pub fn begin_limit_switch(
        &self,
        chat: &str,
        from_harness: HarnessId,
        limited_account: Option<&str>,
        harness: HarnessId,
        account: &AgentAccount,
        configs: Option<(ChatConfig, ChatConfig)>,
    ) -> LimitSwitch {
        let label = account_label(account);
        let mut state = lock(&self.inner.limit_switches);
        let record = match state.chats.get(chat) {
            Some(earlier) => {
                // Off the account it went to: that pin goes when it moves
                // to another harness.
                if earlier.harness != harness {
                    self.set_pin(earlier.harness, chat, None);
                }
                let (from_config, config) = match configs {
                    Some((_, set)) => (earlier.from_config.clone(), Some(set)),
                    None => (earlier.from_config.clone(), earlier.config.clone()),
                };
                LimitSwitch {
                    harness,
                    account: account.id.clone(),
                    label,
                    from_config,
                    config,
                    ..earlier.clone()
                }
            }
            None => {
                let (from_config, config) = configs.map_or((None, None), |(from, set)| (Some(from), Some(set)));
                LimitSwitch {
                    harness,
                    account: account.id.clone(),
                    label,
                    from_harness,
                    from_account: self.pinned_account(from_harness, chat),
                    limited_account: limited_account.map(str::to_owned),
                    from_config,
                    config,
                    at: now_ms(),
                }
            }
        };
        self.set_pin(harness, chat, Some(&account.id));
        state.chats.insert(chat.to_owned(), record.clone());
        let snapshot = state.clone();
        drop(state);
        self.persist_limit_switches(&snapshot);
        tracing::info!(chat, harness = ?harness, slot = %record.account, "chat continues on another account while its own is limited");
        record
    }

    /// Back to what `chat` ran on before its switch: its pins as they were
    /// (the settings are the caller's to put back). The record, `None` when
    /// it wasn't switched.
    pub fn end_limit_switch(&self, chat: &str) -> Option<LimitSwitch> {
        let mut state = lock(&self.inner.limit_switches);
        let record = state.chats.remove(chat)?;
        if record.harness != record.from_harness {
            self.set_pin(record.harness, chat, None);
        }
        self.set_pin(record.from_harness, chat, record.from_account.as_deref());
        let snapshot = state.clone();
        drop(state);
        self.persist_limit_switches(&snapshot);
        tracing::info!(chat, harness = ?record.from_harness, "chat back on its own account after the reset");
        Some(record)
    }
}

/// An account as the person reads it: its email, else its name.
pub fn account_label(a: &AgentAccount) -> String {
    [a.email.as_deref(), a.display_name.as_deref(), a.organization.as_deref()]
        .into_iter()
        .flatten()
        .map(str::trim)
        .find(|s| !s.is_empty())
        .map_or_else(|| format!("account {}", &a.id[..a.id.len().min(6)]), str::to_owned)
}

#[cfg(test)]
pub(crate) mod fixtures;
#[cfg(test)]
mod tests;
