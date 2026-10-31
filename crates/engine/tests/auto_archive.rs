//! Auto-archive on new chat: a project keeps at most 20 active top-level
//! chats. The FIRST real message of a chat (send/run, never an opened
//! composer or a bare `createChat`) archives the project's least recently
//! active chats beyond that. Archive only, never delete; side chats,
//! projectless chats, archived chats and other projects are out of scope;
//! pinned and live chats are exempt. Projects group by
//! `zeron_proto::view::project_key` (clones/worktrees of one repository).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use futures::StreamExt;
use futures::stream::BoxStream;

use zeron_doc::{MessageRole, MessageStatus, SessionCommandPayload, SessionMessageEntry};
use zeron_engine::workspace_host::{AUTO_ARCHIVE_KEEP_ACTIVE, WorkspaceHost, WorkspaceHostConfig};
use zeron_engine::{EngineCore, HarnessRegistry};
use zeron_harness::{Harness, HarnessError, RunControls};
use zeron_proto::{
    AgentEvent, Chat, DoneStatus, HarnessId, Model, ReasoningLevel, RunRequest, SandboxLevel,
    Session, SessionStatus, SidebarPinChange, Space, SteeringMode,
};
use zeron_sync::DocsStore;

const DEVICE: &str = "test-device";

// ── fixtures ────────────────────────────────────────────────────────────────

fn open_host(dir: &tempfile::TempDir) -> WorkspaceHost {
    WorkspaceHost::open(
        Arc::new(DocsStore::open(dir.path()).unwrap()),
        WorkspaceHostConfig {
            device_id: DEVICE.into(),
            device_name: "Test device".into(),
            platform: "macos".into(),
            org_id: "test-org".into(),
            user_id: "test-user".into(),
            edge: None,
            local_only: false,
        },
    )
    .unwrap()
}

fn space(host: &WorkspaceHost, id: &str) {
    host.create_space(id, DEVICE, &format!("/work/{id}"), None, false)
        .unwrap();
}

fn minutes_ago(minutes: i64) -> i64 {
    (Utc::now() - chrono::Duration::minutes(minutes)).timestamp_millis()
}

/// A chat in `space_id` whose last activity was `age` minutes ago. Seeding
/// goes through the backdate seam, never `note_message`, so it never sweeps.
fn seed(host: &WorkspaceHost, space_id: &str, id: &str, age: i64) {
    host.create_chat(id, Some(space_id), None, None, None).unwrap();
    let at = minutes_ago(age);
    assert!(host.set_chat_activity(id, Some(at), Some(at)).unwrap());
}

/// `n` chats `{prefix}-00..` in `space_id`; `-00` is the oldest.
fn seed_many(host: &WorkspaceHost, space_id: &str, prefix: &str, n: usize, oldest_age: i64) {
    for i in 0..n {
        seed(host, space_id, &format!("{prefix}-{i:02}"), oldest_age - i as i64);
    }
}

fn names(prefix: &str, range: std::ops::Range<usize>) -> Vec<String> {
    range.map(|i| format!("{prefix}-{i:02}")).collect()
}

/// Start a new chat in `space_id`: createChat, then its first message.
fn start_chat(host: &WorkspaceHost, space_id: &str, id: &str) {
    host.create_chat(id, Some(space_id), None, None, None).unwrap();
    host.note_message(id, "first message");
}

fn ids(host: &WorkspaceHost, keep: impl Fn(&Chat) -> bool) -> Vec<String> {
    let mut ids: Vec<String> = host
        .read_chats()
        .unwrap()
        .into_iter()
        .filter(|chat| keep(chat))
        .map(|chat| chat.id)
        .collect();
    ids.sort();
    ids
}

fn archived(host: &WorkspaceHost) -> Vec<String> {
    ids(host, |chat| chat.archived)
}

/// Unarchived top-level chats in any of `spaces`.
fn active_in(host: &WorkspaceHost, spaces: &[&str]) -> Vec<String> {
    ids(host, |chat| {
        !chat.archived
            && chat.is_top_level()
            && chat
                .space_id
                .as_deref()
                .is_some_and(|space| spaces.contains(&space))
    })
}

