//! With Orchestrators off (a fresh install), nothing of orchestration runs
//! or is written, its RPCs are refused, and a chat is a plain chat; on, off
//! and on again it starts, stops and starts once more without a restart.

use std::time::Duration;

use serde_json::{Value, json};
use zeron_proto::HarnessId;
use zeron_rpc::{Caller, RpcReply, RpcService, methods, with_caller};

use super::switch::{CAPABILITIES, METHODS, OFF};
use crate::registry::HarnessRegistry;

/// Every registry kind orchestration writes.
const KINDS: &[&str] = &[
    zeron_doc::registry::KIND_ORCHESTRATORS,
    zeron_doc::registry::KIND_CREW,
    zeron_doc::registry::KIND_WORK_ITEMS,
    zeron_doc::registry::KIND_BOARD_CARDS,
    zeron_doc::registry::KIND_ASKS,
    zeron_doc::registry::KIND_DUMPS,
    "orchestrationSettings",
    zeron_doc::registry::KIND_PROGRESS,
    zeron_doc::registry::KIND_SCRATCHPAD,
    zeron_doc::registry::KIND_SCRATCHPAD_HISTORY,
];

fn core(dir: &std::path::Path) -> crate::EngineCore {
    crate::EngineCore::assemble_with_profile(
        crate::EngineProfile::local(dir).unwrap(),
        std::sync::Arc::new(HarnessRegistry::new()),
        HarnessId::Codex,
        None,
    )
    .unwrap()
}

