//! What the sidebar registry writes per streaming turn and per parked hour,
//! measured through a real RegistryRoom on `wrangler dev`: a host engine runs
//! a steerable turn that streams for a while and then parks (as Claude and
//! Codex do), a second engine of the same user watches, and a raw socket
//! records every row the room broadcasts.
//!
//! ```sh
//! (cd edge && npx wrangler dev --port 27640 --var AUTH_MODE:dev)
//! ZERON_EDGE_WS=ws://127.0.0.1:27640 rtk proxy cargo test -p zeron-engine \
//!   --test registry_writes_live -- --ignored --nocapture
//! ```
//!
//! `REGISTRY_TURN_SECS` (default 60) sets how long the turn streams and
//! `REGISTRY_PARKED_SECS` (default 120) how long the parked session is
//! watched afterwards.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::stream::BoxStream;
use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as WsMessage;

use zeron_doc::{CommandBasedOn, SessionCommandEntry, SessionCommandPayload, SessionCommandStatus};
use zeron_engine::doc_host::EdgeConfig;
use zeron_engine::{EngineCore, HarnessRegistry};
use zeron_harness::{Harness, HarnessError, RunControls};
use zeron_proto::{
    AgentEvent, DoneStatus, HarnessId, Model, ReasoningLevel, RunRequest, SandboxLevel,
    SessionStatus, SteeringMode,
};
use zeron_rpc::methods;

const HOST: &str = "reg-host";
const VIEWER: &str = "reg-viewer";
const CHAT: &str = "chat-registry-writes";
const PROMPT: &str = "stream for a while";

/// A steerable harness the test feeds by hand: after Done its stream stays
/// open, so the session parks the way a real persistent session does.
struct FeedHarness {
    feed: tokio::sync::Mutex<Option<mpsc::UnboundedReceiver<AgentEvent>>>,
}

#[async_trait]
impl Harness for FeedHarness {
    fn id(&self) -> HarnessId {
        HarnessId::Mock
    }
    fn display_name(&self) -> &str {
        "Feed"
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
    async fn models(&self) -> Result<Vec<Model>, HarnessError> {
        Ok(vec![])
    }
    async fn run(
        &self,
        request: RunRequest,
        controls: RunControls,
    ) -> Result<BoxStream<'static, Result<AgentEvent, HarnessError>>, HarnessError> {
        let feed = if request.prompt == PROMPT {
            self.feed.lock().await.take()
        } else {
            None
        };
        let Some(mut feed) = feed else {
            let done = AgentEvent::Done {
                status: DoneStatus::Completed,
                result: None,
                error: None,
                session_id: None,
            };
            return Ok(futures::stream::iter(vec![Ok(done)]).boxed());
        };
        let (tx, rx) = mpsc::channel::<Result<AgentEvent, HarnessError>>(64);
        let cancel = controls.interrupt.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => return,
                    event = feed.recv() => match event {
                        Some(event) => if tx.send(Ok(event)).await.is_err() { return },
                        None => return,
                    },
                }
            }
        });
        Ok(futures::stream::unfold(rx, |mut rx| async move {
            rx.recv().await.map(|event| (event, rx))
        })
        .boxed())
    }
}

