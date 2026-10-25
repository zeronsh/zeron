//! Experimental transcript editing through the engine RPC: apply, revision and
//! busy refusals, runtime retirement, the undo stack and the transcript mirror.
//! The agent is a stand-in whose "native session" is a list held in memory.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;
use tokio::sync::mpsc;

use zeron_doc::{MessagePart, MessageRole};
use zeron_engine::{EngineCore, HarnessRegistry};
use zeron_harness::{Harness, HarnessError, NativeHistoryRead, RunControls};
use zeron_proto::native_history::{
    NativeEdit, NativeHistory, NativeHistoryItem, NativeItemKind, NativeRole, ReasoningSeal,
};
use zeron_proto::{
    AgentEvent, DoneStatus, HarnessId, Model, ReasoningLevel, RunRequest, SandboxLevel,
    SessionStatus, SteeringMode,
};
use zeron_rpc::methods;

const CHAT: &str = "chat-native-history";
const PROMPT: &str = "Remember PINEAPPLE";

#[derive(Clone)]
struct Stored {
    items: Vec<NativeHistoryItem>,
    revision: String,
}

#[derive(Default)]
struct Agent {
    spawns: AtomicUsize,
    kills: AtomicUsize,
    /// The session each run was asked to resume.
    resumes: Mutex<Vec<Option<String>>>,
    sessions: Mutex<HashMap<String, Stored>>,
}

fn text(id: &str, role: NativeRole, text: &str) -> NativeHistoryItem {
    NativeHistoryItem {
        id: id.into(),
        role,
        turn: 0,
        kind: NativeItemKind::Text { text: text.into() },
        editable: true,
        removable: false,
    }
}

impl Agent {
    /// What the first session holds once the first prompt ran.
    fn first_session() -> Stored {
        Stored {
            items: vec![
                text("u0", NativeRole::User, PROMPT),
                NativeHistoryItem {
                    id: "r0".into(),
                    role: NativeRole::Assistant,
                    turn: 0,
                    kind: NativeItemKind::Reasoning {
                        text: Some("pondering".into()),
                        seal: ReasoningSeal::Signed,
                    },
                    editable: false,
                    removable: true,
                },
                text("a0", NativeRole::Assistant, &format!("on it: {PROMPT}")),
            ],
            revision: "rev-1".into(),
        }
    }

    fn session(&self, id: &str) -> Result<Stored, HarnessError> {
        let mut sessions = self.sessions.lock().unwrap();
        if id == "s1" {
            return Ok(sessions
                .entry(id.into())
                .or_insert_with(Self::first_session)
                .clone());
        }
        sessions
            .get(id)
            .cloned()
            .ok_or_else(|| HarnessError::History("gone".into()))
    }
}

struct Native(Arc<Agent>);

fn done(session: &str, status: DoneStatus) -> AgentEvent {
    AgentEvent::Done {
        status,
        result: None,
        error: None,
        session_id: Some(session.into()),
    }
}

