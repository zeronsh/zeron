//! Plugins' agent ops and events (agents.rs), against a stand-in engine:
//! each permission granted and refused, reads that never force a probe,
//! switching and setting (paced, attributed, refused while busy), and
//! events told on change only, fitted to each plugin's grants.

use std::sync::atomic::{AtomicUsize, Ordering};

use chrono::{TimeZone, Utc};
use zeron_proto::{
    AccessMode, AgentUsageWindow, ModelOption, ModelOptionChoice, NativeTurnOutcome, SandboxLevel,
};

use super::*;

pub(crate) fn window(label: &str, used: f32, resets: i64) -> AgentUsageWindow {
    AgentUsageWindow {
        label: label.into(),
        used_fraction: used,
        resets_at: Some(Utc.timestamp_opt(resets, 0).unwrap()),
    }
}

pub(crate) fn account(id: &str, active: bool, used: f32) -> AgentAccount {
    AgentAccount {
        id: id.into(),
        harness: HarnessId::ClaudeCode,
        email: Some(format!("{id}@example.com")),
        plan_label: Some("Max".into()),
        active,
        usage_windows: vec![window("Session", used, 1_000), window("Week", 0.2, 9_000)],
        usage_fetched_at: Some(1),
        usage_error: None,
        codex_reset_credits: None,
        display_name: Some(format!("Person {id}")),
        organization: None,
        auth_kind: None,
        switchable: true,
        saved_at: None,
        provider: None,
    }
}

pub(crate) fn model(id: &str) -> Model {
    Model {
        id: id.into(),
        label: id.to_uppercase(),
        description: None,
        reasoning_levels: vec![ReasoningLevel::Low, ReasoningLevel::High],
        options: vec![ModelOption {
            id: "fast".into(),
            label: "Fast".into(),
            choices: vec![
                ModelOptionChoice { id: "on".into(), label: "On".into() },
                ModelOptionChoice { id: "off".into(), label: "Off".into() },
            ],
            default_choice: "off".into(),
        }],
    }
}

pub(crate) fn chat(id: &str, model: &str) -> Chat {
    serde_json::from_value(json!({
        "id": id, "deviceId": "dev-1", "title": "A chat", "archived": false,
        "cwd": null, "branch": null, "checkoutId": null,
        "config": { "harness": "claude-code", "model": model, "reasoning": "low", "modelOptions": {}, "sandbox": "danger-full-access" },
        "lastMessagePreview": null, "lastMessageAt": null, "createdAt": "2026-11-01T00:00:00Z",
    }))
    .expect("a chat")
}

pub(crate) fn session(chat: &str, status: SessionStatus, outcome: Option<DoneStatus>) -> Session {
    Session {
        last_turn_outcome: outcome.map(|status| NativeTurnOutcome { status, finished_at: Utc::now() }),
        terminal_activity: None,
        last_completed_turn: None,
        chat_id: chat.into(),
        device_id: "dev-1".into(),
        status,
        started_at: None,
        updated_at: Utc::now(),
        running_subagents: 0,
        background_tasks: Vec::new(),
        waiting: None,
    }
}

#[derive(Default)]
pub(crate) struct Host {
    pub(crate) snapshot: Mutex<AgentAccountsSnapshot>,
    pub(crate) chats: Mutex<Vec<Chat>>,
    pub(crate) sessions: Mutex<Vec<Session>>,
    pub(crate) forced: AtomicUsize,
    pub(crate) activations: AtomicUsize,
    /// Messages delivered: (chat, text, attachments, how).
    pub(crate) delivered: Mutex<Vec<(String, String, Vec<String>, Delivery)>>,
    pub(crate) created: Mutex<Vec<NewChat>>,
    /// The question each chat waits on.
    pub(crate) pending: Mutex<HashMap<String, String>>,
    pub(crate) answered: Mutex<Vec<(String, String, Vec<UserInputAnswer>)>>,
    pub(crate) sent_by: Mutex<Vec<(String, String, Json)>>,
    /// Chats pinned to an account: (provider, chat) → account.
    pub(crate) pinned: Mutex<Vec<Pin>>,
    /// Providers whose accounts can be pinned, and how.
    pub(crate) mechanisms: Mutex<HashMap<HarnessId, &'static str>>,
}

impl AgentHost for Host {
    fn accounts(&self, force: bool) -> BoxFuture<'static, Result<AgentAccountsSnapshot, String>> {
        if force {
            self.forced.fetch_add(1, Ordering::SeqCst);
        }
        let s = lock(&self.snapshot).clone();
        Box::pin(async move { Ok(s) })
    }

    fn activate(&self, harness: HarnessId, account: String) -> BoxFuture<'static, Result<AgentAccountsSnapshot, String>> {
        self.activations.fetch_add(1, Ordering::SeqCst);
        let mut s = lock(&self.snapshot);
        for a in s.accounts.iter_mut().filter(|a| a.harness == harness) {
            a.active = a.id == account;
        }
        let s = s.clone();
        Box::pin(async move { Ok(s) })
    }

    fn models(&self, _harness: HarnessId) -> BoxFuture<'static, Result<Vec<Model>, String>> {
        Box::pin(async { Ok(vec![model("opus"), model("sonnet")]) })
    }

    fn chats(&self) -> Vec<Chat> {
        lock(&self.chats).clone()
    }

    fn sessions(&self) -> Vec<Session> {
        lock(&self.sessions).clone()
    }

    fn set_chat_config(&self, chat: &str, config: ChatConfig) -> Result<(), String> {
        let mut chats = lock(&self.chats);
        let c = chats.iter_mut().find(|c| c.id == chat).ok_or("no chat")?;
        c.config = Some(config);
        Ok(())
    }

    fn status(&self, chat: &str) -> Option<SessionStatus> {
        lock(&self.sessions).iter().find(|s| s.chat_id == chat).map(|s| s.status)
    }

    fn deliver(&self, chat: &str, text: &str, attachments: Vec<String>, how: Delivery) -> Result<String, String> {
        let mut delivered = lock(&self.delivered);
        delivered.push((chat.into(), text.into(), attachments, how));
        Ok(format!("m{}", delivered.len()))
    }

    fn create_chat(&self, new: &NewChat) -> Result<(), String> {
        let mut made = chat(&new.chat_id, new.config.model.as_deref().unwrap_or("opus"));
        made.config = Some(new.config.clone());
        lock(&self.chats).push(made);
        lock(&self.created).push(new.clone());
        Ok(())
    }

    fn pending_input(&self, chat: &str) -> Result<Option<String>, String> {
        Ok(lock(&self.pending).get(chat).cloned())
    }

    fn answer(&self, chat: &str, request: &str, answers: Vec<UserInputAnswer>) -> Result<String, String> {
        lock(&self.answered).push((chat.into(), request.into(), answers));
        Ok("cmd-1".into())
    }

    fn mark_sent_by(&self, chat: &str, id: &str, by: Json) {
        lock(&self.sent_by).push((chat.into(), id.into(), by));
    }

    fn default_harness(&self) -> Option<HarnessId> {
        Some(HarnessId::ClaudeCode)
    }

    fn pin_mechanism(&self, account: &AgentAccount) -> Option<&'static str> {
        lock(&self.mechanisms).get(&account.harness).copied().filter(|_| account.auth_kind != Some(zeron_proto::AgentAuthKind::ApiKey))
    }

    fn pins(&self) -> Vec<Pin> {
        lock(&self.pinned).clone()
    }

    fn set_pin(&self, harness: HarnessId, chat: &str, account: Option<&str>) -> Result<(), String> {
        let mut pins = lock(&self.pinned);
        pins.retain(|p| !(p.harness == harness && p.chat == chat));
        if let Some(account) = account {
            pins.push(Pin { chat: chat.into(), harness, account: account.into(), since: 7 });
        }
        Ok(())
    }
}

