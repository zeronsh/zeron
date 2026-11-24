//! Delegated work, end to end: a parent hands turns to children on other
//! harnesses (and devices), ends its own turn into Waiting, and is woken once
//! with what they came to. Its completion is marked when that is done and
//! not before; a Stop ends the children with it; a restart loses nothing.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;
use serde_json::{Value, json};
use tokio::sync::{mpsc, watch};

use zeron_doc::{MessagePart, MessageRole, SessionCommandPayload, SessionMessageEntry};
use zeron_engine::delegation::Timing;
use zeron_engine::{EngineCore, HarnessRegistry};
use zeron_harness::{Harness, HarnessError, RunControls};
use zeron_proto::delegation::wake_notice;
use zeron_proto::{
    AccessMode, AgentEvent, ChatConfig, ChildKind, DelegatedStatus, DelegatedTaskView, DeliveryState, DoneStatus,
    HarnessId, Model, NativeTurnOutcome, ReasoningLevel, RunRequest, SandboxLevel, Session, SessionStatus,
    SteeringMode, UserInputAnswer, UserInputQuestion, WaitingKind, WakeKind, WakeNotice,
};
use zeron_rpc::methods;

/// Gates the test opens by name, and what each agent was asked.
#[derive(Default)]
struct Stage {
    gates: Mutex<HashMap<String, watch::Sender<bool>>>,
    prompts: Mutex<Vec<String>>,
}

impl Stage {
    fn gate(&self, name: &str) -> watch::Receiver<bool> {
        self.gates
            .lock()
            .unwrap()
            .entry(name.to_owned())
            .or_insert_with(|| watch::channel(false).0)
            .subscribe()
    }

    fn open(&self, name: &str) {
        drop(self.gate(name));
        self.gates.lock().unwrap()[name].send_replace(true);
    }
}

/// Plays a turn: `hold NAME` waits for the gate and replies `result NAME`,
/// `ask` and `approve` ask first, `fail` errors; anything else replies at once.
struct Scripted {
    id: HarnessId,
    /// Keeps its runtime between turns (else one process per turn, which
    /// takes no steering).
    persistent: bool,
    stage: Arc<Stage>,
}

fn done(status: DoneStatus, error: Option<String>) -> AgentEvent {
    AgentEvent::Done {
        status,
        result: None,
        error,
        session_id: Some("provider-session".into()),
    }
}

async fn play(
    stage: &Stage,
    prompt: &str,
    tx: &mpsc::Sender<Result<AgentEvent, HarnessError>>,
    controls: &mut RunControls,
) -> bool {
    stage.prompts.lock().unwrap().push(prompt.to_owned());
    let mut reply = format!("ok {}", prompt.chars().take(12).collect::<String>());
    if let Some(name) = prompt.strip_prefix("hold ") {
        let mut gate = stage.gate(name);
        tokio::select! {
            () = async { drop(gate.wait_for(|open| *open).await) } => {}
            _ = controls.interrupt.cancelled() => {
                let _ = tx.send(Ok(done(DoneStatus::Interrupted, None))).await;
                return false;
            }
        }
        reply = format!("result {name}");
    }
    if prompt.starts_with("ask") || prompt.starts_with("approve") {
        let question = UserInputQuestion {
            id: "q1".into(),
            header: if prompt.starts_with("approve") { "Approve command".into() } else { "Branch".into() },
            question: "Which branch?".into(),
            options: if prompt.starts_with("approve") {
                vec!["Yes".into(), "No".into()]
            } else {
                vec!["main".into(), "next".into()]
            },
            multi_select: false,
            prefill: None,
            multiline: false,
            note: None,
        };
        let answers = (controls.request_input)(vec![question]).await.unwrap_or_default();
        reply = format!(
            "answered {}",
            answers.first().and_then(|a| a.labels.first()).cloned().unwrap_or_default()
        );
    }
    if prompt.starts_with("fail") {
        let _ = tx.send(Ok(done(DoneStatus::Errored, Some("boom".into())))).await;
        return false;
    }
    let _ = tx.send(Ok(AgentEvent::TextDelta { text: reply })).await;
    let _ = tx.send(Ok(done(DoneStatus::Completed, None))).await;
    true
}

