//! A live runtime is the agent's whole process — its background subagents and
//! background shells included. Sending the next message, steering, "send
//! now", switching models, or leaving a session parked must never tear it
//! down behind the user's back (2026-09-29: "sometimes sending a message
//! closes the existing claude session and re-opens it … it kills subagents
//! and sub processes"). The user's Stop is the one deliberate teardown.
//!
//! The harness here models one persistent agent process per `run()`: every
//! `run()` is a spawn, the `interrupt` token is its death, a turn stop ends
//! only the turn, and the test decides when the "background task" finishes.

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
    SessionStatus, SteeringMode,
};

const CHAT: &str = "chat-runtime-survival";

#[derive(Default)]
struct Ledger {
    spawns: AtomicUsize,
    kills: AtomicUsize,
    /// Every prompt the live process received, with its image count.
    prompts: Mutex<Vec<(String, usize)>>,
    /// The latest runtime's turn control (the test flips its background work).
    turn: Mutex<Option<TurnControl>>,
    /// The model each in-place reconfiguration switched the process to.
    reconfigured: Mutex<Vec<Option<String>>>,
    /// The model each process was spawned with.
    spawned_models: Mutex<Vec<Option<String>>>,
    /// Ends a turn whose prompt contains `wait`.
    release: tokio::sync::Notify,
}

