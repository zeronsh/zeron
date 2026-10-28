//! Chats report to the chat that started them without messaging it
//! (`chat_reports`, `DocHost::drain_notices`):
//!
//! - a side chat's delegated turn reaches its parent's agent between turns,
//!   never into a live one, batched with whatever else came in;
//! - a task's agents reach their coordinator as they go (steered into its
//!   live turn);
//! - a coordinator reaches the chat that started the task with `report`, or,
//!   ending with nothing running and no report, with its last reply.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;

use zeron_doc::{MessagePart, MessageRole};
use zeron_engine::{EngineCore, HarnessRegistry};
use zeron_harness::{Harness, HarnessError, RunControls};
use zeron_proto::chat_events::{EventKind, QUIET_REPLY, split_events};
use zeron_proto::{
    AgentEvent, DoneStatus, HarnessId, Model, ReasoningLevel, RunRequest, SteeringMode,
};

const MAIN: &str = "chat-main";
const SIDE: &str = "chat-side";
const OTHER_SIDE: &str = "chat-side-2";
const COORDINATOR: &str = "chat-coordinator";
const AGENT: &str = "chat-agent";

/// Prompts naming a "Task:" answer at once. Turns started by events answer
/// quietly. Anything else holds until the test finishes it, recording what is
/// steered in meanwhile, so "busy" is a state the test controls.
struct Script {
    finish: tokio::sync::broadcast::Sender<()>,
    prompts: Arc<Mutex<Vec<String>>>,
}

fn done(text: &str) -> BoxStream<'static, Result<AgentEvent, HarnessError>> {
    futures::stream::iter(vec![
        Ok(AgentEvent::TextDelta { text: text.into() }),
        Ok(AgentEvent::Done {
            status: DoneStatus::Completed,
            result: None,
            error: None,
            session_id: None,
        }),
    ])
    .boxed()
}

#[async_trait]
impl Harness for Script {
    fn id(&self) -> HarnessId {
        HarnessId::Mock
    }
    fn display_name(&self) -> &str {
        "Script"
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
        if request.prompt.contains("Task:") {
            let answer = if request.prompt.contains("tests") {
                "Tests are green."
            } else {
                "Docs updated."
            };
            return Ok(done(answer));
        }
        if request.prompt.contains("Wrap up") {
            return Ok(done("Everything is merged."));
        }
        if request.prompt.starts_with("<zeron-chat-events>") {
            return Ok(done(QUIET_REPLY));
        }
        let mut finish = self.finish.subscribe();
        let mut steering = controls.steering;
        let prompts = self.prompts.clone();
        let held = futures::stream::once(async move {
            loop {
                tokio::select! {
                    _ = finish.recv() => break,
                    steer = steering.recv() => match steer {
                        Some(message) => prompts.lock().unwrap().push(format!("STEER {}", message.prompt)),
                        None => break,
                    },
                }
            }
            Ok(AgentEvent::Done {
                status: DoneStatus::Completed,
                result: None,
                error: None,
                session_id: None,
            })
        });
        Ok(
            futures::stream::iter(vec![Ok(AgentEvent::TextDelta { text: "On it.".into() })])
                .chain(held)
                .boxed(),
        )
    }
}

async fn wait_for<F: FnMut() -> bool>(mut predicate: F, what: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !predicate() {
        assert!(tokio::time::Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(15)).await;
    }
}

/// Past the notices' settle window.
const SETTLE_PLUS: u64 = 2_200;

struct World {
    core: EngineCore,
    finish: tokio::sync::broadcast::Sender<()>,
    prompts: Arc<Mutex<Vec<String>>>,
}

impl World {
    async fn new() -> Self {
        let path = tempfile::tempdir().unwrap().keep();
        let (finish, _) = tokio::sync::broadcast::channel(16);
        let prompts = Arc::new(Mutex::new(Vec::new()));
        let registry = HarnessRegistry::new();
        registry.register(Arc::new(Script {
            finish: finish.clone(),
            prompts: prompts.clone(),
        }));
        let core = EngineCore::assemble(&path.join("data"), Arc::new(registry), HarnessId::Mock, None)
            .expect("engine core assembles");
        let world = Self { core, finish, prompts };
        world.create(MAIN, None, None, "Release").await;
        world.create(SIDE, Some(MAIN), None, "Test Runner").await;
        world.create(OTHER_SIDE, Some(MAIN), None, "Docs Writer").await;
        world
    }

    async fn create(&self, chat_id: &str, parent: Option<&str>, role: Option<&str>, title: &str) {
        let client = zeron_rpc::memory_client(self.core.rpc_service());
        let mut op = serde_json::json!({
            "op": "createChat",
            "chatId": chat_id,
            "deviceId": self.core.device_id,
        });
        if let Some(parent) = parent {
            op["parentChatId"] = parent.into();
        }
        if let Some(role) = role {
            op["role"] = role.into();
        }
        client.call(zeron_rpc::methods::MUTATE, op).await.expect("createChat");
        // Pre-titled so the auto-titler never runs a turn of its own.
        self.core.workspace.rename_chat(chat_id, title).expect("rename");
    }

