//! An agent process parked between turns with nothing pending is stopped
//! after a short idle period ([`zeron_engine::sessions::AGENT_IDLE_STOP`]),
//! where its driver resumes sessions whole; the next message resumes it.
//! Nothing pending is ever stopped: background work, a scheduled wake-up, a
//! question, another device reading along.
//!
//! And an agent's Zeron MCP tools come from the engine itself, over a token
//! that names its run's chat and dies with the run.
//!
//! The harness models one agent process per `run()`: `run()` is a spawn, the
//! `interrupt` token its death.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::BoxStream;
use tokio::sync::mpsc;

use zeron_engine::sessions::AGENT_IDLE_STOP;
use zeron_engine::{EngineCore, HarnessRegistry};
use zeron_harness::{Harness, HarnessError, RunControls, TurnControl};
use zeron_proto::{
    AgentEvent, DoneStatus, HarnessId, Model, ReasoningLevel, RunRequest, SandboxLevel,
    SessionStatus, SteeringMode, ToolCall,
};

const CHAT: &str = "chat-idle-stop";

#[derive(Default)]
struct Ledger {
    spawns: AtomicUsize,
    kills: AtomicUsize,
    /// Each spawn's `resume` (the provider session it was told to continue).
    resumes: Mutex<Vec<Option<String>>>,
    prompts: Mutex<Vec<String>>,
    turn: Mutex<Option<TurnControl>>,
    /// Each spawn's Zeron MCP server, as the host stamped it.
    mcp: Mutex<Vec<Option<zeron_proto::McpServer>>>,
    /// The real process standing in for the agent CLI, told to the host.
    process: Mutex<Option<u32>>,
    /// Starts a wake turn (one nobody sent) with this prompt.
    wake: Mutex<Option<mpsc::UnboundedSender<String>>>,
}

/// `bg` in a prompt starts background work (the test ends it); `wake`
/// schedules a wake-up (Claude's `ScheduleWakeup`) with no input seen;
/// `wake:<secs>` / `wake:stop` call it with that delay or `stop`, and
/// `cron:<id>` / `uncron:<id>` create and delete a session cron, each
/// answered as the CLI would.
struct Agent {
    ledger: Arc<Ledger>,
    stops_when_idle: bool,
}

fn done() -> AgentEvent {
    AgentEvent::Done {
        status: DoneStatus::Completed,
        result: None,
        error: None,
        session_id: Some("provider-session".into()),
    }
}

