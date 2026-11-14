//! An engine that starts over chats whose agents were parked idle starts no
//! agent for them: each is resumed when its next message comes (ademark A5,
//! "seeded state, just launched, no agent running"). Only a turn that was
//! cut off mid-way is real work, and that one is picked back up.
//!
//! The harness is a persistent agent per `run()` (a spawn): it answers a
//! prompt and stays up, parked, for the next one; a prompt with `hold` stays
//! in flight.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;
use tokio::sync::mpsc;

use zeron_engine::{EngineCore, HarnessRegistry};
use zeron_harness::{Harness, HarnessError, RunControls};
use zeron_proto::{
    AgentEvent, DoneStatus, HarnessId, Model, ReasoningLevel, RunRequest, SandboxLevel,
    SessionStatus, SteeringMode,
};

#[derive(Default)]
struct Ledger {
    spawns: AtomicUsize,
    /// Each spawn: its chat's first prompt and its `resume`.
    runs: Mutex<Vec<(String, Option<String>)>>,
}

struct Agent {
    ledger: Arc<Ledger>,
}

fn done() -> AgentEvent {
    AgentEvent::Done {
        status: DoneStatus::Completed,
        result: None,
        error: None,
        session_id: Some("provider-session".into()),
    }
}

#[async_trait]
impl Harness for Agent {
    fn id(&self) -> HarnessId {
        HarnessId::Mock
    }
    fn display_name(&self) -> &str {
        "Agent"
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
    async fn models(&self) -> Result<Vec<Model>, HarnessError> {
        Ok(vec![])
    }
    async fn run(
        &self,
        request: RunRequest,
        mut controls: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        self.ledger.spawns.fetch_add(1, Ordering::SeqCst);
        self.ledger
            .runs
            .lock()
            .unwrap()
            .push((request.prompt.clone(), request.resume.clone()));
        let (tx, rx) = mpsc::channel::<Result<AgentEvent, HarnessError>>(64);
        tokio::spawn(async move {
            let send = |event: AgentEvent| {
                let tx = tx.clone();
                async move { tx.send(Ok(event)).await.is_ok() }
            };
            let _ = send(AgentEvent::SessionStarted {
                harness: HarnessId::Mock,
                model: "agent".into(),
                tools: vec![],
                cwd: request.cwd.clone(),
                session_id: "provider-session".into(),
                assistant_message_id: "a0".into(),
            })
            .await;
            let _ = send(AgentEvent::TextDelta {
                text: format!("on it: {}", request.prompt),
            })
            .await;
            if !request.prompt.contains("hold") && !send(done()).await {
                return;
            }
            loop {
                tokio::select! {
                    _ = controls.interrupt.cancelled() => {
                        let _ = tx.send(Ok(AgentEvent::Done {
                            status: DoneStatus::Interrupted,
                            result: None,
                            error: None,
                            session_id: None,
                        })).await;
                        return;
                    }
                    steer = controls.steering.recv() => {
                        let Some(steer) = steer else { return };
                        let boundary = AgentEvent::Steered {
                            assistant_message_id: None,
                            next_assistant_message_id: None,
                        };
                        if !send(boundary).await
                            || !send(AgentEvent::TextDelta { text: format!("on it: {}", steer.prompt) }).await
                            || !send(done()).await
                        {
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

fn assemble(dir: &std::path::Path, ledger: &Arc<Ledger>) -> EngineCore {
    let registry = HarnessRegistry::new();
    registry.register(Arc::new(Agent {
        ledger: ledger.clone(),
    }));
    let core = EngineCore::assemble(dir, Arc::new(registry), HarnessId::Mock, None)
        .expect("engine core assembles");
    core.sessions.set_ipc_port(27699);
    core
}

fn request(prompt: &str) -> RunRequest {
    RunRequest {
        mcp: None,
        env: Default::default(),
        prompt: prompt.into(),
        harness: Some(HarnessId::Mock),
        model: Some("agent-1".into()),
        reasoning: Some(ReasoningLevel::Medium),
        model_options: Default::default(),
        cwd: "/tmp".into(),
        sandbox: SandboxLevel::WorkspaceWrite,
        auto_approve: true,
        access: Default::default(),
        plan: false,
        attachments: Vec::new(),
        worktree: None,
        resume: None,
        extra_mcp: Vec::new(),
    }
}

async fn wait_for(mut predicate: impl FnMut() -> bool, what: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while !predicate() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

const SEEDED: [&str; 5] = ["seed-0", "seed-1", "seed-2", "seed-3", "seed-4"];

/// Five chats, each with a finished turn and its agent parked.
async fn seed(core: &EngineCore, ledger: &Ledger) {
    for chat in SEEDED {
        core.sessions
            .dispatch(chat, HarnessId::Mock, request("seed prompt"), None)
            .await
            .unwrap();
    }
    wait_for(
        || {
            SEEDED.iter().all(|chat| {
                core.sessions.session_status(chat).map(|s| s.status) == Some(SessionStatus::Idle)
            }) && ledger.spawns.load(Ordering::SeqCst) == SEEDED.len()
        },
        "the seeded turns to park",
    )
    .await;
    assert!(
        SEEDED
            .iter()
            .all(|chat| core.sessions.live_run_steerable(chat))
    );
}

/// Launch: recover, then three minutes with nothing sent.
async fn launch(dir: &std::path::Path) -> (EngineCore, Arc<Ledger>) {
    let ledger = Arc::new(Ledger::default());
    let core = assemble(dir, &ledger);
    core.sessions.recover_stale().unwrap();
    tokio::time::sleep(Duration::from_secs(3)).await;
    (core, ledger)
}

/// The next message, once launched: one agent, resuming the chat's session.
async fn next_message_resumes(core: &EngineCore, ledger: &Ledger) {
    core.sessions
        .dispatch("seed-2", HarnessId::Mock, request("next message"), None)
        .await
        .unwrap();
    wait_for(
        || ledger.spawns.load(Ordering::SeqCst) == 1,
        "the next message's agent",
    )
    .await;
    assert_eq!(
        ledger.runs.lock().unwrap()[0],
        (
            "next message".to_owned(),
            Some("provider-session".to_owned())
        ),
        "it resumes the chat's own session"
    );
}

#[tokio::test]
async fn parked_agents_are_not_restarted_after_a_shutdown() {
    let tmp = tempfile::tempdir().unwrap();
    {
        let ledger = Arc::new(Ledger::default());
        let core = assemble(tmp.path(), &ledger);
        seed(&core, &ledger).await;
        core.shutdown().await;
    }
    let (core, ledger) = launch(tmp.path()).await;
    assert_eq!(
        ledger.spawns.load(Ordering::SeqCst),
        0,
        "an agent started at launch"
    );
    assert!(
        SEEDED
            .iter()
            .all(|chat| !core.sessions.live_run_steerable(chat))
    );
    next_message_resumes(&core, &ledger).await;
    core.sessions.shutdown().await;
}

#[test]
fn parked_agents_are_not_restarted_after_a_crash() {
    let tmp = tempfile::tempdir().unwrap();
    // The engine dies with its agents parked: its runtime goes without a
    // shutdown, as a kill -9 leaves it (journals open, nothing closed).
    let seeding = tokio::runtime::Runtime::new().unwrap();
    seeding.block_on(async {
        let ledger = Arc::new(Ledger::default());
        let core = assemble(tmp.path(), &ledger);
        seed(&core, &ledger).await;
        // Let the debounced doc saves land, as minutes of rest would.
        tokio::time::sleep(Duration::from_millis(1500)).await;
        // Gone without its shutdown (its data dir let go, as a dead
        // process's is).
        drop(core);
    });
    seeding.shutdown_background();
    let launching = tokio::runtime::Runtime::new().unwrap();
    launching.block_on(async {
        let (core, ledger) = launch(tmp.path()).await;
        assert_eq!(
            ledger.spawns.load(Ordering::SeqCst),
            0,
            "an agent started at launch"
        );
        next_message_resumes(&core, &ledger).await;
        core.sessions.shutdown().await;
    });
}

#[tokio::test]
async fn a_turn_cut_off_mid_way_is_picked_back_up_and_only_it() {
    let tmp = tempfile::tempdir().unwrap();
    {
        let ledger = Arc::new(Ledger::default());
        let core = assemble(tmp.path(), &ledger);
        seed(&core, &ledger).await;
        core.sessions
            .dispatch(
                "busy",
                HarnessId::Mock,
                request("hold this long task"),
                None,
            )
            .await
            .unwrap();
        wait_for(
            || core.sessions.turn_in_flight("busy") && ledger.spawns.load(Ordering::SeqCst) == 6,
            "the long turn to run",
        )
        .await;
        core.shutdown().await;
    }
    let (core, ledger) = launch(tmp.path()).await;
    wait_for(
        || ledger.spawns.load(Ordering::SeqCst) >= 1,
        "the cut-off turn's revival",
    )
    .await;
    tokio::time::sleep(Duration::from_secs(1)).await;
    let runs = ledger.runs.lock().unwrap().clone();
    assert_eq!(runs.len(), 1, "only the cut-off turn restarts: {runs:?}");
    assert!(
        core.sessions.turn_in_flight("busy"),
        "the revived turn is the cut-off one"
    );
    assert!(
        SEEDED
            .iter()
            .all(|chat| !core.sessions.live_run_steerable(chat))
    );
    core.sessions.shutdown().await;
}
