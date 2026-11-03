//! Waiting without stopping: questions to the person and watches on other
//! chats' turns end in a message delivered into the agent's own chat,
//! steered into a running turn or starting a new one when it is idle.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;
use serde_json::{Value, json};

use zeron_engine::{EngineCore, HarnessRegistry};
use zeron_harness::{Harness, HarnessError, RunControls};
use zeron_proto::{
    AgentEvent, DoneStatus, HarnessId, Model, ReasoningLevel, RunRequest, SteeringMode,
};
use zeron_rpc::{Caller, RpcClient, methods, with_caller};

const ASKER: &str = "chat-asker";
const TARGET: &str = "chat-target";

/// Runs whose prompt starts with "quick" finish at once; every other run
/// holds until `finish` fires. Steers are recorded as prompts too.
struct Held {
    finish: tokio::sync::broadcast::Sender<()>,
    prompts: Arc<Mutex<Vec<String>>>,
    runs: Arc<Mutex<Vec<String>>>,
}

impl Held {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            finish: tokio::sync::broadcast::channel(16).0,
            prompts: Arc::default(),
            runs: Arc::default(),
        })
    }

    fn prompts(&self) -> Vec<String> {
        self.prompts.lock().unwrap().clone()
    }

    fn runs(&self) -> Vec<String> {
        self.runs.lock().unwrap().clone()
    }
}

fn done() -> AgentEvent {
    AgentEvent::Done {
        status: DoneStatus::Completed,
        result: None,
        error: None,
        session_id: Some("sess".into()),
    }
}

#[async_trait]
impl Harness for Held {
    fn id(&self) -> HarnessId {
        HarnessId::Mock
    }
    fn display_name(&self) -> &str {
        "Held"
    }
    fn supports_steering(&self) -> bool {
        true
    }
    fn steering_mode(&self) -> SteeringMode {
        SteeringMode::StepBoundary
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
        controls: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        self.prompts.lock().unwrap().push(request.prompt.clone());
        self.runs.lock().unwrap().push(request.prompt.clone());
        let quick = request.prompt.starts_with("quick");
        let started = futures::stream::iter(vec![
            Ok(AgentEvent::SessionStarted {
                harness: HarnessId::Mock,
                model: "mock-1".into(),
                tools: vec![],
                cwd: request.cwd.clone(),
                session_id: "sess".into(),
                assistant_message_id: uuid::Uuid::new_v4().to_string(),
            }),
            Ok(AgentEvent::TextDelta {
                text: format!(
                    "did {}",
                    request.prompt.chars().take(20).collect::<String>()
                ),
            }),
        ]);
        if quick {
            return Ok(started
                .chain(futures::stream::iter(vec![Ok(done())]))
                .boxed());
        }
        let finish = self.finish.subscribe();
        let steering = controls.steering;
        let prompts = self.prompts.clone();
        let rest = futures::stream::unfold(
            (finish, steering, prompts, false),
            |(mut finish, mut steering, prompts, ended)| async move {
                if ended {
                    return None;
                }
                let (event, ended) = tokio::select! {
                    _ = finish.recv() => (done(), true),
                    steer = steering.recv() => match steer {
                        Some(message) => {
                            prompts.lock().unwrap().push(message.prompt);
                            (AgentEvent::Steered { assistant_message_id: None, next_assistant_message_id: Some(uuid::Uuid::new_v4().to_string()) }, false)
                        }
                        None => (done(), true),
                    }
                };
                Some((Ok(event), (finish, steering, prompts, ended)))
            },
        );
        Ok(started.chain(rest).boxed())
    }
}

