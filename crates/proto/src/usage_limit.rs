//! A chat stopped by its provider's usage limit (a Claude 5-hour or weekly
//! window, a usage limit, a Codex quota): it waits for the limit to reset and
//! is resumed then by its host ("[Resumed] The usage limit reset…"), or,
//! where its orchestrator allows it ([`LimitPolicy`]), continues meanwhile on
//! another account slot of the same harness that has quota.
//!
//! The chat's host writes [`SessionLimit`] on the chat's session row
//! (`Session::limited`) and keeps it until the chat is resumed; every device
//! reads it (the board treats a limited thread as at work, the clients show
//! "Waiting for limit" and "resets 14:05", spaced).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A chat its provider's usage limit stopped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionLimit {
    /// The harness whose limit it hit (`claude`, `codex`, …).
    pub provider: String,
    /// The account slot (`AgentAccount::id`) the chat ran on when it hit
    /// the limit; `None` when it can't be told.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_slot: Option<String>,
    /// That account, as the person reads it (its email or name).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_label: Option<String>,
    /// Which window: "5-hour", "weekly", "usage", "quota".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<String>,
    /// When the limit resets: from the error's own words, else the account's
    /// usage probe; `None` while unknown (re-checked every
    /// [`RECHECK_SECS`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until: Option<DateTime<Utc>>,
    /// When the limit stopped it.
    pub since: DateTime<Utc>,
    /// Resumes in a row that ran straight back into the limit.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub attempt: u32,
    /// Running meanwhile on another account (its orchestrator's
    /// [`LimitPolicy::switch_account`]): switched back after `until`, at a
    /// turn boundary.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub switched_to: Option<SwitchedAccount>,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

/// The account a limited chat continues on meanwhile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SwitchedAccount {
    /// Its slot (`AgentAccount::id`).
    pub account_slot: String,
    /// As the person reads it.
    pub account_label: String,
    /// When it switched.
    pub at: DateTime<Utc>,
    /// Another harness and model, only when [`LimitPolicy::switch_harness`]
    /// allowed one (no slot of its own harness had quota).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

impl SessionLimit {
    /// Still waiting on its own account (not switched to another).
    pub fn waiting(&self) -> bool {
        self.switched_to.is_none()
    }
}

/// What an orchestrator's threads (it and its crew) do when their account
/// hits its usage limit. Off by default: they wait for the reset.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LimitPolicy {
    /// "When limited, continue on another account": another connected
    /// account slot of the same harness with quota.
    #[serde(default)]
    pub switch_account: bool,
    /// And, with none, another connected harness or model.
    #[serde(default)]
    pub switch_harness: bool,
}

/// How often a limit with no known reset is checked again.
pub const RECHECK_SECS: i64 = 15 * 60;
/// The resumes after a reset are spread out by this much, at least and at
/// most (orchestrators first, then crew), with jitter.
pub const STAGGER_MIN_SECS: i64 = 20;
pub const STAGGER_MAX_SECS: i64 = 60;

/// What a chat is sent when its limit reset.
pub const RESUMED_AFTER_LIMIT: &str = "[Resumed] The usage limit reset; continue where you left off.";

/// What a chat is sent when it continues on another account meanwhile.
pub fn switched_message(to: &str, from: &str) -> String {
    format!("[Resumed] Switched to {to} while {from} is limited; continue where you left off.")
}