#[async_trait]
impl Harness for Agent {
    fn id(&self) -> HarnessId {
        HarnessId::Mock
    }
    fn display_name(&self) -> &str {
        "Agent"
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
    fn wakes_on_background(&self) -> bool {
        true
    }
    fn stops_when_idle(&self) -> bool {
        self.stops_when_idle
    }
    /// Claude's own reading of its user's MCP config, where the test gave
    /// the run a config dir of its own.
    fn configured_mcp_servers(
        &self,
        request: &RunRequest,
    ) -> Vec<zeron_harness::process::KnownServer> {
        if !request.env.contains_key("CLAUDE_CONFIG_DIR") {
            return Vec::new();
        }
        zeron_harness::claude::configured_mcp_servers(
            &request.env,
            std::path::Path::new(&request.cwd),
        )
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
        ledger.spawns.fetch_add(1, Ordering::SeqCst);
        ledger.resumes.lock().unwrap().push(request.resume.clone());
        ledger.mcp.lock().unwrap().push(request.mcp.clone());
        *ledger.turn.lock().unwrap() = Some(controls.turn.clone());
        controls.turn.set_agent_pid(*ledger.process.lock().unwrap());
        let (tx, rx) = mpsc::channel::<Result<AgentEvent, HarnessError>>(64);
        tokio::spawn(async move {
            let turn = controls.turn.clone();
            let send = |event: AgentEvent| {
                let tx = tx.clone();
                async move { tx.send(Ok(event)).await.is_ok() }
            };
            let answer = |prompt: String| {
                let ledger = ledger.clone();
                let turn = turn.clone();
                let send = send.clone();
                async move {
                    ledger.prompts.lock().unwrap().push(prompt.clone());
                    if prompt.contains("bg") {
                        turn.set_background(1);
                    }
                    for (n, word) in prompt.split_whitespace().enumerate() {
                        let id = format!("t-{n}-{word}");
                        let (name, input, output) = if let Some(delay) = word.strip_prefix("wake:")
                        {
                            let input = match delay.parse::<u64>() {
                                Ok(secs) => serde_json::json!({ "delaySeconds": secs }),
                                Err(_) => serde_json::json!({ "stop": true }),
                            };
                            ("ScheduleWakeup", input, "Scheduled".to_owned())
                        } else if let Some(cron) = word.strip_prefix("cron:") {
                            (
                                "CronCreate",
                                serde_json::json!({ "cron": "*/5 * * * *" }),
                                format!("Scheduled recurring job {cron} (*/5 * * * *)"),
                            )
                        } else if let Some(cron) = word.strip_prefix("uncron:") {
                            (
                                "CronDelete",
                                serde_json::json!({ "id": cron }),
                                format!("Cancelled job {cron}"),
                            )
                        } else {
                            continue;
                        };
                        let call = AgentEvent::ToolCall {
                            id: id.clone(),
                            call: ToolCall::Unknown {
                                name: name.into(),
                                input: Some(input),
                            },
                        };
                        let result = AgentEvent::ToolResult {
                            id,
                            is_error: false,
                            output: Some(output),
                            diff: None,
                            preview_paths: Vec::new(),
                            image_payloads: Vec::new(),
                            exit_code: None,
                        };
                        if !send(call).await || !send(result).await {
                            return false;
                        }
                    }
                    if prompt.contains("wake")
                        && !prompt.contains("wake:")
                        && !send(AgentEvent::ToolCall {
                            id: "t-wake".into(),
                            call: ToolCall::Unknown {
                                name: "ScheduleWakeup".into(),
                                input: None,
                            },
                        })
                        .await
                    {
                        return false;
                    }
                    send(AgentEvent::TextDelta {
                        text: format!("on it: {prompt}"),
                    })
                    .await
                        && send(done()).await
                }
            };
            let _ = send(AgentEvent::SessionStarted {
                harness: HarnessId::Mock,
                model: "agent".into(),
                tools: vec![],
                cwd: request.cwd.clone(),
                session_id: "provider-session".into(),
                assistant_message_id: "a0".into(),
            })
            .await;
            if !answer(request.prompt.clone()).await {
                return;
            }
            let (wake_tx, mut wakes) = mpsc::unbounded_channel::<String>();
            *ledger.wake.lock().unwrap() = Some(wake_tx);
            loop {
                tokio::select! {
                    biased;
                    Some(prompt) = wakes.recv() => {
                        turn.note_wake();
                        if !answer(prompt).await {
                            return;
                        }
                    }
                    _ = controls.interrupt.cancelled() => {
                        ledger.kills.fetch_add(1, Ordering::SeqCst);
                        let _ = tx.send(Ok(AgentEvent::Done {
                            status: DoneStatus::Interrupted,
                            result: None,
                            error: None,
                            session_id: None,
                        })).await;
                        return;
                    }
                    steer = controls.steering.recv() => {
                        let Some(steer) = steer else { return };
                        let boundary = AgentEvent::Steered {
                            assistant_message_id: None,
                            next_assistant_message_id: None,
                        };
                        if !send(boundary).await || !answer(steer.prompt.clone()).await {
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
    _dir: tempfile::TempDir,
}

fn rig(stops_when_idle: bool) -> Rig {
    let ledger = Arc::new(Ledger::default());
    let registry = HarnessRegistry::new();
    registry.register(Arc::new(Agent {
        ledger: ledger.clone(),
        stops_when_idle,
    }));
    let dir = tempfile::tempdir().unwrap();
    let core = EngineCore::assemble(dir.path(), Arc::new(registry), HarnessId::Mock, None)
        .expect("engine core assembles");
    Rig {
        core,
        ledger,
        _dir: dir,
    }
}

fn request(prompt: &str) -> RunRequest {
    RunRequest {
        mcp: None,
        env: Default::default(),
        prompt: prompt.into(),
        harness: Some(HarnessId::Mock),
        model: Some("agent-1".into()),
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
    async fn send(&self, prompt: &str) {
        self.send_request(request(prompt)).await;
    }

    async fn send_request(&self, request: RunRequest) {
        let before = self.ledger.prompts.lock().unwrap().len();
        self.core
            .sessions
            .dispatch(CHAT, HarnessId::Mock, request, None)
            .await
            .unwrap();
        wait_for(
            || {
                self.core.sessions.session_status(CHAT).map(|s| s.status)
                    == Some(SessionStatus::Idle)
                    && self.ledger.prompts.lock().unwrap().len() > before
            },
            "the turn to park",
        )
        .await;
    }

    /// A wake turn: the runtime starts a turn of its own (as when a
    /// background task ends, or a wake-up fires), which then parks.
    async fn wake_turn(&self, prompt: &str) {
        let before = self.ledger.prompts.lock().unwrap().len();
        let wake = self.ledger.wake.lock().unwrap().clone().unwrap();
        wake.send(prompt.into()).unwrap();
        wait_for(
            || {
                self.ledger.prompts.lock().unwrap().len() > before
                    && self.core.sessions.session_status(CHAT).map(|s| s.status)
                        == Some(SessionStatus::Idle)
            },
            "the wake turn to park",
        )
        .await;
    }

    fn live(&self) -> bool {
        self.core.sessions.live_run_steerable(CHAT)
    }

    fn kills(&self) -> usize {
        self.ledger.kills.load(Ordering::SeqCst)
    }

    async fn stopped(&self) {
        wait_for(
            || self.kills() == 1 && !self.live(),
            "the idle agent to be stopped",
        )
        .await;
    }
}

/// Just under, then just past, the idle window.
async fn idle_window() {
    tokio::time::sleep(AGENT_IDLE_STOP - Duration::from_secs(5)).await;
}

#[tokio::test(start_paused = true)]
async fn an_idle_agent_is_stopped_and_the_next_message_resumes_its_session() {
    let rig = rig(true);
    rig.send("hello").await;
    idle_window().await;
    assert!(rig.live(), "stopped before its idle window ran out");
    tokio::time::sleep(Duration::from_secs(10)).await;
    rig.stopped().await;

    rig.send("and again").await;
    assert_eq!(rig.ledger.spawns.load(Ordering::SeqCst), 2);
    assert_eq!(
        *rig.ledger.resumes.lock().unwrap(),
        [None, Some("provider-session".to_owned())],
        "the next message continues the same provider session"
    );
    rig.core.sessions.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn a_conversation_in_progress_stays_warm() {
    let rig = rig(true);
    rig.send("hello").await;
    // Replies inside the window keep the one process.
    for n in 0..5 {
        tokio::time::sleep(AGENT_IDLE_STOP - Duration::from_secs(10)).await;
        rig.send(&format!("reply {n}")).await;
    }
    assert_eq!(rig.ledger.spawns.load(Ordering::SeqCst), 1);
    assert_eq!(rig.kills(), 0);
    rig.core.sessions.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn an_agent_with_background_work_is_never_stopped() {
    let rig = rig(true);
    rig.send("start a bg build").await;
    // Hours of background work: never stopped.
    tokio::time::sleep(Duration::from_secs(3 * 60 * 60)).await;
    assert!(rig.live(), "stopped while its background work ran");
    assert_eq!(rig.kills(), 0);
    // The work ends: a full idle window from then, then stopped.
    let turn = rig.ledger.turn.lock().unwrap().clone().unwrap();
    turn.set_background(0);
    idle_window().await;
    assert!(rig.live(), "stopped as its background work ended");
    tokio::time::sleep(AGENT_IDLE_STOP * 2).await;
    rig.stopped().await;
    rig.core.sessions.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn an_agent_with_a_scheduled_wake_up_is_never_stopped() {
    let rig = rig(true);
    rig.send("wake me in an hour").await;
    tokio::time::sleep(Duration::from_secs(2 * 60 * 60)).await;
    assert!(rig.live(), "stopped with a wake-up pending");
    assert_eq!(rig.kills(), 0);
    rig.core.sessions.shutdown().await;
}

/// A `/loop` that rescheduled its wake-up twice and then stopped holds
/// nothing: each call replaced the last, and `stop` ended it.
#[tokio::test(start_paused = true)]
async fn a_wake_up_loop_that_stopped_lets_the_agent_stop() {
    let rig = rig(true);
    rig.send("wake:600").await;
    rig.send("wake:900").await;
    rig.send("wake:stop").await;
    tokio::time::sleep(AGENT_IDLE_STOP * 3).await;
    rig.stopped().await;
    rig.core.sessions.shutdown().await;
}

/// A background task's wake turn, long before the wake-up's time, isn't
/// its firing: the wake-up still holds the agent. Once its time (and grace)
/// has passed with no turn of its own, it holds nothing.
#[tokio::test(start_paused = true)]
async fn a_wake_turn_before_the_wake_up_keeps_it_pending() {
    let rig = rig(true);
    rig.send("wake:1800").await;
    tokio::time::sleep(Duration::from_secs(60)).await;
    rig.wake_turn("a background task ended").await;
    tokio::time::sleep(Duration::from_secs(20 * 60)).await;
    assert!(rig.live(), "stopped with its wake-up still to come");
    assert_eq!(rig.kills(), 0);
    tokio::time::sleep(Duration::from_secs(30 * 60)).await;
    rig.stopped().await;
    rig.core.sessions.shutdown().await;
}

/// The same cron deleted twice cancels it once: the other still holds.
#[tokio::test(start_paused = true)]
async fn a_cron_deleted_twice_still_leaves_the_other_holding() {
    let rig = rig(true);
    rig.send("cron:aaaa1111 cron:bbbb2222").await;
    rig.send("uncron:aaaa1111").await;
    rig.send("uncron:aaaa1111").await;
    tokio::time::sleep(Duration::from_secs(3 * 60 * 60)).await;
    assert!(rig.live(), "stopped with a cron still scheduled");
    assert_eq!(rig.kills(), 0);
    rig.send("uncron:bbbb2222").await;
    tokio::time::sleep(AGENT_IDLE_STOP * 3).await;
    rig.stopped().await;
    rig.core.sessions.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn an_agent_another_device_is_reading_stays_warm() {
    let rig = rig(true);
    rig.send("hello").await;
    let viewer = rig.core.sessions.retain_remote_viewer(CHAT);
    tokio::time::sleep(AGENT_IDLE_STOP * 10).await;
    assert!(rig.live(), "stopped while another device had the chat open");
    drop(viewer);
    tokio::time::sleep(AGENT_IDLE_STOP * 3).await;
    rig.stopped().await;
    rig.core.sessions.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn a_driver_that_does_not_opt_in_keeps_the_long_window() {
    let rig = rig(false);
    rig.send("hello").await;
    tokio::time::sleep(Duration::from_secs(29 * 60)).await;
    assert!(rig.live(), "stopped before the 30-minute window");
    tokio::time::sleep(Duration::from_secs(2 * 60)).await;
    rig.stopped().await;
    rig.core.sessions.shutdown().await;
}

/// One raw HTTP/1.1 POST to the engine's MCP endpoint: (status, body).
async fn post(url: &str, token: &str, body: &str) -> (u16, String) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let rest = url.strip_prefix("http://").unwrap();
    let (authority, path) = rest.split_once('/').unwrap();
    let mut stream = tokio::net::TcpStream::connect(authority).await.unwrap();
    let request = format!(
        "POST /{path} HTTP/1.1\r\nHost: {authority}\r\nAuthorization: Bearer {token}\r\n\
         Accept: application/json, text/event-stream\r\nContent-Type: application/json\r\n\
         Connection: close\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut raw = String::new();
    stream.read_to_string(&mut raw).await.unwrap();
    let status = raw.split_whitespace().nth(1).unwrap().parse().unwrap();
    (status, raw)
}

#[tokio::test]
async fn a_run_s_zeron_tools_come_from_the_engine_on_a_token_that_dies_with_it() {
    let rig = rig(true);
    rig.core
        .sessions
        .serve_mcp_endpoint(rig.core.rpc_service())
        .await
        .unwrap();
    rig.send("hello").await;
    let mcp = rig.ledger.mcp.lock().unwrap()[0]
        .clone()
        .expect("the host stamps Zeron's server");
    let http = mcp.http.clone().expect("served over HTTP by the engine");
    assert!(http.url.starts_with("http://127.0.0.1:"), "{}", http.url);
    let token = http.bearer_token().expect("a bearer token").to_owned();
    // Agents that speak only stdio get the small forwarder, not the engine.
    assert_eq!(mcp.args, ["mcp-proxy"]);
    assert_eq!(mcp.env.get(zeron_proto::McpServer::TOKEN_ENV), Some(&token));
    assert_eq!(
        mcp.env.get(zeron_proto::McpServer::URL_ENV),
        Some(&http.url)
    );
    assert!(
        !format!("{mcp:?}").contains(&token),
        "a token never reaches a log line"
    );

    // The token speaks as its run's chat.
    let call = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": { "name": "whoami", "arguments": {} },
    })
    .to_string();
    let (status, body) = post(&http.url, &token, &call).await;
    assert_eq!(status, 200, "{body}");
    assert!(body.contains(CHAT), "whoami names the run's chat: {body}");
    let (status, _) = post(&http.url, "not-a-token", &call).await;
    assert_eq!(status, 401);

    // The run ends: its token with it.
    rig.core.sessions.terminate(CHAT).await.unwrap();
    wait_for(|| !rig.live(), "the run to end").await;
    let mut refused = 0;
    for _ in 0..100 {
        if post(&http.url, &token, &call).await.0 == 401 {
            refused += 1;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(refused, 1, "an ended run's token still worked");

    // The next run gets a token of its own.
    rig.send("again").await;
    let next = rig.ledger.mcp.lock().unwrap()[1].clone().unwrap();
    let next_token = next.http.unwrap().bearer_token().unwrap().to_owned();
    assert_ne!(next_token, token);
    rig.core.sessions.shutdown().await;
}

/// A real process standing in for the agent CLI, with one child: `sleep
/// <secs>` (a background shell, or an MCP server). Killed on drop.
#[cfg(unix)]
struct AgentProcess(std::process::Child);

#[cfg(unix)]
impl AgentProcess {
    fn with_child(secs: &str) -> Self {
        Self::with_children(&[secs])
    }

    /// One `sleep <secs>` child for each.
    fn with_children(secs: &[&str]) -> Self {
        let script: String = secs
            .iter()
            .map(|s| format!("sleep {s} & "))
            .collect::<String>()
            + "wait";
        let agent = std::process::Command::new("/bin/sh")
            .args(["-c", &script])
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while zeron_harness::process::unexpected_descendants(agent.id(), &[]).len() < secs.len() {
            assert!(
                std::time::Instant::now() < deadline,
                "the child never started"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        Self(agent)
    }

    /// Leading its own process group, as Zeron starts agents, with no child
    /// left: its shell ran `sleep <secs> &` and exited, so the sleep is
    /// re-parented to init but still in the agent's group.
    fn with_orphan_in_its_group(secs: &str) -> Self {
        use std::os::unix::process::CommandExt as _;
        let agent = std::process::Command::new("/bin/sh")
            .args([
                "-c",
                &format!("/bin/sh -c 'sleep {secs} >/dev/null 2>&1 &'; read _"),
            ])
            .stdin(std::process::Stdio::piped())
            .process_group(0)
            .spawn()
            .unwrap();
        let group = agent.id() as i32;
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            // `ppid pgid stat args` of every process.
            let out = std::process::Command::new("ps")
                .args(["-A", "-o", "ppid=,pgid=,stat=,args="])
                .output()
                .unwrap();
            let rows: Vec<(i32, i32, String, String)> = String::from_utf8_lossy(&out.stdout)
                .lines()
                .filter_map(|line| {
                    let mut cols = line.split_whitespace();
                    let ppid = cols.next()?.parse().ok()?;
                    let pgid = cols.next()?.parse().ok()?;
                    let stat = cols.next()?.to_owned();
                    Some((ppid, pgid, stat, cols.collect::<Vec<_>>().join(" ")))
                })
                .filter(|(_, _, stat, _)| !stat.starts_with('Z'))
                .collect();
            // The sleep in the group, and its shell gone.
            let orphan = rows.iter().any(|(ppid, pgid, _, args)| {
                *pgid == group && *ppid != group && args == &format!("sleep {secs}")
            });
            if orphan && !rows.iter().any(|(ppid, ..)| *ppid == group) {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the orphan never appeared"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        Self(agent)
    }
}

#[cfg(unix)]
impl Drop for AgentProcess {
    fn drop(&mut self) {
        // SAFETY: the test's own process group: `sh` and its `sleep`.
        let pid = self.0.id() as i32;
        for child in zeron_harness::process::unexpected_descendants(pid as u32, &[]) {
            if let Some(child) = child
                .split_whitespace()
                .next()
                .and_then(|p| p.parse::<i32>().ok())
            {
                unsafe { libc::kill(child, libc::SIGKILL) };
            }
        }
        // SAFETY: as above; a group only when the agent leads its own.
        unsafe {
            if libc::getpgid(pid) == pid {
                libc::kill(-pid, libc::SIGKILL);
            }
        }
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// The agent says it holds no background work, but its process still runs
/// a child nobody configured (a background shell it never reported): never
/// stopped, however long.
#[cfg(unix)]
#[tokio::test(start_paused = true)]
async fn an_agent_whose_process_runs_an_unreported_child_is_never_stopped() {
    let rig = rig(true);
    let agent = AgentProcess::with_child("3603");
    *rig.ledger.process.lock().unwrap() = Some(agent.0.id());
    rig.send("hello").await;
    assert!(
        !rig.ledger
            .turn
            .lock()
            .unwrap()
            .clone()
            .unwrap()
            .background_live()
    );
    tokio::time::sleep(Duration::from_secs(3 * 60 * 60)).await;
    assert!(
        rig.live(),
        "stopped while its process ran an unreported child"
    );
    assert_eq!(rig.kills(), 0);
    drop(agent);
    rig.core.sessions.shutdown().await;
}

/// The agent's shell ran `cmd &` and exited: the process it left has no
/// parent under the agent any more, but it's in the agent's process group,
/// which a stop would signal. Never stopped, however long.
#[cfg(unix)]
#[tokio::test(start_paused = true)]
async fn an_orphan_left_in_the_agents_process_group_holds_the_stop() {
    let rig = rig(true);
    let agent = AgentProcess::with_orphan_in_its_group("3609");
    *rig.ledger.process.lock().unwrap() = Some(agent.0.id());
    rig.send("hello").await;
    tokio::time::sleep(Duration::from_secs(3 * 60 * 60)).await;
    assert!(
        rig.live(),
        "stopped while an orphan of its shell ran in its process group"
    );
    assert_eq!(rig.kills(), 0);
    drop(agent);
    rig.core.sessions.shutdown().await;
}

/// One whose only child is the MCP server its run configured is idle, and
/// is stopped.
#[cfg(unix)]
#[tokio::test(start_paused = true)]
async fn an_agent_whose_only_child_is_its_mcp_server_is_stopped() {
    let rig = rig(true);
    let agent = AgentProcess::with_child("3604");
    *rig.ledger.process.lock().unwrap() = Some(agent.0.id());
    let mut first = request("hello");
    first.mcp = Some(zeron_proto::McpServer {
        name: "zeron".into(),
        command: "sleep".into(),
        args: vec!["3604".into()],
        ..Default::default()
    });
    rig.send_request(first).await;
    tokio::time::sleep(AGENT_IDLE_STOP + Duration::from_secs(10)).await;
    rig.stopped().await;
    drop(agent);
    rig.core.sessions.shutdown().await;
}

/// A run whose user configured an MCP server of their own (user scope, in a
/// config dir of the test's): `config` holds `.claude.json` as written.
#[cfg(unix)]
fn with_user_config(prompt: &str, config: &std::path::Path, claude_json: &str) -> RunRequest {
    std::fs::create_dir_all(config).unwrap();
    std::fs::write(config.join(".claude.json"), claude_json).unwrap();
    let mut run = request(prompt);
    run.env.insert(
        "CLAUDE_CONFIG_DIR".into(),
        config.to_string_lossy().into_owned(),
    );
    run
}

#[cfg(unix)]
const USER_SERVER: &str =
    r#"{"mcpServers":{"mine":{"command":"sleep","args":["3605"],"env":{"TOKEN":"x"}}}}"#;

/// The user's own MCP server (from their Claude config) is the agent's only
/// child: recognised, so the idle agent is stopped.
#[cfg(unix)]
#[tokio::test(start_paused = true)]
async fn an_agent_whose_only_child_is_a_server_from_its_user_s_config_is_stopped() {
    let rig = rig(true);
    let config = tempfile::tempdir().unwrap();
    let agent = AgentProcess::with_child("3605");
    *rig.ledger.process.lock().unwrap() = Some(agent.0.id());
    rig.send_request(with_user_config("hello", config.path(), USER_SERVER))
        .await;
    tokio::time::sleep(AGENT_IDLE_STOP + Duration::from_secs(10)).await;
    rig.stopped().await;
    drop(agent);
    rig.core.sessions.shutdown().await;
}

/// The same server, beside a background shell nobody configured: held.
#[cfg(unix)]
#[tokio::test(start_paused = true)]
async fn a_background_shell_beside_a_configured_server_still_holds() {
    let rig = rig(true);
    let config = tempfile::tempdir().unwrap();
    let agent = AgentProcess::with_children(&["3605", "3606"]);
    *rig.ledger.process.lock().unwrap() = Some(agent.0.id());
    rig.send_request(with_user_config("hello", config.path(), USER_SERVER))
        .await;
    tokio::time::sleep(Duration::from_secs(3 * 60 * 60)).await;
    assert!(rig.live(), "stopped with an unconfigured child running");
    assert_eq!(rig.kills(), 0);
    drop(agent);
    rig.core.sessions.shutdown().await;
}

/// A config that can't be read recognises nothing: its server holds the
/// stop, the safe side.
#[cfg(unix)]
#[tokio::test(start_paused = true)]
async fn a_malformed_user_config_holds() {
    let rig = rig(true);
    let config = tempfile::tempdir().unwrap();
    let agent = AgentProcess::with_child("3605");
    *rig.ledger.process.lock().unwrap() = Some(agent.0.id());
    let broken = &USER_SERVER[..USER_SERVER.len() - 3];
    rig.send_request(with_user_config("hello", config.path(), broken))
        .await;
    tokio::time::sleep(Duration::from_secs(3 * 60 * 60)).await;
    assert!(
        rig.live(),
        "stopped although its child couldn't be recognised"
    );
    assert_eq!(rig.kills(), 0);
    drop(agent);
    rig.core.sessions.shutdown().await;
}
