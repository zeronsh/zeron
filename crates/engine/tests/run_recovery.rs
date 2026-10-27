//! Engine-side run recovery that applies to every provider:
//! - boot recovery revives a turn the engine cut off with a continuation when
//!   the provider already worked on it, and leaves alone what it must not
//!   revive (native commands, open questions, an exhausted revival budget);
//! - a graceful shutdown marks the turns it cuts off and the next boot picks
//!   them back up, while a user's Stop is never undone;
//! - a transient start failure retries with backoff, a deterministic one
//!   surfaces at once;
//! - a resume id the provider says is gone is retried once in a fresh
//!   session (never the dead id again, whichever record still names it, and
//!   the reply says so), which then replaces the stored id.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;

use zeron_doc::{
    MessagePart, MessageRole, MessageStatus, SessionCommandPayload, SessionDoc, SessionMessageEntry,
};
use zeron_engine::{
    Auth, AuthConfig, AuthState, EngineCore, EngineProfile, HarnessRegistry, RunJournal,
    WorkspaceScope,
};
use zeron_harness::{Harness, HarnessError, RunControls};
use zeron_proto::{
    AgentEvent, DoneStatus, HarnessId, Model, ReasoningLevel, RunRequest, SandboxLevel,
    SessionStatus, SteeringMode,
};
use zeron_sync::DocsStore;

const CHAT: &str = "chat-recovery";
const DEVICE: &str = "dev-recovery";

type RequestLog = Arc<Mutex<Vec<RunRequest>>>;
/// What a scripted run does, given its request and its 0-based call index.
type Script =
    Arc<dyn Fn(&RunRequest, usize) -> Result<Vec<AgentEvent>, HarnessError> + Send + Sync>;

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

fn session_started(session_id: &str) -> AgentEvent {
    AgentEvent::SessionStarted {
        harness: HarnessId::Mock,
        model: "mock-1".into(),
        tools: vec![],
        cwd: "/tmp".into(),
        session_id: session_id.into(),
        assistant_message_id: "a-1".into(),
    }
}

/// A full successful turn in session `session_id`.
fn completed_turn(session_id: &str, text: &str) -> Vec<AgentEvent> {
    vec![
        session_started(session_id),
        AgentEvent::TextDelta { text: text.into() },
        AgentEvent::Done {
            status: DoneStatus::Completed,
            result: None,
            error: None,
            session_id: Some(session_id.into()),
        },
    ]
}

/// Records every request and answers it from a script.
struct ScriptedHarness {
    requests: RequestLog,
    script: Script,
}

impl ScriptedHarness {
    /// Every run completes in session `session_id`.
    fn completing(requests: &RequestLog, session_id: &'static str) -> Self {
        Self {
            requests: requests.clone(),
            script: Arc::new(move |request, _| {
                Ok(completed_turn(
                    session_id,
                    &format!("ack: {}", request.prompt),
                ))
            }),
        }
    }
}

#[async_trait]
impl Harness for ScriptedHarness {
    fn id(&self) -> HarnessId {
        HarnessId::Mock
    }
    fn display_name(&self) -> &str {
        "Scripted"
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
        let call = {
            let mut log = self.requests.lock().unwrap();
            log.push(request.clone());
            log.len() - 1
        };
        let events = (self.script)(&request, call)?;
        Ok(futures::stream::iter(events.into_iter().map(Ok)).boxed())
    }
}

/// A [`ScriptedHarness`] registered as `id`, launching the CLI at `cli`.
struct AsProvider {
    id: HarnessId,
    cli: Option<std::path::PathBuf>,
    script: ScriptedHarness,
}

#[async_trait]
impl Harness for AsProvider {
    fn id(&self) -> HarnessId {
        self.id
    }
    fn display_name(&self) -> &str {
        self.id.as_str()
    }
    fn executable_path(&self) -> Option<std::path::PathBuf> {
        self.cli.clone()
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
        controls: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        self.script.run(request, controls).await
    }
}

/// Streams a little output in session `hs-held`, then holds the turn open
/// (or parks it on a question) until the engine interrupts it.
struct HoldingHarness {
    ask: bool,
    on_interrupt: OnInterrupt,
}

/// What a [`HoldingHarness`] turn does when the engine interrupts it.
#[derive(Clone, Copy)]
enum OnInterrupt {
    /// Its stream ends: the engine reads the turn as interrupted.
    End,
    /// The turn finishes just then: a completed `Done`.
    Finish,
    /// Nothing: the engine waits out its grace period.
    Ignore,
    /// Its teardown surfaces an error: an errored `Done`.
    Fail,
}

