//! A side chat reports to the chat that started it without messaging it: its
//! host leaves a notice on the parent's doc when a delegated turn ends, and
//! the parent's host hands the parent's agent every waiting notice at once,
//! never into a live turn (`side_chat_reports`, `DocHost::drain_notices`).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;

use zeron_doc::{MessagePart, MessageRole};
use zeron_engine::{EngineCore, HarnessRegistry};
use zeron_harness::{Harness, HarnessError, RunControls};
use zeron_proto::side_chat_updates::{QUIET_REPLY, UpdateKind, split_updates};
use zeron_proto::{
    AgentEvent, DoneStatus, HarnessId, Model, ReasoningLevel, RunRequest, SteeringMode,
};

const PARENT: &str = "chat-orchestrator";
const SIDE: &str = "chat-side";
const OTHER_SIDE: &str = "chat-side-2";

/// Side chats (any prompt naming a task) answer at once. The orchestrator's
/// turns hang until the test finishes them, so "the parent is busy" is a
/// state the test holds rather than races.
struct Script {
    finish: tokio::sync::broadcast::Sender<()>,
    prompts: Arc<Mutex<Vec<String>>>,
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
        let done = |text: String| {
            futures::stream::iter(vec![
                Ok(AgentEvent::TextDelta { text }),
                Ok(AgentEvent::Done {
                    status: DoneStatus::Completed,
                    result: None,
                    error: None,
                    session_id: None,
                }),
            ])
            .boxed()
        };
        if request.prompt.contains("Task:") {
            let answer = if request.prompt.contains("tests") {
                "Tests are green."
            } else {
                "Docs updated."
            };
            return Ok(done(answer.into()));
        }
        if request.prompt.starts_with("<zeron-side-chat-updates>") {
            return Ok(done(QUIET_REPLY.into()));
        }
        // The orchestrator: hold the turn; record anything steered into it.
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
        Ok(futures::stream::iter(vec![Ok(AgentEvent::TextDelta {
            text: "On it.".into(),
        })])
        .chain(held)
        .boxed())
    }
}

async fn wait_for<F: FnMut() -> bool>(mut predicate: F, what: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !predicate() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(15)).await;
    }
}

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
        let core = EngineCore::assemble(
            &path.join("data"),
            Arc::new(registry),
            HarnessId::Mock,
            None,
        )
        .expect("engine core assembles");
        let world = Self {
            core,
            finish,
            prompts,
        };
        world.create(PARENT, None, "Release").await;
        world.create(SIDE, Some(PARENT), "Run the tests").await;
        world
            .create(OTHER_SIDE, Some(PARENT), "Update the docs")
            .await;
        world
    }

    async fn create(&self, chat_id: &str, parent: Option<&str>, title: &str) {
        let client = zeron_rpc::memory_client(self.core.rpc_service());
        let mut op = serde_json::json!({
            "op": "createChat",
            "chatId": chat_id,
            "deviceId": self.core.device_id,
        });
        if let Some(parent) = parent {
            op["parentChatId"] = parent.into();
        }
        client
            .call(zeron_rpc::methods::MUTATE, op)
            .await
            .expect("createChat");
        // Pre-titled so the auto-titler never runs a turn of its own.
        self.core
            .workspace
            .rename_chat(chat_id, title)
            .expect("rename");
    }

    fn send(&self, chat_id: &str, text: &str) {
        self.core
            .doc_host
            .queue_message(chat_id, text, Vec::new())
            .expect("queue message");
    }

    /// The orchestrator delegates `task` to `side`, as `create_chat` does.
    fn delegate(&self, side: &str, task: &str) {
        let label = format!("Release ({})", &PARENT[..8]);
        self.send(
            side,
            &zeron_proto::agent_messages::delegated_message(
                &label,
                PARENT,
                &format!("Task: {task}"),
            ),
        );
    }

    fn prompts(&self) -> Vec<String> {
        self.prompts.lock().unwrap().clone()
    }

    fn update_prompts(&self) -> Vec<String> {
        self.prompts()
            .into_iter()
            .filter(|p| p.contains("<zeron-side-chat-updates>"))
            .collect()
    }

    fn parent_user_texts(&self) -> Vec<String> {
        let handle = self.core.doc_host.open(PARENT).unwrap();
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

    fn pending_notices(&self) -> usize {
        let handle = self.core.doc_host.open(PARENT).unwrap();
        handle
            .doc()
            .read_notices()
            .unwrap()
            .iter()
            .filter(|n| n.pending())
            .count()
    }
}

