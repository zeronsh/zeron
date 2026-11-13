//! A Cloud machine updates before it answers (`wake_update`), and the message always
//! gets answered. Real engines over real chats: only the release's arrival, its
//! install and the restart are scripted. What those steps do themselves is tested in
//! `crates/update`, with real downloads and real processes for a bad new version.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;
use tokio::sync::Notify;

use zeron_doc::{
    MessagePart, MessageRole, MessageStatus, SessionCommandPayload, SessionDoc, SessionMessageEntry,
};
use zeron_engine::wake_update::{self, Found, Limits, Steps};
use zeron_engine::{EngineCore, HarnessRegistry, RunJournal};
use zeron_harness::{Harness, HarnessError, RunControls};
use zeron_proto::{
    AgentEvent, DoneStatus, HarnessId, Model, ReasoningLevel, RunRequest, SandboxLevel,
    SteeringMode,
};
use zeron_sync::DocsStore;

const CHAT: &str = "chat-wake-update";
const DEVICE: &str = "dev-cloud";

/// `ARMED` is the process's: only one engine here boots at a time.
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Answers at once, except a prompt with "hold" in it, until `release`.
struct Agent {
    release: Arc<Notify>,
    runs: Arc<Mutex<Vec<RunRequest>>>,
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
        self.runs.lock().unwrap().push(request.clone());
        let hold = request.prompt.contains("hold");
        let release = self.release.clone();
        let reply = format!("answered: {}", request.prompt);
        let cwd = request.cwd.clone();
        let (tx, rx) = futures::channel::mpsc::unbounded();
        tokio::spawn(async move {
            let _ = tx.unbounded_send(Ok(AgentEvent::SessionStarted {
                harness: HarnessId::Mock,
                model: "mock-1".into(),
                tools: vec![],
                cwd,
                session_id: "hs".into(),
                assistant_message_id: "a-1".into(),
            }));
            if hold {
                release.notified().await;
            }
            let _ = tx.unbounded_send(Ok(AgentEvent::TextDelta { text: reply }));
            let _ = tx.unbounded_send(Ok(AgentEvent::Done {
                status: DoneStatus::Completed,
                result: None,
                error: None,
                session_id: Some("hs".into()),
            }));
        });
        Ok(rx.boxed())
    }
}

struct Rig {
    _tmp: tempfile::TempDir,
    dir: std::path::PathBuf,
    runs: Arc<Mutex<Vec<RunRequest>>>,
    release: Arc<Notify>,
}

impl Rig {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("data");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("device-id"), DEVICE).unwrap();
        Self {
            _tmp: tmp,
            dir,
            runs: Arc::default(),
            release: Arc::new(Notify::new()),
        }
    }

    fn store_dir(&self) -> std::path::PathBuf {
        self.dir.join("orgs/dev-org/dev-user")
    }

    /// One start of the engine on this machine's disk.
    fn boot(&self) -> EngineCore {
        let registry = HarnessRegistry::new();
        registry.register(Arc::new(Agent {
            release: self.release.clone(),
            runs: self.runs.clone(),
        }));
        let core = EngineCore::assemble(&self.dir, Arc::new(registry), HarnessId::Mock, None)
            .expect("engine core assembles");
        // No port here: a turn revived on boot goes on at once rather than
        // waiting out `ipc_port_ready`'s limit for one. (A real machine's
        // boot records its port; no test here reads the tools a port adds.)
        core.sessions.set_no_ipc_port();
        core
    }

    /// A machine's boot: work held from assembly, as `Engine::run` arms it.
    async fn boot_held(&self) -> EngineCore {
        wake_update::force_armed(true);
        let core = self.boot();
        wake_update::force_armed(false);
        create_chat(&core).await;
        core
    }

    fn answers(&self) -> usize {
        self.runs.lock().unwrap().len()
    }
}

async fn create_chat(core: &EngineCore) {
    let client = zeron_rpc::memory_client(core.rpc_service());
    let _ = client
        .call(
            zeron_rpc::methods::MUTATE,
            serde_json::json!({ "op": "createChat", "chatId": CHAT, "deviceId": core.device_id }),
        )
        .await;
    let _ = core.workspace.rename_chat(CHAT, "Pre-titled");
}