type Told = (String, Target, String, String, Json);

#[derive(Default)]
struct FakeSink {
    grants: Mutex<HashMap<String, Vec<String>>>,
    told: Mutex<Vec<Told>>,
    /// Services that fail every event.
    failing: Mutex<HashSet<String>>,
    /// What a service answers when asked (`run.preparing`), per plugin;
    /// `delay` is how long it takes first.
    answers: Mutex<HashMap<String, (Result<Json, String>, Duration)>>,
    asked: Mutex<Vec<Told>>,
    /// How many asks were in flight at once, at most.
    inflight: Arc<AtomicUsize>,
    most_inflight: Arc<AtomicUsize>,
}

impl Sink for FakeSink {
    fn grants(&self, plugin: &str) -> Option<Grant> {
        lock(&self.grants).get(plugin).map(|allowed| Grant {
            name: format!("Plugin {plugin}"),
            allowed: allowed.clone(),
        })
    }

    fn to_window(&self, plugin: &str, listener: &str, event: &str, data: Json) {
        lock(&self.told).push((plugin.into(), Target::Window, listener.into(), event.into(), data));
    }

    fn to_service(&self, plugin: &str, listener: &str, event: &str, data: Json) -> BoxFuture<'static, Result<(), String>> {
        lock(&self.told).push((plugin.into(), Target::Service, listener.into(), event.into(), data));
        let fails = lock(&self.failing).contains(plugin);
        Box::pin(async move { if fails { Err("it errored".into()) } else { Ok(()) } })
    }

    fn ask_service(&self, plugin: &str, listener: &str, event: &str, data: Json) -> BoxFuture<'static, Result<Json, String>> {
        lock(&self.asked).push((plugin.into(), Target::Service, listener.into(), event.into(), data));
        let (answer, delay) = lock(&self.answers).get(plugin).cloned().unwrap_or((Ok(Json::Null), Duration::ZERO));
        let (inflight, most) = (self.inflight.clone(), self.most_inflight.clone());
        Box::pin(async move {
            let now = inflight.fetch_add(1, Ordering::SeqCst) + 1;
            most.fetch_max(now, Ordering::SeqCst);
            tokio::time::sleep(delay).await;
            inflight.fetch_sub(1, Ordering::SeqCst);
            answer
        })
    }
}

struct Fixture {
    agents: Agents,
    host: Arc<Host>,
    sink: Arc<FakeSink>,
}

impl Fixture {
    fn new() -> Self {
        Self::with(Pacing::default())
    }

    fn with(pacing: Pacing) -> Self {
        let agents = Agents::new(pacing);
        let host = Arc::new(Host::default());
        *lock(&host.snapshot) = AgentAccountsSnapshot {
            accounts: vec![account("a1", true, 0.5), account("a2", false, 0.1)],
            warnings: vec![],
        };
        *lock(&host.chats) = vec![chat("c1", "opus")];
        let sink = Arc::new(FakeSink::default());
        agents.set_host(host.clone());
        agents.set_sink(sink.clone());
        Self { agents, host, sink }
    }

    fn grant(&self, plugin: &str, permissions: &[&str]) {
        lock(&self.sink.grants).insert(plugin.into(), permissions.iter().map(|p| p.to_string()).collect());
    }

    async fn op(&self, plugin: &str, args: Json) -> Result<Json, Refused> {
        self.agents.call(plugin, args).await
    }

    fn told(&self) -> Vec<Told> {
        std::mem::take(&mut *lock(&self.sink.told))
    }

    fn events(&self) -> Vec<String> {
        self.told().into_iter().map(|t| t.3).collect()
    }

    /// The accounts as they'd be read next, with the active one at `used`.
    fn set_used(&self, used: f32) {
        let mut s = lock(&self.host.snapshot);
        for a in s.accounts.iter_mut().filter(|a| a.active) {
            a.usage_windows[0].used_fraction = used;
        }
    }

    fn snapshot(&self) -> AgentAccountsSnapshot {
        lock(&self.host.snapshot).clone()
    }
}

#[tokio::test]
async fn agents_each_op_is_refused_without_its_permission_and_answered_with_it() {
    let f = Fixture::new();
    let ops: &[(&str, Json)] = &[
        (USAGE_READ, json!({ "op": "usage" })),
        (USAGE_READ, json!({ "op": "usage_refresh" })),
        (ACCOUNTS_READ, json!({ "op": "accounts" })),
        (ACCOUNTS_SWITCH, json!({ "op": "switch_account", "provider": "claude-code", "account": "a2" })),
        (MODELS_READ, json!({ "op": "models", "harness": "claude-code" })),
        (MODELS_READ, json!({ "op": "chat_model", "chat": "c1" })),
        (MODELS_SET, json!({ "op": "set_model", "chat": "c1", "model": "sonnet" })),
    ];
    // Allowed everything else: each op still needs its own permission.
    let all = [USAGE_READ, ACCOUNTS_READ, ACCOUNTS_SWITCH, MODELS_READ, MODELS_SET, "sessions.read"];
    for (needed, args) in ops {
        let others: Vec<&str> = all.iter().copied().filter(|p| p != needed).collect();
        f.grant("p", &others);
        let refused = f.op("p", args.clone()).await.expect_err("refused without it");
        assert_eq!(refused.reason, reason::NOT_ALLOWED, "{args}");
        assert!(refused.message.contains(needed), "{}", refused.message);
    }
    for (needed, args) in ops {
        let fresh = Fixture::new();
        fresh.grant("p", &[needed]);
        fresh.op("p", args.clone()).await.unwrap_or_else(|r| panic!("{args}: {r:?}"));
    }
    // A plugin that isn't on here gets nothing.
    let refused = f.op("nobody", json!({ "op": "usage" })).await.unwrap_err();
    assert_eq!(refused.reason, reason::NOT_ALLOWED);
}

#[tokio::test]
async fn agents_reading_usage_shows_windows_and_plan_without_email_and_never_probes() {
    let f = Fixture::new();
    f.grant("p", &[USAGE_READ]);
    let usage = f.op("p", json!({ "op": "usage", "provider": "claude-code" })).await.unwrap();
    let first = &usage[0];
    assert_eq!(first["provider"], "claude-code");
    assert_eq!(first["account"], "a1");
    assert_eq!(first["active"], true);
    assert_eq!(first["plan"], "Max");
    assert_eq!(first["windows"][0], json!({ "label": "Session", "used": 50.0, "remaining": 50.0, "resets_at": 1_000_000 }));
    assert!(first.get("email").is_none(), "usage.read never shows who");
    assert_eq!(f.host.forced.load(Ordering::SeqCst), 0, "a read never forces a probe");
    assert!(f.op("p", json!({ "op": "usage", "provider": "codex" })).await.unwrap().as_array().unwrap().is_empty());
    assert_eq!(f.op("p", json!({ "op": "usage", "provider": "nope" })).await.unwrap_err().reason, reason::NOT_FOUND);
}

#[tokio::test]
async fn agents_a_refresh_forces_once_then_is_paced_per_plugin() {
    let f = Fixture::new();
    f.grant("p", &[USAGE_READ]);
    f.grant("q", &[USAGE_READ]);
    f.op("p", json!({ "op": "usage_refresh" })).await.unwrap();
    assert_eq!(f.host.forced.load(Ordering::SeqCst), 1);
    let again = f.op("p", json!({ "op": "usage_refresh" })).await.unwrap_err();
    assert_eq!(again.reason, reason::PACED);
    assert_eq!(f.host.forced.load(Ordering::SeqCst), 1, "paced: no second probe");
    f.op("q", json!({ "op": "usage_refresh" })).await.unwrap();
    assert_eq!(f.host.forced.load(Ordering::SeqCst), 2, "another plugin's own pace");
}