#[async_trait]
impl Harness for HoldingHarness {
    fn id(&self) -> HarnessId {
        HarnessId::Mock
    }
    fn display_name(&self) -> &str {
        "Holding"
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
        _request: RunRequest,
        controls: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let ask = self.ask;
        let on_interrupt = self.on_interrupt;
        tokio::spawn(async move {
            let _ = tx.send(Ok(session_started("hs-held")));
            let _ = tx.send(Ok(AgentEvent::TextDelta {
                text: "working on it…".into(),
            }));
            let _answer = ask.then(|| (controls.request_input)(Vec::new()));
            controls.interrupt.cancelled().await;
            match on_interrupt {
                OnInterrupt::End => drop(tx),
                OnInterrupt::Finish => {
                    let _ = tx.send(Ok(AgentEvent::Done {
                        status: DoneStatus::Completed,
                        result: None,
                        error: None,
                        session_id: Some("hs-held".into()),
                    }));
                }
                OnInterrupt::Ignore => std::future::pending::<()>().await,
                OnInterrupt::Fail => {
                    let _ = tx.send(Ok(AgentEvent::Done {
                        status: DoneStatus::Errored,
                        result: None,
                        error: Some("stdin closed: Broken pipe (os error 32)".into()),
                        session_id: Some("hs-held".into()),
                    }));
                }
            }
        });
        Ok(futures::stream::unfold(
            rx,
            |mut rx| async move { rx.recv().await.map(|ev| (ev, rx)) },
        )
        .boxed())
    }
}

fn assemble(dir: &std::path::Path, harness: impl Harness + 'static) -> EngineCore {
    let registry = HarnessRegistry::new();
    registry.register(Arc::new(harness));
    EngineCore::assemble(dir, Arc::new(registry), HarnessId::Mock, None)
        .expect("engine core assembles")
}

fn data_dir(tmp: &tempfile::TempDir) -> std::path::PathBuf {
    let dir = tmp.path().join("data");
    std::fs::create_dir_all(&dir).unwrap();
    // Pinned so manufactured entries count as this device's own.
    std::fs::write(dir.join("device-id"), DEVICE).unwrap();
    dir
}

fn journal(dir: &std::path::Path) -> RunJournal {
    RunJournal::open(dir.join("orgs/dev-org/dev-user/journals")).unwrap()
}

/// Create + name the chat row so the auto-titler stays out of the request log.
fn pre_title(core: &EngineCore) {
    core.workspace
        .create_space("space-recovery", &core.device_id, "/tmp", None, false)
        .expect("create space row");
    core.workspace
        .create_chat(CHAT, Some("space-recovery"), None, None, None)
        .expect("create chat row");
    core.workspace
        .rename_chat(CHAT, "Pre-titled")
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

async fn wait_for<F: FnMut() -> bool>(mut predicate: F, what: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !predicate() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(15)).await;
    }
}

fn entries_now(core: &EngineCore) -> Vec<SessionMessageEntry> {
    core.doc_host
        .open(CHAT)
        .ok()
        .and_then(|h| h.doc().read_entries().ok())
        .unwrap_or_default()
}

fn complete_assistant_count(core: &EngineCore) -> usize {
    entries_now(core)
        .iter()
        .filter(|e| e.role == MessageRole::Assistant && e.status == Some(MessageStatus::Complete))
        .count()
}

fn error_notes(entry: &SessionMessageEntry) -> Vec<String> {
    entry
        .parts
        .iter()
        .filter_map(|p| match p {
            MessagePart::Error { message, .. } => Some(message.clone()),
            _ => None,
        })
        .collect()
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

/// The on-disk state a kill -9 mid-turn leaves: the user's message, its
/// reply still `streaming` (when `partial` is given), and a journal that
/// never reached `Done`.
fn manufacture_crash(
    dir: &std::path::Path,
    prompt: &str,
    partial: Option<&str>,
    events: &[AgentEvent],
) {
    let now = now_ms();
    let store = DocsStore::open(dir.join("orgs/dev-org/dev-user")).unwrap();
    let doc = SessionDoc::init(CHAT).unwrap();
    let entry = |id: &str, role, text: &str, created_at, status| SessionMessageEntry {
        id: id.into(),
        role,
        parts: vec![MessagePart::Text {
            id: "t0".into(),
            text: text.into(),
        }],
        created_at,
        device_id: DEVICE.into(),
        status: Some(status),
        continuation_of: None,
        duration_ms: None,
    };
    doc.push_message(&entry(
        "msg-user-1",
        MessageRole::User,
        prompt,
        now - 60_000,
        MessageStatus::Complete,
    ))
    .unwrap();
    if let Some(partial) = partial {
        doc.push_message(&entry(
            "msg-assistant-1",
            MessageRole::Assistant,
            partial,
            now - 30_000,
            MessageStatus::Streaming,
        ))
        .unwrap();
    }
    store
        .save_snapshot(CHAT, &doc.export_snapshot().unwrap())
        .unwrap();
    let journal = journal(dir);
    for event in events {
        journal.append(CHAT, event).unwrap();
    }
}

fn aborted_entry(core: &EngineCore) -> SessionMessageEntry {
    entries_now(core)
        .into_iter()
        .find(|e| e.status == Some(MessageStatus::Aborted))
        .expect("the cut-off entry stays, stamped aborted")
}

/// Long enough for a revival that was (wrongly) armed to reach the harness.
async fn settle() {
    tokio::time::sleep(Duration::from_millis(400)).await;
}

#[tokio::test]
async fn a_crash_before_any_output_resends_the_original_prompt() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = data_dir(&tmp);
    // Started, but nothing streamed: the provider never worked on it.
    manufacture_crash(&dir, "long task", None, &[session_started("hs-crash")]);
    let requests: RequestLog = Default::default();
    let core = assemble(&dir, ScriptedHarness::completing(&requests, "hs-after"));
    wait_for(
        || complete_assistant_count(&core) == 1,
        "the revived turn to complete",
    )
    .await;
    let revived = requests.lock().unwrap()[0].clone();
    assert_eq!(revived.prompt, "long task");
    assert_eq!(revived.resume.as_deref(), Some("hs-crash"));
    core.shutdown().await;
}

