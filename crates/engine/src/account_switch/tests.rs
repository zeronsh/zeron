//! Limited chats moving to another account, against a whole (local,
//! edge-less) engine whose accounts live under the test's own root.

use chrono::{Duration, Utc};
use futures::StreamExt;
use serde_json::{Value, json};
use zeron_doc::SessionCommandPayload;
use zeron_proto::usage_limit::{LimitPolicy, SessionLimit, switched_message};
use zeron_proto::{HarnessId, SessionStatus};
use zeron_rpc::{Caller, RpcReply, RpcService, methods, with_caller};

use crate::agent_accounts::AgentAccounts;
use crate::agent_accounts::limit_fixtures::*;
use crate::registry::{HarnessRegistry, RunEnvironment};
use crate::rpc::EngineRpc;

/// Where orchestrators the tests don't run live: nothing here runs them.
const REMOTE: &str = "dev-remote";

struct Engine {
    _temp: tempfile::TempDir,
    core: crate::EngineCore,
    rpc: std::sync::Arc<EngineRpc>,
    accounts: AgentAccounts,
    /// Slot ids: A is the machine's login and limited, B has the most
    /// quota, C is full, D has some.
    a: String,
    b: String,
    c: String,
}

async fn engine() -> Engine {
    let temp = tempfile::tempdir().unwrap();
    let mut core = crate::EngineCore::assemble_with_profile(
        crate::EngineProfile::local(&temp.path().join("engine")).unwrap(),
        std::sync::Arc::new(HarnessRegistry::new()),
        HarnessId::Codex,
        None,
    )
    .unwrap();
    let root = temp.path().join("accounts");
    let accounts = accounts(&root);
    live_claude(&root, "acct-a", "a@example.com");
    let a = claude_slot(&accounts, "acct-a", "a@example.com", "live-token");
    let b = claude_slot(&accounts, "acct-b", "b@example.com", "at-b");
    let c = claude_slot(&accounts, "acct-c", "c@example.com", "at-c");
    claude_slot(&accounts, "acct-d", "d@example.com", "at-d");
    let hour = 3_600_000;
    usage(&accounts, HarnessId::ClaudeCode, "acct-a", vec![window(1.0, hour)], 0);
    usage(&accounts, HarnessId::ClaudeCode, "acct-b", vec![window(0.2, hour)], 0);
    usage(&accounts, HarnessId::ClaudeCode, "acct-c", vec![window(1.0, hour)], 0);
    usage(&accounts, HarnessId::ClaudeCode, "acct-d", vec![window(0.6, hour)], 0);
    core.agent_accounts = accounts.clone();
    let rpc = core.rpc_service();
    Engine { _temp: temp, core, rpc, accounts, a, b, c }
}

async fn call(core: &crate::EngineCore, method: &str, params: Value) -> Result<Value, String> {
    let rpc = core.rpc_service();
    match with_caller(Caller::Local, rpc.handle(method, params)).await {
        Ok(RpcReply::Value(value)) => Ok(value),
        Ok(RpcReply::Stream(mut stream)) => Ok(stream.next().await.unwrap_or(Value::Null)),
        Err(error) => Err(error.to_string()),
    }
}

async fn op(core: &crate::EngineCore, params: Value) -> String {
    call(core, methods::ORCHESTRATE, params).await.unwrap()["id"].as_str().unwrap_or_default().to_owned()
}

/// A Claude Code chat on `device`.
async fn chat(core: &crate::EngineCore, id: &str, device: &str) {
    call(
        core,
        methods::MUTATE,
        json!({"op":"createChat","chatId":id,"deviceId":device,"config":{"harness":"claude-code","sandbox":"workspace-write"}}),
    )
    .await
    .unwrap();
}

async fn set_policy(core: &crate::EngineCore, orchestrator: &str, policy: LimitPolicy) {
    op(core, json!({"op":"updateOrchestrator","id":orchestrator,"crew":{"onLimit":policy}})).await;
}

/// An orchestrator hosted elsewhere, and a crew thread of it hosted here.
async fn crew(e: &Engine, policy: LimitPolicy) -> (String, &'static str) {
    let orchestrator = op(&e.core, json!({"op":"createOrchestrator","name":"Auth","goal":"g","deviceId":REMOTE})).await;
    chat(&e.core, "worker", &e.core.device_id).await;
    op(&e.core, json!({"op":"addToCrew","orchestratorId":orchestrator,"chatId":"worker"})).await;
    set_policy(&e.core, &orchestrator, policy).await;
    (orchestrator, "worker")
}

