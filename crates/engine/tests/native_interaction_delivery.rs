//! Engine-level delivery invariants shared by provider-native composer controls.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use async_trait::async_trait;
use futures::{StreamExt, stream::BoxStream};
use tokio::sync::mpsc;
use zeron_doc::{MessagePart, MessageRole};
use zeron_engine::{EngineCore, HarnessRegistry};
use zeron_harness::{Harness, HarnessError, RunControls};
use zeron_proto::{
    AgentEvent, HarnessId, Model, ReasoningLevel, RunRequest, SandboxLevel, SteeringMode,
};

const CHAT: &str = "native-interaction-delivery";

enum Delivery {
    Run(RunRequest),
}

struct RecordingHarness {
    id: HarnessId,
    session_id: String,
    delivery: mpsc::UnboundedSender<Delivery>,
    reject_request: AtomicBool,
}

#[async_trait]
impl Harness for RecordingHarness {
    fn id(&self) -> HarnessId {
        self.id
    }

    fn display_name(&self) -> &str {
        "Native interaction delivery probe"
    }

    fn supports_steering(&self) -> bool {
        true
    }

    fn steering_mode(&self) -> SteeringMode {
        SteeringMode::StepBoundary
    }

    fn reasoning_levels(&self) -> &[ReasoningLevel] {
        &[]
    }

    fn validate_request(&self, _request: &RunRequest) -> Result<(), HarnessError> {
        if self.reject_request.load(Ordering::SeqCst) {
            Err(HarnessError::Protocol(
                "unsupported native interaction request".into(),
            ))
        } else {
            Ok(())
        }
    }

    async fn models(&self) -> Result<Vec<Model>, HarnessError> {
        Ok(Vec::new())
    }

    async fn run(
        &self,
        request: RunRequest,
        controls: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        self.delivery.send(Delivery::Run(request.clone())).unwrap();
        let started = futures::stream::iter([Ok(AgentEvent::SessionStarted {
            harness: self.id,
            model: "probe".into(),
            tools: Vec::new(),
            cwd: request.cwd,
            session_id: self.session_id.clone(),
            assistant_message_id: "native-assistant".into(),
        })]);
        let stream = futures::stream::unfold(controls, |mut controls| async move {
            tokio::select! {
                _ = controls.interrupt.cancelled() => None,
                message = controls.steering.recv() => {
                    message?;
                    Some((Ok(AgentEvent::Steered {
                        assistant_message_id: None,
                        next_assistant_message_id: None,
                    }), controls))
                }
            }
        });
        Ok(started.chain(stream).boxed())
    }
}

fn request(prompt: &str) -> RunRequest {
    RunRequest {
        prompt: prompt.into(),
        harness: None,
        model: None,
        reasoning: None,
        model_options: Default::default(),
        cwd: "/tmp".into(),
        sandbox: SandboxLevel::WorkspaceWrite,
        auto_approve: true,
        resume: None,
        attachments: Vec::new(),
        worktree: None,
    }
}

async fn receive(rx: &mut mpsc::UnboundedReceiver<Delivery>) -> RunRequest {
    let delivery = tokio::time::timeout(Duration::from_secs(10), rx.recv())
        .await
        .expect("delivery timed out")
        .expect("delivery channel closed");
    let Delivery::Run(request) = delivery;
    request
}

