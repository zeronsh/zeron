//! The idle reaper must not kill a parked session's live background subagents.
//!
//! A background subagent routinely outlives the turn that spawned it: the
//! parent's Done parks the session while tagged subagent traffic keeps
//! streaming. That traffic never un-parks the chat, so the 30-minute idle
//! reaper used to fire on the parent's park time alone, cancel the child, and
//! stamp every still-running subagent `failed` — the "subagents die after
//! ~20 minutes and can't be resumed" report.

use std::sync::{Arc, Once};
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;
use tokio::sync::{Mutex, mpsc};

use zeron_doc::{MessagePart, SessionMessageEntry, SubagentStatus};
use zeron_engine::{EngineCore, HarnessRegistry};
use zeron_harness::{Harness, HarnessError, RunControls};
use zeron_proto::{
    AgentEvent, DoneStatus, HarnessId, Model, ReasoningLevel, RunRequest, SandboxLevel,
    SessionStatus, SteeringMode, ToolCall,
};

const CHAT: &str = "chat-subagent-reap";
const SPAWN: &str = "toolu_spawn";
/// Idle-reaper window for every test in this file (process-global env knob,
/// set once before any engine assembles).
const IDLE_MS: u64 = 400;

fn init_env() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        // SAFETY: called before any engine (and thus any reader of the var)
        // exists in this test process; all tests share the one value.
        unsafe { std::env::set_var("ZERON_SESSION_IDLE_MS", IDLE_MS.to_string()) };
    });
}

fn run_request(prompt: &str) -> RunRequest {
    RunRequest {
        mcp: None,
        prompt: prompt.into(),
        harness: None,
        model: None,
        reasoning: None,
        model_options: Default::default(),
        cwd: "/tmp".into(),
        sandbox: SandboxLevel::WorkspaceWrite,
        auto_approve: true,
        attachments: Vec::new(),
        worktree: None,
        resume: None,
    }
}

fn done(status: DoneStatus) -> AgentEvent {
    AgentEvent::Done {
        status,
        result: None,
        error: None,
        session_id: Some("hs-r".into()),
    }
}

fn tagged(event: AgentEvent) -> AgentEvent {
    AgentEvent::Subagent {
        parent_tool_use_id: SPAWN.into(),
        event: Box::new(event),
    }
}

/// Feed-by-hand harness (see turn_quiesce.rs): the test pushes events through
/// a channel. The stream ends when the engine cancels the run, like a reaped
/// child's stdout closing.
struct FeedHarness {
    main_prompt: String,
    feed: Mutex<Option<mpsc::UnboundedReceiver<AgentEvent>>>,
}

#[async_trait]
impl Harness for FeedHarness {
    fn id(&self) -> HarnessId {
        HarnessId::Mock
    }
    fn display_name(&self) -> &str {
        "Feed"
    }
    fn supports_subagent_stop(&self) -> bool {
        true
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
        if request.prompt != self.main_prompt {
            let events = vec![Ok(done(DoneStatus::Completed))];
            return Ok(futures::stream::iter(events).boxed());
        }
        let mut feed = self
            .feed
            .lock()
            .await
            .take()
            .expect("FeedHarness serves the main dispatch once per test");
        let (tx, rx) = mpsc::channel::<Result<AgentEvent, HarnessError>>(64);
        let cancel = controls.interrupt.clone();
        let mut child_controls = controls.subagent_control.unwrap();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => return,
                    Some(command) = child_controls.recv() => {
                        let _ = command.reply.send(if command.tool_use_id == SPAWN { Ok(()) } else { Err("provider rejected stop".into()) });
                    }
                    event = feed.recv() => match event {
                        Some(event) => {
                            if tx.send(Ok(event)).await.is_err() {
                                return;
                            }
                        }
                        None => return,
                    },
                }
            }
        });
        Ok(futures::stream::unfold(rx, |mut rx| async move {
            rx.recv().await.map(|event| (event, rx))
        })
        .boxed())
    }
}

fn entries(core: &EngineCore) -> Vec<SessionMessageEntry> {
    core.doc_host
        .open(CHAT)
        .ok()
        .and_then(|h| h.doc().read_entries().ok())
        .unwrap_or_default()
}

fn chip_status(core: &EngineCore) -> Option<SubagentStatus> {
    entries(core)
        .iter()
        .flat_map(|e| &e.parts)
        .find_map(|p| match p {
            MessagePart::Tool {
                id,
                subagent_status,
                ..
            } if id == SPAWN => *subagent_status,
            _ => None,
        })
}

