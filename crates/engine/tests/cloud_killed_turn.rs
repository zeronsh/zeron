//! A turn cut off by its machine dying (design doc, "A killed turn"). The engine
//! marks a command handled before running it, so the command plane never runs it
//! twice; what happens next has to be a resume or a clear failure, never a chat
//! that just stops at the user's message.
//!
//! Each test manufactures the disk a killed machine comes back with, then boots
//! an engine over it, as the wake does.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;

use zeron_doc::{
    MessagePart, MessageRole, MessageStatus, SessionCommandEntry, SessionCommandPayload,
    SessionCommandStatus, SessionDoc, SessionMessageEntry,
};
use zeron_engine::{EngineCore, HarnessRegistry, RunJournal};
use zeron_harness::{Harness, HarnessError, RunControls};
use zeron_proto::{
    AgentEvent, DoneStatus, HarnessId, Model, ReasoningLevel, RunRequest, SandboxLevel,
    SteeringMode,
};
use zeron_sync::DocsStore;

const CHAT: &str = "chat-killed";
const DEVICE: &str = "dev-cloud";
const DAY_MS: i64 = 24 * 60 * 60 * 1000;

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
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
        access: Default::default(),
        plan: false,
        attachments: Vec::new(),
        worktree: None,
        resume: None,
        env: Default::default(),
        extra_mcp: Vec::new(),
    }
}

type RequestLog = Arc<Mutex<Vec<RunRequest>>>;

struct Recording {
    requests: RequestLog,
}

#[async_trait]
impl Harness for Recording {
    fn id(&self) -> HarnessId {
        HarnessId::Mock
    }
    fn display_name(&self) -> &str {
        "Recording"
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
        self.requests.lock().unwrap().push(request.clone());
        let events = vec![
            Ok(AgentEvent::SessionStarted {
                harness: HarnessId::Mock,
                model: "mock-1".into(),
                tools: vec![],
                cwd: request.cwd.clone(),
                session_id: "hs".into(),
                assistant_message_id: "a-1".into(),
            }),
            Ok(AgentEvent::TextDelta {
                text: format!("ack: {}", request.prompt),
            }),
            Ok(AgentEvent::Done {
                status: DoneStatus::Completed,
                result: None,
                error: None,
                session_id: Some("hs".into()),
            }),
        ];
        Ok(futures::stream::iter(events).boxed())
    }
}

/// A data dir whose chat holds `entries` (and `commands`), as a killed machine's
/// disk or the room's replica left it.
struct Disk {
    _tmp: tempfile::TempDir,
    dir: std::path::PathBuf,
    doc: SessionDoc,
}

