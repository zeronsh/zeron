//! A flow's run against a whole (local, edge-less) engine: its cards, its
//! steps' threads, routing on reports, returns and their limit, and a gate's
//! approval ask, all within the board's rules.

use serde_json::{Value, json};
use zeron_proto::orchestration::flow::{FlowRun, RunState};
use zeron_proto::orchestration::*;
use zeron_rpc::{Caller, RpcReply, RpcService, methods, with_caller};

use crate::registry::HarnessRegistry;

const REMOTE: &str = "dev-remote";

async fn core() -> (tempfile::TempDir, crate::EngineCore) {
    let temp = tempfile::tempdir().unwrap();
    let core = crate::EngineCore::assemble_with_profile(
        crate::EngineProfile::local(temp.path()).unwrap(),
        std::sync::Arc::new(HarnessRegistry::new()),
        zeron_proto::HarnessId::Codex,
        None,
    )
    .unwrap();
    (temp, core)
}

async fn op(core: &crate::EngineCore, caller: Caller, params: Value) -> Result<Value, String> {
    let rpc = core.rpc_service();
    match with_caller(caller, rpc.handle(methods::ORCHESTRATE, params)).await {
        Ok(RpcReply::Value(value)) => Ok(value),
        Ok(RpcReply::Stream(_)) => Ok(Value::Null),
        Err(error) => Err(error.to_string()),
    }
}

fn agent(chat: &str) -> Caller {
    Caller::Agent { chat: Some(chat.into()) }
}

/// Entry → Worker → QA; QA's fail back to the Worker (twice at most), its
/// pass to a gate, the gate's approval to the finish (its send-back to the
/// Worker, once at most).
fn flow() -> Value {
    json!({
        "version": 2, "name": "Build and ship", "next_id": 6,
        "nodes": [
            {"id":"1","kind":"chief","name":"Entry","x":0,"y":0},
            {"id":"2","kind":"worker","name":"Worker","x":0,"y":160,"instructions":"Build it","harness":"claude-code","model":"claude-opus-5-5"},
            {"id":"3","kind":"qa","name":"QA","x":0,"y":320,"instructions":"Try it"},
            {"id":"4","kind":"gate","name":"Ship?","x":0,"y":480,"instructions":"Ship it?"},
            {"id":"5","kind":"finish","name":"Finish","x":0,"y":640}
        ],
        "edges": [
            {"from":"1","to":"2","condition":"always"},
            {"from":"2","to":"3","condition":"always"},
            {"from":"3","to":"2","condition":"fail","max_retries":2},
            {"from":"3","to":"4","condition":"pass"},
            {"from":"4","to":"5","condition":"pass"},
            {"from":"4","to":"2","condition":"fail","max_retries":1}
        ]
    })
}

async fn with_flow(core: &crate::EngineCore) -> String {
    let id = op(core, Caller::Local, json!({"op":"createOrchestrator","name":"Ship","goal":"g","deviceId":REMOTE}))
        .await
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    op(core, Caller::Local, json!({"op":"updateOrchestrator","id":id,"workflow":flow()})).await.unwrap();
    id
}

fn run(core: &crate::EngineCore, o: &str) -> FlowRun {
    core.orchestration.orchestrator(o).unwrap().flow_run.unwrap()
}

fn card(core: &crate::EngineCore, o: &str, node: &str) -> WorkItem {
    let id = run(core, o).cards[node].clone();
    core.orchestration.item(&id).unwrap()
}

fn open_ask_on(core: &crate::EngineCore, card: &str) -> Option<Ask> {
    core.orchestration.state().asks.into_iter().find(|a| a.state == AskState::Open && a.item_id.as_deref() == Some(card))
}

async fn report(core: &crate::EngineCore, thread: &str, result: Option<&str>, summary: &str) {
    let mut params = json!({"op":"report","state":"done","summary":summary});
    if let Some(r) = result {
        params["result"] = json!(r);
    }
    op(core, agent(thread), params).await.unwrap();
}

