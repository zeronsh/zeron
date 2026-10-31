//! Settings → Providers for Cloud: the account calls the page makes for a
//! device (`ListAgentAccounts`, `ActivateAgentAccount`, `ForgetAgentAccount`,
//! the sign-in trio) aimed at [`CLOUD_ACCOUNTS_DEVICE`], answered by this
//! device's engine from the vault — Cloud's logins live there, not on a
//! machine.
//!
//! A harness card holds its subscription logins and its API keys as accounts
//! (Claude Code: `claude` + `anthropic-key`; Codex: `codex` + `openai-key`),
//! exactly one of them active: every Cloud machine uses it from its next
//! turn. The broker prefers an active subscription login, so activating a key
//! also clears the active login. Sign-ins are capture-only on this device
//! (the tokens go straight to the vault and become the active login); usage
//! is read by the vault, which alone holds the tokens.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use serde::Deserialize;
use zeron_proto::{
    AgentAccount, AgentAccountsSnapshot, AgentAuthKind, AgentUsageWindow, CLOUD_ACCOUNTS_DEVICE,
    HarnessId, VaultAccount, VaultConnectionStatus, VaultProvider,
};
use zeron_rpc::{RpcError, RpcReply, methods, parse_params};

use crate::agent_accounts::AgentAccounts;
use crate::cloud_client::CloudClient;

/// A login's usage is re-read after this unless the page forces a refresh.
const USAGE_TTL: Duration = Duration::from_secs(5 * 60);

/// The account calls answered here when aimed at [`CLOUD_ACCOUNTS_DEVICE`].
pub(crate) fn handles(method: &str) -> bool {
    matches!(
        method,
        methods::LIST_AGENT_ACCOUNTS
            | methods::ACTIVATE_AGENT_ACCOUNT
            | methods::FORGET_AGENT_ACCOUNT
            | methods::START_AGENT_LOGIN
            | methods::COMPLETE_AGENT_LOGIN
            | methods::POLL_AGENT_LOGIN
            | methods::CANCEL_AGENT_LOGIN
    )
}

/// The vault providers behind a harness card: its subscription login and
/// its API key.
fn providers_of(harness: HarnessId) -> Option<(VaultProvider, VaultProvider)> {
    match harness {
        HarnessId::ClaudeCode => Some((VaultProvider::Claude, VaultProvider::AnthropicKey)),
        HarnessId::Codex => Some((VaultProvider::Codex, VaultProvider::OpenaiKey)),
        _ => None,
    }
}

const CLOUD_HARNESSES: [HarnessId; 2] = [HarnessId::ClaudeCode, HarnessId::Codex];

