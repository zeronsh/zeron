//! A computer's agents from the phone: which harnesses it offers and the CLI
//! logins each one runs on — what desktop Settings → Providers shows for a
//! device picked in its switcher. Every call goes over the device relay to
//! that computer's engine, which owns its own CLI logins.
//!
//! Signing a computer's agent in: the computer runs the sign-in (its CLI or
//! engine listens on its own loopback); the phone opens the page, and the
//! browser's redirect to `localhost:<port>` lands on a listener the phone
//! binds on that same port. The phone relays that one request to the
//! computer (`RelayAgentLoginCallback`), which replays it to the waiting
//! listener — what the desktop does over its P2P tunnel. Device-code and
//! polling sign-ins need no relay; Claude's paste-the-code fallback takes the
//! code back to the computer.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use tokio::io::AsyncWriteExt as _;
use zeron_proto::{
    AgentAccount, AgentAccountsSnapshot, AgentAuthKind, AgentLoginMode, AgentLoginPoll,
    AgentLoginStart, AgentLoginStatus, HarnessId,
};

use crate::Client;
use crate::catalog::HarnessInfo;
use crate::client::Backend;
use crate::error::{ClientError, Result};
use crate::lock;

/// How long the phone's callback listener waits for the browser.
const LOGIN_WAIT: std::time::Duration = std::time::Duration::from_secs(10 * 60);

/// Callback listeners the phone holds for computers' sign-ins, by login id.
static RELAYS: LazyLock<Mutex<HashMap<String, tokio::task::JoinHandle<()>>>> =
    LazyLock::new(Default::default);

impl Client {
    /// `StartAgentLogin` on `device_id` for `harness` (`provider`: one of the
    /// agent's login options, `None` for its default). When the page will
    /// redirect to the computer's loopback, the phone listens on that port
    /// and relays the redirect; it answers the browser with `done_url` so the
    /// auth sheet closes by itself.
    pub async fn device_login_start(
        &self,
        device_id: &str,
        harness: &str,
        provider: Option<&str>,
        done_url: &str,
    ) -> Result<AgentLoginStart> {
        let Backend::Live(live) = self.inner.backend() else {
            return Err(ClientError::Unsupported(
                "Signing a computer's agent in needs that computer.".into(),
            ));
        };
        let reply = live
            .relay
            .call(
                device_id,
                zeron_rpc::methods::START_AGENT_LOGIN,
                serde_json::json!({
                    "harness": harness,
                    "provider": provider,
                    "requesterDeviceId": self.device_id(),
                }),
            )
            .await?;
        let start: AgentLoginStart =
            serde_json::from_value(reply).map_err(|e| ClientError::HostError(e.to_string()))?;
        if start.mode == AgentLoginMode::Browser
            && let Some(port) = start.callback_port
            && let Err(err) = self
                .relay_login_callback(device_id, &start.login_id, port, done_url)
                .await
        {
            self.device_login_cancel(device_id, &start.login_id);
            return Err(err);
        }
        Ok(start)
    }

    /// `PollAgentLogin`: still pending, done, or failed. A page that became
    /// known only now (an agent that installed first) gets its relay here.
    pub async fn device_login_poll(
        &self,
        device_id: &str,
        login_id: &str,
        done_url: &str,
    ) -> Result<AgentLoginPoll> {
        let Backend::Live(live) = self.inner.backend() else {
            return Err(ClientError::Unsupported("No computer in the demo.".into()));
        };
        let reply = live
            .relay
            .call(
                device_id,
                zeron_rpc::methods::POLL_AGENT_LOGIN,
                serde_json::json!({ "loginId": login_id }),
            )
            .await?;
        let poll: AgentLoginPoll =
            serde_json::from_value(reply).map_err(|e| ClientError::HostError(e.to_string()))?;
        match poll.status {
            AgentLoginStatus::Pending => {
                if let Some(port) = poll.callback_port
                    && !lock(&RELAYS).contains_key(login_id)
                {
                    self.relay_login_callback(device_id, login_id, port, done_url)
                        .await?;
                }
            }
            AgentLoginStatus::Done | AgentLoginStatus::Error => stop_relay(login_id),
        }
        Ok(poll)
    }

    /// `CompleteAgentLogin`: the code Claude's page showed, pasted back.
    pub async fn device_login_complete(
        &self,
        device_id: &str,
        login_id: &str,
        code: &str,
    ) -> Result<AgentAccountsSnapshot> {
        let snapshot = self
            .accounts_call(
                device_id,
                zeron_rpc::methods::COMPLETE_AGENT_LOGIN,
                serde_json::json!({ "loginId": login_id, "code": code.trim() }),
            )
            .await;
        stop_relay(login_id);
        snapshot
    }