#[tokio::test]
async fn agents_accounts_show_email_only_with_accounts_read_and_never_secrets() {
    let f = Fixture::new();
    f.grant("p", &[ACCOUNTS_READ]);
    let accounts = f.op("p", json!({ "op": "accounts" })).await.unwrap();
    assert_eq!(accounts[0]["email"], "a1@example.com");
    assert_eq!(accounts[0]["name"], "Person a1");
    assert_eq!(accounts[1]["switchable"], true);
    let text = accounts.to_string();
    for secret in ["token", "credential", "codexResetCredits", "reset_credits"] {
        assert!(!text.contains(secret), "{secret} in {text}");
    }
}

#[tokio::test]
async fn agents_switching_makes_the_account_active_says_who_and_isnt_paced() {
    let f = Fixture::new();
    f.grant("p", &[ACCOUNTS_SWITCH]);
    f.grant("watcher", &[ACCOUNTS_READ]);
    f.agents.observe_accounts(&f.snapshot());
    f.agents.listen("watcher", Target::Window, &[("@1".into(), "account.switched".into(), json!({}))]);
    let done = f.op("p", json!({ "op": "switch_account", "provider": "claude-code", "account": "a2" })).await.unwrap();
    assert_eq!(done["status"], "switched");
    assert!(f.snapshot().accounts.iter().any(|a| a.id == "a2" && a.active));
    let told = f.told();
    assert_eq!(told.len(), 1, "{told:?}");
    let (_, _, _, event, data) = &told[0];
    assert_eq!(event, "account.switched");
    assert_eq!((data["account"].as_str(), data["from"].as_str()), (Some("a2"), Some("a1")));
    assert_eq!(data["by"], json!({ "plugin": "p", "name": "Plugin p" }));
    assert_eq!(data["email"], "a2@example.com");
    // The account that's active already: refused, so nothing loops.
    let again = f.op("p", json!({ "op": "switch_account", "provider": "claude-code", "account": "a2" })).await.unwrap_err();
    assert_eq!(again.reason, reason::ALREADY);
    // Back at once: no pace.
    let back = f.op("p", json!({ "op": "switch_account", "provider": "claude-code", "account": "a1" })).await.unwrap();
    assert_eq!(back["status"], "switched");
    assert_eq!(f.host.activations.load(Ordering::SeqCst), 2);
    assert_eq!(f.agents.status("p")["actions"][0]["op"], "switch_account");
    let missing = f.op("p", json!({ "op": "switch_account", "provider": "claude-code", "account": "zz" })).await.unwrap_err();
    assert_eq!(missing.reason, reason::NOT_FOUND);
}