#[async_trait]
impl Harness for Scripted {
    fn id(&self) -> HarnessId {
        self.id
    }
    fn display_name(&self) -> &str {
        "Scripted"
    }
    fn supports_steering(&self) -> bool {
        self.persistent
    }
    fn steering_mode(&self) -> SteeringMode {
        SteeringMode::TurnBoundary
    }
    fn reasoning_levels(&self) -> &[ReasoningLevel] {
        &[]
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
        let (tx, rx) = mpsc::channel(16);
        let (stage, persistent) = (self.stage.clone(), self.persistent);
        tokio::spawn(async move {
            if !play(&stage, &request.prompt, &tx, &mut controls).await || !persistent {
                return;
            }
            loop {
                tokio::select! {
                    steer = controls.steering.recv() => {
                        let Some(steer) = steer else { return };
                        let _ = tx
                            .send(Ok(AgentEvent::Steered { assistant_message_id: None, next_assistant_message_id: None }))
                            .await;
                        if !play(&stage, &steer.prompt, &tx, &mut controls).await {
                            return;
                        }
                    }
                    _ = controls.interrupt.cancelled() => {
                        let _ = tx.send(Ok(done(DoneStatus::Interrupted, None))).await;
                        return;
                    }
                }
            }
        });
        Ok(tokio_stream(rx))
    }
}

fn tokio_stream(
    rx: mpsc::Receiver<Result<AgentEvent, HarnessError>>,
) -> BoxStream<'static, Result<AgentEvent, HarnessError>> {
    futures::stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|e| (e, rx)) }).boxed()
}

fn fast() -> Timing {
    Timing {
        coalesce: Duration::from_millis(150),
        question_grace: Duration::from_millis(100),
        tick: Duration::from_millis(100),
        reply_local: Duration::from_secs(2),
        reply_remote: Duration::from_millis(300),
    }
}

struct Rig {
    core: EngineCore,
    stage: Arc<Stage>,
    /// Every distinct state the parent's row was published in.
    seen: Arc<Mutex<Vec<Session>>>,
}

fn assemble(dir: &Path, stage: &Arc<Stage>, parent_persistent: bool) -> EngineCore {
    let registry = HarnessRegistry::new();
    for (id, persistent) in [
        (HarnessId::Mock, parent_persistent),
        (HarnessId::Codex, true),
        (HarnessId::Opencode, false),
    ] {
        registry.register(Arc::new(Scripted {
            id,
            persistent,
            stage: stage.clone(),
        }));
    }
    let core = EngineCore::assemble(dir, Arc::new(registry), HarnessId::Mock, None).unwrap();
    core.sessions.delegations().set_timing(fast());
    core
}

fn config(harness: HarnessId) -> ChatConfig {
    ChatConfig {
        harness,
        model: None,
        reasoning: None,
        model_options: Default::default(),
        sandbox: SandboxLevel::WorkspaceWrite,
        access: AccessMode::FullAccess,
        plan: false,
    }
}

fn run_request(harness: HarnessId, prompt: &str) -> RunRequest {
    RunRequest {
        prompt: prompt.into(),
        harness: Some(harness),
        model: None,
        reasoning: None,
        model_options: Default::default(),
        cwd: std::env::temp_dir().display().to_string(),
        sandbox: SandboxLevel::WorkspaceWrite,
        auto_approve: false,
        access: AccessMode::FullAccess,
        plan: false,
        code_mode: false,
        zeron_mcp_only: false,
        resume: None,
        attachments: Vec::new(),
        worktree: None,
        mcp: None,
        env: Default::default(),
        extra_mcp: Vec::new(),
    }
}

impl Rig {
    fn new(dir: &Path, parent_persistent: bool) -> Self {
        let stage = Arc::new(Stage::default());
        let core = assemble(dir, &stage, parent_persistent);
        core.workspace
            .create_chat(
                "parent",
                None,
                Some(&core.device_id),
                Some(config(HarnessId::Mock)),
                Some(std::env::temp_dir().display().to_string()),
            )
            .unwrap();
        Self::watch(core, stage)
    }