fn sorted(mut ids: Vec<String>) -> Vec<String> {
    ids.sort();
    ids
}

fn session(chat_id: &str, status: SessionStatus, age_secs: i64) -> Session {
    Session {
        chat_id: chat_id.into(),
        device_id: DEVICE.into(),
        status,
        last_completed_turn: None,
        started_at: None,
        updated_at: Utc::now() - chrono::Duration::seconds(age_secs),
        running_subagents: 0,
        background_tasks: Vec::new(),
        last_turn_outcome: None,
        terminal_activity: None,
    }
}

// ── the 20 / 21 boundary and overflow ───────────────────────────────────────

#[test]
fn keep_limit_is_twenty() {
    assert_eq!(AUTO_ARCHIVE_KEEP_ACTIVE, 20);
}

#[tokio::test]
async fn new_chat_reaching_twenty_archives_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let host = open_host(&dir);
    space(&host, "proj");
    seed_many(&host, "proj", "c", 19, 100);

    start_chat(&host, "proj", "new");

    assert!(archived(&host).is_empty());
    assert_eq!(active_in(&host, &["proj"]).len(), 20);
    assert_eq!(host.read_chats().unwrap().len(), 20, "nothing deleted");
}

#[tokio::test]
async fn new_chat_reaching_twenty_one_archives_the_oldest() {
    let dir = tempfile::tempdir().unwrap();
    let host = open_host(&dir);
    space(&host, "proj");
    seed_many(&host, "proj", "c", 20, 100);

    start_chat(&host, "proj", "new");

    assert_eq!(archived(&host), vec!["c-00".to_string()]);
    let active = active_in(&host, &["proj"]);
    assert_eq!(active.len(), 20);
    assert!(active.contains(&"new".to_string()));
    // Archive only: the row (with its history pointer) is still there.
    assert_eq!(host.read_chats().unwrap().len(), 21);
    assert!(host.chat("c-00").unwrap().unwrap().archived);
}

#[tokio::test]
async fn overflowing_project_is_trimmed_back_to_twenty() {
    let dir = tempfile::tempdir().unwrap();
    let host = open_host(&dir);
    space(&host, "proj");
    // A project already over the limit (e.g. from before the feature).
    seed_many(&host, "proj", "c", 25, 100);

    start_chat(&host, "proj", "new");

    assert_eq!(archived(&host), names("c", 0..6));
    assert_eq!(active_in(&host, &["proj"]).len(), 20);
    assert_eq!(host.read_chats().unwrap().len(), 26);
}

// ── order ───────────────────────────────────────────────────────────────────

#[tokio::test]
async fn oldest_by_last_activity_with_created_at_fallback() {
    let dir = tempfile::tempdir().unwrap();
    let host = open_host(&dir);
    space(&host, "proj");
    // 20 chats active 100..119 minutes ago (c-00 = 119), all created long ago.
    for i in 0..20 {
        let id = format!("c-{i:02}");
        host.create_chat(&id, Some("proj"), None, None, None).unwrap();
        host.set_chat_activity(&id, Some(minutes_ago(119 - i)), Some(minutes_ago(5_000)))
            .unwrap();
    }
    // Oldest creation of all, but the most recent message: must survive.
    host.create_chat("old-but-busy", Some("proj"), None, None, None)
        .unwrap();
    host.set_chat_activity("old-but-busy", Some(minutes_ago(1)), Some(minutes_ago(9_000)))
        .unwrap();
    // Never sent: ranks by createdAt (500 min) — older than every c-*.
    host.create_chat("never-sent", Some("proj"), None, None, None)
        .unwrap();
    host.set_chat_activity("never-sent", None, Some(minutes_ago(500)))
        .unwrap();
    assert!(host.chat("never-sent").unwrap().unwrap().last_message_at.is_none());

    // 22 + new = 23 → archive the 3 least recently active.
    start_chat(&host, "proj", "new");

    assert_eq!(
        archived(&host),
        sorted(vec!["never-sent".into(), "c-00".into(), "c-01".into()])
    );
    assert!(!host.chat("old-but-busy").unwrap().unwrap().archived);
    assert_eq!(active_in(&host, &["proj"]).len(), 20);
}

