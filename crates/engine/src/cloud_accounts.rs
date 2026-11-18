//! Settings → Providers for Cloud: the account calls the page makes for a
//! device (`ListAgentAccounts`, `ActivateAgentAccount`, `ForgetAgentAccount`,
//! the sign-in trio) aimed at [`CLOUD_ACCOUNTS_DEVICE`], answered by this
//! device's engine from the vault — Cloud's logins live there, not on a
//! machine.
//!
//! Every agent that runs on Cloud has a card, holding the vault accounts of
//! each credential it runs on there (`zeron_proto::providers::
//! cloud_credentials`): Claude Code's `claude` sign-ins and `anthropic-key`
//! keys, Pi's ChatGPT (`codex`) and Copilot sign-ins beside every vendor
//! key, Muse's `meta-key`, … A vendor's key is one vault provider shared by
//! every card that takes it. Within one of a card's groups the first
//! credential with an active account is the one machines use (a sign-in
//! before a key, the broker's order), so activating a key also clears the
//! sign-in it would otherwise stand behind. Agents that keep a login per
//! model provider (Pi, OpenCode, Hermes) show each group apart. Sign-ins are
//! capture-only on this device (the tokens go straight to the vault and
//! become the active login); usage is read by the vault, which alone holds
//! the tokens.

use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use serde::Deserialize;
use zeron_proto::providers::{
    CloudCredential, cloud_credentials, cloud_login_options, cloud_login_vault, groups_by_provider,
    provider_name, runs_on_cloud,
};
use zeron_proto::{
    AgentAccount, AgentAccountsSnapshot, AgentAuthKind, AgentUsageWindow, CLOUD_ACCOUNTS_DEVICE,
    HarnessId, VaultAccount, VaultConnectionStatus, VaultProvider,
};
use zeron_rpc::{RpcError, RpcReply, methods, parse_params};

use crate::agent_accounts::AgentAccounts;
use crate::cloud_client::CloudClient;

/// A login's usage is re-read after this unless the page forces a refresh.
const USAGE_TTL: Duration = Duration::from_secs(5 * 60);

/// The vault providers every vault answers for. A newer provider an older
/// vault refuses reads as holding no accounts instead of failing the page.
const ORIGINAL_PROVIDERS: [VaultProvider; 5] = [
    VaultProvider::Codex,
    VaultProvider::Claude,
    VaultProvider::Github,
    VaultProvider::AnthropicKey,
    VaultProvider::OpenaiKey,
];

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

/// The agents with a card on Cloud, in the catalog's order.
fn cloud_harnesses() -> impl Iterator<Item = HarnessId> {
    HarnessId::ALL.into_iter().filter(|h| runs_on_cloud(*h))
}

/// Every vault provider some card shows, once each.
fn cloud_vault_providers() -> Vec<VaultProvider> {
    let mut out: Vec<VaultProvider> = Vec::new();
    for credential in cloud_harnesses().flat_map(cloud_credentials) {
        if !out.contains(&credential.vault) {
            out.push(credential.vault);
        }
    }
    out
}

