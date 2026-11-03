//! Cloud over FFI: the account's status and credits, GitHub, the vault's
//! agent logins (signed in from the phone itself), billing, and Cloud
//! projects and sessions (`zeron_client::cloud`, `zeron_client::provider_login`).

use zeron_client as zc;
use zeron_proto::VaultProvider;

use super::{ChatConfig, CoreClient, CoreError, CoreResult, on_runtime};

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct CloudInfo {
    /// This account can use Cloud (signed in, an edge with Cloud).
    pub available: bool,
    /// `off` · `ready` · `deleting` · `error` …
    pub state: String,
    pub device_id: Option<String>,
    /// Credits left (plan + extra − unmetered use).
    pub credits: Option<f64>,
    pub awake_sessions: u32,
    pub max_awake_sessions: u32,
    pub error: Option<String>,
}

impl From<zeron_proto::CloudStatus> for CloudInfo {
    fn from(s: zeron_proto::CloudStatus) -> Self {
        Self {
            available: s.available,
            state: cloud_state(s.state),
            device_id: s.device_id,
            credits: s.credits,
            awake_sessions: s.awake_sessions,
            max_awake_sessions: s.max_awake_sessions,
            error: s.error,
        }
    }
}

fn cloud_state(state: zeron_proto::CloudState) -> String {
    serde_json::to_value(state)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct CloudSessionInfo {
    pub chat_id: String,
    pub device_id: String,
    pub state: String,
    pub repo: Option<String>,
    pub error: Option<String>,
}

impl From<zeron_proto::CloudSession> for CloudSessionInfo {
    fn from(s: zeron_proto::CloudSession) -> Self {
        Self {
            chat_id: s.chat_id,
            device_id: s.device_id,
            state: cloud_state(s.state),
            repo: s.repo,
            error: s.error,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct GithubRepoInfo {
    /// `owner/name`.
    pub full_name: String,
    pub default_branch: String,
    pub private: bool,
    pub description: Option<String>,
    pub pushed_at_ms: i64,
}

impl From<zeron_proto::GithubRepo> for GithubRepoInfo {
    fn from(r: zeron_proto::GithubRepo) -> Self {
        Self {
            full_name: r.full_name,
            default_branch: r.default_branch,
            private: r.private,
            description: r.description,
            pushed_at_ms: r.pushed_at,
        }
    }
}

/// GitHub's device flow: show `user_code`, open `verification_uri`, poll.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct GithubDeviceFlowInfo {
    pub flow_id: String,
    pub user_code: String,
    pub verification_uri: String,
    pub interval_secs: u64,
    pub expires_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct GithubConnectInfo {
    /// `pending` · `connected` · `failed`.
    pub state: String,
    pub account: Option<String>,
    pub error: Option<String>,
}

/// One provider the vault holds logins for.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct VaultConnectionInfo {
    /// `claude` · `codex` · `github` · `anthropic-key` · `openai-key`.
    pub provider: String,
    pub account: Option<String>,
    pub needs_reconnect: bool,
    pub accounts: u32,
    pub has_active: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct VaultInfo {
    pub available: bool,
    pub connections: Vec<VaultConnectionInfo>,
    /// Where to install the Zeron GitHub App on more repositories.
    pub github_install_url: Option<String>,
}

impl From<zeron_proto::VaultStatus> for VaultInfo {
    fn from(s: zeron_proto::VaultStatus) -> Self {
        Self {
            available: s.available,
            connections: s
                .connections
                .into_iter()
                .map(|c| VaultConnectionInfo {
                    provider: c.provider.as_str().to_owned(),
                    account: c.account,
                    needs_reconnect: c.status == zeron_proto::VaultConnectionStatus::NeedsReconnect,
                    accounts: c.accounts,
                    has_active: c.has_active,
                })
                .collect(),
            github_install_url: s.github_install_url,
        }
    }
}

/// One login Cloud keeps for a provider.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct CloudAccountInfo {
    pub slot: String,
    pub active: bool,
    /// `oauth` (a subscription sign-in) or `key`.
    pub kind: String,
    pub email: Option<String>,
    pub display_name: Option<String>,
    pub organization: Option<String>,
    pub plan: Option<String>,
    pub needs_reconnect: bool,
}

impl From<zeron_proto::VaultAccount> for CloudAccountInfo {
    fn from(a: zeron_proto::VaultAccount) -> Self {
        Self {
            slot: a.slot,
            active: a.active,
            kind: a.kind,
            email: a.email.or(a.account),
            display_name: a.display_name,
            organization: a.organization,
            plan: a.plan,
            needs_reconnect: a.status == zeron_proto::VaultConnectionStatus::NeedsReconnect,
        }
    }
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct BillingPlanInfo {
    pub id: String,
    pub name: String,
    pub price_cents: u32,
    pub monthly_credits: f64,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct BillingInfo {
    /// Stripe is set up on this server.
    pub available: bool,
    pub plan: Option<BillingPlanInfo>,
    /// `active` · `trialing` · `past_due` · …
    pub plan_status: Option<String>,
    pub period_end_ms: Option<i64>,
    pub cancel_at_period_end: bool,
    pub credits_total: f64,
    pub credits_monthly: f64,
    pub credits_monthly_total: f64,
    /// When this period's plan credits reset (epoch ms).
    pub credits_monthly_ends_ms: Option<i64>,
    pub credits_extra: f64,
    pub plans: Vec<BillingPlanInfo>,
    pub pack_price_cents: u32,
    pub pack_credits: f64,
    pub has_customer: bool,
}

fn plan_info(p: zeron_proto::BillingPlan) -> BillingPlanInfo {
    BillingPlanInfo {
        id: p.id,
        name: p.name,
        price_cents: p.price_cents,
        monthly_credits: p.monthly_credits,
    }
}

impl From<zeron_proto::BillingStatus> for BillingInfo {
    fn from(b: zeron_proto::BillingStatus) -> Self {
        let (plan, plan_status, period_end_ms, cancel_at_period_end) = match b.plan {
            Some(sub) => (
                Some(plan_info(sub.plan)),
                Some(sub.status),
                Some(sub.period_end),
                sub.cancel_at_period_end,
            ),
            None => (None, None, None, false),
        };
        Self {
            available: b.available,
            plan,
            plan_status,
            period_end_ms,
            cancel_at_period_end,
            credits_total: b.credits.total,
            credits_monthly: b.credits.monthly,
            credits_monthly_total: b.credits.monthly_total,
            credits_monthly_ends_ms: b.credits.monthly_ends_at,
            credits_extra: b.credits.extra,
            plans: b.plans.into_iter().map(plan_info).collect(),
            pack_price_cents: b.pack.price_cents,
            pack_credits: b.pack.credits,
            has_customer: b.has_customer,
        }
    }
}

/// A Cloud login's plan usage.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct CloudUsageInfo {
    pub windows: Vec<super::UsageWindowInfo>,
    /// The live plan ("ChatGPT Plus"), when the reply names it.
    pub plan: Option<String>,
}

/// A provider sign-in under way: open `url` in an auth sheet that waits for
/// the `done_url` scheme it was started with, then await
/// `provider_login_finish(login_id)`.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ProviderLoginInfo {
    pub login_id: String,
    pub url: String,
}

fn vault_provider(provider: &str) -> CoreResult<VaultProvider> {
    VaultProvider::parse(provider).ok_or_else(|| CoreError::InvalidArgument {
        message: format!("unknown provider {provider}"),
    })
}

#[uniffi::export]
impl CoreClient {
    /// Cloud's status (an edge without Cloud reads as unavailable).
    pub async fn cloud_status(&self) -> CoreResult<CloudInfo> {
        let client = self.client.clone();
        on_runtime(async move { client.cloud_status().await })
            .await
            .map(Into::into)
    }

    /// Set Cloud up for this account (it's part of every signed-in one).
    pub async fn cloud_enable(&self) -> CoreResult<CloudInfo> {
        let client = self.client.clone();
        on_runtime(async move { client.cloud_enable().await })
            .await
            .map(Into::into)
    }

    pub async fn cloud_sessions(&self) -> CoreResult<Vec<CloudSessionInfo>> {
        let client = self.client.clone();
        on_runtime(async move { client.cloud_sessions().await })
            .await
            .map(|s| s.sessions.into_iter().map(Into::into).collect())
    }

    /// Wake (or put to sleep) a session's machine; `chat_id` is the
    /// session's own chat.
    pub async fn cloud_session_action(
        &self,
        chat_id: String,
        wake: bool,
    ) -> CoreResult<CloudSessionInfo> {
        let client = self.client.clone();
        on_runtime(async move { client.cloud_session_action(&chat_id, wake).await })
            .await
            .map(Into::into)
    }

    /// The repositories Cloud reaches (empty when GitHub isn't connected).
    pub async fn github_repos(&self, query: Option<String>) -> CoreResult<Vec<GithubRepoInfo>> {
        let client = self.client.clone();
        on_runtime(async move { client.github_repos(query.as_deref()).await })
            .await
            .map(|repos| repos.into_iter().map(Into::into).collect())
    }

    pub async fn github_branches(&self, repo: String) -> CoreResult<Vec<String>> {
        let client = self.client.clone();
        on_runtime(async move { client.github_branches(&repo).await }).await
    }

    pub async fn github_connect_start(&self) -> CoreResult<GithubDeviceFlowInfo> {
        let client = self.client.clone();
        on_runtime(async move { client.github_connect_start().await })
            .await
            .map(|f| GithubDeviceFlowInfo {
                flow_id: f.flow_id,
                user_code: f.user_code,
                verification_uri: f.verification_uri,
                interval_secs: f.interval_secs,
                expires_at_ms: f.expires_at,
            })
    }

    pub async fn github_connect_poll(&self, flow_id: String) -> CoreResult<GithubConnectInfo> {
        let client = self.client.clone();
        on_runtime(async move { client.github_connect_poll(&flow_id).await })
            .await
            .map(|p| GithubConnectInfo {
                state: match p.state {
                    zeron_proto::GithubConnectState::Pending => "pending",
                    zeron_proto::GithubConnectState::Connected => "connected",
                    zeron_proto::GithubConnectState::Failed => "failed",
                }
                .to_owned(),
                account: p.account,
                error: p.error,
            })
    }

    pub async fn vault_status(&self) -> CoreResult<VaultInfo> {
        let client = self.client.clone();
        on_runtime(async move { client.vault_status().await })
            .await
            .map(Into::into)
    }

    /// A Cloud login's plan usage (windows, live plan), read by the vault.
    pub async fn vault_usage(&self, provider: String, slot: String) -> CoreResult<CloudUsageInfo> {
        let provider = vault_provider(&provider)?;
        let client = self.client.clone();
        on_runtime(async move { client.vault_usage(provider, &slot).await })
            .await
            .map(|usage| match usage {
                Some((windows, plan)) => CloudUsageInfo {
                    windows: windows
                        .into_iter()
                        .map(|w| super::UsageWindowInfo {
                            label: w.label,
                            used_fraction: w.used_fraction,
                            resets_at_ms: w.resets_at.map(|at| at.timestamp_millis()),
                        })
                        .collect(),
                    plan,
                },
                None => CloudUsageInfo {
                    windows: Vec::new(),
                    plan: None,
                },
            })
    }

    /// Every login Cloud keeps for `provider` (`claude` · `codex`).
    pub async fn vault_accounts(&self, provider: String) -> CoreResult<Vec<CloudAccountInfo>> {
        let provider = vault_provider(&provider)?;
        let client = self.client.clone();
        on_runtime(async move { client.vault_accounts(provider).await })
            .await
            .map(|list| list.into_iter().map(Into::into).collect())
    }

    /// Make `slot` the login Cloud uses for `provider`.
    pub async fn vault_activate(
        &self,
        provider: String,
        slot: String,
    ) -> CoreResult<Vec<CloudAccountInfo>> {
        let provider = vault_provider(&provider)?;
        let client = self.client.clone();
        on_runtime(async move { client.vault_activate(provider, Some(&slot)).await })
            .await
            .map(|list| list.into_iter().map(Into::into).collect())
    }

    pub async fn vault_forget(
        &self,
        provider: String,
        slot: String,
    ) -> CoreResult<Vec<CloudAccountInfo>> {
        let provider = vault_provider(&provider)?;
        let client = self.client.clone();
        on_runtime(async move { client.vault_forget(provider, &slot).await })
            .await
            .map(|list| list.into_iter().map(Into::into).collect())
    }

    /// Store an API key (`anthropic-key` · `openai-key`) as the one Cloud uses.
    pub async fn vault_put_api_key(&self, provider: String, key: String) -> CoreResult<()> {
        let provider = vault_provider(&provider)?;
        let client = self.client.clone();
        on_runtime(async move { client.vault_put_api_key(provider, &key).await }).await
    }

    pub async fn billing_status(&self) -> CoreResult<BillingInfo> {
        let client = self.client.clone();
        on_runtime(async move { client.billing_status().await })
            .await
            .map(Into::into)
    }

    /// A Stripe Checkout url: a plan (`kind = "subscription"`, `plan`) or
    /// extra credits (`kind = "credits"`, `packs`).
    pub async fn billing_checkout(
        &self,
        kind: String,
        plan: Option<String>,
        packs: Option<u32>,
    ) -> CoreResult<String> {
        let client = self.client.clone();
        on_runtime(async move { client.billing_checkout(&kind, plan.as_deref(), packs).await })
            .await
    }

    /// The Stripe customer portal url.
    pub async fn billing_portal(&self) -> CoreResult<String> {
        let client = self.client.clone();
        on_runtime(async move { client.billing_portal().await }).await
    }

    /// Make `repo` (`owner/name`) a project on Cloud alone; returns its id.
    pub async fn create_cloud_project(&self, repo: String) -> CoreResult<String> {
        let client = self.client.clone();
        on_runtime(async move { client.create_cloud_project(&repo).await }).await
    }

    /// Start a session on its own Cloud machine in `space_id`; returns the
    /// chat id (send its first message as usual).
    pub async fn create_cloud_session(
        &self,
        space_id: String,
        config: Option<ChatConfig>,
        branch: Option<String>,
        title: Option<String>,
    ) -> CoreResult<String> {
        let client = self.client.clone();
        let config: Option<zc::ChatConfig> = config.map(TryInto::try_into).transpose()?;
        on_runtime(async move {
            client
                .create_cloud_session(&space_id, config, branch, title)
                .await
        })
        .await
    }

    /// Sign in to `provider` (`claude` · `codex`) for Cloud. The browser is
    /// sent to `done_url` once the provider answers.
    pub async fn provider_login_start(
        &self,
        provider: String,
        done_url: String,
    ) -> CoreResult<ProviderLoginInfo> {
        let provider = match provider.as_str() {
            "claude" => zc::LoginProvider::Claude,
            "codex" => zc::LoginProvider::Codex,
            other => {
                return Err(CoreError::InvalidArgument {
                    message: format!("{other} can't be signed in from here"),
                });
            }
        };
        let client = self.client.clone();
        on_runtime(async move { client.provider_login_start(provider, &done_url).await })
            .await
            .map(|start| ProviderLoginInfo {
                login_id: start.login_id,
                url: start.url,
            })
    }

    /// Wait for a started sign-in to land in Cloud's vault.
    pub async fn provider_login_finish(&self, login_id: String) -> CoreResult<()> {
        let client = self.client.clone();
        on_runtime(async move { client.provider_login_finish(&login_id).await }).await
    }

    /// The auth sheet was dismissed.
    pub fn provider_login_cancel(&self, login_id: String) {
        self.client.provider_login_cancel(&login_id);
    }
}
