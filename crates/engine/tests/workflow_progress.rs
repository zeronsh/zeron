//! A workflow spawn's progress summary lands on its chip — never in the
//! subagent's own transcript.
//!
//! Claude's `Workflow` tool runs its agents in the background and reports
//! them only as progress. The driver summarizes that into a tagged
//! `SubagentProgress` ("1/3 agents · 26.6k tokens") on each agent-count
//! movement; the engine stamps it onto the chip's `subagent_tail`, both while
//! the parent turn still streams (live fold) and after it parked (in place).

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;
use tokio::sync::{Mutex, mpsc};

use zeron_doc::{MessagePart, MessageRole, SessionMessageEntry, SubagentStatus};
use zeron_engine::{EngineCore, HarnessRegistry};
use zeron_harness::{Harness, HarnessError, RunControls};
use zeron_proto::{
    AgentEvent, DoneStatus, HarnessId, Model, ReasoningLevel, RunRequest, SandboxLevel,
    SessionStatus, SteeringMode, ToolCall,
};

const CHAT: &str = "chat-workflow-progress";
const SPAWN: &str = "toolu_wf";

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
        session_id: Some("hs-wf".into()),
    }
}

fn tagged(event: AgentEvent) -> AgentEvent {
    AgentEvent::Subagent {
        parent_tool_use_id: SPAWN.into(),
        event: Box::new(event),
    }
}

fn progress(summary: &str) -> AgentEvent {
    tagged(AgentEvent::SubagentProgress {
        summary: summary.into(),
    })
}