fn failed(error: impl std::fmt::Display) -> RpcError {
    RpcError::Failed(error.to_string())
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct ListParams {
    #[serde(default)]
    force_usage: Option<bool>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AccountParams {
    harness: HarnessId,
    account_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StartParams {
    harness: HarnessId,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CompleteParams {
    login_id: String,
    code: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LoginParams {
    login_id: String,
}

/// Answer one account call for Cloud.
pub(crate) async fn dispatch(
    cloud: CloudClient,
    accounts: AgentAccounts,
    method: &str,
    params: serde_json::Value,
) -> Result<RpcReply, RpcError> {
    match method {
        methods::LIST_AGENT_ACCOUNTS => {
            let p: ListParams = parse_params(params).unwrap_or_default();
            RpcReply::value(&snapshot(&cloud, p.force_usage.unwrap_or(false)).await?)
        }
        methods::ACTIVATE_AGENT_ACCOUNT => {
            let p: AccountParams = parse_params(params)?;
            let (provider, slot) = account_ref(p.harness, &p.account_id)?;
            let (login, key) = providers_of(p.harness).ok_or_else(not_cloud)?;
            cloud
                .vault_activate(provider, Some(slot))
                .await
                .map_err(failed)?;
            if provider == key {
                // Machines prefer a subscription login: clear it so the key
                // is what they use.
                cloud.vault_activate(login, None).await.map_err(failed)?;
            }
            RpcReply::value(&snapshot(&cloud, false).await?)
        }
        methods::FORGET_AGENT_ACCOUNT => {
            let p: AccountParams = parse_params(params)?;
            let (provider, slot) = account_ref(p.harness, &p.account_id)?;
            cloud.vault_forget(provider, slot).await.map_err(failed)?;
            forget_usage(provider, slot);
            RpcReply::value(&snapshot(&cloud, false).await?)
        }
        methods::START_AGENT_LOGIN => {
            let p: StartParams = parse_params(params)?;
            if providers_of(p.harness).is_none() {
                return Err(not_cloud());
            }
            if !cloud.is_available() {
                return Err(RpcError::Failed(crate::cloud_client::UNAVAILABLE.into()));
            }
            // Logins are authorized for the logical Cloud device: every
            // session machine (its child) may draw grants from them.
            let device = cloud
                .status()
                .await
                .map_err(failed)?
                .device_id
                .ok_or_else(|| {
                    RpcError::Failed("Turn Cloud on in Settings → Cloud first.".into())
                })?;
            let sink = crate::cloud_client::vault_upload_sink(cloud, p.harness, vec![device]);
            let start = accounts
                .start_capture_login(p.harness, sink)
                .await
                .map_err(failed)?;
            RpcReply::value(&start)
        }
        // The capture flow runs on this engine, so it is driven here too.
        methods::POLL_AGENT_LOGIN => {
            let p: LoginParams = parse_params(params)?;
            RpcReply::value(&accounts.poll_login(&p.login_id).await.map_err(failed)?)
        }
        methods::CANCEL_AGENT_LOGIN => {
            let p: LoginParams = parse_params(params)?;
            accounts.cancel_login(&p.login_id);
            RpcReply::value(&serde_json::json!({ "ok": true }))
        }
        methods::COMPLETE_AGENT_LOGIN => {
            let p: CompleteParams = parse_params(params)?;
            // The upload has happened once this returns; answer with Cloud's
            // accounts, not this device's.
            accounts
                .complete_login(&p.login_id, &p.code)
                .await
                .map_err(failed)?;
            RpcReply::value(&snapshot(&cloud, false).await?)
        }
        other => Err(RpcError::UnknownMethod(other.to_string())),
    }
}

fn not_cloud() -> RpcError {
    RpcError::Failed("Cloud runs Codex and Claude Code only".into())
}

/// `"{provider}:{slot}"` → the vault provider (one of `harness`'s) and slot.
fn account_ref(harness: HarnessId, account_id: &str) -> Result<(VaultProvider, &str), RpcError> {
    let (login, key) = providers_of(harness).ok_or_else(not_cloud)?;
    let (provider, slot) = account_id
        .split_once(':')
        .and_then(|(p, slot)| Some((VaultProvider::parse(p)?, slot)))
        .filter(|(p, slot)| (*p == login || *p == key) && !slot.is_empty())
        .ok_or_else(|| RpcError::Failed(format!("Unknown Cloud account {account_id}")))?;
    Ok((provider, slot))
}

/// Cloud's accounts as the Providers page renders a device's.
async fn snapshot(
    cloud: &CloudClient,
    force_usage: bool,
) -> Result<AgentAccountsSnapshot, RpcError> {
    let mut out = Vec::new();
    for harness in CLOUD_HARNESSES {
        let (login, key) = providers_of(harness).ok_or_else(not_cloud)?;
        let (logins, keys) =
            futures::future::join(cloud.vault_accounts(login), cloud.vault_accounts(key)).await;
        let logins = logins.map_err(failed)?;
        let keys = keys.map_err(failed)?;
        // One active per card: a subscription login wins (the broker's order).
        let login_active = logins.iter().any(|a| a.active);
        let usage = futures::future::join_all(logins.iter().map(|account| async move {
            if account.kind == "oauth" && account.status == VaultConnectionStatus::Connected {
                Some(usage(cloud, harness, login, &account.slot, force_usage).await)
            } else {
                None
            }
        }))
        .await;
        for (account, usage) in logins.iter().zip(usage) {
            out.push(to_agent_account(
                harness,
                login,
                account,
                account.active,
                usage,
            ));
        }
        for account in &keys {
            out.push(to_agent_account(
                harness,
                key,
                account,
                account.active && !login_active,
                None,
            ));
        }
    }
    Ok(AgentAccountsSnapshot {
        accounts: out,
        warnings: Vec::new(),
    })
}

fn to_agent_account(
    harness: HarnessId,
    provider: VaultProvider,
    account: &VaultAccount,
    active: bool,
    usage: Option<CachedUsage>,
) -> AgentAccount {
    let reconnect = account.status == VaultConnectionStatus::NeedsReconnect;
    let api_key = account.kind == "api-key";
    let (windows, plan, fetched_at, usage_error) = match usage {
        Some(usage) => (
            usage.windows,
            usage.plan,
            Some(usage.fetched_at),
            usage.error,
        ),
        None => (Vec::new(), None, None, None),
    };
    // The vault's `account` is an email for an identified login, else a
    // label (a Claude login stored without its profile names its plan).
    let label = account.account.as_deref();
    let label_is_email = label.is_some_and(|l| l.contains('@'));
    AgentAccount {
        id: format!("{}:{}", provider.as_str(), account.slot),
        harness,
        email: if api_key {
            None
        } else {
            account
                .email
                .clone()
                .or_else(|| label.filter(|_| label_is_email).map(str::to_string))
        },
        plan_label: plan.or_else(|| account.plan.clone()).or_else(|| {
            if api_key {
                Some(format!("API key {}", label.unwrap_or_default()))
            } else {
                label.filter(|_| !label_is_email).map(plan_name)
            }
        }),
        active,
        usage_windows: windows,
        usage_fetched_at: fetched_at,
        usage_error: if reconnect {
            Some("Sign in again — Cloud can no longer use this login".into())
        } else {
            usage_error
        },
        display_name: account.display_name.clone(),
        organization: account.organization.clone(),
        auth_kind: Some(if api_key {
            AgentAuthKind::ApiKey
        } else {
            AgentAuthKind::Oauth
        }),
        switchable: !reconnect,
        saved_at: Some(account.updated_at),
        provider: None,
    }
}

/// A stored subscription type as the plan badge reads it (`max` → `Max`).
fn plan_name(subscription: &str) -> String {
    let mut chars = subscription.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

#[derive(Clone)]
struct CachedUsage {
    windows: Vec<AgentUsageWindow>,
    plan: Option<String>,
    error: Option<String>,
    fetched_at: i64,
    at: Instant,
}

/// Usage per login, process-wide (slot ids are random per login, so keys
/// never collide across users).
fn usage_cache() -> &'static Mutex<HashMap<String, CachedUsage>> {
    static CACHE: OnceLock<Mutex<HashMap<String, CachedUsage>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

fn usage_key(provider: VaultProvider, slot: &str) -> String {
    format!("{}:{slot}", provider.as_str())
}

fn forget_usage(provider: VaultProvider, slot: &str) {
    usage_cache()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .remove(&usage_key(provider, slot));
}

async fn usage(
    cloud: &CloudClient,
    harness: HarnessId,
    provider: VaultProvider,
    slot: &str,
    force: bool,
) -> CachedUsage {
    let key = usage_key(provider, slot);
    let cached = usage_cache()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&key)
        .cloned();
    if let Some(cached) = &cached
        && !force
        && cached.at.elapsed() < USAGE_TTL
    {
        return cached.clone();
    }
    let fresh = match cloud.vault_usage(provider, slot).await {
        Ok(body) => match crate::agent_accounts::usage_windows_from_reply(harness, &body) {
            Some((windows, plan)) => CachedUsage {
                windows,
                plan,
                error: None,
                fetched_at: crate::now_ms(),
                at: Instant::now(),
            },
            None => CachedUsage {
                error: Some("Usage unavailable".into()),
                ..stale(cached.as_ref())
            },
        },
        // Keep the last good windows beside the reason.
        Err(error) => CachedUsage {
            error: Some(error.to_string()),
            ..stale(cached.as_ref())
        },
    };
    usage_cache()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(key, fresh.clone());
    fresh
}

fn stale(previous: Option<&CachedUsage>) -> CachedUsage {
    match previous {
        Some(previous) => CachedUsage {
            at: Instant::now(),
            ..previous.clone()
        },
        None => CachedUsage {
            windows: Vec::new(),
            plan: None,
            error: None,
            fetched_at: crate::now_ms(),
            at: Instant::now(),
        },
    }
}

/// The harnesses Cloud can run now: an active account (login or key) that
/// still works. `None` when the vault can't be read.
pub(crate) async fn ready_harnesses(cloud: &CloudClient) -> Option<Vec<HarnessId>> {
    let vault = cloud.vault_status().await.ok().filter(|v| v.available)?;
    Some(ready_from(&vault))
}

fn ready_from(vault: &zeron_proto::VaultStatus) -> Vec<HarnessId> {
    let ready = |provider: VaultProvider| {
        vault.connections.iter().any(|c| {
            c.provider == provider
                && c.status == VaultConnectionStatus::Connected
                // An older vault holds one account and no `hasActive`.
                && (c.has_active || c.accounts == 0)
        })
    };
    CLOUD_HARNESSES
        .into_iter()
        .filter(|harness| {
            providers_of(*harness).is_some_and(|(login, key)| ready(login) || ready(key))
        })
        .collect()
}

/// Whether `target` is Cloud as the Providers page addresses it.
pub(crate) fn is_cloud_accounts_target(target: &str) -> bool {
    target == CLOUD_ACCOUNTS_DEVICE
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vault(slot: &str, kind: &str, active: bool) -> VaultAccount {
        VaultAccount {
            slot: slot.into(),
            status: VaultConnectionStatus::Connected,
            active,
            kind: kind.into(),
            account: Some(
                if kind == "api-key" {
                    "…abcd"
                } else {
                    "me@x.com"
                }
                .into(),
            ),
            email: (kind != "api-key").then(|| "me@x.com".into()),
            display_name: None,
            organization: None,
            plan: Some("Max".into()),
            authorized_devices: vec!["cloud-1".into()],
            created_at: 1,
            updated_at: 2,
        }
    }

    #[test]
    fn a_harness_is_ready_with_an_active_working_login_or_key() {
        let connection = |provider, status, has_active| zeron_proto::VaultConnection {
            provider,
            status,
            authorized_devices: vec![],
            account: None,
            updated_at: 0,
            accounts: 1,
            has_active,
        };
        let vault = |connections| zeron_proto::VaultStatus {
            available: true,
            connections,
            ..Default::default()
        };
        assert_eq!(ready_from(&vault(vec![])), Vec::<HarnessId>::new());
        assert_eq!(
            ready_from(&vault(vec![
                connection(
                    VaultProvider::Claude,
                    VaultConnectionStatus::Connected,
                    false
                ),
                connection(
                    VaultProvider::AnthropicKey,
                    VaultConnectionStatus::Connected,
                    true
                ),
                connection(
                    VaultProvider::Codex,
                    VaultConnectionStatus::NeedsReconnect,
                    true
                ),
            ])),
            vec![HarnessId::ClaudeCode]
        );
        assert_eq!(
            ready_from(&vault(vec![connection(
                VaultProvider::Codex,
                VaultConnectionStatus::Connected,
                true
            )])),
            vec![HarnessId::Codex]
        );
    }

    #[test]
    fn account_ids_name_one_of_the_cards_providers() {
        assert_eq!(
            account_ref(HarnessId::ClaudeCode, "anthropic-key:a1").unwrap(),
            (VaultProvider::AnthropicKey, "a1")
        );
        assert!(account_ref(HarnessId::ClaudeCode, "codex:a1").is_err());
        assert!(account_ref(HarnessId::Codex, "openai-key:").is_err());
        assert!(account_ref(HarnessId::Cursor, "codex:a1").is_err());
    }

    #[test]
    fn keys_read_as_api_key_accounts_and_logins_carry_their_identity() {
        let login = to_agent_account(
            HarnessId::ClaudeCode,
            VaultProvider::Claude,
            &vault("a1", "oauth", true),
            true,
            None,
        );
        assert_eq!(login.id, "claude:a1");
        assert_eq!(login.email.as_deref(), Some("me@x.com"));
        assert_eq!(login.auth_kind, Some(AgentAuthKind::Oauth));
        assert!(login.active && login.switchable);
        let key = to_agent_account(
            HarnessId::ClaudeCode,
            VaultProvider::AnthropicKey,
            &vault("k1", "api-key", false),
            false,
            None,
        );
        assert_eq!(key.email, None);
        assert_eq!(key.auth_kind, Some(AgentAuthKind::ApiKey));
        // A Claude login stored without its profile: its label is the plan.
        let mut bare = vault("a3", "oauth", true);
        bare.email = None;
        bare.plan = None;
        bare.account = Some("max".into());
        let bare = to_agent_account(
            HarnessId::ClaudeCode,
            VaultProvider::Claude,
            &bare,
            true,
            None,
        );
        assert_eq!(bare.email, None);
        assert_eq!(bare.plan_label.as_deref(), Some("Max"));
        let mut gone = vault("a2", "oauth", false);
        gone.status = VaultConnectionStatus::NeedsReconnect;
        let gone = to_agent_account(HarnessId::Codex, VaultProvider::Codex, &gone, false, None);
        assert!(!gone.switchable);
        assert!(gone.usage_error.unwrap().contains("Sign in again"));
    }
}