#[tokio::test]
async fn agents_a_switch_happens_at_once_or_waits_when_asked() {
    let f = Fixture::new();
    f.grant("p", &[ACCOUNTS_SWITCH]);
    *lock(&f.host.sessions) = vec![session("c1", SessionStatus::Working, None)];
    f.agents.observe_sessions(&f.host.sessions());
    // A chat running: switched at once all the same.
    let now = f.op("p", json!({ "op": "switch_account", "provider": "claude-code", "account": "a2" })).await.unwrap();
    assert_eq!(now["status"], "switched");
    assert_eq!(f.host.activations.load(Ordering::SeqCst), 1);
    // `when_idle`: waits for it.
    let waiting = f
        .op("p", json!({ "op": "switch_account", "provider": "claude-code", "account": "a1", "when_idle": true }))
        .await
        .unwrap();
    assert_eq!(waiting["status"], "waiting");
    assert_eq!(f.host.activations.load(Ordering::SeqCst), 1, "nothing more switched under a running chat");
    *lock(&f.host.sessions) = vec![session("c1", SessionStatus::Idle, Some(DoneStatus::Completed))];
    f.agents.observe_sessions(&f.host.sessions());
    for _ in 0..50 {
        if f.host.activations.load(Ordering::SeqCst) == 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(f.host.activations.load(Ordering::SeqCst), 2, "switched once the turn finished");
    assert!(f.snapshot().accounts.iter().any(|a| a.id == "a1" && a.active));
}

#[tokio::test]
async fn agents_setting_a_model_checks_it_against_the_catalog_and_is_told_with_by() {
    let f = Fixture::new();
    f.grant("p", &[MODELS_SET]);
    f.grant("watcher", &[MODELS_READ]);
    f.agents.observe_chats(&f.host.chats());
    f.agents.listen("watcher", Target::Service, &[("@1".into(), "model.changed".into(), json!({}))]);
    let set = f
        .op("p", json!({ "op": "set_model", "chat": "c1", "model": "sonnet", "effort": "high", "options": { "fast": "on" } }))
        .await
        .unwrap();
    assert_eq!(set["status"], "set");
    let config = f.host.chats()[0].config.clone().unwrap();
    assert_eq!((config.model.as_deref(), config.reasoning), (Some("sonnet"), Some(ReasoningLevel::High)));
    assert_eq!(config.model_options["fast"], "on");
    assert_eq!((config.sandbox, config.access), (SandboxLevel::DangerFullAccess, AccessMode::default()), "the rest is kept");
    f.agents.observe_chats(&f.host.chats());
    tokio::task::yield_now().await;
    let told = f.told();
    assert_eq!(told.len(), 1, "{told:?}");
    assert_eq!(told[0].3, "model.changed");
    assert_eq!(told[0].4["model"], "sonnet");
    assert_eq!(told[0].4["effort"], "high");
    assert_eq!(told[0].4["by"]["plugin"], "p");
    for (args, why) in [
        (json!({ "op": "set_model", "chat": "c1", "model": "gpt" }), reason::NOT_FOUND),
        (json!({ "op": "set_model", "chat": "c1", "effort": "max" }), reason::NOT_FOUND),
        (json!({ "op": "set_model", "chat": "c1", "options": { "fast": "maybe" } }), reason::NOT_FOUND),
        (json!({ "op": "set_model", "chat": "c1", "provider": "codex", "model": "x" }), reason::BAD_CALL),
        (json!({ "op": "set_model", "chat": "c1", "model": "sonnet" }), reason::ALREADY),
        (json!({ "op": "set_model", "chat": "zz" }), reason::NOT_FOUND),
    ] {
        assert_eq!(f.op("p", args.clone()).await.unwrap_err().reason, why, "{args}");
    }
    // Someone else changing it (the person): told without `by`.
    f.host.set_chat_config("c1", ChatConfig { model: Some("opus".into()), ..config }).unwrap();
    f.agents.observe_chats(&f.host.chats());
    tokio::task::yield_now().await;
    let told = f.told();
    assert_eq!(told.len(), 1);
    assert!(told[0].4.get("by").is_none());
}

#[tokio::test]
async fn agents_a_threshold_fires_on_crossing_only_and_again_after_it_recovers() {
    let f = Fixture::new();
    f.grant("p", &[USAGE_READ]);
    f.agents.listen(
        "p",
        Target::Window,
        &[("@1".into(), "usage.threshold".into(), json!({ "below": 10, "provider": "claude-code", "window": "Session" }))],
    );
    // The first reading is only a baseline (50% left).
    f.agents.observe_accounts(&f.snapshot());
    assert!(f.events().is_empty());
    // 50 → 20 → 10.5 left: still above the line.
    for used in [0.8, 0.895] {
        f.set_used(used);
        f.agents.observe_accounts(&f.snapshot());
    }
    assert!(!f.events().contains(&"usage.threshold".to_string()));
    // 10.5 → 5 left: crossed. Told once, with the numbers.
    f.set_used(0.95);
    f.agents.observe_accounts(&f.snapshot());
    let told: Vec<Told> = f.told().into_iter().filter(|t| t.3 == "usage.threshold").collect();
    assert_eq!(told.len(), 1);
    assert_eq!(told[0].4["remaining"], 5.0);
    assert_eq!(told[0].4["window"], "Session");
    assert_eq!(told[0].4["account"], "a1");
    assert!(told[0].4.get("email").is_none(), "no accounts.read: no email");
    // Lower still: no second time.
    f.set_used(0.99);
    f.agents.observe_accounts(&f.snapshot());
    assert!(!f.events().contains(&"usage.threshold".to_string()));
    // Back over (a reset), then under again: told again.
    f.set_used(0.0);
    f.agents.observe_accounts(&f.snapshot());
    f.told();
    f.set_used(0.92);
    f.agents.observe_accounts(&f.snapshot());
    assert_eq!(f.events().iter().filter(|e| *e == "usage.threshold").count(), 1);
    // Without usage.read, never.
    f.grant("p", &[]);
    f.set_used(0.0);
    f.agents.observe_accounts(&f.snapshot());
    f.set_used(0.97);
    f.agents.observe_accounts(&f.snapshot());
    assert!(f.events().is_empty());
}

#[tokio::test]
async fn agents_usage_changes_resets_and_limits_are_told_on_change_only() {
    let f = Fixture::new();
    f.grant("p", &[USAGE_READ]);
    f.agents.listen(
        "p",
        Target::Window,
        &[
            ("@1".into(), "usage.changed".into(), json!({})),
            ("@2".into(), "usage.reset".into(), json!({})),
            ("@3".into(), "limit.hit".into(), json!({})),
        ],
    );
    f.agents.observe_accounts(&f.snapshot());
    f.agents.observe_accounts(&f.snapshot());
    assert!(f.events().is_empty(), "the same snapshot again: nothing");
    f.set_used(1.0);
    f.agents.observe_accounts(&f.snapshot());
    assert_eq!(f.events(), vec!["usage.changed", "limit.hit"]);
    {
        let mut s = lock(&f.host.snapshot);
        let w = &mut s.accounts[0].usage_windows[0];
        w.used_fraction = 0.0;
        w.resets_at = Some(Utc.timestamp_opt(20_000, 0).unwrap());
    }
    f.agents.observe_accounts(&f.snapshot());
    assert_eq!(f.events(), vec!["usage.changed", "usage.reset"]);
}

#[tokio::test]
async fn agents_turn_events_need_sessions_read_and_say_how_the_turn_ended() {
    let f = Fixture::new();
    f.grant("p", &["sessions.read", USAGE_READ]);
    f.grant("blind", &[USAGE_READ]);
    let all: Vec<(String, String, Json)> = ["turn.started", "turn.finished", "turn.errored", "chat.awaiting_input", "chat.input_answered", "limit.hit"]
        .iter()
        .enumerate()
        .map(|(i, e)| (format!("@{i}"), e.to_string(), json!({})))
        .collect();
    f.agents.listen("p", Target::Service, &all);
    f.agents.listen("blind", Target::Service, &all);
    let step = |status, outcome| {
        f.agents.observe_sessions(&[session("c1", status, outcome)]);
    };
    step(SessionStatus::Idle, None); // a baseline
    step(SessionStatus::Working, None);
    step(SessionStatus::AwaitingInput, None);
    step(SessionStatus::Working, None);
    step(SessionStatus::Idle, Some(DoneStatus::Completed));
    tokio::task::yield_now().await;
    let told = f.told();
    assert!(told.iter().all(|t| t.0 == "p"), "no sessions.read: no turn events");
    let events: Vec<&str> = told.iter().map(|t| t.3.as_str()).collect();
    assert_eq!(events, vec!["turn.started", "chat.awaiting_input", "chat.input_answered", "turn.finished"]);
    assert_eq!(told[3].4, json!({ "chat": "c1", "outcome": "completed" }), "no models.read: no provider field at all");
    // A turn the usage limit stopped.
    step(SessionStatus::Working, None);
    f.agents.observe_problem("c1", "Claude usage limit reached — try again after the limit resets.");
    step(SessionStatus::Idle, Some(DoneStatus::Errored));
    tokio::task::yield_now().await;
    let told = f.told();
    let events: Vec<&str> = told.iter().filter(|t| t.0 == "p").map(|t| t.3.as_str()).collect();
    assert_eq!(events, vec!["turn.started", "limit.hit", "turn.finished", "turn.errored"]);
    let finished = told.iter().find(|t| t.3 == "turn.finished").unwrap();
    assert_eq!(finished.4["outcome"], "limited");
    let hit = told.iter().find(|t| t.0 == "p" && t.3 == "limit.hit").unwrap();
    assert_eq!(hit.4["chat"], "c1", "sessions.read: which chat");
    let blind_hit = told.iter().find(|t| t.0 == "blind" && t.3 == "limit.hit").unwrap();
    assert!(blind_hit.4.get("chat").is_none(), "no sessions.read: no chat");
    assert_eq!(blind_hit.4["provider"], "claude-code");
}

#[tokio::test]
async fn agents_a_failing_service_listener_stays_and_its_row_says_so() {
    let f = Fixture::new();
    f.grant("p", &[USAGE_READ]);
    lock(&f.sink.failing).insert("p".into());
    f.agents.listen("p", Target::Service, &[("@1".into(), "usage.changed".into(), json!({}))]);
    f.agents.observe_accounts(&f.snapshot());
    for used in [0.6, 0.7, 0.8, 0.9] {
        f.set_used(used);
        f.agents.observe_accounts(&f.snapshot());
        for _ in 0..5 {
            tokio::task::yield_now().await;
        }
    }
    let told = f.told();
    let to_service = told.iter().filter(|t| t.1 == Target::Service).count();
    assert_eq!(to_service, 4, "told every change, failing or not: {told:?}");
    // Its window is told each time, to say so in its row in Settings → Plugins.
    let said: Vec<&Told> = told.iter().filter(|t| t.1 == Target::Window).collect();
    assert_eq!(said.len(), 4, "{told:?}");
    assert_eq!((said[0].2.as_str(), said[0].3.as_str()), ("@notice", "plugin.notice"));
    let notices = f.agents.notices("p");
    let last = notices.last().unwrap();
    assert!(last.contains("usage.changed") && last.contains("4 times in a row"), "{notices:?}");
    // Hearing one again starts the count over.
    lock(&f.sink.failing).remove("p");
    f.set_used(0.95);
    f.agents.observe_accounts(&f.snapshot());
    for _ in 0..5 {
        tokio::task::yield_now().await;
    }
    lock(&f.sink.failing).insert("p".into());
    f.set_used(0.97);
    f.agents.observe_accounts(&f.snapshot());
    for _ in 0..5 {
        tokio::task::yield_now().await;
    }
    let notices = f.agents.notices("p");
    assert!(!notices.last().unwrap().contains("in a row"), "{notices:?}");
}

#[tokio::test]
async fn agents_a_plugin_answering_its_own_changes_is_stopped_after_16() {
    let f = Fixture::new();
    f.grant("p", &[ACCOUNTS_SWITCH, ACCOUNTS_READ]);
    f.agents.observe_accounts(&f.snapshot());
    // It hears its own switches, and answers each with another.
    f.agents.listen("p", Target::Window, &[("@1".into(), "account.switched".into(), json!({}))]);
    let mut to = ["a2", "a1"].into_iter().cycle();
    for hop in 1..=LOOP_HOPS {
        let done = f.op("p", json!({ "op": "switch_account", "provider": "claude-code", "account": to.next().unwrap() })).await;
        assert_eq!(done.unwrap()["status"], "switched", "hop {hop}");
    }
    let stopped = f
        .op("p", json!({ "op": "switch_account", "provider": "claude-code", "account": to.next().unwrap() }))
        .await
        .unwrap_err();
    assert_eq!(stopped.reason, reason::LOOP);
    assert_eq!(f.host.activations.load(Ordering::SeqCst), LOOP_HOPS as usize);
    assert!(f.agents.notices("p").last().unwrap().contains("Stopped a loop"));
    // The listener stays, and the next change (the one refused, made again)
    // starts over.
    let next = f.op("p", json!({ "op": "switch_account", "provider": "claude-code", "account": "a2" })).await;
    assert_eq!(next.unwrap()["status"], "switched");
}

#[tokio::test]
async fn agents_many_changes_not_set_off_by_their_own_never_count_as_a_loop() {
    let f = Fixture::new();
    f.grant("p", &[ACCOUNTS_SWITCH]);
    // It doesn't listen for its own switches: however many, no loop.
    f.agents.observe_accounts(&f.snapshot());
    let mut to = ["a2", "a1"].into_iter().cycle();
    for hop in 0..(LOOP_HOPS * 3) {
        let done = f.op("p", json!({ "op": "switch_account", "provider": "claude-code", "account": to.next().unwrap() })).await;
        assert_eq!(done.unwrap()["status"], "switched", "change {hop}");
    }
}

#[tokio::test]
async fn agents_events_go_only_to_listeners_that_match() {
    let f = Fixture::new();
    f.grant("p", &[USAGE_READ]);
    f.agents.listen("p", Target::Window, &[("@1".into(), "usage.changed".into(), json!({ "provider": "codex" }))]);
    f.agents.listen("p", Target::Service, &[("@1".into(), "not.an.event".into(), json!({}))]);
    f.agents.observe_accounts(&f.snapshot());
    f.set_used(0.7);
    f.agents.observe_accounts(&f.snapshot());
    assert!(f.events().is_empty(), "a codex listener hears nothing of claude-code");
}

// ── Messages (`messages.send`) ──────────────────────────────────────────────

impl Fixture {
    /// Chat c1's turn as this device sees it, told to `agents`.
    fn turn(&self, status: SessionStatus) {
        let outcome = (status == SessionStatus::Idle).then_some(DoneStatus::Completed);
        let list = vec![session("c1", status, outcome)];
        *lock(&self.host.sessions) = list.clone();
        self.agents.observe_sessions(&list);
    }

    fn delivered(&self) -> Vec<(String, String, Vec<String>, Delivery)> {
        lock(&self.host.delivered).clone()
    }
}

#[tokio::test]
async fn messages_each_op_needs_messages_send() {
    let ops = [
        json!({ "op": "send_message", "chat": "c1", "text": "hi" }),
        json!({ "op": "new_chat", "prompt": "hi" }),
        json!({ "op": "answer", "chat": "c1", "answers": { "q1": "Yes" } }),
    ];
    for args in &ops {
        let f = Fixture::new();
        lock(&f.host.pending).insert("c1".into(), "r1".into());
        // Everything else, never messages.send: refused, and nothing sent.
        f.grant("p", &[USAGE_READ, ACCOUNTS_READ, ACCOUNTS_SWITCH, MODELS_READ, MODELS_SET, "sessions.read", "files.read"]);
        let refused = f.op("p", args.clone()).await.unwrap_err();
        assert_eq!(refused.reason, reason::NOT_ALLOWED, "{args}");
        assert!(refused.message.contains(MESSAGES_SEND), "{}", refused.message);
        f.grant("p", &[MESSAGES_SEND]);
        f.op("p", args.clone()).await.unwrap_or_else(|r| panic!("{args}: {r:?}"));
    }
    let f = Fixture::new();
    f.grant("p", &[MESSAGES_SEND]);
    assert!(f.delivered().is_empty());
    assert!(lock(&f.host.answered).is_empty());
}

#[tokio::test]
async fn messages_go_as_the_composer_sends_them() {
    let f = Fixture::new();
    f.grant("p", &[MESSAGES_SEND]);
    f.turn(SessionStatus::Idle);
    // Nothing running: a turn of its own, whatever the mode.
    let sent = f.op("p", json!({ "op": "send_message", "chat": "c1", "text": " go on ", "mode": "queue" })).await.unwrap();
    assert_eq!((sent["delivery"].as_str(), sent["chat"].as_str()), (Some("run"), Some("c1")));
    // Running: queued for its end by default, or steering it.
    f.turn(SessionStatus::Working);
    let queued = f.op("p", json!({ "op": "send_message", "chat": "c1", "text": "next" })).await.unwrap();
    assert_eq!(queued["delivery"], "queue");
    let steered = f.op("p", json!({ "op": "send_message", "chat": "c1", "text": "also", "mode": "steer" })).await.unwrap();
    assert_eq!(steered["delivery"], "steer");
    let d = f.delivered();
    assert_eq!(d.iter().map(|x| (x.1.as_str(), x.3)).collect::<Vec<_>>(), [("go on", Delivery::Run), ("next", Delivery::Queue), ("also", Delivery::Steer)]);
    // Waiting on an answer: a steer is refused (answer it, or queue).
    f.turn(SessionStatus::AwaitingInput);
    let refused = f.op("p", json!({ "op": "send_message", "chat": "c1", "text": "x", "mode": "steer" })).await.unwrap_err();
    assert_eq!(refused.reason, reason::BAD_CALL);
    for (args, why) in [
        (json!({ "op": "send_message", "chat": "c1", "text": "  " }), reason::BAD_CALL),
        (json!({ "op": "send_message", "chat": "zz", "text": "x" }), reason::NOT_FOUND),
        (json!({ "op": "send_message", "chat": "c1", "text": "x", "mode": "shout" }), reason::BAD_CALL),
        // A file attached names it: that needs files.read too.
        (json!({ "op": "send_message", "chat": "c1", "text": "x", "attachments": ["/etc/hosts"] }), reason::NOT_ALLOWED),
    ] {
        assert_eq!(f.op("p", args.clone()).await.unwrap_err().reason, why, "{args}");
    }
    f.grant("p", &[MESSAGES_SEND, "files.read"]);
    f.op("p", json!({ "op": "send_message", "chat": "c1", "text": "see", "attachments": ["/tmp/a.png"] })).await.unwrap();
    assert_eq!(f.delivered().last().unwrap().2, ["/tmp/a.png"]);
}

#[tokio::test]
async fn messages_say_which_plugin_sent_them_beside_the_transcript() {
    let f = Fixture::new();
    f.grant("p", &[MESSAGES_SEND]);
    f.turn(SessionStatus::Idle);
    let sent = f.op("p", json!({ "op": "send_message", "chat": "c1", "text": "hi" })).await.unwrap();
    let marks = lock(&f.host.sent_by).clone();
    assert_eq!(marks.len(), 1);
    let (chat, id, by) = &marks[0];
    assert_eq!((chat.as_str(), id.as_str()), ("c1", sent["id"].as_str().unwrap()));
    assert_eq!((by["plugin"].as_str(), by["name"].as_str(), by["kind"].as_str()), (Some("p"), Some("Plugin p"), Some("run")));
    let action = &f.agents.status("p")["actions"][0];
    assert_eq!((action["op"].as_str(), action["chat"].as_str()), (Some("send_message"), Some("c1")));
}

#[tokio::test]
async fn messages_turns_a_plugin_set_off_carry_its_by() {
    let f = Fixture::new();
    f.grant("p", &[MESSAGES_SEND]);
    f.grant("w", &["sessions.read"]);
    let events: Vec<(String, String, Json)> = ["turn.started", "turn.finished", "chat.awaiting_input"]
        .iter()
        .enumerate()
        .map(|(i, e)| (format!("@{i}"), e.to_string(), json!({})))
        .collect();
    f.agents.listen("w", Target::Window, &events);
    f.turn(SessionStatus::Idle);
    f.op("p", json!({ "op": "send_message", "chat": "c1", "text": "go" })).await.unwrap();
    f.turn(SessionStatus::Working);
    f.turn(SessionStatus::AwaitingInput);
    f.turn(SessionStatus::Working);
    f.turn(SessionStatus::Idle);
    let told = f.told();
    let by: Vec<(&str, Option<&str>)> = told.iter().map(|t| (t.3.as_str(), t.4["by"]["plugin"].as_str())).collect();
    assert_eq!(by, [("turn.started", Some("p")), ("chat.awaiting_input", Some("p")), ("turn.finished", Some("p"))]);
    // The person's next turn is theirs: no by.
    f.turn(SessionStatus::Working);
    f.turn(SessionStatus::Idle);
    assert!(f.told().iter().all(|t| t.4.get("by").is_none()));
    // Queued behind a running turn: the turn after it is the plugin's.
    f.turn(SessionStatus::Working);
    f.op("p", json!({ "op": "send_message", "chat": "c1", "text": "after" })).await.unwrap();
    f.turn(SessionStatus::Idle);
    f.turn(SessionStatus::Working);
    f.turn(SessionStatus::Idle);
    let by: Vec<(String, Option<String>)> =
        f.told().iter().map(|t| (t.3.clone(), t.4["by"]["plugin"].as_str().map(str::to_owned))).collect();
    assert_eq!(by[..2], [("turn.started".to_string(), None), ("turn.finished".to_string(), None)], "the running turn was the person's: {by:?}");
    assert_eq!(by[2..], [("turn.started".to_string(), Some("p".to_string())), ("turn.finished".to_string(), Some("p".to_string()))]);
}

#[tokio::test]
async fn messages_a_plugin_sending_on_its_own_turns_end_is_stopped_after_16() {
    let f = Fixture::new();
    f.grant("p", &[MESSAGES_SEND, "sessions.read"]);
    f.agents.listen("p", Target::Window, &[("@1".into(), "turn.finished".into(), json!({}))]);
    f.turn(SessionStatus::Idle);
    // Its first send, then one on each of its own turns' end.
    for hop in 1..=LOOP_HOPS {
        let sent = f.op("p", json!({ "op": "send_message", "chat": "c1", "text": "continue" })).await;
        assert_eq!(sent.unwrap()["status"], "sent", "hop {hop}");
        f.turn(SessionStatus::Working);
        f.turn(SessionStatus::Idle);
    }
    let stopped = f.op("p", json!({ "op": "send_message", "chat": "c1", "text": "continue" })).await.unwrap_err();
    assert_eq!(stopped.reason, reason::LOOP);
    assert_eq!(f.delivered().len(), LOOP_HOPS as usize);
    assert!(f.agents.notices("p").last().unwrap().contains("Stopped a loop"));
}

#[tokio::test]
async fn messages_a_send_not_set_off_by_its_own_last_starts_the_count_over() {
    let f = Fixture::new();
    f.grant("p", &[MESSAGES_SEND, "sessions.read"]);
    f.agents.listen("p", Target::Window, &[("@1".into(), "turn.finished".into(), json!({}))]);
    f.turn(SessionStatus::Idle);
    let send = || f.op("p", json!({ "op": "send_message", "chat": "c1", "text": "continue" }));
    // Ten of its own in a row.
    for _ in 0..10 {
        send().await.unwrap();
        f.turn(SessionStatus::Working);
        f.turn(SessionStatus::Idle);
    }
    // A turn of the person's ends (no `by`), and it sends on that: not set
    // off by its own, so it counts from one again...
    f.turn(SessionStatus::Working);
    f.turn(SessionStatus::Idle);
    send().await.unwrap();
    // ...so 15 more of its own make 16 in a row, and only the next is stopped.
    for hop in 2..=LOOP_HOPS {
        f.turn(SessionStatus::Working);
        f.turn(SessionStatus::Idle);
        send().await.unwrap_or_else(|r| panic!("hop {hop}: {r:?}"));
    }
    f.turn(SessionStatus::Working);
    f.turn(SessionStatus::Idle);
    assert_eq!(send().await.unwrap_err().reason, reason::LOOP);
}

#[tokio::test]
async fn messages_a_new_chat_is_made_and_sent_its_first_message() {
    let f = Fixture::new();
    f.grant("p", &[MESSAGES_SEND]);
    let made = f
        .op("p", json!({ "op": "new_chat", "prompt": "Fix the build", "project": "atlas", "model": "sonnet", "effort": "high", "cwd": "/work" }))
        .await
        .unwrap();
    let id = made["chat"].as_str().unwrap().to_owned();
    let created = lock(&f.host.created).clone();
    assert_eq!(created.len(), 1);
    let new = &created[0];
    assert_eq!(new.chat_id, id);
    assert_eq!((new.project.as_deref(), new.cwd.as_deref(), new.device.as_deref()), (Some("atlas"), Some("/work"), None));
    assert_eq!(new.config.harness, HarnessId::ClaudeCode, "the default harness");
    assert_eq!((new.config.model.as_deref(), new.config.reasoning), (Some("sonnet"), Some(ReasoningLevel::High)));
    assert_eq!(f.delivered(), [(id.clone(), "Fix the build".to_string(), vec![], Delivery::Run)]);
    assert_eq!(lock(&f.host.sent_by)[0].0, id);
    for (args, why) in [
        (json!({ "op": "new_chat", "prompt": "" }), reason::BAD_CALL),
        (json!({ "op": "new_chat", "prompt": "x", "model": "gpt" }), reason::NOT_FOUND),
        (json!({ "op": "new_chat", "prompt": "x", "harness": "nope" }), reason::NOT_FOUND),
        (json!({ "op": "new_chat", "prompt": "x", "effort": "max" }), reason::NOT_FOUND),
    ] {
        assert_eq!(f.op("p", args.clone()).await.unwrap_err().reason, why, "{args}");
    }
}

#[tokio::test]
async fn messages_a_question_is_answered_as_respond_to_input_answers_it() {
    let f = Fixture::new();
    f.grant("p", &[MESSAGES_SEND]);
    let none = f.op("p", json!({ "op": "answer", "chat": "c1", "answers": { "q1": "Yes" } })).await.unwrap_err();
    assert_eq!(none.reason, reason::NOT_FOUND, "nothing waits");
    lock(&f.host.pending).insert("c1".into(), "r7".into());
    let done = f
        .op("p", json!({ "op": "answer", "chat": "c1", "answers": [{ "question": "q1", "labels": ["Yes"] }, { "question": "q2", "label": "Later" }] }))
        .await
        .unwrap();
    assert_eq!((done["status"].as_str(), done["request"].as_str()), (Some("answered"), Some("r7")));
    let answered = lock(&f.host.answered).clone();
    assert_eq!(answered[0].0, "c1");
    assert_eq!(answered[0].1, "r7");
    assert_eq!(
        answered[0].2,
        [
            UserInputAnswer { question_id: "q1".into(), labels: vec!["Yes".into()] },
            UserInputAnswer { question_id: "q2".into(), labels: vec!["Later".into()] },
        ]
    );
    // A request named is answered as named; no answers is a bad call.
    f.op("p", json!({ "op": "answer", "chat": "c1", "request": "r9", "answers": { "q1": ["A", "B"] } })).await.unwrap();
    assert_eq!(lock(&f.host.answered)[1].1, "r9");
    assert_eq!(f.op("p", json!({ "op": "answer", "chat": "c1", "answers": {} })).await.unwrap_err().reason, reason::BAD_CALL);
}

// ── Pinning a chat to an account ───────────────────────────────────────────

const PIN: &[&str] = &[ACCOUNTS_PIN, ACCOUNTS_READ, SESSIONS_READ];

impl Fixture {
    fn pinning(&self) -> &Self {
        lock(&self.host.mechanisms).insert(HarnessId::ClaudeCode, "env");
        self
    }

    fn answer(&self, plugin: &str, answer: Result<Json, String>, delay: Duration) {
        lock(&self.sink.answers).insert(plugin.into(), (answer, delay));
    }

    fn hook(&self, plugin: &str) {
        self.agents.listen(plugin, Target::Service, &[("@1".into(), "run.preparing".into(), json!({}))]);
    }
}

fn pin(chat: &str, account: &str) -> Json {
    json!({ "op": "pin_account", "provider": "claude-code", "chat": chat, "account": account })
}

#[tokio::test]
async fn pins_need_their_permissions_and_are_answered_with_them() {
    let f = Fixture::new();
    f.pinning();
    f.grant("p", &[ACCOUNTS_READ, SESSIONS_READ]);
    let refused = f.op("p", pin("c1", "a2")).await.unwrap_err();
    assert_eq!(refused.reason, reason::NOT_ALLOWED);
    assert!(refused.message.contains(ACCOUNTS_PIN), "{}", refused.message);
    f.grant("p", &[ACCOUNTS_PIN]);
    assert_eq!(f.op("p", json!({ "op": "pins" })).await.unwrap_err().reason, reason::NOT_ALLOWED, "reading pins is accounts.read");
    f.op("p", pin("c1", "a2")).await.unwrap();
    f.grant("p", &[ACCOUNTS_PIN, ACCOUNTS_READ]);
    f.op("p", json!({ "op": "pins" })).await.unwrap();
}

#[tokio::test]
async fn a_pin_runs_a_chat_on_a_saved_account_says_who_and_shows_in_pins() {
    let f = Fixture::new();
    f.pinning().grant("p", PIN);
    f.grant("watcher", &[ACCOUNTS_READ]);
    f.agents.observe_accounts(&f.snapshot());
    f.agents.listen("watcher", Target::Window, &[("@1".into(), "account.pinned".into(), json!({}))]);
    let done = f.op("p", pin("c1", "a2")).await.unwrap();
    assert_eq!(done, json!({ "provider": "claude-code", "account": "a2", "chat": "c1" }));
    assert_eq!(lock(&f.host.pinned).len(), 1);
    let told = f.told();
    assert_eq!(told.len(), 1, "{told:?}");
    let (_, _, _, event, data) = &told[0];
    assert_eq!(event, "account.pinned");
    assert_eq!((data["account"].as_str(), data["from"].as_str()), (Some("a2"), None));
    assert_eq!(data["by"], json!({ "plugin": "p", "name": "Plugin p" }));
    assert!(data.get("chat").is_none(), "a chat's id needs sessions.read: {data}");
    let pins = f.op("p", json!({ "op": "pins" })).await.unwrap();
    assert_eq!(pins, json!([{ "provider": "claude-code", "account": "a2", "chat": "c1", "since": 7 }]));
    // Without sessions.read the rows are counted, not named.
    f.grant("q", &[ACCOUNTS_READ]);
    assert_eq!(f.op("q", json!({ "op": "pins" })).await.unwrap(), json!([{ "provider": "claude-code", "account": "a2", "since": 7 }]));
    assert_eq!(f.agents.status("p")["actions"][0]["op"], "pin_account");
    // Moving it says where it was; the same again is refused so nothing loops.
    f.op("p", pin("c1", "a1")).await.unwrap();
    assert_eq!(f.told().last().unwrap().4["from"], "a2");
    assert_eq!(f.op("p", pin("c1", "a1")).await.unwrap_err().reason, reason::ALREADY);
}

#[tokio::test]
async fn a_pin_is_taken_back_with_no_account() {
    let f = Fixture::new();
    f.pinning().grant("p", PIN);
    f.agents.observe_accounts(&f.snapshot());
    f.agents.listen("p", Target::Window, &[("@1".into(), "account.unpinned".into(), json!({}))]);
    f.op("p", pin("c1", "a2")).await.unwrap();
    f.told();
    let done = f.op("p", json!({ "op": "pin_account", "provider": "claude-code", "chat": "c1" })).await.unwrap();
    assert_eq!(done, json!({ "provider": "claude-code", "chat": "c1" }));
    assert!(lock(&f.host.pinned).is_empty());
    let told = f.told();
    assert_eq!(told.len(), 1, "{told:?}");
    assert_eq!(told[0].3, "account.unpinned");
    assert_eq!((told[0].4["account"].as_str(), told[0].4["chat"].as_str()), (Some("a2"), Some("c1")));
    let nothing = f.op("p", json!({ "op": "pin_account", "provider": "claude-code", "chat": "c1" })).await.unwrap_err();
    assert_eq!(nothing.reason, reason::ALREADY, "nothing was pinned");
}

#[tokio::test]
async fn a_pin_is_refused_for_what_cant_be_pinned() {
    let f = Fixture::new();
    f.grant("p", PIN);
    // No provider mechanism on this device.
    assert_eq!(f.op("p", pin("c1", "a2")).await.unwrap_err().reason, reason::FAILED);
    f.pinning();
    assert_eq!(f.op("p", pin("c1", "zz")).await.unwrap_err().reason, reason::NOT_FOUND);
    assert_eq!(f.op("p", pin("nope", "a2")).await.unwrap_err().reason, reason::NOT_FOUND);
    assert_eq!(f.op("p", json!({ "op": "pin_account", "chat": "c1", "account": "a2" })).await.unwrap_err().reason, reason::BAD_CALL);
    assert_eq!(f.op("p", json!({ "op": "pin_account", "provider": "claude-code", "account": "a2" })).await.unwrap_err().reason, reason::BAD_CALL);
    // An API key bills per token: spreading over it saves nothing.
    lock(&f.host.snapshot).accounts[1].auth_kind = Some(zeron_proto::AgentAuthKind::ApiKey);
    let refused = f.op("p", pin("c1", "a2")).await.unwrap_err();
    assert_eq!(refused.reason, reason::FAILED);
    assert!(lock(&f.host.pinned).is_empty());
}

#[tokio::test]
async fn pins_are_part_of_the_loop_guard() {
    let f = Fixture::new();
    f.pinning().grant("p", PIN);
    f.agents.observe_accounts(&f.snapshot());
    f.agents.listen("p", Target::Window, &[("@1".into(), "account.pinned".into(), json!({}))]);
    let mut refused = None;
    for i in 0..(LOOP_HOPS + 2) {
        let account = if i % 2 == 0 { "a2" } else { "a1" };
        match f.op("p", pin("c1", account)).await {
            Ok(_) => {}
            Err(r) => {
                refused = Some(r);
                break;
            }
        }
    }
    assert_eq!(refused.expect("stopped").reason, reason::LOOP);
}

#[tokio::test]
async fn account_rows_say_whether_they_can_be_pinned_and_whether_they_are_usable() {
    let f = Fixture::new();
    f.grant("p", &[ACCOUNTS_READ, USAGE_READ]);
    let rows = f.op("p", json!({ "op": "accounts" })).await.unwrap();
    assert_eq!((rows[0]["pinnable"].clone(), rows[0].get("mechanism")), (json!(false), None), "no mechanism on this device");
    f.pinning();
    lock(&f.host.snapshot).accounts[1].usage_error = Some("Sign in again".into());
    lock(&f.host.snapshot).accounts[0].usage_windows[0] = window("Session", 1.0, 4_000_000_000);
    // A full window whose reset has passed is stale, not a limit.
    lock(&f.host.snapshot).accounts[1].usage_windows[0] = window("Session", 1.0, 1_000);
    let rows = f.op("p", json!({ "op": "accounts" })).await.unwrap();
    assert_eq!(rows[0]["pinnable"], true);
    assert_eq!(rows[0]["mechanism"], "env");
    assert_eq!(rows[0]["signed_out"], false);
    assert_eq!(rows[1]["signed_out"], true);
    // A window that's full and hasn't reset says when it will (usage.read).
    let usage = f.op("p", json!({ "op": "usage" })).await.unwrap();
    assert_eq!(usage[0]["limited_until"], 4_000_000_000_000_i64);
    assert!(usage[1].get("limited_until").is_none());
    assert!(rows[0].get("limited_until").is_none(), "limits are usage");
}

#[tokio::test]
async fn pins_of_an_account_that_is_gone_are_taken_back_and_told() {
    let f = Fixture::new();
    f.pinning().grant("p", PIN);
    f.agents.observe_accounts(&f.snapshot());
    f.op("p", pin("c1", "a2")).await.unwrap();
    f.agents.listen("p", Target::Window, &[("@1".into(), "account.unpinned".into(), json!({}))]);
    f.told();
    let mut next = f.snapshot();
    next.accounts.retain(|a| a.id != "a2");
    *lock(&f.host.snapshot) = next.clone();
    f.agents.observe_accounts(&next);
    assert!(lock(&f.host.pinned).is_empty());
    let told = f.told();
    let unpinned: Vec<_> = told.iter().filter(|t| t.3 == "account.unpinned").collect();
    assert_eq!(unpinned.len(), 1, "{told:?}");
    assert_eq!(unpinned[0].4["reason"], "removed");
    assert!(unpinned[0].4.get("by").is_none(), "nobody's change");
}

#[tokio::test]
async fn the_pre_run_hook_takes_the_listeners_account_and_pins_the_chat() {
    let f = Fixture::new();
    f.pinning().grant("p", PIN);
    f.agents.observe_accounts(&f.snapshot());
    f.hook("p");
    f.answer("p", Ok(json!({ "account": "a2" })), Duration::ZERO);
    let got = f.agents.preparing(HarnessId::ClaudeCode, "c1").await;
    assert_eq!(got.as_deref(), Some("a2"));
    assert_eq!(lock(&f.host.pinned)[0].account, "a2");
    let asked = lock(&f.sink.asked).clone();
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].3, "run.preparing");
    assert_eq!((asked[0].4["provider"].as_str(), asked[0].4["chat"].as_str()), (Some("claude-code"), Some("c1")));
    // Told as any pin: with who made it.
    assert_eq!(f.agents.status("p")["actions"][0]["op"], "pin_account");
    // A chat that has its pin isn't asked again.
    assert_eq!(f.agents.preparing(HarnessId::ClaudeCode, "c1").await, None);
    assert_eq!(lock(&f.sink.asked).len(), 1);
}