/// `harness`'s credential that `provider` is, if it takes one.
fn credential_of(harness: HarnessId, provider: VaultProvider) -> Option<&'static CloudCredential> {
    cloud_credentials(harness)
        .iter()
        .find(|c| c.vault == provider)
}

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
    /// The model provider to sign in to, on an agent that signs in per
    /// provider (Pi: `openai-codex`, `github-copilot`).
    #[serde(default)]
    provider: Option<String>,
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
            cloud
                .vault_activate(provider, Some(slot))
                .await
                .map_err(failed)?;
            if let Some(login) = login_stepping_aside(p.harness, provider) {
                // Machines use the sign-in before a key in the same group:
                // clear it so the key is what they use.
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
            let provider = p.provider.as_deref().filter(|p| !p.is_empty());
            let vault = cloud_login_vault(p.harness, provider)
                .ok_or_else(|| no_cloud_sign_in(p.harness, provider))?;
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
            let sink = crate::cloud_client::vault_upload_sink(cloud, vault, vec![device]);
            let start = accounts
                .start_capture_login(p.harness, provider, sink)
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

fn not_cloud(harness: HarnessId) -> RpcError {
    RpcError::Failed(format!("{} doesn't run on Cloud", provider_name(harness)))
}

/// Why `harness` has no Cloud sign-in for `provider`.
fn no_cloud_sign_in(harness: HarnessId, provider: Option<&str>) -> RpcError {
    if !runs_on_cloud(harness) {
        return not_cloud(harness);
    }
    let name = provider_name(harness);
    if harness == HarnessId::Pi && provider == Some("anthropic") {
        return RpcError::Failed(
            "Pi's Claude sign-in isn't available on Cloud — add an Anthropic API key instead."
                .into(),
        );
    }
    if cloud_login_options(harness).is_empty() {
        return RpcError::Failed(format!(
            "{name} on Cloud uses API keys — add one in Settings → Providers → Cloud."
        ));
    }
    let what = provider
        .map(|p| zeron_proto::providers::account_group_label(harness, p))
        .unwrap_or_else(|| name.to_string());
    RpcError::Failed(format!("{name} on Cloud can't sign in to {what}."))
}

/// The sign-in a machine would use before `provider` (a key) on `harness`'s
/// card — same group — which activating the key clears.
fn login_stepping_aside(harness: HarnessId, provider: VaultProvider) -> Option<VaultProvider> {
    let login = provider.login_it_replaces()?;
    let key = credential_of(harness, provider)?;
    let login_credential = credential_of(harness, login)?;
    (login_credential.group == key.group).then_some(login)
}

/// `"{provider}:{slot}"` → the vault provider (one of `harness`'s) and slot.
fn account_ref(harness: HarnessId, account_id: &str) -> Result<(VaultProvider, &str), RpcError> {
    if !runs_on_cloud(harness) {
        return Err(not_cloud(harness));
    }
    let (provider, slot) = account_id
        .split_once(':')
        .and_then(|(p, slot)| Some((VaultProvider::parse(p)?, slot)))
        .filter(|(p, slot)| credential_of(harness, *p).is_some() && !slot.is_empty())
        .ok_or_else(|| RpcError::Failed(format!("Unknown Cloud account {account_id}")))?;
    Ok((provider, slot))
}

/// Cloud's accounts as the Providers page renders a device's: each vault
/// provider read once (several cards share a vendor's key), concurrently.
async fn snapshot(
    cloud: &CloudClient,
    force_usage: bool,
) -> Result<AgentAccountsSnapshot, RpcError> {
    let providers = cloud_vault_providers();
    let fetched = futures::future::join_all(
        providers
            .iter()
            .map(|provider| async move { (*provider, cloud.vault_accounts(*provider).await) }),
    )
    .await;
    let mut by_provider: HashMap<VaultProvider, Vec<VaultAccount>> = HashMap::new();
    for (provider, listed) in fetched {
        match listed {
            Ok(listed) => {
                by_provider.insert(provider, listed);
            }
            Err(error) if ORIGINAL_PROVIDERS.contains(&provider) => return Err(failed(error)),
            Err(error) => {
                // A vault older than this build refuses providers it doesn't
                // know: those cards show no accounts yet.
                tracing::warn!(provider = provider.as_str(), %error, "Cloud accounts: provider unreadable");
            }
        }
    }
    // Usage once per subscription login, however many cards show it.
    let logins: Vec<(VaultProvider, String)> = by_provider
        .iter()
        .filter(|(provider, _)| usage_harness(**provider).is_some())
        .flat_map(|(provider, accounts)| {
            accounts
                .iter()
                .filter(|a| a.kind == "oauth" && a.status == VaultConnectionStatus::Connected)
                .map(move |a| (*provider, a.slot.clone()))
        })
        .collect();
    let read = futures::future::join_all(logins.iter().map(|(provider, slot)| async move {
        let harness = usage_harness(*provider).unwrap_or(HarnessId::Codex);
        (
            (*provider, slot.clone()),
            usage(cloud, harness, *provider, slot, force_usage).await,
        )
    }))
    .await;
    let usages: HashMap<(VaultProvider, String), CachedUsage> = read.into_iter().collect();
    let accounts = cloud_harnesses()
        .flat_map(|harness| card(harness, &by_provider, &usages))
        .collect();
    Ok(AgentAccountsSnapshot {
        accounts,
        warnings: Vec::new(),
    })
}

/// Whose usage view a provider's logins have (only subscription logins do).
fn usage_harness(provider: VaultProvider) -> Option<HarnessId> {
    match provider {
        VaultProvider::Claude => Some(HarnessId::ClaudeCode),
        VaultProvider::Codex => Some(HarnessId::Codex),
        _ => None,
    }
}

/// `harness`'s card: the accounts of every credential it runs on, in its
/// order. In each group only the first credential with an active, working
/// account is in use — the one a machine would run on.
fn card(
    harness: HarnessId,
    by_provider: &HashMap<VaultProvider, Vec<VaultAccount>>,
    usages: &HashMap<(VaultProvider, String), CachedUsage>,
) -> Vec<AgentAccount> {
    let grouped = groups_by_provider(harness);
    let mut taken: HashSet<&str> = HashSet::new();
    let mut out = Vec::new();
    for credential in cloud_credentials(harness) {
        let Some(accounts) = by_provider.get(&credential.vault) else {
            continue;
        };
        let in_use = !taken.contains(credential.group);
        for account in accounts {
            out.push(to_agent_account(
                harness,
                credential.vault,
                account,
                // In use = what a machine runs on now: a login it can no
                // longer use isn't, and the key behind it is.
                account.active && account.status == VaultConnectionStatus::Connected && in_use,
                usages
                    .get(&(credential.vault, account.slot.clone()))
                    .cloned(),
                grouped.then(|| credential.group.to_string()),
            ));
        }
        if accounts
            .iter()
            .any(|a| a.active && a.status == VaultConnectionStatus::Connected)
        {
            taken.insert(credential.group);
        }
    }
    out
}

fn to_agent_account(
    harness: HarnessId,
    provider: VaultProvider,
    account: &VaultAccount,
    active: bool,
    usage: Option<CachedUsage>,
    group: Option<String>,
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
    let vendor = provider.key_vendor();
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
                Some(match vendor {
                    // A card holding several vendors' keys names whose.
                    Some(vendor)
                        if cloud_credentials(harness)
                            .iter()
                            .filter(|c| c.vault.is_api_key())
                            .count()
                            > 1 =>
                    {
                        format!("{vendor} API key {}", label.unwrap_or_default())
                    }
                    _ => format!("API key {}", label.unwrap_or_default()),
                })
            } else {
                label.filter(|_| !label_is_email).map(plan_name)
            }
        }),
        active,
        usage_windows: windows,
        codex_reset_credits: None,
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
        provider: group,
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
    cloud_harnesses()
        .filter(|harness| cloud_credentials(*harness).iter().any(|c| ready(c.vault)))
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
            plan: (kind != "api-key").then(|| "Max".into()),
            avatar_url: None,
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
                    VaultProvider::Codex,
                    VaultConnectionStatus::NeedsReconnect,
                    true
                ),
                connection(
                    VaultProvider::MetaKey,
                    VaultConnectionStatus::Connected,
                    true
                ),
            ])),
            vec![HarnessId::Muse]
        );
        // One vendor key readies every agent that takes it.
        assert_eq!(
            ready_from(&vault(vec![connection(
                VaultProvider::AnthropicKey,
                VaultConnectionStatus::Connected,
                true
            )])),
            vec![
                HarnessId::ClaudeCode,
                HarnessId::Hermes,
                HarnessId::Fx,
                HarnessId::Pi,
                HarnessId::Opencode
            ]
        );
        assert_eq!(
            ready_from(&vault(vec![connection(
                VaultProvider::Codex,
                VaultConnectionStatus::Connected,
                true
            )])),
            vec![HarnessId::Codex, HarnessId::Pi]
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
        assert_eq!(
            account_ref(HarnessId::Pi, "codex:a1").unwrap(),
            (VaultProvider::Codex, "a1")
        );
        assert!(account_ref(HarnessId::Pi, "claude:a1").is_err());
        assert!(account_ref(HarnessId::Mock, "codex:a1").is_err());
    }

    #[test]
    fn a_key_clears_only_the_sign_in_it_stands_behind() {
        assert_eq!(
            login_stepping_aside(HarnessId::Codex, VaultProvider::OpenaiKey),
            Some(VaultProvider::Codex)
        );
        assert_eq!(
            login_stepping_aside(HarnessId::Grok, VaultProvider::XaiKey),
            Some(VaultProvider::Grok)
        );
        // Pi uses its ChatGPT sign-in and an OpenAI key side by side.
        assert_eq!(
            login_stepping_aside(HarnessId::Pi, VaultProvider::OpenaiKey),
            None
        );
        assert_eq!(
            login_stepping_aside(HarnessId::Fx, VaultProvider::AnthropicKey),
            None
        );
    }

    #[test]
    fn sign_ins_cloud_cannot_take_say_what_to_do_instead() {
        let message = |harness, provider| match no_cloud_sign_in(harness, provider) {
            RpcError::Failed(message) => message,
            other => panic!("{other:?}"),
        };
        assert!(message(HarnessId::Pi, Some("anthropic")).contains("Anthropic API key"));
        assert!(message(HarnessId::Muse, None).contains("uses API keys"));
        assert!(message(HarnessId::Mock, None).contains("doesn't run on Cloud"));
    }

    #[test]
    fn cards_show_what_machines_run_on() {
        let mut by: HashMap<VaultProvider, Vec<VaultAccount>> = HashMap::new();
        by.insert(VaultProvider::Codex, vec![vault("c1", "oauth", true)]);
        by.insert(VaultProvider::OpenaiKey, vec![vault("k1", "api-key", true)]);
        by.insert(
            VaultProvider::OpenrouterKey,
            vec![vault("r1", "api-key", true)],
        );
        let usages = HashMap::new();
        // Codex: one group, the sign-in before the key.
        let codex = card(HarnessId::Codex, &by, &usages);
        let active: Vec<_> = codex
            .iter()
            .filter(|a| a.active)
            .map(|a| a.id.as_str())
            .collect();
        assert_eq!(active, ["codex:c1"]);
        assert!(codex.iter().all(|a| a.provider.is_none()));
        // Pi: its groups apart, each with its own in use.
        let pi = card(HarnessId::Pi, &by, &usages);
        let groups: Vec<_> = pi
            .iter()
            .map(|a| (a.id.as_str(), a.provider.as_deref(), a.active))
            .collect();
        assert_eq!(
            groups,
            [
                ("codex:c1", Some("openai-codex"), true),
                ("openai-key:k1", Some("openai"), true),
                ("openrouter-key:r1", Some("openrouter"), true),
            ]
        );
        // A key on a card of several vendors names its vendor.
        assert_eq!(
            pi[2].plan_label.as_deref(),
            Some("OpenRouter API key …abcd")
        );
        assert_eq!(codex[1].plan_label.as_deref(), Some("API key …abcd"));
        // A broken sign-in doesn't hold the group: the key is in use.
        by.get_mut(&VaultProvider::Codex).unwrap()[0].status =
            VaultConnectionStatus::NeedsReconnect;
        let codex = card(HarnessId::Codex, &by, &usages);
        let active: Vec<_> = codex
            .iter()
            .filter(|a| a.active)
            .map(|a| a.id.as_str())
            .collect();
        assert_eq!(active, ["openai-key:k1"]);
    }

    #[test]
    fn keys_read_as_api_key_accounts_and_logins_carry_their_identity() {
        let login = to_agent_account(
            HarnessId::ClaudeCode,
            VaultProvider::Claude,
            &vault("a1", "oauth", true),
            true,
            None,
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
            None,
        );
        assert_eq!(bare.email, None);
        assert_eq!(bare.plan_label.as_deref(), Some("Max"));
        let mut gone = vault("a2", "oauth", false);
        gone.status = VaultConnectionStatus::NeedsReconnect;
        let gone = to_agent_account(
            HarnessId::Codex,
            VaultProvider::Codex,
            &gone,
            false,
            None,
            None,
        );
        assert!(!gone.switchable);
        assert!(gone.usage_error.unwrap().contains("Sign in again"));
    }
}
