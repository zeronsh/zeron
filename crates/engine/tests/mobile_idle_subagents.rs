//! A mobile follow-up to a desktop-started, parked parent must reuse its runtime.

#[path = "../../client/tests/support/mock_edge.rs"]
mod mock_edge;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use futures::{StreamExt, stream::BoxStream};
use mock_edge::MockEdge;
use zeron_client::events::NullListener;
use zeron_client::{Client, ClientConfig, Credentials, SendOutcome, SendRequest};
use zeron_doc::{
    MessagePart, MessageRole, MessageStatus, SessionCommandPayload, SessionCommandStatus,
    SubagentStatus,
};
use zeron_engine::{EngineCore, HarnessRegistry};
use zeron_harness::{Harness, HarnessError, RunControls};
use zeron_proto::{
    AgentEvent, ChatConfig, DoneStatus, HarnessId, Model, ReasoningLevel, RunRequest, SandboxLevel,
    SessionStatus, SteeringMode, ToolCall,
};

const CHAT: &str = "mobile-idle-subagents";
const SPAWN: &str = "spawn-background";

struct BackgroundHarness {
    starts: Arc<AtomicUsize>,
    interrupted: Arc<AtomicBool>,
    callback_script: Option<Vec<AgentEvent>>,
}

fn done() -> AgentEvent {
    AgentEvent::Done {
        status: DoneStatus::Completed,
        result: None,
        error: None,
        session_id: Some("background-session".into()),
    }
}