async fn wait_for<F: FnMut() -> bool>(mut predicate: F, what: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    while !predicate() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn assemble(path: &std::path::Path, harness: Arc<Held>) -> EngineCore {
    let registry = HarnessRegistry::new();
    registry.register(harness);
    EngineCore::assemble(path, Arc::new(registry), HarnessId::Mock, None).expect("engine assembles")
}

async fn setup() -> (EngineCore, Arc<Held>, RpcClient, std::path::PathBuf) {
    let path = tempfile::tempdir().unwrap().keep().join("data");
    let harness = Held::new();
    let core = assemble(&path, harness.clone());
    let client = zeron_rpc::memory_client(core.rpc_service());
    for chat in [ASKER, TARGET] {
        client
            .call(
                methods::MUTATE,
                json!({ "op": "createChat", "chatId": chat, "deviceId": core.device_id }),
            )
            .await
            .expect("createChat");
        // Pre-titled, so the titler never runs a harness request of its own.
        core.workspace.rename_chat(chat, chat).unwrap();
    }
    (core, harness, client, path)
}

async fn ask(client: &RpcClient, blocking: bool) -> Value {
    client
        .call(
            methods::ASK_QUESTION,
            json!({ "chatId": ASKER, "question": "Which branch?", "options": ["main", "dev"], "blocking": blocking }),
        )
        .await
        .expect("AskQuestion")
}

fn question(core: &EngineCore, id: &str) -> zeron_proto::AgentQuestion {
    core.workspace.question(id).expect("question row")
}

/// Send `prompt` to `chat` the way the composer does, and wait for its run.
async fn start_turn(core: &EngineCore, harness: &Held, chat: &str, prompt: &str) {
    core.doc_host
        .queue_message(chat, prompt, Vec::new())
        .unwrap();
    wait_for(
        || harness.runs().iter().any(|p| p == prompt),
        "the turn to start",
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_answer_starts_a_turn_in_an_idle_chat() {
    let (core, harness, client, _) = setup().await;
    let asked = ask(&client, false).await;
    assert_eq!(asked["status"], "pending");
    assert_eq!(asked["blocking"], false);
    // An open question never makes the chat look like it's waiting on you.
    let id = asked["id"].as_str().unwrap();
    let answered = client
        .call(
            methods::ANSWER_QUESTION,
            json!({ "questionId": id, "answer": " main " }),
        )
        .await
        .unwrap();
    assert_eq!(answered["status"], "answered");
    assert_eq!(answered["answer"], "main");
    let expected = format!("Answer to your question «Which branch?» (question {id}): main");
    wait_for(
        || harness.runs().contains(&expected),
        "the answer to start a turn",
    )
    .await;
    assert!(question(&core, id).delivered_at.is_some());
    // Once answered, it stays answered.
    let again = client
        .call(methods::DISMISS_QUESTION, json!({ "questionId": id }))
        .await
        .unwrap_err();
    assert!(again.to_string().contains("already answered"), "{again}");
    let _ = harness.finish.send(());
    core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_answer_steers_a_running_turn() {
    let (core, harness, client, _) = setup().await;
    start_turn(&core, &harness, ASKER, "work").await;
    let asked = ask(&client, false).await;
    let id = asked["id"].as_str().unwrap();
    client
        .call(
            methods::ANSWER_QUESTION,
            json!({ "questionId": id, "answer": "dev" }),
        )
        .await
        .unwrap();
    wait_for(
        || {
            harness
                .prompts()
                .iter()
                .any(|p| p.starts_with("Answer to your question") && p.ends_with(": dev"))
        },
        "the answer to be steered in",
    )
    .await;
    assert_eq!(
        harness.runs(),
        vec!["work".to_owned()],
        "steered, not a new turn"
    );
    let _ = harness.finish.send(());
    core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_blocking_question_is_delivered_only_once_released() {
    let (core, harness, client, _) = setup().await;
    let asked = ask(&client, true).await;
    let id = asked["id"].as_str().unwrap();
    let answered = client
        .call(
            methods::ANSWER_QUESTION,
            json!({ "questionId": id, "answer": "main" }),
        )
        .await
        .unwrap();
    assert_eq!(answered["status"], "answered");
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(
        harness.runs().is_empty(),
        "the waiting tool call returns it instead"
    );
    assert!(question(&core, id).delivered_at.is_none());
    // The tool call ended without it: deliver it now, once.
    let released = client
        .call(
            methods::RELEASE_QUESTION,
            json!({ "questionId": id, "deliver": true }),
        )
        .await
        .unwrap();
    assert_eq!(released["blocking"], false);
    assert!(released["deliveredAt"].is_string());
    wait_for(
        || harness.runs().len() == 1,
        "the released answer to arrive",
    )
    .await;
    client
        .call(
            methods::RELEASE_QUESTION,
            json!({ "questionId": id, "deliver": true }),
        )
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(harness.runs().len(), 1, "delivered once");
    let _ = harness.finish.send(());
    core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dismissing_tells_the_agent_and_cancelling_does_not() {
    let (core, harness, client, _) = setup().await;
    let first = ask(&client, false).await;
    let second = ask(&client, false).await;
    client
        .call(
            methods::CANCEL_QUESTION,
            json!({ "questionId": second["id"] }),
        )
        .await
        .unwrap();
    client
        .call(
            methods::DISMISS_QUESTION,
            json!({ "questionId": first["id"] }),
        )
        .await
        .unwrap();
    wait_for(
        || {
            harness
                .runs()
                .iter()
                .any(|p| p.starts_with("The person dismissed your question"))
        },
        "the dismissal to arrive",
    )
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(harness.runs().len(), 1, "a withdrawn question says nothing");
    let statuses: Vec<_> = core
        .workspace
        .read_questions()
        .into_iter()
        .map(|q| q.status)
        .collect();
    assert_eq!(
        statuses,
        vec![
            zeron_proto::QuestionStatus::Dismissed,
            zeron_proto::QuestionStatus::Cancelled
        ]
    );
    let _ = harness.finish.send(());
    core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_agent_asks_and_watches_only_for_its_own_chat() {
    let (core, harness, client, _) = setup().await;
    let service = core.rpc_service();
    use zeron_rpc::RpcService as _;
    let asked = with_caller(
        Caller::Agent {
            chat: Some(ASKER.into()),
        },
        async {
            service
                .handle(
                    methods::ASK_QUESTION,
                    json!({ "chatId": TARGET, "question": "Ship?" }),
                )
                .await
        },
    )
    .await
    .unwrap();
    let asked = match asked {
        zeron_rpc::RpcReply::Value(v) => v,
        _ => panic!("a value"),
    };
    assert_eq!(
        asked["chatId"], ASKER,
        "the caller's chat, whatever the params say"
    );
    // Another chat can't withdraw it.
    let refused = with_caller(
        Caller::Agent {
            chat: Some(TARGET.into()),
        },
        async {
            service
                .handle(
                    methods::CANCEL_QUESTION,
                    json!({ "questionId": asked["id"] }),
                )
                .await
        },
    )
    .await;
    assert!(refused.is_err());
    // An agent with no chat has nowhere to hear back.
    let nowhere = with_caller(Caller::Agent { chat: None }, async {
        service
            .handle(methods::NOTIFY_TURN_END, json!({ "chatId": TARGET }))
            .await
    })
    .await;
    assert!(nowhere.is_err());
    // WatchQuestions narrows to one chat.
    let mut sub = client
        .subscribe(methods::WATCH_QUESTIONS, json!({ "chatId": TARGET }))
        .await
        .unwrap();
    assert_eq!(sub.recv().await.unwrap(), json!([]));
    let _ = harness.finish.send(());
    core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn questions_survive_a_restart() {
    let (core, harness, client, path) = setup().await;
    let asked = ask(&client, false).await;
    core.shutdown().await;
    drop(core);
    let core = assemble(&path, harness.clone());
    let client = zeron_rpc::memory_client(core.rpc_service());
    let mut sub = client
        .subscribe(methods::WATCH_QUESTIONS, json!({}))
        .await
        .unwrap();
    let all = sub.recv().await.unwrap();
    assert_eq!(all[0]["id"], asked["id"]);
    assert_eq!(all[0]["status"], "pending");
    assert_eq!(all[0]["options"], json!(["main", "dev"]));
    core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_finished_turn_starts_a_turn_in_an_idle_watcher() {
    let (core, harness, client, _) = setup().await;
    client
        .call(
            methods::NOTIFY_TURN_END,
            json!({ "chatId": TARGET, "watcherChatId": ASKER, "baseline": null, "expectTurn": true }),
        )
        .await
        .unwrap();
    start_turn(&core, &harness, TARGET, "quick job").await;
    wait_for(
        || {
            harness
                .runs()
                .iter()
                .any(|p| p.contains("finished its turn"))
        },
        "the watcher to hear the turn ended",
    )
    .await;
    let note = harness
        .runs()
        .into_iter()
        .find(|p| p.contains("finished its turn"))
        .unwrap();
    assert!(note.contains("Chat «chat-target"), "{note}");
    assert!(note.contains("did quick job"), "the reply: {note}");
    assert!(
        core.agent_waits.turn_watches().is_empty(),
        "settled watches go"
    );
    let _ = harness.finish.send(());
    core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_finished_turn_steers_a_busy_watcher_and_each_watch_notifies() {
    let (core, harness, client, _) = setup().await;
    start_turn(&core, &harness, ASKER, "busy").await;
    for chat in [TARGET] {
        client
            .call(
                methods::NOTIFY_TURN_END,
                json!({ "chatId": chat, "watcherChatId": ASKER, "expectTurn": true }),
            )
            .await
            .unwrap();
    }
    start_turn(&core, &harness, TARGET, "quick one").await;
    wait_for(
        || {
            harness
                .prompts()
                .iter()
                .any(|p| p.contains("finished its turn") && p.contains("did quick one"))
        },
        "the first notification",
    )
    .await;
    // A second watch on the same chat, for its next turn.
    client
        .call(
            methods::NOTIFY_TURN_END,
            json!({ "chatId": TARGET, "watcherChatId": ASKER, "expectTurn": true,
                    "baseline": core.workspace.read_sessions().unwrap().into_iter().find(|s| s.chat_id == TARGET) }),
        )
        .await
        .unwrap();
    start_turn(&core, &harness, TARGET, "quick two").await;
    wait_for(
        || {
            harness
                .prompts()
                .iter()
                .any(|p| p.contains("did quick two"))
        },
        "the second notification",
    )
    .await;
    let runs = harness.runs();
    assert_eq!(
        runs.iter().filter(|p| !p.starts_with("quick")).count(),
        1,
        "both steered into the running turn: {runs:?}"
    );
    let _ = harness.finish.send(());
    core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_watch_survives_a_restart() {
    let (core, harness, client, path) = setup().await;
    start_turn(&core, &harness, TARGET, "long").await;
    let session = core
        .workspace
        .read_sessions()
        .unwrap()
        .into_iter()
        .find(|s| s.chat_id == TARGET);
    client
        .call(
            methods::NOTIFY_TURN_END,
            json!({ "chatId": TARGET, "watcherChatId": ASKER, "baseline": session, "expectTurn": false }),
        )
        .await
        .unwrap();
    wait_for(
        || {
            core.agent_waits
                .turn_watches()
                .first()
                .is_some_and(|w| w.progress.saw_working)
        },
        "the watch to see the turn working",
    )
    .await;
    core.shutdown().await;
    drop(core);
    // The turn died with the engine; the watch, kept on disk, still reports it.
    let harness = Held::new();
    let core = assemble(&path, harness.clone());
    wait_for(
        || {
            harness
                .runs()
                .iter()
                .any(|p| p.contains("Chat «chat-target"))
        },
        "the resumed watch to notify",
    )
    .await;
    let _ = harness.finish.send(());
    core.shutdown().await;
}