fn run_request() -> RunRequest {
    RunRequest {
        mcp: None,
        prompt: PROMPT.into(),
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

async fn wait_for(mut predicate: impl FnMut() -> bool, what: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while !predicate() {
        assert!(tokio::time::Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn secs(var: &str, default: u64) -> u64 {
    std::env::var(var).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

/// What the observer socket saw: broadcast batches, rows per kind, and
/// which fields each row's write actually moved (by clock).
#[derive(Default, Clone)]
struct Tally {
    batches: u64,
    rows: BTreeMap<String, u64>,
    fields: BTreeMap<String, u64>,
}

#[derive(Default)]
struct Observed {
    known: HashMap<(String, String), Value>,
    tally: Tally,
    /// When a reader last received a live (Working/AwaitingInput) session
    /// row, and the longest it then went without a newer one: what a
    /// shipped reader's 45 s staleness gate sees.
    live_since: Option<std::time::Instant>,
    longest_live_gap: Duration,
}

impl Observed {
    fn note(&mut self, row: &Value, count: bool) {
        let kind = row["kind"].as_str().unwrap_or("").to_string();
        let id = row["id"].as_str().unwrap_or("").to_string();
        if count && kind == "sessions" {
            let now = std::time::Instant::now();
            if let Some(since) = self.live_since {
                self.longest_live_gap = self.longest_live_gap.max(now - since);
            }
            let live = matches!(row["fields"]["status"].as_str(), Some("working" | "awaitingInput"));
            self.live_since = live.then_some(now);
        }
        let clocks = row["clocks"].clone();
        if count {
            *self.tally.rows.entry(kind.clone()).or_default() += 1;
            let before = self.known.get(&(kind.clone(), id.clone()));
            if let Some(map) = clocks.as_object() {
                for (field, clock) in map {
                    if before.and_then(|b| b.get(field)) != Some(clock) {
                        *self.tally.fields.entry(format!("{kind}.{field}")).or_default() += 1;
                    }
                }
            }
        }
        self.known.insert((kind, id), clocks);
    }
}

async fn observe(url: String, observed: Arc<Mutex<Observed>>) {
    let (mut socket, _) = tokio_tungstenite::connect_async(url).await.expect("observer connects");
    socket
        .send(WsMessage::Text(json!({"t": "hello", "cursor": null, "device": "observer"}).to_string().into()))
        .await
        .expect("observer hello");
    while let Some(Ok(message)) = socket.next().await {
        let WsMessage::Text(text) = message else { continue };
        let Ok(frame) = serde_json::from_str::<Value>(&text) else { continue };
        let mut observed = observed.lock().unwrap();
        match frame["t"].as_str() {
            Some("state") => {
                for row in frame["rows"].as_array().into_iter().flatten() {
                    observed.note(row, false);
                }
            }
            Some("rows") => {
                observed.tally.batches += 1;
                for row in frame["rows"].as_array().into_iter().flatten() {
                    observed.note(row, true);
                }
            }
            _ => {}
        }
    }
}

/// The room's head: the batches it applied. (Its other /stats counters live
/// in memory and reset whenever the idle object is evicted, which `wrangler
/// dev` does within seconds.)
async fn seq(http: &str, org: &str, token: &str) -> u64 {
    let body: Value = reqwest::Client::new()
        .get(format!("{http}/registry/{org}/stats"))
        .bearer_auth(token)
        .send()
        .await
        .expect("stats")
        .json()
        .await
        .expect("stats json");
    body["seq"].as_u64().unwrap_or(0)
}

/// Rows moved, by kind, and which fields moved. Each row moved costs the room
/// two billed rows (the row and its seq index entry); before the room stopped
/// storing per-push bookkeeping, a one-row push cost five.
fn report(phase: &str, seconds: u64, from: u64, to: u64, before: &Tally, after: &Tally) {
    let diff = |a: &BTreeMap<String, u64>, b: &BTreeMap<String, u64>| -> BTreeMap<String, u64> {
        b.iter()
            .map(|(k, n)| (k.clone(), n.saturating_sub(a.get(k).copied().unwrap_or(0))))
            .filter(|(_, n)| *n > 0)
            .collect()
    };
    let rows = diff(&before.rows, &after.rows);
    println!("── {phase} ({seconds} s)");
    println!("   batches applied (seq): {}", to - from);
    println!("   rows moved by kind: {rows:?} = {} rows", rows.values().sum::<u64>());
    println!("   field writes: {:?}", diff(&before.fields, &after.fields));
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires a live edge: set ZERON_EDGE_WS (e.g. ws://127.0.0.1:27640)"]
async fn registry_writes_per_turn_and_parked_minute() {
    let base = std::env::var("ZERON_EDGE_WS").expect("set ZERON_EDGE_WS");
    let http = base.replacen("ws", "http", 1);
    let org = format!("org-{}", uuid::Uuid::new_v4().simple());
    let token = format!("alice@{org}");
    let turn_secs = secs("REGISTRY_TURN_SECS", 60);
    let parked_secs = secs("REGISTRY_PARKED_SECS", 120);

    let (feed, rx) = mpsc::unbounded_channel();
    let assemble = |dir: &std::path::Path, device: &str, feed: Option<mpsc::UnboundedReceiver<AgentEvent>>| {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("device-id"), device).unwrap();
        let registry = HarnessRegistry::new();
        registry.register(Arc::new(FeedHarness { feed: tokio::sync::Mutex::new(feed) }));
        let edge = Some(EdgeConfig::with_static_token(base.clone(), token.clone()));
        EngineCore::assemble_with_identity(dir, Arc::new(registry), HarnessId::Mock, edge, &org, "alice")
            .expect("engine assembles")
    };
    let dir_host = tempfile::tempdir().unwrap();
    let dir_viewer = tempfile::tempdir().unwrap();
    let host = assemble(dir_host.path(), HOST, Some(rx));
    let viewer = assemble(dir_viewer.path(), VIEWER, None);

    let observed = Arc::new(Mutex::new(Observed::default()));
    tokio::spawn(observe(
        format!("{base}/registry/{org}/ws?token={token}&device=observer"),
        observed.clone(),
    ));

    for core in [&host, &viewer] {
        wait_for(
            || core.workspace.read_devices().unwrap_or_default().len() == 2,
            "both device rows",
        )
        .await;
    }
    let rpc = zeron_rpc::memory_client(host.rpc_service());
    rpc.call(
        methods::MUTATE,
        json!({"op": "createSpace", "spaceId": "space-w", "deviceId": HOST, "path": "/tmp"}),
    )
    .await
    .expect("create space");
    rpc.call(methods::MUTATE, json!({"op": "createChat", "chatId": CHAT, "spaceId": "space-w"}))
        .await
        .expect("create chat");
    wait_for(|| viewer.workspace.chat(CHAT).ok().flatten().is_some(), "chat on the viewer").await;
    tokio::time::sleep(Duration::from_secs(3)).await;

    let tally = || observed.lock().unwrap().tally.clone();
    let (s0, t0) = (seq(&http, &org, &token).await, tally());

    // The turn: queued by a viewer, run by the host, streaming a delta a second.
    let handle = host.doc_host.open(CHAT).expect("open chat");
    handle
        .doc()
        .queue_command(&SessionCommandEntry {
            id: "cmd-1".into(),
            payload: SessionCommandPayload::Run { request: run_request(), message_id: "m-1".into() },
            issued_by: VIEWER.into(),
            issued_at: chrono::Utc::now().timestamp_millis(),
            based_on: None::<CommandBasedOn>,
            expires_at: None,
            status: SessionCommandStatus::Pending,
            resolution: None,
        })
        .expect("queue run");
    wait_for(
        || host.sessions.session_status(CHAT).is_some_and(|s| s.status == SessionStatus::Working),
        "Working",
    )
    .await;
    feed.send(AgentEvent::SessionStarted {
        harness: HarnessId::Mock,
        model: "mock-1".into(),
        tools: vec![],
        cwd: "/tmp".into(),
        session_id: "hs-w".into(),
        assistant_message_id: "a-w".into(),
    })
    .unwrap();
    // The first half streams; the second is a quiet stretch inside the turn
    // (a long tool call), where only the liveness tick keeps the row fresh.
    let started = tokio::time::Instant::now();
    while started.elapsed() < Duration::from_secs(turn_secs / 2) {
        feed.send(AgentEvent::TextDelta { text: "more words. ".into() }).unwrap();
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    tokio::time::sleep(Duration::from_secs(turn_secs - turn_secs / 2)).await;
    feed.send(AgentEvent::Done {
        status: DoneStatus::Completed,
        result: None,
        error: None,
        session_id: Some("hs-w".into()),
    })
    .unwrap();
    wait_for(
        || host.sessions.session_status(CHAT).is_some_and(|s| s.status == SessionStatus::Idle),
        "parked Idle",
    )
    .await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    let (s1, t1) = (seq(&http, &org, &token).await, tally());

    // Parked: the turn is over, the runtime stays warm.
    tokio::time::sleep(Duration::from_secs(parked_secs)).await;
    let (s2, t2) = (seq(&http, &org, &token).await, tally());

    println!("registry writes, org {org}");
    report("streaming turn", turn_secs + 3, s0, s1, &t0, &t1);
    report("parked after the turn", parked_secs, s1, s2, &t1, &t2);
    let gap = observed.lock().unwrap().longest_live_gap;
    println!("longest a reader held a live session row without a newer one: {gap:?}");
    // Shipped readers drop a live row at 45 s; restamps come at most 30 s
    // apart, leaving the rest for delivery and clock skew.
    assert!(
        gap < Duration::from_secs(33),
        "a live session row went {gap:?} without a restamp"
    );

    drop(feed);
    host.shutdown().await;
    viewer.shutdown().await;
}