/// Feed-by-hand harness (see subagent_idle_reap.rs).
struct FeedHarness {
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
        _request: RunRequest,
        controls: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        let Some(mut feed) = self.feed.lock().await.take() else {
            return Ok(futures::stream::iter(vec![Ok(done(DoneStatus::Completed))]).boxed());
        };
        let (tx, rx) = mpsc::channel::<Result<AgentEvent, HarnessError>>(64);
        let cancel = controls.interrupt.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => return,
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

/// The workflow chip's (ref, status, tail), once it is in the doc.
fn chip(core: &EngineCore) -> Option<(Option<String>, Option<SubagentStatus>, Option<String>)> {
    entries(core)
        .iter()
        .flat_map(|e| &e.parts)
        .find_map(|p| match p {
            MessagePart::Tool {
                id,
                subagent_ref,
                subagent_status,
                subagent_tail,
                ..
            } if id == SPAWN => Some((
                subagent_ref.clone(),
                *subagent_status,
                subagent_tail.clone(),
            )),
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

struct Run {
    core: EngineCore,
    feed: mpsc::UnboundedSender<AgentEvent>,
    _dir: tempfile::TempDir,
}

/// Dispatch a run whose first act is a Workflow spawn (resolved eagerly, as
/// claude's background launch is).
async fn spawn_workflow() -> Run {
    let (feed, rx) = mpsc::unbounded_channel();
    let registry = HarnessRegistry::new();
    registry.register(Arc::new(FeedHarness {
        feed: Mutex::new(Some(rx)),
    }));
    let dir = tempfile::tempdir().unwrap();
    let core = EngineCore::assemble(dir.path(), Arc::new(registry), HarnessId::Mock, None)
        .expect("engine core assembles");
    core.sessions
        .dispatch(CHAT, HarnessId::Mock, run_request("fan out"), None)
        .await
        .expect("dispatch");
    for event in [
        AgentEvent::SessionStarted {
            harness: HarnessId::Mock,
            model: "mock-1".into(),
            tools: vec![],
            cwd: "/tmp".into(),
            session_id: "hs-wf".into(),
            assistant_message_id: "a-wf".into(),
        },
        AgentEvent::ToolCall {
            id: SPAWN.into(),
            call: ToolCall::Unknown {
                name: "Agent: Repo scan".into(),
                input: Some(serde_json::json!({"subagent_type": "workflow", "script": "…"})),
            },
        },
        AgentEvent::ToolResult {
            id: SPAWN.into(),
            is_error: false,
            output: None,
            diff: None,
        },
        tagged(AgentEvent::UserMessage {
            text: "Repo scan".into(),
        }),
    ] {
        feed.send(event).unwrap();
    }
    Run {
        core,
        feed,
        _dir: dir,
    }
}

/// While the parent turn still streams, the summary rides the live fold.
#[tokio::test]
async fn summary_folds_onto_a_streaming_workflow_chip() {
    let Run { core, feed, _dir } = spawn_workflow().await;
    feed.send(progress("0/2 agents")).unwrap();
    feed.send(done(DoneStatus::Completed)).unwrap();
    wait_for(
        || chip(&core).is_some_and(|(_, _, tail)| tail.as_deref() == Some("0/2 agents")),
        "streamed summary on the chip",
    )
    .await;
    let (sub_ref, status, _) = chip(&core).unwrap();
    assert!(sub_ref.is_some(), "the opening message still links the doc");
    assert_eq!(status, Some(SubagentStatus::Running));
}

/// After the parent parked (eager-done), each summary restamps the chip in
/// place; the final one lands before the workflow settles, and the subagent
/// transcript never sees any of them.
#[tokio::test]
async fn summary_restamps_a_parked_workflow_chip_in_place() {
    let Run { core, feed, _dir } = spawn_workflow().await;
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
    feed.send(progress("1/2 agents · 12.0k tokens")).unwrap();
    wait_for(
        || {
            chip(&core)
                .is_some_and(|(_, _, tail)| tail.as_deref() == Some("1/2 agents · 12.0k tokens"))
        },
        "in-place summary",
    )
    .await;
    feed.send(progress("2 agents · 40.1k tokens · 13s"))
        .unwrap();
    feed.send(tagged(done(DoneStatus::Completed))).unwrap();
    wait_for(
        || {
            chip(&core).is_some_and(|(_, status, tail)| {
                status == Some(SubagentStatus::Done)
                    && tail.as_deref() == Some("2 agents · 40.1k tokens · 13s")
            })
        },
        "final summary and settle",
    )
    .await;
    let (sub_ref, _, _) = chip(&core).unwrap();
    let sub_doc = core
        .doc_host
        .open(&sub_ref.expect("linked"))
        .expect("subagent doc");
    let text: String = sub_doc
        .doc()
        .read_entries()
        .unwrap()
        .iter()
        .flat_map(|e| &e.parts)
        .filter_map(|p| match p {
            MessagePart::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        !text.contains("agents"),
        "summary leaked into the transcript: {text}"
    );
}

/// A workflow's agent rows are NESTED spawns: each links to its own live
/// transcript and carries its lifecycle inside the workflow's doc, while the
/// chat doc only ever holds the workflow chip.
#[tokio::test]
async fn workflow_agent_rows_link_to_their_own_transcripts() {
    let Run { core, feed, _dir } = spawn_workflow().await;
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
    const ROW: &str = "toolu_wf:wf1";
    let nested = |event: AgentEvent| AgentEvent::Subagent {
        parent_tool_use_id: ROW.into(),
        event: Box::new(event),
    };
    for event in [
        tagged(AgentEvent::ToolCall {
            id: ROW.into(),
            call: ToolCall::Unknown {
                name: "Agent: scan-a — Bash: ls".into(),
                input: Some(serde_json::json!({"model": "claude-haiku-4-5"})),
            },
        }),
        nested(AgentEvent::UserMessage {
            text: "List files.".into(),
        }),
        nested(AgentEvent::TextDelta {
            text: "AssetCacheLocatorUtil".into(),
        }),
    ] {
        feed.send(event).unwrap();
    }
    let workflow_doc = || {
        let (sub_ref, _, _) = chip(&core)?;
        core.doc_host
            .open(&sub_ref?)
            .ok()?
            .doc()
            .read_entries()
            .ok()
    };
    let row = || {
        workflow_doc()?
            .iter()
            .flat_map(|e| e.parts.clone())
            .find_map(|p| match p {
                MessagePart::Tool {
                    id,
                    subagent_ref,
                    subagent_status,
                    ..
                } if id == ROW => Some((subagent_ref, subagent_status)),
                _ => None,
            })
    };
    wait_for(
        || row().is_some_and(|(r, s)| r.is_some() && s == Some(SubagentStatus::Running)),
        "row linked and running",
    )
    .await;
    feed.send(nested(done(DoneStatus::Completed))).unwrap();
    feed.send(tagged(done(DoneStatus::Completed))).unwrap();
    wait_for(
        || row().is_some_and(|(_, s)| s == Some(SubagentStatus::Done)),
        "row settled",
    )
    .await;
    let (row_ref, _) = row().unwrap();
    let agent_text: String = core
        .doc_host
        .open(&row_ref.unwrap())
        .unwrap()
        .doc()
        .read_entries()
        .unwrap()
        .iter()
        .filter(|e| e.role == MessageRole::Assistant)
        .flat_map(|e| &e.parts)
        .filter_map(|p| match p {
            MessagePart::Text { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(agent_text, "AssetCacheLocatorUtil");
    // The chat doc holds only the workflow chip, never the agent row.
    assert!(
        entries(&core)
            .iter()
            .flat_map(|e| &e.parts)
            .all(|p| !matches!(p, MessagePart::Tool { id, .. } if id == ROW))
    );
}