/// The orchestrator is mid-reply when both side chats finish: nothing steers
/// into its turn. When it ends, one turn hands it both results, and the
/// transcript reads that turn back as updates rather than a user message.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn updates_wait_for_the_turn_and_arrive_together() {
    let world = World::new().await;
    world.send(PARENT, "Ship the release");
    wait_for(
        || world.prompts().iter().any(|p| p == "Ship the release"),
        "the orchestrator's turn",
    )
    .await;

    world.delegate(SIDE, "run the tests");
    world.delegate(OTHER_SIDE, "update the docs");
    wait_for(|| world.pending_notices() == 2, "both side chats to report").await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        !world.prompts().iter().any(|p| p.starts_with("STEER")),
        "an update never steers a live turn"
    );
    assert!(world.update_prompts().is_empty());

    let _ = world.finish.send(());
    wait_for(|| !world.update_prompts().is_empty(), "the updates turn").await;
    let updates = world.update_prompts();
    assert_eq!(updates.len(), 1, "both updates share one turn");
    assert!(updates[0].contains("Tests are green.") && updates[0].contains("Docs updated."));
    assert_eq!(world.pending_notices(), 0);

    let texts = world.parent_user_texts();
    let (refs, typed) =
        split_updates(texts.last().unwrap()).expect("the turn reads back as updates");
    assert_eq!(typed, "");
    let mut reported: Vec<&str> = refs.iter().map(|r| r.chat_id.as_str()).collect();
    reported.sort();
    assert_eq!(reported, [SIDE, OTHER_SIDE]);
    assert!(refs.iter().all(|r| r.kind == UpdateKind::Finished));
    world.core.shutdown().await;
}

/// The user's own message, sent while updates wait (here: frozen by a Stop),
/// carries them at its head: one turn answers both.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_next_message_carries_waiting_updates() {
    let world = World::new().await;
    world.send(PARENT, "Ship the release");
    wait_for(
        || world.prompts().iter().any(|p| p == "Ship the release"),
        "the orchestrator's turn",
    )
    .await;
    world.delegate(SIDE, "run the tests");
    wait_for(|| world.pending_notices() == 1, "the side chat to report").await;

    // A Stop freezes automatic delivery, like the queue.
    let client = zeron_rpc::memory_client(world.core.rpc_service());
    client
        .call(
            zeron_rpc::methods::QUEUE_COMMAND,
            serde_json::json!({ "chatId": PARENT, "command": { "kind": "interrupt" } }),
        )
        .await
        .expect("stop");
    tokio::time::sleep(Duration::from_millis(NOTICE_SETTLE_PLUS)).await;
    assert!(
        world.update_prompts().is_empty(),
        "a Stop holds the updates"
    );
    assert_eq!(world.pending_notices(), 1);

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
            PARENT,
            zeron_doc::SessionCommandPayload::Run {
                request,
                message_id: "typed".into(),
            },
        )
        .expect("run");
    wait_for(|| !world.update_prompts().is_empty(), "the user's turn").await;
    let carried = world.update_prompts().pop().unwrap();
    assert!(carried.contains("Tests are green."));
    assert!(carried.ends_with("How's it going?"));
    let texts = world.parent_user_texts();
    let (refs, typed) = split_updates(texts.last().unwrap()).unwrap();
    assert_eq!((refs.len(), typed), (1, "How's it going?"));
    assert_eq!(world.pending_notices(), 0);
    let _ = world.finish.send(());
    world.core.shutdown().await;
}

const NOTICE_SETTLE_PLUS: u64 = 2_200;

/// A turn the user started by typing into the side chat is theirs: the
/// orchestrator hears nothing of it. And a result its agent already read
/// (an acknowledgement from `wait_for_turn`) starts no turn either.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn only_unread_delegated_work_is_reported() {
    let world = World::new().await;
    world.send(SIDE, "Task: run the tests yourself");
    wait_for(
        || {
            world
                .prompts()
                .iter()
                .any(|p| p == "Task: run the tests yourself")
        },
        "the user's own side-chat turn",
    )
    .await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(
        world.pending_notices(),
        0,
        "the user's own turn is not reported"
    );

    // The orchestrator waited on this one and already has the reply.
    world.delegate(OTHER_SIDE, "update the docs");
    wait_for(
        || {
            world
                .prompts()
                .iter()
                .filter(|p| p.contains("update the docs"))
                .count()
                == 1
        },
        "the delegated turn",
    )
    .await;
    let user_id = {
        let handle = world.core.doc_host.open(OTHER_SIDE).unwrap();
        let entries = handle.doc().read_entries().unwrap();
        entries
            .iter()
            .rev()
            .find(|e| e.role == MessageRole::User)
            .unwrap()
            .id
            .clone()
    };
    world
        .core
        .doc_host
        .ack_notices(
            PARENT,
            &[zeron_doc::notice_id(
                UpdateKind::Finished,
                OTHER_SIDE,
                &user_id,
            )],
        )
        .unwrap();
    tokio::time::sleep(Duration::from_millis(NOTICE_SETTLE_PLUS)).await;
    assert_eq!(world.pending_notices(), 0);
    assert!(
        world.update_prompts().is_empty(),
        "nothing unread, so no turn"
    );
    world.core.shutdown().await;
}
