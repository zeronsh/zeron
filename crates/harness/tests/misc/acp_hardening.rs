//! AcpHarness durability cases against `tests/fixtures/fake-hardened-acp.py`:
//! stops the agent never settles, dialogs a stop must answer, crashes during
//! the handshake, sign-in retries and session restore.

#![cfg(unix)]

use std::path::PathBuf;
use std::time::Duration;

use futures::StreamExt;
use futures::stream::BoxStream;
use tokio::sync::{mpsc, oneshot};

use zeron_harness::{
    AcpHarness, CancellationToken, Harness, HarnessError, RunControls, SteerMessage, TurnControl,
};
use zeron_proto::{AgentEvent, DoneStatus, RunRequest, SandboxLevel};

fn fixture_path() -> PathBuf {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("fake-hardened-acp.py");
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755));
    path
}

fn harness() -> AcpHarness {
    AcpHarness::grok().with_executable(fixture_path())
}

fn request(mode: &str) -> RunRequest {
    RunRequest {
        mcp: None,
        prompt: "hello".into(),
        harness: None,
        model: None,
        reasoning: None,
        model_options: serde_json::Map::new(),
        cwd: "/tmp".into(),
        sandbox: SandboxLevel::WorkspaceWrite,
        auto_approve: true,
        attachments: Vec::new(),
        worktree: None,
        resume: None,
        env: [("FAKE_ACP_MODE".to_owned(), mode.to_owned())]
            .into_iter()
            .collect(),
    }
}

struct Run {
    stream: BoxStream<'static, Result<AgentEvent, HarnessError>>,
    steer: mpsc::Sender<SteerMessage>,
    turn: TurnControl,
    _interrupt: CancellationToken,
    /// Held so a question dialog stays open until the test drops it.
    _answers: mpsc::UnboundedReceiver<oneshot::Sender<Vec<zeron_proto::UserInputAnswer>>>,
}

impl Run {
    async fn next(&mut self) -> Option<AgentEvent> {
        tokio::time::timeout(Duration::from_secs(10), self.stream.next())
            .await
            .expect("event in time")
            .map(|event| event.expect("stream event"))
    }

    /// Events up to and including the next Done.
    async fn until_done(&mut self) -> Vec<AgentEvent> {
        let mut events = Vec::new();
        loop {
            let event = self.next().await.expect("stream ended before Done");
            let done = matches!(event, AgentEvent::Done { .. });
            events.push(event);
            if done {
                return events;
            }
        }
    }

    async fn until_text(&mut self, wanted: &str) {
        loop {
            match self.next().await {
                Some(AgentEvent::TextDelta { text }) if text == wanted => return,
                Some(_) => {}
                None => panic!("stream ended before {wanted:?}"),
            }
        }
    }
}

/// A run whose question dialogs are never answered by the user.
async fn start(harness: &AcpHarness, req: RunRequest) -> Run {
    let (steer, steer_rx) = mpsc::channel(8);
    let (answer_tx, answers) = mpsc::unbounded_channel();
    let interrupt = CancellationToken::new();
    let turn = TurnControl::default();
    let controls = RunControls {
        realtime: None,
        execution_lease: None,
        request_input: Box::new(move |_questions| {
            let (tx, rx) = oneshot::channel();
            let _ = answer_tx.send(tx);
            rx
        }),
        steering: steer_rx,
        interrupt: interrupt.clone(),
        turn: turn.clone(),
    };
    let stream = harness.run(req, controls).await.expect("run starts");
    Run {
        stream,
        steer,
        turn,
        _interrupt: interrupt,
        _answers: answers,
    }
}

fn done_of(events: &[AgentEvent]) -> (DoneStatus, Option<String>) {
    events
        .iter()
        .find_map(|e| match e {
            AgentEvent::Done { status, error, .. } => Some((*status, error.clone())),
            _ => None,
        })
        .expect("a Done")
}

