//! Auto-resume after the usage limit: a turn the limit stopped carries its
//! stop on the session row and continues by itself once the limit resets —
//! the stopped turn's message goes back to the provider's session as a short
//! continuation, with no new message in the transcript; a message the user
//! sends meanwhile drops the stop; a stop outlives an engine restart, and
//! the per-stop toggle and the device setting decide whether it resumes.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;

use chrono::Utc;
use zeron_doc::{MessageRole, SessionCommandPayload};
use zeron_engine::{EngineCore, HarnessRegistry};
use zeron_harness::{Harness, HarnessError, RunControls};
use zeron_proto::{
    AgentEvent, AutoResume, AutoResumePhase, DoneStatus, HarnessId, Model, ReasoningLevel,
    RunRequest, SandboxLevel, SessionStatus, SteeringMode,
};

const CHAT: &str = "chat-auto-resume";
const SESSION: &str = "hs-auto-resume";

type RequestLog = Arc<Mutex<Vec<RunRequest>>>;

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
        access: Default::default(),
        plan: false,
        attachments: Vec::new(),
        worktree: None,
        resume: None,
        env: Default::default(),
    }
}

/// The first `limited` runs hit the usage limit (resetting at
/// `resets_at`); every later one completes. Records every request.
struct LimitedHarness {
    requests: RequestLog,
    resets_at: chrono::DateTime<Utc>,
    limited: usize,
}

#[async_trait]
impl Harness for LimitedHarness {
    fn id(&self) -> HarnessId {
        HarnessId::Mock
    }
    fn display_name(&self) -> &str {
        "Limited"
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
        let first = {
            let mut log = self.requests.lock().unwrap();
            log.push(request.clone());
            log.len() <= self.limited
        };
        let started = AgentEvent::SessionStarted {
            harness: HarnessId::Mock,
            model: "mock-1".into(),
            tools: vec![],
            cwd: request.cwd.clone(),
            session_id: SESSION.into(),
            assistant_message_id: "a-1".into(),
        };
        let events = if first {
            vec![
                started,
                AgentEvent::TextDelta {
                    text: "Halfway there".into(),
                },
                AgentEvent::LimitReached {
                    resets_at: Some(self.resets_at),
                },
                AgentEvent::Error {
                    message: "You've hit your 5-hour limit".into(),
                },
                AgentEvent::Done {
                    status: DoneStatus::Errored,
                    result: None,
                    error: None,
                    session_id: Some(SESSION.into()),
                },
            ]
        } else {
            vec![
                started,
                AgentEvent::TextDelta {
                    text: format!("ack: {}", request.prompt),
                },
                AgentEvent::Done {
                    status: DoneStatus::Completed,
                    result: None,
                    error: None,
                    session_id: Some(SESSION.into()),
                },
            ]
        };
        Ok(futures::stream::iter(events.into_iter().map(Ok)).boxed())
    }
}

fn assemble(
    dir: &std::path::Path,
    requests: &RequestLog,
    resets_at: chrono::DateTime<Utc>,
) -> EngineCore {
    assemble_limited(dir, requests, resets_at, 1)
}

fn assemble_limited(
    dir: &std::path::Path,
    requests: &RequestLog,
    resets_at: chrono::DateTime<Utc>,
    limited: usize,
) -> EngineCore {
    let registry = HarnessRegistry::new();
    registry.register(Arc::new(LimitedHarness {
        requests: requests.clone(),
        resets_at,
        limited,
    }));
    let core = EngineCore::assemble(dir, Arc::new(registry), HarnessId::Mock, None)
        .expect("engine core assembles");
    // Resume the moment the limit resets.
    core.auto_resume.set_reset_margin(chrono::Duration::zero());
    core
}

