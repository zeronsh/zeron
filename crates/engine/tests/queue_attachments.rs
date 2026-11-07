//! A queued message's files reach the agent as attachments (images as
//! images), however the row leaves the queue: steered into the running
//! turn, sent now, or drained when the turn ends. A row queued on another
//! device names its files by `pending://` refs while the bytes chase it;
//! it goes once they land, by this device's paths.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine as _;
use futures::StreamExt;
use futures::stream::BoxStream;
use tokio::sync::mpsc;

use zeron_engine::doc_host::{
    BeginQueueEditOutcome, FinishQueueEditAction, FinishQueueEditOutcome,
};
use zeron_engine::{EngineCore, HarnessRegistry};
use zeron_harness::{Harness, HarnessError, RunControls};
use zeron_proto::{
    AgentEvent, DoneStatus, HarnessId, Model, ReasoningLevel, RunRequest, SandboxLevel,
    SessionStatus, SteeringMode,
};

const CHAT: &str = "chat-queue-attachments";

#[derive(Default)]
struct Ledger {
    /// Every prompt the agent read, with the files it came with.
    read: Mutex<Vec<(String, Vec<String>)>>,
    /// Ends the turn in flight.
    release: tokio::sync::Notify,
}

/// One agent process per `run()`. A prompt containing `hold` keeps its turn
/// running until released or stopped; steers join it as they come.
struct Recorder {
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
impl Harness for Recorder {
    fn id(&self) -> HarnessId {
        HarnessId::Mock
    }
    fn display_name(&self) -> &str {
        "Recorder"
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
        let ledger = self.ledger.clone();
        let (tx, rx) = mpsc::channel::<Result<AgentEvent, HarnessError>>(64);
        tokio::spawn(async move {
            let turn = controls.turn.clone();
            let send = |event: AgentEvent| {
                let tx = tx.clone();
                async move { tx.send(Ok(event)).await.is_ok() }
            };
            let _ = send(AgentEvent::SessionStarted {
                harness: HarnessId::Mock,
                model: "recorder".into(),
                tools: vec![],
                cwd: request.cwd.clone(),
                session_id: "provider-session".into(),
                assistant_message_id: "a0".into(),
            })
            .await;
            let read = |prompt: &str, files: &[String]| {
                ledger
                    .read
                    .lock()
                    .unwrap()
                    .push((prompt.to_owned(), files.to_vec()));
                prompt.contains("hold")
            };
            let mut holding = read(&request.prompt, &request.attachments);
            if !holding && !send(done(DoneStatus::Completed)).await {
                return;
            }
            loop {
                tokio::select! {
                    biased;
                    _ = controls.interrupt.cancelled() => {
                        let _ = send(done(DoneStatus::Interrupted)).await;
                        return;
                    }
                    _ = ledger.release.notified(), if holding => {
                        holding = false;
                        if !send(done(DoneStatus::Completed)).await {
                            return;
                        }
                    }
                    _ = turn.stop_requested() => {
                        holding = false;
                        if !send(done(DoneStatus::Interrupted)).await {
                            return;
                        }
                    }
                    steer = controls.steering.recv() => {
                        let Some(steer) = steer else { return };
                        if !send(AgentEvent::Steered {
                            assistant_message_id: None,
                            next_assistant_message_id: None,
                        })
                        .await
                        {
                            return;
                        }
                        // Into the running turn, or a turn of its own.
                        let starts = !holding;
                        holding |= read(&steer.prompt, &steer.attachments);
                        if starts && !holding && !send(done(DoneStatus::Completed)).await {
                            return;
                        }
                    }
                }
            }
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
    dir: tempfile::TempDir,
}

fn rig() -> Rig {
    let ledger = Arc::new(Ledger::default());
    let registry = HarnessRegistry::new();
    registry.register(Arc::new(Recorder {
        ledger: ledger.clone(),
    }));
    let dir = tempfile::tempdir().unwrap();
    let core = EngineCore::assemble(dir.path(), Arc::new(registry), HarnessId::Mock, None)
        .expect("engine core assembles");
    Rig { core, ledger, dir }
}

fn request(prompt: &str) -> RunRequest {
    RunRequest {
        mcp: None,
        env: Default::default(),
        prompt: prompt.into(),
        harness: Some(HarnessId::Mock),
        model: Some("recorder-1".into()),
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

fn status(core: &EngineCore) -> Option<SessionStatus> {
    core.sessions.session_status(CHAT).map(|s| s.status)
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

/// What the agent read for the prompt starting `words`.
fn read_for(rig: &Rig, words: &str) -> Option<(String, Vec<String>)> {
    rig.ledger
        .read
        .lock()
        .unwrap()
        .iter()
        .find(|(p, _)| p.starts_with(words))
        .cloned()
}

/// A picture and a file, as this device's uploads hold them.
fn files(rig: &Rig) -> Vec<String> {
    let dir = rig.dir.path().join("files");
    std::fs::create_dir_all(&dir).unwrap();
    let png = dir.join("shot.png");
    std::fs::write(&png, b"\x89PNG\r\n\x1a\n").unwrap();
    let txt = dir.join("note.txt");
    std::fs::write(&txt, b"hello").unwrap();
    vec![
        png.to_string_lossy().into_owned(),
        txt.to_string_lossy().into_owned(),
    ]
}

async fn running(rig: &Rig) {
    rig.core
        .sessions
        .dispatch(CHAT, HarnessId::Mock, request("hold: working"), None)
        .await
        .unwrap();
    wait_for(
        || status(&rig.core) == Some(SessionStatus::Working) && read_for(rig, "hold").is_some(),
        "the turn to run",
    )
    .await;
}

fn queue(rig: &Rig, text: &str, attachments: Vec<String>) -> String {
    rig.core
        .doc_host
        .queue_message_with_behavior(CHAT, text, attachments, true)
        .unwrap()
}

fn queued(rig: &Rig) -> usize {
    rig.core
        .doc_host
        .open(CHAT)
        .unwrap()
        .doc()
        .read_queue()
        .unwrap()
        .len()
}

#[tokio::test]
async fn a_queued_row_with_files_steers_with_them_as_attachments() {
    let rig = rig();
    running(&rig).await;
    let files = files(&rig);
    let id = queue(&rig, "look at these", files.clone());
    assert!(
        rig.core.doc_host.steer_queued_now(CHAT, &id).await.unwrap(),
        "the row steers"
    );
    wait_for(|| read_for(&rig, "look at these").is_some(), "the steer").await;
    let (_, got) = read_for(&rig, "look at these").unwrap();
    assert_eq!(got, files, "its files ride the steer as attachments");
    assert_eq!(queued(&rig), 0);
    rig.core.sessions.shutdown().await;
}

#[tokio::test]
async fn a_row_from_another_device_steers_once_its_bytes_land() {
    let rig = rig();
    running(&rig).await;
    let reference = "pending://up-1/shot.png";
    let id = queue(&rig, "from my phone", vec![reference.to_string()]);
    let early = rig.core.doc_host.steer_queued_now(CHAT, &id).await;
    assert!(
        early
            .as_ref()
            .is_err_and(|e| e.to_string().contains("still uploading")),
        "says why it can't go yet: {early:?}"
    );
    assert_eq!(queued(&rig), 1, "and stays queued");
    // The bytes land.
    let png = base64::engine::general_purpose::STANDARD.encode(b"\x89PNG\r\n\x1a\n");
    rig.core.uploads.append("up-1", &png, Some(0)).unwrap();
    let landed = rig.core.uploads.commit("up-1", "shot.png").unwrap();
    assert!(rig.core.doc_host.steer_queued_now(CHAT, &id).await.unwrap());
    wait_for(|| read_for(&rig, "from my phone").is_some(), "the steer").await;
    let (prompt, got) = read_for(&rig, "from my phone").unwrap();
    assert_eq!(got, vec![landed.clone()], "by this device's path");
    assert!(
        !prompt.contains("pending://"),
        "no ref left in the words: {prompt}"
    );
    rig.core.sessions.shutdown().await;
}

#[tokio::test]
async fn a_row_with_files_sent_now_or_drained_keeps_them() {
    let rig = rig();
    running(&rig).await;
    let files = files(&rig);
    let now = queue(&rig, "send this now", files.clone());
    assert!(rig.core.doc_host.send_queued_now(CHAT, &now).await.unwrap());
    wait_for(|| read_for(&rig, "send this now").is_some(), "the send").await;
    assert_eq!(read_for(&rig, "send this now").unwrap().1, files);
    wait_for(
        || status(&rig.core) == Some(SessionStatus::Idle),
        "that turn to end",
    )
    .await;

    // Held for the turn's end, then drained.
    rig.core
        .sessions
        .dispatch(CHAT, HarnessId::Mock, request("hold: again"), None)
        .await
        .unwrap();
    wait_for(
        || status(&rig.core) == Some(SessionStatus::Working),
        "the next turn",
    )
    .await;
    queue(&rig, "after the turn", files.clone());
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        read_for(&rig, "after the turn").is_none(),
        "held for the turn"
    );
    rig.ledger.release.notify_one();
    wait_for(|| read_for(&rig, "after the turn").is_some(), "the drain").await;
    assert_eq!(read_for(&rig, "after the turn").unwrap().1, files);
    rig.core.sessions.shutdown().await;
}

/// Files alone, sent while a turn runs: the app's `QueueMessage` has no
/// words to give. It goes as a run's does, and never comes back refused
/// (2026-11-07: a screenshot said "Your message didn't send", twice).
#[tokio::test]
async fn files_with_no_words_queue_and_reach_the_agent() {
    let rig = rig();
    running(&rig).await;
    let files = files(&rig);
    let client = zeron_rpc::memory_client(rig.core.rpc_service());
    client
        .call(
            zeron_rpc::methods::QUEUE_MESSAGE,
            serde_json::json!({
                "chatId": CHAT,
                "text": "",
                "attachments": files,
                "holdForTurnEnd": true,
            }),
        )
        .await
        .expect("a message of files alone is queued");
    assert_eq!(queued(&rig), 1, "it waits in the queue, its files with it");
    rig.ledger.release.notify_one();
    wait_for(
        || read_for(&rig, "See the attached image(s).").is_some(),
        "the drain",
    )
    .await;
    assert_eq!(
        read_for(&rig, "See the attached image(s).").unwrap().1,
        files
    );
    rig.core.sessions.shutdown().await;
}

/// Still refused: nothing at all.
#[tokio::test]
async fn a_message_with_no_words_and_no_files_is_refused() {
    let rig = rig();
    assert!(
        rig.core
            .doc_host
            .queue_message_with_behavior(CHAT, "  ", Vec::new(), true)
            .is_err()
    );
    rig.core.sessions.shutdown().await;
}

async fn edit_lease(rig: &Rig, id: &str) -> (String, String) {
    let BeginQueueEditOutcome::Acquired {
        lease_id,
        base_text_hash,
        ..
    } = rig
        .core
        .doc_host
        .begin_queued_message_edit(CHAT, id, "phone", "view-1")
        .await
        .unwrap()
    else {
        panic!("the edit is acquired");
    };
    (lease_id, base_text_hash)
}

/// An edit that clears the words of a row keeping its files leaves the row;
/// taking the files off as well is how it goes.
#[tokio::test]
async fn clearing_the_words_of_a_row_with_files_keeps_it() {
    let rig = rig();
    running(&rig).await;
    let files = files(&rig);
    let id = queue(&rig, "look at these", files.clone());
    let (lease, hash) = edit_lease(&rig, &id).await;
    assert!(matches!(
        rig.core
            .doc_host
            .finish_queued_message_edit_with_attachments(
                CHAT,
                &id,
                &lease,
                FinishQueueEditAction::Commit,
                Some(""),
                Some(&hash),
                Some(&files),
            )
            .await
            .unwrap(),
        FinishQueueEditOutcome::Committed
    ));
    let rows = rig
        .core
        .doc_host
        .open(CHAT)
        .unwrap()
        .doc()
        .read_queue()
        .unwrap();
    assert_eq!(rows.len(), 1, "the row stays");
    assert_eq!(rows[0].text, "See the attached image(s).");
    assert_eq!(rows[0].attachments, files);

    let (lease, hash) = edit_lease(&rig, &id).await;
    rig.core
        .doc_host
        .finish_queued_message_edit_with_attachments(
            CHAT,
            &id,
            &lease,
            FinishQueueEditAction::Commit,
            Some(""),
            Some(&hash),
            Some(&[]),
        )
        .await
        .unwrap();
    assert_eq!(queued(&rig), 0, "no words and no files: it goes");
    rig.core.sessions.shutdown().await;
}

/// Another Windows user's (or device's) path for an upload this device also
/// holds: what reached the agent on 2026-11-07 as `C:\Users\ADMIN\…`, from a
/// chat running as `HP MEDIA`.
const THEIRS: &str = r"C:\Users\ADMIN\AppData\Local\Zeron\uploads\2fbd2b3b-image.png";

/// The same upload committed on this device: its path here.
async fn committed_here(rig: &Rig) -> String {
    let client = zeron_rpc::memory_client(rig.core.rpc_service());
    client
        .call(
            zeron_rpc::methods::UPLOAD_CHUNK,
            serde_json::json!({
                "uploadId": "2fbd2b3b-9c1e",
                "seq": 0,
                "data": base64::engine::general_purpose::STANDARD.encode(b"\x89PNG\r\n\x1a\n"),
            }),
        )
        .await
        .unwrap();
    let reply = client
        .call(
            zeron_rpc::methods::UPLOAD_COMMIT,
            serde_json::json!({ "uploadId": "2fbd2b3b-9c1e", "fileName": "image.png" }),
        )
        .await
        .unwrap();
    reply["path"].as_str().unwrap().to_owned()
}

#[tokio::test]
async fn a_row_naming_another_users_path_reaches_the_agent_by_this_devices_copy() {
    let rig = rig();
    running(&rig).await;
    let here = committed_here(&rig).await;
    // Steered in.
    let id = queue(&rig, "steered with it", vec![THEIRS.into()]);
    assert!(rig.core.doc_host.steer_queued_now(CHAT, &id).await.unwrap());
    wait_for(|| read_for(&rig, "steered with it").is_some(), "the steer").await;
    let (prompt, got) = read_for(&rig, "steered with it").unwrap();
    assert_eq!(got, vec![here.clone()]);
    assert!(
        prompt.contains(&here) && !prompt.contains("ADMIN"),
        "got: {prompt}"
    );
    // Drained at the turn's end.
    queue(&rig, "drained with it", vec![THEIRS.into()]);
    rig.ledger.release.notify_one();
    wait_for(|| read_for(&rig, "drained with it").is_some(), "the drain").await;
    let (prompt, got) = read_for(&rig, "drained with it").unwrap();
    assert_eq!(got, vec![here.clone()]);
    assert!(
        prompt.contains(&here) && !prompt.contains("ADMIN"),
        "got: {prompt}"
    );
    rig.core.sessions.shutdown().await;
}

#[tokio::test]
async fn a_path_this_device_cant_place_goes_as_it_was() {
    let rig = rig();
    running(&rig).await;
    // No copy here: nothing to rebase it to.
    let id = queue(&rig, "nothing here", vec![THEIRS.into()]);
    assert!(rig.core.doc_host.steer_queued_now(CHAT, &id).await.unwrap());
    wait_for(|| read_for(&rig, "nothing here").is_some(), "the steer").await;
    assert_eq!(
        read_for(&rig, "nothing here").unwrap().1,
        vec![THEIRS.to_string()]
    );
    rig.core.sessions.shutdown().await;
}