#[async_trait]
impl Harness for BackgroundHarness {
    fn id(&self) -> HarnessId {
        HarnessId::Mock
    }
    fn display_name(&self) -> &str {
        "Background subagent probe"
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
    async fn models(&self) -> Result<Vec<Model>, HarnessError> {
        Ok(vec![])
    }
    async fn run(
        &self,
        _request: RunRequest,
        mut controls: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        self.starts.fetch_add(1, Ordering::SeqCst);
        let interrupted = self.interrupted.clone();
        let callback_script = self.callback_script.clone();
        let (tx, rx) = tokio::sync::mpsc::channel(16);
        tokio::spawn(async move {
            if let Some(script) = callback_script {
                for event in script {
                    if tx.send(Ok(event)).await.is_err() {
                        return;
                    }
                    tokio::time::sleep(Duration::from_millis(400)).await;
                }
                controls.interrupt.cancelled().await;
                return;
            }
            for event in [
                AgentEvent::ToolCall {
                    id: SPAWN.into(),
                    call: ToolCall::Unknown {
                        name: "Agent: background probe".into(),
                        input: None,
                    },
                },
                AgentEvent::Subagent {
                    parent_tool_use_id: SPAWN.into(),
                    event: Box::new(AgentEvent::TextDelta {
                        text: "child still working".into(),
                    }),
                },
                AgentEvent::TextDelta {
                    text: "parent done".into(),
                },
                done(),
            ] {
                if tx.send(Ok(event)).await.is_err() {
                    return;
                }
            }
            loop {
                tokio::select! {
                    _ = controls.interrupt.cancelled() => {
                        interrupted.store(true, Ordering::SeqCst);
                        return;
                    }
                    message = controls.steering.recv() => {
                        let Some(message) = message else { return };
                        for event in [
                            AgentEvent::Steered {
                                assistant_message_id: None,
                                next_assistant_message_id: None,
                            },
                            AgentEvent::Subagent {
                                parent_tool_use_id: SPAWN.into(),
                                event: Box::new(AgentEvent::TextDelta {
                                    text: "; child survived follow-up".into(),
                                }),
                            },
                            AgentEvent::TextDelta { text: message.prompt },
                            done(),
                        ] {
                            if tx.send(Ok(event)).await.is_err() {
                                return;
                            }
                        }
                    }
                }
            }
        });
        Ok(futures::stream::unfold(rx, |mut rx| async move {
            rx.recv().await.map(|event| (event, rx))
        })
        .boxed())
    }
}

fn run_request(prompt: &str) -> RunRequest {
    RunRequest {
        mcp: None,
        prompt: prompt.into(),
        harness: Some(HarnessId::Mock),
        model: None,
        reasoning: None,
        model_options: Default::default(),
        cwd: "/tmp".into(),
        sandbox: SandboxLevel::WorkspaceWrite,
        auto_approve: false,
        attachments: vec![],
        resume: None,
        worktree: None,
    }
}

async fn wait_for(what: &str, mut predicate: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while !predicate() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mobile_send_to_idle_parent_preserves_running_subagents() {
    let edge = MockEdge::start().await;
    let host_dir = tempfile::tempdir().unwrap();
    let phone_dir = tempfile::tempdir().unwrap();
    let starts = Arc::new(AtomicUsize::new(0));
    let interrupted = Arc::new(AtomicBool::new(false));
    let registry = HarnessRegistry::new();
    registry.register(Arc::new(BackgroundHarness {
        starts: starts.clone(),
        interrupted: interrupted.clone(),
        callback_script: None,
    }));
    let core = EngineCore::assemble_with_identity(
        host_dir.path(),
        Arc::new(registry),
        HarnessId::Mock,
        None,
        "org-1",
        "user-1",
    )
    .unwrap();
    core.workspace.connect_registry_url(&edge.registry.url());
    core.workspace
        .create_chat(
            CHAT,
            None,
            Some(&core.device_id),
            Some(ChatConfig {
                harness: HarnessId::Mock,
                model: None,
                reasoning: None,
                model_options: Default::default(),
                sandbox: SandboxLevel::WorkspaceWrite,
            }),
            Some("/tmp".into()),
        )
        .unwrap();
    let handle = core.doc_host.open(CHAT).unwrap();
    // Match the desktop composer, including its approval policy.
    let initial = run_request("start background work");
    core.sessions
        .dispatch(
            CHAT,
            HarnessId::Mock,
            initial,
            Some("desktop-message".into()),
        )
        .await
        .unwrap();
    wait_for("idle parent with a running child", || {
        core.sessions
            .session_status(CHAT)
            .is_some_and(|s| s.status == SessionStatus::Idle && s.running_subagents == 1)
    })
    .await;
    edge.inject(
        CHAT,
        &core.device_id,
        handle.doc().export_snapshot().unwrap(),
    );

    let mut config = ClientConfig::new(edge.edge_url(), phone_dir.path());
    config.device_id = "android-viewer".into();
    config.platform = "android".into();
    let phone = Client::new(
        config,
        Credentials::Dev {
            user_id: "user-1".into(),
            org_id: "org-1".into(),
        },
        Arc::new(NullListener),
    )
    .unwrap();
    wait_for("remote chat row", || {
        phone
            .workspace()
            .session(CHAT)
            .is_some_and(|s| s.running_subagents == 1)
    })
    .await;
    let session = phone.open_session(CHAT).unwrap();
    session.set_view_attached(true);
    wait_for("remote transcript", || session.snapshot().hydrated).await;
    assert!(!session.composer().live.turn_running);
    let SendOutcome::Started { message_id } = session
        .send(SendRequest::text("follow up from Android"))
        .unwrap()
    else {
        panic!("an idle parent accepts a new turn");
    };
    // Deliver the viewer's real command rows to the host doc, as its chat
    // room would. The production command executor handles the Run.
    wait_for("follow-up applied on the host", || {
        for row in edge.rows(CHAT) {
            handle.doc().doc().import(&row.bytes).unwrap();
        }
        handle.doc().read_commands().unwrap().iter().any(|c| {
            c.status == SessionCommandStatus::Applied
                && matches!(&c.payload, SessionCommandPayload::Run { message_id: id, .. }
                    if id == &message_id)
        })
    })
    .await;

    assert!(
        !interrupted.load(Ordering::SeqCst),
        "mobile send cancelled the runtime"
    );
    assert_eq!(
        starts.load(Ordering::SeqCst),
        1,
        "mobile send respawned the harness"
    );
    let status = core.sessions.session_status(CHAT).unwrap();
    assert_eq!(status.running_subagents, 1);
    wait_for("parent reply to the follow-up", || {
        handle.doc().read_entries().unwrap().iter().any(|e| {
            e.role == MessageRole::Assistant
                && e.status == Some(MessageStatus::Complete)
                && e.parts.iter().any(|p| {
                    matches!(p,
                    MessagePart::Text { text, .. } if text == "follow up from Android")
                })
        })
    })
    .await;
    let entries = handle.doc().read_entries().unwrap();
    assert_eq!(entries.iter().filter(|e| e.id == message_id).count(), 1);
    assert!(entries.iter().flat_map(|e| &e.parts).any(|p| matches!(p,
        MessagePart::Tool { id, subagent_status: Some(SubagentStatus::Running), .. }
            if id == SPAWN)));
    let child = core
        .doc_host
        .open(&format!("{CHAT}--sub--{SPAWN}"))
        .unwrap();
    wait_for("child output after the follow-up", || {
        child.doc().read_entries().unwrap().iter().flat_map(|e| &e.parts).any(|p|
            matches!(p, MessagePart::Text { text, .. } if text.contains("child survived follow-up")))
    })
    .await;
    phone.shutdown();
    core.shutdown().await;
}

/// Background metadata survives a completed turn without fabricating another
/// assistant entry, then clears on callback completion or runtime shutdown.
#[tokio::test(flavor = "multi_thread")]
async fn parked_background_callbacks_do_not_unpark_the_main_thread() {
    let dir = tempfile::tempdir().unwrap();
    let registry = HarnessRegistry::new();
    registry.register(Arc::new(BackgroundHarness {
        starts: Arc::new(AtomicUsize::new(0)),
        interrupted: Arc::new(AtomicBool::new(false)),
        callback_script: Some(vec![
            AgentEvent::TextDelta {
                text: "Waiting for a build".into(),
            },
            AgentEvent::PendingCallbacks { count: 1 },
            done(),
            AgentEvent::PendingCallbacks { count: 0 },
            AgentEvent::TextDelta {
                text: "Build finished".into(),
            },
            done(),
            AgentEvent::PendingCallbacks { count: 1 },
        ]),
    }));
    let core = EngineCore::assemble(dir.path(), Arc::new(registry), HarnessId::Mock, None).unwrap();
    let handle = core.doc_host.open(CHAT).unwrap();
    core.sessions
        .dispatch(
            CHAT,
            HarnessId::Mock,
            run_request("start a background build"),
            Some("callback-user".into()),
        )
        .await
        .unwrap();
    let entries = || handle.doc().read_entries().unwrap();
    let idle_with = |count| {
        core.sessions
            .session_status(CHAT)
            .is_some_and(|s| s.status == SessionStatus::Idle && s.pending_callbacks == count)
    };
    wait_for("settled main thread awaiting callback", || idle_with(1)).await;
    let first_completion = core
        .sessions
        .session_status(CHAT)
        .unwrap()
        .last_completed_turn;
    let first_assistants = entries()
        .iter()
        .filter(|entry| entry.role == MessageRole::Assistant)
        .count();
    wait_for("callback completion metadata", || idle_with(0)).await;
    assert_eq!(
        core.sessions
            .session_status(CHAT)
            .unwrap()
            .last_completed_turn,
        first_completion,
        "metadata cannot fabricate another completed main turn"
    );
    assert_eq!(
        entries()
            .iter()
            .filter(|entry| entry.role == MessageRole::Assistant)
            .count(),
        first_assistants,
        "callback metadata cannot fabricate assistant output"
    );
    wait_for("new callback after an actual wake turn", || idle_with(1)).await;
    assert!(
        entries().iter().any(|entry| {
            entry.role == MessageRole::Assistant
                && entry.status == Some(MessageStatus::Complete)
                && entry.parts.iter().any(|part| {
                    matches!(part, MessagePart::Text { text, .. } if text == "Build finished")
                })
        }),
        "main output still wakes and settles its own assistant entry"
    );
    // Cancel targets an in-flight user turn; an idle warm process is retired
    // by engine shutdown, which must clear its deferred-work metadata too.
    core.sessions.shutdown().await;
    wait_for("runtime shutdown clears pending callbacks", || idle_with(0)).await;
}

/// Counts follow confirmed child lifecycles even if no child has emitted text.
#[tokio::test(flavor = "multi_thread")]
async fn idle_parent_counts_silent_children_and_deduplicates_lifecycle_events() {
    let child = |id: &str, event| AgentEvent::Subagent {
        parent_tool_use_id: id.into(),
        event: Box::new(event),
    };
    let start = || AgentEvent::Steered {
        assistant_message_id: None,
        next_assistant_message_id: None,
    };
    let dir = tempfile::tempdir().unwrap();
    let registry = HarnessRegistry::new();
    registry.register(Arc::new(BackgroundHarness {
        starts: Arc::new(AtomicUsize::new(0)),
        interrupted: Arc::new(AtomicBool::new(false)),
        callback_script: Some(vec![
            child("one", start()),
            child("one", start()),
            child("two", start()),
            AgentEvent::TextDelta {
                text: "Waiting for the children".into(),
            },
            done(),
            child("one", done()),
            child("one", done()),
            child("two", start()),
            child("two", done()),
        ]),
    }));
    let core = EngineCore::assemble(dir.path(), Arc::new(registry), HarnessId::Mock, None).unwrap();
    core.sessions
        .dispatch(CHAT, HarnessId::Mock, run_request("fan out"), None)
        .await
        .unwrap();
    let idle_with = |count| {
        core.sessions
            .session_status(CHAT)
            .is_some_and(|s| s.status == SessionStatus::Idle && s.running_subagents == count)
    };
    wait_for("two silent children with main idle", || idle_with(2)).await;
    let completion = core
        .sessions
        .session_status(CHAT)
        .unwrap()
        .last_completed_turn;
    wait_for("one child completes", || idle_with(1)).await;
    wait_for("all children complete", || idle_with(0)).await;
    assert_eq!(
        core.sessions
            .session_status(CHAT)
            .unwrap()
            .last_completed_turn,
        completion,
        "child lifecycle cannot fabricate another main turn"
    );
    core.shutdown().await;
}
