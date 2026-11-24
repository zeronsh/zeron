//! A new chat's warm start (`SessionsEngine::warm_start`): its agent's
//! runtime started before the first message, taken by that chat's first
//! run, and stopped by every rule that ends it.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::stream::BoxStream;

use zeron_engine::{EngineCore, HarnessRegistry};
use zeron_harness::{Harness, HarnessError, RunControls};
use zeron_proto::{
    AccessMode, AgentEvent, DoneStatus, HarnessId, Model, ReasoningLevel, RunRequest, SandboxLevel,
    SteeringMode,
};

/// What the fixtures saw.
#[derive(Default)]
struct Seen {
    /// `(key, request)` of each warm launch.
    prewarms: Vec<(String, RunRequest)>,
    /// Keys of warm launches ended unused.
    discarded: Vec<String>,
    /// `(key, request)` of each run: the warm key it took, or "" (cold).
    runs: Vec<(String, RunRequest)>,
    /// Prompts a warm start was handed before its run (must stay empty).
    early_prompts: Vec<String>,
    /// Prompts titling runs were handed.
    titles: Vec<String>,
}

impl Seen {
    fn prompts(&self) -> Vec<(String, String)> {
        self.runs
            .iter()
            .map(|(key, r)| (key.clone(), r.prompt.clone()))
            .collect()
    }
}

struct WarmFixture {
    id: HarnessId,
    seen: Arc<Mutex<Seen>>,
    parked: Mutex<std::collections::HashMap<String, RunRequest>>,
    prewarms: bool,
    /// How long a launch takes.
    launch: Duration,
}

fn reply(prompt: &str) -> BoxStream<'static, Result<AgentEvent, HarnessError>> {
    use futures::StreamExt;
    futures::stream::iter(vec![
        Ok(AgentEvent::SessionStarted {
            harness: HarnessId::Mock,
            model: "mock-1".into(),
            tools: vec![],
            cwd: String::new(),
            session_id: "sess-warm".into(),
            assistant_message_id: format!("a-{prompt}"),
        }),
        Ok(AgentEvent::TextDelta {
            text: format!("re: {prompt}"),
        }),
        Ok(AgentEvent::Done {
            status: DoneStatus::Completed,
            result: None,
            error: None,
            session_id: Some("sess-warm".into()),
        }),
    ])
    .boxed()
}

/// What a launch bakes in (the prompt and attachments aside), as a real
/// driver compares it.
fn launches_alike(a: &RunRequest, b: &RunRequest) -> bool {
    a.model == b.model
        && a.reasoning == b.reasoning
        && a.model_options == b.model_options
        && a.cwd == b.cwd
        && a.access == b.access
        && a.plan == b.plan
        && a.auto_approve == b.auto_approve
        && a.mcp == b.mcp
        && a.env == b.env
        && a.resume == b.resume
}

