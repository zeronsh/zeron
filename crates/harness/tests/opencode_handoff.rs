//! OpenCode runs across a live update: freeze a run driving an `opencode
//! serve` session, hand it over, and adopt it.
//!
//! OpenCode is unlike the stdio harnesses: the agent is a loopback HTTP server
//! that already outlives any pipe, and the run follows it over an SSE bus (no
//! replay) plus REST. What happened while nobody listened — the handoff gap —
//! must be RECONCILED from REST before the adopted run resumes.
//!
//! Most tests drive an in-test fake server (`Fake`: REST state plus a
//! broadcast bus the test can cut, exactly like the real bus drops frames
//! nobody is subscribed to). The process tests spawn
//! `tests/fixtures/fake_opencode_serve.py` as a real `opencode serve` child.
//!
//! A "commit" stands in for the engine's `execve`: the old run gives up the
//! server without stopping it and a successor in this same process adopts it.

#![cfg(unix)]

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::StreamExt;
use futures::stream::BoxStream;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{broadcast, mpsc, oneshot, watch};

use zeron_harness::{
    CancellationToken, FreezeRefusal, FreezeRequest, FrozenRun, Harness, HarnessError,
    HarnessHandoff, OpencodeHarness, RunControls, SteerMessage,
};
use zeron_proto::{
    AgentEvent, DoneStatus, HarnessId, RunRequest, SandboxLevel, UserInputAnswer, UserInputQuestion,
};

type Events = BoxStream<'static, Result<AgentEvent, HarnessError>>;
type Answer = oneshot::Sender<Vec<UserInputAnswer>>;

const WAIT: Duration = Duration::from_secs(15);
const SES: &str = "ses_h";

// ---------------------------------------------------------------------------
// Fake opencode server (1.x wire)
// ---------------------------------------------------------------------------

#[derive(Default)]
struct State {
    statuses: serde_json::Map<String, Value>,
    /// session → [(info, parts)], in creation order.
    messages: HashMap<String, Vec<(Value, Vec<Value>)>>,
    children: HashMap<String, Vec<Value>>,
    permissions: Vec<Value>,
    questions: Vec<Value>,
    posts: Vec<(String, Value)>,
    gets: Vec<String>,
    /// Paths answered 404 (a server without the route).
    missing: HashSet<String>,
    /// Paths whose POST is never answered (a native command running a turn).
    hold_posts: HashSet<String>,
    /// How long a new bus connection takes to become live (subscribed).
    bus_setup: Duration,
}

/// (GET arrived, release it).
type Gate = (oneshot::Sender<()>, oneshot::Receiver<()>);