impl Disk {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("data");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("device-id"), DEVICE).unwrap();
        Self {
            _tmp: tmp,
            dir,
            doc: SessionDoc::init(CHAT).unwrap(),
        }
    }

    fn store_dir(&self) -> std::path::PathBuf {
        self.dir.join("orgs/dev-org/dev-user")
    }

    fn user_message(self, id: &str, created_at: i64) -> Self {
        self.doc
            .push_message(&SessionMessageEntry {
                id: id.into(),
                role: MessageRole::User,
                parts: vec![MessagePart::Text {
                    id: "t0".into(),
                    text: "long task".into(),
                }],
                created_at,
                device_id: DEVICE.into(),
                status: Some(MessageStatus::Complete),
                continuation_of: None,
                duration_ms: None,
            })
            .unwrap();
        self
    }

    fn streaming_reply(self, id: &str, created_at: i64) -> Self {
        self.doc
            .push_message(&SessionMessageEntry {
                id: id.into(),
                role: MessageRole::Assistant,
                parts: vec![MessagePart::Text {
                    id: "t0".into(),
                    text: "partial".into(),
                }],
                created_at,
                device_id: DEVICE.into(),
                status: Some(MessageStatus::Streaming),
                continuation_of: None,
                duration_ms: None,
            })
            .unwrap();
        self
    }

    fn run_command(self, id: &str, message_id: &str, status: SessionCommandStatus) -> Self {
        self.doc
            .queue_command(&SessionCommandEntry {
                id: id.into(),
                payload: SessionCommandPayload::Run {
                    request: run_request("long task"),
                    message_id: message_id.into(),
                },
                issued_by: DEVICE.into(),
                issued_at: now_ms(),
                based_on: None,
                expires_at: None,
                status: SessionCommandStatus::Pending,
                resolution: None,
            })
            .unwrap();
        if status != SessionCommandStatus::Pending {
            self.doc.set_command_status(id, status, None).unwrap();
        }
        self
    }

    /// The journal of a turn the machine's death left open.
    fn open_journal(self) -> Self {
        let journal = RunJournal::open(self.store_dir().join("journals")).unwrap();
        journal
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
        self
    }

    fn revived_too_often(self) -> Self {
        let journal = RunJournal::open(self.store_dir().join("journals")).unwrap();
        for _ in 0..3 {
            journal.note_resume_attempt(CHAT, now_ms(), now_ms() - DAY_MS);
        }
        self
    }

    fn user_stopped(self) -> Self {
        RunJournal::open(self.store_dir().join("journals"))
            .unwrap()
            .note_user_stop(CHAT);
        self
    }

    fn processed(self, command: &str) -> Self {
        DocsStore::open(self.store_dir())
            .unwrap()
            .mark_processed(command)
            .unwrap();
        self
    }

    /// Boot over it, with a harness that records what it's asked to run.
    fn boot(&self) -> (EngineCore, RequestLog) {
        self.boot_with(|_| {})
    }

    /// The disk a bad snapshot or restore leaves: the chat's own pages intact,
    /// a page further on (the outbox's) torn, so every check of the file fails.
    fn boot_damaged(&self) -> (EngineCore, RequestLog) {
        let store = DocsStore::open(self.store_dir()).unwrap();
        store
            .save_snapshot(CHAT, &self.doc.export_snapshot().unwrap())
            .unwrap();
        for i in 0..400 {
            store
                .enqueue_chat_update("other-chat", &format!("batch-{i}"), &[7u8; 900])
                .unwrap();
        }
        assert!(store.flush_for_sleep());
        drop(store);
        let path = self.store_dir().join("docs.sqlite3");
        let pages = std::fs::metadata(&path).unwrap().len() / 4096;
        let mut bytes = std::fs::read(&path).unwrap();
        let at = ((pages - 5) * 4096) as usize;
        for (i, byte) in bytes[at..at + 4096].iter_mut().enumerate() {
            *byte = byte.wrapping_mul(31).wrapping_add(i as u8) ^ 0x5a;
        }
        std::fs::write(&path, bytes).unwrap();
        let requests: RequestLog = Arc::default();
        let registry = HarnessRegistry::new();
        registry.register(Arc::new(Recording {
            requests: requests.clone(),
        }));
        let core = EngineCore::assemble(&self.dir, Arc::new(registry), HarnessId::Mock, None)
            .expect("an engine over a damaged store still assembles");
        core.sessions.set_no_ipc_port();
        (core, requests)
    }

    fn boot_with(&self, set_up: impl FnOnce(&HarnessRegistry)) -> (EngineCore, RequestLog) {
        let store = DocsStore::open(self.store_dir()).unwrap();
        store
            .save_snapshot(CHAT, &self.doc.export_snapshot().unwrap())
            .unwrap();
        drop(store);
        let requests: RequestLog = Arc::default();
        let registry = HarnessRegistry::new();
        registry.register(Arc::new(Recording {
            requests: requests.clone(),
        }));
        set_up(&registry);
        let core = EngineCore::assemble(&self.dir, Arc::new(registry), HarnessId::Mock, None)
            .expect("engine core assembles");
        // No port here: a turn revived on boot goes on at once rather than
        // waiting out `ipc_port_ready`'s limit for one. (A real machine's
        // boot records its port; no test here reads the tools a port adds.)
        core.sessions.set_no_ipc_port();
        (core, requests)
    }
}

fn entries(core: &EngineCore) -> Vec<SessionMessageEntry> {
    core.doc_host
        .open(CHAT)
        .ok()
        .and_then(|handle| handle.doc().read_entries().ok())
        .unwrap_or_default()
}

