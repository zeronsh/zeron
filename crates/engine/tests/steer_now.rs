//! A steer is read at once. While the agent runs a blocking command (a
//! `sleep`, a test run, a wait) a step-boundary agent would only read the
//! steer once the command returned, so the engine stops the turn in place
//! first: the command ends, the runtime and its background work stay, and
//! the steer starts the next turn in that same runtime. Steers sent before
//! it, which the stop cancels, go again first. Agents that haven't opted
//! in (`Harness::steer_stops_tool`) keep the step-boundary wait, and so does
//! a message from another chat.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;
use tokio::sync::mpsc;

use zeron_doc::{MessageStatus, SessionCommandPayload};
use zeron_engine::{EngineCore, HarnessRegistry};
use zeron_harness::{Harness, HarnessError, RunControls, TurnControl};
use zeron_proto::{
    AgentEvent, DoneStatus, HarnessId, Model, ReasoningLevel, RunRequest, SandboxLevel,
    SessionStatus, SteeringMode, ToolCall,
};

const CHAT: &str = "chat-steer-now";

#[derive(Default)]
struct Ledger {
    spawns: AtomicUsize,
    kills: AtomicUsize,
    /// Turn stops the runtime took in place.
    stops: AtomicUsize,
    /// Every prompt the agent read, in order.
    read: Mutex<Vec<String>>,
    turn: Mutex<Option<TurnControl>>,
    /// Ends the open command.
    release: tokio::sync::Notify,
}

/// One persistent agent process per `run()`, reading steers at step
/// boundaries. A prompt containing `tool` runs a command that stays open
/// until released or stopped; steers that arrive meanwhile wait for it.
struct StepAgent {
    ledger: Arc<Ledger>,
    stops_tool: bool,
}

fn done(status: DoneStatus) -> AgentEvent {
    AgentEvent::Done {
        status,
        result: None,
        error: None,
        session_id: Some("provider-session".into()),
    }
}

#[async_trait]
impl Harness for StepAgent {
    fn id(&self) -> HarnessId {
        HarnessId::Mock
    }
    fn display_name(&self) -> &str {
        "Step agent"
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
    fn deterministic_turn_end(&self) -> bool {
        true
    }
    fn stops_turn_in_place(&self) -> bool {
        true
    }
    fn steer_stops_tool(&self) -> bool {
        self.stops_tool
    }
    async fn models(&self) -> Result<Vec<Model>, HarnessError> {
        Ok(vec![])
    }
    async fn run(
        &self,
        request: RunRequest,
        mut controls: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        let ledger = self.ledger.clone();
        ledger.spawns.fetch_add(1, Ordering::SeqCst);
        *ledger.turn.lock().unwrap() = Some(controls.turn.clone());
        let (tx, rx) = mpsc::channel::<Result<AgentEvent, HarnessError>>(64);
        tokio::spawn(async move {
            let turn = controls.turn.clone();
            let send = |event: AgentEvent| {
                let tx = tx.clone();
                async move { tx.send(Ok(event)).await.is_ok() }
            };
            let _ = send(AgentEvent::SessionStarted {
                harness: HarnessId::Mock,
                model: "step".into(),
                tools: vec![],
                cwd: request.cwd.clone(),
                session_id: "provider-session".into(),
                assistant_message_id: "a0".into(),
            })
            .await;
            let mut next = Some(request.prompt.clone());
            let mut calls = 0;
            loop {
                let prompt = match next.take() {
                    Some(prompt) => prompt,
                    // Parked: the next prompt comes by the mailbox.
                    None => tokio::select! {
                        _ = controls.interrupt.cancelled() => {
                            ledger.kills.fetch_add(1, Ordering::SeqCst);
                            return;
                        }
                        steer = controls.steering.recv() => {
                            let Some(steer) = steer else { return };
                            if !send(AgentEvent::Steered {
                                assistant_message_id: None,
                                next_assistant_message_id: None,
                            })
                            .await
                            {
                                return;
                            }
                            steer.prompt
                        }
                    },
                };
                ledger.read.lock().unwrap().push(prompt.clone());
                let _ = send(AgentEvent::TextDelta {
                    text: format!("on it: {prompt}"),
                })
                .await;
                if !prompt.contains("tool") {
                    if !send(done(DoneStatus::Completed)).await {
                        return;
                    }
                    continue;
                }
                calls += 1;
                let id = format!("call-{calls}");
                let _ = send(AgentEvent::ToolCall {
                    id: id.clone(),
                    call: ToolCall::Exec {
                        command: "sleep 120".into(),
                    },
                })
                .await;
                let mut held = Vec::new();
                let stopped = loop {
                    tokio::select! {
                        biased;
                        _ = controls.interrupt.cancelled() => {
                            ledger.kills.fetch_add(1, Ordering::SeqCst);
                            let _ = send(done(DoneStatus::Interrupted)).await;
                            return;
                        }
                        _ = turn.stop_requested() => break true,
                        _ = ledger.release.notified() => break false,
                        steer = controls.steering.recv() => {
                            let Some(steer) = steer else { return };
                            held.push(steer.prompt);
                        }
                    }
                };
                let _ = send(AgentEvent::ToolResult {
                    id,
                    is_error: stopped,
                    output: None,
                    diff: None,
                    exit_code: None,
                })
                .await;
                if stopped {
                    // Steers held by the stopped turn are cancelled with it.
                    ledger.stops.fetch_add(1, Ordering::SeqCst);
                    if !send(done(DoneStatus::Interrupted)).await {
                        return;
                    }
                    continue;
                }
                for prompt in held {
                    let _ = send(AgentEvent::Steered {
                        assistant_message_id: None,
                        next_assistant_message_id: None,
                    })
                    .await;
                    ledger.read.lock().unwrap().push(prompt);
                }
                if !send(done(DoneStatus::Completed)).await {
                    return;
                }
            }
        });
        Ok(
            futures::stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|e| (e, rx)) })
                .boxed(),
        )
    }
}