#[tokio::test]
async fn the_pre_run_hook_gives_up_after_its_wait_and_runs_the_chat_on_the_live_login() {
    let f = Fixture::with(Pacing { prepare_wait: Duration::from_millis(50), ..Pacing::default() });
    f.pinning().grant("p", PIN);
    f.agents.observe_accounts(&f.snapshot());
    f.hook("p");
    f.answer("p", Ok(json!({ "account": "a2" })), Duration::from_secs(5));
    let started = Instant::now();
    assert_eq!(f.agents.preparing(HarnessId::ClaudeCode, "c1").await, None);
    assert!(started.elapsed() < Duration::from_secs(2), "it didn't wait for the answer");
    assert!(lock(&f.host.pinned).is_empty());
    assert!(f.agents.notices("p").iter().any(|n| n.contains("run.preparing") && n.contains("answer")), "{:?}", f.agents.notices("p"));
}

#[tokio::test]
async fn the_pre_run_hook_ignores_what_it_cant_use() {
    let f = Fixture::new();
    f.pinning().grant("p", PIN);
    f.agents.observe_accounts(&f.snapshot());
    f.hook("p");
    // An error, an account that isn't there, a bare nothing: no pin.
    for answer in [Err("boom".to_string()), Ok(json!({ "account": "zz" })), Ok(Json::Null), Ok(json!({ "account": 4 }))] {
        f.answer("p", answer, Duration::ZERO);
        let chat = format!("c{}", lock(&f.sink.asked).len() + 10);
        lock(&f.host.chats).push(super::tests::chat(&chat, "opus"));
        assert_eq!(f.agents.preparing(HarnessId::ClaudeCode, &chat).await, None);
    }
    assert!(lock(&f.host.pinned).is_empty());
    // A plugin that wasn't allowed accounts.pin is never asked.
    let g = Fixture::new();
    g.pinning().grant("p", &[ACCOUNTS_READ]);
    g.agents.observe_accounts(&g.snapshot());
    g.hook("p");
    g.answer("p", Ok(json!({ "account": "a2" })), Duration::ZERO);
    assert_eq!(g.agents.preparing(HarnessId::ClaudeCode, "c1").await, None);
    assert!(lock(&g.sink.asked).is_empty());
    // Nor is one asked about a provider with no mechanism here.
    let h = Fixture::new();
    h.grant("p", PIN);
    h.hook("p");
    h.answer("p", Ok(json!({ "account": "a2" })), Duration::ZERO);
    assert_eq!(h.agents.preparing(HarnessId::ClaudeCode, "c1").await, None);
    assert!(lock(&h.sink.asked).is_empty());
}

