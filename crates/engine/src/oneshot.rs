//! One-shot model runs: a single turn through a harness, in a scratch folder
//! with no MCP servers, that does no work of its own — the title generator's
//! profile (see [`crate::titles`]), for work that isn't a chat.
//!
//! A plugin asks through it (`zeron.ask`, `models.ask` in plugins/agents).
//! Nothing here touches a chat's transcript, workspace or credentials beyond
//! the harness's own login.

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use zeron_harness::{CancellationToken, Harness, RunControls, SteerMessage};
use zeron_proto::{
    AgentEvent, DoneStatus, ReasoningLevel, RunRequest, SandboxLevel, UserInputAnswer,
    UserInputQuestion,
};

use crate::EngineError;
use crate::plugins::agents::OneShot;
use crate::registry::HarnessRegistry;

/// Run `ask` on this device and return its text: on the first enabled
/// harness without one of its own, on the harness's default model without
/// one of its own.
pub(crate) async fn ask(registry: &HarnessRegistry, ask: OneShot) -> Result<String, EngineError> {
    let harness_id = match ask.harness {
        Some(id) => id,
        None => registry.enabled_set().first().copied().ok_or_else(|| {
            EngineError::Other("No agent is enabled here to run an ask".into())
        })?,
    };
    // Ordered against a queued update for the same CLI, as a title run is.
    let lease = Arc::new(registry.execution_lease(harness_id).await);
    let harness = registry.resolve(harness_id)?;
    // No repository instructions, files, or active coding-session context.
    let scratch = tempfile::tempdir()
        .map_err(|e| EngineError::Other(format!("no scratch folder for the ask: {e}")))?;
    let mut request = RunRequest {
        mcp: None,
        prompt: ask.input,
        harness: Some(harness_id),
        model: ask.model,
        reasoning: Some(ReasoningLevel::Minimal),
        model_options: serde_json::Map::new(),
        cwd: scratch.path().to_string_lossy().into_owned(),
        sandbox: SandboxLevel::ReadOnly,
        auto_approve: false,
        access: Default::default(),
        plan: false,
        attachments: Vec::new(),
        resume: None,
        worktree: None,
        env: Default::default(),
        extra_mcp: Vec::new(),
    };
    // A Cloud device's credential broker supplies the CLI's login; without one
    // there is nothing to run with.
    registry
        .prepare_run(harness_id, None, &mut request)
        .await
        .map_err(|e| EngineError::Other(format!("no login for {}: {e}", harness_id.as_str())))?;
    tokio::time::timeout(
        // Long enough for a cold start of the CLI, short enough that a
        // plugin's promise isn't left hanging.
        Duration::from_secs(60),
        run_and_collect(
            harness.as_ref(),
            request,
            Some(ask.instructions),
            Some(lease),
            "the ask",
        ),
    )
    .await
    .map_err(|_| EngineError::Other("the ask timed out".into()))?
}

/// One run through `harness`, its text collected: `instructions` for a
/// one-shot ask, `None` for a title (whose instructions ride its prompt).
pub(crate) async fn run_and_collect(
    harness: &dyn Harness,
    request: RunRequest,
    instructions: Option<String>,
    execution_lease: Option<Arc<tokio::sync::OwnedRwLockReadGuard<()>>>,
    what: &str,
) -> Result<String, EngineError> {
    let (steer_tx, steer_rx) = tokio::sync::mpsc::channel::<SteerMessage>(1);
    let interrupt = CancellationToken::new();
    let _cancel_on_drop = interrupt.clone().drop_guard();
    let controls = RunControls {
        realtime: None,
        execution_lease,
        // A one-shot run must never block on input: questions are answered empty.
        request_input: Box::new(|_questions: Vec<UserInputQuestion>| {
            let (tx, rx) = tokio::sync::oneshot::channel::<Vec<UserInputAnswer>>();
            let _ = tx.send(Vec::new());
            rx
        }),
        steering: steer_rx,
        interrupt: interrupt.clone(),
        turn: Default::default(),
    };
    let mut stream = match instructions {
        Some(instructions) => harness.run_instructions(request, instructions, controls).await?,
        None => harness.run_title(request, controls).await?,
    };
    let mut text = String::new();
    let mut completed = false;
    while let Some(event) = stream.next().await {
        match event? {
            AgentEvent::TextDelta { text: delta } => text.push_str(&delta),
            AgentEvent::ToolCall { .. } => {
                return Err(EngineError::Other(format!("{what} attempted to use a tool")));
            }
            AgentEvent::Error { message } => {
                return Err(EngineError::Other(format!("{what} failed: {message}")));
            }
            AgentEvent::Done { status, error, .. } => {
                if status == DoneStatus::Completed {
                    completed = true;
                    break;
                }
                return Err(EngineError::Other(format!(
                    "{what} ended {status:?}: {}",
                    error.unwrap_or_default()
                )));
            }
            _ => {}
        }
    }
    // Held until here: a run whose mailbox closes mid-flight is told so.
    drop(steer_tx);
    if completed {
        Ok(text)
    } else {
        Err(EngineError::Other(format!("{what} ended without completing")))
    }
}
