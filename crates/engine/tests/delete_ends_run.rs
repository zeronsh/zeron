//! Deleting a chat ends its live run, and the agent's process with it
//! (2026-11-11, beta.6 staging: a deleted Claude Code chat's `claude --print`
//! and its MCP servers, ≈550 MB, stayed warm under the engine until the
//! 30-minute idle reaper). Whatever removes the row — Delete here, a delete
//! on another device that syncs in, a deleted project's sweep — and whether
//! the run is mid-turn, parked, or held by background work, the process
//! exits at once. Archiving keeps the run: an archived chat is still one.
//!
//! The harness here spawns a real child process per `run()` and kills it when
//! the run's interrupt token fires, as the real drivers tear their child
//! down.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;
use tokio::sync::mpsc;

use zeron_engine::{EngineCore, HarnessRegistry};
use zeron_harness::{Harness, HarnessError, RunControls, TurnControl};
use zeron_proto::{
    AgentEvent, ChildKind, DoneStatus, HarnessId, Model, ReasoningLevel, RunRequest, SandboxLevel,
    SessionStatus, SteeringMode,
};
use zeron_rpc::{memory_client, methods};

const CHAT: &str = "chat-delete-ends-run";

#[derive(Default)]
struct Ledger {
    /// Each live agent process's pid and turn control, by its first prompt.
    pids: Mutex<std::collections::HashMap<String, u32>>,
    turns: Mutex<std::collections::HashMap<String, TurnControl>>,
}

/// One real child process per `run()`. A prompt containing `hold` stays in
/// flight until the run is torn down; any other prompt completes at once.
struct ChildHarness {
    ledger: Arc<Ledger>,
}

fn done(status: DoneStatus) -> AgentEvent {
    AgentEvent::Done {
        status,
        result: None,
        error: None,
        session_id: Some("provider-session".into()),
    }
}

#[async_trait]
impl Harness for ChildHarness {
    fn id(&self) -> HarnessId {
        HarnessId::Mock
    }
    fn display_name(&self) -> &str {
        "Child"
    }
    fn supports_steering(&self) -> bool {
        true
    }
    fn steering_mode(&self) -> SteeringMode {
        SteeringMode::StepBoundary
    }
    fn reasoning_levels(&self) -> &[ReasoningLevel] {
        &[ReasoningLevel::Medium]
    }
    fn deterministic_turn_end(&self) -> bool {
        true
    }
    fn stops_turn_in_place(&self) -> bool {
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
        let mut child = tokio::process::Command::new("/bin/sleep")
            .arg("600")
            .kill_on_drop(true)
            .spawn()
            .expect("spawn the agent stand-in");
        let key = request.prompt.clone();
        self.ledger.pids.lock().unwrap().insert(key.clone(), child.id().unwrap());
        self.ledger.turns.lock().unwrap().insert(key, controls.turn.clone());
        let (tx, rx) = mpsc::channel::<Result<AgentEvent, HarnessError>>(64);
        tokio::spawn(async move {
            let send = |event: AgentEvent| {
                let tx = tx.clone();
                async move { tx.send(Ok(event)).await.is_ok() }
            };
            let _ = send(AgentEvent::SessionStarted {
                harness: HarnessId::Mock,
                model: "child".into(),
                tools: vec![],
                cwd: request.cwd.clone(),
                session_id: "provider-session".into(),
                assistant_message_id: "a0".into(),
            })
            .await;
            let _ = send(AgentEvent::TextDelta { text: "on it".into() }).await;
            if !request.prompt.contains("hold") && !send(done(DoneStatus::Completed)).await {
                let _ = child.kill().await;
                return;
            }
            loop {
                tokio::select! {
                    biased;
                    _ = controls.interrupt.cancelled() => {
                        let _ = child.kill().await;
                        let _ = send(done(DoneStatus::Interrupted)).await;
                        return;
                    }
                    _ = controls.turn.stop_requested() => {
                        if !send(done(DoneStatus::Interrupted)).await {
                            break;
                        }
                    }
                    steer = controls.steering.recv() => {
                        if steer.is_none() {
                            break;
                        }
                    }
                }
            }
            let _ = child.kill().await;
        });
        Ok(
            futures::stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|e| (e, rx)) })
                .boxed(),
        )
    }
}