struct Rig {
    core: EngineCore,
    ledger: Arc<Ledger>,
    _dir: tempfile::TempDir,
}

fn rig(stops_tool: bool) -> Rig {
    let ledger = Arc::new(Ledger::default());
    let registry = HarnessRegistry::new();
    registry.register(Arc::new(StepAgent {
        ledger: ledger.clone(),
        stops_tool,
    }));
    let dir = tempfile::tempdir().unwrap();
    let core = EngineCore::assemble(dir.path(), Arc::new(registry), HarnessId::Mock, None)
        .expect("engine core assembles");
    Rig {
        core,
        ledger,
        _dir: dir,
    }
}

fn request(prompt: &str) -> RunRequest {
    RunRequest {
        mcp: None,
        env: Default::default(),
        prompt: prompt.into(),
        harness: Some(HarnessId::Mock),
        model: Some("step-1".into()),
        reasoning: Some(ReasoningLevel::Medium),
        model_options: Default::default(),
        cwd: "/tmp".into(),
        sandbox: SandboxLevel::WorkspaceWrite,
        auto_approve: false,
        access: Default::default(),
        plan: false,
        attachments: Vec::new(),
        worktree: None,
        resume: None,
        extra_mcp: Vec::new(),
    }
}

fn status(core: &EngineCore) -> Option<SessionStatus> {
    core.sessions.session_status(CHAT).map(|s| s.status)
}

async fn wait_for(mut predicate: impl FnMut() -> bool, what: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !predicate() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn read(rig: &Rig) -> Vec<String> {
    rig.ledger.read.lock().unwrap().clone()
}

fn steer(rig: &Rig, prompt: &str, id: &str) {
    rig.core
        .doc_host
        .queue_command(
            CHAT,
            SessionCommandPayload::Steer {
                prompt: prompt.into(),
                message_id: Some(id.into()),
            },
        )
        .unwrap();
}

/// A turn running its command, with a background process held.
async fn running_a_command(rig: &Rig) -> TurnControl {
    rig.core
        .sessions
        .dispatch(CHAT, HarnessId::Mock, request("tool: run the suite"), None)
        .await
        .unwrap();
    wait_for(
        || read(rig).len() == 1 && rig.ledger.turn.lock().unwrap().is_some(),
        "the command to run",
    )
    .await;
    let turn = rig.ledger.turn.lock().unwrap().clone().unwrap();
    turn.set_background(1);
    turn
}

#[tokio::test]
async fn a_steer_during_a_command_is_read_at_once_and_keeps_the_runtime() {
    let rig = rig(true);
    let turn = running_a_command(&rig).await;
    wait_for(
        || rig.core.sessions.steer_stops_tool(CHAT),
        "the command to be open",
    )
    .await;
    steer(&rig, "stop and look at the logs", "m-steer");
    wait_for(
        || read(&rig).iter().any(|p| p == "stop and look at the logs"),
        "the steer to be read without the command returning",
    )
    .await;
    wait_for(
        || status(&rig.core) == Some(SessionStatus::Idle),
        "the steered turn to park",
    )
    .await;
    assert_eq!(
        rig.ledger.stops.load(Ordering::SeqCst),
        1,
        "the command's turn stopped in place"
    );
    assert_eq!(rig.ledger.kills.load(Ordering::SeqCst), 0, "no teardown");
    assert_eq!(
        rig.ledger.spawns.load(Ordering::SeqCst),
        1,
        "the same runtime"
    );
    assert!(turn.background_live(), "its background work still runs");
    let doc = rig.core.doc_host.open(CHAT).unwrap();
    let entries = doc.doc().read_entries().unwrap();
    assert!(
        entries
            .iter()
            .any(|e| e.status == Some(MessageStatus::Aborted)),
        "the stopped turn reads as stopped"
    );
    assert_eq!(
        entries.iter().filter(|e| e.id == "m-steer").count(),
        1,
        "the steer is in the transcript once"
    );
    rig.core.sessions.shutdown().await;
}

#[tokio::test]
async fn steers_waiting_for_the_command_go_again_first() {
    let rig = rig(true);
    let _turn = running_a_command(&rig).await;
    wait_for(
        || rig.core.sessions.steer_stops_tool(CHAT),
        "the command to be open",
    )
    .await;
    // An earlier steer, sent the step-boundary way: it waits for the command.
    rig.core
        .sessions
        .steer(CHAT, "first: earlier thought", Some("m-1".into()))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        read(&rig).len(),
        1,
        "the earlier steer waits for the command"
    );
    steer(&rig, "second: now", "m-2");
    wait_for(
        || read(&rig).iter().any(|p| p == "second: now"),
        "the steer to be read",
    )
    .await;
    wait_for(
        || status(&rig.core) == Some(SessionStatus::Idle),
        "the turn to park",
    )
    .await;
    assert_eq!(
        read(&rig),
        vec![
            "tool: run the suite",
            "first: earlier thought",
            "second: now"
        ],
        "both, in the order sent, once each"
    );
    assert_eq!(rig.ledger.kills.load(Ordering::SeqCst), 0);
    rig.core.sessions.shutdown().await;
}

