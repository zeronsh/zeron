//! Option B of the usage-limit plan: a thread whose account hit its usage
//! limit continues meanwhile on another connected account slot of the same
//! harness that has quota, when its orchestrator allows it
//! (`CrewPolicy::on_limit`, off by default), and switches back after the
//! original account's reset, at a turn boundary.
//!
//! - **Who may switch**: an orchestrator (or Brain dump) and its crew
//!   threads, by the orchestrator's `on_limit`; a standalone chat never
//!   does. Only on the chat's host: account slots are per device.
//! - **Where to**: [`pick_switch_account`]: another switchable subscription
//!   login of the chat's harness that a chat can run on beside the
//!   machine's login, with quota by a fresh probe (a stale one is probed
//!   again first), the most headroom first. With none and `switch_harness`
//!   on: the other of Claude Code and Codex, when it is enabled here and
//!   one of its logins has quota; the chat's model becomes that harness's
//!   default (or the crew's default model, when it is for that harness).
//! - **How**: a per-chat pin ([`crate::agent_accounts::PinRunEnvironment`]):
//!   Claude Code gets the account's token in its process, Codex the
//!   account's own `CODEX_HOME`; the machine's login never changes. What
//!   the chat ran on before is kept on disk, so a restart keeps the switch.
//!   A parked session is replaced before its next turn, because its run
//!   credential changed.
//! - **Back**: at a turn boundary once the limit's `until` passed, the pins
//!   (and, after a harness switch, the chat's settings, unless changed
//!   since) go back to what they were.

use chrono::Utc;
use zeron_proto::HarnessId;
use zeron_proto::usage_limit::{LimitPolicy, SessionLimit, SwitchedAccount, switched_message};

use crate::agent_accounts::{FRESH_USAGE_MS, pick_switch_account};
use crate::orchestration::ChatRole;
use crate::orchestration::ops::{When, deliver};
use crate::rpc::EngineRpc;

/// The harnesses a limited chat may move between (`switch_harness`): the
/// ones orchestrators run, each with a per-chat account mechanism.
const SWITCH_HARNESSES: [HarnessId; 2] = [HarnessId::ClaudeCode, HarnessId::Codex];

fn harness_name(harness: HarnessId) -> &'static str {
    match harness {
        HarnessId::ClaudeCode => "Claude Code",
        HarnessId::Codex => "Codex",
        other => other.as_str(),
    }
}

impl EngineRpc {
    /// Continue `chat_id`, stopped by `limit`, on another account when its
    /// orchestrator's policy allows it and one has quota: the account it now
    /// runs on, from its next run. The chat is sent [`switched_message`]
    /// here (a `limit_reset` delivery, after its turn); the caller records
    /// the account on the limit. `None`: wait for the reset.
    pub(crate) async fn switch_account_for_limit(&self, chat_id: &str, limit: &SessionLimit) -> Option<SwitchedAccount> {
        let policy = self.limit_policy(chat_id)?;
        if !policy.switch_account {
            return None;
        }
        let workspace = self.workspace_host();
        let chat = workspace.chat(chat_id).ok().flatten()?;
        if chat.device_id != self.orchestration().device_id() {
            return None;
        }
        let config = workspace.chat_config(chat_id)?;
        let harness = config.harness;
        let accounts = self.agent_accounts_ref();
        let earlier = accounts.limit_switch(chat_id);
        let probed: &[HarnessId] = if policy.switch_harness { &SWITCH_HARNESSES } else { std::slice::from_ref(&harness) };
        let listed = self.fresh_accounts(probed).await?;
        let now = Utc::now().timestamp_millis();
        let runnable = |a: &zeron_proto::AgentAccount| accounts.pin_mechanism(a).is_some();

        // What it ran on is out: the limited account, its pin, and the
        // machine's login when it ran on that.
        let pinned = accounts.pinned_account(harness, chat_id);
        let active = listed.iter().find(|a| a.harness == harness && a.active).map(|a| a.id.clone());
        let mut exclude: Vec<String> = Vec::new();
        exclude.extend(limit.account_slot.clone());
        exclude.extend(earlier.as_ref().and_then(|e| e.limited_account.clone()));
        match pinned.clone() {
            Some(pin) => exclude.push(pin),
            None => exclude.extend(active.clone()),
        }
        let exclude: Vec<&str> = exclude.iter().map(String::as_str).collect();
        let limited = limit.account_slot.clone().or(pinned).or(active);

        let (to_harness, account) = match pick_switch_account(&listed, harness, &exclude, now, runnable) {
            Some(account) => (harness, account),
            None if policy.switch_harness => {
                let enabled = self.registry_ref().enabled_set();
                SWITCH_HARNESSES
                    .into_iter()
                    .filter(|other| *other != harness && enabled.contains(other))
                    .find_map(|other| Some((other, pick_switch_account(&listed, other, &exclude, now, runnable)?)))?
            }
            None => return None,
        };
        let configs = (to_harness != harness).then(|| {
            let crew = self.limit_orchestrator(chat_id).map(|o| o.crew);
            let model = crew.filter(|c| c.harness.as_deref() == Some(to_harness.as_str())).and_then(|c| c.model);
            let set = zeron_proto::ChatConfig {
                harness: to_harness,
                model,
                reasoning: None,
                model_options: Default::default(),
                ..config.clone()
            };
            (config.clone(), set)
        });
        let record = accounts.begin_limit_switch(chat_id, harness, limited.as_deref(), to_harness, account, configs.clone());
        if let Some((_, set)) = &configs
            && let Err(error) = workspace.set_chat_config(chat_id, set)
        {
            tracing::warn!(chat = %chat_id, %error, "a limited chat's harness couldn't be switched");
            accounts.end_limit_switch(chat_id);
            return None;
        }
        let switched = SwitchedAccount {
            account_slot: record.account.clone(),
            account_label: record.label.clone(),
            at: Utc::now(),
            harness: configs.as_ref().map(|(_, set)| set.harness.as_str().to_owned()),
            model: configs.as_ref().and_then(|(_, set)| set.model.clone()),
        };
        let to = match &configs {
            Some(_) => format!("{} ({})", record.label, harness_name(to_harness)),
            None => record.label.clone(),
        };
        let from = limit.account_label.clone().unwrap_or_else(|| format!("your {} account", harness_name(harness)));
        let injected =
            zeron_doc::Injected { chat_id: Some(chat_id.to_owned()), ..zeron_doc::Injected::of(zeron_doc::injected::LIMIT_RESET) };
        if let Err(error) = deliver(self, chat_id, &switched_message(&to, &from), None, When::Next, Some(injected)) {
            tracing::warn!(chat = %chat_id, %error, "a switched chat couldn't be told to continue");
        }
        Some(switched)
    }