fn texts(events: &[AgentEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::TextDelta { text } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

/// An agent that never settles a stopped turn: the stop closes the turn
/// Interrupted at its deadline and retires the process, ending the stream.
#[tokio::test]
async fn a_stop_the_agent_never_settles_closes_the_turn_at_its_deadline() {
    let harness = harness().with_stop_timeout(Duration::from_millis(500));
    let mut run = start(&harness, request("stop-wedge")).await;
    run.until_text("working").await;
    let started = std::time::Instant::now();
    run.turn.stop_turn();
    let events = run.until_done().await;
    assert_eq!(done_of(&events), (DoneStatus::Interrupted, None));
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
    assert!(
        run.next().await.is_none(),
        "the wedged runtime was not retired"
    );
}

/// ACP requires every pending permission request to be answered `cancelled`
/// once the prompt is cancelled; the agent here waits on exactly that.
#[tokio::test]
async fn a_stop_answers_an_open_question_cancelled() {
    let mut run = start(&harness(), request("question")).await;
    run.until_text("asking").await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    run.turn.stop_turn();
    let events = run.until_done().await;
    assert_eq!(texts(&events), ["question cancelled"]);
    assert_eq!(done_of(&events), (DoneStatus::Interrupted, None));
}

/// Grok's cancel carries the user gesture; the fixture settles a gesture-less
/// cancel as a refusal. Its initialize asserts no client type is declared.
#[tokio::test]
async fn grok_cancel_carries_its_meta_and_initialize_names_no_client_type() {
    let mut run = start(&harness(), request("grok-meta")).await;
    run.until_text("working").await;
    run.turn.stop_turn();
    let events = run.until_done().await;
    assert_eq!(done_of(&events), (DoneStatus::Interrupted, None));
}

/// A config switch the agent never answers must not hold off a stop.
#[tokio::test]
async fn a_stop_gets_through_an_unanswered_config_switch() {
    let mut run = start(&harness(), request("config-hang")).await;
    let events = run.until_done().await;
    assert_eq!(done_of(&events).0, DoneStatus::Completed);
    let mut next = request("config-hang");
    next.model = Some("grok-4-fast".into());
    run.steer
        .send(SteerMessage {
            config: Some(Box::new(next)),
            ..SteerMessage::text("switch")
        })
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let started = std::time::Instant::now();
    run.turn.stop_turn();
    let events = run.until_done().await;
    assert_eq!(done_of(&events), (DoneStatus::Interrupted, None));
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
}

/// The adapter dies mid-handshake while a detached descendant holds stdout
/// open: the crash surfaces at once, not after the 120s handshake bound.
#[tokio::test]
async fn a_crash_during_the_handshake_reports_without_waiting_for_eof() {
    let started = std::time::Instant::now();
    let mut run = start(&harness(), request("handshake-crash")).await;
    let events = run.until_done().await;
    let (status, error) = done_of(&events);
    assert_eq!(status, DoneStatus::Errored);
    let error = error.unwrap_or_default();
    assert!(error.contains("exited during the ACP handshake"), "{error}");
    assert!(error.contains("adapter blew up"), "{error}");
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
}

/// auth_required on session/new: the run authenticates with the agent's own
/// default method once and retries.
#[tokio::test]
async fn auth_required_signs_in_with_the_default_method_and_retries() {
    let mut run = start(&harness(), request("auth")).await;
    let events = run.until_done().await;
    assert_eq!(texts(&events), ["signed in"]);
    assert_eq!(done_of(&events), (DoneStatus::Completed, None));
}

#[tokio::test]
async fn a_terminal_only_sign_in_is_refused_with_a_clear_message() {
    let mut run = start(&harness(), request("auth-terminal")).await;
    let events = run.until_done().await;
    let (status, error) = done_of(&events);
    assert_eq!(status, DoneStatus::Errored);
    let error = error.unwrap_or_default();
    assert!(error.contains("Grok isn't signed in"), "{error}");
    assert!(
        error.contains("(terminal) can't run inside a chat"),
        "{error}"
    );
}

/// An agent advertising session/resume but not loadSession is restored with
/// resume, not a load it never offered.
#[tokio::test]
async fn resume_uses_the_advertised_restore_method() {
    let mut req = request("resume");
    req.resume = Some("old-session".into());
    let mut run = start(&harness(), req).await;
    let events = run.until_done().await;
    assert!(
        !events.iter().any(|e| matches!(e, AgentEvent::Error { .. })),
        "{events:?}"
    );
    assert!(events.iter().any(|e| matches!(
        e,
        AgentEvent::SessionStarted { session_id, .. } if session_id == "old-session"
    )));
    assert_eq!(texts(&events), ["resumed old-session"]);
}

/// An agent advertising image prompts gets attachments as image blocks, on
/// the first prompt and on later messages alike.
#[tokio::test]
async fn attachments_inline_as_image_blocks_when_advertised() {
    let dir = tempfile::tempdir().unwrap();
    let image = dir.path().join("shot.png");
    std::fs::write(&image, b"\x89PNG\r\n\x1a\nfake").unwrap();
    let image = image.display().to_string();
    let mut req = request("image");
    req.attachments = vec![image.clone()];
    let mut run = start(&harness(), req).await;
    let events = run.until_done().await;
    assert_eq!(texts(&events), ["text image/png"]);
    run.steer
        .send(SteerMessage {
            attachments: vec![image.clone(), image],
            ..SteerMessage::text("again")
        })
        .await
        .unwrap();
    let events = run.until_done().await;
    assert_eq!(texts(&events), ["text image/png image/png"]);
}

/// Without the capability the prompt stays a single text block.
#[tokio::test]
async fn attachments_stay_path_refs_without_the_image_capability() {
    let dir = tempfile::tempdir().unwrap();
    let image = dir.path().join("shot.png");
    std::fs::write(&image, b"\x89PNG\r\n\x1a\nfake").unwrap();
    let mut req = request("image-unsupported");
    req.attachments = vec![image.display().to_string()];
    let mut run = start(&harness(), req).await;
    let events = run.until_done().await;
    assert_eq!(texts(&events), ["text"]);
}
