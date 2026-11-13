//! Crew reports to an orchestrator: filed as one "Crew updates" message per
//! stretch between its turns (never a user turn of their own), and a wake
//! that waits for more reports to join it, or for the turn it's in.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;
use serde_json::{Value, json};

use zeron_doc::{MessagePart, MessageRole, SessionMessageEntry};
use zeron_engine::{EngineCore, HarnessRegistry};
use zeron_harness::{Harness, HarnessError, RunControls};
use zeron_proto::orchestration::ReportState;
use zeron_proto::{AgentEvent, DoneStatus, HarnessId, Model, ReasoningLevel, RunRequest, SteeringMode};
use zeron_rpc::{Caller, RpcReply, RpcService, methods, with_caller};

const ORCH: &str = "orch-chat";
const CREW: &str = "crew-chat";
const DEBOUNCE: Duration = Duration::from_millis(400);

/// Each turn lasts until the test ends it.
struct HeldHarness {
    finish: tokio::sync::broadcast::Sender<()>,
    prompts: Arc<Mutex<Vec<String>>>,
}

#[async_trait]
impl Harness for HeldHarness {
    fn id(&self) -> HarnessId {
        HarnessId::Mock
    }
    fn display_name(&self) -> &str {
        "Held"
    }
    fn supports_steering(&self) -> bool {
        false
    }
    fn steering_mode(&self) -> SteeringMode {
        SteeringMode::TurnBoundary
    }
    fn reasoning_levels(&self) -> &[ReasoningLevel] {
        &[ReasoningLevel::Medium]
    }
    async fn models(&self) -> Result<Vec<Model>, HarnessError> {
        Ok(vec![])
    }
    async fn run(
        &self,
        request: RunRequest,
        _controls: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        self.prompts.lock().unwrap().push(request.prompt.clone());
        let mut finish = self.finish.subscribe();
        let started = futures::stream::iter(vec![Ok(AgentEvent::SessionStarted {
            harness: HarnessId::Mock,
            model: "mock-1".into(),
            tools: vec![],
            cwd: request.cwd.clone(),
            session_id: "sess-orch".into(),
            assistant_message_id: uuid::Uuid::new_v4().to_string(),
        }), Ok(AgentEvent::TextDelta { text: "On it.".into() })]);
        let done = futures::stream::once(async move {
            let _ = finish.recv().await;
            Ok(AgentEvent::Done {
                status: DoneStatus::Completed,
                result: None,
                error: None,
                session_id: Some("sess-orch".into()),
            })
        });
        Ok(started.chain(done).boxed())
    }
}

struct Setup {
    core: EngineCore,
    finish: tokio::sync::broadcast::Sender<()>,
    prompts: Arc<Mutex<Vec<String>>>,
}

async fn wait_for<F: FnMut() -> bool>(mut predicate: F, what: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !predicate() {
        assert!(tokio::time::Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(15)).await;
    }
}

async fn call(core: &EngineCore, caller: Caller, method: &str, params: Value) -> Value {
    let rpc = core.rpc_service();
    match with_caller(caller, rpc.handle(method, params)).await.expect(method) {
        RpcReply::Value(value) => value,
        RpcReply::Stream(_) => panic!("{method} streamed"),
    }
}

async fn setup() -> Setup {
    let path = tempfile::tempdir().unwrap().keep();
    let (finish, _) = tokio::sync::broadcast::channel(16);
    let prompts = Arc::new(Mutex::new(Vec::new()));
    let registry = HarnessRegistry::new();
    registry.register(Arc::new(HeldHarness { finish: finish.clone(), prompts: prompts.clone() }));
    let core = EngineCore::assemble(&path.join("data"), Arc::new(registry), HarnessId::Mock, None)
        .expect("engine core assembles");
    core.sessions.set_no_ipc_port();
    core.doc_host.set_crew_wake_debounce(DEBOUNCE);
    for chat in [ORCH, CREW] {
        call(
            &core,
            Caller::Local,
            methods::MUTATE,
            json!({ "op": "createChat", "chatId": chat, "deviceId": core.device_id,
                "config": { "harness": "mock", "sandbox": "workspace-write", "access": "full-access" } }),
        )
        .await;
        core.workspace.rename_chat(chat, chat).unwrap();
    }
    call(&core, Caller::Local, methods::ORCHESTRATE, json!({ "op": "promoteChat", "chatId": ORCH, "name": "Ops" })).await;
    call(
        &core,
        Caller::Local,
        methods::ORCHESTRATE,
        json!({ "op": "addToCrew", "orchestratorId": ORCH, "chatId": CREW, "role": "tests" }),
    )
    .await;
    Setup { core, finish, prompts }
}

async fn report(core: &EngineCore, state: &str, summary: &str) -> String {
    let reply = call(
        core,
        Caller::Agent { chat: Some(CREW.into()) },
        methods::ORCHESTRATE,
        json!({ "op": "report", "state": state, "summary": summary }),
    )
    .await;
    reply["id"].as_str().unwrap().to_owned()
}

fn entries(core: &EngineCore) -> Vec<SessionMessageEntry> {
    core.doc_host.open(ORCH).unwrap().doc().read_entries().unwrap()
}

/// The reports each Crew updates message shows, in transcript order.
fn crew_messages(core: &EngineCore) -> Vec<Vec<String>> {
    entries(core)
        .iter()
        .filter_map(|e| {
            e.parts.iter().find_map(|p| match p {
                MessagePart::CrewReports { reports, .. } => {
                    Some(reports.iter().map(|r| r.summary.clone()).collect())
                }
                _ => None,
            })
        })
        .collect()
}