#[async_trait]
impl Harness for WarmFixture {
    fn id(&self) -> HarnessId {
        self.id
    }
    fn display_name(&self) -> &str {
        "Warm fixture"
    }
    fn supports_steering(&self) -> bool {
        false
    }
    fn steering_mode(&self) -> SteeringMode {
        SteeringMode::TurnBoundary
    }
    fn reasoning_levels(&self) -> &[ReasoningLevel] {
        &[ReasoningLevel::Medium, ReasoningLevel::High]
    }
    async fn models(&self) -> Result<Vec<Model>, HarnessError> {
        Ok(vec![])
    }
    fn prewarms(&self) -> bool {
        self.prewarms
    }
    async fn prewarm(&self, key: &str, request: RunRequest) -> Result<(), HarnessError> {
        tokio::time::sleep(self.launch).await;
        let mut seen = self.seen.lock().unwrap();
        seen.prewarms.push((key.to_owned(), request.clone()));
        if !request.prompt.is_empty() {
            seen.early_prompts.push(request.prompt.clone());
        }
        self.parked.lock().unwrap().insert(key.to_owned(), request);
        Ok(())
    }
    async fn run_warm(
        &self,
        key: &str,
        request: RunRequest,
        controls: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        let parked = self.parked.lock().unwrap().remove(key);
        match parked {
            Some(warm) if launches_alike(&warm, &request) => {
                let prompt = request.prompt.clone();
                self.seen
                    .lock()
                    .unwrap()
                    .runs
                    .push((key.to_owned(), request));
                Ok(reply(&prompt))
            }
            Some(_) => {
                self.seen.lock().unwrap().discarded.push(key.to_owned());
                self.run(request, controls).await
            }
            None => self.run(request, controls).await,
        }
    }
    async fn discard_warm(&self, key: &str) {
        if self.parked.lock().unwrap().remove(key).is_some() {
            self.seen.lock().unwrap().discarded.push(key.to_owned());
        }
    }
    async fn run(
        &self,
        request: RunRequest,
        _controls: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        let prompt = request.prompt.clone();
        self.seen
            .lock()
            .unwrap()
            .runs
            .push((String::new(), request));
        Ok(reply(&prompt))
    }
    async fn run_title(
        &self,
        request: RunRequest,
        _controls: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        use futures::StreamExt;
        self.seen.lock().unwrap().titles.push(request.prompt);
        Ok(futures::stream::iter(vec![
            Ok(AgentEvent::TextDelta {
                text: "Warm Title".into(),
            }),
            Ok(AgentEvent::Done {
                status: DoneStatus::Completed,
                result: None,
                error: None,
                session_id: None,
            }),
        ])
        .boxed())
    }
}

fn ask(model: &str, cwd: &str) -> RunRequest {
    RunRequest {
        prompt: String::new(),
        harness: Some(HarnessId::Mock),
        model: Some(model.into()),
        reasoning: None,
        model_options: Default::default(),
        cwd: cwd.into(),
        sandbox: SandboxLevel::WorkspaceWrite,
        auto_approve: false,
        access: Default::default(),
        plan: false,
        code_mode: false,
        zeron_mcp_only: false,
        resume: None,
        attachments: vec![],
        worktree: None,
        mcp: None,
        extra_mcp: vec![],
        env: Default::default(),
    }
}

struct Rig {
    core: EngineCore,
    seen: Arc<Mutex<Seen>>,
    project: tempfile::TempDir,
    data: tempfile::TempDir,
}

struct Options {
    prewarms: bool,
    launch: Duration,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            prewarms: true,
            launch: Duration::ZERO,
        }
    }
}

fn rig(prewarms: bool) -> Rig {
    rig_with(Options {
        prewarms,
        ..Default::default()
    })
}

fn assemble(data: &std::path::Path, seen: &Arc<Mutex<Seen>>, options: &Options) -> EngineCore {
    let registry = HarnessRegistry::new();
    for id in [HarnessId::Mock, HarnessId::Pi, HarnessId::ClaudeCode, HarnessId::Codex] {
        registry.register(Arc::new(WarmFixture {
            id,
            seen: seen.clone(),
            parked: Mutex::default(),
            prewarms: options.prewarms,
            launch: options.launch,
        }));
    }
    let core = EngineCore::assemble(data, Arc::new(registry), HarnessId::Mock, None)
        .expect("engine core assembles");
    core.sessions.set_no_ipc_port();
    core
}

fn rig_with(options: Options) -> Rig {
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let seen = Arc::new(Mutex::new(Seen::default()));
    let core = assemble(data.path(), &seen, &options);
    Rig {
        core,
        seen,
        project,
        data,
    }
}