// ── scope: archived / side / projectless / other projects ───────────────────

#[tokio::test]
async fn archived_side_projectless_and_other_projects_are_out_of_scope() {
    let dir = tempfile::tempdir().unwrap();
    let host = open_host(&dir);
    space(&host, "proj");
    space(&host, "other");
    seed_many(&host, "proj", "c", 20, 100);
    // Already archived, very old: not counted (else they'd push extra archives).
    seed_many(&host, "proj", "arch", 5, 9_000);
    for id in names("arch", 0..5) {
        host.set_chat_archived(&id, true).unwrap();
    }
    // Side chats in the project, older than everything: never counted/archived.
    for i in 0..5 {
        let id = format!("side-{i:02}");
        host.create_chat_with_parent(&id, Some("proj"), None, None, None, Some("c-10".into()))
            .unwrap();
        let at = minutes_ago(20_000 + i);
        host.set_chat_activity(&id, Some(at), Some(at)).unwrap();
    }
    // Another project over the limit: untouched by a sweep in "proj".
    seed_many(&host, "other", "o", 25, 30_000);
    // Projectless chats, very old.
    for i in 0..21 {
        let id = format!("np-{i:02}");
        host.create_chat(&id, None, Some(DEVICE), None, None).unwrap();
        let at = minutes_ago(40_000 + i);
        host.set_chat_activity(&id, Some(at), Some(at)).unwrap();
    }

    start_chat(&host, "proj", "new");

    let mut expected = names("arch", 0..5);
    expected.push("c-00".into());
    assert_eq!(archived(&host), sorted(expected.clone()));
    assert_eq!(active_in(&host, &["proj"]).len(), 20);
    assert_eq!(active_in(&host, &["other"]).len(), 25);
    assert!(ids(&host, |c| c.id.starts_with("side-") && c.archived).is_empty());
    assert!(ids(&host, |c| c.id.starts_with("np-") && c.archived).is_empty());

    // A projectless first message never sweeps (22 projectless chats).
    host.create_chat("np-new", None, Some(DEVICE), None, None).unwrap();
    host.note_message("np-new", "hello");
    // A side chat's first message never sweeps its parent's project (25 active).
    host.create_chat_with_parent(
        "side-new",
        Some("other"),
        None,
        None,
        None,
        Some("o-24".into()),
    )
    .unwrap();
    host.note_message("side-new", "hello");

    assert_eq!(archived(&host), sorted(expected));
    assert_eq!(active_in(&host, &["other"]).len(), 25);
}

#[tokio::test]
async fn repository_spaces_share_one_project_by_project_key() {
    let dir = tempfile::tempdir().unwrap();
    let host = open_host(&dir);
    // Two checkouts of one repository (another device's clone included) and
    // an unrelated plain folder.
    space(&host, "a1");
    assert!(
        host.set_space_git("a1", true, None, Some("github.com/o/a"), None)
            .unwrap()
    );
    host.import_space_row(&Space {
        id: "a2".into(),
        device_id: "other-device".into(),
        path: "/elsewhere/a".into(),
        name: None,
        git_detected: true,
        git_checked_at: None,
        checkout_id: None,
        repository_id: Some("github.com/o/a".into()),
        github_repo: None,
        created_at: Utc::now(),
    })
    .unwrap();
    space(&host, "b");
    seed_many(&host, "a1", "a1", 10, 100);
    seed_many(&host, "a2", "a2", 10, 1_000); // the repository's oldest
    seed_many(&host, "b", "b", 15, 50_000);

    // 10 + 10 + new = 21 across the repository.
    start_chat(&host, "a1", "new");

    assert_eq!(archived(&host), vec!["a2-00".to_string()]);
    assert_eq!(active_in(&host, &["a1", "a2"]).len(), 20);
    assert_eq!(active_in(&host, &["b"]).len(), 15);
}