    fn send(&self, chat_id: &str, text: &str) {
        self.core
            .doc_host
            .queue_message(chat_id, text, Vec::new())
            .expect("queue message");
    }

    /// `from` delegates `task` to its child `to`, with the header the Zeron
    /// MCP server writes.
    fn delegate(&self, from: &str, to: &str, task: &str) {
        let label = format!("{from} ({})", &from[..8]);
        let text = format!("Task: {task}");
        let prompt = if from == COORDINATOR {
            zeron_proto::agent_messages::coordinated_message(&label, from, &text)
        } else {
            zeron_proto::agent_messages::delegated_message(&label, from, &text)
        };
        self.send(to, &prompt);
    }

    fn prompts(&self) -> Vec<String> {
        self.prompts.lock().unwrap().clone()
    }

    fn event_prompts(&self) -> Vec<String> {
        self.prompts()
            .into_iter()
            .filter(|p| p.contains("<zeron-chat-events>"))
            .collect()
    }

    fn user_texts(&self, chat: &str) -> Vec<String> {
        let handle = self.core.doc_host.open(chat).unwrap();
        handle
            .doc()
            .read_entries()
            .unwrap()
            .into_iter()
            .filter(|e| e.role == MessageRole::User)
            .map(|e| {
                e.parts
                    .iter()
                    .filter_map(|p| match p {
                        MessagePart::Text { text, .. } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect()
            })
            .collect()
    }

    fn notices(&self, chat: &str) -> Vec<zeron_doc::ChatNotice> {
        self.core.doc_host.open(chat).unwrap().doc().read_notices().unwrap()
    }

    fn pending_notices(&self, chat: &str) -> usize {
        self.notices(chat).iter().filter(|n| n.pending()).count()
    }
}

/// The parent is mid-reply when both side chats finish: nothing steers into
/// its turn. When it ends, one turn hands it both results, and the transcript
/// reads that turn back as events rather than a user message.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn side_chats_report_after_the_turn_together() {
    let world = World::new().await;
    world.send(MAIN, "Ship the release");
    wait_for(|| world.prompts().iter().any(|p| p == "Ship the release"), "the main turn").await;

    world.delegate(MAIN, SIDE, "run the tests");
    world.delegate(MAIN, OTHER_SIDE, "update the docs");
    wait_for(|| world.pending_notices(MAIN) == 2, "both side chats to report").await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        !world.prompts().iter().any(|p| p.starts_with("STEER")),
        "an event never steers a chat's live turn"
    );
    assert!(world.event_prompts().is_empty());

    let _ = world.finish.send(());
    wait_for(|| !world.event_prompts().is_empty(), "the events turn").await;
    let events = world.event_prompts();
    assert_eq!(events.len(), 1, "both events share one turn");
    assert!(events[0].contains("Tests are green.") && events[0].contains("Docs updated."));
    assert_eq!(world.pending_notices(MAIN), 0);

    let texts = world.user_texts(MAIN);
    let (refs, typed) = split_events(texts.last().unwrap()).expect("the turn reads back as events");
    assert_eq!(typed, "");
    let mut reported: Vec<&str> = refs.iter().map(|r| r.chat_id.as_str()).collect();
    reported.sort();
    assert_eq!(reported, [SIDE, OTHER_SIDE]);
    assert!(refs.iter().all(|r| r.kind == EventKind::Finished));
    world.core.shutdown().await;
}

/// The user's own message, sent while events wait (here: frozen by a Stop),
/// carries them at its head: one turn answers both.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_next_message_carries_waiting_events() {
    let world = World::new().await;
    world.send(MAIN, "Ship the release");
    wait_for(|| world.prompts().iter().any(|p| p == "Ship the release"), "the main turn").await;
    world.delegate(MAIN, SIDE, "run the tests");
    wait_for(|| world.pending_notices(MAIN) == 1, "the side chat to report").await;

    // A Stop freezes automatic delivery, like the queue.
    let client = zeron_rpc::memory_client(world.core.rpc_service());
    client
        .call(
            zeron_rpc::methods::QUEUE_COMMAND,
            serde_json::json!({ "chatId": MAIN, "command": { "kind": "interrupt" } }),
        )
        .await
        .expect("stop");
    tokio::time::sleep(Duration::from_millis(SETTLE_PLUS)).await;
    assert!(world.event_prompts().is_empty(), "a Stop holds the events");
    assert_eq!(world.pending_notices(MAIN), 1);

    // Typed into an idle chat, a message is a Run (the explicit send that
    // thaws a Stop), not a queue row.
    let request: RunRequest = serde_json::from_value(serde_json::json!({
        "prompt": "How's it going?",
        "cwd": "~",
        "sandbox": "workspace-write",
        "autoApprove": false,
    }))
    .expect("run request");
    world
        .core
        .doc_host
        .queue_command(
            MAIN,
            zeron_doc::SessionCommandPayload::Run {
                request,
                message_id: "typed".into(),
            },
        )
        .expect("run");
    wait_for(|| !world.event_prompts().is_empty(), "the user's turn").await;
    let carried = world.event_prompts().pop().unwrap();
    assert!(carried.contains("Tests are green."));
    assert!(carried.ends_with("How's it going?"));
    let texts = world.user_texts(MAIN);
    let (refs, typed) = split_events(texts.last().unwrap()).unwrap();
    assert_eq!((refs.len(), typed), (1, "How's it going?"));
    assert_eq!(world.pending_notices(MAIN), 0);
    let _ = world.finish.send(());
    world.core.shutdown().await;
}