    fn watch(core: EngineCore, stage: Arc<Stage>) -> Self {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let mut rows = core.sessions.watch_sessions();
        let log = seen.clone();
        tokio::spawn(async move {
            while rows.changed().await.is_ok() {
                if let Some(row) = rows.borrow_and_update().iter().find(|s| s.chat_id == "parent") {
                    log.lock().unwrap().push(row.clone());
                }
            }
        });
        Self { core, stage, seen }
    }

    fn child(&self, id: &str, harness: HarnessId, device: &str) {
        self.core
            .workspace
            .create_chat_hosted(
                id,
                None,
                Some(device),
                None,
                Some(config(harness)),
                Some(std::env::temp_dir().display().to_string()),
                Some("parent".into()),
                Some(ChildKind::Subagent),
            )
            .unwrap();
    }

    fn send(&self, chat: &str, harness: HarnessId, prompt: &str) -> String {
        let message_id = uuid::Uuid::new_v4().to_string();
        self.core
            .doc_host
            .queue_command(
                chat,
                SessionCommandPayload::Run {
                    request: run_request(harness, prompt),
                    message_id: message_id.clone(),
                },
            )
            .unwrap();
        message_id
    }

    async fn rpc(&self, method: &str, params: Value) -> Result<Value, String> {
        zeron_rpc::memory_client(self.core.rpc_service())
            .call(method, params)
            .await
            .map_err(|e| e.to_string())
    }

    /// What create_chat's `prompt` does: recorded, then sent.
    async fn delegate(&self, child: &str, harness: HarnessId, prompt: &str, wake: &str) -> String {
        let message_id = uuid::Uuid::new_v4().to_string();
        let task = self
            .rpc(
                methods::REGISTER_DELEGATED_TASK,
                json!({ "chatId": "parent", "childChatId": child, "messageId": message_id, "title": child, "completionWake": wake }),
            )
            .await
            .unwrap();
        self.core
            .doc_host
            .queue_command(
                child,
                SessionCommandPayload::Run {
                    request: run_request(harness, prompt),
                    message_id,
                },
            )
            .unwrap();
        task["taskId"].as_str().unwrap().to_owned()
    }

    async fn tasks(&self) -> Vec<DelegatedTaskView> {
        serde_json::from_value(
            self.rpc(methods::LIST_DELEGATED_TASKS, json!({ "chatId": "parent" }))
                .await
                .unwrap(),
        )
        .unwrap()
    }

    fn row(&self) -> Session {
        self.core.sessions.session_status("parent").expect("the parent has a row")
    }

    fn status_of(&self, chat: &str) -> Option<SessionStatus> {
        self.core.sessions.session_status(chat).map(|s| s.status)
    }

    fn entries(&self, chat: &str) -> Vec<SessionMessageEntry> {
        self.core.doc_host.open(chat).unwrap().doc().read_entries().unwrap()
    }

    fn wakes(&self) -> Vec<(WakeNotice, String)> {
        self.entries("parent")
            .iter()
            .filter(|e| e.role == MessageRole::User)
            .filter_map(|e| match &e.parts[0] {
                MessagePart::Text { text, .. } => wake_notice(text).map(|(n, body)| (n, body.to_owned())),
                _ => None,
            })
            .collect()
    }

    /// The parent's turn running on `hold main`, so its tasks are its own.
    async fn parent_working(&self) {
        self.send("parent", HarnessId::Mock, "hold main");
        self.until("the parent works", |r| r.status_of("parent") == Some(SessionStatus::Working)).await;
    }

    async fn parent_parked(&self) {
        self.stage.open("main");
        self.until("the parent waits", |r| {
            let row = r.row();
            row.status == SessionStatus::Idle && row.waiting.is_some()
        })
        .await;
    }