#[tokio::test]
async fn a_crashed_native_command_is_not_revived() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = data_dir(&tmp);
    manufacture_crash(
        &dir,
        "/compact",
        Some("compacting…"),
        &[
            session_started("hs-crash"),
            AgentEvent::TextDelta {
                text: "compacting…".into(),
            },
        ],
    );
    let requests: RequestLog = Default::default();
    let core = assemble(&dir, ScriptedHarness::completing(&requests, "hs-after"));
    settle().await;
    assert!(
        requests.lock().unwrap().is_empty(),
        "maintenance commands are never re-run on their own"
    );
    let notes = error_notes(&aborted_entry(&core));
    assert!(
        notes
            .iter()
            .any(|n| n.contains("engine restart") && !n.contains("resuming")),
        "{notes:?}"
    );
    core.shutdown().await;
}

#[tokio::test]
async fn a_crashed_turn_that_starts_with_a_path_is_revived() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = data_dir(&tmp);
    let prompt = "/tmp/app/main.rs panics on start";
    manufacture_crash(
        &dir,
        prompt,
        Some("looking…"),
        &[
            session_started("hs-crash"),
            AgentEvent::TextDelta {
                text: "looking…".into(),
            },
        ],
    );
    let requests: RequestLog = Default::default();
    let core = assemble(&dir, ScriptedHarness::completing(&requests, "hs-after"));
    wait_for(
        || !requests.lock().unwrap().is_empty(),
        "the revived turn to reach the provider",
    )
    .await;
    let revived = requests.lock().unwrap()[0].clone();
    assert!(revived.prompt.contains("Continue where you left off"));
    assert!(revived.prompt.contains(prompt));
    core.shutdown().await;
}

#[tokio::test]
async fn a_crash_while_waiting_for_an_answer_is_not_revived() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = data_dir(&tmp);
    manufacture_crash(
        &dir,
        "deploy it",
        Some("Which environment?"),
        &[
            session_started("hs-crash"),
            AgentEvent::TextDelta {
                text: "Which environment?".into(),
            },
            AgentEvent::InputRequested {
                request_id: "q-1".into(),
                questions: vec![],
            },
        ],
    );
    let requests: RequestLog = Default::default();
    let core = assemble(&dir, ScriptedHarness::completing(&requests, "hs-after"));
    settle().await;
    assert!(requests.lock().unwrap().is_empty());
    let notes = error_notes(&aborted_entry(&core));
    assert!(
        notes.iter().any(|n| n.contains("waited for your answer")),
        "{notes:?}"
    );
    core.shutdown().await;
}

#[tokio::test]
async fn revivals_stop_after_three_in_a_row_and_say_so() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = data_dir(&tmp);
    manufacture_crash(
        &dir,
        "long task",
        Some("partial…"),
        &[
            session_started("hs-crash"),
            AgentEvent::TextDelta {
                text: "partial…".into(),
            },
        ],
    );
    // Three revivals in a row already died with the engine (a crash loop).
    let ledger = journal(&dir);
    for _ in 0..3 {
        ledger.note_resume_attempt(CHAT, now_ms(), 0);
    }
    let requests: RequestLog = Default::default();
    let core = assemble(&dir, ScriptedHarness::completing(&requests, "hs-after"));
    settle().await;
    assert!(requests.lock().unwrap().is_empty(), "the loop must stop");
    let notes = error_notes(&aborted_entry(&core));
    assert!(
        notes
            .iter()
            .any(|n| n.contains("not resumed automatically")),
        "the cap says why the run did not resume: {notes:?}"
    );
    core.shutdown().await;
}

#[tokio::test]
async fn a_message_the_user_sends_resets_the_revival_budget() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = data_dir(&tmp);
    journal(&dir).note_resume_attempt(CHAT, now_ms(), 0);
    let requests: RequestLog = Default::default();
    // The turn errors (no clean completion to reset the budget on its own).
    let core = assemble(
        &dir,
        ScriptedHarness {
            requests: requests.clone(),
            script: Arc::new(|_, _| {
                Ok(vec![
                    session_started("hs-1"),
                    AgentEvent::Done {
                        status: DoneStatus::Errored,
                        result: None,
                        error: Some("model overloaded".into()),
                        session_id: None,
                    },
                ])
            }),
        },
    );
    pre_title(&core);
    queue_run(&core, "hello", "msg-user-1");
    wait_for(
        || {
            core.sessions
                .session_status(CHAT)
                .is_some_and(|s| s.status == SessionStatus::Errored)
        },
        "the turn to end",
    )
    .await;
    assert_eq!(journal(&dir).resume_attempts(CHAT, 0), 0);
    core.shutdown().await;
}

