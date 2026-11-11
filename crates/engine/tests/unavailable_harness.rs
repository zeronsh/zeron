//! A run sent with an agent this device can't run (not registered here: a
//! Cloud machine has only Codex and Claude Code; a harness switched off; a
//! stale client's pick) must end in the chat, saying why. It used to be
//! rejected without a word: the message sat in the transcript with no reply
//! and no error, and the composer read "Sending…" for good.

use std::time::Duration;

use zeron_doc::{
    MessagePart, MessageRole, MessageStatus, SessionCommandPayload, SessionCommandStatus,
    SessionMessageEntry,
};
use zeron_engine::{EngineCore, HarnessRegistry};
use zeron_proto::{HarnessId, RunRequest, SandboxLevel};

const CHAT: &str = "chat-unavailable-harness";

fn entries(core: &EngineCore) -> Vec<SessionMessageEntry> {
    core.doc_host
        .open(CHAT)
        .ok()
        .and_then(|h| h.doc().read_entries().ok())
        .unwrap_or_default()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_run_for_an_agent_this_device_lacks_fails_in_the_chat() {
    let tmp = tempfile::tempdir().unwrap();
    // Nothing registered: no agent can run here.
    let registry = std::sync::Arc::new(HarnessRegistry::new());
    let core = EngineCore::assemble(&tmp.path().join("data"), registry, HarnessId::Mock, None)
        .expect("engine core assembles");
    let client = zeron_rpc::memory_client(core.rpc_service());
    client
        .call(
            zeron_rpc::methods::MUTATE,
            serde_json::json!({ "op": "createChat", "chatId": CHAT, "deviceId": core.device_id }),
        )
        .await
        .expect("createChat");
    // Pre-titled, so no titling run joins in.
    core.workspace.rename_chat(CHAT, "Pre-titled").unwrap();

    core.doc_host
        .queue_command(
            CHAT,
            SessionCommandPayload::Run {
                request: RunRequest {
                    mcp: None,
                    prompt: "hello".into(),
                    harness: Some(HarnessId::Codex),
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
                message_id: "msg-1".into(),
            },
        )
        .expect("queue run command");

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let failure = loop {
        let now = entries(&core);
        if let Some(entry) = now.iter().find(|e| e.role == MessageRole::Assistant) {
            break entry.clone();
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the run neither ran nor failed in the chat: {now:?}"
        );
        tokio::time::sleep(Duration::from_millis(15)).await;
    };

    let now = entries(&core);
    assert_eq!(now.len(), 2, "the message, then its failure: {now:?}");
    assert_eq!(now[0].id, "msg-1");
    assert_eq!(now[0].role, MessageRole::User);
    assert_eq!(failure.status, Some(MessageStatus::Aborted));
    let said: Vec<&str> = failure
        .parts
        .iter()
        .filter_map(|p| match p {
            MessagePart::Error { message, .. } => Some(message.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        said.len() == 1 && said[0].starts_with("Couldn't start this turn: "),
        "the failure says why: {said:?}"
    );

    // The command is settled, not left pending.
    let handle = core.doc_host.open(CHAT).unwrap();
    let commands = handle.doc().read_commands().unwrap();
    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0].status, SessionCommandStatus::Rejected);

    core.shutdown().await;
}