    async fn until(&self, what: &str, done: impl Fn(&Self) -> bool) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        while !done(self) {
            assert!(
                tokio::time::Instant::now() < deadline,
                "never: {what}\nparent: {:?}\ntasks: {:#?}",
                self.core.sessions.session_status("parent"),
                self.core.sessions.delegations().of_parent("parent"),
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    async fn finished(&self) {
        self.until("the parent finishes", |r| {
            let row = r.row();
            row.status == SessionStatus::Idle && row.waiting.is_none() && row.last_completed_turn.is_some()
        })
        .await;
    }

    /// Nothing published so far read as the parent being finished.
    fn never_completed(&self) {
        for row in self.seen.lock().unwrap().iter() {
            assert!(row.last_completed_turn.is_none(), "a completion flickered: {row:?}");
            assert!(row.last_turn_outcome.is_none(), "an outcome flickered: {row:?}");
        }
    }
}

/// Two children on two other harnesses (one of which takes no steering and
/// exits after its turn), whichever kind of runtime the parent has: the
/// parent waits without ever reading finished, one wake brings both results,
/// and its completion is that wake turn's.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_fan_out_parks_the_parent_and_one_wake_brings_every_result() {
    for persistent in [true, false] {
        let dir = tempfile::tempdir().unwrap();
        let rig = Rig::new(dir.path(), persistent);
        rig.child("a", HarnessId::Codex, &rig.core.device_id);
        rig.child("b", HarnessId::Opencode, &rig.core.device_id);
        rig.parent_working().await;
        rig.delegate("a", HarnessId::Codex, "hold a", "always").await;
        rig.delegate("b", HarnessId::Opencode, "hold b", "always").await;
        rig.parent_parked().await;
        let row = rig.row();
        let on = &row.waiting.as_ref().unwrap().on;
        assert_eq!(on.len(), 2);
        assert!(on.iter().all(|w| w.kind == WaitingKind::SubagentChat));
        assert_eq!(on[0].child_chat_id.as_deref(), Some("a"));
        assert_eq!(on[0].device_id.as_deref(), Some(rig.core.device_id.as_str()));
        assert_eq!(on[0].harness, Some(HarnessId::Codex));
        rig.until("both run", |r| {
            r.core
                .sessions
                .delegations()
                .of_parent("parent")
                .iter()
                .all(|t| t.status == DelegatedStatus::Running)
        })
        .await;

        // (Past every timer there is: it waits for as long as they run.)
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert!(rig.row().waiting.is_some());
        rig.never_completed();
        assert!(rig.wakes().is_empty());

        rig.stage.open("a");
        rig.stage.open("b");
        rig.finished().await;
        let wakes = rig.wakes();
        assert_eq!(wakes.len(), 1, "persistent {persistent}: one wake for both");
        let (notice, body) = &wakes[0];
        assert_eq!(notice.kind, WakeKind::Results);
        assert_eq!(notice.tasks.len(), 2);
        assert_eq!(notice.open, 0);
        assert!(notice.tasks.iter().all(|t| t.status == DelegatedStatus::Completed));
        assert!(body.contains("result a") && body.contains("result b"), "{body}");
        assert!(rig.tasks().await.iter().all(|t| t.task.delivery_state == DeliveryState::Acknowledged));
        // It was never read as finished before the wake's own turn began.
        let seen = rig.seen.lock().unwrap().clone();
        let woke = seen.iter().rposition(|r| r.status == SessionStatus::Working).unwrap();
        for row in &seen[..woke] {
            assert!(row.last_completed_turn.is_none(), "{row:?}");
        }
        assert!(
            rig.core.workspace.chat("parent").unwrap().unwrap().last_message_preview.is_none_or(|p| !p.contains("delegated")),
            "a wake is no preview"
        );
        rig.core.shutdown().await;
    }
}

/// A child that settles alone wakes the parent alone; its wake turn's end
/// is held like the first, until the last child is in.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn each_result_wakes_the_parent_and_only_the_last_one_finishes_it() {
    let dir = tempfile::tempdir().unwrap();
    let rig = Rig::new(dir.path(), true);
    rig.child("a", HarnessId::Codex, &rig.core.device_id);
    rig.child("b", HarnessId::Opencode, &rig.core.device_id);
    rig.parent_working().await;
    rig.delegate("a", HarnessId::Codex, "fail now", "always").await;
    rig.delegate("b", HarnessId::Opencode, "hold b", "always").await;
    rig.parent_parked().await;
    rig.until("the failure is told", |r| r.wakes().len() == 1).await;
    let (notice, body) = rig.wakes().remove(0);
    assert_eq!(notice.tasks[0].status, DelegatedStatus::Failed);
    assert_eq!(notice.open, 1);
    assert!(body.contains("Still running: b"), "{body}");
    rig.until("it waits again", |r| {
        let row = r.row();
        row.status == SessionStatus::Idle && row.waiting.as_ref().is_some_and(|w| w.on.len() == 1)
    })
    .await;
    rig.never_completed();
    // The person can write meanwhile: that turn's end is held too.
    rig.send("parent", HarnessId::Mock, "how is it going");
    rig.until("their message is answered", |r| {
        r.stage.prompts.lock().unwrap().iter().any(|p| p == "how is it going")
            && r.status_of("parent") == Some(SessionStatus::Idle)
    })
    .await;
    assert!(rig.row().waiting.is_some());
    rig.never_completed();
    rig.stage.open("b");
    rig.finished().await;
    assert_eq!(rig.wakes().len(), 2);
    rig.core.shutdown().await;
}