#[tokio::test]
async fn a_chat_the_hook_passed_on_isnt_asked_again_but_can_still_be_pinned() {
    let f = Fixture::new();
    f.pinning().grant("p", PIN);
    f.agents.observe_accounts(&f.snapshot());
    f.hook("p");
    f.answer("p", Ok(Json::Null), Duration::ZERO);
    assert_eq!(f.agents.preparing(HarnessId::ClaudeCode, "c1").await, None);
    assert_eq!(f.agents.preparing(HarnessId::ClaudeCode, "c1").await, None);
    assert_eq!(lock(&f.sink.asked).len(), 1, "once per chat");
    f.op("p", pin("c1", "a2")).await.unwrap();
}

#[tokio::test]
async fn the_first_listener_with_an_account_wins() {
    let f = Fixture::new();
    f.pinning().grant("p", PIN);
    f.grant("q", PIN);
    f.agents.observe_accounts(&f.snapshot());
    f.hook("p");
    f.hook("q");
    f.answer("p", Ok(Json::Null), Duration::ZERO);
    f.answer("q", Ok(json!({ "account": "a1" })), Duration::ZERO);
    assert_eq!(f.agents.preparing(HarnessId::ClaudeCode, "c1").await.as_deref(), Some("a1"));
    assert_eq!(lock(&f.sink.asked).len(), 2);
}