/// Phase one of the shutdown tests: a turn streaming in a held run.
async fn hold_a_turn(dir: &std::path::Path, ask: bool) -> EngineCore {
    let core = assemble(
        dir,
        HoldingHarness {
            ask,
            on_interrupt: OnInterrupt::End,
        },
    );
    hold_a_turn_in(core, ask).await
}

/// [`hold_a_turn`] in an engine assembled by the caller.
async fn hold_a_turn_in(core: EngineCore, ask: bool) -> EngineCore {
    pre_title(&core);
    queue_run(&core, "long task", "msg-user-1");
    let want = if ask {
        SessionStatus::AwaitingInput
    } else {
        SessionStatus::Working
    };
    wait_for(
        || {
            core.sessions
                .session_status(CHAT)
                .is_some_and(|s| s.status == want)
                && entries_now(&core).iter().any(|e| {
                    e.role == MessageRole::Assistant
                        && e.parts.iter().any(
                            |p| matches!(p, MessagePart::Text { text, .. } if !text.is_empty()),
                        )
                })
        },
        "the held turn to stream",
    )
    .await;
    core
}

/// A harness whose teardown reports an error (its pipe broke as the engine
/// shut down) was cut off by the shutdown like any other: picked back up.
#[tokio::test]
async fn a_turn_whose_teardown_errors_during_shutdown_is_picked_back_up() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = data_dir(&tmp);
    let core = assemble(
        &dir,
        HoldingHarness {
            ask: false,
            on_interrupt: OnInterrupt::Fail,
        },
    );
    let core = hold_a_turn_in(core, false).await;
    core.shutdown().await;
    drop(core);

    let requests: RequestLog = Default::default();
    let core = assemble(&dir, ScriptedHarness::completing(&requests, "hs-held"));
    wait_for(
        || !requests.lock().unwrap().is_empty(),
        "the cut-off turn to be picked back up",
    )
    .await;
    {
        let log = requests.lock().unwrap();
        assert_eq!(log.len(), 1, "{log:#?}");
        assert!(log[0].prompt.contains("Continue where you left off"));
    }
    core.shutdown().await;
}

#[tokio::test]
async fn a_graceful_shutdown_mid_turn_is_picked_back_up() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = data_dir(&tmp);
    let core = hold_a_turn(&dir, false).await;
    core.shutdown().await;
    let notes = error_notes(&aborted_entry(&core));
    assert!(
        notes.iter().any(|n| n.contains("engine shutdown")),
        "the cut-off entry says why it ended: {notes:?}"
    );
    drop(core);

    let requests: RequestLog = Default::default();
    let core = assemble(&dir, ScriptedHarness::completing(&requests, "hs-held"));
    wait_for(
        || complete_assistant_count(&core) == 1,
        "the interrupted turn to be picked back up",
    )
    .await;
    {
        let log = requests.lock().unwrap();
        assert_eq!(log.len(), 1, "{log:#?}");
        assert!(
            log[0].prompt.contains("Continue where you left off"),
            "the session already holds the request: {:?}",
            log[0].prompt
        );
        // Still carried, should the resumed session have started over.
        assert!(
            log[0].prompt.contains("<request>\nlong task\n</request>"),
            "{:?}",
            log[0].prompt
        );
        assert_eq!(log[0].resume.as_deref(), Some("hs-held"));
    }
    // Still one user message, in the user's own words.
    let users: Vec<_> = entries_now(&core)
        .into_iter()
        .filter(|e| e.role == MessageRole::User)
        .collect();
    assert_eq!(users.len(), 1);
    assert!(matches!(&users[0].parts[0], MessagePart::Text { text, .. } if text == "long task"));
    core.shutdown().await;
    drop(core);

    // The mark is spent: a further restart revives nothing.
    let core = assemble(&dir, ScriptedHarness::completing(&requests, "hs-held"));
    settle().await;
    assert_eq!(requests.lock().unwrap().len(), 1);
    core.shutdown().await;
}

#[tokio::test]
async fn a_stopped_turn_is_not_revived_after_shutdown() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = data_dir(&tmp);
    let core = hold_a_turn(&dir, false).await;
    assert!(
        core.sessions.interrupt(CHAT).await.unwrap(),
        "the user's Stop"
    );
    core.shutdown().await;
    drop(core);

    let requests: RequestLog = Default::default();
    let core = assemble(&dir, ScriptedHarness::completing(&requests, "hs-held"));
    settle().await;
    assert!(
        requests.lock().unwrap().is_empty(),
        "a Stop is never undone"
    );
    core.shutdown().await;
}

#[tokio::test]
async fn a_shutdown_while_waiting_for_an_answer_is_not_revived() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = data_dir(&tmp);
    let core = hold_a_turn(&dir, true).await;
    core.shutdown().await;
    drop(core);

    let requests: RequestLog = Default::default();
    let core = assemble(&dir, ScriptedHarness::completing(&requests, "hs-held"));
    settle().await;
    assert!(requests.lock().unwrap().is_empty());
    core.shutdown().await;
}