    /// Give up on a computer's sign-in (best effort on the computer).
    pub fn device_login_cancel(&self, device_id: &str, login_id: &str) {
        stop_relay(login_id);
        if !matches!(self.inner.backend(), Backend::Live(_)) {
            return;
        }
        let client = self.clone();
        let (device_id, login_id) = (device_id.to_owned(), login_id.to_owned());
        crate::runtime::shared().spawn(async move {
            if let Backend::Live(live) = client.inner.backend() {
                let _ = live
                    .relay
                    .call(
                        &device_id,
                        zeron_rpc::methods::CANCEL_AGENT_LOGIN,
                        serde_json::json!({ "loginId": login_id }),
                    )
                    .await;
            }
        });
    }

    /// Listen on the phone's loopback `port` for the sign-in's browser
    /// redirect and relay it to the computer.
    async fn relay_login_callback(
        &self,
        device_id: &str,
        login_id: &str,
        port: u16,
        done_url: &str,
    ) -> Result<()> {
        if !matches!(self.inner.backend(), Backend::Live(_)) {
            return Ok(());
        }
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port))
            .await
            .map_err(|_| {
                ClientError::HostError(format!(
                    "Port {port} is in use on this phone, so the sign-in can't come back to it."
                ))
            })?;
        let client = self.clone();
        let requester = self.device_id().to_owned();
        let (device_id, login, done_url) = (
            device_id.to_owned(),
            login_id.to_owned(),
            done_url.to_owned(),
        );
        let task = crate::runtime::shared().spawn(async move {
            let serve = async {
                loop {
                    let Ok((mut socket, _)) = listener.accept().await else {
                        continue;
                    };
                    let Some(path) = crate::provider_login::read_request_target(&mut socket).await
                    else {
                        continue;
                    };
                    if path.starts_with("/favicon") {
                        let _ = socket
                            .write_all(
                                crate::provider_login::http_response("404 Not Found", &[], "")
                                    .as_bytes(),
                            )
                            .await;
                        continue;
                    }
                    let Backend::Live(live) = client.inner.backend() else {
                        return;
                    };
                    let relayed = live
                        .relay
                        .call(
                            &device_id,
                            zeron_rpc::methods::RELAY_AGENT_LOGIN_CALLBACK,
                            serde_json::json!({
                                "loginId": login,
                                "requesterDeviceId": requester,
                                "path": path,
                            }),
                        )
                        .await;
                    let status = relayed
                        .ok()
                        .and_then(|v| v.get("status").and_then(|s| s.as_u64()))
                        .unwrap_or(0);
                    if (200..400).contains(&status) {
                        let _ = socket
                            .write_all(
                                crate::provider_login::http_response(
                                    "302 Found",
                                    &[("Location", &done_url)],
                                    "",
                                )
                                .as_bytes(),
                            )
                            .await;
                        return;
                    }
                    let _ = socket
                        .write_all(
                            crate::provider_login::http_response(
                                "502 Bad Gateway",
                                &[("Content-Type", "text/plain; charset=utf-8")],
                                "The computer didn't accept the sign-in. Go back to Zeron and try again.",
                            )
                            .as_bytes(),
                        )
                        .await;
                }
            };
            let _ = tokio::time::timeout(LOGIN_WAIT, serve).await;
        });
        if let Some(previous) = lock(&RELAYS).insert(login_id.to_owned(), task) {
            previous.abort();
        }
        Ok(())
    }

    /// `SetHarnessEnabled` on `device_id`: offer `harness` in its pickers or
    /// not. Replies with the device's fresh catalog.
    pub async fn set_harness_enabled(
        &self,
        device_id: &str,
        harness: &str,
        enabled: bool,
    ) -> Result<Vec<HarnessInfo>> {
        let list = match self.inner.backend() {
            Backend::Demo(demo) => {
                lock(&DEMO_DISABLED).insert(harness.to_owned(), !enabled);
                demo.list_harnesses(device_id).await
            }
            Backend::Live(live) => {
                let reply = live
                    .relay
                    .call(
                        device_id,
                        zeron_rpc::methods::SET_HARNESS_ENABLED,
                        serde_json::json!({ "harness": harness, "enabled": enabled }),
                    )
                    .await?;
                let list: Vec<HarnessInfo> = serde_json::from_value(reply)
                    .map_err(|e| ClientError::HostError(e.to_string()))?;
                list.into_iter().filter(|h| h.id != "mock").collect()
            }
        };
        let list = with_demo_toggles(self.inner.is_demo(), list);
        lock(&self.inner.harness_catalogs).insert(device_id.to_owned(), list.clone());
        self.inner.recompute_workspace();
        Ok(list)
    }

    /// `ListAgentAccounts` on `device_id`: its CLI logins per agent.
    pub async fn device_agent_accounts(&self, device_id: &str) -> Result<AgentAccountsSnapshot> {
        self.accounts_call(
            device_id,
            zeron_rpc::methods::LIST_AGENT_ACCOUNTS,
            serde_json::json!({}),
        )
        .await
    }

    /// `ActivateAgentAccount`: the login `harness` runs on from now on.
    pub async fn device_activate_account(
        &self,
        device_id: &str,
        harness: &str,
        account_id: &str,
    ) -> Result<AgentAccountsSnapshot> {
        if self.inner.is_demo() {
            lock(&DEMO_ACTIVE).insert(harness.to_owned(), account_id.to_owned());
        }
        self.accounts_call(
            device_id,
            zeron_rpc::methods::ACTIVATE_AGENT_ACCOUNT,
            serde_json::json!({ "harness": harness, "accountId": account_id }),
        )
        .await
    }

    /// `ForgetAgentAccount`: drop a saved login from that computer.
    pub async fn device_forget_account(
        &self,
        device_id: &str,
        harness: &str,
        account_id: &str,
    ) -> Result<AgentAccountsSnapshot> {
        if self.inner.is_demo() {
            lock(&DEMO_FORGOTTEN).push(account_id.to_owned());
        }
        self.accounts_call(
            device_id,
            zeron_rpc::methods::FORGET_AGENT_ACCOUNT,
            serde_json::json!({ "harness": harness, "accountId": account_id }),
        )
        .await
    }

    async fn accounts_call(
        &self,
        device_id: &str,
        method: &str,
        params: serde_json::Value,
    ) -> Result<AgentAccountsSnapshot> {
        match self.inner.backend() {
            Backend::Demo(_) => Ok(demo_accounts()),
            Backend::Live(live) => {
                let reply = live.relay.call(device_id, method, params).await?;
                serde_json::from_value(reply).map_err(|e| ClientError::HostError(e.to_string()))
            }
        }
    }
}