#[derive(Clone)]
struct Fake {
    base: String,
    state: Arc<Mutex<State>>,
    bus: broadcast::Sender<String>,
    /// Bumped to cut every live SSE connection.
    cut: watch::Sender<u64>,
    /// Holds the next `GET /session/{SES}/message` until released: the test
    /// emits live frames while the adopter's REST snapshot is in flight.
    message_gate: Arc<tokio::sync::Mutex<Option<Gate>>>,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

impl Fake {
    async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let (bus, _) = broadcast::channel(1024);
        let fake = Self {
            base,
            state: Arc::default(),
            bus,
            cut: watch::channel(0).0,
            message_gate: Arc::default(),
        };
        let accept = fake.clone();
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                let fake = accept.clone();
                tokio::spawn(async move { fake.serve(stream).await });
            }
        });
        fake
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap()
    }

    /// Publish one bus frame. With nobody subscribed it is simply gone, as
    /// on the real (no-replay) bus.
    fn emit(&self, payload: Value) {
        let framed = format!(
            "data: {}\n\n",
            json!({ "directory": "/tmp", "payload": payload })
        );
        let _ = self.bus.send(framed);
    }

    fn subscribers(&self) -> usize {
        self.bus.receiver_count()
    }

    /// Drop every live bus connection (the handoff gap starts here).
    async fn cut_bus(&self) {
        self.cut.send_modify(|n| *n += 1);
        eventually("the bus connections close", || self.subscribers() == 0).await;
    }

    fn status(&self, session: &str, status: &str) {
        self.state()
            .statuses
            .insert(session.into(), json!({ "type": status }));
        self.emit(json!({
            "type": "session.status",
            "properties": { "sessionID": session, "status": { "type": status } },
        }));
    }

    fn assistant(&self, session: &str, message: &str) {
        let info = json!({
            "id": message, "sessionID": session, "role": "assistant",
            "time": { "created": now_ms() },
        });
        self.state()
            .messages
            .entry(session.into())
            .or_default()
            .push((info.clone(), Vec::new()));
        self.emit(json!({ "type": "message.updated", "properties": { "info": info } }));
    }

    /// The provider failed the assistant message (1.x keeps it on the info).
    fn fail_message(&self, session: &str, message: &str, text: &str) {
        let mut state = self.state();
        let (info, _) = state
            .messages
            .get_mut(session)
            .unwrap()
            .iter_mut()
            .find(|(info, _)| info["id"] == message)
            .expect("message exists");
        info["error"] = json!({ "name": "APIError", "data": { "message": text } });
        let info = info.clone();
        drop(state);
        self.emit(json!({ "type": "message.updated", "properties": { "info": info } }));
        self.emit(json!({
            "type": "session.error",
            "properties": { "sessionID": session, "error": info["error"] },
        }));
    }

    /// Insert or replace a part in the REST state; returns it.
    fn put_part(&self, session: &str, part: Value) -> Value {
        let mut state = self.state();
        let message = part["messageID"].as_str().unwrap().to_owned();
        let parts = &mut state
            .messages
            .get_mut(session)
            .unwrap()
            .iter_mut()
            .find(|(info, _)| info["id"] == message)
            .expect("message exists")
            .1;
        match parts.iter_mut().find(|p| p["id"] == part["id"]) {
            Some(existing) => *existing = part.clone(),
            None => parts.push(part.clone()),
        }
        part
    }

    fn part(&self, session: &str, part_id: &str) -> Value {
        self.state().messages[session]
            .iter()
            .flat_map(|(_, parts)| parts)
            .find(|p| p["id"] == part_id)
            .cloned()
            .expect("part exists")
    }

    fn text_open(&self, session: &str, message: &str, part: &str) {
        let part = self.put_part(
            session,
            json!({ "id": part, "sessionID": session, "messageID": message, "type": "text", "text": "" }),
        );
        self.emit(json!({ "type": "message.part.updated", "properties": { "part": part } }));
    }

    fn delta(&self, session: &str, message: &str, part: &str, delta: &str) {
        let mut current = self.part(session, part);
        let text = format!("{}{delta}", current["text"].as_str().unwrap());
        current["text"] = json!(text);
        self.put_part(session, current);
        self.emit(json!({
            "type": "message.part.delta",
            "properties": {
                "sessionID": session, "messageID": message, "partID": part,
                "field": "text", "delta": delta,
            },
        }));
    }

    fn text_close(&self, session: &str, part: &str) {
        let mut current = self.part(session, part);
        current["time"] = json!({ "end": now_ms() });
        let part = self.put_part(session, current);
        self.emit(json!({ "type": "message.part.updated", "properties": { "part": part } }));
    }

    fn task(&self, session: &str, message: &str, part: &str, status: &str, child: Option<&str>) {
        let mut state = json!({ "status": status, "input": { "description": "explore" } });
        if let Some(child) = child {
            state["metadata"] = json!({ "sessionId": child });
        }
        if status == "completed" {
            state["output"] = json!("found it");
        }
        let part = self.put_part(
            session,
            json!({
                "id": part, "sessionID": session, "messageID": message, "type": "tool",
                "tool": "task", "callID": format!("call-{part}"), "state": state,
            }),
        );
        self.emit(json!({ "type": "message.part.updated", "properties": { "part": part } }));
    }

    fn child_created(&self, parent: &str, child: &str, title: &str) {
        let info = json!({
            "id": child, "parentID": parent, "title": title,
            "time": { "created": now_ms() },
        });
        self.state()
            .children
            .entry(parent.into())
            .or_default()
            .push(info.clone());
        self.state().messages.entry(child.into()).or_default();
        self.emit(json!({ "type": "session.created", "properties": { "info": info } }));
    }

    fn ask_permission(&self, session: &str, id: &str) {
        let request = json!({
            "id": id, "sessionID": session, "permission": "bash", "patterns": ["ls"],
            "metadata": {}, "always": [],
        });
        self.state().permissions.push(request.clone());
        self.emit(json!({ "type": "permission.asked", "properties": request }));
    }

    fn ask_question(&self, session: &str, id: &str, question: &str) {
        let request = json!({
            "id": id, "sessionID": session,
            "questions": [{
                "question": question, "header": "Choice",
                "options": [{ "label": "A" }, { "label": "B" }],
            }],
        });
        self.state().questions.push(request.clone());
        self.emit(json!({ "type": "question.asked", "properties": request }));
    }

    /// The question is gone server-side (answered elsewhere, or its turn ended).
    fn drop_question(&self, id: &str) {
        self.state().questions.retain(|q| q["id"] != id);
    }

    fn posts_to(&self, path: &str) -> Vec<Value> {
        self.state()
            .posts
            .iter()
            .filter(|(p, _)| p == path)
            .map(|(_, b)| b.clone())
            .collect()
    }

    fn posted_containing(&self, fragment: &str) -> Vec<String> {
        self.state()
            .posts
            .iter()
            .filter(|(p, _)| p.contains(fragment))
            .map(|(p, _)| p.clone())
            .collect()
    }

    async fn wait_posts(&self, path: &str, n: usize) -> Vec<Value> {
        let deadline = tokio::time::Instant::now() + WAIT;
        loop {
            let posts = self.posts_to(path);
            if posts.len() >= n {
                return posts;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "{path} never saw {n} posts; posts: {:?}",
                self.state().posts
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// Hold the next main-session message GET until the returned sender
    /// fires; the receiver resolves when that GET has arrived.
    async fn gate_messages(&self) -> (oneshot::Receiver<()>, oneshot::Sender<()>) {
        let (arrived_tx, arrived_rx) = oneshot::channel();
        let (release_tx, release_rx) = oneshot::channel();
        *self.message_gate.lock().await = Some((arrived_tx, release_rx));
        (arrived_rx, release_tx)
    }

    async fn serve(self, mut stream: tokio::net::TcpStream) {
        let mut buf: Vec<u8> = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            let header_end = loop {
                if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                    break pos + 4;
                }
                match stream.read(&mut chunk).await {
                    Ok(0) | Err(_) => return,
                    Ok(n) => buf.extend_from_slice(&chunk[..n]),
                }
            };
            let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
            let mut lines = head.lines();
            let start = lines.next().unwrap_or_default().to_owned();
            let content_length = lines
                .filter_map(|l| {
                    let (k, v) = l.split_once(':')?;
                    k.eq_ignore_ascii_case("content-length")
                        .then(|| v.trim().parse::<usize>().ok())
                        .flatten()
                })
                .next()
                .unwrap_or(0);
            while buf.len() < header_end + content_length {
                match stream.read(&mut chunk).await {
                    Ok(0) | Err(_) => return,
                    Ok(n) => buf.extend_from_slice(&chunk[..n]),
                }
            }
            let body: Value = serde_json::from_slice(&buf[header_end..header_end + content_length])
                .unwrap_or(Value::Null);
            buf.drain(..header_end + content_length);
            let mut parts = start.split_whitespace();
            let method = parts.next().unwrap_or_default().to_owned();
            let target = parts.next().unwrap_or_default().to_owned();
            let path = target.split('?').next().unwrap_or_default().to_owned();

            if method == "GET" && path == "/global/event" {
                let setup = self.state().bus_setup;
                tokio::time::sleep(setup).await;
                let mut rx = self.bus.subscribe();
                let mut cut = self.cut.subscribe();
                let _ = stream
                    .write_all(
                        b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\
                          cache-control: no-cache\r\nconnection: close\r\n\r\n\
                          data: {\"payload\":{\"type\":\"server.connected\",\"properties\":{}}}\n\n",
                    )
                    .await;
                loop {
                    tokio::select! {
                        _ = cut.changed() => return,
                        frame = rx.recv() => match frame {
                            Ok(frame) => {
                                if stream.write_all(frame.as_bytes()).await.is_err() {
                                    return;
                                }
                            }
                            Err(broadcast::error::RecvError::Lagged(_)) => continue,
                            Err(_) => return,
                        },
                    }
                }
            }

            if method == "GET" && path == format!("/session/{SES}/message") {
                let gate = self.message_gate.lock().await.take();
                if let Some((arrived, release)) = gate {
                    let _ = arrived.send(());
                    let _ = release.await;
                }
            }
            let held = {
                let mut state = self.state();
                if method == "POST" {
                    state.posts.push((path.clone(), body));
                } else {
                    state.gets.push(path.clone());
                }
                state.hold_posts.contains(&path)
            };
            if held {
                std::future::pending::<()>().await;
            }
            let (status, payload) = self.route(&method, &path);
            let body = payload.to_string();
            let resp = format!(
                "HTTP/1.1 {status}\r\ncontent-type: application/json\r\n\
                 content-length: {}\r\n\r\n{body}",
                body.len()
            );
            if stream.write_all(resp.as_bytes()).await.is_err() {
                return;
            }
        }
    }

    fn route(&self, method: &str, path: &str) -> (&'static str, Value) {
        let mut state = self.state();
        if state.missing.contains(path) {
            return ("404 Not Found", json!({ "missing": path }));
        }
        match (method, path) {
            ("GET", "/global/health") => {
                ("200 OK", json!({ "healthy": true, "version": "1.18.33" }))
            }
            ("GET", "/provider") => ("200 OK", json!({ "all": [], "default": {} })),
            ("GET", "/command") => (
                "200 OK",
                json!([{ "name": "review", "description": "Review" }]),
            ),
            ("POST", "/session") => ("200 OK", json!({ "id": SES })),
            ("GET", "/session/status") => ("200 OK", Value::Object(state.statuses.clone())),
            ("GET", "/permission") => ("200 OK", Value::Array(state.permissions.clone())),
            ("GET", "/question") => ("200 OK", Value::Array(state.questions.clone())),
            ("GET", p) if p.starts_with("/session/") && p.ends_with("/message") => {
                let session = p.split('/').nth(2).unwrap();
                let list: Vec<Value> = state
                    .messages
                    .get(session)
                    .map(|messages| {
                        messages
                            .iter()
                            .map(|(info, parts)| json!({ "info": info, "parts": parts }))
                            .collect()
                    })
                    .unwrap_or_default();
                ("200 OK", Value::Array(list))
            }
            ("GET", p) if p.starts_with("/session/") && p.ends_with("/children") => {
                let session = p.split('/').nth(2).unwrap();
                (
                    "200 OK",
                    Value::Array(state.children.get(session).cloned().unwrap_or_default()),
                )
            }
            ("POST", p) if p.ends_with("/prompt_async") => ("204 No Content", json!({})),
            ("POST", p) if p.ends_with("/abort") => ("200 OK", json!(true)),
            ("POST", p) if p.starts_with("/permission/") && p.ends_with("/reply") => {
                let id = p.split('/').nth(2).unwrap().to_owned();
                state.permissions.retain(|r| r["id"] != id);
                ("200 OK", json!(true))
            }
            ("POST", p) if p.starts_with("/question/") => {
                let id = p.split('/').nth(2).unwrap().to_owned();
                state.questions.retain(|r| r["id"] != id);
                ("200 OK", json!(true))
            }
            _ => ("404 Not Found", json!({ "missing": path })),
        }
    }
}

async fn eventually(what: &str, mut ok: impl FnMut() -> bool) {
    let deadline = tokio::time::Instant::now() + WAIT;
    while !ok() {
        assert!(tokio::time::Instant::now() < deadline, "never: {what}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

// ---------------------------------------------------------------------------
// Harness-side rig
// ---------------------------------------------------------------------------

fn request(prompt: &str) -> RunRequest {
    RunRequest {
        mcp: None,
        prompt: prompt.into(),
        harness: None,
        model: None,
        reasoning: None,
        model_options: serde_json::Map::new(),
        cwd: "/tmp".into(),
        sandbox: SandboxLevel::DangerFullAccess,
        auto_approve: true,
        attachments: Vec::new(),
        resume: None,
        worktree: None,
    }
}

/// The engine's side of one run's controls.
struct Rig {
    steer: mpsc::Sender<SteerMessage>,
    freeze: mpsc::Sender<FreezeRequest>,
    interrupt: CancellationToken,
    /// Every `request_input` call: the questions and the answer slot.
    questions: mpsc::UnboundedReceiver<(Vec<UserInputQuestion>, Answer)>,
    /// Every `rebind_input` call: the key and the answer slot.
    rebinds: mpsc::UnboundedReceiver<(String, Answer)>,
}

fn controls() -> (RunControls, Rig) {
    let (steer, steering) = mpsc::channel(32);
    let (freeze_tx, freeze) = mpsc::channel(1);
    let interrupt = CancellationToken::new();
    let (question_tx, questions) = mpsc::unbounded_channel();
    let (rebind_tx, rebinds) = mpsc::unbounded_channel();
    let controls = RunControls {
        execution_lease: None,
        request_input: Box::new(move |questions| {
            let (tx, rx) = oneshot::channel();
            let _ = question_tx.send((questions, tx));
            rx
        }),
        steering,
        interrupt: interrupt.clone(),
        freeze,
        rebind_input: Box::new(move |key| {
            let (tx, rx) = oneshot::channel();
            let _ = rebind_tx.send((key, tx));
            rx
        }),
    };
    (
        controls,
        Rig {
            steer,
            freeze: freeze_tx,
            interrupt,
            questions,
            rebinds,
        },
    )
}

fn harness(fake: &Fake) -> OpencodeHarness {
    OpencodeHarness::new()
        .with_base_url(fake.base.clone())
        .with_graces(Duration::from_secs(2), Duration::from_millis(200))
}

fn steer(prompt: &str) -> SteerMessage {
    SteerMessage {
        prompt: prompt.into(),
        message_id: Some(format!("msg-{prompt}")),
    }
}

async fn next(stream: &mut Events, seen: &[AgentEvent]) -> Option<AgentEvent> {
    tokio::time::timeout(WAIT, stream.next())
        .await
        .unwrap_or_else(|_| panic!("the run went quiet; seen so far: {seen:?}"))
        .map(|event| event.expect("no stream error"))
}

async fn until(stream: &mut Events, hit: impl Fn(&AgentEvent) -> bool) -> Vec<AgentEvent> {
    let mut seen = Vec::new();
    loop {
        let Some(event) = next(stream, &seen).await else {
            panic!("the stream ended first; seen: {seen:?}");
        };
        let done = hit(&event);
        seen.push(event);
        if done {
            return seen;
        }
    }
}

fn is_done(event: &AgentEvent) -> bool {
    matches!(event, AgentEvent::Done { .. })
}

/// Every remaining event, until the stream ends.
async fn drain(stream: &mut Events) -> Vec<AgentEvent> {
    let mut seen = Vec::new();
    while let Some(event) = next(stream, &seen).await {
        seen.push(event);
    }
    seen
}

/// Whatever arrives within `window` (the stream stays open).
async fn quiet_for(stream: &mut Events, window: Duration) -> Vec<AgentEvent> {
    let mut seen = Vec::new();
    let deadline = tokio::time::Instant::now() + window;
    while let Ok(Some(event)) = tokio::time::timeout_at(deadline, stream.next()).await {
        seen.push(event.expect("no stream error"));
    }
    seen
}

async fn freeze(rig: &Rig) -> Result<FrozenRun, FreezeRefusal> {
    let (reply, rx) = oneshot::channel();
    rig.freeze
        .send(FreezeRequest { reply })
        .await
        .expect("the run takes freeze requests");
    tokio::time::timeout(WAIT, rx)
        .await
        .expect("the run answers a freeze")
        .expect("the run replies")
}

/// Commit a freeze (a same-process successor) and check the old run let go.
async fn commit(frozen: FrozenRun, old: &mut Events) -> HarnessHandoff {
    let handoff = frozen.handoff.clone();
    frozen.commit.send(()).expect("the run awaits the verdict");
    let tail = drain(old).await;
    assert!(
        !tail.iter().any(is_done),
        "a committed run ends its stream without a Done: {tail:?}"
    );
    handoff
}

fn texts(events: &[AgentEvent]) -> String {
    events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::TextDelta { text } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

fn session_starts(events: &[AgentEvent]) -> usize {
    events
        .iter()
        .filter(|e| matches!(e, AgentEvent::SessionStarted { .. }))
        .count()
}

/// Start a run and bring it to "turn open": the prompt landed, the session
/// is busy, and an assistant message `msg_1` streams text part `prt_1`.
async fn started(fake: &Fake, rig_controls: RunControls, said: &str) -> Events {
    let mut run = harness(fake)
        .run(request("first"), rig_controls)
        .await
        .unwrap();
    fake.wait_posts(&format!("/session/{SES}/prompt_async"), 1)
        .await;
    fake.status(SES, "busy");
    fake.assistant(SES, "msg_1");
    fake.text_open(SES, "msg_1", "prt_1");
    fake.delta(SES, "msg_1", "prt_1", said);
    until(
        &mut run,
        |e| matches!(e, AgentEvent::TextDelta { text } if text == said),
    )
    .await;
    run
}

/// Freeze, commit, cut the bus: everything emitted next falls into the gap.
async fn into_the_gap(fake: &Fake, rig: &Rig, run: &mut Events) -> HarnessHandoff {
    let frozen = freeze(rig).await.expect("a safe point");
    let handoff = commit(frozen, run).await;
    fake.cut_bus().await;
    handoff
}

async fn adopt(fake: &Fake, handoff: HarnessHandoff) -> (Events, Rig) {
    let (c, rig) = controls();
    let adopted = harness(fake)
        .adopt(handoff, c, request("first"))
        .await
        .expect("adopts");
    (adopted, rig)
}

// ---------------------------------------------------------------------------
// Reconcile across the gap
// ---------------------------------------------------------------------------

/// Text that streamed before the freeze, text completed in the gap, and
/// deltas that race the adopter's REST snapshot: every byte exactly once.
#[tokio::test]
async fn text_completed_during_the_gap_is_reconciled_from_rest_without_duplicating_streamed_text() {
    let fake = Fake::start().await;
    let (c, rig) = controls();
    let mut run = started(&fake, c, "Hello").await;
    let handoff = into_the_gap(&fake, &rig, &mut run).await;
    assert_eq!(handoff.harness, HarnessId::Opencode);
    assert_eq!(handoff.stdin_fd, HarnessHandoff::NO_PIPE);
    assert_eq!(handoff.stdout_fd, HarnessHandoff::NO_PIPE);
    assert!(handoff.stdout_leftover.is_empty());

    // In the gap: more text nobody hears.
    fake.delta(SES, "msg_1", "prt_1", " world");
    // A bus that takes a while to go live: REST must wait for it.
    fake.state().bus_setup = Duration::from_millis(300);

    let (arrived, release) = fake.gate_messages().await;
    let (mut adopted, _rig2) = adopt(&fake, handoff).await;
    tokio::time::timeout(WAIT, arrived)
        .await
        .expect("the adopter reads the messages")
        .unwrap();
    assert_eq!(fake.subscribers(), 1, "the bus is subscribed BEFORE REST");
    // While the snapshot is in flight a live delta lands on the bus: it is in
    // the snapshot AND buffered on the bus. It must render once.
    fake.delta(SES, "msg_1", "prt_1", " again");
    tokio::time::sleep(Duration::from_millis(100)).await;
    release.send(()).unwrap();
    // After the reconcile the part keeps streaming.
    tokio::time::sleep(Duration::from_millis(200)).await;
    fake.delta(SES, "msg_1", "prt_1", "!");
    fake.text_close(SES, "prt_1");
    fake.status(SES, "idle");
    let seen = until(&mut adopted, is_done).await;
    assert_eq!(
        session_starts(&seen),
        0,
        "no second SessionStarted: {seen:?}"
    );
    assert_eq!(
        format!("Hello{}", texts(&seen)),
        "Hello world again!",
        "every byte once: {seen:?}"
    );
    assert!(matches!(
        seen.last(),
        Some(AgentEvent::Done { status: DoneStatus::Completed, session_id: Some(id), .. }) if id == SES
    ));
}

#[tokio::test]
async fn a_turn_that_finished_during_the_gap_settles_after_reconcile_not_before() {
    let fake = Fake::start().await;
    let (c, rig) = controls();
    let mut run = started(&fake, c, "Hello").await;
    let handoff = into_the_gap(&fake, &rig, &mut run).await;
    fake.delta(SES, "msg_1", "prt_1", " from the gap");
    fake.text_close(SES, "prt_1");
    fake.status(SES, "idle");

    let (mut adopted, _rig2) = adopt(&fake, handoff).await;
    let seen = until(&mut adopted, is_done).await;
    let healed = seen
        .iter()
        .position(|e| matches!(e, AgentEvent::TextDelta { text } if text == " from the gap"))
        .unwrap_or_else(|| panic!("the gap's text is healed: {seen:?}"));
    assert_eq!(seen.iter().filter(|e| is_done(e)).count(), 1, "{seen:?}");
    assert!(
        healed < seen.len() - 1,
        "Done comes after the heal: {seen:?}"
    );
    assert!(matches!(
        seen.last(),
        Some(AgentEvent::Done {
            status: DoneStatus::Completed,
            ..
        })
    ));
    // Settled: nothing more, and the (still open) mailbox keeps the run.
    assert!(
        quiet_for(&mut adopted, Duration::from_millis(300))
            .await
            .is_empty()
    );
}

/// The provider failed the turn in the gap: its `session.error` frame is gone,
/// but the error is on the message. It surfaces before the turn settles, and
/// the turn settles as it would have live.
#[tokio::test]
async fn an_error_that_ended_the_turn_in_the_gap_surfaces_before_it_settles() {
    let fake = Fake::start().await;
    let (c, rig) = controls();
    let mut run = started(&fake, c, "Hello").await;
    let handoff = into_the_gap(&fake, &rig, &mut run).await;
    fake.fail_message(SES, "msg_1", "rate limited");
    fake.status(SES, "idle");
    let (mut adopted, _rig2) = adopt(&fake, handoff).await;
    let seen = until(&mut adopted, is_done).await;
    assert_eq!(
        seen.iter()
            .filter(|e| matches!(e, AgentEvent::Error { message } if message == "rate limited"))
            .count(),
        1,
        "{seen:?}"
    );
    // Content streamed before the failure, so (as live) the turn completes
    // with the error chip.
    assert!(
        matches!(
            seen.last(),
            Some(AgentEvent::Done {
                status: DoneStatus::Completed,
                ..
            })
        ),
        "{seen:?}"
    );

    // Nothing streamed yet: the same failure settles the turn as errored.
    let fake = Fake::start().await;
    let (c, rig) = controls();
    let mut run = harness(&fake).run(request("first"), c).await.unwrap();
    fake.wait_posts(&format!("/session/{SES}/prompt_async"), 1)
        .await;
    fake.status(SES, "busy");
    fake.assistant(SES, "msg_1");
    tokio::time::sleep(Duration::from_millis(100)).await;
    let handoff = into_the_gap(&fake, &rig, &mut run).await;
    fake.fail_message(SES, "msg_1", "no credits");
    fake.status(SES, "idle");
    let (mut adopted, _rig2) = adopt(&fake, handoff).await;
    let seen = until(&mut adopted, is_done).await;
    assert!(
        matches!(
            seen.last(),
            Some(AgentEvent::Done { status: DoneStatus::Errored, error: Some(error), .. })
                if error == "no credits"
        ),
        "{seen:?}"
    );
}

#[tokio::test]
async fn a_permission_asked_during_the_gap_is_surfaced_after_adopt_not_lost() {
    let fake = Fake::start().await;
    let (c, rig) = controls();
    let mut run = started(&fake, c, "Hello").await;
    let handoff = into_the_gap(&fake, &rig, &mut run).await;
    fake.ask_permission(SES, "per_gap");
    fake.ask_permission("ses_foreign", "per_foreign");

    let (mut adopted, _rig2) = adopt(&fake, handoff).await;
    let replies = fake.wait_posts("/permission/per_gap/reply", 1).await;
    assert_eq!(replies[0], json!({ "reply": "once" }));
    fake.status(SES, "idle");
    until(&mut adopted, is_done).await;
    assert!(
        fake.posted_containing("per_foreign").is_empty(),
        "another session's permission is never answered"
    );
}

#[tokio::test]
async fn a_question_asked_during_the_gap_is_surfaced_after_adopt() {
    let fake = Fake::start().await;
    let (c, rig) = controls();
    let mut run = started(&fake, c, "Hello").await;
    let handoff = into_the_gap(&fake, &rig, &mut run).await;
    fake.ask_question(SES, "que_gap", "Pick one");

    let (arrived, release) = fake.gate_messages().await;
    let (mut adopted, mut rig2) = adopt(&fake, handoff).await;
    arrived.await.unwrap();
    // Asked while the reconcile is in flight: buffered on the bus AND listed
    // by `GET /question`. One prompt, not two.
    fake.ask_question(SES, "que_live", "Pick again");
    tokio::time::sleep(Duration::from_millis(100)).await;
    release.send(()).unwrap();
    let mut asked = HashMap::new();
    for _ in 0..2 {
        let (questions, answer) = tokio::time::timeout(WAIT, rig2.questions.recv())
            .await
            .expect("the gap's questions are asked")
            .unwrap();
        assert_eq!(questions.len(), 1);
        asked.insert(questions[0].question.clone(), (questions, answer));
    }
    assert!(rig2.rebinds.try_recv().is_err(), "nothing to rebind");
    let (questions, answer) = asked.remove("Pick one").expect("the gap's question");
    answer
        .send(vec![UserInputAnswer {
            question_id: questions[0].id.clone(),
            labels: vec!["B".into()],
        }])
        .unwrap();
    let replies = fake.wait_posts("/question/que_gap/reply", 1).await;
    assert_eq!(replies[0], json!({ "answers": [["B"]] }));
    assert!(asked.contains_key("Pick again"));
    fake.status(SES, "idle");
    until(&mut adopted, is_done).await;
    assert!(
        rig2.questions.try_recv().is_err(),
        "each question asked exactly once"
    );
}

#[tokio::test]
async fn child_session_created_during_the_gap_is_bound() {
    let fake = Fake::start().await;
    let (c, rig) = controls();
    let mut run = started(&fake, c, "Hello").await;
    // A task spawn is running (no child session known yet).
    fake.task(SES, "msg_1", "prt_task", "running", None);
    until(
        &mut run,
        |e| matches!(e, AgentEvent::ToolCall { id, .. } if id == "prt_task"),
    )
    .await;
    let handoff = into_the_gap(&fake, &rig, &mut run).await;
    // In the gap: the child session starts and speaks.
    fake.child_created(SES, "ses_child", "explore (@general subagent)");
    fake.assistant("ses_child", "msg_c1");
    fake.text_open("ses_child", "msg_c1", "prt_c1");
    fake.delta("ses_child", "msg_c1", "prt_c1", "child says hi");

    let (mut adopted, _rig2) = adopt(&fake, handoff).await;
    let seen = until(&mut adopted, |e| {
        matches!(e, AgentEvent::Subagent { parent_tool_use_id, event }
            if parent_tool_use_id == "prt_task"
                && matches!(event.as_ref(), AgentEvent::TextDelta { text } if text == "child says hi"))
    })
    .await;
    assert_eq!(
        texts(&seen),
        "",
        "child text never leaks into the parent: {seen:?}"
    );
    // The spawn completes live: its chip settles the bound child.
    fake.task(SES, "msg_1", "prt_task", "completed", Some("ses_child"));
    until(&mut adopted, |e| {
        matches!(e, AgentEvent::Subagent { parent_tool_use_id, event }
            if parent_tool_use_id == "prt_task" && is_done(event))
    })
    .await;
    fake.status(SES, "idle");
    until(&mut adopted, is_done).await;
}

#[tokio::test]
async fn missing_reconcile_routes_are_skipped_not_fatal() {
    let fake = Fake::start().await;
    let (c, rig) = controls();
    let mut run = started(&fake, c, "Hello").await;
    let handoff = into_the_gap(&fake, &rig, &mut run).await;
    {
        let mut state = fake.state();
        for path in ["/permission", "/question", "/session/ses_h/children"] {
            state.missing.insert(path.into());
        }
    }
    fake.delta(SES, "msg_1", "prt_1", " still healed");
    fake.status(SES, "idle");
    let (mut adopted, _rig2) = adopt(&fake, handoff).await;
    let seen = until(&mut adopted, is_done).await;
    assert_eq!(texts(&seen), " still healed", "{seen:?}");
    assert!(matches!(
        seen.last(),
        Some(AgentEvent::Done {
            status: DoneStatus::Completed,
            ..
        })
    ));
}

// ---------------------------------------------------------------------------
// Parked questions, answers, steers
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_parked_question_is_rebound_under_its_original_id_and_never_asked_twice() {
    let fake = Fake::start().await;
    let (c, mut rig) = controls();
    let mut run = started(&fake, c, "Hello").await;
    fake.ask_question(SES, "que_1", "Pick one");
    fake.ask_question(SES, "que_2", "Pick another");
    let (first, _old_first) = tokio::time::timeout(WAIT, rig.questions.recv())
        .await
        .unwrap()
        .unwrap();
    let (second, _old_second) = tokio::time::timeout(WAIT, rig.questions.recv())
        .await
        .unwrap()
        .unwrap();
    assert_ne!(
        first[0].id, second[0].id,
        "question ids are unique across requests (the engine rebinds by them)"
    );
    let handoff = into_the_gap(&fake, &rig, &mut run).await;
    // In the gap the second question went away server-side.
    fake.drop_question("que_2");

    let (mut adopted, mut rig2) = adopt(&fake, handoff).await;
    let mut rebinds = HashMap::new();
    for _ in 0..2 {
        let (key, slot) = tokio::time::timeout(WAIT, rig2.rebinds.recv())
            .await
            .expect("parked questions are rebound")
            .unwrap();
        rebinds.insert(key, slot);
    }
    let stale = rebinds.remove(&second[0].id).expect("rebound by its id");
    eventually("the stale question is let go", || stale.is_closed()).await;
    let live = rebinds.remove(&first[0].id).expect("rebound by its id");
    live.send(vec![UserInputAnswer {
        question_id: first[0].id.clone(),
        labels: vec!["A".into()],
    }])
    .unwrap();
    let replies = fake.wait_posts("/question/que_1/reply", 1).await;
    assert_eq!(replies[0], json!({ "answers": [["A"]] }));
    fake.status(SES, "idle");
    until(&mut adopted, is_done).await;
    assert!(
        rig2.questions.try_recv().is_err(),
        "no second prompt for a parked question"
    );
    assert!(fake.posts_to("/question/que_2/reply").is_empty());
}

/// The user answered a moment before the freeze: the reply must reach the
/// server before the freeze completes, not be exported as still parked.
#[tokio::test]
async fn an_answer_given_just_before_the_freeze_is_posted_before_the_freeze_completes() {
    let fake = Fake::start().await;
    let (c, mut rig) = controls();
    let mut run = started(&fake, c, "Hello").await;
    fake.ask_question(SES, "que_1", "Pick one");
    let (questions, answer) = tokio::time::timeout(WAIT, rig.questions.recv())
        .await
        .unwrap()
        .unwrap();
    // Answer and freeze become ready in the same breath (no await between).
    answer
        .send(vec![UserInputAnswer {
            question_id: questions[0].id.clone(),
            labels: vec!["B".into()],
        }])
        .unwrap();
    let frozen = freeze(&rig).await.expect("safe point");
    assert_eq!(
        fake.posts_to("/question/que_1/reply"),
        [json!({ "answers": [["B"]] })],
        "written before the freeze answered"
    );
    let handoff = commit(frozen, &mut run).await;
    let (mut adopted, mut rig2) = adopt(&fake, handoff).await;
    fake.status(SES, "idle");
    until(&mut adopted, is_done).await;
    assert!(
        rig2.rebinds.try_recv().is_err(),
        "an answered question is not rebound"
    );
    assert!(rig2.questions.try_recv().is_err());
    assert_eq!(fake.posts_to("/question/que_1/reply").len(), 1);
}

#[tokio::test]
async fn a_thawed_run_resumes_with_its_drained_steers_first_and_in_order() {
    let fake = Fake::start().await;
    let (c, rig) = controls();
    let mut run = started(&fake, c, "Hello").await;
    fake.text_close(SES, "prt_1");
    fake.status(SES, "idle");
    until(&mut run, is_done).await;
    let prompts = |frozen: &FrozenRun| -> Vec<String> {
        frozen
            .handoff
            .undrained_steers
            .iter()
            .map(|s| s.prompt.clone())
            .collect()
    };
    let frozen = freeze(&rig).await.expect("parked between turns");
    assert!(prompts(&frozen).is_empty());
    rig.steer.send(steer("one")).await.unwrap();
    rig.steer.send(steer("two")).await.unwrap();
    drop(frozen); // a failed exec: the run resumes where it was
    let frozen = freeze(&rig).await.expect("still a safe point");
    assert_eq!(prompts(&frozen), ["one", "two"]);
    rig.steer.send(steer("three")).await.unwrap();
    drop(frozen);
    let frozen = freeze(&rig).await.expect("still a safe point");
    assert_eq!(prompts(&frozen), ["one", "two", "three"]);
    drop(frozen);
    // The first goes out at once (nothing is running), in order.
    let posts = fake
        .wait_posts(&format!("/session/{SES}/prompt_async"), 2)
        .await;
    assert_eq!(posts[1]["parts"][0]["text"], "one", "{posts:?}");
    drop(rig);
    let _ = quiet_for(&mut run, Duration::from_millis(100)).await;
}

/// The bus dropped while the run was frozen and the gap's frames are gone:
/// a thaw reconciles exactly like an adoption.
#[tokio::test]
async fn a_thaw_after_the_bus_dropped_reconciles_the_gap() {
    let fake = Fake::start().await;
    let (c, rig) = controls();
    let mut run = started(&fake, c, "Hello").await;
    let frozen = freeze(&rig).await.expect("a safe point");
    fake.cut_bus().await;
    fake.delta(SES, "msg_1", "prt_1", " lost frames");
    fake.text_close(SES, "prt_1");
    fake.status(SES, "idle");
    drop(frozen); // thaw
    let seen = until(&mut run, is_done).await;
    assert_eq!(texts(&seen), " lost frames", "{seen:?}");
    assert!(matches!(
        seen.last(),
        Some(AgentEvent::Done {
            status: DoneStatus::Completed,
            ..
        })
    ));
}

/// The prompt is acknowledged but the server has not turned busy yet: a
/// reconcile now would find "not running" and end a turn that never started.
#[tokio::test]
async fn freeze_is_busy_until_the_server_has_turned_busy_for_the_turn() {
    let fake = Fake::start().await;
    let (c, rig) = controls();
    let mut run = harness(&fake).run(request("first"), c).await.unwrap();
    fake.wait_posts(&format!("/session/{SES}/prompt_async"), 1)
        .await;
    // Let the acknowledged POST finish; the session is still not busy.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        freeze(&rig).await.unwrap_err(),
        FreezeRefusal::Busy("the turn has not started yet")
    );
    // Once the server reports busy, the very same run may freeze.
    fake.status(SES, "busy");
    fake.assistant(SES, "msg_1");
    fake.text_open(SES, "msg_1", "prt_1");
    fake.delta(SES, "msg_1", "prt_1", "working");
    until(
        &mut run,
        |e| matches!(e, AgentEvent::TextDelta { text } if text == "working"),
    )
    .await;
    let frozen = freeze(&rig).await.expect("safe once the turn is busy");
    drop(frozen);
    rig.interrupt.cancel();
    fake.status(SES, "idle");
    let _ = drain(&mut run).await;
}

#[tokio::test]
async fn freeze_is_busy_while_a_native_command_or_an_interrupt_is_in_flight() {
    // A native `/command` holds its HTTP request for the whole turn.
    let fake = Fake::start().await;
    fake.state()
        .hold_posts
        .insert(format!("/session/{SES}/command"));
    let (c, rig) = controls();
    let mut run = harness(&fake).run(request("/review now"), c).await.unwrap();
    fake.wait_posts(&format!("/session/{SES}/command"), 1).await;
    assert_eq!(
        freeze(&rig).await.unwrap_err(),
        FreezeRefusal::Busy("a native command is in flight")
    );
    rig.interrupt.cancel();
    let _ = drain(&mut run).await;

    // Interrupting: the abort is out, its idle has not arrived.
    let fake = Fake::start().await;
    let (c, rig) = controls();
    let mut run = started(&fake, c, "Hello").await;
    rig.interrupt.cancel();
    fake.wait_posts(&format!("/session/{SES}/abort"), 1).await;
    assert_eq!(
        freeze(&rig).await.unwrap_err(),
        FreezeRefusal::Busy("interrupting")
    );
    fake.status(SES, "idle");
    let tail = drain(&mut run).await;
    assert!(matches!(
        tail.last(),
        Some(AgentEvent::Done {
            status: DoneStatus::Interrupted,
            ..
        })
    ));
}

#[tokio::test]
async fn an_unknown_state_version_is_refused_so_the_engine_falls_back() {
    let fake = Fake::start().await;
    let handoff = HarnessHandoff {
        harness: HarnessId::Opencode,
        state_version: 9999,
        pid: 0,
        stdin_fd: HarnessHandoff::NO_PIPE,
        stdout_fd: HarnessHandoff::NO_PIPE,
        stderr_fd: None,
        stdout_leftover: Vec::new(),
        stderr_tail: Vec::new(),
        state: json!({}),
        extra_fds: Vec::new(),
        undrained_steers: Vec::new(),
    };
    let (c, _rig) = controls();
    let Err(err) = harness(&fake).adopt(handoff, c, request("x")).await else {
        panic!("adopted an unknown state version");
    };
    assert!(matches!(err, HarnessError::Protocol(_)), "{err}");
    assert!(harness(&fake).supports_adoption());
}

// ---------------------------------------------------------------------------
// A real `opencode serve` child
// ---------------------------------------------------------------------------

fn fixture() -> PathBuf {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("fake_opencode_serve.py");
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755));
    path
}

fn process_harness() -> OpencodeHarness {
    OpencodeHarness::new()
        .with_executable(fixture())
        .with_graces(Duration::from_secs(2), Duration::from_secs(2))
}

fn alive(pid: i32) -> bool {
    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
    let rc = unsafe {
        libc::waitid(
            libc::P_PID,
            pid as libc::id_t,
            &mut info,
            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
        )
    };
    let exited = rc == 0 && unsafe { info.si_pid() } == pid;
    let signalable = unsafe { libc::kill(pid, 0) } == 0;
    signalable && !exited
}

fn dup(fd: i32) -> i32 {
    let copy = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, 3) };
    assert!(copy >= 3, "dup {fd}");
    copy
}