#[tokio::test]
async fn a_turn_that_finishes_during_shutdown_is_not_revived() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = data_dir(&tmp);
    // Working when the shutdown began; done by the time it was stopped.
    let core = assemble(
        &dir,
        HoldingHarness {
            ask: false,
            on_interrupt: OnInterrupt::Finish,
        },
    );
    let core = hold_a_turn_in(core, false).await;
    core.shutdown().await;
    drop(core);
    assert!(journal(&dir).shutdown_interrupts().unwrap().is_empty());

    let requests: RequestLog = Default::default();
    let core = assemble(&dir, ScriptedHarness::completing(&requests, "hs-held"));
    settle().await;
    assert!(
        requests.lock().unwrap().is_empty(),
        "a finished turn is not carried on"
    );
    core.shutdown().await;
}

#[tokio::test]
async fn a_shutdown_mark_over_a_finished_turn_is_ignored() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = data_dir(&tmp);
    // An engine that marked its working turns up front, one of which then
    // finished before it was stopped.
    manufacture_crash(
        &dir,
        "long task",
        None,
        &completed_turn("hs-done", "all done"),
    );
    journal(&dir).note_shutdown_interrupt(CHAT, now_ms());
    let requests: RequestLog = Default::default();
    let core = assemble(&dir, ScriptedHarness::completing(&requests, "hs-after"));
    settle().await;
    assert!(requests.lock().unwrap().is_empty());
    assert!(journal(&dir).shutdown_interrupts().unwrap().is_empty());
    core.shutdown().await;
}

/// An engine over `dir` in `scope`, its account signed out.
fn assemble_signed_out(dir: &std::path::Path, scope: WorkspaceScope) -> EngineCore {
    let registry = HarnessRegistry::new();
    registry.register(Arc::new(HoldingHarness {
        ask: false,
        on_interrupt: OnInterrupt::End,
    }));
    let profile = match scope {
        WorkspaceScope::Synced => EngineProfile::synced(dir, "dev-org", "dev-user"),
        _ => EngineProfile::development(dir, "dev-org", "dev-user"),
    };
    let core =
        EngineCore::assemble_with_profile(profile, Arc::new(registry), HarnessId::Mock, None)
            .expect("engine core assembles");
    let mut config = AuthConfig::new("http://127.0.0.1:9", dir.join("auth"));
    config.workos_client_id = Some("client_test".into());
    let auth = Auth::new(config);
    assert!(matches!(auth.state(), AuthState::SignedOut));
    core.set_auth(auth);
    core
}

#[tokio::test]
async fn a_shutdown_after_sign_out_is_not_revived() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = data_dir(&tmp);
    // The runtime's own shutdown decides, from the auth state: no caller
    // has to remember to.
    let core = hold_a_turn_in(assemble_signed_out(&dir, WorkspaceScope::Synced), false).await;
    core.shutdown().await;
    drop(core);
    assert!(journal(&dir).shutdown_interrupts().unwrap().is_empty());

    let requests: RequestLog = Default::default();
    let core = assemble(&dir, ScriptedHarness::completing(&requests, "hs-held"));
    settle().await;
    assert!(requests.lock().unwrap().is_empty());
    core.shutdown().await;
}

#[tokio::test]
async fn a_signed_out_local_engine_still_revives_after_shutdown() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = data_dir(&tmp);
    // Signed out is a local engine's normal state, not a sign-out.
    let core = hold_a_turn_in(
        assemble_signed_out(&dir, WorkspaceScope::Development),
        false,
    )
    .await;
    core.shutdown().await;
    drop(core);

    let requests: RequestLog = Default::default();
    let core = assemble(&dir, ScriptedHarness::completing(&requests, "hs-held"));
    wait_for(
        || complete_assistant_count(&core) == 1,
        "the interrupted turn to be picked back up",
    )
    .await;
    assert_eq!(requests.lock().unwrap().len(), 1);
    core.shutdown().await;
}

/// A harness whose first `failures` starts are refused with `kind`, run
/// from the CLI at `cli`.
fn refusing(
    requests: &RequestLog,
    kind: std::io::ErrorKind,
    failures: usize,
    cli: Option<std::path::PathBuf>,
) -> AsProvider {
    AsProvider {
        id: HarnessId::Mock,
        cli,
        script: ScriptedHarness {
            requests: requests.clone(),
            script: Arc::new(move |request, call| {
                if call < failures {
                    Err(HarnessError::Io(std::io::Error::from(kind)))
                } else {
                    Ok(completed_turn("hs-1", &format!("ack: {}", request.prompt)))
                }
            }),
        },
    }
}

#[tokio::test]
async fn a_transient_start_failure_retries_with_backoff() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = data_dir(&tmp);
    let requests: RequestLog = Default::default();
    // Antivirus holding a freshly updated CLI: two refused spawns.
    let cli = tmp.path().join("mock-cli");
    std::fs::write(&cli, b"updated just now").unwrap();
    let core = assemble(
        &dir,
        refusing(
            &requests,
            std::io::ErrorKind::PermissionDenied,
            2,
            Some(cli),
        ),
    );
    pre_title(&core);
    queue_run(&core, "hello", "msg-user-1");
    wait_for(
        || complete_assistant_count(&core) == 1,
        "the retried turn to complete",
    )
    .await;
    assert_eq!(requests.lock().unwrap().len(), 3, "two retries");
    let entries = entries_now(&core);
    assert_eq!(entries.len(), 2, "one user entry, one reply: {entries:#?}");
    assert!(error_notes(&entries[1]).is_empty());
    core.shutdown().await;
}