fn send(core: &EngineCore, id: &str, prompt: &str) {
    core.doc_host
        .queue_command(
            CHAT,
            SessionCommandPayload::Run {
                request: RunRequest {
                    mcp: None,
                    prompt: prompt.into(),
                    harness: None,
                    model: None,
                    reasoning: None,
                    model_options: Default::default(),
                    cwd: "~".into(),
                    sandbox: SandboxLevel::WorkspaceWrite,
                    auto_approve: true,
                    access: Default::default(),
                    plan: false,
                    attachments: Vec::new(),
                    worktree: None,
                    resume: None,
                    env: Default::default(),
                    extra_mcp: Vec::new(),
                },
                message_id: id.into(),
            },
        )
        .expect("queue run command");
}

fn answered(core: &EngineCore, prompt: &str) -> bool {
    let text = format!("answered: {prompt}");
    core.doc_host
        .open(CHAT)
        .ok()
        .and_then(|h| h.doc().read_entries().ok())
        .unwrap_or_default()
        .iter()
        .any(|e| {
            e.role == MessageRole::Assistant
                && e.status == Some(MessageStatus::Complete)
                && e.parts
                    .iter()
                    .any(|p| matches!(p, MessagePart::Text { text: t, .. } if t == &text))
        })
}

async fn wait_for(mut what: impl FnMut() -> bool, label: &str) {
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    while !what() {
        assert!(std::time::Instant::now() < deadline, "timed out: {label}");
        tokio::time::sleep(Duration::from_millis(15)).await;
    }
}

/// Long enough for a run that was going to start to have started.
async fn settle() {
    tokio::time::sleep(Duration::from_millis(400)).await;
}

const FAST: Limits = Limits {
    check: Duration::from_millis(100),
    install: Duration::from_millis(100),
    restart: Duration::from_millis(100),
    trial: Duration::from_secs(1),
};

#[derive(Default)]
struct Script {
    found: Mutex<Option<anyhow::Result<Found>>>,
    check_hangs: bool,
    busy: AtomicBool,
    install_error: Option<&'static str>,
    installs: AtomicUsize,
    restarts: AtomicUsize,
    restart_asked: Notify,
    failed: Mutex<Vec<String>>,
}

impl Script {
    fn finding(found: Found) -> Self {
        Self {
            found: Mutex::new(Some(Ok(found))),
            ..Self::default()
        }
    }
}

#[async_trait]
impl Steps for Script {
    async fn check(&self) -> anyhow::Result<Found> {
        if self.check_hangs {
            std::future::pending::<()>().await;
        }
        self.found.lock().unwrap().take().expect("checked once")
    }
    fn busy(&self) -> bool {
        self.busy.load(Ordering::SeqCst)
    }
    async fn install(&self, _limit: Duration) -> anyhow::Result<()> {
        self.installs.fetch_add(1, Ordering::SeqCst);
        match self.install_error {
            Some(error) => anyhow::bail!("{error}"),
            None => Ok(()),
        }
    }
    fn install_failed(&self, version: &str) {
        self.failed.lock().unwrap().push(version.to_owned());
    }
    async fn restart(&self, _trial_limit: Duration) -> anyhow::Result<()> {
        self.restarts.fetch_add(1, Ordering::SeqCst);
        self.restart_asked.notify_one();
        Ok(())
    }
    fn restart_never_came(&self) {}
}

#[tokio::test(flavor = "multi_thread")]
async fn no_update_answers_straight_away_and_held_until_the_check_says_so() {
    let _serial = SERIAL.lock().await;
    let rig = Rig::new();
    let core = rig.boot_held().await;
    send(&core, "m1", "hello");
    settle().await;
    assert_eq!(rig.answers(), 0, "a run began before the check");

    let started = std::time::Instant::now();
    let steps = Script::finding(Found::Current);
    assert_eq!(
        wake_update::gate_then_release(&core, &steps, wake_update::LIMITS).await,
        "current"
    );
    wait_for(|| answered(&core, "hello"), "the waiting message").await;
    assert_eq!(steps.installs.load(Ordering::SeqCst), 0);
    assert_eq!(rig.answers(), 1);
    eprintln!(
        "released to answered, with the check instant: {:?}",
        started.elapsed()
    );
    core.shutdown().await;
}