fn error_notes(core: &EngineCore) -> Vec<String> {
    entries(core)
        .iter()
        .flat_map(|entry| entry.parts.clone())
        .filter_map(|part| match part {
            MessagePart::Error { message, .. } => Some(message),
            _ => None,
        })
        .collect()
}

async fn wait_for(mut what: impl FnMut() -> bool, label: &str) {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !what() {
        assert!(std::time::Instant::now() < deadline, "timed out: {label}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Long enough for anything that was going to happen after a boot to happen.
async fn settle() {
    tokio::time::sleep(Duration::from_millis(600)).await;
}

fn on_a_cloud_machine() {
    zeron_engine::runner::set_session(Some(zeron_engine::runner::CloudSession {
        account: "cloud-acct".into(),
        chat_id: CHAT.into(),
        repo: None,
        path: None,
        branch: None,
    }));
}

// ── what the journal saw ─────────────────────────────────────────────────────

/// What the credential broker does: an agent started without this has no login.
struct Logins;

#[async_trait]
impl zeron_engine::registry::RunEnvironment for Logins {
    async fn prepare(
        &self,
        _harness: HarnessId,
        _chat_id: Option<&str>,
        request: &mut RunRequest,
    ) -> Result<(), String> {
        request.env.insert("LOGGED_IN".into(), "yes".into());
        Ok(())
    }
}

/// Seen on a real machine: boot recovery beat the broker to it, and Codex came
/// up with no login and no way to find its earlier conversation.
#[tokio::test]
async fn a_revived_turn_waits_for_the_machines_logins() {
    let now = now_ms();
    let disk = Disk::new()
        .user_message("m1", now - 60_000)
        .streaming_reply("a1", now - 30_000)
        .open_journal();
    let (core, requests) = disk.boot_with(|registry| registry.expect_run_environment());
    settle().await;
    assert!(
        requests.lock().unwrap().is_empty(),
        "a run started ahead of the broker"
    );
    core.registry.set_run_environment(Arc::new(Logins));
    wait_for(
        || !requests.lock().unwrap().is_empty(),
        "the killed turn to run once the logins are there",
    )
    .await;
    assert_eq!(
        requests.lock().unwrap()[0].env.get("LOGGED_IN").map(String::as_str),
        Some("yes")
    );
    core.shutdown().await;
}

#[tokio::test]
async fn a_recent_kill_resumes_the_turn() {
    let now = now_ms();
    let disk = Disk::new()
        .user_message("m1", now - 60_000)
        .streaming_reply("a1", now - 30_000)
        .open_journal();
    let (core, requests) = disk.boot();
    wait_for(
        || !requests.lock().unwrap().is_empty(),
        "the killed turn to run again",
    )
    .await;
    assert_eq!(
        requests.lock().unwrap()[0].resume.as_deref(),
        Some("hs-killed")
    );
    assert!(
        error_notes(&core)
            .iter()
            .any(|note| note.contains("resuming"))
    );
    core.shutdown().await;
}

#[tokio::test]
async fn a_kill_before_the_reply_reached_disk_resumes_too() {
    let now = now_ms();
    let disk = Disk::new().user_message("m1", now - 5_000).open_journal();
    let (core, requests) = disk.boot();
    wait_for(
        || !requests.lock().unwrap().is_empty(),
        "the killed turn to run again",
    )
    .await;
    core.shutdown().await;
}

#[tokio::test]
async fn a_kill_found_days_later_with_no_reply_says_so_instead_of_stopping_silently() {
    // A machine that died and was only woken days later, whose reply never
    // reached its disk: too stale to resume, and nothing to stamp.
    let now = now_ms();
    let disk = Disk::new()
        .user_message("m1", now - 3 * DAY_MS)
        .open_journal();
    let (core, requests) = disk.boot();
    settle().await;
    assert!(requests.lock().unwrap().is_empty(), "too stale to resume");
    let all = entries(&core);
    let note = all.last().unwrap();
    assert_eq!(note.role, MessageRole::Assistant);
    assert_eq!(note.status, Some(MessageStatus::Aborted));
    assert_eq!(error_notes(&core), ["Run interrupted by engine restart"]);
    core.shutdown().await;
}

#[tokio::test]
async fn a_kill_past_its_revivals_with_no_reply_says_so() {
    let now = now_ms();
    let disk = Disk::new()
        .user_message("m1", now - 5_000)
        .open_journal()
        .revived_too_often();
    let (core, requests) = disk.boot();
    settle().await;
    assert!(requests.lock().unwrap().is_empty(), "out of revivals");
    assert!(
        error_notes(&core)
            .iter()
            .any(|note| note.contains("not resumed automatically")),
        "{:?}",
        error_notes(&core)
    );
    core.shutdown().await;
}

#[tokio::test]
async fn a_reboot_says_it_only_once() {
    let now = now_ms();
    let disk = Disk::new()
        .user_message("m1", now - 3 * DAY_MS)
        .open_journal();
    let (core, _) = disk.boot();
    settle().await;
    core.shutdown().await;
    drop(core);
    let registry = HarnessRegistry::new();
    let core = EngineCore::assemble(&disk.dir, Arc::new(registry), HarnessId::Mock, None).unwrap();
    core.sessions.set_no_ipc_port();
    settle().await;
    assert_eq!(error_notes(&core).len(), 1);
    core.shutdown().await;
}

#[tokio::test]
async fn a_turn_the_user_stopped_gets_no_note() {
    let now = now_ms();
    let disk = Disk::new()
        .user_message("m1", now - 5_000)
        .open_journal()
        .user_stopped();
    let (core, requests) = disk.boot();
    settle().await;
    assert!(requests.lock().unwrap().is_empty());
    assert_eq!(error_notes(&core), Vec::<String>::new());
    core.shutdown().await;
}

#[tokio::test]
async fn a_resume_that_cant_start_says_so() {
    // Nothing can run it: the harness is gone from this build.
    let now = now_ms();
    let disk = Disk::new().user_message("m1", now - 5_000).open_journal();
    let store = DocsStore::open(disk.store_dir()).unwrap();
    store
        .save_snapshot(CHAT, &disk.doc.export_snapshot().unwrap())
        .unwrap();
    drop(store);
    let core = EngineCore::assemble(
        &disk.dir,
        Arc::new(HarnessRegistry::new()),
        HarnessId::Mock,
        None,
    )
    .unwrap();
    core.sessions.set_no_ipc_port();
    wait_for(
        || {
            error_notes(&core)
                .iter()
                .any(|note| note.contains("couldn't be resumed"))
        },
        "the failed resume to be noted",
    )
    .await;
    core.shutdown().await;
}

// ── what the journal never saw: a disk older than the room ───────────────────

#[tokio::test]
async fn a_reply_streaming_with_no_journal_is_settled_on_a_cloud_machine() {
    on_a_cloud_machine();
    let now = now_ms();
    // The room holds what the snapshot this disk came back from doesn't.
    let disk = Disk::new()
        .user_message("m1", now - 60_000)
        .streaming_reply("a1", now - 30_000)
        .run_command("c1", "m1", SessionCommandStatus::Applied);
    let (core, requests) = disk.boot();
    core.doc_host.open(CHAT).unwrap();
    wait_for(
        || !error_notes(&core).is_empty(),
        "the lost turn to be noted",
    )
    .await;
    assert!(requests.lock().unwrap().is_empty());
    let reply = entries(&core).into_iter().find(|e| e.id == "a1").unwrap();
    assert_eq!(reply.status, Some(MessageStatus::Aborted));
    assert!(error_notes(&core)[0].contains("Send a message to continue"));
    core.shutdown().await;
}

#[tokio::test]
async fn a_delivered_message_this_disk_never_ran_is_noted() {
    on_a_cloud_machine();
    let now = now_ms();
    let disk = Disk::new().user_message("m1", now - 60_000).run_command(
        "c1",
        "m1",
        SessionCommandStatus::Applied,
    );
    let (core, _) = disk.boot();
    core.doc_host.open(CHAT).unwrap();
    wait_for(
        || !error_notes(&core).is_empty(),
        "the lost turn to be noted",
    )
    .await;
    assert_eq!(entries(&core).last().unwrap().role, MessageRole::Assistant);
    core.shutdown().await;
}

#[tokio::test]
async fn a_message_this_disk_did_run_is_left_alone() {
    on_a_cloud_machine();
    let now = now_ms();
    // It ran here and ended with no reply: nothing was lost with the disk.
    let disk = Disk::new()
        .user_message("m1", now - 60_000)
        .run_command("c1", "m1", SessionCommandStatus::Applied)
        .processed("c1");
    let (core, requests) = disk.boot();
    core.doc_host.open(CHAT).unwrap();
    settle().await;
    assert!(requests.lock().unwrap().is_empty());
    assert_eq!(error_notes(&core), Vec::<String>::new());
    core.shutdown().await;
}

#[tokio::test]
async fn a_message_still_pending_just_runs() {
    on_a_cloud_machine();
    let now = now_ms();
    // Rolled back past the moment it ran: this disk's ledger hasn't seen it, so
    // the drain runs it, once. The rollback is what makes this a retry.
    let disk = Disk::new().user_message("m1", now - 60_000).run_command(
        "c1",
        "m1",
        SessionCommandStatus::Pending,
    );
    let (core, requests) = disk.boot();
    core.doc_host.open(CHAT).unwrap();
    wait_for(|| !requests.lock().unwrap().is_empty(), "it to run").await;
    settle().await;
    assert_eq!(requests.lock().unwrap().len(), 1);
    assert_eq!(error_notes(&core), Vec::<String>::new());
    core.shutdown().await;
}

#[tokio::test]
async fn consumed_but_unresolved_is_rejected_where_the_sender_sees_it() {
    on_a_cloud_machine();
    let now = now_ms();
    // Killed between the ledger write and the outcome: never run twice, and
    // never left "sending" either.
    let disk = Disk::new()
        .user_message("m1", now - 60_000)
        .run_command("c1", "m1", SessionCommandStatus::Pending)
        .processed("c1");
    let (core, requests) = disk.boot();
    let handle = core.doc_host.open(CHAT).unwrap();
    wait_for(
        || {
            handle
                .doc()
                .read_commands()
                .unwrap()
                .iter()
                .any(|c| c.status == SessionCommandStatus::Rejected)
        },
        "the dead command to be rejected",
    )
    .await;
    assert!(requests.lock().unwrap().is_empty());
    core.shutdown().await;
}

// ── a damaged store ──────────────────────────────────────────────────────────

/// Seen on staging: a machine's store came back from a sleep with torn pages,
/// every read of the chat failed, and the message sent to it waited for good
/// while the machine read "ready". The store is rebuilt at open, and the
/// message runs, once.
#[tokio::test]
async fn a_damaged_store_is_rebuilt_and_the_waiting_message_runs_once() {
    on_a_cloud_machine();
    let now = now_ms();
    let disk = Disk::new()
        .user_message("m1", now - 60_000)
        .run_command("c1", "m1", SessionCommandStatus::Pending);
    let (core, requests) = disk.boot_damaged();
    let recovery = core
        .doc_host
        .store()
        .recovery()
        .cloned()
        .expect("the damage was found");
    assert!(recovery.kept_at.exists(), "the damaged file is kept");
    core.doc_host.open(CHAT).unwrap();
    wait_for(|| !requests.lock().unwrap().is_empty(), "the waiting message to run").await;
    settle().await;
    assert_eq!(requests.lock().unwrap().len(), 1, "ran exactly once");
    assert_eq!(requests.lock().unwrap()[0].prompt, "long task");
    core.shutdown().await;
}

/// A command the damaged store's ledger had already marked handled stays
/// handled: the rebuilt store keeps every ledger row it could read.
#[tokio::test]
async fn a_rebuilt_store_never_runs_a_handled_command_again() {
    on_a_cloud_machine();
    let now = now_ms();
    let disk = Disk::new()
        .user_message("m1", now - 60_000)
        .run_command("c1", "m1", SessionCommandStatus::Pending)
        .processed("c1");
    let (core, requests) = disk.boot_damaged();
    assert!(core.doc_host.store().recovery().is_some());
    core.doc_host.open(CHAT).unwrap();
    settle().await;
    settle().await;
    assert!(requests.lock().unwrap().is_empty(), "the ledger came across");
    core.shutdown().await;
}