    /// At a turn boundary of `chat_id` running on another account after its
    /// own account's limit reset: back to its own account. `true` when it
    /// switched back.
    pub(crate) async fn switch_back_after_reset(&self, chat_id: &str, limit: &SessionLimit) -> bool {
        let accounts = self.agent_accounts_ref();
        if accounts.limit_switch(chat_id).is_none() {
            return false;
        }
        if limit.until.is_none_or(|until| until > Utc::now()) {
            return false;
        }
        let sessions = self.sessions_ref();
        if sessions.turn_in_flight(chat_id) || sessions.holds_background_work(chat_id) {
            return false;
        }
        let Some(record) = accounts.end_limit_switch(chat_id) else { return false };
        // Its settings, unless someone changed them since the switch.
        if let (Some(from), Some(set)) = (&record.from_config, &record.config) {
            let workspace = self.workspace_host();
            if workspace.chat_config(chat_id).as_ref() == Some(set)
                && let Err(error) = workspace.set_chat_config(chat_id, from)
            {
                tracing::warn!(chat = %chat_id, %error, "a switched chat's settings couldn't be put back");
            }
        }
        true
    }

    /// The account `chat_id` runs on while its own is limited, as kept on
    /// this host (`limited.switched_to` carries the same to every device).
    #[allow(dead_code)]
    pub(crate) fn limit_switch_of(&self, chat_id: &str) -> Option<crate::agent_accounts::LimitSwitch> {
        self.agent_accounts_ref().limit_switch(chat_id)
    }

    /// The orchestrator whose policy `chat_id` follows: itself, or its
    /// crew's. `None` for a standalone chat.
    fn limit_orchestrator(&self, chat_id: &str) -> Option<zeron_proto::orchestration::Orchestrator> {
        // (No orchestrator's policy while Orchestrators is off: no switch.)
        let orchestration = self.active_orchestration()?;
        let id = match orchestration.role_of(chat_id) {
            ChatRole::Plain => return None,
            role => role.orchestrator_id()?.to_owned(),
        };
        orchestration.orchestrator(&id).filter(|o| !o.archived)
    }

    fn limit_policy(&self, chat_id: &str) -> Option<LimitPolicy> {
        self.limit_orchestrator(chat_id).map(|o| o.crew.on_limit)
    }

    /// This device's accounts, with usage probed again first when any of
    /// `harnesses`' logins has none fresher than [`FRESH_USAGE_MS`].
    async fn fresh_accounts(&self, harnesses: &[HarnessId]) -> Option<Vec<zeron_proto::AgentAccount>> {
        let accounts = self.agent_accounts_ref();
        let listed = accounts.list(false).await.ok()?.accounts;
        let now = Utc::now().timestamp_millis();
        let stale = listed
            .iter()
            .filter(|a| harnesses.contains(&a.harness) && a.switchable)
            // (An API key has no windows to probe.)
            .filter(|a| a.auth_kind != Some(zeron_proto::AgentAuthKind::ApiKey))
            .any(|a| a.usage_fetched_at.is_none_or(|at| now - at > FRESH_USAGE_MS));
        if !stale {
            return Some(listed);
        }
        Some(accounts.list(true).await.map(|s| s.accounts).unwrap_or(listed))
    }
}

#[cfg(test)]
mod tests;
