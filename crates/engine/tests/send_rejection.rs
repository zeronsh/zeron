//! A send its host refuses reaches the client: `WaitingMessages` lists it as
//! refused, with the host's reason and whether it asked for a worktree, so
//! the app shows it failed instead of "Sending…" forever. `RetryDelivery`
//! sends it again as it was, or without its worktree request.

use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;

use zeron_doc::{MessageRole, MessageStatus, SessionCommandPayload, SessionMessageEntry};
use zeron_engine::{EngineCore, HarnessRegistry};
use zeron_harness::{Harness, HarnessError, RunControls};
use zeron_proto::{
    AgentEvent, DoneStatus, HarnessId, Model, ReasoningLevel, RunRequest, SandboxLevel,
    SteeringMode, WaitingMessage, WorktreeSpec,
};

const CHAT: &str = "chat-send-rejection";
const MESSAGE: &str = "msg-refused";

/// Completes a one-line turn and records the cwd each run spawned with.
struct RecordingHarness {
    cwds: Arc<Mutex<Vec<String>>>,
}

#[async_trait]
impl Harness for RecordingHarness {
    fn id(&self) -> HarnessId {
        HarnessId::Mock
    }
    fn display_name(&self) -> &str {
        "Recorder"
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
        self.cwds.lock().unwrap().push(request.cwd.clone());
        let events: Vec<Result<AgentEvent, HarnessError>> = vec![
            Ok(AgentEvent::SessionStarted {
                harness: HarnessId::Mock,
                model: "mock-1".into(),
                tools: vec![],
                cwd: request.cwd.clone(),
                session_id: "sess-refused".into(),
                assistant_message_id: "a-1".into(),
            }),
            Ok(AgentEvent::TextDelta {
                text: format!("ack: {}", request.prompt),
            }),
            Ok(AgentEvent::Done {
                status: DoneStatus::Completed,
                result: None,
                error: None,
                session_id: Some("sess-refused".into()),
            }),
        ];
        Ok(futures::stream::iter(events).boxed())
    }
}

async fn wait_for<F>(mut predicate: F, what: &str)
where
    F: FnMut() -> bool,
{
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    while !predicate() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(15)).await;
    }
}

fn git(cwd: &std::path::Path, args: &[&str]) {
    let out = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn waiting(core: &EngineCore) -> Vec<WaitingMessage> {
    core.doc_host.waiting_messages(CHAT).unwrap_or_default()
}

fn refused(core: &EngineCore) -> Option<WaitingMessage> {
    waiting(core).into_iter().find(|w| w.refused)
}

fn complete_assistant_count(core: &EngineCore) -> usize {
    let entries: Vec<SessionMessageEntry> = core
        .doc_host
        .open(CHAT)
        .ok()
        .and_then(|h| h.doc().read_entries().ok())
        .unwrap_or_default();
    entries
        .iter()
        .filter(|e| e.role == MessageRole::Assistant && e.status == Some(MessageStatus::Complete))
        .count()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refused_worktree_send_reaches_the_client_and_retries_without_it() {
    let tmp = tempfile::tempdir().unwrap();
    let tmp_path = tmp.path().canonicalize().unwrap();
    let repo_dir = tmp_path.join("repo");
    std::fs::create_dir_all(&repo_dir).unwrap();
    git(&repo_dir, &["init", "-b", "main"]);
    git(&repo_dir, &["config", "user.email", "t@example.com"]);
    git(&repo_dir, &["config", "user.name", "Test"]);
    std::fs::write(repo_dir.join("README.md"), "hello\n").unwrap();
    git(&repo_dir, &["add", "."]);
    git(&repo_dir, &["commit", "-m", "init"]);
    let repo_path = repo_dir.to_string_lossy().to_string();

    let cwds: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let registry = HarnessRegistry::new();
    registry.register(Arc::new(RecordingHarness { cwds: cwds.clone() }));
    let core = EngineCore::assemble(
        &tmp_path.join("data"),
        Arc::new(registry),
        HarnessId::Mock,
        None,
    )
    .expect("engine core assembles");
    let client = zeron_rpc::memory_client(core.rpc_service());
    client
        .call(
            zeron_rpc::methods::MUTATE,
            serde_json::json!({ "op": "createChat", "chatId": CHAT, "deviceId": core.device_id }),
        )
        .await
        .expect("createChat");
    core.workspace
        .rename_chat(CHAT, "Pre-titled")
        .expect("rename chat");

    // A worktree off a base the repo doesn't have: its host can't make it.
    let run = SessionCommandPayload::Run {
        request: RunRequest {
            mcp: None,
            prompt: "isolated please".into(),
            harness: None,
            model: None,
            reasoning: None,
            model_options: Default::default(),
            cwd: repo_path.clone(),
            sandbox: SandboxLevel::WorkspaceWrite,
            auto_approve: true,
            access: Default::default(),
            plan: false,
            attachments: Vec::new(),
            resume: None,
            worktree: Some(WorktreeSpec {
                repo_path: repo_path.clone(),
                base: "no-such-base".into(),
                space_id: None,
            }),
            env: Default::default(),
            extra_mcp: Vec::new(),
        },
        message_id: MESSAGE.into(),
    };
    let first = core
        .doc_host
        .queue_command(CHAT, run)
        .expect("queue run command");
    wait_for(
        || refused(&core).is_some(),
        "the refusal to reach the waiting list",
    )
    .await;

    let shown = refused(&core).unwrap();
    assert_eq!(shown.id, first);
    assert_eq!(shown.message_id.as_deref(), Some(MESSAGE));
    assert_eq!(shown.prompt, "isolated please");
    assert!(shown.worktree, "it asked for a worktree: {shown:?}");
    assert!(!shown.held);
    let why = shown.failure.clone().unwrap_or_default();
    assert!(
        !why.is_empty(),
        "the host's reason comes with it: {shown:?}"
    );
    assert_eq!(
        waiting(&core).len(),
        1,
        "one row for the message: {:?}",
        waiting(&core)
    );
    assert!(cwds.lock().unwrap().is_empty(), "nothing ran");

    // Retry as it was: refused again, and still one row for it (the newest
    // attempt speaks for the message).
    client
        .call(
            zeron_rpc::methods::RETRY_DELIVERY,
            serde_json::json!({ "chatId": CHAT }),
        )
        .await
        .expect("retry");
    wait_for(
        || refused(&core).is_some_and(|w| w.id != first),
        "the retried attempt to be refused again",
    )
    .await;
    assert_eq!(waiting(&core).len(), 1, "{:?}", waiting(&core));

    // Without its worktree: it runs in the chat's folder, and leaves the list.
    client
        .call(
            zeron_rpc::methods::RETRY_DELIVERY,
            serde_json::json!({ "chatId": CHAT, "withoutWorktree": true }),
        )
        .await
        .expect("retry without worktree");
    wait_for(
        || complete_assistant_count(&core) == 1,
        "the run without its worktree",
    )
    .await;
    assert_eq!(cwds.lock().unwrap().as_slice(), [repo_path.clone()]);
    wait_for(|| waiting(&core).is_empty(), "the waiting list to clear").await;
}