#[tokio::test]
async fn a_flow_runs_as_cards_and_threads_step_by_step() {
    let (_temp, core) = core().await;
    let o = with_flow(&core).await;
    let started = op(&core, Caller::Local, json!({"op":"startFlow","orchestratorId":o,"task":"Add dark mode"})).await.unwrap();
    assert!(started["run"].is_string());

    // A card per step that gets one (not the entry or the finish), in order,
    // each waiting on the one before it.
    let r = run(&core, &o);
    assert_eq!(r.state, RunState::Running);
    assert_eq!(r.cards.len(), 3);
    let (worker, qa, gate) = (card(&core, &o, "2"), card(&core, &o, "3"), card(&core, &o, "4"));
    assert_eq!(worker.flow.as_ref().unwrap().index, 1);
    assert_eq!(qa.flow.as_ref().unwrap().step, "QA");
    assert_eq!(gate.flow.as_ref().unwrap().of, 3);
    assert_eq!(qa.depends_on, vec![worker.id.clone()]);
    assert_eq!(gate.depends_on, vec![qa.id.clone()]);

    // The first step started: its card in Now with a crew thread on the
    // step's own agent and model; the rest in Up next.
    assert_eq!(worker.state, WorkState::Active);
    let worker_thread = worker.threads[0].clone();
    let chat = core.workspace.chat(&worker_thread).unwrap().unwrap();
    assert_eq!(chat.config.as_ref().map(|c| c.harness), Some(zeron_proto::HarnessId::ClaudeCode));
    assert_eq!(chat.config.as_ref().and_then(|c| c.model.clone()).as_deref(), Some("claude-opus-5-5"));
    assert_eq!(core.orchestration.crew_of(&worker_thread).unwrap().item_id.as_deref(), Some(worker.id.as_str()));
    assert_eq!(core.orchestration.crew_of(&worker_thread).unwrap().role, "Worker");
    assert!(worker.detail.contains("Add dark mode") && worker.detail.contains("Build it"));
    assert_eq!(qa.state, WorkState::Planned);
    assert_eq!(r.at.as_deref(), Some("2"));

    // The worker's done: its card Done with what it said, QA started and
    // told what the worker did.
    report(&core, &worker_thread, None, "Built the toggle").await;
    assert_eq!(card(&core, &o, "2").state, WorkState::Done);
    assert_eq!(card(&core, &o, "2").outcome.as_deref(), Some("Built the toggle"));
    let qa = card(&core, &o, "3");
    assert_eq!(qa.state, WorkState::Active);
    assert!(qa.detail.contains("Worker: Built the toggle"));
    assert!(qa.detail.contains("`result`"));
    let qa_thread = qa.threads[0].clone();

    // QA fails it: back to the worker's own thread, told why, the return
    // counted.
    report(&core, &qa_thread, Some("fail"), "Toggle doesn't persist").await;
    assert_eq!(card(&core, &o, "2").state, WorkState::Active);
    assert_eq!(card(&core, &o, "2").threads, vec![worker_thread.clone()]);
    assert_eq!(run(&core, &o).returns["3>2"], 1);
    assert_eq!(run(&core, &o).at.as_deref(), Some("2"));
}

#[tokio::test]
async fn past_its_return_limit_the_card_waits_on_the_person() {
    let (_temp, core) = core().await;
    let o = with_flow(&core).await;
    op(&core, Caller::Local, json!({"op":"startFlow","orchestratorId":o,"task":"T"})).await.unwrap();
    let worker = card(&core, &o, "2").threads[0].clone();
    report(&core, &worker, None, "v1").await;
    let qa = card(&core, &o, "3").threads[0].clone();
    for i in 0..2 {
        report(&core, &qa, Some("fail"), &format!("no {i}")).await;
        report(&core, &worker, None, &format!("v{}", i + 2)).await;
    }
    // A third fail: past the limit of 2.
    report(&core, &qa, Some("fail"), "still no").await;
    let qa_card = card(&core, &o, "3");
    assert_eq!(qa_card.state, WorkState::Blocked);
    let ask = open_ask_on(&core, &qa_card.id).expect("a Blocked card has its ask");
    assert!(ask.question.contains("limit"));
    assert_eq!(run(&core, &o).state, RunState::Blocked);

    // "Once more": one more return.
    let once = ask.options.iter().position(|o| o.label == super::flow::ONCE_MORE).unwrap();
    op(&core, Caller::Local, json!({"op":"answerAsk","id":ask.id,"answer":"","option":once})).await.unwrap();
    assert_eq!(card(&core, &o, "2").state, WorkState::Active);
    assert_eq!(run(&core, &o).state, RunState::Running);
}

#[tokio::test]
async fn a_gate_asks_the_person_and_their_answer_leads_on() {
    let (_temp, core) = core().await;
    let o = with_flow(&core).await;
    op(&core, Caller::Local, json!({"op":"startFlow","orchestratorId":o,"task":"T"})).await.unwrap();
    let worker = card(&core, &o, "2").threads[0].clone();
    report(&core, &worker, None, "built").await;
    let qa = card(&core, &o, "3").threads[0].clone();
    report(&core, &qa, Some("pass"), "works").await;

    // The gate: its card Blocked with an approval ask (the board's rule).
    let gate = card(&core, &o, "4");
    assert_eq!(gate.state, WorkState::Blocked);
    assert!(gate.threads.is_empty());
    let ask = open_ask_on(&core, &gate.id).expect("the gate's ask");
    assert_eq!(ask.kind, AskKind::Approval);
    let labels: Vec<&str> = ask.options.iter().map(|o| o.label.as_str()).collect();
    assert_eq!(labels, [super::flow::APPROVE, super::flow::SEND_BACK]);
    assert_eq!(run(&core, &o).state, RunState::Blocked);

    // Approved: the gate's card Done, the run finished.
    op(&core, Caller::Local, json!({"op":"answerAsk","id":ask.id,"answer":"","option":0})).await.unwrap();
    assert_eq!(card(&core, &o, "4").state, WorkState::Done);
    let r = run(&core, &o);
    assert_eq!(r.state, RunState::Finished);
    assert!(r.ended_at.is_some() && r.at.is_none());
}

