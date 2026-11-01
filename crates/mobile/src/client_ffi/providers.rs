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
    /// The computer reports rotation for this agent at all (an older
    /// engine, or an agent it doesn't list, doesn't: no Rotation section).
    pub rotation_reported: bool,
    /// Its agent's account rotation on this device — the same on every row
    /// of the agent: the user's setting, and whether this agent can rotate.
    pub rotation_enabled: bool,
    pub rotation_supported: bool,
    /// Rotation is in effect (enabled, supported, two usable accounts).
    pub rotation_active: bool,
    /// Why rotation is unsupported or not in effect, for the switch's caption.
    pub rotation_note: Option<String>,
    /// This account takes turns.
    pub in_rotation: bool,
    /// The next new chat starts on this account.
    pub rotation_next: bool,
    /// Chats rotation put on this account with a turn running right now.
    pub live_chats: u32,
    /// Why rotation passes it over right now, the computer's words
    /// ("Usage limit", "Signed out").
    pub rotation_skipped: Option<String>,
    /// Epoch millis a skipped account is expected back (its limit resets).
    pub rotation_skipped_until_ms: Option<i64>,
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
            rotation_reported: false,
            rotation_enabled: false,
            rotation_supported: false,
            rotation_active: false,
            rotation_note: None,
            in_rotation: false,
            rotation_next: false,
            live_chats: 0,
            rotation_skipped: None,
            rotation_skipped_until_ms: None,
        }
    }
}

impl DeviceAccountInfo {
    /// `account` with its part in `rotation` (its agent's, if reported).
    fn with_rotation(
        account: zeron_proto::AgentAccount,
        rotation: Option<&zeron_proto::AgentAccountRotation>,
    ) -> Self {
        let mut info = Self::from(account);
        if let Some(rotation) = rotation {
            info.rotation_reported = true;
            info.rotation_enabled = rotation.enabled;
            info.rotation_supported = rotation.supported;
            info.rotation_active = rotation.active;
            info.rotation_note = rotation.note.clone();
            info.in_rotation = rotation.accounts.iter().any(|id| *id == info.id);
            info.rotation_next =
                rotation.active && rotation.next_account_id.as_deref() == Some(info.id.as_str());
            info.live_chats = rotation.running_count(&info.id) as u32;
            if let Some(skip) = rotation.skipped.iter().find(|s| s.account_id == info.id) {
                info.rotation_skipped = Some(skip.reason.clone());
                info.rotation_skipped_until_ms = skip.until;
            }
        }
        info
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
    let rotation = snapshot.rotation;
    snapshot
        .accounts
        .into_iter()
        .map(|account| {
            let harness = account.harness;
            DeviceAccountInfo::with_rotation(
                account,
                rotation.iter().find(|r| r.harness == harness),
            )
        })
        .collect()
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

    /// Spread `harness`'s new chats on `device_id` across its saved accounts
    /// (`enabled`), or run them all on the active one again.
    pub async fn device_set_account_rotation(
        &self,
        device_id: String,
        harness: String,
        enabled: bool,
    ) -> CoreResult<Vec<DeviceAccountInfo>> {
        let client = self.client.clone();
        on_runtime(async move {
            client
                .device_set_account_rotation(&device_id, &harness, enabled)
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

#[cfg(test)]
mod tests {
    use super::*;
    use zeron_proto::{AgentAccount, AgentAccountChat, AgentAccountRotation, HarnessId};

    fn account(id: &str, used: f32) -> AgentAccount {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "harness": "claude-code",
            "active": id == "a",
            "switchable": true,
            "usageWindows": [{ "label": "Session", "usedFraction": used }],
        }))
        .unwrap()
    }

    #[test]
    fn rows_carry_their_agents_rotation() {
        let chat = |chat: &str, account: &str, running: bool| AgentAccountChat {
            chat_id: chat.into(),
            account_id: account.into(),
            assigned_at: 0,
            used_at: 0,
            previous_account_id: None,
            moved_reason: None,
            running,
        };
        let snapshot = zeron_proto::AgentAccountsSnapshot {
            accounts: vec![account("a", 0.2), account("b", 0.97), account("c", 0.5)],
            warnings: Vec::new(),
            rotation: vec![AgentAccountRotation {
                harness: HarnessId::ClaudeCode,
                supported: true,
                enabled: true,
                active: true,
                accounts: vec!["a".into(), "b".into(), "c".into()],
                next_account_id: Some("b".into()),
                chats: vec![
                    chat("1", "a", true),
                    chat("2", "a", true),
                    chat("3", "c", true),
                    // Idle chats don't count.
                    chat("4", "b", false),
                ],
                note: None,
                // The computer's own skip list — "c" is at a limit until a
                // known time, "a" signed out — not a guess from the meters
                // ("c" isn't full by them, "b" nearly is).
                skipped: vec![
                    zeron_proto::AgentAccountSkip {
                        account_id: "c".into(),
                        reason: "Usage limit".into(),
                        until: Some(1_700_000_000_000),
                    },
                    zeron_proto::AgentAccountSkip {
                        account_id: "a".into(),
                        reason: "Signed out".into(),
                        until: None,
                    },
                ],
            }],
        };
        let rows = accounts(snapshot);
        assert!(
            rows.iter()
                .all(|r| r.rotation_reported && r.rotation_enabled && r.rotation_supported)
        );
        assert_eq!(
            rows.iter().map(|r| r.live_chats).collect::<Vec<_>>(),
            [2, 0, 1]
        );
        assert_eq!(
            rows.iter().map(|r| r.rotation_next).collect::<Vec<_>>(),
            [false, true, false]
        );
        assert_eq!(rows[2].rotation_skipped.as_deref(), Some("Usage limit"));
        assert_eq!(rows[2].rotation_skipped_until_ms, Some(1_700_000_000_000));
        assert_eq!(rows[0].rotation_skipped.as_deref(), Some("Signed out"));
        assert_eq!(rows[0].rotation_skipped_until_ms, None);
        assert!(rows[1].rotation_skipped.is_none());

        // An engine without rotation (or an agent it doesn't list): not
        // reported, so the phone shows no Rotation section.
        let plain = accounts(zeron_proto::AgentAccountsSnapshot {
            accounts: vec![account("a", 0.2)],
            warnings: Vec::new(),
            rotation: Vec::new(),
        });
        assert!(!plain[0].rotation_reported);
        assert!(!plain[0].rotation_enabled && !plain[0].rotation_supported);
    }
}