async fn handle(core: &crate::EngineCore, caller: Caller, method: &str, params: Value) -> Result<Option<Value>, String> {
    match with_caller(caller, core.rpc_service().handle(method, params)).await {
        Ok(RpcReply::Value(value)) => Ok(Some(value)),
        Ok(RpcReply::Stream(_)) => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

fn versions(core: &crate::EngineCore) -> Vec<(&'static str, u64)> {
    KINDS.iter().map(|k| (*k, core.workspace.kind_version(k))).collect()
}

/// This device's advertised capabilities, as its row says.
fn advertised(core: &crate::EngineCore) -> Vec<String> {
    let me = core.workspace.device_id().to_owned();
    core.workspace.read_devices().unwrap().into_iter().find(|d| d.id == me).unwrap().capabilities
}

async fn until(what: &str, mut done: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !done() {
        assert!(tokio::time::Instant::now() < deadline, "timed out: {what}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

async fn chat(core: &crate::EngineCore, caller: Caller, id: &str, extra: Value) -> Result<(), String> {
    let mut params = json!({"op":"createChat","chatId":id,"deviceId":core.workspace.device_id(),
        "config":{"harness":"codex","sandbox":"workspace-write","access":"full-access"}});
    if let (Some(params), Some(extra)) = (params.as_object_mut(), extra.as_object()) {
        params.extend(extra.clone());
    }
    handle(core, caller, methods::MUTATE, params).await.map(drop)
}

#[tokio::test]
async fn off_no_task_runs_its_rpcs_are_refused_and_nothing_routes_here() {
    let temp = tempfile::tempdir().unwrap();
    let core = core(temp.path());
    let rpc = core.rpc_service();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!core.orchestration.is_on(), "off unless turned on");
    assert_eq!(core.orchestration.running_tasks(), 0, "no orchestration task after attach_rpc");
    for method in METHODS {
        for caller in [Caller::Local, Caller::Agent { chat: Some("c1".into()) }] {
            let error = handle(&core, caller, method, json!({"op":"read"})).await.expect_err(method);
            assert!(error.contains(OFF), "{method}: {error}");
        }
    }
    // Other devices are told nothing routes here.
    let caps = advertised(&core);
    assert!(CAPABILITIES.iter().all(|c| !caps.iter().any(|a| a == c)), "{caps:?}");
    let info = handle(&core, Caller::Local, methods::ENGINE_INFO, json!({})).await.unwrap().unwrap();
    let info: Vec<String> = serde_json::from_value(info["capabilities"].clone()).unwrap_or_default();
    assert!(CAPABILITIES.iter().all(|c| !info.iter().any(|a| a == c)), "{info:?}");
    assert!(info.iter().any(|c| c == zeron_proto::capabilities::INJECTED_MESSAGES_V1), "the general ones stay");
    drop(rpc);
}

#[tokio::test]
async fn off_a_crew_chat_is_made_plain_and_no_orchestration_row_is_written() {
    let temp = tempfile::tempdir().unwrap();
    let core = core(temp.path());
    let _rpc = core.rpc_service();
    let before = versions(&core);
    chat(&core, Caller::Local, "lead", json!({})).await.unwrap();
    // Asked for as crew, by the person and by an agent: plain children.
    chat(&core, Caller::Local, "c1", json!({"parentChatId":"lead","childKind":"crew"})).await.unwrap();
    let agent = Caller::Agent { chat: Some("lead".into()) };
    chat(&core, agent.clone(), "c2", json!({"parentChatId":"lead","childKind":"crew"})).await.unwrap();
    for id in ["c1", "c2"] {
        let row = core.workspace.chat(id).unwrap().unwrap();
        assert_eq!(row.parent_chat_id.as_deref(), Some("lead"), "{id}");
        assert_eq!(row.child_kind, None, "{id}: a plain child");
    }
    // An agent's message to another chat goes as sent (no comms, no card).
    handle(&core, agent, methods::QUEUE_MESSAGE, json!({"chatId":"c1","text":"hello"})).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(versions(&core), before, "no orchestration row written");
    assert_eq!(core.orchestration.running_tasks(), 0);
}

#[tokio::test]
async fn on_off_on_starts_its_loops_stops_them_and_starts_them_once_again() {
    let temp = tempfile::tempdir().unwrap();
    let core = core(temp.path());
    let _rpc = core.rpc_service();
    core.set_orchestrators(true).await;
    until("loops started", || core.orchestration.running_tasks() > 0).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    let running = core.orchestration.running_tasks();
    let caps = advertised(&core);
    assert!(CAPABILITIES.iter().all(|c| caps.iter().any(|a| a == c)), "advertised while on: {caps:?}");
    let made = handle(&core, Caller::Local, methods::ORCHESTRATE, json!({"op":"createOrchestrator","name":"Ops","goal":"g"}))
        .await
        .unwrap()
        .unwrap();
    let id = made["id"].as_str().unwrap().to_owned();
    // A window watching it sees its stream end when it goes off.
    let stream = match with_caller(Caller::Local, core.rpc_service().handle(methods::WATCH_ORCHESTRATION, json!({}))).await {
        Ok(RpcReply::Stream(stream)) => stream,
        _ => panic!("a stream"),
    };

    core.set_orchestrators(false).await;
    until("loops stopped", || core.orchestration.running_tasks() == 0).await;
    let ended = tokio::time::timeout(Duration::from_secs(5), futures::StreamExt::collect::<Vec<_>>(stream)).await;
    assert!(ended.is_ok(), "the watch ends when it goes off");
    let error = handle(&core, Caller::Local, methods::ORCHESTRATE, json!({"op":"createOrchestrator","name":"No","goal":"g"}))
        .await
        .unwrap_err();
    assert!(error.contains(OFF), "{error}");
    let caps = advertised(&core);
    assert!(CAPABILITIES.iter().all(|c| !caps.iter().any(|a| a == c)), "{caps:?}");
    assert!(core.orchestration.orchestrator(&id).is_some(), "its rows are kept");

    core.set_orchestrators(true).await;
    until("loops started again", || core.orchestration.running_tasks() > 0).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    // (The first run has a few more short-lived tasks of its start; none
    // runs twice.)
    let again = core.orchestration.running_tasks();
    assert!(again <= running, "the same loops, none twice: {again} > {running}");
    core.set_orchestrators(false).await;
    until("loops stopped again", || core.orchestration.running_tasks() == 0).await;
    core.set_orchestrators(true).await;
    until("loops started a third time", || core.orchestration.running_tasks() > 0).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(core.orchestration.running_tasks(), again, "a run's loops, once each");
    assert!(core.orchestration.orchestrator(&id).is_some());
    handle(&core, Caller::Local, methods::ORCHESTRATE, json!({"op":"createOrchestrator","name":"Two","goal":"g"}))
        .await
        .unwrap();
}

/// A working chat's session row is as main writes it while Orchestrators
/// is off: no live activity or todo (`WatchSessions` sends the same).
#[tokio::test]
async fn off_a_working_chats_row_has_no_activity_or_todo() {
    use zeron_proto::{AgentEvent, ToolCall, TodoItem, TodoStatus};

    use crate::sessions::tests::{settled, wake_rig};
    let rig = wake_rig("chat-live-off", false).await;
    rig.feed
        .send(AgentEvent::ToolCall { id: "t1".into(), call: ToolCall::Exec { command: "cargo test".into() } })
        .unwrap();
    rig.feed
        .send(AgentEvent::ToolCall {
            id: zeron_proto::LIVE_PLAN_TOOL_ID.into(),
            call: ToolCall::Todo { items: vec![TodoItem::new("a", TodoStatus::InProgress)] },
        })
        .unwrap();
    let row = settled(&rig).await;
    assert_eq!(row.status, zeron_proto::SessionStatus::Working);
    assert!(row.activity.is_none() && row.todo.is_none(), "{row:?}");
    let sent = serde_json::to_value(&*rig.core.sessions.watch_sessions().borrow()).unwrap();
    let text = sent.to_string();
    assert!(!text.contains("\"activity\"") && !text.contains("\"todo\""), "{text}");

    // On, the same chat's next tool is on its row.
    rig.core.set_orchestrators(true).await;
    rig.feed
        .send(AgentEvent::ToolCall { id: "t2".into(), call: ToolCall::ReadFile { path: "a.rs".into() } })
        .unwrap();
    let row = settled(&rig).await;
    assert!(row.activity.is_some(), "{row:?}");
    rig.core.sessions.shutdown().await;
}

/// The edge registry's row id rule (`edge/src/registry-core.ts:55`,
/// `ID_RE = /^[A-Za-z0-9_.:@\/-]{1,256}$/`): a row whose id fails it is
/// refused there and never syncs. Copied here; keep the two the same.
fn edge_takes(id: &str) -> bool {
    (1..=256).contains(&id.len())
        && id.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b':' | b'@' | b'/' | b'-'))
}

#[tokio::test]
async fn every_orchestration_row_id_the_engine_makes_is_one_the_edge_takes() {
    let chat = uuid::Uuid::new_v4().to_string();
    let message = uuid::Uuid::new_v4().to_string();
    let ids = [
        super::turn_asks::ask_id(&chat, &message),
        super::turn_asks::ask_id(&chat, &super::turn_asks::part_turn(&message, 3)),
        super::board::pr_ask_id(Some(&chat)),
        super::board::pr_ask_id(None),
        zeron_proto::orchestration::new_brain_dump_chat_id(),
        uuid::Uuid::new_v4().simple().to_string(),
        uuid::Uuid::new_v4().to_string(),
        zeron_doc::registry::SCRATCHPAD_ROW.to_owned(),
        "settings".to_owned(),
        "slot07".to_owned(),
    ];
    for id in &ids {
        assert!(edge_takes(id), "{id}");
    }
    // The parts of a compound decision as asks were raised before: refused
    // by the edge, still read here.
    assert!(!edge_takes(&super::turn_asks::ask_id(&chat, &format!("{message}#1"))));
    assert_eq!(super::turn_asks::turn_base(&format!("{message}#1")), message);
    assert_eq!(super::turn_asks::turn_base(&super::turn_asks::part_turn(&message, 1)), message);
    assert_eq!(super::turn_asks::turn_base(&message), message);

    // And the rows a working engine writes.
    let temp = tempfile::tempdir().unwrap();
    let core = super::on(core(temp.path())).await;
    let rpc = core.rpc_service();
    rpc.ensure_brain_dump().unwrap();
    let made = handle(&core, Caller::Local, methods::ORCHESTRATE, json!({"op":"createOrchestrator","name":"Ops","goal":"g"}))
        .await
        .unwrap()
        .unwrap();
    let orchestrator = made["id"].as_str().unwrap().to_owned();
    handle(&core, Caller::Local, methods::ORCHESTRATE, json!({"op":"upsertItem","item":{"title":"Ship","orchestratorId":orchestrator}}))
        .await
        .unwrap();
    let state = core.orchestration.state();
    let rows: Vec<String> = state
        .orchestrators
        .iter()
        .map(|o| o.id.clone())
        .chain(state.crew.iter().map(|m| m.chat_id.clone()))
        .chain(state.items.iter().map(|i| i.id.clone()))
        .chain(state.asks.iter().map(|a| a.id.clone()))
        .chain(state.dumps.iter().map(|d| d.id.clone()))
        .chain(state.cards.iter().map(|c| c.id.clone()))
        .collect();
    assert!(rows.len() >= 3, "{rows:?}");
    for id in &rows {
        assert!(edge_takes(id), "{id}");
    }
}