#[async_trait]
impl Harness for Native {
    fn id(&self) -> HarnessId {
        HarnessId::Mock
    }
    fn display_name(&self) -> &str {
        "Native"
    }
    fn supports_steering(&self) -> bool {
        true
    }
    fn steering_mode(&self) -> SteeringMode {
        SteeringMode::StepBoundary
    }
    fn reasoning_levels(&self) -> &[ReasoningLevel] {
        &[]
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
    fn supports_native_history(&self) -> bool {
        true
    }
    async fn read_native_history(
        &self,
        session_id: &str,
        _cwd: &std::path::Path,
        _env: &std::collections::BTreeMap<String, String>,
    ) -> Result<NativeHistoryRead, HarnessError> {
        let stored = self.0.session(session_id)?;
        Ok(NativeHistoryRead {
            revision: stored.revision,
            items: stored.items,
            compacted: false,
        })
    }
    async fn write_edited_session(
        &self,
        session_id: &str,
        _cwd: &std::path::Path,
        _env: &std::collections::BTreeMap<String, String>,
        revision: &str,
        edits: &[NativeEdit],
    ) -> Result<String, HarnessError> {
        let mut stored = self.0.session(session_id)?;
        if stored.revision != revision {
            return Err(HarnessError::History("stale".into()));
        }
        for edit in edits {
            match edit {
                NativeEdit::Replace { id, text } => {
                    for item in stored.items.iter_mut().filter(|item| &item.id == id) {
                        item.kind = NativeItemKind::Text { text: text.clone() };
                    }
                }
                NativeEdit::Remove { id } => stored.items.retain(|item| &item.id != id),
            }
        }
        let mut sessions = self.0.sessions.lock().unwrap();
        let new_id = format!("s{}", sessions.len() + 1);
        stored.revision = format!("rev-{new_id}");
        sessions.insert(new_id.clone(), stored);
        Ok(new_id)
    }
    async fn run(
        &self,
        request: RunRequest,
        mut controls: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        let agent = self.0.clone();
        agent.spawns.fetch_add(1, Ordering::SeqCst);
        agent.resumes.lock().unwrap().push(request.resume.clone());
        let session = request.resume.clone().unwrap_or_else(|| "s1".into());
        let (tx, rx) = mpsc::channel::<Result<AgentEvent, HarnessError>>(64);
        tokio::spawn(async move {
            let send = |event: AgentEvent| {
                let tx = tx.clone();
                async move { tx.send(Ok(event)).await.is_ok() }
            };
            let _ = send(AgentEvent::SessionStarted {
                harness: HarnessId::Mock,
                model: "native".into(),
                tools: vec![],
                cwd: request.cwd.clone(),
                session_id: session.clone(),
                assistant_message_id: "a0".into(),
            })
            .await;
            let _ = send(AgentEvent::TextDelta {
                text: format!("on it: {}", request.prompt),
            })
            .await;
            // A prompt containing `hold` stays in flight until interrupted.
            if !request.prompt.contains("hold")
                && !send(done(&session, DoneStatus::Completed)).await
            {
                return;
            }
            loop {
                tokio::select! {
                    _ = controls.interrupt.cancelled() => {
                        agent.kills.fetch_add(1, Ordering::SeqCst);
                        let _ = send(done(&session, DoneStatus::Interrupted)).await;
                        return;
                    }
                    steer = controls.steering.recv() => {
                        if steer.is_none() {
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
    agent: Arc<Agent>,
    client: zeron_rpc::RpcClient,
    _dir: tempfile::TempDir,
}

fn rig() -> Rig {
    let agent = Arc::new(Agent::default());
    let registry = HarnessRegistry::new();
    registry.register(Arc::new(Native(agent.clone())));
    let dir = tempfile::tempdir().unwrap();
    let core = EngineCore::assemble(dir.path(), Arc::new(registry), HarnessId::Mock, None)
        .expect("engine core assembles");
    core.workspace
        .create_chat(CHAT, None, Some(&core.device_id), None, Some("/tmp".into()))
        .unwrap();
    let client = zeron_rpc::memory_client(core.rpc_service());
    Rig {
        core,
        agent,
        client,
        _dir: dir,
    }
}

fn request(prompt: &str, resume: Option<&str>) -> RunRequest {
    RunRequest {
        mcp: None,
        env: Default::default(),
        prompt: prompt.into(),
        harness: Some(HarnessId::Mock),
        model: None,
        reasoning: None,
        model_options: Default::default(),
        cwd: "/tmp".into(),
        sandbox: SandboxLevel::WorkspaceWrite,
        auto_approve: false,
        attachments: Vec::new(),
        worktree: None,
        resume: resume.map(str::to_owned),
        system_prompt: None,
    }
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

impl Rig {
    async fn parked_after(&self, prompt: &str) {
        self.core
            .sessions
            .dispatch(CHAT, HarnessId::Mock, request(prompt, None), None)
            .await
            .unwrap();
        wait_for(
            || {
                self.core.sessions.session_status(CHAT).map(|s| s.status)
                    == Some(SessionStatus::Idle)
            },
            "the turn to park",
        )
        .await;
    }

    async fn get(&self) -> NativeHistory {
        self.client
            .call_as(
                methods::GET_NATIVE_HISTORY,
                serde_json::json!({ "chatId": CHAT }),
            )
            .await
            .unwrap()
    }

    async fn apply(
        &self,
        revision: &str,
        edits: serde_json::Value,
    ) -> Result<NativeHistory, String> {
        self.client
            .call_as(
                methods::APPLY_NATIVE_EDITS,
                serde_json::json!({ "chatId": CHAT, "revision": revision, "edits": edits }),
            )
            .await
            .map_err(|e| e.to_string())
    }

    async fn undo(&self) -> Result<NativeHistory, String> {
        self.client
            .call_as(
                methods::UNDO_NATIVE_EDIT,
                serde_json::json!({ "chatId": CHAT }),
            )
            .await
            .map_err(|e| e.to_string())
    }

    fn stored_session(&self) -> Option<String> {
        self.core
            .workspace
            .chat_harness_session(CHAT)
            .map(|stored| stored.session_id)
    }

    fn transcript(&self) -> Vec<(MessageRole, String)> {
        self.core
            .doc_host
            .open(CHAT)
            .unwrap()
            .doc()
            .read_entries()
            .unwrap()
            .into_iter()
            .map(|entry| {
                let text = entry
                    .parts
                    .iter()
                    .filter_map(|part| match part {
                        MessagePart::Text { text, .. } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<String>();
                (entry.role, text)
            })
            .collect()
    }
}

#[tokio::test]
async fn an_edit_repoints_the_chat_retires_the_runtime_and_undo_goes_back() {
    let rig = rig();
    rig.parked_after(PROMPT).await;
    assert_eq!(rig.stored_session().as_deref(), Some("s1"));

    let before = rig.get().await;
    assert_eq!(before.session_id, "s1");
    assert_eq!(before.harness, "mock");
    assert_eq!(before.revision, "rev-1");
    assert_eq!(before.items.len(), 3);
    assert!(!before.can_undo);

    let after = rig
        .apply(
            &before.revision,
            serde_json::json!([
                { "op": "replace", "id": "u0", "text": "Remember KIWI" },
                { "op": "replace", "id": "a0", "text": "on it: Remember KIWI" },
                { "op": "remove", "id": "r0" },
            ]),
        )
        .await
        .unwrap();
    assert_eq!(after.session_id, "s2");
    assert_eq!(after.items.len(), 2);
    assert!(after.can_undo);
    // The chat resumes the edited copy; the parked runtime on the old one is gone.
    assert_eq!(rig.stored_session().as_deref(), Some("s2"));
    assert_eq!(rig.agent.kills.load(Ordering::SeqCst), 1);
    assert!(!rig.core.sessions.has_run(CHAT));
    // The original was never touched.
    assert_eq!(rig.agent.session("s1").unwrap().items.len(), 3);
    // Zeron's own transcript says what the agent now believes.
    assert_eq!(
        rig.transcript(),
        [
            (MessageRole::User, "Remember KIWI".to_string()),
            (MessageRole::Assistant, "on it: Remember KIWI".to_string()),
        ]
    );

    // The next prompt relaunches on the new session.
    rig.core
        .sessions
        .dispatch(CHAT, HarnessId::Mock, request("what word?", None), None)
        .await
        .unwrap();
    wait_for(
        || rig.agent.spawns.load(Ordering::SeqCst) == 2,
        "the relaunch",
    )
    .await;
    assert_eq!(
        rig.agent.resumes.lock().unwrap().last().unwrap().as_deref(),
        Some("s2")
    );
    wait_for(
        || rig.core.sessions.session_status(CHAT).map(|s| s.status) == Some(SessionStatus::Idle),
        "the turn to park",
    )
    .await;

    // Undo points back at the session before the edit and restores the text.
    let undone = rig.undo().await.unwrap();
    assert_eq!(undone.session_id, "s1");
    assert_eq!(undone.items.len(), 3);
    assert!(!undone.can_undo);
    assert_eq!(rig.stored_session().as_deref(), Some("s1"));
    assert_eq!(rig.agent.kills.load(Ordering::SeqCst), 2);
    assert_eq!(rig.transcript()[0].1, "Remember PINEAPPLE");
    assert_eq!(rig.undo().await.unwrap_err(), "There is nothing to undo");
}

#[tokio::test]
async fn a_stale_revision_is_refused_and_changes_nothing() {
    let rig = rig();
    rig.parked_after(PROMPT).await;
    let err = rig
        .apply(
            "rev-0",
            serde_json::json!([{ "op": "replace", "id": "u0", "text": "x" }]),
        )
        .await
        .unwrap_err();
    assert_eq!(err, "The conversation changed since you started editing");
    assert_eq!(rig.stored_session().as_deref(), Some("s1"));
    assert!(!rig.get().await.can_undo);
    assert_eq!(rig.transcript()[0].1, PROMPT);
}

#[tokio::test]
async fn a_running_agent_must_be_stopped_first() {
    let rig = rig();
    rig.core
        .sessions
        .dispatch(CHAT, HarnessId::Mock, request("hold on", None), None)
        .await
        .unwrap();
    wait_for(
        || rig.core.sessions.turn_in_flight(CHAT) && rig.stored_session().is_some(),
        "the turn to run",
    )
    .await;
    let revision = rig.get().await.revision;
    let edits = serde_json::json!([{ "op": "remove", "id": "r0" }]);
    assert_eq!(
        rig.apply(&revision, edits.clone()).await.unwrap_err(),
        "Stop the agent before editing"
    );
    assert_eq!(
        rig.undo().await.unwrap_err(),
        "Stop the agent before editing"
    );
    // Nothing was retired or switched.
    assert_eq!(rig.agent.kills.load(Ordering::SeqCst), 0);
    assert_eq!(rig.stored_session().as_deref(), Some("s1"));
}

#[tokio::test]
async fn a_chat_hosted_elsewhere_is_refused() {
    let rig = rig();
    rig.core
        .workspace
        .create_chat(
            "elsewhere",
            None,
            Some("another-device"),
            None,
            Some("/tmp".into()),
        )
        .unwrap();
    let err = rig
        .client
        .call(
            methods::GET_NATIVE_HISTORY,
            serde_json::json!({ "chatId": "elsewhere" }),
        )
        .await
        .unwrap_err()
        .to_string();
    assert_eq!(err, "Edit this conversation on the device that runs it");
}