/// A turn the user started by typing into the side chat is theirs: the
/// parent hears nothing of it. And a result its agent already read (an
/// acknowledgement from `wait_for_turn`) starts no turn either.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn only_unread_delegated_work_is_reported() {
    let world = World::new().await;
    world.send(SIDE, "Task: run the tests yourself");
    wait_for(
        || world.prompts().iter().any(|p| p == "Task: run the tests yourself"),
        "the user's own side-chat turn",
    )
    .await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(world.pending_notices(MAIN), 0, "the user's own turn is not reported");

    world.delegate(MAIN, OTHER_SIDE, "update the docs");
    wait_for(
        || world.prompts().iter().filter(|p| p.contains("update the docs")).count() == 1,
        "the delegated turn",
    )
    .await;
    let user_id = {
        let handle = world.core.doc_host.open(OTHER_SIDE).unwrap();
        let entries = handle.doc().read_entries().unwrap();
        entries.iter().rev().find(|e| e.role == MessageRole::User).unwrap().id.clone()
    };
    world
        .core
        .doc_host
        .ack_notices(MAIN, &[zeron_doc::notice_id(EventKind::Finished, OTHER_SIDE, &user_id)])
        .unwrap();
    tokio::time::sleep(Duration::from_millis(SETTLE_PLUS)).await;
    assert_eq!(world.pending_notices(MAIN), 0);
    assert!(world.event_prompts().is_empty(), "nothing unread, so no turn");
    world.core.shutdown().await;
}

/// A coordinator supervises its agents: their results reach it at its next
/// step, mid-turn, so it can steer before it finishes. The chat that started
/// the task hears none of it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_coordinator_hears_its_agents_as_they_go() {
    let world = World::new().await;
    world.create(COORDINATOR, Some(MAIN), Some("coordinator"), "Zeron Refactor").await;
    world.create(AGENT, Some(COORDINATOR), None, "Test Author").await;
    world.send(COORDINATOR, "Coordinate the refactor");
    wait_for(
        || world.prompts().iter().any(|p| p == "Coordinate the refactor"),
        "the coordinator's turn",
    )
    .await;

    world.delegate(COORDINATOR, AGENT, "write the tests");
    wait_for(
        || {
            world
                .prompts()
                .iter()
                .any(|p| p.starts_with("STEER <zeron-chat-events>") && p.contains("Tests are green."))
        },
        "the agent's result steered into the coordinator",
    )
    .await;
    let steered = world
        .prompts()
        .into_iter()
        .find(|p| p.starts_with("STEER <zeron-chat-events>"))
        .unwrap();
    assert!(steered.contains("`report`"), "a coordinator is told how to report");
    assert_eq!(world.notices(MAIN).len(), 0, "the agent's traffic stays in the task");
    let _ = world.finish.send(());
    world.core.shutdown().await;
}

/// A coordinator's `report` reaches the chat that started the task, as an
/// event, never a message; one that ends a turn with nothing running and no
/// report has its last reply reported for it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_task_reports_to_the_chat_that_started_it() {
    let world = World::new().await;
    world.create(COORDINATOR, Some(MAIN), Some("coordinator"), "Zeron Refactor").await;

    // Only a coordinator reports, and only to its parent.
    let client = zeron_rpc::memory_client(world.core.rpc_service());
    let refused = client
        .call(
            zeron_rpc::methods::REPORT_TO_PARENT,
            serde_json::json!({ "chatId": MAIN, "fromChatId": SIDE, "status": "done", "text": "x" }),
        )
        .await;
    assert!(refused.is_err());
    client
        .call(
            zeron_rpc::methods::REPORT_TO_PARENT,
            serde_json::json!({
                "chatId": MAIN, "fromChatId": COORDINATOR,
                "status": "question", "text": "Keep the old API around?",
            }),
        )
        .await
        .expect("report");
    wait_for(|| !world.event_prompts().is_empty(), "the report's turn").await;
    let report = world.event_prompts().pop().unwrap();
    assert!(report.contains("[Task question · \"Zeron Refactor\" · chat chat-coordinator]"));
    assert!(report.contains("Keep the old API around?"));

    // A turn that ends with no report and nothing running reports itself.
    world.send(COORDINATOR, "Wrap up");
    wait_for(
        || world.notices(MAIN).iter().any(|n| n.kind == EventKind::TaskDone),
        "the task's result",
    )
    .await;
    let result = world
        .notices(MAIN)
        .into_iter()
        .find(|n| n.kind == EventKind::TaskDone)
        .unwrap();
    assert_eq!(result.text, "Everything is merged.");
    world.core.shutdown().await;
}