async fn wait_for(what: &str, mut done: impl FnMut() -> bool) {
    for _ in 0..300 {
        if done() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("never: {what}");
}

impl Rig {
    fn cwd(&self) -> String {
        self.project.path().to_string_lossy().into_owned()
    }

    async fn rpc(&self, method: &str, params: serde_json::Value) -> serde_json::Value {
        let client = zeron_rpc::memory_client(self.core.rpc_service());
        client.call(method, params).await.expect(method)
    }

    /// The app's WarmStart, as `chat.warm` sends it.
    async fn warm_with(&self, chat: &str, request: RunRequest, typed: bool) -> serde_json::Value {
        let mut request = serde_json::to_value(request).unwrap();
        if request["harness"].is_null() {
            request["harness"] = serde_json::to_value(HarnessId::Mock).unwrap();
        }
        self.rpc(
            "WarmStart",
            serde_json::json!({ "chatId": chat, "request": request, "typed": typed }),
        )
        .await
    }

    async fn warm(&self, chat: &str, model: &str, typed: bool) -> serde_json::Value {
        self.warm_with(chat, ask(model, &self.cwd()), typed).await
    }

    /// The app's Enter on a new chat: createChat with the id, then its run.
    async fn send_with(&self, chat: &str, request: RunRequest) {
        let device = self.core.device_id.clone();
        self.rpc(
            "Mutate",
            serde_json::json!({ "op": "createChat", "chatId": chat, "deviceId": device, "cwd": request.cwd }),
        )
        .await;
        self.queue(chat, request).await;
    }

    async fn queue(&self, chat: &str, request: RunRequest) {
        let prompt = request.prompt.clone();
        let request = serde_json::to_value(request).unwrap();
        self.rpc(
            "QueueCommand",
            serde_json::json!({ "chatId": chat, "command": { "kind": "run", "messageId": format!("m-{}", prompt.len()), "request": request } }),
        )
        .await;
        wait_for(&format!("the run of {prompt:?}"), || {
            self.seen
                .lock()
                .unwrap()
                .runs
                .iter()
                .any(|(_, r)| r.prompt == prompt)
        })
        .await;
    }

    async fn send(&self, chat: &str, model: &str, prompt: &str) {
        let mut request = ask(model, &self.cwd());
        request.prompt = prompt.into();
        self.send_with(chat, request).await;
    }

    fn warm_key(&self, n: usize) -> String {
        self.seen.lock().unwrap().prewarms[n].0.clone()
    }
}

const CHAT: &str = "0f3b2a1c-9d4e-4c5b-8a7f-123456789abc";
const OTHER: &str = "1f3b2a1c-9d4e-4c5b-8a7f-123456789abc";

/// The message typed goes, as typed, into the warm runtime, and nothing
/// reached that runtime before Enter.
#[tokio::test]
async fn enter_runs_in_the_warm_start_and_nothing_reached_it_before() {
    let rig = rig(true);
    let answer = rig.warm(CHAT, "mock-1", false).await;
    assert_eq!(answer["outcome"], "started");
    // Nothing of the chat exists yet: no row, no transcript.
    assert!(rig.core.workspace.chat(CHAT).unwrap().is_none());
    assert_eq!(rig.core.sessions.warm_chats(), vec![CHAT.to_string()]);
    let typed = "fix the \"parser\" — 日本語 ✓\nand the lexer";
    rig.send(CHAT, "mock-1", typed).await;
    let seen = rig.seen.lock().unwrap();
    assert_eq!(seen.prewarms.len(), 1);
    assert!(
        seen.prewarms[0].0.starts_with(CHAT),
        "keyed by the chat: {}",
        seen.prewarms[0].0
    );
    assert_eq!(
        seen.prompts(),
        vec![(seen.prewarms[0].0.clone(), typed.to_string())],
        "Enter took the warm start, with the message as typed"
    );
    assert!(seen.early_prompts.is_empty(), "no prompt before Enter");
    assert!(seen.discarded.is_empty());
    assert!(rig.core.sessions.warm_chats().is_empty());
}

#[tokio::test]
async fn the_same_ask_again_renews_it_and_never_starts_a_second() {
    let rig = rig(true);
    assert_eq!(rig.warm(CHAT, "mock-1", false).await["outcome"], "started");
    assert_eq!(rig.warm(CHAT, "mock-1", false).await["outcome"], "renewed");
    assert_eq!(rig.warm(CHAT, "mock-1", true).await["outcome"], "renewed");
    assert_eq!(rig.seen.lock().unwrap().prewarms.len(), 1);
}

/// Each launch setting the person may change after the warm start (model,
/// effort, folder, access, plan): another runtime starts in its place, the
/// old one ends, and the send takes the new one.
#[tokio::test]
async fn a_changed_config_starts_another_in_its_place() {
    type Change = fn(&mut RunRequest, &std::path::Path);
    let changes: [(&str, Change); 5] = [
        ("model", |r, _| r.model = Some("mock-2".into())),
        ("effort", |r, _| r.reasoning = Some(ReasoningLevel::High)),
        ("folder", |r, other| {
            r.cwd = other.to_string_lossy().into_owned()
        }),
        ("access", |r, _| r.access = AccessMode::Supervised),
        ("plan", |r, _| r.plan = true),
    ];
    for (what, change) in changes {
        let rig = rig(true);
        let other = tempfile::tempdir().unwrap();
        rig.warm(CHAT, "mock-1", false).await;
        let mut changed = ask("mock-1", &rig.cwd());
        change(&mut changed, other.path());
        assert_eq!(
            rig.warm_with(CHAT, changed.clone(), false).await["outcome"],
            "started",
            "{what}"
        );
        wait_for(&format!("{what}: the old one ended"), || {
            rig.seen.lock().unwrap().discarded.len() == 1
        })
        .await;
        let (first, second) = (rig.warm_key(0), rig.warm_key(1));
        assert_ne!(first, second, "{what}: one key per ask");
        assert_eq!(
            rig.seen.lock().unwrap().discarded,
            vec![first],
            "{what}: the old one ended"
        );
        changed.prompt = format!("with the new {what}");
        rig.send_with(CHAT, changed).await;
        assert_eq!(rig.seen.lock().unwrap().runs[0].0, second, "{what}: taken");
    }
}

/// Sent with another config than the warm start's (changed at the last
/// moment, nothing told the engine): it is never used stale.
#[tokio::test]
async fn sent_with_another_config_than_warmed_it_is_not_used() {
    let rig = rig(true);
    rig.warm(CHAT, "mock-1", false).await;
    rig.send(CHAT, "mock-2", "changed at the last moment").await;
    let seen = rig.seen.lock().unwrap();
    assert_eq!(seen.discarded, vec![seen.prewarms[0].0.clone()]);
    assert_eq!(seen.runs.len(), 1);
    assert_eq!(
        seen.runs[0].1.model.as_deref(),
        Some("mock-2"),
        "ran with what was sent"
    );
}

/// Sent with another agent than the warm start's: that one ends, and the
/// chosen agent starts its own.
#[tokio::test]
async fn sent_with_another_agent_ends_the_warm_start() {
    let rig = rig(true);
    rig.warm(CHAT, "mock-1", false).await;
    let mut request = ask("pi-1", &rig.cwd());
    request.harness = Some(HarnessId::Pi);
    request.prompt = "on another agent".into();
    rig.send_with(CHAT, request).await;
    let seen = rig.seen.lock().unwrap();
    assert_eq!(
        seen.discarded,
        vec![seen.prewarms[0].0.clone()],
        "the warm one ended"
    );
    assert_eq!(seen.runs[0].0, "", "a run of its own");
    assert_eq!(seen.runs[0].1.harness, Some(HarnessId::Pi));
}

/// Enter before the warm start finished launching: the run waits for it and
/// takes it, rather than starting a second agent beside it.
#[tokio::test]
async fn sending_before_it_is_ready_takes_it_once_launched() {
    let rig = Arc::new(rig_with(Options {
        launch: Duration::from_millis(600),
        ..Default::default()
    }));
    let warming = {
        let rig = rig.clone();
        tokio::spawn(async move { rig.warm(CHAT, "mock-1", false).await })
    };
    wait_for("the ask registered", || {
        !rig.core.sessions.warm_chats().is_empty()
    })
    .await;
    assert!(
        rig.seen.lock().unwrap().prewarms.is_empty(),
        "still launching"
    );
    rig.send(CHAT, "mock-1", "right away").await;
    assert_eq!(warming.await.unwrap()["outcome"], "started");
    let seen = rig.seen.lock().unwrap();
    assert_eq!(seen.prewarms.len(), 1);
    assert_eq!(
        seen.prompts(),
        vec![(seen.prewarms[0].0.clone(), "right away".to_string())]
    );
    assert!(seen.discarded.is_empty());
}

/// A warm start asked after its chat's run started (the send went first):
/// nothing starts, and nothing is left running.
#[tokio::test]
async fn an_ask_after_the_send_starts_nothing() {
    let rig = rig(true);
    rig.send(CHAT, "mock-1", "sent first").await;
    assert_eq!(
        rig.warm(CHAT, "mock-1", true).await["outcome"],
        "unsupported"
    );
    assert!(rig.seen.lock().unwrap().prewarms.is_empty());
    assert!(rig.core.sessions.warm_chats().is_empty());
}

/// Only a new chat: an existing chat (stopped or not) is never warmed.
#[tokio::test]
async fn an_existing_chat_is_never_warmed() {
    let rig = rig(true);
    rig.send(CHAT, "mock-1", "first").await;
    wait_for("its turn ended", || !rig.core.sessions.turn_in_flight(CHAT)).await;
    rig.core.sessions.interrupt(CHAT).await.ok();
    assert_eq!(
        rig.warm(CHAT, "mock-1", true).await["outcome"],
        "unsupported"
    );
    // (A row made without a run, too.)
    rig.rpc(
        "Mutate",
        serde_json::json!({ "op": "createChat", "chatId": OTHER, "deviceId": rig.core.device_id, "cwd": rig.cwd() }),
    )
    .await;
    assert_eq!(
        rig.warm(OTHER, "mock-1", true).await["outcome"],
        "unsupported"
    );
    assert!(rig.seen.lock().unwrap().prewarms.is_empty());
}

/// A new chat whose run makes a worktree as it's sent has no folder to start
/// in early.
#[tokio::test]
async fn a_new_worktree_starts_nothing_early() {
    let rig = rig(true);
    let mut request = serde_json::to_value(ask("mock-1", &rig.cwd())).unwrap();
    request["worktree"] = serde_json::json!({ "repoPath": rig.cwd(), "base": "HEAD" });
    request["harness"] = serde_json::to_value(HarnessId::Mock).unwrap();
    let answer = rig
        .rpc(
            "WarmStart",
            serde_json::json!({ "chatId": CHAT, "request": request, "typed": false }),
        )
        .await;
    assert_eq!(answer["outcome"], "unsupported");
    assert!(rig.seen.lock().unwrap().prewarms.is_empty());
}

#[tokio::test]
async fn a_stop_ends_it() {
    let rig = rig(true);
    rig.warm(CHAT, "mock-1", false).await;
    // (The app's WarmStop: the draft left empty, the window closed.)
    rig.rpc("WarmStop", serde_json::json!({ "chatId": CHAT }))
        .await;
    assert!(rig.core.sessions.warm_chats().is_empty());
    let key = rig.warm_key(0);
    assert_eq!(rig.seen.lock().unwrap().discarded, vec![key]);
    // And a run afterwards starts cold.
    rig.send(CHAT, "mock-1", "later").await;
    assert_eq!(rig.seen.lock().unwrap().runs[0].0, "");
}

/// A stop while it launches: the process it was making ends as it lands.
#[tokio::test]
async fn a_stop_during_its_launch_ends_what_it_made() {
    let rig = Arc::new(rig_with(Options {
        launch: Duration::from_millis(400),
        ..Default::default()
    }));
    let warming = {
        let rig = rig.clone();
        tokio::spawn(async move { rig.warm(CHAT, "mock-1", false).await })
    };
    wait_for("the ask registered", || {
        !rig.core.sessions.warm_chats().is_empty()
    })
    .await;
    rig.rpc("WarmStop", serde_json::json!({ "chatId": CHAT }))
        .await;
    assert_eq!(warming.await.unwrap()["outcome"], "unsupported");
    let seen = rig.seen.lock().unwrap();
    assert_eq!(seen.prewarms.len(), 1);
    assert_eq!(seen.discarded, vec![seen.prewarms[0].0.clone()]);
}

#[tokio::test]
async fn the_app_quitting_ends_every_one() {
    let rig = rig(true);
    rig.warm(CHAT, "mock-1", false).await;
    rig.warm(OTHER, "mock-1", false).await;
    rig.core.sessions.warm_stop_all().await;
    assert!(rig.core.sessions.warm_chats().is_empty());
    assert_eq!(rig.seen.lock().unwrap().discarded.len(), 2);
}

/// The engine restarting mid-warm: its shutdown ends the warm start, and the
/// chat's send to the next engine starts its agent as it always did.
#[tokio::test]
async fn an_engine_restart_mid_warm_leaves_nothing_and_the_send_still_runs() {
    let options = Options::default();
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let seen = Arc::new(Mutex::new(Seen::default()));
    let first = assemble(data.path(), &seen, &options);
    let rig = Rig {
        core: first,
        seen: seen.clone(),
        project,
        data,
    };
    rig.warm(CHAT, "mock-1", true).await;
    rig.core.shutdown().await;
    let key = rig.warm_key(0);
    assert_eq!(
        seen.lock().unwrap().discarded,
        vec![key],
        "ended with its engine"
    );
    let Rig {
        core,
        project,
        data,
        ..
    } = rig;
    drop(core);
    let rig = Rig {
        core: assemble(data.path(), &seen, &options),
        seen: seen.clone(),
        project,
        data,
    };
    assert!(rig.core.sessions.warm_chats().is_empty());
    rig.send(CHAT, "mock-1", "after the restart").await;
    let seen = seen.lock().unwrap();
    assert_eq!(
        seen.prompts(),
        vec![(String::new(), "after the restart".to_string())]
    );
}

/// Opened and never typed into: gone once the open lease runs out. Typed
/// into: kept as long as keystrokes keep coming (the composer's renewals),
/// past both windows; then gone once they stop.
#[tokio::test]
async fn one_being_written_is_never_stopped_and_one_abandoned_always_is() {
    let rig = rig(true);
    rig.core
        .sessions
        .set_warm_leases(Duration::from_millis(300), Duration::from_millis(500));
    rig.warm(CHAT, "mock-1", false).await;
    rig.warm(OTHER, "mock-1", true).await;
    // Typing into OTHER for 1.5 s: renewed every 150 ms.
    for _ in 0..10 {
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert_eq!(rig.warm(OTHER, "mock-1", true).await["outcome"], "renewed");
    }
    assert_eq!(
        rig.core.sessions.warm_chats(),
        vec![OTHER.to_string()],
        "the empty open stopped; the one being written stays"
    );
    assert_eq!(
        rig.seen.lock().unwrap().prewarms.len(),
        2,
        "never restarted"
    );
    // Left alone: gone once its lease runs out.
    wait_for("the abandoned one stopped", || {
        rig.core.sessions.warm_chats().is_empty()
    })
    .await;
    assert_eq!(rig.seen.lock().unwrap().discarded.len(), 2);
}

#[tokio::test]
async fn a_harness_that_cannot_park_is_not_warmed() {
    let cold = rig(false);
    assert_eq!(
        cold.warm(CHAT, "mock-1", false).await["outcome"],
        "unsupported"
    );
    assert!(cold.seen.lock().unwrap().prewarms.is_empty());
}

/// A warm start is no work in progress: quitting with "keep running" never
/// waits on it.
#[tokio::test]
async fn a_warm_start_is_never_work() {
    let rig = rig(true);
    rig.warm(CHAT, "mock-1", false).await;
    assert!(rig.core.sessions.chats_with_work().is_empty());
}

/// Its CLI being updated: no warm start, and one made before an update was
/// accepted is not used by the run (which waits behind the update).
#[tokio::test]
async fn a_cli_update_refuses_and_retires_warm_starts() {
    let rig = rig(true);
    rig.core.registry.begin_update(HarnessId::Mock);
    assert_eq!(
        rig.warm(CHAT, "mock-1", false).await["outcome"],
        "unsupported"
    );
    rig.core.registry.end_update(HarnessId::Mock);
    assert_eq!(rig.warm(CHAT, "mock-1", false).await["outcome"], "started");
    rig.core.registry.begin_update(HarnessId::Mock);
    let device = rig.core.device_id.clone();
    rig.rpc(
        "Mutate",
        serde_json::json!({ "op": "createChat", "chatId": CHAT, "deviceId": device, "cwd": rig.cwd() }),
    )
    .await;
    let mut request = serde_json::to_value(ask("mock-1", &rig.cwd())).unwrap();
    request["prompt"] = "behind the update".into();
    rig.rpc(
        "QueueCommand",
        serde_json::json!({ "chatId": CHAT, "command": { "kind": "run", "messageId": "m-u", "request": request } }),
    )
    .await;
    wait_for("the warm one ended", || {
        rig.seen.lock().unwrap().discarded.len() == 1
    })
    .await;
    assert!(
        rig.seen.lock().unwrap().runs.is_empty(),
        "the run waits for the update"
    );
    rig.core.registry.end_update(HarnessId::Mock);
    wait_for("the run after the update", || {
        !rig.seen.lock().unwrap().runs.is_empty()
    })
    .await;
    assert_eq!(rig.seen.lock().unwrap().runs[0].0, "", "its own runtime");
}

/// The account a run would use changed after the warm start (an account
/// switch): the warm runtime, started on the old one, is not used.
#[tokio::test]
async fn a_changed_credential_is_never_run_on_the_old_one() {
    struct Switchable(Mutex<String>);
    #[async_trait]
    impl zeron_engine::registry::RunEnvironment for Switchable {
        async fn prepare(
            &self,
            _: HarnessId,
            _: Option<&str>,
            request: &mut RunRequest,
        ) -> Result<(), String> {
            request.env.insert("ACCOUNT".into(), "same-env".into());
            Ok(())
        }
        async fn credential(&self, _: HarnessId, _: Option<&str>) -> Option<String> {
            Some(self.0.lock().unwrap().clone())
        }
    }
    let rig = rig(true);
    let account = Arc::new(Switchable(Mutex::new("account-a".into())));
    rig.core.registry.set_run_environment(account.clone());
    rig.warm(CHAT, "mock-1", true).await;
    *account.0.lock().unwrap() = "account-b".into();
    rig.send(CHAT, "mock-1", "on account b").await;
    let seen = rig.seen.lock().unwrap();
    assert_eq!(seen.discarded, vec![seen.prewarms[0].0.clone()]);
    assert_eq!(seen.runs[0].0, "", "a runtime of its own");
}

/// The first message's attachments ride the run that takes the warm start.
#[tokio::test]
async fn the_first_message_s_attachments_reach_the_warm_runtime() {
    let rig = rig(true);
    rig.warm(CHAT, "mock-1", true).await;
    let file = rig.project.path().join("shot.png");
    std::fs::write(&file, b"png").unwrap();
    let mut request = ask("mock-1", &rig.cwd());
    request.prompt = format!("look at @{}", file.display());
    request.attachments = vec![file.to_string_lossy().into_owned()];
    rig.send_with(CHAT, request).await;
    let seen = rig.seen.lock().unwrap();
    assert_eq!(seen.runs[0].0, seen.prewarms[0].0, "taken");
    assert_eq!(
        seen.runs[0].1.attachments,
        vec![file.to_string_lossy().into_owned()]
    );
    assert!(
        seen.prewarms[0].1.attachments.is_empty(),
        "none before Enter"
    );
}

/// The chat is titled from its first message, as any new chat is; and its
/// next message (queued behind the first turn) goes as usual.
#[tokio::test]
async fn a_warm_started_chat_is_titled_and_its_next_message_goes() {
    let rig = rig(true);
    rig.warm(CHAT, "mock-1", true).await;
    rig.send(CHAT, "mock-1", "please fix the login flow").await;
    rig.queue(CHAT, {
        let mut r = ask("mock-1", &rig.cwd());
        r.prompt = "and then the logout".into();
        r
    })
    .await;
    wait_for("the title", || {
        rig.core
            .workspace
            .chat(CHAT)
            .ok()
            .flatten()
            .and_then(|c| c.title)
            .is_some_and(|t| t == "Warm Title")
    })
    .await;
    let seen = rig.seen.lock().unwrap();
    assert!(
        seen.titles
            .iter()
            .any(|t| t.contains("please fix the login flow")),
        "{:?}",
        seen.titles
    );
    let prompts: Vec<String> = seen.runs.iter().map(|(_, r)| r.prompt.clone()).collect();
    assert_eq!(
        prompts,
        vec![
            "please fix the login flow".to_string(),
            "and then the logout".to_string()
        ]
    );
    assert_eq!(seen.prewarms.len(), 1, "no second warm start");
}

/// At most a few at once on one engine, whatever a client asks.
#[tokio::test]
async fn warm_starts_are_bounded() {
    let rig = rig(true);
    let ids: Vec<String> = (0..6)
        .map(|i| format!("{i}f3b2a1c-9d4e-4c5b-8a7f-123456789abc"))
        .collect();
    let mut started = 0;
    for id in &ids {
        if rig.warm(id, "mock-1", false).await["outcome"] == "started" {
            started += 1;
        }
    }
    assert_eq!(started, 4);
    assert_eq!(rig.core.sessions.warm_chats().len(), 4);
}

/// Claude's native subagents switched off after the warm start (side chats
/// only from then on): the warm runtime, launched with them, is not used.
#[tokio::test]
async fn a_switch_of_native_subagents_is_never_run_on_the_old_setting() {
    let rig = rig(true);
    let mut ask_claude = ask("sonnet", &rig.cwd());
    ask_claude.harness = Some(HarnessId::ClaudeCode);
    assert_eq!(rig.warm_with(CHAT, ask_claude.clone(), true).await["outcome"], "started");
    rig.core
        .registry
        .set_native_subagents(HarnessId::ClaudeCode, false)
        .unwrap();
    ask_claude.prompt = "after the switch".into();
    rig.send_with(CHAT, ask_claude).await;
    let seen = rig.seen.lock().unwrap();
    assert_eq!(seen.discarded, vec![seen.prewarms[0].0.clone()]);
    assert_eq!(seen.runs[0].0, "", "a runtime of its own");
}

/// Codex's Code Mode switched on after the warm start: the warm runtime,
/// launched with it off, is discarded and the run starts with Code Mode on.
#[tokio::test]
async fn a_switch_of_code_mode_is_never_run_on_the_old_setting() {
    let rig = rig(true);
    let mut ask_codex = ask("gpt-5.4", &rig.cwd());
    ask_codex.harness = Some(HarnessId::Codex);
    assert_eq!(rig.warm_with(CHAT, ask_codex.clone(), true).await["outcome"], "started");
    rig.core
        .registry
        .set_code_mode(HarnessId::Codex, true)
        .unwrap();
    ask_codex.prompt = "after enabling code mode".into();
    rig.send_with(CHAT, ask_codex).await;
    let seen = rig.seen.lock().unwrap();
    assert_eq!(seen.discarded, vec![seen.prewarms[0].0.clone()]);
    assert_eq!(seen.runs[0].0, "", "a runtime of its own");
    assert!(seen.runs[0].1.code_mode, "run receives code_mode=true");
}
