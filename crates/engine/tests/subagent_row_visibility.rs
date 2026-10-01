//! A chat's subagent count must reach every device's session row and STAY
//! there for as long as subagents run -- including when the parent is parked
//! idle and the children are quiet -- and must follow count changes.

#[path = "../../client/tests/support/mock_edge.rs"]
mod mock_edge;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::{StreamExt, stream::BoxStream};
use mock_edge::MockEdge;
use zeron_client::events::NullListener;
use zeron_client::{Client, ClientConfig, Credentials};
use zeron_engine::{EngineCore, HarnessRegistry};
use zeron_harness::{Harness, HarnessError, RunControls};
use zeron_proto::{
    AgentEvent, ChatConfig, DoneStatus, HarnessId, Model, ReasoningLevel, RunRequest, SandboxLevel,
    SteeringMode, ToolCall,
};

const CHAT: &str = "subagent-visibility";

/// Harness whose stream is fed by the test, event by event.
#[derive(Clone, Default)]
struct Feed(Arc<Mutex<Option<tokio::sync::mpsc::Sender<AgentEvent>>>>);

impl Feed {
    async fn send(&self, event: AgentEvent) {
        let tx = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if let Some(tx) = self.0.lock().unwrap().clone() {
                    return tx;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("run started");
        tx.send(event).await.expect("run alive");
    }
}

struct FedHarness(Feed);

#[async_trait]
impl Harness for FedHarness {
    fn id(&self) -> HarnessId {
        HarnessId::Mock
    }
    fn display_name(&self) -> &str {
        "Fed"
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
    async fn models(&self) -> Result<Vec<Model>, HarnessError> {
        Ok(vec![])
    }
    async fn run(
        &self,
        _request: RunRequest,
        _controls: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        let (tx, rx) = tokio::sync::mpsc::channel(32);
        *self.0.0.lock().unwrap() = Some(tx);
        Ok(futures::stream::unfold(rx, |mut rx| async move {
            rx.recv().await.map(|event| (Ok(event), rx))
        })
        .boxed())
    }
}

fn spawn(id: &str) -> AgentEvent {
    AgentEvent::ToolCall {
        id: id.into(),
        call: ToolCall::Unknown {
            name: "Agent: probe".into(),
            input: None,
        },
    }
}

fn child(id: &str, event: AgentEvent) -> AgentEvent {
    AgentEvent::Subagent {
        parent_tool_use_id: id.into(),
        event: Box::new(event),
    }
}

fn chatter(id: &str) -> AgentEvent {
    child(
        id,
        AgentEvent::TextDelta {
            text: "working".into(),
        },
    )
}

fn child_done(id: &str) -> AgentEvent {
    child(
        id,
        AgentEvent::Done {
            status: DoneStatus::Completed,
            result: None,
            error: None,
            session_id: None,
        },
    )
}

fn parent_done() -> AgentEvent {
    AgentEvent::Done {
        status: DoneStatus::Completed,
        result: None,
        error: None,
        session_id: Some("s".into()),
    }
}

fn run_request() -> RunRequest {
    RunRequest {
        mcp: None,
        prompt: "fan out".into(),
        harness: Some(HarnessId::Mock),
        model: None,
        reasoning: None,
        model_options: Default::default(),
        cwd: "/tmp".into(),
        sandbox: SandboxLevel::WorkspaceWrite,
        auto_approve: false,
        attachments: vec![],
        resume: None,
        worktree: None,
    }
}

async fn wait_for(what: &str, mut predicate: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(15), async {
        while !predicate() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
}

struct Rig {
    edge: MockEdge,
    core: EngineCore,
    feed: Feed,
    _host_dir: tempfile::TempDir,
    _phone_dir: tempfile::TempDir,
}

impl Rig {
    async fn start() -> Self {
        let edge = MockEdge::start().await;
        let host_dir = tempfile::tempdir().unwrap();
        let feed = Feed::default();
        let registry = HarnessRegistry::new();
        registry.register(Arc::new(FedHarness(feed.clone())));
        let core = EngineCore::assemble_with_identity(
            host_dir.path(),
            Arc::new(registry),
            HarnessId::Mock,
            None,
            "org-1",
            "user-1",
        )
        .unwrap();
        core.workspace.connect_registry_url(&edge.registry.url());
        core.workspace
            .create_chat(
                CHAT,
                None,
                Some(&core.device_id),
                Some(ChatConfig {
                    harness: HarnessId::Mock,
                    model: None,
                    reasoning: None,
                    model_options: Default::default(),
                    sandbox: SandboxLevel::WorkspaceWrite,
                }),
                Some("/tmp".into()),
            )
            .unwrap();
        Self {
            edge,
            core,
            feed,
            _host_dir: host_dir,
            _phone_dir: tempfile::tempdir().unwrap(),
        }
    }

    async fn dispatch(&self) {
        self.core
            .sessions
            .dispatch(CHAT, HarnessId::Mock, run_request(), Some("m-1".into()))
            .await
            .unwrap();
    }

    /// A phone that starts cold: no local replica of the registry.
    fn phone(&self) -> Client {
        let dir = self._phone_dir.path().join(format!(
            "phone-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let mut config = ClientConfig::new(self.edge.edge_url(), &dir);
        config.device_id = "android-viewer".into();
        config.platform = "android".into();
        Client::new(
            config,
            Credentials::Dev {
                user_id: "user-1".into(),
                org_id: "org-1".into(),
            },
            Arc::new(NullListener),
        )
        .unwrap()
    }
}

/// What the phone's Sessions list would draw for the chat right now.
fn seen(phone: &Client) -> (String, u32) {
    phone
        .workspace()
        .session(CHAT)
        .map_or(("none".into(), 0), |s| {
            (format!("{:?}", s.indicator), s.running_subagents)
        })
}

async fn phone_sees(phone: &Client, what: &str, indicator: &str, count: u32) {
    let want = (indicator.to_string(), count);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        let got = seen(phone);
        if got == want {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "phone never saw {what}: want {want:?}, last saw {got:?}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn main_text(text: &str) -> AgentEvent {
    AgentEvent::TextDelta { text: text.into() }
}

/// One stops and another launches; a count that dips to zero and returns.
/// Every state the phone's list can land on is the true one.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn count_follows_children_through_wake_turns_and_respawns() {
    let rig = Rig::start().await;
    let phone = rig.phone();
    rig.dispatch().await;
    for ev in [
        spawn("a"),
        chatter("a"),
        spawn("b"),
        chatter("b"),
        parent_done(),
    ] {
        rig.feed.send(ev).await;
    }
    phone_sees(&phone, "two children, main parked", "Completed", 2).await;
    // one stops...
    rig.feed.send(child_done("a")).await;
    phone_sees(&phone, "one child left", "Completed", 1).await;
    // ...another launches via a wake turn
    rig.feed.send(main_text("woke")).await;
    rig.feed.send(spawn("c")).await;
    rig.feed.send(chatter("c")).await;
    phone_sees(&phone, "wake turn with two", "Working", 2).await;
    rig.feed.send(parent_done()).await;
    phone_sees(&phone, "parked again with two", "Completed", 2).await;
    rig.feed.send(child_done("b")).await;
    rig.feed.send(child_done("c")).await;
    phone_sees(&phone, "all done", "Completed", 0).await;
    // 1 -> 0 -> 1
    rig.feed.send(main_text("again")).await;
    rig.feed.send(spawn("d")).await;
    rig.feed.send(chatter("d")).await;
    rig.feed.send(parent_done()).await;
    phone_sees(&phone, "new child alone", "Completed", 1).await;
    // A settled child is steered and runs again.
    rig.feed.send(child_done("d")).await;
    phone_sees(&phone, "d settled", "Completed", 0).await;
    rig.feed
        .send(child(
            "d",
            AgentEvent::Steered {
                assistant_message_id: None,
                next_assistant_message_id: None,
            },
        ))
        .await;
    phone_sees(&phone, "d reopened by a steer", "Completed", 1).await;
    phone.shutdown();
    rig.core.shutdown().await;
}

/// Children can be the only traffic for minutes. The session row keeps being
/// refreshed on the host's heartbeat so the phone's 45s staleness gate never
/// trips, and the cold-started phone keeps the badge. (Real time: the
/// heartbeat is 15s and the throttle reads the wall clock.)
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn quiet_children_keep_the_row_fresh_past_the_staleness_window() {
    let rig = Rig::start().await;
    rig.dispatch().await;
    for ev in [
        spawn("a"),
        spawn("b"),
        chatter("a"),
        chatter("b"),
        parent_done(),
    ] {
        rig.feed.send(ev).await;
    }
    wait_for("host counts two", || {
        rig.core
            .sessions
            .session_status(CHAT)
            .is_some_and(|s| s.running_subagents == 2)
    })
    .await;
    // Cold start while they run.
    let phone = rig.phone();
    phone_sees(&phone, "two children, cold", "Completed", 2).await;
    let started = std::time::Instant::now();
    let mut beats = std::collections::BTreeSet::new();
    let mut worst_age = 0;
    while started.elapsed() < Duration::from_secs(50) {
        let host = rig.core.sessions.session_status(CHAT).unwrap();
        beats.insert(host.updated_at.timestamp_millis());
        let age = (chrono::Utc::now() - host.updated_at).num_milliseconds();
        worst_age = worst_age.max(age);
        assert!(
            age < zeron_proto::view::SESSION_STALE_MS / 2,
            "row went {age}ms without a refresh while two children ran"
        );
        assert_eq!(
            seen(&phone),
            ("Completed".to_string(), 2),
            "the phone's badge dropped {}s in (worst row age so far {worst_age}ms)",
            started.elapsed().as_secs()
        );
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    assert!(beats.len() >= 3, "heartbeats seen: {}", beats.len());
    phone.shutdown();
    rig.core.shutdown().await;
}