struct Rig {
    core: EngineCore,
    ledger: Arc<Ledger>,
    _dir: tempfile::TempDir,
}

fn rig() -> Rig {
    let ledger = Arc::new(Ledger::default());
    let registry = HarnessRegistry::new();
    registry.register(Arc::new(ChildHarness {
        ledger: ledger.clone(),
    }));
    let dir = tempfile::tempdir().unwrap();
    let core = EngineCore::assemble(dir.path(), Arc::new(registry), HarnessId::Mock, None)
        .expect("engine core assembles");
    create_chat(&core, CHAT, None);
    Rig {
        core,
        ledger,
        _dir: dir,
    }
}

fn create_chat(core: &EngineCore, chat: &str, parent: Option<(&str, ChildKind)>) {
    let here = core.workspace.device_id().to_string();
    core.workspace
        .create_chat_hosted(
            chat,
            None,
            Some(&here),
            Some(&here),
            None,
            Some("/tmp".into()),
            parent.map(|(p, _)| p.to_owned()),
            parent.map(|(_, kind)| kind),
        )
        .unwrap();
}

fn request(prompt: &str) -> RunRequest {
    RunRequest {
        mcp: None,
        env: Default::default(),
        prompt: prompt.into(),
        harness: Some(HarnessId::Mock),
        model: Some("child-1".into()),
        reasoning: Some(ReasoningLevel::Medium),
        model_options: Default::default(),
        cwd: "/tmp".into(),
        sandbox: SandboxLevel::WorkspaceWrite,
        auto_approve: false,
        access: Default::default(),
        plan: false,
        attachments: Vec::new(),
        worktree: None,
        resume: None,
        extra_mcp: Vec::new(),
    }
}

fn status(core: &EngineCore, chat: &str) -> Option<SessionStatus> {
    core.sessions.session_status(chat).map(|s| s.status)
}