/// `completionWake: settledOnly` says it once, when every task its turn
/// started is in.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn settled_only_waits_for_its_whole_cohort() {
    let dir = tempfile::tempdir().unwrap();
    let rig = Rig::new(dir.path(), true);
    for child in ["a", "b"] {
        rig.child(child, HarnessId::Opencode, &rig.core.device_id);
    }
    rig.parent_working().await;
    for child in ["a", "b"] {
        rig.delegate(child, HarnessId::Opencode, &format!("hold {child}"), "settledOnly").await;
    }
    rig.parent_parked().await;
    rig.stage.open("a");
    rig.until("a is in", |r| {
        r.row().waiting.as_ref().is_some_and(|w| w.on.len() == 1)
    })
    .await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(rig.wakes().is_empty());
    rig.stage.open("b");
    rig.finished().await;
    assert_eq!(rig.wakes().len(), 1);
    assert_eq!(rig.wakes()[0].0.tasks.len(), 2);
    rig.core.shutdown().await;
}

/// A Stop on a waiting parent stops what it delegated, and nothing of it
/// comes back: the parent reads stopped, never finished.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stopping_the_parent_stops_its_children_and_disposes_their_results() {
    let dir = tempfile::tempdir().unwrap();
    let rig = Rig::new(dir.path(), true);
    rig.child("a", HarnessId::Codex, &rig.core.device_id);
    rig.child("far", HarnessId::Codex, "other-device");
    rig.parent_working().await;
    rig.delegate("a", HarnessId::Codex, "hold a", "always").await;
    rig.delegate("far", HarnessId::Codex, "hold far", "always").await;
    rig.until("a works", |r| r.status_of("a") == Some(SessionStatus::Working)).await;
    rig.parent_parked().await;
    rig.core
        .doc_host
        .queue_command("parent", SessionCommandPayload::Interrupt {})
        .unwrap();
    rig.until("a is stopped", |r| r.status_of("a") == Some(SessionStatus::Idle)).await;
    let row = rig.row();
    assert!(row.waiting.is_none());
    assert_eq!(row.last_completed_turn, None);
    assert_eq!(row.last_turn_outcome.map(|o| o.status), Some(DoneStatus::Interrupted));
    // The other device's is asked to stop through its chat, like any Stop.
    let commands = rig.core.doc_host.open("far").unwrap().doc().read_commands().unwrap();
    assert!(commands.iter().any(|c| matches!(c.payload, SessionCommandPayload::Interrupt {})));
    // Late results: both "finish" now, and nobody is woken.
    rig.core.workspace.record_session(&remote_row("far", Some("turn-1"), SessionStatus::Idle));
    rig.stage.open("a");
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert!(rig.wakes().is_empty());
    for task in rig.tasks().await {
        assert_eq!(task.task.status, DelegatedStatus::Cancelled);
        assert_eq!(task.task.delivery_state, DeliveryState::Disposed);
        assert_eq!(task.cohort, zeron_proto::CohortDisposition::Stopped);
    }
    // The person's next message works as ever.
    rig.send("parent", HarnessId::Mock, "go on");
    rig.finished().await;
    rig.core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn archiving_the_parent_stops_its_children() {
    let dir = tempfile::tempdir().unwrap();
    let rig = Rig::new(dir.path(), true);
    rig.child("a", HarnessId::Codex, &rig.core.device_id);
    rig.parent_working().await;
    rig.delegate("a", HarnessId::Codex, "hold a", "always").await;
    rig.until("a works", |r| r.status_of("a") == Some(SessionStatus::Working)).await;
    rig.parent_parked().await;
    rig.core.workspace.set_chat_archived("parent", true).unwrap();
    rig.until("a is stopped", |r| r.status_of("a") == Some(SessionStatus::Idle)).await;
    rig.until("nothing is waited on", |r| r.row().waiting.is_none()).await;
    assert!(rig.wakes().is_empty());
    rig.core.shutdown().await;
}

