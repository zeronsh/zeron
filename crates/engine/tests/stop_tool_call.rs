//! Stopping one call of a live turn — a hung command, a subagent — while
//! the turn goes on (`Sessions::stop_tool_call`, the `StopToolCall` RPC).
//! The mock's `/hang` turn holds a command and a subagent open until each is
//! stopped alone; the turn then carries on and completes by itself.

use std::sync::Arc;
use std::time::Duration;

use zeron_doc::{MessagePart, SessionMessageEntry, SubagentStatus};
use zeron_engine::{EngineCore, HarnessRegistry};
use zeron_harness::ToolStop;
use zeron_harness::mock::{HANG_AGENT_ID, HANG_COMMAND_ID, MockHarness};
use zeron_proto::{HarnessId, RunRequest, SandboxLevel, SessionStatus};

const CHAT: &str = "chat-stop-tool-call";

fn request(prompt: &str) -> RunRequest {
    RunRequest {
        mcp: None,
        env: Default::default(),
        prompt: prompt.into(),
        harness: Some(HarnessId::Mock),
        model: None,
        reasoning: None,
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

fn entries(core: &EngineCore) -> Vec<SessionMessageEntry> {
    core.doc_host
        .open(CHAT)
        .ok()
        .and_then(|h| h.doc().read_entries().ok())
        .unwrap_or_default()
}

/// The tool part `id` of the transcript: (resolved, is_error, subagent status).
fn tool(core: &EngineCore, id: &str) -> Option<(bool, bool, Option<SubagentStatus>)> {
    entries(core)
        .into_iter()
        .flat_map(|e| e.parts)
        .find_map(|p| match p {
            MessagePart::Tool {
                id: part,
                resolved,
                is_error,
                subagent_status,
                ..
            } if part == id => Some((resolved, is_error, subagent_status)),
            _ => None,
        })
}

async fn wait_for(mut ok: impl FnMut() -> bool, what: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !ok() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn a_hung_command_and_a_subagent_stop_alone_and_the_turn_goes_on() {
    let registry = HarnessRegistry::new();
    registry.register(Arc::new(MockHarness { script: Vec::new() }));
    let dir = tempfile::tempdir().unwrap();
    let core = EngineCore::assemble(dir.path(), Arc::new(registry), HarnessId::Mock, None)
        .expect("engine core assembles");

    // Nothing runs yet: nothing to stop.
    assert_eq!(
        core.sessions.stop_tool_call(CHAT, HANG_COMMAND_ID).await,
        ToolStop::NotRunning
    );

    core.sessions
        .dispatch(CHAT, HarnessId::Mock, request("/hang"), None)
        .await
        .unwrap();
    wait_for(
        || tool(&core, HANG_COMMAND_ID).is_some() && tool(&core, HANG_AGENT_ID).is_some(),
        "the command and the subagent to start",
    )
    .await;
    assert_eq!(
        tool(&core, HANG_COMMAND_ID).map(|t| t.0),
        Some(false),
        "running"
    );

    // The command alone: it returns killed, the subagent and the turn run on.
    assert_eq!(
        core.sessions.stop_tool_call(CHAT, HANG_COMMAND_ID).await,
        ToolStop::Stopping
    );
    wait_for(
        || tool(&core, HANG_COMMAND_ID).is_some_and(|t| t.0),
        "the command's result",
    )
    .await;
    assert_eq!(
        tool(&core, HANG_COMMAND_ID).map(|t| t.1),
        Some(true),
        "it failed"
    );
    assert_eq!(
        tool(&core, HANG_AGENT_ID).map(|t| t.0),
        Some(false),
        "the subagent runs on"
    );
    assert_eq!(
        status(&core),
        Some(SessionStatus::Working),
        "the turn goes on"
    );

    // Asked again, it's over.
    assert_eq!(
        core.sessions.stop_tool_call(CHAT, HANG_COMMAND_ID).await,
        ToolStop::NotRunning
    );

    // The subagent alone, by its spawn's call: then the turn completes itself.
    assert_eq!(
        core.sessions.stop_tool_call(CHAT, HANG_AGENT_ID).await,
        ToolStop::Stopping
    );
    wait_for(
        || tool(&core, HANG_AGENT_ID).is_some_and(|t| t.0),
        "the subagent's result",
    )
    .await;
    wait_for(
        || status(&core) == Some(SessionStatus::Idle),
        "the turn to complete",
    )
    .await;
    let last = entries(&core).into_iter().last().expect("a reply");
    let said: String = last
        .parts
        .iter()
        .filter_map(|p| match p {
            MessagePart::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        said.contains("carried on"),
        "the reply went on past both stops: {said}"
    );
    core.sessions.shutdown().await;
}

#[tokio::test]
async fn a_driver_without_single_call_stops_says_so() {
    // The plain scripted mock never accepts single-call stops.
    let registry = HarnessRegistry::new();
    registry.register(Arc::new(MockHarness { script: Vec::new() }));
    let dir = tempfile::tempdir().unwrap();
    let core = EngineCore::assemble(dir.path(), Arc::new(registry), HarnessId::Mock, None)
        .expect("engine core assembles");
    core.sessions
        .dispatch(CHAT, HarnessId::Mock, request("/fanout 2"), None)
        .await
        .unwrap();
    wait_for(|| status(&core) == Some(SessionStatus::Working), "the turn").await;
    assert!(matches!(
        core.sessions.stop_tool_call(CHAT, "mock-fanout-0").await,
        ToolStop::Unsupported(_) | ToolStop::NotRunning
    ));
    core.sessions.shutdown().await;
}