/// One persistent agent process per `run()`. A prompt containing `hold`
/// stays in flight until stopped; any other prompt completes at once. A
/// steer containing `vanish` kills the process before it confirms the steer.
/// A prompt containing `wait` runs until the test releases it.
struct ProcessHarness {
    ledger: Arc<Ledger>,
    /// Model changes apply to the live process (`set_model`-style).
    adopts_config: bool,
    /// Takes input only between turns (Hermes, Antigravity): the engine
    /// holds sends made mid-turn for the turn's end.
    turn_boundary: bool,
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
impl Harness for ProcessHarness {
    fn id(&self) -> HarnessId {
        HarnessId::Mock
    }
    fn display_name(&self) -> &str {
        "Process"
    }
    fn supports_steering(&self) -> bool {
        true
    }
    fn steering_mode(&self) -> SteeringMode {
        if self.turn_boundary {
            SteeringMode::TurnBoundary
        } else {
            SteeringMode::StepBoundary
        }
    }
    fn reasoning_levels(&self) -> &[ReasoningLevel] {
        &[ReasoningLevel::Medium, ReasoningLevel::High]
    }
    fn deterministic_turn_end(&self) -> bool {
        true
    }
    fn stops_turn_in_place(&self) -> bool {
        true
    }
    fn reconfigures_in_place(&self, live: &RunRequest, next: &RunRequest) -> bool {
        self.adopts_config && live.cwd == next.cwd
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
        ledger
            .spawned_models
            .lock()
            .unwrap()
            .push(request.model.clone());
        *ledger.turn.lock().unwrap() = Some(controls.turn.clone());
        let (tx, rx) = mpsc::channel::<Result<AgentEvent, HarnessError>>(64);
        tokio::spawn(async move {
            let turn = controls.turn.clone();
            let send = |event: AgentEvent| {
                let tx = tx.clone();
                async move { tx.send(Ok(event)).await.is_ok() }
            };
            let prompt = |text: &str, images: usize| {
                ledger
                    .prompts
                    .lock()
                    .unwrap()
                    .push((text.to_owned(), images));
                text.contains("hold")
            };
            let _ = send(AgentEvent::SessionStarted {
                harness: HarnessId::Mock,
                model: "process".into(),
                tools: vec![],
                cwd: request.cwd.clone(),
                session_id: "provider-session".into(),
                assistant_message_id: "a0".into(),
            })
            .await;
            let holding = prompt(&request.prompt, request.attachments.len());
            let _ = send(AgentEvent::TextDelta {
                text: format!("on it: {}", request.prompt),
            })
            .await;
            let mut waiting = request.prompt.contains("wait");
            if !holding && !waiting && !send(done(DoneStatus::Completed)).await {
                return;
            }
            loop {
                tokio::select! {
                    biased;
                    _ = ledger.release.notified(), if waiting => {
                        waiting = false;
                        if !send(done(DoneStatus::Completed)).await {
                            return;
                        }
                    }
                    _ = controls.interrupt.cancelled() => {
                        // The process dies, and its background work with it.
                        ledger.kills.fetch_add(1, Ordering::SeqCst);
                        let _ = send(done(DoneStatus::Interrupted)).await;
                        return;
                    }
                    _ = turn.stop_requested() => {
                        if !send(done(DoneStatus::Interrupted)).await {
                            return;
                        }
                    }
                    steer = controls.steering.recv() => {
                        let Some(steer) = steer else { return };
                        if steer.prompt.contains("vanish") {
                            return;
                        }
                        if let Some(config) = &steer.config {
                            ledger.reconfigured.lock().unwrap().push(config.model.clone());
                        }
                        let holding = prompt(&steer.prompt, steer.attachments.len());
                        let boundary = AgentEvent::Steered {
                            assistant_message_id: None,
                            next_assistant_message_id: None,
                        };
                        if !send(boundary).await
                            || !send(AgentEvent::TextDelta { text: format!("on it: {}", steer.prompt) }).await
                        {
                            return;
                        }
                        waiting = steer.prompt.contains("wait");
                        if !holding && !waiting && !send(done(DoneStatus::Completed)).await {
                            return;
                        }
                    }
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

fn rig() -> Rig {
    rig_with(false)
}

fn rig_with(adopts_config: bool) -> Rig {
    rig_full(adopts_config, false)
}

fn rig_full(adopts_config: bool, turn_boundary: bool) -> Rig {
    let ledger = Arc::new(Ledger::default());
    let registry = HarnessRegistry::new();
    registry.register(Arc::new(ProcessHarness {
        ledger: ledger.clone(),
        adopts_config,
        turn_boundary,
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
        prompt: prompt.into(),
        harness: Some(HarnessId::Mock),
        model: Some("process-1".into()),
        reasoning: Some(ReasoningLevel::High),
        model_options: serde_json::Map::from_iter([("fastMode".into(), serde_json::json!(true))]),
        cwd: "/tmp".into(),
        sandbox: SandboxLevel::WorkspaceWrite,
        auto_approve: false,
        attachments: Vec::new(),
        worktree: None,
        resume: None,
        resume_policy: Default::default(),
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

async fn settle(rig: &Rig, prompts: usize) {
    wait_for(
        || {
            status(&rig.core) == Some(SessionStatus::Idle)
                && rig.ledger.prompts.lock().unwrap().len() >= prompts
        },
        "the turn to park",
    )
    .await;
}

#[tokio::test]
async fn sends_with_images_or_respelled_options_join_the_live_runtime() {
    let rig = rig();
    rig.core
        .sessions
        .dispatch(CHAT, HarnessId::Mock, request("launch the scouts"), None)
        .await
        .unwrap();
    settle(&rig, 1).await;

    // The phone spells the same option as a string, and the message carries
    // an image: neither is a different runtime.
    let mut follow_up = request("what is in this screenshot?");
    follow_up.model_options =
        serde_json::Map::from_iter([("fastMode".into(), serde_json::json!("true"))]);
    follow_up.sandbox = SandboxLevel::WorkspaceWrite;
    follow_up.attachments = vec!["/tmp/shot.png".into()];
    rig.core
        .sessions
        .dispatch(CHAT, HarnessId::Mock, follow_up, None)
        .await
        .unwrap();
    settle(&rig, 2).await;

    // Through the durable command plane too (the desktop/mobile send path).
    rig.core
        .doc_host
        .queue_command(
            CHAT,
            SessionCommandPayload::Run {
                request: request("and one more"),
                message_id: "m-3".into(),
            },
        )
        .unwrap();
    settle(&rig, 3).await;

    assert_eq!(rig.ledger.spawns.load(Ordering::SeqCst), 1, "one process");
    assert_eq!(rig.ledger.kills.load(Ordering::SeqCst), 0);
    assert_eq!(
        rig.ledger.prompts.lock().unwrap()[1],
        ("what is in this screenshot?".to_string(), 1),
        "the image rode the mailbox"
    );

    // A real configuration change still gets a fresh runtime.
    let mut other_model = request("switch models");
    other_model.model = Some("process-2".into());
    rig.core
        .sessions
        .dispatch(CHAT, HarnessId::Mock, other_model, None)
        .await
        .unwrap();
    settle(&rig, 4).await;
    assert_eq!(rig.ledger.spawns.load(Ordering::SeqCst), 2);
    rig.core.sessions.shutdown().await;
}

/// Switching models mid-chat on a driver that adopts configuration in place
/// (Claude's `set_model`/`apply_flag_settings`, Codex's per-turn params)
/// keeps the process — and every background subagent and shell it holds.
#[tokio::test]
async fn a_model_switch_is_adopted_by_the_live_runtime() {
    let rig = rig_with(true);
    rig.core
        .sessions
        .dispatch(CHAT, HarnessId::Mock, request("launch the scouts"), None)
        .await
        .unwrap();
    settle(&rig, 1).await;

    let mut other_model = request("switch models");
    other_model.model = Some("process-2".into());
    other_model.reasoning = Some(ReasoningLevel::Medium);
    rig.core
        .sessions
        .dispatch(CHAT, HarnessId::Mock, other_model, None)
        .await
        .unwrap();
    settle(&rig, 2).await;

    // The same model again is the (now reconfigured) same runtime: no
    // second reconfiguration rides along.
    let mut again = request("still on the new model");
    again.model = Some("process-2".into());
    again.reasoning = Some(ReasoningLevel::Medium);
    rig.core
        .sessions
        .dispatch(CHAT, HarnessId::Mock, again, None)
        .await
        .unwrap();
    settle(&rig, 3).await;

    assert_eq!(rig.ledger.spawns.load(Ordering::SeqCst), 1, "one process");
    assert_eq!(rig.ledger.kills.load(Ordering::SeqCst), 0);
    assert_eq!(
        *rig.ledger.reconfigured.lock().unwrap(),
        vec![Some("process-2".to_string())]
    );

    // A different directory still needs a different process.
    let mut elsewhere = request("over there");
    elsewhere.cwd = "/var".into();
    rig.core
        .sessions
        .dispatch(CHAT, HarnessId::Mock, elsewhere, None)
        .await
        .unwrap();
    settle(&rig, 4).await;
    assert_eq!(rig.ledger.spawns.load(Ordering::SeqCst), 2);
    rig.core.sessions.shutdown().await;
}

/// A message the runtime accepted but died before confirming is replayed as
/// it was sent — its images and configuration included, under its own id —
/// not as the chat's latest request with the images stripped.
#[tokio::test]
async fn a_message_orphaned_by_a_dying_runtime_replays_as_sent() {
    let rig = rig_with(true);
    rig.core
        .sessions
        .dispatch(CHAT, HarnessId::Mock, request("hold: working"), None)
        .await
        .unwrap();
    wait_for(
        || rig.ledger.prompts.lock().unwrap().len() == 1,
        "the agent to be running the turn",
    )
    .await;

    let mut sent = request("vanish: what is in this screenshot?");
    sent.model = Some("process-2".into());
    sent.attachments = vec!["/tmp/shot.png".into()];
    rig.core
        .sessions
        .dispatch(CHAT, HarnessId::Mock, sent, Some("m-orphan".into()))
        .await
        .unwrap();
    settle(&rig, 2).await;

    assert_eq!(
        rig.ledger.spawns.load(Ordering::SeqCst),
        2,
        "a fresh runtime"
    );
    assert_eq!(
        rig.ledger.prompts.lock().unwrap()[1],
        ("vanish: what is in this screenshot?".to_string(), 1),
        "the image survived the replay"
    );
    assert_eq!(
        rig.ledger.spawned_models.lock().unwrap()[1].as_deref(),
        Some("process-2"),
        "the configuration it was sent with"
    );
    let entries = rig
        .core
        .doc_host
        .open(CHAT)
        .unwrap()
        .doc()
        .read_entries()
        .unwrap();
    let orphan: Vec<_> = entries
        .iter()
        .filter(|e| e.role == zeron_doc::MessageRole::User && e.id != entries[0].id)
        .map(|e| e.id.as_str())
        .collect();
    assert_eq!(orphan, ["m-orphan"], "one entry, under its own id");
    rig.core.sessions.shutdown().await;
}

/// Every distinct completion ping the chat publishes, recorded until the
/// returned handle is aborted.
fn record_pings(rig: &Rig) -> (Arc<Mutex<Vec<String>>>, tokio::task::JoinHandle<()>) {
    let pings = Arc::new(Mutex::new(Vec::<String>::new()));
    let sessions = rig.core.sessions.clone();
    let seen = pings.clone();
    let task = tokio::spawn(async move {
        loop {
            if let Some(ping) = sessions
                .session_status(CHAT)
                .and_then(|s| s.last_completed_turn)
            {
                let mut seen = seen.lock().unwrap();
                if seen.last() != Some(&ping) {
                    seen.push(ping);
                }
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    });
    (pings, task)
}

fn queued(rig: &Rig) -> Vec<String> {
    rig.core
        .doc_host
        .open(CHAT)
        .unwrap()
        .watch_queue()
        .borrow()
        .iter()
        .map(|row| row.text.clone())
        .collect()
}

/// A send to a turn-boundary agent mid-turn waits for the turn's end — and
/// then runs as it was sent (its image, its model), with no "done" ping for
/// the turn that hands over to it.
#[tokio::test]
async fn a_send_held_for_the_turn_end_runs_as_sent_without_a_ping() {
    let rig = rig_full(true, true);
    let (pings, recorder) = record_pings(&rig);
    rig.core
        .sessions
        .dispatch(CHAT, HarnessId::Mock, request("wait: first"), None)
        .await
        .unwrap();
    wait_for(
        || rig.ledger.prompts.lock().unwrap().len() == 1,
        "the first turn to run",
    )
    .await;
    let mut sent = request("second");
    sent.model = Some("process-2".into());
    sent.attachments = vec!["/tmp/shot.png".into()];
    rig.core
        .doc_host
        .queue_command(
            CHAT,
            SessionCommandPayload::Run {
                request: sent,
                message_id: "m-held".into(),
            },
        )
        .unwrap();
    wait_for(|| queued(&rig) == ["second"], "the send to be held").await;

    rig.ledger.release.notify_one();
    settle(&rig, 2).await;
    wait_for(
        || {
            rig.core
                .sessions
                .session_status(CHAT)
                .and_then(|s| s.last_completed_turn)
                .is_some()
        },
        "the held send's reply to complete",
    )
    .await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    recorder.abort();
    assert_eq!(
        rig.ledger.prompts.lock().unwrap()[1],
        ("second".to_string(), 1),
        "the held send kept its image"
    );
    assert_eq!(
        *rig.ledger.reconfigured.lock().unwrap(),
        vec![Some("process-2".to_string())],
        "the held send ran on the model it was sent with"
    );
    assert_eq!(rig.ledger.spawns.load(Ordering::SeqCst), 1);
    let last_reply = rig
        .core
        .doc_host
        .open(CHAT)
        .unwrap()
        .doc()
        .read_entries()
        .unwrap()
        .last()
        .map(|e| e.id.clone())
        .unwrap();
    assert_eq!(
        *pings.lock().unwrap(),
        vec![last_reply],
        "only the held send's reply pings: the handover to it is not a completion"
    );
    rig.core.sessions.shutdown().await;
}

/// A Stop cancels what was sent during the turn it stops: a send held for
/// that turn's end never runs — not even after the user's next message
/// thaws the queue.
#[tokio::test]
async fn stop_cancels_sends_held_for_the_stopped_turn() {
    let rig = rig_full(false, true);
    rig.core
        .sessions
        .dispatch(CHAT, HarnessId::Mock, request("wait: first"), None)
        .await
        .unwrap();
    wait_for(
        || rig.ledger.prompts.lock().unwrap().len() == 1,
        "the first turn to run",
    )
    .await;
    rig.core
        .doc_host
        .queue_command(
            CHAT,
            SessionCommandPayload::Run {
                request: request("held before the stop"),
                message_id: "m-held".into(),
            },
        )
        .unwrap();
    wait_for(
        || queued(&rig) == ["held before the stop"],
        "the send to be held",
    )
    .await;
    rig.core
        .doc_host
        .queue_command(CHAT, SessionCommandPayload::Interrupt {})
        .unwrap();
    wait_for(
        || {
            rig.ledger.kills.load(Ordering::SeqCst) == 1
                && status(&rig.core) == Some(SessionStatus::Idle)
                && queued(&rig).is_empty()
        },
        "the stop to cancel the held send",
    )
    .await;
    rig.core
        .doc_host
        .queue_command(
            CHAT,
            SessionCommandPayload::Run {
                request: request("after the stop"),
                message_id: "m-after".into(),
            },
        )
        .unwrap();
    wait_for(
        || {
            rig.ledger
                .prompts
                .lock()
                .unwrap()
                .iter()
                .any(|(p, _)| p == "after the stop")
        },
        "the next message to run",
    )
    .await;
    settle(&rig, 2).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        !rig.ledger
            .prompts
            .lock()
            .unwrap()
            .iter()
            .any(|(p, _)| p.contains("held before the stop")),
        "a send held for the stopped turn ran after the Stop"
    );
    rig.core.sessions.shutdown().await;
}

/// A message sent before a Stop whose dispatch only lands once the Stop
/// has torn the runtime down (a loaded host, a slow agent) was cancelled by
/// it: it must not start the stopped work again in a fresh runtime.
#[tokio::test]
async fn a_message_the_stop_cancelled_never_reaches_a_fresh_runtime() {
    let rig = rig();
    rig.core
        .sessions
        .dispatch(CHAT, HarnessId::Mock, request("hold: working"), None)
        .await
        .unwrap();
    wait_for(
        || rig.ledger.prompts.lock().unwrap().len() == 1,
        "the agent to be running the turn",
    )
    .await;
    // The Stop acts with the message still on its way …
    rig.core
        .sessions
        .cancel_sent_messages(CHAT, vec!["m-late".into()]);
    rig.core.sessions.interrupt(CHAT).await.unwrap();
    // … and the message reaches dispatch only now.
    rig.core
        .sessions
        .dispatch(
            CHAT,
            HarnessId::Mock,
            request("hold: sent before the stop"),
            Some("m-late".into()),
        )
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        rig.ledger.spawns.load(Ordering::SeqCst),
        1,
        "no fresh runtime"
    );
    assert_eq!(status(&rig.core), Some(SessionStatus::Idle));
    // The user's next message still runs.
    rig.core
        .sessions
        .dispatch(
            CHAT,
            HarnessId::Mock,
            request("after the stop"),
            Some("m-next".into()),
        )
        .await
        .unwrap();
    settle(&rig, 2).await;
    assert_eq!(rig.ledger.prompts.lock().unwrap()[1].0, "after the stop");
    rig.core.sessions.shutdown().await;
}

/// "Send now" replaces the in-flight turn but keeps the runtime and its
/// background work; the user's Stop is a hard boundary that tears all of it
/// down, and the next message starts a fresh process.
#[tokio::test]
async fn send_now_keeps_the_runtime_and_stop_tears_it_down() {
    let rig = rig();
    rig.core
        .sessions
        .dispatch(CHAT, HarnessId::Mock, request("hold: build it"), None)
        .await
        .unwrap();
    wait_for(
        || rig.ledger.turn.lock().unwrap().is_some(),
        "the runtime to start",
    )
    .await;
    let turn = rig.ledger.turn.lock().unwrap().clone().unwrap();
    turn.set_background(1);
    wait_for(
        || status(&rig.core) == Some(SessionStatus::Working),
        "the turn to run",
    )
    .await;

    // Send now: the turn ends in place.
    assert!(rig.core.sessions.stop_turn(CHAT).await.unwrap());
    assert_eq!(status(&rig.core), Some(SessionStatus::Idle));
    let doc = rig.core.doc_host.open(CHAT).unwrap();
    assert!(
        doc.doc()
            .read_entries()
            .unwrap()
            .iter()
            .any(|entry| entry.status == Some(MessageStatus::Aborted)),
        "the replaced turn reads as stopped"
    );
    assert_eq!(rig.ledger.kills.load(Ordering::SeqCst), 0, "no teardown");
    assert!(
        !rig.core.sessions.stop_turn(CHAT).await.unwrap(),
        "a parked runtime has no turn to stop"
    );
    rig.core
        .sessions
        .dispatch(CHAT, HarnessId::Mock, request("hold: again"), None)
        .await
        .unwrap();
    wait_for(
        || {
            status(&rig.core) == Some(SessionStatus::Working)
                && rig.ledger.prompts.lock().unwrap().len() == 2
        },
        "the second turn to run",
    )
    .await;
    assert_eq!(rig.ledger.spawns.load(Ordering::SeqCst), 1, "one process");
    assert!(turn.background_live());

    // Stop (the Cancel command): the runtime and its work are gone.
    rig.core
        .doc_host
        .queue_command(CHAT, SessionCommandPayload::Interrupt {})
        .unwrap();
    wait_for(
        || status(&rig.core) == Some(SessionStatus::Idle),
        "stop to settle",
    )
    .await;
    wait_for(
        || rig.ledger.kills.load(Ordering::SeqCst) == 1,
        "the runtime to be torn down",
    )
    .await;
    assert!(!rig.core.sessions.live_run_steerable(CHAT));

    // The next message starts a fresh process.
    rig.core
        .sessions
        .dispatch(CHAT, HarnessId::Mock, request("status?"), None)
        .await
        .unwrap();
    settle(&rig, 3).await;
    assert_eq!(rig.ledger.spawns.load(Ordering::SeqCst), 2);
    rig.core.sessions.shutdown().await;
}

/// Stop pressed right after Send: controls bypass the prompt drain, and the
/// Stop used to run before the send had dispatched — it found no turn, did
/// nothing, and the turn then ran in full.
#[tokio::test]
async fn a_stop_right_after_send_stops_that_turn() {
    let rig = rig();
    rig.core
        .doc_host
        .queue_command(
            CHAT,
            SessionCommandPayload::Run {
                request: request("hold: the essay"),
                message_id: "m-1".into(),
            },
        )
        .unwrap();
    rig.core
        .doc_host
        .queue_command(CHAT, SessionCommandPayload::Interrupt {})
        .unwrap();
    wait_for(
        || rig.ledger.kills.load(Ordering::SeqCst) == 1,
        "the Stop to tear the turn down",
    )
    .await;
    wait_for(
        || status(&rig.core) == Some(SessionStatus::Idle),
        "the stopped chat to settle",
    )
    .await;
    assert_eq!(rig.ledger.prompts.lock().unwrap().len(), 1);
    rig.core.sessions.shutdown().await;
}

/// Send pressed right after Stop: it used to land in the mailbox of the
/// runtime the Stop was tearing down, and an interrupted run re-dispatches
/// nothing — the message showed as sent and never ran.
#[tokio::test]
async fn a_send_right_after_stop_runs_in_a_fresh_runtime() {
    let rig = rig();
    rig.core
        .sessions
        .dispatch(CHAT, HarnessId::Mock, request("hold: working"), None)
        .await
        .unwrap();
    wait_for(
        || rig.ledger.prompts.lock().unwrap().len() == 1,
        "the agent to be running the turn",
    )
    .await;
    rig.core
        .doc_host
        .queue_command(CHAT, SessionCommandPayload::Interrupt {})
        .unwrap();
    rig.core
        .doc_host
        .queue_command(
            CHAT,
            SessionCommandPayload::Run {
                request: request("after the stop"),
                message_id: "m-2".into(),
            },
        )
        .unwrap();
    wait_for(
        || {
            rig.ledger
                .prompts
                .lock()
                .unwrap()
                .iter()
                .any(|(p, _)| p == "after the stop")
        },
        "the message sent after Stop to run",
    )
    .await;
    settle(&rig, 2).await;
    assert_eq!(rig.ledger.kills.load(Ordering::SeqCst), 1);
    assert_eq!(
        rig.ledger.spawns.load(Ordering::SeqCst),
        2,
        "a fresh runtime"
    );
    rig.core.sessions.shutdown().await;
}

/// The raw race under the command plane: a routed send accepted into a
/// runtime after its Stop began is re-dispatched, never dropped.
#[tokio::test]
async fn a_send_accepted_by_a_stopping_runtime_is_redispatched() {
    let rig = rig();
    rig.core
        .sessions
        .dispatch(CHAT, HarnessId::Mock, request("hold: working"), None)
        .await
        .unwrap();
    wait_for(
        || rig.ledger.prompts.lock().unwrap().len() == 1,
        "the agent to be running the turn",
    )
    .await;
    let (stop, send) = tokio::join!(
        rig.core.sessions.interrupt(CHAT),
        rig.core
            .sessions
            .dispatch(CHAT, HarnessId::Mock, request("racing the stop"), None)
    );
    stop.unwrap();
    send.unwrap();
    wait_for(
        || {
            rig.ledger
                .prompts
                .lock()
                .unwrap()
                .iter()
                .filter(|(p, _)| p == "racing the stop")
                .count()
                >= 1
                && status(&rig.core) == Some(SessionStatus::Idle)
        },
        "the racing message to run",
    )
    .await;
    assert_eq!(
        rig.ledger.kills.load(Ordering::SeqCst),
        1,
        "the Stop tore it down"
    );
    rig.core.sessions.shutdown().await;
}

/// A steer sent before Stop is cancelled with the turn: it must never run
/// later in the fresh runtime as if it were the user's next message.
#[tokio::test]
async fn a_steer_sent_before_stop_is_cancelled_with_it() {
    let rig = rig();
    rig.core
        .sessions
        .dispatch(CHAT, HarnessId::Mock, request("hold: working"), None)
        .await
        .unwrap();
    wait_for(
        || rig.ledger.prompts.lock().unwrap().len() == 1,
        "the agent to be running the turn",
    )
    .await;
    rig.core
        .doc_host
        .queue_command(
            CHAT,
            SessionCommandPayload::Run {
                request: request("hold: steer before stop"),
                message_id: "m-steer".into(),
            },
        )
        .unwrap();
    rig.core
        .doc_host
        .queue_command(CHAT, SessionCommandPayload::Interrupt {})
        .unwrap();
    wait_for(
        || {
            rig.ledger.kills.load(Ordering::SeqCst) == 1
                && status(&rig.core) == Some(SessionStatus::Idle)
        },
        "the stop to settle",
    )
    .await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(
        rig.ledger.spawns.load(Ordering::SeqCst),
        1,
        "nothing re-ran"
    );
    assert_eq!(status(&rig.core), Some(SessionStatus::Idle));
    rig.core.sessions.shutdown().await;
}

/// The idle reaper spares a parked runtime holding background work, so the
/// user's Stop must reach it and end that work.
#[tokio::test]
async fn stop_ends_a_parked_runtime_holding_background_work() {
    let rig = rig();
    rig.core
        .sessions
        .dispatch(
            CHAT,
            HarnessId::Mock,
            request("launch a background scout"),
            None,
        )
        .await
        .unwrap();
    settle(&rig, 1).await;
    // A runtime parked with nothing running is left alone.
    rig.core
        .doc_host
        .queue_command(CHAT, SessionCommandPayload::Interrupt {})
        .unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(rig.ledger.kills.load(Ordering::SeqCst), 0);

    let turn = rig.ledger.turn.lock().unwrap().clone().unwrap();
    turn.set_background(1);
    rig.core
        .doc_host
        .queue_command(CHAT, SessionCommandPayload::Interrupt {})
        .unwrap();
    wait_for(
        || rig.ledger.kills.load(Ordering::SeqCst) == 1,
        "the Stop to end the background work",
    )
    .await;
    assert!(!rig.core.sessions.live_run_steerable(CHAT));
    rig.core.sessions.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn the_idle_reaper_spares_a_runtime_holding_background_work() {
    let rig = rig();
    rig.core
        .sessions
        .dispatch(
            CHAT,
            HarnessId::Mock,
            request("launch a background scout"),
            None,
        )
        .await
        .unwrap();
    settle(&rig, 1).await;
    let turn = rig.ledger.turn.lock().unwrap().clone().unwrap();
    turn.set_background(1);

    // Far past the 30-minute idle window: the scout is still running. (The
    // odd 20 minutes put its end mid-way through a deferred window.)
    tokio::time::sleep(Duration::from_secs(3 * 60 * 60 + 20 * 60)).await;
    assert!(
        rig.core.sessions.live_run_steerable(CHAT),
        "the reaper killed a runtime holding live background work"
    );
    assert_eq!(rig.ledger.kills.load(Ordering::SeqCst), 0);

    // Once the background work is done, an idle runtime is released again —
    // but not before a full idle window has passed since: the completion's
    // wake turn starts a moment after the agent reports the work drained.
    turn.set_background(0);
    tokio::time::sleep(Duration::from_secs(29 * 60)).await;
    assert!(
        rig.core.sessions.live_run_steerable(CHAT),
        "the reaper killed the runtime as its background work ended"
    );
    tokio::time::sleep(Duration::from_secs(2 * 60 * 60)).await;
    wait_for(
        || rig.ledger.kills.load(Ordering::SeqCst) == 1,
        "the idle runtime to be reaped",
    )
    .await;
    rig.core.sessions.shutdown().await;
}