async fn create_chat(core: &EngineCore) {
    zeron_rpc::memory_client(core.rpc_service())
        .call(
            zeron_rpc::methods::MUTATE,
            serde_json::json!({
                "op": "createChat",
                "chatId": CHAT,
                "deviceId": core.device_id,
            }),
        )
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rejected_recovered_answer_keeps_its_question_open_for_retry() {
    use zeron_doc::{
        MessageStatus, SessionCommandPayload, SessionCommandStatus, SessionMessageEntry,
    };

    let tmp = tempfile::tempdir().unwrap();
    let (delivery, mut rx) = mpsc::unbounded_channel();
    let harness = Arc::new(RecordingHarness {
        id: HarnessId::Codex,
        session_id: "codex-session".into(),
        delivery,
        reject_request: AtomicBool::new(false),
    });
    let registry = HarnessRegistry::new();
    registry.register(harness.clone());
    let core = EngineCore::assemble(
        &tmp.path().join("data"),
        Arc::new(registry),
        HarnessId::Codex,
        None,
    )
    .unwrap();
    create_chat(&core).await;

    let handle = core.doc_host.open(CHAT).unwrap();
    handle
        .doc()
        .push_message(&SessionMessageEntry {
            id: "settled-question".into(),
            role: MessageRole::Assistant,
            parts: vec![MessagePart::Input {
                id: "question-part".into(),
                request_id: "orphan-question".into(),
                questions: vec![zeron_proto::UserInputQuestion {
                    id: "q1".into(),
                    header: "Choice".into(),
                    question: "Continue?".into(),
                    options: vec!["Yes".into()],
                    option_descriptions: Vec::new(),
                    allow_custom: false,
                    non_blocking: false,
                    multi_select: false,
                }],
                resolved: false,
            }],
            created_at: 0,
            device_id: core.device_id.clone(),
            status: Some(MessageStatus::Complete),
            continuation_of: None,
        })
        .unwrap();

    let answer = || SessionCommandPayload::RespondInput {
        request_id: "orphan-question".into(),
        answers: vec![zeron_proto::UserInputAnswer {
            question_id: "q1".into(),
            labels: vec!["Yes".into()],
        }],
    };
    harness.reject_request.store(true, Ordering::SeqCst);
    let command = core.doc_host.queue_command(CHAT, answer()).unwrap();
    let status = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Some(status) = handle
                .doc()
                .read_commands()
                .unwrap()
                .into_iter()
                .find(|entry| entry.id == command && entry.status != SessionCommandStatus::Pending)
                .map(|entry| entry.status)
            {
                break status;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(status, SessionCommandStatus::Rejected);
    assert!(matches!(
        &handle.doc().read_entries().unwrap()[0].parts[0],
        MessagePart::Input {
            resolved: false,
            ..
        }
    ));
    assert!(rx.try_recv().is_err());

    harness.reject_request.store(false, Ordering::SeqCst);
    core.doc_host.queue_command(CHAT, answer()).unwrap();
    let run = receive(&mut rx).await;
    assert!(run.prompt.contains("Yes"));
    assert!(handle.doc().read_entries().unwrap().iter().any(|entry| {
        entry
            .parts
            .iter()
            .any(|part| matches!(part, MessagePart::Input { resolved: true, .. }))
    }));
    core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_session_resume_is_scoped_to_the_selected_harness() {
    let tmp = tempfile::tempdir().unwrap();
    let (delivery, mut rx) = mpsc::unbounded_channel();
    let harness = |id, session_id: &str| {
        Arc::new(RecordingHarness {
            id,
            session_id: session_id.into(),
            delivery: delivery.clone(),
            reject_request: AtomicBool::new(false),
        })
    };
    let registry = HarnessRegistry::new();
    registry.register(harness(HarnessId::ClaudeCode, "claude-session"));
    registry.register(harness(HarnessId::Cursor, "cursor-session"));
    let core = EngineCore::assemble(
        &tmp.path().join("data"),
        Arc::new(registry),
        HarnessId::ClaudeCode,
        None,
    )
    .unwrap();
    create_chat(&core).await;

    core.sessions
        .dispatch(CHAT, HarnessId::ClaudeCode, request("claude turn"), None)
        .await
        .unwrap();
    assert_eq!(receive(&mut rx).await.resume, None);
    tokio::time::timeout(Duration::from_secs(10), async {
        while core
            .workspace
            .chat(CHAT)
            .unwrap()
            .unwrap()
            .harness_session_harness
            != Some(HarnessId::ClaudeCode)
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    core.sessions.interrupt(CHAT).await.unwrap();

    core.sessions
        .dispatch(CHAT, HarnessId::Cursor, request("cursor turn"), None)
        .await
        .unwrap();
    assert_eq!(receive(&mut rx).await.resume, None);
    tokio::time::timeout(Duration::from_secs(10), async {
        while core
            .workspace
            .chat(CHAT)
            .unwrap()
            .unwrap()
            .harness_session_harness
            != Some(HarnessId::Cursor)
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    core.sessions.interrupt(CHAT).await.unwrap();

    core.sessions
        .dispatch(CHAT, HarnessId::Cursor, request("cursor again"), None)
        .await
        .unwrap();
    assert_eq!(
        receive(&mut rx).await.resume.as_deref(),
        Some("cursor-session")
    );
    core.shutdown().await;
}