/// The whole road: the old engine installs and restarts without starting the
/// message; the next start of the engine, on the same disk, answers it, once.
#[tokio::test(flavor = "multi_thread")]
async fn an_update_answers_from_the_new_start_and_only_once() {
    let _serial = SERIAL.lock().await;
    let rig = Rig::new();
    let old = rig.boot_held().await;
    send(&old, "m1", "hello");

    let steps = Arc::new(Script::finding(Found::Newer("9.9.9".into())));
    let limits = Limits {
        restart: Duration::from_secs(600),
        ..FAST
    };
    let gate = {
        let steps = steps.clone();
        tokio::spawn(async move { wake_update::gate(steps.as_ref(), limits).await })
    };
    tokio::time::timeout(Duration::from_secs(5), steps.restart_asked.notified())
        .await
        .expect("it asks for the restart");
    assert_eq!(steps.installs.load(Ordering::SeqCst), 1);
    settle().await;
    assert_eq!(
        rig.answers(),
        0,
        "the message must wait for the new version"
    );
    assert!(
        !gate.is_finished(),
        "work stays held until the process stops"
    );

    // The restart: this process stops, gracefully, as SIGTERM has it do.
    gate.abort();
    old.shutdown().await;
    drop(old);

    let new = rig.boot_held().await;
    let _ = new.doc_host.open(CHAT);
    settle().await;
    assert_eq!(rig.answers(), 0, "held again, as a trial holds it");
    // Its trial commits (`update_trial`), which is what lifts this.
    let after = Script::finding(Found::Current);
    wake_update::gate_then_release(&new, &after, wake_update::LIMITS).await;
    wait_for(
        || answered(&new, "hello"),
        "the waiting message, on the new start",
    )
    .await;
    settle().await;
    assert_eq!(rig.answers(), 1, "answered exactly once");
    new.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_check_that_hangs_answers_at_its_limit() {
    let _serial = SERIAL.lock().await;
    let rig = Rig::new();
    let core = rig.boot_held().await;
    send(&core, "m1", "hello");
    let steps = Script {
        check_hangs: true,
        ..Script::default()
    };
    let started = std::time::Instant::now();
    assert_eq!(
        wake_update::gate_then_release(&core, &steps, FAST).await,
        "checkTimedOut"
    );
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );
    wait_for(|| answered(&core, "hello"), "the message, on this version").await;
    core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_download_answers_on_this_version_and_marks_it_for_later() {
    let _serial = SERIAL.lock().await;
    let rig = Rig::new();
    let core = rig.boot_held().await;
    send(&core, "m1", "hello");
    let steps = Script {
        install_error: Some("HTTP 500"),
        ..Script::finding(Found::Newer("9.9.9".into()))
    };
    assert_eq!(
        wake_update::gate_then_release(&core, &steps, FAST).await,
        "installFailed"
    );
    assert_eq!(steps.restarts.load(Ordering::SeqCst), 0);
    assert_eq!(*steps.failed.lock().unwrap(), ["9.9.9"]);
    wait_for(|| answered(&core, "hello"), "the message, on this version").await;
    core.shutdown().await;
}

/// After a bad new version's rollback the old one boots here again. It finds the
/// rollback (`zeron_update::boot_skip_reason`, with real processes in
/// `update_transaction.rs`) and answers.
#[tokio::test(flavor = "multi_thread")]
async fn a_bad_new_versions_rollback_ends_with_the_message_answered() {
    let _serial = SERIAL.lock().await;
    let rig = Rig::new();
    let core = rig.boot_held().await;
    send(&core, "m1", "hello");
    let steps = Script::finding(Found::Skip("wentBack"));
    assert_eq!(
        wake_update::gate_then_release(&core, &steps, wake_update::LIMITS).await,
        "wentBack"
    );
    assert_eq!(steps.installs.load(Ordering::SeqCst), 0);
    assert_eq!(steps.restarts.load(Ordering::SeqCst), 0);
    wait_for(
        || answered(&core, "hello"),
        "the message, on the old version",
    )
    .await;
    core.shutdown().await;
}

/// An update arriving mid-turn: the wake's attempt steps aside, and the background
/// path's check says no until the turn is over and nothing is waiting.
#[tokio::test(flavor = "multi_thread")]
async fn an_update_arriving_mid_turn_waits_for_it() {
    let _serial = SERIAL.lock().await;
    let rig = Rig::new();
    let core = rig.boot();
    create_chat(&core).await;
    let quiet_on_cloud = core.update_quiescence(true);
    let quiet_elsewhere = core.update_quiescence(false);
    assert!(quiet_on_cloud(), "nothing going on");

    send(&core, "m1", "hold on");
    wait_for(|| rig.answers() == 1, "the turn to start").await;
    assert!(!quiet_on_cloud(), "a restart now would cut the turn off");
    assert!(!quiet_elsewhere());

    let steps = Script::finding(Found::Newer("9.9.9".into()));
    steps
        .busy
        .store(core.sessions.any_active(), Ordering::SeqCst);
    assert_eq!(wake_update::gate(&steps, FAST).await, "busy");
    assert_eq!(steps.installs.load(Ordering::SeqCst), 0);
    assert_eq!(steps.restarts.load(Ordering::SeqCst), 0);
    assert!(
        steps.failed.lock().unwrap().is_empty(),
        "nothing against the release"
    );

    rig.release.notify_one();
    wait_for(|| answered(&core, "hold on"), "the turn").await;
    wait_for(&*quiet_on_cloud, "the machine to be quiet").await;
    core.shutdown().await;
}

/// Only a Cloud machine counts a message waiting: nothing else changes elsewhere.
#[tokio::test(flavor = "multi_thread")]
async fn a_message_waiting_holds_a_restart_on_a_cloud_machine_only() {
    let _serial = SERIAL.lock().await;
    let rig = Rig::new();
    let core = rig.boot_held().await;
    let quiet_on_cloud = core.update_quiescence(true);
    let quiet_elsewhere = core.update_quiescence(false);
    assert!(quiet_on_cloud());
    send(&core, "m1", "hello");
    wait_for(|| !quiet_on_cloud(), "the message to count").await;
    assert!(quiet_elsewhere(), "the same as ever off Cloud");
    let steps = Script::finding(Found::Current);
    wake_update::gate_then_release(&core, &steps, wake_update::LIMITS).await;
    wait_for(|| answered(&core, "hello"), "the message").await;
    wait_for(&*quiet_on_cloud, "the machine to be quiet").await;
    core.shutdown().await;
}

/// A turn a dying machine left: an update would cut off its revival, so it waits
/// with everything else, and resumes once work may start.
#[tokio::test(flavor = "multi_thread")]
async fn a_killed_turn_waits_for_the_check_too() {
    let _serial = SERIAL.lock().await;
    let rig = Rig::new();
    let now = chrono::Utc::now().timestamp_millis();
    let doc = SessionDoc::init(CHAT).unwrap();
    for (id, role, status, created_at) in [
        (
            "m1",
            MessageRole::User,
            MessageStatus::Complete,
            now - 60_000,
        ),
        (
            "a1",
            MessageRole::Assistant,
            MessageStatus::Streaming,
            now - 30_000,
        ),
    ] {
        doc.push_message(&SessionMessageEntry {
            id: id.into(),
            role,
            parts: vec![MessagePart::Text {
                id: "t0".into(),
                text: "long task".into(),
            }],
            created_at,
            device_id: DEVICE.into(),
            status: Some(status),
            continuation_of: None,
            duration_ms: None,
        })
        .unwrap();
    }
    let store = DocsStore::open(rig.store_dir()).unwrap();
    store
        .save_snapshot(CHAT, &doc.export_snapshot().unwrap())
        .unwrap();
    drop(store);
    RunJournal::open(rig.store_dir().join("journals"))
        .unwrap()
        .append(
            CHAT,
            &AgentEvent::SessionStarted {
                harness: HarnessId::Mock,
                model: "mock-1".into(),
                tools: vec![],
                cwd: "/tmp".into(),
                session_id: "hs-killed".into(),
                assistant_message_id: "a-killed".into(),
            },
        )
        .unwrap();

    wake_update::force_armed(true);
    let core = rig.boot();
    wake_update::force_armed(false);
    settle().await;
    assert_eq!(rig.answers(), 0, "revived ahead of the check");
    let steps = Script::finding(Found::Current);
    wake_update::gate_then_release(&core, &steps, wake_update::LIMITS).await;
    wait_for(|| rig.answers() == 1, "the killed turn to resume").await;
    assert_eq!(
        rig.runs.lock().unwrap()[0].resume.as_deref(),
        Some("hs-killed")
    );
    core.shutdown().await;
}