// ── trigger: only the first real message ────────────────────────────────────

#[tokio::test]
async fn empty_composer_and_later_messages_never_sweep() {
    let dir = tempfile::tempdir().unwrap();
    let host = open_host(&dir);
    space(&host, "proj");
    seed_many(&host, "proj", "c", 20, 100);

    // Opened composers: createChat with no message.
    for id in ["draft-0", "draft-1", "draft-2"] {
        host.create_chat(id, Some("proj"), None, None, None).unwrap();
    }
    assert!(archived(&host).is_empty());
    assert_eq!(active_in(&host, &["proj"]).len(), 23);

    // More messages in an existing chat: no sweep either.
    host.note_message("c-05", "another message");
    host.note_message("c-00", "and another");
    assert!(archived(&host).is_empty());

    // The first real send of a draft is the new chat: trims to 20. c-00 was
    // just active, so the 3 least recently active are c-01..c-03.
    host.note_message("draft-0", "go");
    assert_eq!(archived(&host), names("c", 1..4));
    assert_eq!(active_in(&host, &["proj"]).len(), 20);

    // Its second message sweeps nothing more even with the project re-grown.
    seed(&host, "proj", "late", 1);
    host.note_message("draft-0", "second message");
    assert_eq!(archived(&host), names("c", 1..4));
    assert_eq!(active_in(&host, &["proj"]).len(), 21);
}

// ── exemptions ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn pinned_and_live_chats_are_exempt_but_counted() {
    let dir = tempfile::tempdir().unwrap();
    let host = open_host(&dir);
    space(&host, "proj");
    seed_many(&host, "proj", "c", 22, 100);
    for id in ["c-00", "c-01"] {
        host.change_sidebar_pin(&SidebarPinChange::Pin {
            session_id: id.into(),
            after: None,
            before: None,
        })
        .unwrap();
    }
    host.record_session(&session("c-02", SessionStatus::Working, 1));
    host.record_session(&session("c-03", SessionStatus::AwaitingInput, 1));
    // Not live: a crashed engine's stale "Working", an errored and an idle run.
    host.record_session(&session("c-04", SessionStatus::Working, 600));
    host.record_session(&session("c-05", SessionStatus::Errored, 1));
    host.record_session(&session("c-06", SessionStatus::Idle, 1));

    // 22 + new = 23 → 3 archives, skipping the pinned and live chats.
    start_chat(&host, "proj", "new");

    assert_eq!(archived(&host), names("c", 4..7));
    let active = active_in(&host, &["proj"]);
    assert_eq!(active.len(), 20);
    for id in ["c-00", "c-01", "c-02", "c-03"] {
        assert!(active.contains(&id.to_string()), "{id} must stay active");
    }
}

#[tokio::test]
async fn exempt_chats_may_leave_the_project_above_twenty() {
    let dir = tempfile::tempdir().unwrap();
    let host = open_host(&dir);
    space(&host, "proj");
    seed_many(&host, "proj", "c", 21, 100);
    // Everything but the newest seeded chat is pinned or live.
    for i in 0..20 {
        let id = format!("c-{i:02}");
        if i % 2 == 0 {
            host.change_sidebar_pin(&SidebarPinChange::Pin {
                session_id: id,
                after: None,
                before: None,
            })
            .unwrap();
        } else {
            host.record_session(&session(&id, SessionStatus::Working, 1));
        }
    }

    // 21 + new = 22 → excess 2, but only c-20 is archivable.
    start_chat(&host, "proj", "new");

    assert_eq!(archived(&host), vec!["c-20".to_string()]);
    assert_eq!(active_in(&host, &["proj"]).len(), 21);
}