/// Captures every log line the harness writes, at every level.
#[derive(Clone, Default)]
struct LogSink(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for LogSink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogSink {
    type Writer = LogSink;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

fn files_containing(root: &std::path::Path, needles: &[String]) -> Vec<PathBuf> {
    let mut hits = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if let Ok(bytes) = std::fs::read(&path) {
                let text = String::from_utf8_lossy(&bytes);
                if needles.iter().any(|n| text.contains(n.as_str())) {
                    hits.push(path);
                }
            }
        }
    }
    hits
}

#[tokio::test]
async fn opencode_server_process_is_not_killed_and_password_is_never_written_to_disk() {
    let logs = LogSink::default();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_writer(logs.clone())
        .with_ansi(false)
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    let home = tempfile::tempdir().unwrap();
    let mut req = request("first");
    req.cwd = home.path().to_string_lossy().into_owned();

    let (c, rig) = controls();
    let mut run = process_harness().run(req.clone(), c).await.unwrap();
    until(
        &mut run,
        |e| matches!(e, AgentEvent::TextDelta { text } if text == "echo: first"),
    )
    .await;
    until(&mut run, is_done).await;
    let frozen = freeze(&rig)
        .await
        .expect("a parked session is a safe point");
    let handoff = frozen.handoff.clone();
    assert!(handoff.pid > 1, "the server's pid is handed over");
    assert!(
        handoff.stderr_fd.is_some(),
        "stderr is a pipe, kept drained"
    );
    assert_eq!(handoff.stdin_fd, HarnessHandoff::NO_PIPE);
    assert_eq!(handoff.stdout_fd, HarnessHandoff::NO_PIPE);
    assert!(
        handoff
            .stderr_tail
            .iter()
            .any(|l| l == "fake-opencode: started"),
        "{:?}",
        handoff.stderr_tail
    );
    // The secret is in the (anonymous) manifest state and nowhere else.
    let state = handoff.state.to_string();
    let secret = handoff.state["auth"]
        .as_str()
        .expect("the server's credential rides in the state")
        .to_owned();
    use base64::Engine as _;
    let decoded = String::from_utf8(
        base64::engine::general_purpose::STANDARD
            .decode(secret.trim_start_matches("Basic "))
            .unwrap(),
    )
    .unwrap();
    let password = decoded.trim_start_matches("opencode:").to_owned();
    assert!(password.len() >= 16 && state.contains(&secret));
    let debug = format!("{handoff:?} {frozen:?}");
    assert!(
        !debug.contains(&password) && !debug.contains(&secret),
        "{debug}"
    );
    let handoff = commit(frozen, &mut run).await;
    assert!(alive(handoff.pid), "the server is handed over, not killed");

    let (c2, rig2) = controls();
    let mut adopted = process_harness()
        .adopt(handoff.clone(), c2, req.clone())
        .await
        .expect("adopts");
    if let Some(fd) = handoff.stderr_fd {
        unsafe { libc::close(fd) };
    }
    rig2.steer.send(steer("second")).await.unwrap();
    let seen = until(
        &mut adopted,
        |e| matches!(e, AgentEvent::TextDelta { text } if text == "echo: second"),
    )
    .await;
    assert_eq!(session_starts(&seen), 0);
    until(&mut adopted, is_done).await;
    // Ending the adopted run stops the server it adopted.
    drop(rig2);
    drain(&mut adopted).await;
    let deadline = std::time::Instant::now() + WAIT;
    while alive(handoff.pid) {
        assert!(
            std::time::Instant::now() < deadline,
            "the adopter stops the server"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    drop(rig);

    let needles = vec![password.clone(), secret.clone()];
    let logged = String::from_utf8_lossy(&logs.0.lock().unwrap()).into_owned();
    assert!(!logged.is_empty(), "the log capture works");
    assert!(
        !needles.iter().any(|n| logged.contains(n.as_str())),
        "a secret reached the logs"
    );
    assert_eq!(
        files_containing(home.path(), &needles),
        Vec::<PathBuf>::new(),
        "a secret was written to disk"
    );
}

#[test]
fn a_run_task_dropped_while_frozen_leaves_the_server_running_and_adoptable() {
    // Stands in for the exec path: the engine never commits, the old image
    // just stops existing. Dropping the runtime drops the run task and its
    // tokio `Child`; a dup keeps the stderr pipe open as the exec'd image's
    // inherited descriptor would.
    let old = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let (frozen, inherited) = old.block_on(async {
        let (c, rig) = controls();
        let mut run = process_harness().run(request("first"), c).await.unwrap();
        until(&mut run, is_done).await;
        let frozen = freeze(&rig).await.expect("safe point");
        let mut inherited = frozen.handoff.clone();
        inherited.stderr_fd = inherited.stderr_fd.map(dup);
        std::mem::forget(rig);
        std::mem::forget(run);
        (frozen, inherited)
    });
    drop(old);
    drop(frozen);
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        alive(inherited.pid),
        "a dropped run must not kill its server"
    );

    let new = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    new.block_on(async {
        let (c, rig) = controls();
        let mut adopted = process_harness()
            .adopt(inherited.clone(), c, request("first"))
            .await
            .unwrap();
        if let Some(fd) = inherited.stderr_fd {
            unsafe { libc::close(fd) };
        }
        rig.steer.send(steer("after exec")).await.unwrap();
        until(
            &mut adopted,
            |e| matches!(e, AgentEvent::TextDelta { text } if text == "echo: after exec"),
        )
        .await;
        until(&mut adopted, is_done).await;
        // The adopted server can be frozen again, and a thaw lets its waiter
        // reap it again (the gate is released on every non-commit path).
        let frozen = freeze(&rig).await.expect("an adopted run freezes again");
        assert_eq!(frozen.handoff.pid, inherited.pid);
        drop(frozen);
        rig.steer.send(steer("after thaw")).await.unwrap();
        until(
            &mut adopted,
            |e| matches!(e, AgentEvent::TextDelta { text } if text == "echo: after thaw"),
        )
        .await;
        drop(rig);
        drain(&mut adopted).await;
    });
    assert!(!alive(inherited.pid), "the adopter stopped its server");
}