/// A titled chat row, so the auto-titler sends no requests of its own.
fn create_chat(core: &EngineCore) {
    core.workspace
        .create_space("space-auto-resume", &core.device_id, "/tmp", None, false)
        .expect("create space row");
    core.workspace
        .create_chat(CHAT, Some("space-auto-resume"), None, None, None)
        .expect("create chat row");
    core.workspace
        .rename_chat(CHAT, "Titled")
        .expect("rename chat");
}

fn queue_run(core: &EngineCore, prompt: &str, message_id: &str) {
    core.doc_host
        .queue_command(
            CHAT,
            SessionCommandPayload::Run {
                request: run_request(prompt),
                message_id: message_id.into(),
            },
        )
        .expect("queue run command");
}

async fn wait_for(mut predicate: impl FnMut() -> bool, what: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while !predicate() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn stop(core: &EngineCore) -> Option<AutoResume> {
    core.sessions
        .session_status(CHAT)
        .and_then(|row| row.auto_resume)
}

fn user_messages(core: &EngineCore) -> usize {
    core.doc_host
        .open(CHAT)
        .ok()
        .and_then(|handle| handle.doc().read_entries().ok())
        .unwrap_or_default()
        .iter()
        .filter(|entry| entry.role == MessageRole::User)
        .count()
}

#[tokio::test]
async fn a_limited_turn_continues_once_the_limit_has_reset() {
    let tmp = tempfile::tempdir().unwrap();
    let requests: RequestLog = Default::default();
    let reset = Utc::now() + chrono::Duration::seconds(1);
    let core = assemble(&tmp.path().join("data"), &requests, reset);
    create_chat(&core);
    queue_run(&core, "refactor the parser", "msg-1");

    wait_for(|| stop(&core).is_some(), "the limit stop").await;
    let waiting = stop(&core).unwrap();
    assert_eq!(waiting.resume_at, Some(reset));

    wait_for(
        || requests.lock().unwrap().len() == 2,
        "the continuation run",
    )
    .await;
    let continuation = requests.lock().unwrap()[1].clone();
    assert!(
        continuation.prompt.contains("usage limit was reached"),
        "{}",
        continuation.prompt
    );
    assert!(
        continuation
            .prompt
            .contains("<request>\nrefactor the parser\n</request>")
    );
    assert_eq!(
        continuation.resume.as_deref(),
        Some(SESSION),
        "the continuation resumes the stopped turn's provider session"
    );
    wait_for(
        || {
            core.sessions
                .session_status(CHAT)
                .is_some_and(|row| row.status == SessionStatus::Idle)
        },
        "the continuation to finish",
    )
    .await;
    let resumed = stop(&core).expect("the row keeps the stop");
    assert_eq!(resumed.phase, AutoResumePhase::Resumed);
    assert!(resumed.resumed_at.is_some());
    assert_eq!(user_messages(&core), 1, "no new message in the transcript");
    core.shutdown().await;
}

#[tokio::test]
async fn a_message_the_user_sends_drops_the_waiting_stop() {
    let tmp = tempfile::tempdir().unwrap();
    let requests: RequestLog = Default::default();
    let resets_at = Utc::now() + chrono::Duration::hours(2);
    let core = assemble(&tmp.path().join("data"), &requests, resets_at);
    create_chat(&core);
    queue_run(&core, "refactor the parser", "msg-1");

    wait_for(|| stop(&core).is_some(), "the limit stop").await;
    let waiting = stop(&core).unwrap();
    assert_eq!(waiting.phase, AutoResumePhase::Waiting);
    assert!(waiting.enabled, "on by default");
    assert_eq!(waiting.resume_at, Some(resets_at));
    assert_eq!(
        core.sessions.session_status(CHAT).unwrap().status,
        SessionStatus::Errored
    );

    // The per-stop toggle.
    assert!(!core.auto_resume.set_chat(CHAT, false).unwrap().enabled);
    assert!(core.auto_resume.set_chat(CHAT, true).unwrap().enabled);

    queue_run(&core, "never mind, do this instead", "msg-2");
    wait_for(|| requests.lock().unwrap().len() == 2, "the user's run").await;
    wait_for(|| stop(&core).is_none(), "the stop to drop").await;
    assert_eq!(
        requests.lock().unwrap()[1].prompt,
        "never mind, do this instead"
    );
    core.shutdown().await;
}

#[tokio::test]
async fn a_stop_outlives_a_restart_and_resumes_when_switched_on() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("data");
    let requests: RequestLog = Default::default();
    let reset = Utc::now() + chrono::Duration::seconds(1);
    {
        let core = assemble(&dir, &requests, reset);
        create_chat(&core);
        // Off on this device: the stop is recorded, switched off.
        assert!(!core.auto_resume.set_enabled(false).unwrap().enabled);
        queue_run(&core, "refactor the parser", "msg-1");
        wait_for(|| stop(&core).is_some(), "the limit stop").await;
        assert!(!stop(&core).unwrap().enabled);
        // Long enough for a pass that must not resume it.
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(requests.lock().unwrap().len(), 1);
        core.shutdown().await;
    }

    let core = assemble(&dir, &requests, reset);
    assert!(!core.auto_resume.settings().enabled, "the setting persists");
    let restored = stop(&core).expect("the stop is back after the restart");
    assert_eq!(restored.phase, AutoResumePhase::Waiting);
    assert!(!restored.enabled);
    assert_eq!(
        core.sessions.session_status(CHAT).unwrap().status,
        SessionStatus::Errored
    );

    // Switched on after its reset passed: resumes right away.
    while Utc::now() <= reset {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    core.auto_resume.set_chat(CHAT, true);
    wait_for(
        || requests.lock().unwrap().len() == 2,
        "the continuation run",
    )
    .await;
    assert!(
        requests.lock().unwrap()[1]
            .prompt
            .contains("usage limit was reached")
    );
    wait_for(
        || stop(&core).is_some_and(|s| s.phase == AutoResumePhase::Resumed),
        "the stop to read resumed",
    )
    .await;
    core.shutdown().await;
}

#[tokio::test]
async fn a_reset_already_past_is_no_time_to_resume_at() {
    let tmp = tempfile::tempdir().unwrap();
    let requests: RequestLog = Default::default();
    let core = assemble(
        &tmp.path().join("data"),
        &requests,
        Utc::now() - chrono::Duration::minutes(5),
    );
    create_chat(&core);
    queue_run(&core, "refactor the parser", "msg-1");
    wait_for(|| stop(&core).is_some(), "the limit stop").await;
    assert_eq!(stop(&core).unwrap().resume_at, None);
    // No account to ask: the first check is minutes away, not now.
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(requests.lock().unwrap().len(), 1);
    assert_eq!(stop(&core).unwrap().phase, AutoResumePhase::Waiting);
    core.shutdown().await;
}

#[tokio::test]
async fn a_stop_or_an_archive_drops_the_waiting_stop() {
    let tmp = tempfile::tempdir().unwrap();
    let requests: RequestLog = Default::default();
    let core = assemble_limited(
        &tmp.path().join("data"),
        &requests,
        Utc::now() + chrono::Duration::hours(2),
        usize::MAX,
    );
    create_chat(&core);
    queue_run(&core, "refactor the parser", "msg-1");
    wait_for(|| stop(&core).is_some(), "the limit stop").await;
    core.sessions.interrupt(CHAT).await.expect("stop");
    wait_for(|| stop(&core).is_none(), "the Stop to drop it").await;

    queue_run(&core, "try once more", "msg-2");
    wait_for(|| stop(&core).is_some(), "the second limit stop").await;
    core.workspace
        .set_chat_archived(CHAT, true)
        .expect("archive");
    wait_for(|| stop(&core).is_none(), "archiving to drop it").await;
    assert_eq!(requests.lock().unwrap().len(), 2, "nothing resumed");
    core.shutdown().await;
}