fn user_texts(core: &EngineCore) -> Vec<String> {
    entries(core)
        .iter()
        .filter(|e| e.role == MessageRole::User)
        .filter(|e| !e.parts.iter().any(|p| matches!(p, MessagePart::CrewReports { .. })))
        .flat_map(|e| {
            e.parts.iter().filter_map(|p| match p {
                MessagePart::Text { text, .. } => Some(text.clone()),
                _ => None,
            })
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reports_coalesce_into_one_message_and_one_wake_and_progress_never_wakes() {
    let Setup { core, finish, prompts } = setup().await;
    report(&core, "progress", "halfway").await;
    wait_for(|| crew_messages(&core) == vec![vec!["halfway".to_owned()]], "the progress report filed").await;
    tokio::time::sleep(DEBOUNCE * 3).await;
    assert!(prompts.lock().unwrap().is_empty(), "progress never wakes");

    report(&core, "done", "tests pass").await;
    report(&core, "blocked", "needs a token").await;
    wait_for(|| prompts.lock().unwrap().len() == 1, "one wake").await;
    let prompt = prompts.lock().unwrap()[0].clone();
    for summary in ["halfway", "tests pass", "needs a token"] {
        assert!(prompt.contains(summary), "the wake carries every unread report: {prompt}");
    }
    assert!(prompt.contains(&format!("chat {CREW}")), "{prompt}");
    // One message, extended in place, and it IS the wake's transcript entry.
    assert_eq!(
        crew_messages(&core),
        vec![vec!["halfway".to_owned(), "tests pass".to_owned(), "needs a token".to_owned()]]
    );
    assert!(user_texts(&core).is_empty(), "no user turn: {:?}", user_texts(&core));
    tokio::time::sleep(DEBOUNCE * 2).await;
    assert_eq!(prompts.lock().unwrap().len(), 1, "exactly one wake");
    // The crew row keeps the newest report.
    let member = core.workspace.read_orchestration_with(|d| d.crew_member(CREW)).unwrap();
    assert_eq!(member.last_report.unwrap().state, ReportState::Blocked);

    // Reports after the wake started open a new message.
    let _ = finish.send(());
    wait_for(|| !core.sessions.turn_in_flight(ORCH), "the wake turn to end").await;
    report(&core, "done", "merged").await;
    wait_for(|| crew_messages(&core).len() == 2, "a second Crew updates message").await;
    wait_for(|| prompts.lock().unwrap().len() == 2, "the second wake").await;
    let _ = finish.send(());
    core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_working_orchestrator_hears_reports_when_its_turn_ends() {
    let Setup { core, finish, prompts } = setup().await;
    core.doc_host.queue_message(ORCH, "plan the release", Vec::new()).unwrap();
    wait_for(|| prompts.lock().unwrap().len() == 1, "the person's turn").await;
    wait_for(|| core.sessions.turn_in_flight(ORCH), "the turn to be in flight").await;
    report(&core, "done", "shipped").await;
    tokio::time::sleep(DEBOUNCE * 3).await;
    assert_eq!(prompts.lock().unwrap().len(), 1, "no wake inside a turn");
    assert!(crew_messages(&core).is_empty(), "nothing filed inside a streaming turn");

    let _ = finish.send(());
    wait_for(|| prompts.lock().unwrap().len() == 2, "the wake after the turn").await;
    assert!(prompts.lock().unwrap()[1].contains("shipped"));
    assert_eq!(crew_messages(&core), vec![vec!["shipped".to_owned()]]);
    // Filed after the turn's reply, not inside it.
    let roles: Vec<_> = entries(&core).iter().map(|e| e.role).collect();
    let crew_at = entries(&core)
        .iter()
        .position(|e| e.parts.iter().any(|p| matches!(p, MessagePart::CrewReports { .. })))
        .unwrap();
    assert!(roles[..crew_at].contains(&MessageRole::Assistant), "{roles:?}");
    let _ = finish.send(());
    core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_crew_threads_message_to_its_orchestrator_is_a_report() {
    let Setup { core, finish, prompts } = setup().await;
    let text = zeron_proto::agent_messages::agent_message("crew-chat (crew-cha)", CREW, "All green on main\nRan 412 tests.");
    let reply = call(
        &core,
        Caller::Agent { chat: Some(CREW.into()) },
        methods::QUEUE_MESSAGE,
        json!({ "chatId": ORCH, "text": text, "holdForTurnEnd": true }),
    )
    .await;
    assert!(reply["report"].is_string(), "{reply}");
    wait_for(|| prompts.lock().unwrap().len() == 1, "the wake").await;
    assert!(user_texts(&core).is_empty(), "never a user turn: {:?}", user_texts(&core));
    assert_eq!(crew_messages(&core), vec![vec!["All green on main".to_owned()]]);
    let member = core.workspace.read_orchestration_with(|d| d.crew_member(CREW)).unwrap();
    assert_eq!(member.last_report.unwrap().state, ReportState::Message);

    // A run command to it goes the same way; a send to anyone else is a send.
    let reply = call(
        &core,
        Caller::Agent { chat: Some(ORCH.into()) },
        methods::QUEUE_MESSAGE,
        json!({ "chatId": CREW, "text": "next: the docs", "holdForTurnEnd": true }),
    )
    .await;
    assert!(reply["report"].is_null(), "an orchestrator's send to its crew is unchanged: {reply}");
    let _ = finish.send(());
    let _ = finish.send(());
    core.shutdown().await;
}