#[tokio::test]
async fn an_agent_that_has_not_opted_in_reads_the_steer_after_the_command() {
    let rig = rig(false);
    let _turn = running_a_command(&rig).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!rig.core.sessions.steer_stops_tool(CHAT));
    steer(&rig, "when you can", "m-late");
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(read(&rig).len(), 1, "it waits for the command");
    assert_eq!(rig.ledger.stops.load(Ordering::SeqCst), 0);
    rig.ledger.release.notify_one();
    wait_for(
        || read(&rig).iter().any(|p| p == "when you can"),
        "the steer at the step boundary",
    )
    .await;
    assert_eq!(rig.ledger.kills.load(Ordering::SeqCst), 0);
    rig.core.sessions.shutdown().await;
}

#[tokio::test]
async fn a_steer_with_no_command_running_stops_nothing() {
    let rig = rig(true);
    let _turn = running_a_command(&rig).await;
    wait_for(
        || rig.core.sessions.steer_stops_tool(CHAT),
        "the command to be open",
    )
    .await;
    rig.ledger.release.notify_one();
    wait_for(
        || status(&rig.core) == Some(SessionStatus::Idle),
        "the turn to end",
    )
    .await;
    assert!(
        !rig.core.sessions.steer_stops_tool(CHAT),
        "nothing open, nothing to stop"
    );
    steer(&rig, "and then", "m-after");
    wait_for(|| read(&rig).iter().any(|p| p == "and then"), "the steer").await;
    assert_eq!(rig.ledger.stops.load(Ordering::SeqCst), 0);
    assert_eq!(rig.ledger.kills.load(Ordering::SeqCst), 0);
    rig.core.sessions.shutdown().await;
}

/// Claude Code tells the agent the person rejected the call and to stop and
/// wait whenever a turn is interrupted, so a message from another chat that
/// stopped the command would read as an order to stop. It waits for the
/// command instead, as `send_message` promises.
#[tokio::test]
async fn a_message_from_another_chat_never_stops_the_command() {
    let rig = rig(true);
    let turn = running_a_command(&rig).await;
    wait_for(
        || rig.core.sessions.steer_stops_tool(CHAT),
        "the command to be open",
    )
    .await;
    let message =
        zeron_proto::agent_messages::agent_message("Coordinator (139d8682)", "139d8682", "status?");
    steer(&rig, &message, "m-agent");
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(read(&rig).len(), 1, "it waits for the command");
    assert_eq!(
        rig.ledger.stops.load(Ordering::SeqCst),
        0,
        "nothing stopped"
    );
    rig.ledger.release.notify_one();
    wait_for(
        || read(&rig).iter().any(|p| p.ends_with("status?")),
        "the message at the step boundary",
    )
    .await;
    wait_for(
        || status(&rig.core) == Some(SessionStatus::Idle),
        "the turn to end",
    )
    .await;
    assert_eq!(rig.ledger.stops.load(Ordering::SeqCst), 0);
    assert_eq!(rig.ledger.kills.load(Ordering::SeqCst), 0);
    assert!(turn.background_live(), "its background work still runs");
    let doc = rig.core.doc_host.open(CHAT).unwrap();
    let entries = doc.doc().read_entries().unwrap();
    assert!(
        !entries
            .iter()
            .any(|e| e.status == Some(MessageStatus::Aborted)),
        "the command's turn was not stopped"
    );
    rig.core.sessions.shutdown().await;
}