fn stop_relay(login_id: &str) {
    if let Some(task) = lock(&RELAYS).remove(login_id) {
        task.abort();
    }
}

/// Demo mode's toggles and switches, so the screens respond offline.
static DEMO_DISABLED: LazyLock<Mutex<HashMap<String, bool>>> = LazyLock::new(Default::default);
static DEMO_ACTIVE: LazyLock<Mutex<HashMap<String, String>>> = LazyLock::new(Default::default);
static DEMO_FORGOTTEN: LazyLock<Mutex<Vec<String>>> = LazyLock::new(Default::default);

/// The demo catalog with the toggles applied.
pub(crate) fn with_demo_toggles(demo: bool, mut list: Vec<HarnessInfo>) -> Vec<HarnessInfo> {
    if demo {
        let disabled = lock(&DEMO_DISABLED);
        for harness in &mut list {
            if disabled.get(&harness.id).copied().unwrap_or(false) {
                harness.enabled = Some(false);
            }
        }
    }
    list
}

fn demo_accounts() -> AgentAccountsSnapshot {
    let window = |label: &str, used: f32, hours: i64| zeron_proto::AgentUsageWindow {
        label: label.into(),
        used_fraction: used,
        resets_at: Some(chrono::Utc::now() + chrono::Duration::hours(hours)),
    };
    let account = |id: &str, harness: HarnessId, email: &str, plan: &str| AgentAccount {
        id: id.into(),
        harness,
        email: Some(email.into()),
        plan_label: Some(plan.into()),
        active: false,
        usage_windows: vec![window("Session", 0.34, 3), window("Week", 0.12, 96)],
        usage_fetched_at: None,
        usage_error: None,
        display_name: None,
        organization: None,
        auth_kind: Some(AgentAuthKind::Oauth),
        switchable: true,
        saved_at: None,
        provider: None,
    };
    let mut accounts = vec![
        account(
            "demo-claude-1",
            HarnessId::ClaudeCode,
            "you@acme.dev",
            "Max",
        ),
        account(
            "demo-claude-2",
            HarnessId::ClaudeCode,
            "you@personal.dev",
            "Pro",
        ),
        account("demo-codex-1", HarnessId::Codex, "you@acme.dev", "Plus"),
    ];
    let forgotten = lock(&DEMO_FORGOTTEN);
    accounts.retain(|a| !forgotten.contains(&a.id));
    let active = lock(&DEMO_ACTIVE);
    for harness in [HarnessId::ClaudeCode, HarnessId::Codex] {
        let key = serde_json::to_value(harness)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default();
        let chosen = active.get(&key).cloned();
        let mut picked = false;
        for a in accounts.iter_mut().filter(|a| a.harness == harness) {
            a.active = match &chosen {
                Some(id) => *id == a.id,
                None => !picked,
            };
            picked |= a.active;
        }
    }
    AgentAccountsSnapshot {
        accounts,
        warnings: Vec::new(),
    }
}