fn remote_row(chat: &str, completed: Option<&str>, status: SessionStatus) -> Session {
    let now = chrono::Utc::now();
    Session {
        last_turn_outcome: completed.map(|_| NativeTurnOutcome {
            status: DoneStatus::Completed,
            finished_at: now,
        }),
        terminal_activity: None,
        last_completed_turn: completed.map(str::to_owned),
        chat_id: chat.into(),
        device_id: "other-device".into(),
        status,
        started_at: None,
        updated_at: now,
        running_subagents: 0,
        background_tasks: Vec::new(),
        waiting: None,
        blocked_on_wait: false,
    }
}

/// A child on another device, across an engine restart mid-wait: the wait is
/// back on boot, a device gone quiet is never a failure, and its result is
/// delivered when it comes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_wait_on_another_device_survives_a_restart_and_a_quiet_spell() {
    let dir = tempfile::tempdir().unwrap();
    let rig = Rig::new(dir.path(), false);
    rig.child("far", HarnessId::Codex, "other-device");
    rig.parent_working().await;
    rig.delegate("far", HarnessId::Codex, "hold far", "always").await;
    rig.core.workspace.record_session(&remote_row("far", None, SessionStatus::Working));
    rig.parent_parked().await;
    rig.until("it runs over there", |r| {
        r.core.sessions.delegations().of_parent("parent")[0].status == DelegatedStatus::Running
    })
    .await;
    let stage = rig.stage.clone();
    rig.core.shutdown().await;
    drop(rig);

    let rig = Rig::watch(assemble(dir.path(), &stage, false), stage);
    rig.until("it waits again after the restart", |r| {
        r.core
            .sessions
            .session_status("parent")
            .and_then(|s| s.waiting)
            .is_some_and(|w| w.on.len() == 1 && w.on[0].child_chat_id.as_deref() == Some("far"))
    })
    .await;
    // Not heard from for hours.
    let mut quiet = remote_row("far", None, SessionStatus::Working);
    quiet.updated_at -= chrono::Duration::hours(6);
    rig.core.workspace.record_session(&quiet);
    tokio::time::sleep(Duration::from_millis(400)).await;
    let task = rig.tasks().await.remove(0);
    assert_eq!(task.task.status, DelegatedStatus::Running);
    assert!(task.unreachable, "said to be out of reach, not failed");
    assert!(rig.wakes().is_empty());

    rig.core.workspace.record_session(&remote_row("far", Some("turn-1"), SessionStatus::Idle));
    rig.finished().await;
    let (notice, body) = rig.wakes().remove(0);
    assert_eq!(notice.tasks[0].device_id, "other-device");
    assert_eq!(notice.tasks[0].status, DelegatedStatus::Completed);
    // (Its transcript never reached this device: said so, not made up.)
    assert!(body.contains("read_chat"), "{body}");
    rig.core.shutdown().await;
}

