//! Accounts on disk under a test's own root (never a real login): saved
//! Claude Code and Codex logins, a live one, and fresh usage.

use std::path::Path;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as BASE64_URL;
use zeron_proto::{AgentAuthKind, AgentUsageWindow, HarnessId};

use super::super::{
    AgentAccounts, AgentAccountsConfig, Slot, SlotProfile, UsageEntry, UsageSnapshot, lock, now_ms, slot_id_for, usage_key,
};

/// Accounts whose every store lives under `root`.
pub(crate) fn accounts(root: &Path) -> AgentAccounts {
    AgentAccounts::new(AgentAccountsConfig::isolated(root))
}

fn jwt(claims: serde_json::Value) -> String {
    format!("e30.{}.sig", BASE64_URL.encode(serde_json::to_vec(&claims).unwrap()))
}

/// A Codex `auth.json` for `email` in workspace `workspace` (its account key).
pub(crate) fn codex_auth(email: &str, workspace: &str, access: &str, refreshed: &str) -> serde_json::Value {
    let id_token = jwt(serde_json::json!({
        "email": email,
        "https://api.openai.com/auth": { "chatgpt_account_id": workspace, "chatgpt_plan_type": "plus" },
    }));
    serde_json::json!({
        "tokens": { "id_token": id_token, "access_token": access, "refresh_token": format!("rt-{access}"), "account_id": workspace },
        "last_refresh": refreshed,
    })
}

fn profile(email: &str) -> SlotProfile {
    SlotProfile { email: email.into(), display_name: None, organization: None, plan: None, auth_kind: AgentAuthKind::Oauth }
}

/// A saved Claude Code login (`uuid`, `email`, access token `access`): its slot id.
pub(crate) fn claude_slot(accounts: &AgentAccounts, uuid: &str, email: &str, access: &str) -> String {
    let slot = Slot {
        id: slot_id_for(HarnessId::ClaudeCode, uuid),
        harness: HarnessId::ClaudeCode,
        account_key: uuid.into(),
        profile: profile(email),
        credentials: serde_json::json!({ "claudeAiOauth": {
            "accessToken": access, "refreshToken": format!("rt-{access}"), "expiresAt": now_ms() + 3_600_000,
        }}),
        claude_config: Some(serde_json::json!({ "oauthAccount": { "accountUuid": uuid, "emailAddress": email } })),
        saved_at: now_ms(),
        created_at: None,
        store_key: None,
        user_id: None,
    };
    accounts.write_slot(&slot).unwrap();
    slot.id
}

/// A saved Codex login (workspace `workspace` is its account key): its slot id.
pub(crate) fn codex_slot(accounts: &AgentAccounts, workspace: &str, email: &str, access: &str) -> String {
    let slot = Slot {
        id: slot_id_for(HarnessId::Codex, workspace),
        harness: HarnessId::Codex,
        account_key: workspace.into(),
        profile: profile(email),
        credentials: codex_auth(email, workspace, access, "2026-11-17T10:00:00Z"),
        claude_config: None,
        saved_at: now_ms(),
        created_at: None,
        store_key: None,
        user_id: None,
    };
    accounts.write_slot(&slot).unwrap();
    slot.id
}

/// Make `uuid` the machine's Claude Code login (under the test root).
pub(crate) fn live_claude(root: &Path, uuid: &str, email: &str) {
    let config = AgentAccountsConfig::isolated(root);
    std::fs::create_dir_all(&config.claude_config_dir).unwrap();
    std::fs::write(
        &config.claude_config_file,
        serde_json::json!({ "oauthAccount": { "accountUuid": uuid, "emailAddress": email } }).to_string(),
    )
    .unwrap();
    std::fs::write(
        config.claude_creds_file(),
        serde_json::json!({ "claudeAiOauth": {
            "accessToken": "live-token", "refreshToken": "live-rt", "expiresAt": now_ms() + 3_600_000,
        }})
        .to_string(),
    )
    .unwrap();
}

/// Make `workspace` the machine's Codex login (under the test root).
pub(crate) fn live_codex(root: &Path, workspace: &str, email: &str) {
    let config = AgentAccountsConfig::isolated(root);
    std::fs::create_dir_all(&config.codex_home).unwrap();
    let auth = codex_auth(email, workspace, "live-codex-token", "2026-11-17T09:00:00Z");
    std::fs::write(config.codex_auth_file(), auth.to_string()).unwrap();
}

/// One usage window `used` full, resetting in `resets_in_ms` (negative:
/// reset already).
pub(crate) fn window(used: f32, resets_in_ms: i64) -> AgentUsageWindow {
    AgentUsageWindow {
        label: "Session".into(),
        used_fraction: used,
        resets_at: chrono::DateTime::from_timestamp_millis(now_ms() + resets_in_ms),
    }
}

/// Usage of `harness`'s account `account_key`, probed `age_ms` ago.
pub(crate) fn usage(accounts: &AgentAccounts, harness: HarnessId, account_key: &str, windows: Vec<AgentUsageWindow>, age_ms: i64) {
    let at = now_ms() - age_ms;
    let mut usage = lock(&accounts.inner.usage);
    usage.insert(
        usage_key(harness, account_key),
        UsageEntry {
            usage: Some(UsageSnapshot { windows, plan_label: None, codex_reset_credits: None }),
            fetched_at: Some(at),
            checked_at: at,
            ..Default::default()
        },
    );
    accounts.persist_usage(&usage);
}

/// The run environment pinned chats get, as the engine installs it.
pub(crate) fn run_environment(accounts: &AgentAccounts) -> super::super::PinRunEnvironment {
    super::super::PinRunEnvironment(accounts.clone())
}
