//! The Cloud sleep handshake (`DocHost::quiesce_for_sleep`): before the edge
//! stops a session machine's engine, the engine starts no new work, so a
//! message sent while the machine goes to sleep is never begun and then cut
//! off — it waits in its chat for the woken machine. A turn in flight (or a
//! message already waiting) declines the sleep instead.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;
use tokio::sync::Notify;

use zeron_doc::{MessageRole, MessageStatus, SessionCommandPayload};
use zeron_engine::{EngineCore, HarnessRegistry};
use zeron_harness::{Harness, HarnessError, RunControls};
use zeron_proto::{
    AgentEvent, DoneStatus, HarnessId, Model, ReasoningLevel, RunRequest, SandboxLevel,
    SteeringMode,
};

const CHAT: &str = "chat-sleep-quiesce";

/// Answers at once, except a prompt containing "hold", which answers only
/// once `release` is notified (a turn in flight).
struct HoldingHarness {
    release: Arc<Notify>,
}

#[async_trait]
impl Harness for HoldingHarness {
    fn id(&self) -> HarnessId {
        HarnessId::Mock
    }
    fn display_name(&self) -> &str {
        "Holding"
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
        let hold = request.prompt.contains("hold");
        let release = self.release.clone();
        let reply = format!("answered: {}", request.prompt);
        let (tx, rx) = futures::channel::mpsc::unbounded();
        let cwd = request.cwd.clone();
        tokio::spawn(async move {
            let _ = tx.unbounded_send(Ok(AgentEvent::SessionStarted {
                harness: HarnessId::Mock,
                model: "mock-1".into(),
                tools: vec![],
                cwd,
                session_id: "sess-quiesce".into(),
                assistant_message_id: "a-1".into(),
            }));
            if hold {
                release.notified().await;
            }
            let _ = tx.unbounded_send(Ok(AgentEvent::TextDelta { text: reply }));
            let _ = tx.unbounded_send(Ok(AgentEvent::Done {
                status: DoneStatus::Completed,
                result: None,
                error: None,
                session_id: Some("sess-quiesce".into()),
            }));
        });
        Ok(rx.boxed())
    }
}

fn assemble(dir: &std::path::Path, release: Arc<Notify>) -> EngineCore {
    let registry = HarnessRegistry::new();
    registry.register(Arc::new(HoldingHarness { release }));
    EngineCore::assemble(dir, Arc::new(registry), HarnessId::Mock, None)
        .expect("engine core assembles")
}

async fn wait_for(mut predicate: impl FnMut() -> bool, what: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !predicate() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(15)).await;
    }
}

fn answered(core: &EngineCore, prompt: &str) -> bool {
    let text = format!("answered: {prompt}");
    core.doc_host
        .open(CHAT)
        .ok()
        .and_then(|h| h.doc().read_entries().ok())
        .unwrap_or_default()
        .iter()
        .any(|e| {
            e.role == MessageRole::Assistant
                && e.status == Some(MessageStatus::Complete)
                && e.parts.iter().any(
                    |p| matches!(p, zeron_doc::MessagePart::Text { text: t, .. } if t == &text),
                )
        })
}

fn user_message(core: &EngineCore, id: &str) -> bool {
    core.doc_host
        .open(CHAT)
        .ok()
        .and_then(|h| h.doc().read_entries().ok())
        .unwrap_or_default()
        .iter()
        .any(|e| e.id == id)
}

fn send(core: &EngineCore, id: &str, prompt: &str) {
    core.doc_host
        .queue_command(
            CHAT,
            SessionCommandPayload::Run {
                request: RunRequest {
                    mcp: None,
                    prompt: prompt.into(),
                    harness: None,
                    model: None,
                    reasoning: None,
                    model_options: Default::default(),
                    cwd: "~".into(),
                    sandbox: SandboxLevel::WorkspaceWrite,
                    auto_approve: true,
                    attachments: Vec::new(),
                    worktree: None,
                    resume: None,
                    env: Default::default(),
                },
                message_id: id.into(),
            },
        )
        .expect("queue run command");
}

async fn create_chat(core: &EngineCore) {
    let client = zeron_rpc::memory_client(core.rpc_service());
    client
        .call(
            zeron_rpc::methods::MUTATE,
            serde_json::json!({ "op": "createChat", "chatId": CHAT, "deviceId": core.device_id }),
        )
        .await
        .expect("createChat");
    // Pre-titled: the auto-titler's harness request stays out of the runs.
    core.workspace.rename_chat(CHAT, "Pre-titled").unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_message_sent_while_the_machine_goes_to_sleep_waits_instead_of_being_cut_off() {
    let tmp = tempfile::tempdir().unwrap();
    let core = assemble(tmp.path(), Arc::new(Notify::new()));
    create_chat(&core).await;
    send(&core, "m-before", "before the sleep");
    wait_for(|| answered(&core, "before the sleep"), "the first turn").await;

    core.doc_host
        .quiesce_for_sleep()
        .await
        .expect("idle: quiesced");
    // The machine is about to stop: nothing starts, the message waits.
    send(&core, "m-during", "during the sleep");
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(
        !user_message(&core, "m-during"),
        "a run began on a quiescing engine"
    );
    assert!(!answered(&core, "during the sleep"));

    // Lifting it (the woken machine's reconcile, in production) runs it.
    core.doc_host.lift_quiesce();
    wait_for(
        || answered(&core, "during the sleep"),
        "the waiting message",
    )
    .await;
    core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_turn_in_flight_declines_the_sleep_and_work_goes_on() {
    let tmp = tempfile::tempdir().unwrap();
    let release = Arc::new(Notify::new());
    let core = assemble(tmp.path(), release.clone());
    create_chat(&core).await;
    send(&core, "m-long", "hold on");
    wait_for(|| user_message(&core, "m-long"), "the long turn to start").await;

    let busy = core.doc_host.quiesce_for_sleep().await.unwrap_err();
    assert!(busy.contains("turn in flight"), "{busy}");

    // Not quiesced: the turn finishes and the next message runs.
    release.notify_one();
    wait_for(|| answered(&core, "hold on"), "the long turn").await;
    send(&core, "m-next", "next one");
    wait_for(|| answered(&core, "next one"), "the next message").await;
    core.shutdown().await;
}