/// A wake recorded as being sent when the engine died, and never in the
/// chat, is sent again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_wake_lost_to_a_restart_is_sent_again() {
    let dir = tempfile::tempdir().unwrap();
    let rig = Rig::new(dir.path(), false);
    rig.child("far", HarnessId::Codex, "other-device");
    rig.parent_working().await;
    rig.delegate("far", HarnessId::Codex, "hold far", "always").await;
    rig.parent_parked().await;
    let stage = rig.stage.clone();
    rig.core.shutdown().await;
    drop(rig);
    let store = find(dir.path(), "delegated-tasks.json");
    let mut state: Value = serde_json::from_slice(&std::fs::read(&store).unwrap()).unwrap();
    state["tasks"][0]["status"] = json!("completed");
    state["tasks"][0]["result"] = json!("the parser is fixed");
    state["tasks"][0]["deliveryState"] = json!("delivered");
    state["tasks"][0]["deliveryMessageId"] = json!("delegated-lost");
    std::fs::write(&store, serde_json::to_vec(&state).unwrap()).unwrap();

    let rig = Rig::watch(assemble(dir.path(), &stage, false), stage);
    rig.finished().await;
    assert_eq!(rig.wakes().len(), 1);
    assert!(rig.wakes()[0].1.contains("the parser is fixed"));
    rig.core.shutdown().await;
}

fn find(dir: &Path, name: &str) -> std::path::PathBuf {
    let mut left = vec![dir.to_path_buf()];
    while let Some(at) = left.pop() {
        for entry in std::fs::read_dir(&at).unwrap().flatten() {
            if entry.path().is_dir() {
                left.push(entry.path());
            } else if entry.file_name() == name {
                return entry.path();
            }
        }
    }
    panic!("no {name}");
}

async fn answer(rig: &Rig, child: &str, label: &str) {
    let request_id = rig
        .entries(child)
        .iter()
        .flat_map(|e| e.parts.clone())
        .find_map(|p| match p {
            MessagePart::Input { request_id, resolved: false, .. } => Some(request_id),
            _ => None,
        })
        .unwrap();
    rig.core
        .doc_host
        .queue_command(
            child,
            SessionCommandPayload::RespondInput {
                request_id,
                answers: vec![UserInputAnswer {
                    question_id: "q1".into(),
                    labels: vec![label.into()],
                }],
            },
        )
        .unwrap();
}

/// A child's question reaches its waiting parent; answering it lets the
/// child go on, and its result comes as any other's.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_childs_question_wakes_the_parent() {
    let dir = tempfile::tempdir().unwrap();
    let rig = Rig::new(dir.path(), true);
    rig.child("a", HarnessId::Opencode, &rig.core.device_id);
    rig.parent_working().await;
    rig.delegate("a", HarnessId::Opencode, "ask", "always").await;
    rig.parent_parked().await;
    rig.until("the parent is asked", |r| r.wakes().len() == 1).await;
    let (notice, body) = rig.wakes().remove(0);
    assert_eq!(notice.kind, WakeKind::Question);
    assert_eq!(notice.tasks[0].chat_id, "a");
    assert!(body.contains("Which branch?") && body.contains("main | next") && body.contains("request="), "{body}");
    rig.until("it waits on", |r| {
        let row = r.row();
        row.status == SessionStatus::Idle && row.waiting.is_some()
    })
    .await;
    // Once is enough.
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(rig.wakes().len(), 1);
    answer(&rig, "a", "next").await;
    rig.finished().await;
    assert_eq!(rig.wakes().len(), 2);
    assert!(rig.wakes()[1].1.contains("answered next"));
    rig.core.shutdown().await;
}