#[tokio::test]
async fn a_refused_permission_without_a_fresh_cli_surfaces_at_once() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = data_dir(&tmp);
    let requests: RequestLog = Default::default();
    // A CLI that lost its execute bit, untouched for a day: no retry fixes it.
    let cli = tmp.path().join("mock-cli");
    let file = std::fs::File::create(&cli).unwrap();
    file.set_modified(std::time::SystemTime::now() - Duration::from_secs(24 * 60 * 60))
        .unwrap();
    drop(file);
    let core = assemble(
        &dir,
        refusing(
            &requests,
            std::io::ErrorKind::PermissionDenied,
            usize::MAX,
            Some(cli),
        ),
    );
    pre_title(&core);
    queue_run(&core, "hello", "msg-user-1");
    wait_for(
        || {
            core.sessions
                .session_status(CHAT)
                .is_some_and(|s| s.status == SessionStatus::Errored)
        },
        "the start failure to surface",
    )
    .await;
    settle().await;
    assert_eq!(requests.lock().unwrap().len(), 1, "no retry");
    core.shutdown().await;
}

#[tokio::test]
async fn a_deterministic_start_failure_surfaces_at_once() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = data_dir(&tmp);
    let requests: RequestLog = Default::default();
    let core = assemble(
        &dir,
        ScriptedHarness {
            requests: requests.clone(),
            script: Arc::new(|_, _| Err(HarnessError::NotInstalled("install mock".into()))),
        },
    );
    pre_title(&core);
    queue_run(&core, "hello", "msg-user-1");
    wait_for(
        || {
            core.sessions
                .session_status(CHAT)
                .is_some_and(|s| s.status == SessionStatus::Errored)
        },
        "the start failure to surface",
    )
    .await;
    settle().await;
    assert_eq!(requests.lock().unwrap().len(), 1, "no retry");
    core.shutdown().await;
}

#[tokio::test]
async fn a_resume_the_provider_rejects_retries_in_a_fresh_session() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = data_dir(&tmp);
    let requests: RequestLog = Default::default();
    {
        let core = assemble(&dir, ScriptedHarness::completing(&requests, "hs-gone"));
        pre_title(&core);
        queue_run(&core, "first", "msg-user-1");
        wait_for(|| complete_assistant_count(&core) == 1, "first turn").await;
        core.shutdown().await;
    }
    // The provider has since lost that conversation (its store was wiped).
    let core = assemble(
        &dir,
        ScriptedHarness {
            requests: requests.clone(),
            script: Arc::new(|request, _| {
                Ok(match &request.resume {
                    Some(id) => vec![AgentEvent::Done {
                        status: DoneStatus::Errored,
                        result: None,
                        error: Some(format!(
                            "Mock exited unexpectedly (exit code 1): No conversation found with session ID: {id}"
                        )),
                        session_id: None,
                    }],
                    None => completed_turn("hs-fresh", "fresh start"),
                })
            }),
        },
    );
    queue_run(&core, "second", "msg-user-2");
    wait_for(|| complete_assistant_count(&core) == 2, "second turn").await;
    {
        let log = requests.lock().unwrap();
        assert_eq!(log.len(), 3, "{log:#?}");
        assert_eq!(log[1].resume.as_deref(), Some("hs-gone"));
        assert_eq!(log[2].resume, None, "the retry starts a fresh session");
        assert_eq!(log[2].prompt, "second");
    }
    // The fresh session replaces the dead id: the next turn resumes it.
    queue_run(&core, "third", "msg-user-3");
    wait_for(|| complete_assistant_count(&core) == 3, "third turn").await;
    assert_eq!(
        requests.lock().unwrap()[3].resume.as_deref(),
        Some("hs-fresh")
    );
    core.shutdown().await;
}