#[tokio::test]
async fn a_pass_fail_step_told_neither_asks_which() {
    let (_temp, core) = core().await;
    let o = with_flow(&core).await;
    op(&core, Caller::Local, json!({"op":"startFlow","orchestratorId":o,"task":"T"})).await.unwrap();
    let worker = card(&core, &o, "2").threads[0].clone();
    report(&core, &worker, None, "built").await;
    let qa = card(&core, &o, "3").threads[0].clone();
    report(&core, &qa, None, "looked at it").await;
    let qa_card = card(&core, &o, "3");
    assert_eq!(qa_card.state, WorkState::Blocked);
    let ask = open_ask_on(&core, &qa_card.id).unwrap();
    let pass = ask.options.iter().position(|o| o.label == super::flow::PASS).unwrap();
    op(&core, Caller::Local, json!({"op":"answerAsk","id":ask.id,"answer":"","option":pass})).await.unwrap();
    // On to the gate.
    assert_eq!(card(&core, &o, "4").state, WorkState::Blocked);
    assert_eq!(run(&core, &o).at.as_deref(), Some("4"));
}

#[tokio::test]
async fn only_the_person_runs_a_flow_and_a_broken_one_doesnt() {
    let (_temp, core) = core().await;
    let o = with_flow(&core).await;
    // An agent can't start or stop it, nor set it.
    assert!(op(&core, agent(&o), json!({"op":"startFlow","orchestratorId":o,"task":"T"})).await.is_err());
    assert!(op(&core, agent(&o), json!({"op":"updateOrchestrator","id":o,"workflow":flow()})).await.is_err());
    // A loop back with no limit doesn't run.
    let mut broken = flow();
    broken["edges"][2]["max_retries"] = Value::Null;
    op(&core, Caller::Local, json!({"op":"updateOrchestrator","id":o,"workflow":broken})).await.unwrap();
    let err = op(&core, Caller::Local, json!({"op":"startFlow","orchestratorId":o,"task":"T"})).await.unwrap_err();
    assert!(err.contains("can't run"), "{err}");
    assert!(core.orchestration.state().items.iter().all(|i| i.flow.is_none()));
    // Stopped, nothing after.
    op(&core, Caller::Local, json!({"op":"updateOrchestrator","id":o,"workflow":flow()})).await.unwrap();
    op(&core, Caller::Local, json!({"op":"startFlow","orchestratorId":o,"task":"T"})).await.unwrap();
    op(&core, Caller::Local, json!({"op":"stopFlow","orchestratorId":o})).await.unwrap();
    assert_eq!(run(&core, &o).state, RunState::Stopped);
    let worker = card(&core, &o, "2").threads[0].clone();
    report(&core, &worker, None, "built").await;
    assert_eq!(card(&core, &o, "3").state, WorkState::Planned);
}

#[tokio::test]
async fn a_flows_cards_keep_the_boards_rules() {
    let (_temp, core) = core().await;
    let o = with_flow(&core).await;
    op(&core, Caller::Local, json!({"op":"startFlow","orchestratorId":o,"task":"T"})).await.unwrap();
    let worker = card(&core, &o, "2").threads[0].clone();
    report(&core, &worker, None, "built").await;
    let qa = card(&core, &o, "3").threads[0].clone();
    report(&core, &qa, Some("pass"), "works").await;
    // Every Blocked card has its open ask; the check finds nothing to move
    // on the gate (its ask is open).
    let state = core.orchestration.state();
    for c in state.items.iter().filter(|c| c.flow.is_some() && c.state == WorkState::Blocked) {
        assert!(open_ask_on(&core, &c.id).is_some(), "{} is Blocked with no ask", c.title);
    }
    let rpc = core.rpc_service();
    let changes = rpc.plan_board_checks(chrono::Utc::now(), true);
    let gate = card(&core, &o, "4").id;
    assert!(changes.iter().all(|c| c.item != gate), "the board check moved the gate's card");
}