#[tokio::test]
async fn the_pre_run_hook_is_asked_one_chat_at_a_time_so_each_answer_sees_the_last_pin() {
    let f = Fixture::new();
    f.pinning().grant("p", PIN);
    f.agents.observe_accounts(&f.snapshot());
    lock(&f.host.chats).push(super::tests::chat("c2", "opus"));
    f.hook("p");
    f.answer("p", Ok(json!({ "account": "a2" })), Duration::from_millis(40));
    let (a, b) = tokio::join!(
        f.agents.preparing(HarnessId::ClaudeCode, "c1"),
        f.agents.preparing(HarnessId::ClaudeCode, "c2"),
    );
    assert_eq!((a.as_deref(), b.as_deref()), (Some("a2"), Some("a2")));
    assert_eq!(f.sink.most_inflight.load(Ordering::SeqCst), 1, "never two asks at once");
    assert_eq!(lock(&f.host.pinned).len(), 2);
}

#[tokio::test]
async fn with_nobody_listening_the_pre_run_hook_reads_nothing_and_waits_for_no_one() {
    let f = Fixture::new();
    f.pinning().grant("p", PIN);
    assert_eq!(f.agents.preparing(HarnessId::ClaudeCode, "c1").await, None);
    assert!(lock(&f.sink.asked).is_empty());
    assert_eq!(f.host.forced.load(Ordering::SeqCst), 0);
    // It isn't remembered as passed on: a listener that arrives later is asked.
    f.agents.observe_accounts(&f.snapshot());
    f.hook("p");
    f.answer("p", Ok(json!({ "account": "a2" })), Duration::ZERO);
    assert_eq!(f.agents.preparing(HarnessId::ClaudeCode, "c1").await.as_deref(), Some("a2"));
}