// ── races ───────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_first_messages_serialize_and_never_over_archive() {
    let dir = tempfile::tempdir().unwrap();
    let host = open_host(&dir);
    space(&host, "proj");
    seed_many(&host, "proj", "c", 20, 100);
    let fresh = names("new", 0..6);
    for id in &fresh {
        host.create_chat(id, Some("proj"), None, None, None).unwrap();
    }
    assert_eq!(active_in(&host, &["proj"]).len(), 26);

    let barrier = std::sync::Barrier::new(fresh.len() * 2);
    std::thread::scope(|scope| {
        for id in &fresh {
            // Two racing first messages per chat: only one sees "no message yet".
            for _ in 0..2 {
                let host = host.clone();
                let barrier = &barrier;
                scope.spawn(move || {
                    barrier.wait();
                    host.note_message(id, "go");
                });
            }
        }
    });

    assert_eq!(archived(&host), names("c", 0..6));
    let active = active_in(&host, &["proj"]);
    assert_eq!(active.len(), 20);
    for id in &fresh {
        assert!(active.contains(id), "{id} must stay active");
    }
}

#[tokio::test]
async fn run_claim_racing_ahead_of_create_chat_sweeps_once() {
    let dir = tempfile::tempdir().unwrap();
    let host = open_host(&dir);
    space(&host, "proj");
    seed_many(&host, "proj", "c", 20, 100);

    // The run command outruns createChat: claim resolves the project by cwd.
    host.claim_chat("raced", Some("/work/proj")).unwrap();
    let claimed = host.chat("raced").unwrap().unwrap();
    assert_eq!(claimed.space_id.as_deref(), Some("proj"));
    assert!(archived(&host).is_empty(), "a claim alone is not a message");

    host.note_message("raced", "hello");
    assert_eq!(archived(&host), vec!["c-00".to_string()]);

    // The late createChat (and a retry) are idempotent no-ops.
    host.create_chat("raced", Some("proj"), None, None, None).unwrap();
    host.create_chat("raced", Some("proj"), None, None, None).unwrap();
    host.claim_chat("raced", Some("/work/proj")).unwrap();
    let row = host.chat("raced").unwrap().unwrap();
    assert!(!row.archived);
    assert!(row.last_message_at.is_some());
    assert_eq!(archived(&host), vec!["c-00".to_string()]);
    assert_eq!(active_in(&host, &["proj"]).len(), 20);
}

#[tokio::test]
async fn message_for_an_unclaimed_chat_without_cwd_never_sweeps() {
    let dir = tempfile::tempdir().unwrap();
    let host = open_host(&dir);
    space(&host, "proj");
    seed_many(&host, "proj", "c", 25, 100);

    // A pre-workspace chat gains a bare row here: no project, no sweep.
    host.note_message("legacy", "hello");

    assert!(host.chat("legacy").unwrap().is_some());
    assert!(archived(&host).is_empty());
}

// ── end to end: the real send/run path ──────────────────────────────────────

type RequestLog = Arc<Mutex<Vec<RunRequest>>>;

/// Completes every turn with one line.
struct EchoHarness {
    requests: RequestLog,
}

#[async_trait]
impl Harness for EchoHarness {
    fn id(&self) -> HarnessId {
        HarnessId::Mock
    }
    fn display_name(&self) -> &str {
        "Echo"
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
        let events: Vec<Result<AgentEvent, HarnessError>> = vec![
            Ok(AgentEvent::TextDelta {
                text: format!("ack: {}", request.prompt),
            }),
            Ok(AgentEvent::Done {
                status: DoneStatus::Completed,
                result: None,
                error: None,
                session_id: None,
            }),
        ];
        Ok(futures::stream::iter(events).boxed())
    }
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
        tokio::time::sleep(Duration::from_millis(15)).await;
    }
}

fn completed_turns(core: &EngineCore, chat_id: &str) -> usize {
    let entries: Vec<SessionMessageEntry> = core
        .doc_host
        .open(chat_id)
        .ok()
        .and_then(|h| h.doc().read_entries().ok())
        .unwrap_or_default();
    entries
        .iter()
        .filter(|e| e.role == MessageRole::Assistant && e.status == Some(MessageStatus::Complete))
        .count()
}