async fn wait_for<F>(mut predicate: F, what: &str)
where
    F: FnMut() -> bool,
{
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !predicate() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// A parked session: the parent spawned a background subagent and finished
/// its turn. The returned feed pushes further (tagged) harness events; it
/// closes when the engine reaps the run.
struct Parked {
    core: EngineCore,
    feed: mpsc::UnboundedSender<AgentEvent>,
    _dir: tempfile::TempDir,
}

async fn park_after_spawn() -> Parked {
    init_env();
    let (feed, rx) = mpsc::unbounded_channel();
    let registry = HarnessRegistry::new();
    registry.register(Arc::new(FeedHarness {
        main_prompt: "fan out".into(),
        feed: Mutex::new(Some(rx)),
    }));
    let dir = tempfile::tempdir().unwrap();
    let core = EngineCore::assemble(dir.path(), Arc::new(registry), HarnessId::Mock, None)
        .expect("engine core assembles");
    core.sessions
        .dispatch(CHAT, HarnessId::Mock, run_request("fan out"), None)
        .await
        .expect("dispatch");

    feed.send(AgentEvent::SessionStarted {
        harness: HarnessId::Mock,
        model: "mock-1".into(),
        tools: vec![],
        cwd: "/tmp".into(),
        session_id: "hs-r".into(),
        assistant_message_id: "a-r".into(),
    })
    .unwrap();
    feed.send(AgentEvent::ToolCall {
        id: SPAWN.into(),
        call: ToolCall::Unknown {
            name: "Agent: scout".into(),
            input: Some(serde_json::json!({"description": "scout", "prompt": "look around"})),
        },
    })
    .unwrap();
    feed.send(AgentEvent::ToolResult {
        id: SPAWN.into(),
        is_error: false,
        output: None,
        diff: None,
    })
    .unwrap();
    feed.send(done(DoneStatus::Completed)).unwrap();
    wait_for(
        || {
            core.sessions
                .session_status(CHAT)
                .is_some_and(|s| s.status == SessionStatus::Idle)
        },
        "park after Done",
    )
    .await;
    Parked {
        core,
        feed,
        _dir: dir,
    }
}

/// The subagent keeps working for several idle windows, then finishes. It
/// must finish `Done`, not be reaped mid-flight as `Failed`; once it settles,
/// the parked session is reaped as usual, counted from its last activity.
#[tokio::test]
async fn reaper_spares_a_parked_session_with_a_live_subagent() {
    let Parked { core, feed, _dir } = park_after_spawn().await;

    // The subagent works on, well past the idle window, with the parent parked.
    let started = tokio::time::Instant::now();
    while started.elapsed() < Duration::from_millis(IDLE_MS * 4) {
        // A reaped run drops the receiver; the chip assertion below reports it.
        let _ = feed.send(tagged(AgentEvent::TextDelta {
            text: "still digging. ".into(),
        }));
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let _ = feed.send(tagged(done(DoneStatus::Completed)));

    wait_for(
        || chip_status(&core).is_some_and(|s| s != SubagentStatus::Running),
        "subagent chip to settle",
    )
    .await;
    assert_eq!(
        chip_status(&core),
        Some(SubagentStatus::Done),
        "the idle reaper killed a live background subagent"
    );

    // Settled: nothing is using the child any more, so the ordinary window
    // (from the subagent's last event, not the long-past park) ends it.
    wait_for(|| feed.is_closed(), "reap after the subagent settled").await;
    assert_eq!(chip_status(&core), Some(SubagentStatus::Done));

    core.sessions.shutdown().await;
}

/// A subagent whose terminal signal never arrives must not pin the child
/// forever: the stretched window (8x the idle window) still reaps it, and the
/// reap stamps the lost subagent `Failed`.
#[tokio::test]
async fn reaper_still_ends_a_session_whose_subagent_went_silent() {
    let Parked { core, feed, _dir } = park_after_spawn().await;

    feed.send(tagged(AgentEvent::TextDelta {
        text: "starting. ".into(),
    }))
    .unwrap();
    wait_for(
        || chip_status(&core) == Some(SubagentStatus::Running),
        "subagent chip to run",
    )
    .await;

    // Past the plain idle window, the open sink still holds the reaper off...
    tokio::time::sleep(Duration::from_millis(IDLE_MS * 3)).await;
    assert!(!feed.is_closed(), "reaped a subagent inside its window");
    assert_eq!(chip_status(&core), Some(SubagentStatus::Running));

    // ...but not forever.
    wait_for(|| feed.is_closed(), "reap of the silent subagent").await;
    wait_for(
        || chip_status(&core) == Some(SubagentStatus::Failed),
        "lost subagent chip to fail",
    )
    .await;

    core.sessions.shutdown().await;
}

#[tokio::test]
async fn runtime_death_settles_running_chips_without_sinks() {
    let Parked { core, feed, _dir } = park_after_spawn().await;
    // A resumed chip may be Running in a completed turn without this runtime
    // having received child traffic (and hence without opening any sink).
    core.doc_host
        .open(CHAT)
        .unwrap()
        .doc()
        .update_subagent_chip(SPAWN, None, Some("running"), None)
        .unwrap();
    assert_eq!(chip_status(&core), Some(SubagentStatus::Running));
    drop(feed);
    wait_for(
        || chip_status(&core) == Some(SubagentStatus::Failed),
        "orphaned chip settlement",
    )
    .await;
    core.sessions.shutdown().await;
}

#[tokio::test]
async fn stop_subagent_rpc_finalizes_only_target_and_parent_can_continue() {
    let Parked { core, feed, _dir } = park_after_spawn().await;
    feed.send(tagged(AgentEvent::TextDelta {
        text: "scouting".into(),
    }))
    .unwrap();
    wait_for(
        || chip_status(&core) == Some(SubagentStatus::Running),
        "running child",
    )
    .await;
    feed.send(AgentEvent::Steered {
        assistant_message_id: None,
        next_assistant_message_id: None,
    })
    .unwrap();
    feed.send(AgentEvent::ToolCall {
        id: "sibling".into(),
        call: ToolCall::Unknown {
            name: "Agent: reviewer".into(),
            input: None,
        },
    })
    .unwrap();
    feed.send(AgentEvent::Subagent {
        parent_tool_use_id: "sibling".into(),
        event: Box::new(AgentEvent::TextDelta {
            text: "reviewing".into(),
        }),
    })
    .unwrap();
    let sibling_running = || {
        entries(&core).iter().flat_map(|e| &e.parts).any(|p| matches!(p, MessagePart::Tool { id, subagent_status: Some(SubagentStatus::Running), .. } if id == "sibling"))
    };
    wait_for(sibling_running, "running sibling").await;
    let client = zeron_rpc::memory_client(core.rpc_service());
    assert!(
        client
            .call(
                zeron_rpc::methods::STOP_SUBAGENT,
                serde_json::json!({"chatId":CHAT,"toolUseId":"sibling"})
            )
            .await
            .is_err()
    );
    assert!(sibling_running(), "rejected stop mutated child status");
    let result = client
        .call(
            zeron_rpc::methods::STOP_SUBAGENT,
            serde_json::json!({"chatId":CHAT,"toolUseId":SPAWN}),
        )
        .await
        .unwrap();
    assert_eq!(result["stopped"], true);
    wait_for(
        || chip_status(&core) == Some(SubagentStatus::Done),
        "stopped child chip",
    )
    .await;
    assert!(!feed.is_closed(), "parent runtime was killed");
    assert!(sibling_running(), "stopping one child stopped its sibling");
    let child = core
        .doc_host
        .open(&format!("{CHAT}--sub--{SPAWN}"))
        .unwrap()
        .doc()
        .read_entries()
        .unwrap();
    assert!(
        child
            .iter()
            .any(|entry| entry.status == Some(zeron_doc::MessageStatus::Aborted))
    );
    // A malformed/foreign id must not reach the provider or interrupt parent.
    assert!(
        client
            .call(
                zeron_rpc::methods::STOP_SUBAGENT,
                serde_json::json!({"chatId":CHAT,"toolUseId":"unknown"})
            )
            .await
            .is_err()
    );
    feed.send(AgentEvent::TextDelta {
        text: "parent still works".into(),
    })
    .unwrap();
    wait_for(|| entries(&core).iter().flat_map(|e| &e.parts).any(|p| matches!(p, MessagePart::Text { text, .. } if text.contains("parent still works"))), "parent continuation").await;
    core.sessions.shutdown().await;
}

#[tokio::test]
async fn stop_without_runtime_repairs_only_running_orphans() {
    let Parked { core, feed, _dir } = park_after_spawn().await;
    core.sessions.shutdown().await;
    assert!(feed.is_closed());
    let handle = core.doc_host.open(CHAT).unwrap();
    handle
        .writer()
        .update_subagent_chip(SPAWN, None, Some("done"), None)
        .unwrap();
    core.sessions.stop_subagent(CHAT, SPAWN).await.unwrap();
    assert_eq!(chip_status(&core), Some(SubagentStatus::Done));
    handle
        .writer()
        .update_subagent_chip(SPAWN, None, Some("running"), None)
        .unwrap();
    core.sessions.stop_subagent(CHAT, SPAWN).await.unwrap();
    assert_eq!(chip_status(&core), Some(SubagentStatus::Failed));
    assert!(core.sessions.stop_subagent(CHAT, "unknown").await.is_err());
}