fn limit(e: &Engine, until_in: Duration) -> SessionLimit {
    SessionLimit {
        provider: "claude".into(),
        account_slot: Some(e.a.clone()),
        account_label: Some("a@example.com".into()),
        window: Some("5-hour".into()),
        until: Some(Utc::now() + until_in),
        since: Utc::now(),
        attempt: 0,
        switched_to: None,
    }
}

const ON: LimitPolicy = LimitPolicy { switch_account: true, switch_harness: false };

/// What the engine sent `chat` (runs, steers and queued rows), with what it
/// said each one is.
fn sent(core: &crate::EngineCore, chat: &str) -> Vec<(String, Option<zeron_doc::Injected>)> {
    let handle = core.doc_host.open(chat).unwrap();
    let commands = handle.doc().read_commands().unwrap().into_iter().filter_map(|c| match c.payload {
        SessionCommandPayload::Run { request, injected, .. } => Some((request.prompt, injected)),
        SessionCommandPayload::Steer { prompt, injected, .. } => Some((prompt, injected)),
        _ => None,
    });
    let rows = handle.doc().read_queue().unwrap().into_iter().map(|r| (r.text, r.injected));
    commands.chain(rows).collect()
}

async fn token(accounts: &AgentAccounts, chat: &str) -> Option<String> {
    let mut request: zeron_proto::RunRequest =
        serde_json::from_str(r#"{"prompt":"p","model":null,"reasoning":null,"cwd":".","sandbox":"workspace-write","resume":null}"#)
            .unwrap();
    run_environment(accounts).prepare(HarnessId::ClaudeCode, Some(chat), &mut request).await.unwrap();
    request.env.get("CLAUDE_CODE_OAUTH_TOKEN").cloned()
}

#[tokio::test]
async fn off_by_default_and_never_for_a_standalone_chat() {
    let e = engine().await;
    let (orchestrator, worker) = crew(&e, LimitPolicy::default()).await;
    assert_eq!(e.rpc.switch_account_for_limit(worker, &limit(&e, Duration::hours(1))).await, None, "off");
    chat(&e.core, "solo", &e.core.device_id).await;
    set_policy(&e.core, &orchestrator, ON).await;
    assert_eq!(e.rpc.switch_account_for_limit("solo", &limit(&e, Duration::hours(1))).await, None, "standalone");
    assert!(e.accounts.limit_switch("solo").is_none());
    assert_eq!(token(&e.accounts, "solo").await, None);
    assert!(sent(&e.core, "solo").is_empty());
}

#[tokio::test]
async fn a_crew_thread_follows_its_orchestrator_onto_the_account_with_the_most_quota() {
    let e = engine().await;
    let (_, worker) = crew(&e, ON).await;
    let switched = e.rpc.switch_account_for_limit(worker, &limit(&e, Duration::hours(1))).await.expect("switched");
    // Never A (limited), never C (full); B over D (more headroom).
    assert_eq!((switched.account_slot.as_str(), switched.account_label.as_str()), (e.b.as_str(), "b@example.com"));
    assert_eq!((switched.harness, switched.model), (None, None));
    // Its next run is on B; the machine's login stays A.
    assert_eq!(token(&e.accounts, worker).await.as_deref(), Some("at-b"));
    assert_eq!(e.rpc.limit_switch_of(worker).map(|s| s.label).as_deref(), Some("b@example.com"));
    let told = sent(&e.core, worker);
    let (text, injected) = told.last().expect("told to continue");
    assert_eq!(text, &switched_message("b@example.com", "a@example.com"));
    let injected = injected.as_ref().unwrap();
    assert_eq!((injected.kind.as_str(), injected.chat_id.as_deref()), (zeron_doc::injected::LIMIT_RESET, Some(worker)));
    let _ = &e.c;
}

#[tokio::test]
async fn an_orchestrator_follows_its_own_setting_and_waits_with_nothing_left() {
    let e = engine().await;
    chat(&e.core, "boss", &e.core.device_id).await;
    let orchestrator = op(&e.core, json!({"op":"promoteChat","chatId":"boss","name":"Boss"})).await;
    assert_eq!(orchestrator, "boss");
    assert_eq!(e.rpc.switch_account_for_limit("boss", &limit(&e, Duration::hours(1))).await, None, "off");
    set_policy(&e.core, "boss", ON).await;
    // B and D full too: nothing with quota, it waits.
    usage(&e.accounts, HarnessId::ClaudeCode, "acct-b", vec![window(1.0, 3_600_000)], 0);
    usage(&e.accounts, HarnessId::ClaudeCode, "acct-d", vec![window(0.99, 3_600_000), window(1.0, 600_000)], 0);
    assert_eq!(e.rpc.switch_account_for_limit("boss", &limit(&e, Duration::hours(1))).await, None);
    // Another harness is allowed but none is enabled here: still waits.
    set_policy(&e.core, "boss", LimitPolicy { switch_account: true, switch_harness: true }).await;
    assert_eq!(e.rpc.switch_account_for_limit("boss", &limit(&e, Duration::hours(1))).await, None);
    // D's window reset meanwhile: it goes there.
    usage(&e.accounts, HarnessId::ClaudeCode, "acct-d", vec![window(1.0, -1_000)], 0);
    let switched = e.rpc.switch_account_for_limit("boss", &limit(&e, Duration::hours(1))).await.unwrap();
    assert_eq!(switched.account_label, "d@example.com");
}

#[tokio::test]
async fn the_switch_back_waits_for_the_reset_and_a_turn_boundary() {
    let e = engine().await;
    let (_, worker) = crew(&e, ON).await;
    let limit = limit(&e, Duration::hours(1));
    e.rpc.switch_account_for_limit(worker, &limit).await.unwrap();
    // Before the reset: stays on B.
    assert!(!e.rpc.switch_back_after_reset(worker, &limit).await);
    assert_eq!(token(&e.accounts, worker).await.as_deref(), Some("at-b"));
    // Reset, mid-turn: stays.
    let reset = SessionLimit { until: Some(Utc::now() - Duration::minutes(1)), ..limit.clone() };
    e.core.sessions.set_status_for_test(worker, SessionStatus::Working);
    assert!(!e.rpc.switch_back_after_reset(worker, &reset).await);
    assert_eq!(token(&e.accounts, worker).await.as_deref(), Some("at-b"));
    // Unknown reset: stays.
    e.core.sessions.set_status_for_test(worker, SessionStatus::Idle);
    assert!(!e.rpc.switch_back_after_reset(worker, &SessionLimit { until: None, ..limit.clone() }).await);
    // Reset, between turns: back on the machine's login.
    assert!(e.rpc.switch_back_after_reset(worker, &reset).await);
    assert_eq!(token(&e.accounts, worker).await, None);
    assert!(e.accounts.limit_switch(worker).is_none());
    assert!(!e.rpc.switch_back_after_reset(worker, &reset).await, "once");
}

#[tokio::test]
async fn on_limit_rides_update_orchestrator_and_is_kept() {
    let e = engine().await;
    let orchestrator = op(&e.core, json!({"op":"createOrchestrator","name":"Auth","goal":"g","deviceId":REMOTE})).await;
    assert_eq!(e.core.orchestration.orchestrator(&orchestrator).unwrap().crew.on_limit, LimitPolicy::default());
    let both = LimitPolicy { switch_account: true, switch_harness: true };
    op(&e.core, json!({"op":"updateOrchestrator","id":orchestrator,"crew":{"maxRunning":2,"onLimit":{"switchAccount":true,"switchHarness":true}}})).await;
    let row = e.core.orchestration.orchestrator(&orchestrator).unwrap();
    assert_eq!((row.crew.on_limit, row.crew.max_running), (both, Some(2)));
    // As every client reads it.
    let state = call(&e.core, methods::WATCH_ORCHESTRATION, json!({})).await.unwrap();
    let listed = state["orchestrators"].as_array().unwrap().iter().find(|o| o["id"] == json!(orchestrator)).unwrap().clone();
    assert_eq!(listed["crew"]["onLimit"], json!({"switchAccount": true, "switchHarness": true}));
    let parsed: zeron_proto::orchestration::Orchestrator = serde_json::from_value(listed).unwrap();
    assert_eq!(parsed.crew.on_limit, both);
    // Other settings changed later keep it when they send it back.
    let mut crew = parsed.crew.clone();
    crew.max_running = Some(3);
    op(&e.core, json!({"op":"updateOrchestrator","id":orchestrator,"crew":crew})).await;
    assert_eq!(e.core.orchestration.orchestrator(&orchestrator).unwrap().crew.on_limit, both);
    // Off again.
    set_policy(&e.core, &orchestrator, LimitPolicy::default()).await;
    assert_eq!(e.core.orchestration.orchestrator(&orchestrator).unwrap().crew.on_limit, LimitPolicy::default());
}