fn queue_run(core: &EngineCore, chat_id: &str, message_id: &str, cwd: &str) {
    core.doc_host
        .queue_command(
            chat_id,
            SessionCommandPayload::Run {
                request: RunRequest {
                    mcp: None,
                    prompt: format!("hello from {message_id}"),
                    harness: None,
                    model: None,
                    reasoning: None,
                    model_options: Default::default(),
                    cwd: cwd.into(),
                    sandbox: SandboxLevel::WorkspaceWrite,
                    auto_approve: true,
                    attachments: Vec::new(),
                    worktree: None,
                    resume: None,
                    access: Default::default(),
                    plan: false,
                    env: Default::default(),
                },
                message_id: message_id.into(),
            },
        )
        .expect("queue run command");
}

#[tokio::test(flavor = "multi_thread")]
async fn first_real_run_archives_and_later_turns_and_claims_behave() {
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path().join("proj");
    std::fs::create_dir_all(&project).unwrap();
    let cwd = project.to_string_lossy().into_owned();
    let registry = HarnessRegistry::new();
    registry.register(Arc::new(EchoHarness {
        requests: RequestLog::default(),
    }));
    let core = EngineCore::assemble(
        &tmp.path().join("data"),
        Arc::new(registry),
        HarnessId::Mock,
        None,
    )
    .expect("engine core assembles");
    let ws = &core.workspace;
    ws.create_space("proj", &core.device_id, &cwd, None, false)
        .unwrap();
    for i in 0..20 {
        let id = format!("c-{i:02}");
        ws.create_chat(&id, Some("proj"), None, None, None).unwrap();
        let at = minutes_ago(100 - i);
        ws.set_chat_activity(&id, Some(at), Some(at)).unwrap();
    }

    // The composer's createChat (no message yet): 21 active, nothing archived.
    let client = zeron_rpc::memory_client(core.rpc_service());
    client
        .call(
            zeron_rpc::methods::MUTATE,
            serde_json::json!({ "op": "createChat", "chatId": "fresh", "spaceId": "proj" }),
        )
        .await
        .expect("createChat");
    ws.rename_chat("fresh", "Pre-titled").unwrap();
    assert!(archived(ws).is_empty());
    assert_eq!(active_in(ws, &["proj"]).len(), 21);

    // First real run: the oldest goes to the archive.
    queue_run(&core, "fresh", "m-1", &cwd);
    wait_for(|| completed_turns(&core, "fresh") == 1, "first turn").await;
    assert_eq!(archived(ws), vec!["c-00".to_string()]);
    assert_eq!(active_in(ws, &["proj"]).len(), 20);

    // A later turn in the same chat does not sweep, even over the limit.
    seed(ws, "proj", "late", 1);
    queue_run(&core, "fresh", "m-2", &cwd);
    wait_for(|| completed_turns(&core, "fresh") == 2, "second turn").await;
    assert_eq!(archived(ws), vec!["c-00".to_string()]);
    assert_eq!(active_in(ws, &["proj"]).len(), 21);

    // Command-first: the run claims an unknown chat into the project and its
    // first message trims the project back to 20.
    queue_run(&core, "claimed", "m-3", &cwd);
    wait_for(|| completed_turns(&core, "claimed") == 1, "claimed turn").await;
    let claimed = ws.chat("claimed").unwrap().unwrap();
    assert_eq!(claimed.space_id.as_deref(), Some("proj"));
    assert!(!claimed.archived);
    assert_eq!(archived(ws), names("c", 0..3));
    assert_eq!(active_in(ws, &["proj"]).len(), 20);

    // The createChat arriving after the claim changes nothing.
    client
        .call(
            zeron_rpc::methods::MUTATE,
            serde_json::json!({ "op": "createChat", "chatId": "claimed", "spaceId": "proj" }),
        )
        .await
        .expect("late createChat");
    assert_eq!(archived(ws), names("c", 0..3));
    assert_eq!(ws.read_chats().unwrap().len(), 23, "nothing deleted");

    core.shutdown().await;
}