/// A question the person answers before the parent is asked never reaches
/// it; an approval never does at all.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_question_answered_first_and_every_approval_stay_the_persons() {
    for (prompt, label, grace) in [
        ("ask", "main", Duration::from_secs(600)),
        ("approve", "Yes", Duration::from_millis(50)),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let rig = Rig::new(dir.path(), true);
        rig.core.sessions.delegations().set_timing(Timing { question_grace: grace, ..fast() });
        rig.child("a", HarnessId::Opencode, &rig.core.device_id);
        rig.parent_working().await;
        rig.delegate("a", HarnessId::Opencode, prompt, "always").await;
        rig.parent_parked().await;
        rig.until("the child asks", |r| {
            r.core.sessions.delegations().of_parent("parent")[0].status == DelegatedStatus::AwaitingInput
        })
        .await;
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert!(rig.wakes().is_empty(), "{prompt}");
        answer(&rig, "a", label).await;
        rig.finished().await;
        let wakes = rig.wakes();
        assert_eq!(wakes.len(), 1, "{prompt}: the result alone");
        assert_eq!(wakes[0].0.kind, WakeKind::Results);
        assert!(rig.tasks().await[0].task.input.is_none());
        rig.core.shutdown().await;
    }
}

/// What a tool returned isn't sent again, and a cancelled task stops its
/// child and wakes nobody: either way, with nothing left the end stands.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn acknowledged_and_cancelled_tasks_release_the_parent_without_a_wake() {
    let dir = tempfile::tempdir().unwrap();
    let rig = Rig::new(dir.path(), false);
    rig.child("a", HarnessId::Codex, &rig.core.device_id);
    rig.child("b", HarnessId::Codex, &rig.core.device_id);
    rig.parent_working().await;
    let a = rig.delegate("a", HarnessId::Codex, "hold a", "always").await;
    let b = rig.delegate("b", HarnessId::Codex, "hold b", "always").await;
    rig.until("b works", |r| r.status_of("b") == Some(SessionStatus::Working)).await;
    rig.parent_parked().await;
    let update = |task: &str, action: &str| {
        json!({ "chatId": "parent", "taskId": task, "action": action, "status": "completed" })
    };
    rig.rpc(methods::UPDATE_DELEGATED_TASK, update(&a, "acknowledge")).await.unwrap();
    rig.never_completed();
    let cancelled = rig.rpc(methods::UPDATE_DELEGATED_TASK, update(&b, "cancel")).await.unwrap();
    assert_eq!(cancelled["ok"], true);
    rig.until("b is stopped", |r| r.status_of("b") == Some(SessionStatus::Idle)).await;
    rig.finished().await;
    assert!(rig.wakes().is_empty());
    assert_eq!(rig.row().last_turn_outcome.map(|o| o.status), Some(DoneStatus::Completed));
    // Nobody else's tasks, and no chat that isn't its child.
    let other = rig
        .rpc(methods::UPDATE_DELEGATED_TASK, json!({ "chatId": "a", "taskId": b, "action": "cancel" }))
        .await
        .unwrap();
    assert_eq!(other["ok"], false);
    rig.rpc(
        methods::REGISTER_DELEGATED_TASK,
        json!({ "chatId": "a", "childChatId": "b", "messageId": "m" }),
    )
    .await
    .unwrap_err();
    rig.core.shutdown().await;
}

/// A follow-up to a child is a task of its own: the first turn's end isn't
/// its result, and one wake answers the last.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_follow_up_is_a_task_of_its_own() {
    let dir = tempfile::tempdir().unwrap();
    let rig = Rig::new(dir.path(), true);
    rig.child("a", HarnessId::Codex, &rig.core.device_id);
    rig.parent_working().await;
    rig.delegate("a", HarnessId::Codex, "first", "always").await;
    rig.parent_parked().await;
    rig.finished().await;
    let first_end = rig.row().last_completed_turn;
    rig.delegate("a", HarnessId::Codex, "hold second", "always").await;
    rig.until("it waits on the follow-up", |r| r.row().waiting.is_some()).await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(rig.wakes().len(), 1);
    rig.stage.open("second");
    rig.until("the follow-up's result finishes it", |r| {
        let row = r.row();
        row.waiting.is_none() && row.last_completed_turn != first_end
    })
    .await;
    assert_eq!(rig.wakes().len(), 2);
    assert!(rig.wakes()[1].1.contains("result second"));
    rig.core.shutdown().await;
}
