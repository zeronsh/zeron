//! A computer's agents over FFI: which ones it offers and the CLI logins
//! each runs on (`zeron_client::providers`) — the phone's Settings →
//! Providers for a device picked in its switcher.

use super::{CoreClient, CoreResult, HarnessInfo, on_runtime};

/// One plan rate-limit window ("Session", "Week").
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct UsageWindowInfo {
    pub label: String,
    /// 0.0…1.0 of the window used.
    pub used_fraction: f32,
    pub resets_at_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct DeviceAccountInfo {
    pub id: String,
    /// `claude-code` · `codex` · …
    pub harness: String,
    pub email: Option<String>,
    pub display_name: Option<String>,
    pub plan_label: Option<String>,
    pub active: bool,
    /// Can be switched to / removed (false for logins Zeron can't read).
    pub switchable: bool,
    /// An API key rather than an account.
    pub api_key: bool,
    pub organization: Option<String>,
    /// The upstream login of an agent that keeps one per model provider
    /// (OpenCode's `openai`, …).
    pub provider: Option<String>,
    pub usage: Vec<UsageWindowInfo>,
    /// Why usage couldn't be read ("Sign in again", "Rate limited …").
    pub usage_error: Option<String>,
}

impl From<zeron_proto::AgentAccount> for DeviceAccountInfo {
    fn from(a: zeron_proto::AgentAccount) -> Self {
        Self {
            id: a.id,
            harness: serde_json::to_value(a.harness)
                .ok()
                .and_then(|v| v.as_str().map(str::to_owned))
                .unwrap_or_default(),
            email: a.email,
            display_name: a.display_name,
            plan_label: a.plan_label,
            active: a.active,
            switchable: a.switchable,
            api_key: a.auth_kind == Some(zeron_proto::AgentAuthKind::ApiKey),
            organization: a.organization,
            provider: a.provider,
            usage: a
                .usage_windows
                .into_iter()
                .map(|w| UsageWindowInfo {
                    label: w.label,
                    used_fraction: w.used_fraction,
                    resets_at_ms: w.resets_at.map(|at| at.timestamp_millis()),
                })
                .collect(),
            usage_error: a.usage_error,
        }
    }
}

/// A computer's sign-in under way: open `url` in an auth sheet waiting for
/// the `done_url` scheme; poll until done — or, `paste_code`, take the code
/// the page shows back with `device_login_complete`.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct DeviceLoginInfo {
    pub login_id: String,
    /// Empty when the page is only known later (a poll carries it).
    pub url: String,
    pub paste_code: bool,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct DeviceLoginPollInfo {
    /// `pending` · `done` · `error`.
    pub status: String,
    pub message: Option<String>,
    /// A page first known now, to open.
    pub url: Option<String>,
}

/// How an agent signs in, from the rules every platform's Providers follows.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct AgentLoginRules {
    /// The agent's display name ("Claude Code"); empty for one Zeron doesn't
    /// sign in to.
    pub name: String,
    /// The agent has accounts Zeron can sign in to at all.
    pub signs_in: bool,
    /// Zeron switches between its accounts (not Hermes, not Antigravity).
    pub switches: bool,
    /// It holds one login: once connected there is nothing to add.
    pub keeps_one_login: bool,
    pub reports_usage: bool,
    /// A standing note for an agent whose accounts work differently.
    pub note: Option<String>,
    pub options: Vec<LoginOptionInfo>,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct LoginOptionInfo {
    /// The engine's `provider` (`None`: the agent's only login).
    pub provider: Option<String>,
    pub label: String,
    /// The add row's title with no account yet, and with some.
    pub first_title: String,
    pub more_title: String,
}

/// [`AgentLoginRules`] for `harness` (`claude-code`, `codex`, …).
#[uniffi::export]
pub fn agent_login_rules(harness: String) -> AgentLoginRules {
    use zeron_proto::providers as rules;
    let Ok(id) = serde_json::from_value::<zeron_proto::HarnessId>(serde_json::json!(harness))
    else {
        return AgentLoginRules {
            name: String::new(),
            signs_in: false,
            switches: false,
            keeps_one_login: false,
            reports_usage: false,
            note: None,
            options: Vec::new(),
        };
    };
    let signs_in = rules::signs_in(id);
    AgentLoginRules {
        name: if signs_in {
            rules::provider_name(id).to_owned()
        } else {
            String::new()
        },
        signs_in,
        switches: rules::switches_accounts(id),
        keeps_one_login: rules::keeps_one_login(id),
        reports_usage: rules::reports_usage(id),
        note: rules::provider_note(id).map(str::to_owned),
        options: if signs_in {
            rules::login_options(id)
                .into_iter()
                .map(|option| LoginOptionInfo {
                    provider: option.provider.map(str::to_owned),
                    label: option.label.to_owned(),
                    first_title: rules::add_option_label(id, option, true),
                    more_title: rules::add_option_label(id, option, false),
                })
                .collect()
        } else {
            Vec::new()
        },
    }
}

fn accounts(snapshot: zeron_proto::AgentAccountsSnapshot) -> Vec<DeviceAccountInfo> {
    snapshot.accounts.into_iter().map(Into::into).collect()
}

#[uniffi::export]
impl CoreClient {
    /// Offer `harness` on `device_id` or not; the device's fresh catalog.
    pub async fn set_harness_enabled(
        &self,
        device_id: String,
        harness: String,
        enabled: bool,
    ) -> CoreResult<Vec<HarnessInfo>> {
        let client = self.client.clone();
        on_runtime(async move {
            client
                .set_harness_enabled(&device_id, &harness, enabled)
                .await
        })
        .await
        .map(|list| list.into_iter().map(Into::into).collect())
    }

    /// The CLI logins on `device_id`, every agent's.
    pub async fn device_agent_accounts(
        &self,
        device_id: String,
    ) -> CoreResult<Vec<DeviceAccountInfo>> {
        let client = self.client.clone();
        on_runtime(async move { client.device_agent_accounts(&device_id).await })
            .await
            .map(accounts)
    }

    /// Run `harness` on `account_id` from now on.
    pub async fn device_activate_account(
        &self,
        device_id: String,
        harness: String,
        account_id: String,
    ) -> CoreResult<Vec<DeviceAccountInfo>> {
        let client = self.client.clone();
        on_runtime(async move {
            client
                .device_activate_account(&device_id, &harness, &account_id)
                .await
        })
        .await
        .map(accounts)
    }

    /// Start signing `harness` in on `device_id` (`provider`: a login
    /// option's, `None` for the default).
    pub async fn device_login_start(
        &self,
        device_id: String,
        harness: String,
        provider: Option<String>,
        done_url: String,
    ) -> CoreResult<DeviceLoginInfo> {
        let client = self.client.clone();
        on_runtime(async move {
            client
                .device_login_start(&device_id, &harness, provider.as_deref(), &done_url)
                .await
        })
        .await
        .map(|start| DeviceLoginInfo {
            login_id: start.login_id,
            url: start.url,
            paste_code: start.mode == zeron_proto::AgentLoginMode::PasteCode,
        })
    }

    pub async fn device_login_poll(
        &self,
        device_id: String,
        login_id: String,
        done_url: String,
    ) -> CoreResult<DeviceLoginPollInfo> {
        let client = self.client.clone();
        on_runtime(async move {
            client
                .device_login_poll(&device_id, &login_id, &done_url)
                .await
        })
        .await
        .map(|poll| DeviceLoginPollInfo {
            status: match poll.status {
                zeron_proto::AgentLoginStatus::Pending => "pending",
                zeron_proto::AgentLoginStatus::Done => "done",
                zeron_proto::AgentLoginStatus::Error => "error",
            }
            .into(),
            message: poll.message,
            url: poll.url,
        })
    }

    /// The code the sign-in page showed (Claude's paste fallback).
    pub async fn device_login_complete(
        &self,
        device_id: String,
        login_id: String,
        code: String,
    ) -> CoreResult<Vec<DeviceAccountInfo>> {
        let client = self.client.clone();
        on_runtime(async move {
            client
                .device_login_complete(&device_id, &login_id, &code)
                .await
        })
        .await
        .map(accounts)
    }

    pub fn device_login_cancel(&self, device_id: String, login_id: String) {
        self.client.device_login_cancel(&device_id, &login_id);
    }

    /// Remove a saved login from `device_id`.
    pub async fn device_forget_account(
        &self,
        device_id: String,
        harness: String,
        account_id: String,
    ) -> CoreResult<Vec<DeviceAccountInfo>> {
        let client = self.client.clone();
        on_runtime(async move {
            client
                .device_forget_account(&device_id, &harness, &account_id)
                .await
        })
        .await
        .map(accounts)
    }
}