#[tokio::test]
async fn a_rejected_resume_in_a_switched_chat_starts_fresh_and_says_so() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = data_dir(&tmp);
    let mock_log: RequestLog = Default::default();
    let pi_log: RequestLog = Default::default();
    let registry = HarnessRegistry::new();
    registry.register(Arc::new(ScriptedHarness::completing(&mock_log, "hs-mock")));
    // Pi's first session is lost after a while; every later one sticks.
    registry.register(Arc::new(AsProvider {
        id: HarnessId::Pi,
        cli: None,
        script: ScriptedHarness {
            requests: pi_log.clone(),
            script: Arc::new(|request, call| {
                Ok(match (call, request.resume.as_deref()) {
                    (0, _) => completed_turn("hs-pi", "pi answer"),
                    (_, Some("hs-pi")) => vec![AgentEvent::Done {
                        status: DoneStatus::Errored,
                        result: None,
                        error: Some("Pi exited unexpectedly: session not found: hs-pi".into()),
                        session_id: None,
                    }],
                    _ => completed_turn("hs-pi-2", "pi again"),
                })
            }),
        },
    }));
    let core = EngineCore::assemble(&dir, Arc::new(registry), HarnessId::Mock, None)
        .expect("engine core assembles");
    pre_title(&core);
    let turn = |harness, prompt: &'static str, id: &'static str, replies| {
        let core = &core;
        async move {
            core.sessions
                .dispatch(CHAT, harness, run_request(prompt), Some(id.into()))
                .await
                .unwrap();
            wait_for(|| complete_assistant_count(core) == replies, prompt).await;
            // The provider marks ride the Done; let them land.
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    };
    turn(HarnessId::Mock, "one", "msg-user-1", 1).await;
    turn(HarnessId::Pi, "two", "msg-user-2", 2).await;
    turn(HarnessId::Mock, "three", "msg-user-3", 3).await;
    // Back on Pi: the chat's record of Pi's session names `hs-pi`, gone now.
    turn(HarnessId::Pi, "four", "msg-user-4", 4).await;
    {
        let log = pi_log.lock().unwrap();
        assert_eq!(log.len(), 3, "{log:#?}");
        assert_eq!(log[1].resume.as_deref(), Some("hs-pi"));
        assert_eq!(log[2].resume, None, "the retry never resumes the dead id");
    }
    let reply = entries_now(&core).pop().unwrap();
    assert_eq!(reply.role, MessageRole::Assistant);
    let notes = error_notes(&reply);
    assert!(
        notes.iter().any(|n| n.contains("starts a new one")),
        "the reply says the session started over: {notes:?}"
    );
    // The new session replaces the dead one.
    turn(HarnessId::Mock, "five", "msg-user-5", 5).await;
    turn(HarnessId::Pi, "six", "msg-user-6", 6).await;
    assert_eq!(pi_log.lock().unwrap()[3].resume.as_deref(), Some("hs-pi-2"));
    core.shutdown().await;
}

/// Holds a dispatch inside its own awaits, before its run starts: the
/// engine reads the harness's CLI there (`binary_identity`), so the first
/// CLI lookup once armed blocks until released.
#[derive(Clone, Default)]
struct DispatchGate {
    armed: Arc<std::sync::atomic::AtomicBool>,
    entered: Arc<std::sync::atomic::AtomicBool>,
    released: Arc<std::sync::atomic::AtomicBool>,
}

impl DispatchGate {
    fn arm(&self) {
        self.armed.store(true, std::sync::atomic::Ordering::SeqCst);
    }
    fn entered(&self) -> bool {
        self.entered.load(std::sync::atomic::Ordering::SeqCst)
    }
    fn release(&self) {
        self.released
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }
    fn pass(&self) {
        if self.armed.swap(false, std::sync::atomic::Ordering::SeqCst) {
            self.entered
                .store(true, std::sync::atomic::Ordering::SeqCst);
            while !self.released.load(std::sync::atomic::Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }
}

/// A [`ScriptedHarness`] whose CLI lookup passes through a [`DispatchGate`].
struct GatedHarness {
    gate: DispatchGate,
    script: ScriptedHarness,
}

#[async_trait]
impl Harness for GatedHarness {
    fn id(&self) -> HarnessId {
        HarnessId::Mock
    }
    fn display_name(&self) -> &str {
        "Gated"
    }
    fn executable_path(&self) -> Option<std::path::PathBuf> {
        self.gate.pass();
        None
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
        controls: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        let gate = self.gate.clone();
        let started = self.script.run(request, controls).await;
        // The next dispatch (the retry of a failed start) is held.
        if started.is_err() {
            gate.arm();
        }
        started
    }
}

#[tokio::test]
async fn a_stop_between_a_failed_start_and_its_retry_ends_the_turn() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = data_dir(&tmp);
    let requests: RequestLog = Default::default();
    let gate = DispatchGate::default();
    let core = assemble(
        &dir,
        GatedHarness {
            gate: gate.clone(),
            script: ScriptedHarness {
                requests: requests.clone(),
                script: Arc::new(|request, call| {
                    if call == 0 {
                        Err(HarnessError::Io(std::io::Error::from(
                            std::io::ErrorKind::ResourceBusy,
                        )))
                    } else {
                        Ok(completed_turn("hs-1", &format!("ack: {}", request.prompt)))
                    }
                }),
            },
        },
    );
    pre_title(&core);
    queue_run(&core, "hello", "msg-user-1");
    // The failed run is gone and its retry not started yet: no run is live
    // for the Stop to find.
    wait_for(|| gate.entered(), "the retry to be on its way").await;
    assert!(!core.sessions.has_run(CHAT));
    core.sessions.interrupt(CHAT).await.unwrap();
    gate.release();
    settle().await;
    assert_eq!(
        requests.lock().unwrap().len(),
        1,
        "the stopped turn never starts again"
    );
    assert_eq!(
        core.sessions.session_status(CHAT).map(|s| s.status),
        Some(SessionStatus::Idle)
    );
    core.shutdown().await;
}

#[tokio::test]
async fn a_shutdown_during_a_start_retry_starts_nothing_and_revives_on_boot() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = data_dir(&tmp);
    let requests: RequestLog = Default::default();
    let core = assemble(
        &dir,
        refusing(&requests, std::io::ErrorKind::ResourceBusy, 1, None),
    );
    pre_title(&core);
    queue_run(&core, "hello", "msg-user-1");
    wait_for(|| !requests.lock().unwrap().is_empty(), "the failed start").await;
    // Inside the retry's backoff.
    core.shutdown().await;
    settle().await;
    assert_eq!(
        requests.lock().unwrap().len(),
        1,
        "nothing starts during shutdown"
    );
    drop(core);