fn alive(pid: u32) -> bool {
    // SAFETY: signal 0 only checks that the process exists.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

async fn wait_for(mut predicate: impl FnMut() -> bool, what: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !predicate() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Start `chat`'s run and return its child's pid once `ready` holds.
async fn start_in(rig: &Rig, chat: &str, prompt: &str, ready: SessionStatus) -> u32 {
    rig.core
        .sessions
        .dispatch(chat, HarnessId::Mock, request(prompt), None)
        .await
        .unwrap();
    wait_for(
        || {
            status(&rig.core, chat) == Some(ready)
                && rig.ledger.pids.lock().unwrap().contains_key(prompt)
        },
        "the run to start",
    )
    .await;
    let pid = rig.ledger.pids.lock().unwrap()[prompt];
    assert!(alive(pid), "the agent process runs");
    pid
}

async fn start(rig: &Rig, prompt: &str, ready: SessionStatus) -> u32 {
    start_in(rig, CHAT, prompt, ready).await
}

async fn delete_chat(rig: &Rig) {
    let client = memory_client(rig.core.rpc_service());
    let _: serde_json::Value = client
        .call_as(
            methods::MUTATE,
            serde_json::json!({ "op": "deleteChat", "chatId": CHAT }),
        )
        .await
        .unwrap();
    assert!(rig.core.workspace.chat(CHAT).unwrap().is_none());
}

async fn assert_ended_in(rig: &Rig, chat: &str, pid: u32) {
    wait_for(|| !alive(pid), "the agent process to exit").await;
    wait_for(|| !rig.core.sessions.has_run(chat), "the run to end").await;
}

async fn assert_ended(rig: &Rig, pid: u32) {
    assert_ended_in(rig, CHAT, pid).await;
}

/// The run's last words (and its status) never bring a deleted chat back.
async fn assert_stays_deleted(rig: &Rig, chat: &str) {
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(rig.core.workspace.chat(chat).unwrap().is_none(), "{chat}'s row came back");
    assert!(
        !rig.core.workspace.watch_session_rows().borrow().iter().any(|s| s.chat_id == chat),
        "{chat}'s status row came back"
    );
}

#[tokio::test]
async fn deleting_a_parked_chat_ends_its_agent_process() {
    let rig = rig();
    let pid = start(&rig, "do the thing", SessionStatus::Idle).await;
    assert!(rig.core.sessions.has_run(CHAT), "the run parks warm");
    delete_chat(&rig).await;
    assert_ended(&rig, pid).await;
}

#[tokio::test]
async fn deleting_a_chat_mid_turn_ends_its_agent_process() {
    let rig = rig();
    let pid = start(&rig, "hold on", SessionStatus::Working).await;
    delete_chat(&rig).await;
    assert_ended(&rig, pid).await;
    assert_stays_deleted(&rig, CHAT).await;
}

#[tokio::test]
async fn deleting_a_chat_whose_agent_holds_background_work_ends_it() {
    let rig = rig();
    let pid = start(&rig, "do the thing", SessionStatus::Idle).await;
    // The idle reaper would wait on this work; a delete does not.
    let turn = rig.ledger.turns.lock().unwrap()["do the thing"].clone();
    turn.set_background(1);
    delete_chat(&rig).await;
    assert_ended(&rig, pid).await;
}

/// A row removed without this engine's Delete RPC — a delete made on another
/// device syncing in, a deleted project's orphan sweep — ends the run too.
#[tokio::test]
async fn a_chat_row_removed_by_sync_ends_its_agent_process() {
    let rig = rig();
    let pid = start(&rig, "do the thing", SessionStatus::Idle).await;
    rig.core.workspace.delete_chat(CHAT).unwrap();
    assert_ended(&rig, pid).await;
    assert_stays_deleted(&rig, CHAT).await;
}

/// A row removed by sync leaves the chat's doc open here, a message queued
/// behind the turn with it: ending the run must not send that message as a
/// new one.
#[tokio::test]
async fn a_message_queued_behind_a_deleted_chats_turn_starts_no_run() {
    let rig = rig();
    let pid = start(&rig, "hold on", SessionStatus::Working).await;
    rig.core
        .doc_host
        .queue_message(CHAT, "after that", Vec::new())
        .unwrap();
    rig.core.workspace.delete_chat(CHAT).unwrap();
    assert_ended(&rig, pid).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        !rig.ledger.pids.lock().unwrap().contains_key("after that"),
        "the queued message started a new agent"
    );
    assert!(!rig.core.sessions.has_run(CHAT));
    assert_stays_deleted(&rig, CHAT).await;
}

#[tokio::test]
async fn archiving_a_chat_keeps_its_run() {
    let rig = rig();
    let pid = start(&rig, "do the thing", SessionStatus::Idle).await;
    let client = memory_client(rig.core.rpc_service());
    let _: serde_json::Value = client
        .call_as(
            methods::MUTATE,
            serde_json::json!({ "op": "setChatArchived", "chatId": CHAT, "archived": true }),
        )
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(alive(pid), "an archived chat's agent stays warm");
    assert!(rig.core.sessions.has_run(CHAT));
}

/// A deleted chat's children on this computer go with it, and theirs in
/// turn: the apps show a child under its parent, so one left behind could
/// not be reached, yet would keep its doc and its warm agent.
#[tokio::test]
async fn deleting_a_parent_deletes_its_local_children_and_ends_their_runs() {
    let rig = rig();
    create_chat(&rig.core, "subagent", Some((CHAT, ChildKind::Subagent)));
    create_chat(&rig.core, "side", Some((CHAT, ChildKind::Side)));
    create_chat(&rig.core, "grandchild", Some(("subagent", ChildKind::Subagent)));
    create_chat(&rig.core, "stranger", None);
    let parent = start(&rig, "parent work", SessionStatus::Idle).await;
    let subagent = start_in(&rig, "subagent", "subagent work", SessionStatus::Idle).await;
    let side = start_in(&rig, "side", "side work", SessionStatus::Idle).await;
    let stranger = start_in(&rig, "stranger", "stranger work", SessionStatus::Idle).await;
    delete_chat(&rig).await;
    for gone in [CHAT, "subagent", "side", "grandchild"] {
        assert!(rig.core.workspace.chat(gone).unwrap().is_none(), "{gone} survived");
    }
    assert_ended_in(&rig, CHAT, parent).await;
    assert_ended_in(&rig, "subagent", subagent).await;
    assert_ended_in(&rig, "side", side).await;
    assert!(rig.core.workspace.chat("stranger").unwrap().is_some(), "an unrelated chat stays");
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(alive(stranger) && rig.core.sessions.has_run("stranger"), "and keeps its run");
}
