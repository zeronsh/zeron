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
    }
}

#[derive(Default)]
pub(crate) struct Host {
    pub(crate) snapshot: Mutex<AgentAccountsSnapshot>,
    pub(crate) chats: Mutex<Vec<Chat>>,
    pub(crate) sessions: Mutex<Vec<Session>>,
    pub(crate) forced: AtomicUsize,
    pub(crate) activations: AtomicUsize,
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
}

type Told = (String, Target, String, String, Json);

#[derive(Default)]
struct FakeSink {
    grants: Mutex<HashMap<String, Vec<String>>>,
    told: Mutex<Vec<Told>>,
    /// Services that fail every event.
    failing: Mutex<HashSet<String>>,
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
}

struct Fixture {
    agents: Agents,
    host: Arc<Host>,
    sink: Arc<FakeSink>,
}

impl Fixture {
    fn new() -> Self {
        let agents = Agents::new(Pacing::default());
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
async fn agents_switching_makes_the_account_active_says_who_and_is_paced() {
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
    // Back at once: paced.
    let back = f.op("p", json!({ "op": "switch_account", "provider": "claude-code", "account": "a1" })).await.unwrap_err();
    assert_eq!(back.reason, reason::PACED);
    assert_eq!(f.host.activations.load(Ordering::SeqCst), 1);
    assert_eq!(f.agents.status("p")["actions"][0]["op"], "switch_account");
    let missing = f.op("p", json!({ "op": "switch_account", "provider": "claude-code", "account": "zz" })).await.unwrap_err();
    assert_eq!(missing.reason, reason::NOT_FOUND);
}

#[tokio::test]
async fn agents_a_switch_waits_for_running_chats_on_that_provider() {
    let f = Fixture::new();
    f.grant("p", &[ACCOUNTS_SWITCH]);
    *lock(&f.host.sessions) = vec![session("c1", SessionStatus::Working, None)];
    f.agents.observe_sessions(&f.host.sessions());
    let busy = f.op("p", json!({ "op": "switch_account", "provider": "claude-code", "account": "a2" })).await.unwrap_err();
    assert_eq!(busy.reason, reason::BUSY);
    let waiting = f
        .op("p", json!({ "op": "switch_account", "provider": "claude-code", "account": "a2", "when_idle": true }))
        .await
        .unwrap();
    assert_eq!(waiting["status"], "waiting");
    assert_eq!(f.host.activations.load(Ordering::SeqCst), 0, "nothing switched under a running chat");
    *lock(&f.host.sessions) = vec![session("c1", SessionStatus::Idle, Some(DoneStatus::Completed))];
    f.agents.observe_sessions(&f.host.sessions());
    for _ in 0..50 {
        if f.host.activations.load(Ordering::SeqCst) == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(f.host.activations.load(Ordering::SeqCst), 1, "switched once the turn finished");
    assert!(f.snapshot().accounts.iter().any(|a| a.id == "a2" && a.active));
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
async fn agents_a_failing_service_listener_is_dropped_and_said() {
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
    assert_eq!(to_service, 3, "dropped after its third failure: {told:?}");
    // Its window is told, to say so in its row in Settings → Plugins.
    let said: Vec<&Told> = told.iter().filter(|t| t.1 == Target::Window).collect();
    assert_eq!(said.len(), 1, "{told:?}");
    assert_eq!((said[0].2.as_str(), said[0].3.as_str()), ("@dropped", "listener.dropped"));
    let dropped = f.agents.dropped("p");
    assert_eq!(dropped.len(), 1);
    assert!(dropped[0].contains("usage.changed"), "{dropped:?}");
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