    let requests: RequestLog = Default::default();
    let core = assemble(&dir, ScriptedHarness::completing(&requests, "hs-1"));
    wait_for(
        || complete_assistant_count(&core) == 1,
        "the cut-off turn to be picked back up",
    )
    .await;
    assert_eq!(requests.lock().unwrap()[0].prompt, "hello");
    core.shutdown().await;
}

#[tokio::test]
async fn a_revival_yields_to_a_message_sent_while_it_was_on_its_way() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = data_dir(&tmp);
    manufacture_crash(
        &dir,
        "long task",
        Some("partial…"),
        &[
            session_started("hs-crash"),
            AgentEvent::TextDelta {
                text: "partial…".into(),
            },
        ],
    );
    let requests: RequestLog = Default::default();
    let gate = DispatchGate::default();
    // Boot recovery's revival is held before its run starts.
    gate.arm();
    let core = assemble(
        &dir,
        GatedHarness {
            gate: gate.clone(),
            script: ScriptedHarness::completing(&requests, "hs-crash"),
        },
    );
    wait_for(|| gate.entered(), "the revival to be on its way").await;
    // Meanwhile the user sends something new.
    core.sessions
        .dispatch(
            CHAT,
            HarnessId::Mock,
            run_request("something else"),
            Some("msg-user-2".into()),
        )
        .await
        .unwrap();
    wait_for(
        || !requests.lock().unwrap().is_empty(),
        "the new message to reach the provider",
    )
    .await;
    gate.release();
    settle().await;
    let log = requests.lock().unwrap().clone();
    assert_eq!(
        log.len(),
        1,
        "the old turn is not delivered again: {log:#?}"
    );
    assert_eq!(log[0].prompt, "something else");
    core.shutdown().await;
}

/// A sign-out's shutdown that dies before it reached a turn (killed in its
/// serial terminate loop) still never revives that turn.
#[test]
fn a_sign_out_shutdown_that_dies_midway_revives_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = data_dir(&tmp);
    {
        let engine = tokio::runtime::Runtime::new().unwrap();
        engine.block_on(async {
            let core = assemble(
                &dir,
                HoldingHarness {
                    ask: false,
                    on_interrupt: OnInterrupt::Ignore,
                },
            );
            let core = hold_a_turn_in(core, false).await;
            core.sessions.forgo_revival();
            let sessions = core.sessions.clone();
            tokio::spawn(async move { sessions.shutdown().await });
            tokio::time::sleep(Duration::from_millis(200)).await;
            core.doc_host.flush_all();
            drop(core);
        });
        engine.shutdown_timeout(Duration::from_millis(100));
    }
    assert!(
        journal(&dir)
            .stale_sessions()
            .unwrap()
            .contains(&CHAT.to_string()),
        "the journal reads like a crash"
    );

    let engine = tokio::runtime::Runtime::new().unwrap();
    engine.block_on(async {
        let requests: RequestLog = Default::default();
        let core = assemble(&dir, ScriptedHarness::completing(&requests, "hs-held"));
        settle().await;
        assert!(
            requests.lock().unwrap().is_empty(),
            "a sign-out is never undone"
        );
        core.shutdown().await;
    });
}

#[test]
fn a_stop_the_engine_dies_before_settling_is_not_revived() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = data_dir(&tmp);
    {
        let engine = tokio::runtime::Runtime::new().unwrap();
        engine.block_on(async {
            // A harness slow to let go: the stopped run settles only after
            // its grace period.
            let core = assemble(
                &dir,
                HoldingHarness {
                    ask: false,
                    on_interrupt: OnInterrupt::Ignore,
                },
            );
            let core = hold_a_turn_in(core, false).await;
            let sessions = core.sessions.clone();
            tokio::spawn(async move { sessions.interrupt(CHAT).await });
            tokio::time::sleep(Duration::from_millis(200)).await;
            core.doc_host.flush_all();
            drop(core);
        });
        // The engine dies before the stopped run has closed its journal.
        engine.shutdown_timeout(Duration::from_millis(100));
    }
    assert!(
        journal(&dir)
            .stale_sessions()
            .unwrap()
            .contains(&CHAT.to_string()),
        "the journal reads like a crash"
    );

    let engine = tokio::runtime::Runtime::new().unwrap();
    engine.block_on(async {
        let requests: RequestLog = Default::default();
        let core = assemble(&dir, ScriptedHarness::completing(&requests, "hs-held"));
        settle().await;
        assert!(
            requests.lock().unwrap().is_empty(),
            "a Stop is never undone"
        );
        core.shutdown().await;
    });
}
