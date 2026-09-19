//! "Don't work in a project" against a real engine: a chat minted through the
//! UI's exact wire shape (`Mutate createChat` with a `deviceId` and no
//! `spaceId`) stores cwd `~`, spawns its run from the host's REAL home dir,
//! and never mints a space row — the two failure modes of pre-#40 engines
//! (a phantom project at root, and the run dying on the literal `~`).

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
    SteeringMode,
};

const CHAT: &str = "chat-projectless";

type RequestLog = Arc<Mutex<Vec<RunRequest>>>;

/// Records every `RunRequest` it receives (the cwd probe), then completes a
/// one-line turn.
struct RecordingHarness {
    requests: RequestLog,
}

#[async_trait]
impl Harness for RecordingHarness {
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
        self.requests
            .lock()
            .expect("request log")
            .push(request.clone());
        let events: Vec<Result<AgentEvent, HarnessError>> = vec![
            Ok(AgentEvent::SessionStarted {
                harness: HarnessId::Mock,
                model: "mock-1".into(),
                tools: vec![],
                cwd: request.cwd.clone(),
                session_id: "sess-np".into(),
                assistant_message_id: "a-1".into(),
            }),
            Ok(AgentEvent::TextDelta {
                text: format!("ack: {}", request.prompt),
            }),
            Ok(AgentEvent::Done {
                status: DoneStatus::Completed,
                result: None,
                error: None,
                session_id: Some("sess-np".into()),
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
async fn projectless_chat_runs_from_home_and_mints_no_space() {
    exercise_projectless(false).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn projectless_terminal_resolves_home_and_preserves_explicit_paths() {
    use zeron_rpc::methods;

    let tmp = tempfile::tempdir().unwrap();
    let core = EngineCore::assemble(
        &tmp.path().join("data"),
        Arc::new(HarnessRegistry::new()),
        HarnessId::Mock,
        None,
    )
    .expect("engine core assembles");
    let client = zeron_rpc::memory_client(core.rpc_service());
    let home = std::env::var("HOME").expect("HOME set in test env");

    for (chat_id, cwd, expected) in [
        ("default-home", None, home.clone()),
        ("home-slash", Some("~/".to_string()), home.clone()),
        (
            "home-relative",
            Some("~/../".to_string()),
            format!("{home}/../"),
        ),
        (
            "explicit",
            Some(tmp.path().to_string_lossy().into_owned()),
            tmp.path().to_string_lossy().into_owned(),
        ),
    ] {
        let mut params = serde_json::json!({
            "op": "createChat", "chatId": chat_id, "deviceId": core.device_id,
        });
        if let Some(cwd) = &cwd {
            params["cwd"] = serde_json::json!(cwd);
        }
        client.call(methods::MUTATE, params).await.unwrap();
        let session = client
            .call(
                methods::OPEN_TERMINAL,
                serde_json::json!({
                    "chatId": chat_id, "cols": 80, "rows": 24,
                }),
            )
            .await
            .expect("open terminal");
        // Compare paths to tolerate the trailing slash from joining ~/.
        assert_eq!(
            std::path::Path::new(session["cwd"].as_str().unwrap()),
            std::path::Path::new(&expected)
        );
        client
            .call(
                methods::CLOSE_TERMINAL,
                serde_json::json!({
                    "terminalId": session["id"],
                }),
            )
            .await
            .expect("close terminal");
        let chat = core.workspace.chat(chat_id).unwrap().unwrap();
        assert_eq!(chat.cwd.as_deref(), Some(cwd.as_deref().unwrap_or("~")));
        assert_eq!(chat.space_id, None);
    }

    // The existing fallback for a chat without metadata remains available.
    let session = client
        .call(
            methods::OPEN_TERMINAL,
            serde_json::json!({
                "chatId": "missing-row", "cols": 80, "rows": 24,
            }),
        )
        .await
        .expect("home fallback");
    assert_eq!(session["cwd"], home);
    client
        .call(
            methods::CLOSE_TERMINAL,
            serde_json::json!({
                "terminalId": session["id"],
            }),
        )
        .await
        .unwrap();

    core.workspace
        .create_chat(
            "invalid",
            None,
            Some(&core.device_id),
            None,
            Some(tmp.path().join("missing").to_string_lossy().into_owned()),
        )
        .unwrap();
    let err = client
        .call(
            methods::OPEN_TERMINAL,
            serde_json::json!({
                "chatId": "invalid", "cols": 80, "rows": 24,
            }),
        )
        .await
        .expect_err("explicit missing path must not fall back to home");
    assert!(
        err.to_string()
            .contains("Session working directory is unavailable")
    );
    assert!(core.workspace.read_spaces().unwrap().is_empty());
    core.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn projectless_command_before_metadata_survives_restart_and_resume() {
    exercise_projectless(true).await;
}

async fn exercise_projectless(command_first: bool) {
    let tmp = tempfile::tempdir().unwrap();
    let requests: RequestLog = RequestLog::default();
    let registry = HarnessRegistry::new();
    registry.register(Arc::new(RecordingHarness {
        requests: requests.clone(),
    }));
    let registry = Arc::new(registry);
    let core = EngineCore::assemble(
        &tmp.path().join("data"),
        registry.clone(),
        HarnessId::Mock,
        None,
    )
    .expect("engine core assembles");

    // The composer's exact wire shape for "Don't work in a project": a
    // deviceId, no spaceId, no cwd.
    let client = zeron_rpc::memory_client(core.rpc_service());
    if !command_first {
        client
            .call(
                zeron_rpc::methods::MUTATE,
                serde_json::json!({
                    "op": "createChat",
                    "chatId": CHAT,
                    "deviceId": core.device_id,
                }),
            )
            .await
            .expect("createChat without a space");
    }
    if !command_first {
        // Pre-title so the auto-titler's own harness request stays out of the log.
        core.workspace
            .rename_chat(CHAT, "Pre-titled")
            .expect("rename chat");

        let chat = core
            .workspace
            .chat(CHAT)
            .expect("read chat row")
            .expect("chat row exists");
        assert_eq!(chat.space_id, None, "project-less chat must carry no space");
        assert_eq!(chat.cwd.as_deref(), Some("~"), "cwd defaults to `~`");
        assert_eq!(chat.device_id, core.device_id);
    }
    // Run exactly as the composer sends it: the chat's stored cwd, `~`.
    let queue_turn = |core: &EngineCore, message_id: &str| {
        core.doc_host
            .queue_command(
                CHAT,
                SessionCommandPayload::Run {
                    request: RunRequest {
                        prompt: "hello from no project".into(),
                        harness: None,
                        model: None,
                        reasoning: None,
                        model_options: Default::default(),
                        cwd: "~".into(),
                        sandbox: SandboxLevel::WorkspaceWrite,
                        auto_approve: true,
                        attachments: Vec::new(),
                        worktree: None,
                        resume: None,
                    },
                    message_id: message_id.into(),
                },
            )
            .expect("queue run command");
    };
    queue_turn(&core, "msg-np-1");
    wait_for(|| complete_assistant_count(&core) == 1, "turn to complete").await;

    let chat = core.workspace.chat(CHAT).unwrap().unwrap();
    assert_eq!(chat.space_id, None);
    assert_eq!(chat.cwd.as_deref(), Some("~"));
    assert_eq!(chat.device_id, core.device_id);
    // Simulate the metadata arriving after the command and a retry.
    for _ in 0..2 {
        client
            .call(
                zeron_rpc::methods::MUTATE,
                serde_json::json!({
                    "op": "createChat", "chatId": CHAT, "deviceId": core.device_id,
                }),
            )
            .await
            .unwrap();
    }

    // The harness must see the host's real home dir, not the literal `~`.
    let cwds: Vec<String> = requests
        .lock()
        .expect("request log")
        .iter()
        .filter(|r| r.prompt == "hello from no project")
        .map(|r| r.cwd.clone())
        .collect();
    let home = std::env::var("HOME").expect("HOME set in test env");
    assert_eq!(cwds, vec![home], "run spawns from the expanded home dir");

    // And no phantom project: the flow must not mint any space row.
    let spaces = core.workspace.read_spaces().expect("read spaces");
    assert!(
        spaces.is_empty(),
        "project-less chat minted a space: {spaces:?}"
    );

    core.shutdown().await;
    drop(client);
    drop(core);

    let core = EngineCore::assemble(&tmp.path().join("data"), registry, HarnessId::Mock, None)
        .expect("reopen persisted engine");
    let chat = core.workspace.chat(CHAT).unwrap().unwrap();
    assert_eq!(chat.space_id, None);
    assert_eq!(chat.cwd.as_deref(), Some("~"));
    assert_eq!(
        complete_assistant_count(&core),
        1,
        "transcript survives restart"
    );
    queue_turn(&core, "msg-np-2");
    wait_for(|| complete_assistant_count(&core) == 2, "resumed turn").await;
    {
        let log = requests.lock().unwrap();
        let requests: Vec<_> = log
            .iter()
            .filter(|r| r.prompt == "hello from no project")
            .collect();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[1].cwd, std::env::var("HOME").unwrap());
        assert_eq!(requests[1].resume.as_deref(), Some("sess-np"));
    }
    assert!(core.workspace.read_spaces().unwrap().is_empty());
    assert_eq!(core.workspace.read_chats().unwrap().len(), 1);
    core.shutdown().await;
}
