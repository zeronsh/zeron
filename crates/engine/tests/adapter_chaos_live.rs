//! Opt-in adapter chaos suite against the REAL agent CLIs: the races and edge
//! shapes where turn-end detection and runtime survival break, each checked
//! against hard invariants rather than happy-path output.
//!
//! ZERON_TEST_HARNESS=claude ZERON_TEST_MODEL=claude-sonnet-5-5 \
//! ZERON_TEST_OTHER_MODEL=claude-haiku-4-5 ZERON_CHAOS_ROUNDS=3 \
//!   cargo test -p zeron-engine --test adapter_chaos_live -- --ignored --nocapture
//!
//! `ZERON_CHAOS_ONLY=<scenario>` runs one scenario. Invariants, per chat:
//! - Never "done" early: no agent output after a Done unless a new user turn
//!   began first (a steer boundary or a fresh process); no Idle→Working flip
//!   the user did not cause; every requested sentinel present at the end.
//! - Never stuck: every scenario settles Idle.
//! - Every routed message is delivered (one steer boundary each).
//! - One process for the chat's life, except after the user's Stop.
//! - Nothing the agent started outlives a Stop.
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use zeron_doc::SessionCommandPayload;
use zeron_engine::{EngineCore, HarnessRegistry};
use zeron_harness::{ClaudeHarness, CodexHarness, Harness};
use zeron_proto::{
    AgentEvent, ChatConfig, HarnessId, ReasoningLevel, RunRequest, SandboxLevel, SessionStatus,
};

struct Suite {
    core: EngineCore,
    id: HarnessId,
    name: String,
    model: Option<String>,
    other_model: Option<String>,
    cwd: String,
    dir: tempfile::TempDir,
    /// Unique per harness so parallel suites never count each other's work.
    sleep_base: u32,
}

/// What one scenario did and saw.
struct Chat<'a> {
    suite: &'a Suite,
    id: String,
    sends: Arc<Mutex<Vec<Instant>>>,
    /// (when, status, last_completed_turn) at every observed change.
    timeline: Arc<Mutex<Vec<(Instant, SessionStatus, Option<String>)>>>,
    sentinels: Vec<String>,
    /// Every message sent: each must start a steer boundary or a process.
    runs_sent: usize,
    stops: usize,
    monitor: tokio::task::JoinHandle<()>,
    errors: Vec<String>,
}

impl Suite {
    fn new() -> Self {
        let name = std::env::var("ZERON_TEST_HARNESS").unwrap_or_else(|_| "claude".into());
        let (id, harness, sleep_base): (HarnessId, Arc<dyn Harness>, u32) = match name.as_str() {
            "claude" => (HarnessId::ClaudeCode, Arc::new(ClaudeHarness::new()), 270),
            "codex" => (HarnessId::Codex, Arc::new(CodexHarness::new()), 370),
            _ => panic!("claude or codex"),
        };
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_str().unwrap().to_owned();
        let registry = HarnessRegistry::new();
        registry.register(harness);
        let core =
            EngineCore::assemble(&dir.path().join("engine"), Arc::new(registry), id, None).unwrap();
        core.workspace
            .create_space("space", &core.device_id, &cwd, None, false)
            .unwrap();
        Self {
            core,
            id,
            name,
            model: std::env::var("ZERON_TEST_MODEL").ok(),
            other_model: std::env::var("ZERON_TEST_OTHER_MODEL").ok(),
            cwd,
            dir,
            sleep_base,
        }
    }

    fn request(&self, prompt: &str) -> RunRequest {
        RunRequest {
            mcp: None,
            prompt: prompt.into(),
            harness: Some(self.id),
            model: self.model.clone(),
            reasoning: None,
            model_options: Default::default(),
            cwd: self.cwd.clone(),
            sandbox: SandboxLevel::DangerFullAccess,
            auto_approve: true,
            attachments: vec![],
            worktree: None,
            resume: None,
        }
    }

    /// A probe command: sleeps `secs`, tagged with a marker unique to this
    /// suite and slot so it can be found in the process table.
    fn probe(&self, slot: u32, secs: u32) -> String {
        format!(
            "python3 -c 'import time; zq{}=1; time.sleep({secs})'",
            self.sleep_base + slot
        )
    }

    fn probe_marker(&self, slot: u32) -> String {
        format!("zq{}=1", self.sleep_base + slot)
    }

    /// Processes still running one of this suite's probe commands.
    fn probes_alive(&self) -> usize {
        let pattern = format!("zq{}[0-9]=1", self.sleep_base / 10);
        std::process::Command::new("pgrep")
            .args(["-f", &pattern])
            .output()
            .map(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .split_whitespace()
                    .count()
            })
            .unwrap_or(0)
    }

    fn kill_probes(&self) {
        let pattern = format!("zq{}[0-9]=1", self.sleep_base / 10);
        let _ = std::process::Command::new("pkill")
            .args(["-f", &pattern])
            .status();
    }

    fn chat(&self, scenario: &str, round: usize) -> Chat<'_> {
        let id = format!("{scenario}-{round}");
        self.core
            .workspace
            .create_chat(&id, Some("space"), None, None, None)
            .unwrap();
        self.core
            .workspace
            .set_chat_config(
                &id,
                &ChatConfig {
                    harness: self.id,
                    model: self.model.clone(),
                    reasoning: None,
                    model_options: Default::default(),
                    sandbox: SandboxLevel::DangerFullAccess,
                },
            )
            .unwrap();
        let timeline = Arc::new(Mutex::new(Vec::new()));
        let monitor = {
            let sessions = self.core.sessions.clone();
            let timeline = timeline.clone();
            let chat = id.clone();
            tokio::spawn(async move {
                let mut last = None;
                loop {
                    let now = sessions
                        .session_status(&chat)
                        .map(|s| (s.status, s.last_completed_turn));
                    if now.is_some() && now != last {
                        let (status, ping) = now.clone().unwrap();
                        timeline
                            .lock()
                            .unwrap()
                            .push((Instant::now(), status, ping));
                        last = now;
                    }
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
        };
        Chat {
            suite: self,
            id,
            sends: Default::default(),
            timeline,
            sentinels: Vec::new(),
            runs_sent: 0,
            stops: 0,
            monitor,
            errors: Vec::new(),
        }
    }
}

impl Chat<'_> {
    fn sentinel(&mut self, tag: &str) -> String {
        let token = format!("ZQ{}{}", tag, uuid::Uuid::new_v4().simple())
            .chars()
            .take(14)
            .collect::<String>()
            .to_uppercase();
        self.sentinels.push(token.clone());
        token
    }

    fn status(&self) -> Option<SessionStatus> {
        self.suite
            .core
            .sessions
            .session_status(&self.id)
            .map(|s| s.status)
    }

    fn events(&self) -> Vec<AgentEvent> {
        self.suite
            .core
            .sessions
            .subscribe(&self.id, 0)
            .map(|(events, _)| events.into_iter().map(|e| e.event).collect())
            .unwrap_or_default()
    }

    fn text(&self) -> String {
        self.events()
            .iter()
            .filter_map(|e| match e {
                AgentEvent::TextDelta { text } => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }

    fn tool_started(&self, needle: &str) -> bool {
        self.events().iter().any(|e| {
            matches!(e, AgentEvent::ToolCall { call, .. }
                if serde_json::to_string(call).is_ok_and(|c| c.contains(needle)))
        })
    }

    /// The user's send, through the durable command plane like every client.
    fn send(&mut self, request: RunRequest) {
        self.runs_sent += 1;
        self.sends.lock().unwrap().push(Instant::now());
        self.suite
            .core
            .doc_host
            .queue_command(
                &self.id,
                SessionCommandPayload::Run {
                    request,
                    message_id: uuid::Uuid::new_v4().to_string(),
                },
            )
            .unwrap();
    }

    fn say(&mut self, prompt: &str) {
        let request = self.suite.request(prompt);
        self.send(request);
    }

    fn stop(&mut self) {
        self.stops += 1;
        self.sends.lock().unwrap().push(Instant::now());
        self.suite
            .core
            .doc_host
            .queue_command(&self.id, SessionCommandPayload::Interrupt {})
            .unwrap();
    }

    async fn until(
        &mut self,
        secs: u64,
        what: &str,
        mut predicate: impl FnMut(&Self) -> bool,
    ) -> bool {
        let deadline = Instant::now() + Duration::from_secs(secs);
        while !predicate(self) {
            if Instant::now() > deadline {
                self.errors.push(format!("timed out: {what}"));
                return false;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        true
    }

    async fn streaming(&mut self, what: &str) -> bool {
        let base = self.text().len();
        self.until(180, what, move |c| {
            c.status() == Some(SessionStatus::Working) && c.text().len() > base + 40
        })
        .await
    }

    /// Settled: Idle, and every sentinel asked for so far has arrived.
    async fn settled(&mut self, secs: u64) -> bool {
        let sentinels = self.sentinels.clone();
        self.until(
            secs,
            "the chat to settle Idle with every sentinel",
            move |c| {
                c.status() == Some(SessionStatus::Idle) && {
                    let text = c.text();
                    sentinels.iter().all(|s| text.contains(s.as_str()))
                }
            },
        )
        .await
    }

    /// Check every invariant; returns the violations.
    async fn verdict(mut self) -> Vec<String> {
        // Let trailing frames land before judging.
        tokio::time::sleep(Duration::from_millis(1500)).await;
        self.monitor.abort();
        let events = self.events();

        // Done followed by agent output with no new user turn in between.
        let mut after_done = None;
        for (i, e) in events.iter().enumerate() {
            match e {
                AgentEvent::Done { status, .. } => after_done = Some((i, *status)),
                AgentEvent::Steered { .. } | AgentEvent::SessionStarted { .. } => after_done = None,
                AgentEvent::TextDelta { text } if !text.trim().is_empty() => {
                    if let Some((at, status)) = after_done.take() {
                        self.errors.push(format!(
                            "output after Done({status:?}) at event {at} with no new turn: {text:?}"
                        ));
                    }
                }
                AgentEvent::ToolCall { id, .. } => {
                    if let Some((at, status)) = after_done.take() {
                        self.errors.push(format!(
                            "tool {id} after Done({status:?}) at event {at} with no new turn"
                        ));
                    }
                }
                _ => {}
            }
        }

        // Idle→Working without a user action in between.
        let sends = self.sends.lock().unwrap().clone();
        let timeline = self.timeline.lock().unwrap().clone();
        let mut idle_since: Option<Instant> = None;
        for (at, status, _) in &timeline {
            match status {
                SessionStatus::Idle => idle_since = Some(*at),
                SessionStatus::Working | SessionStatus::AwaitingInput => {
                    // A send queued just before the turn ended reaches the
                    // agent just after: allow the command plane's latency.
                    if let Some(idle) = idle_since.take()
                        && !sends
                            .iter()
                            .any(|s| *s <= *at && *s + Duration::from_secs(2) >= idle)
                    {
                        self.errors.push(format!(
                            "went Idle then Working again {:?} later with no user action (premature done)",
                            at.duration_since(idle)
                        ));
                    }
                }
                _ => {}
            }
        }

        // Every sentinel arrived; nothing stuck.
        let text = self.text();
        for s in &self.sentinels {
            if !text.contains(s.as_str()) {
                self.errors.push(format!("sentinel {s} never arrived"));
            }
        }
        if self.status() != Some(SessionStatus::Idle) {
            self.errors
                .push(format!("ended {:?}, not Idle", self.status()));
        }

        // Every message delivered (a steer boundary, or the process it
        // started); one process except after Stops.
        let steered = events
            .iter()
            .filter(|e| matches!(e, AgentEvent::Steered { .. }))
            .count();
        let processes = events
            .iter()
            .filter(|e| matches!(e, AgentEvent::SessionStarted { .. }))
            .count();
        if steered + processes < self.runs_sent {
            self.errors.push(format!(
                "{} messages sent but only {steered} steer boundaries + {processes} processes",
                self.runs_sent
            ));
        }
        if processes > 1 + self.stops {
            self.errors
                .push(format!("{processes} processes for {} stops", self.stops));
        }
        if self.events().iter().any(|e| {
            matches!(
                e,
                AgentEvent::Done {
                    status: zeron_proto::DoneStatus::Errored,
                    ..
                }
            )
        }) {
            self.errors.push("a turn errored".into());
        }
        if !self.errors.is_empty() {
            let journal: Vec<String> = events
                .iter()
                .filter(|e| {
                    !matches!(
                        e,
                        AgentEvent::TextDelta { .. }
                            | AgentEvent::ReasoningDelta { .. }
                            | AgentEvent::Usage { .. }
                            | AgentEvent::ContextUsage { .. }
                    )
                })
                .map(|e| format!("{e:?}").chars().take(160).collect())
                .collect();
            self.errors.push(format!("journal: {journal:#?}"));
        }
        self.errors
    }
}

fn pixel(dir: &std::path::Path) -> String {
    let path = dir.join(format!("pixel-{}.png", uuid::Uuid::new_v4().simple()));
    std::fs::write(
        &path,
        [
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00,
            0x00, 0x90, 0x77, 0x53, 0xDE, 0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41, 0x54, 0x08,
            0xD7, 0x63, 0xF8, 0xCF, 0xC0, 0x00, 0x00, 0x03, 0x01, 0x01, 0x00, 0x18, 0xDD, 0x8D,
            0xB0, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
        ],
    )
    .unwrap();
    path.display().to_string()
}

const ESSAY: &str = "Write a 500-word essay about rivers. Do not use any tools.";

async fn scenario(suite: &Suite, name: &str, round: usize) -> Vec<String> {
    let mut chat = suite.chat(name, round);
    match name {
        // Three messages within a second while the reply streams.
        "rapid_steers" => {
            chat.say(ESSAY);
            if chat.streaming("the essay to stream").await {
                let token = chat.sentinel("RS");
                chat.say("Remember the word ALPHA.");
                tokio::time::sleep(Duration::from_millis(300)).await;
                chat.say("Remember the word BRAVO.");
                tokio::time::sleep(Duration::from_millis(300)).await;
                chat.say(&format!(
                    "Stop the essay. Reply with exactly {token} followed by every word I asked you to remember. No tools."
                ));
                if chat.settled(240).await {
                    let text = chat.text();
                    let tail = &text[text.find(&token).unwrap_or(0)..];
                    for word in ["ALPHA", "BRAVO"] {
                        if !tail.contains(word) {
                            chat.errors
                                .push(format!("steer {word} never reached the agent"));
                        }
                    }
                }
            }
        }
        // A steer while a foreground command runs.
        "steer_during_tool" => {
            let first = chat.sentinel("TA");
            chat.say(&format!(
                "Run this exact shell command in the FOREGROUND and wait for it: {} . Then reply {first}.",
                suite.probe(1, 15)
            ));
            let marker = suite.probe_marker(1);
            if chat
                .until(180, "the command to start", |c| c.tool_started(&marker))
                .await
            {
                let second = chat.sentinel("TB");
                chat.say(&format!(
                    "When that is done, also reply {second} on its own line."
                ));
                chat.settled(240).await;
            }
        }
        // A send landing right as the reply finishes.
        "boundary_race" => {
            let first = chat.sentinel("BA");
            chat.say(&format!("Reply with exactly: {first} and nothing else."));
            if chat
                .until(120, "the first reply", |c| {
                    c.text().contains(first.as_str())
                })
                .await
            {
                let second = chat.sentinel("BB");
                chat.say(&format!("Reply with exactly: {second} and nothing else."));
                chat.settled(180).await;
            }
            // And a send right after Idle.
            let third = chat.sentinel("BC");
            chat.say(&format!("Reply with exactly: {third} and nothing else."));
            chat.settled(180).await;
        }
        // Stop the instant after sending, then send again at once.
        "stop_races" => {
            chat.say(ESSAY);
            chat.stop();
            // The Stop may cancel this message before its agent even starts.
            chat.runs_sent -= 1;
            chat.until(60, "the instant stop to settle", |c| {
                c.status() == Some(SessionStatus::Idle)
            })
            .await;
            let token = chat.sentinel("SR");
            chat.say(&format!("Reply with exactly: {token} and nothing else."));
            chat.settled(180).await;
            // Stop mid-command, and send immediately after the Stop.
            chat.say(&format!(
                "Run this exact shell command in the FOREGROUND and wait for it: {} . Then reply FINISHED.",
                suite.probe(2, 300)
            ));
            let marker = suite.probe_marker(2);
            if chat
                .until(180, "the command to start", |c| c.tool_started(&marker))
                .await
            {
                chat.stop();
                let token = chat.sentinel("SS");
                chat.say(&format!("Reply with exactly: {token} and nothing else."));
                chat.settled(180).await;
                if !chat
                    .until(20, "the stopped command's process to be gone", |c| {
                        c.suite.probes_alive() == 0
                    })
                    .await
                {
                    chat.errors
                        .push("a Stop left the agent's command running".into());
                }
            }
        }
        // An image sent while the agent works.
        "image_mid_turn" => {
            chat.say(ESSAY);
            if chat.streaming("the essay to stream").await {
                let token = chat.sentinel("IM");
                let mut request = suite.request(&format!(
                    "Stop the essay. I attached a 1x1 test image. Reply with exactly {token} and nothing else. No tools."
                ));
                request.attachments = vec![pixel(suite.dir.path())];
                chat.send(request);
                chat.settled(240).await;
            }
        }
        // Another model chosen for a message sent mid-turn.
        "model_switch_mid_turn" => {
            let Some(other) = suite.other_model.clone() else {
                return Vec::new();
            };
            chat.say(ESSAY);
            if chat.streaming("the essay to stream").await {
                let token = chat.sentinel("MS");
                let mut request = suite.request(&format!(
                    "Stop the essay. Reply with exactly {token} and nothing else. No tools."
                ));
                request.model = Some(other.clone());
                chat.send(request);
                if chat.settled(240).await {
                    let token = chat.sentinel("MT");
                    let mut request =
                        suite.request(&format!("Reply with exactly {token} and nothing else."));
                    request.model = Some(other);
                    chat.send(request);
                    chat.settled(180).await;
                }
            }
        }
        // Long silent reasoning must not read as a dead turn.
        "long_reasoning" => {
            let token = chat.sentinel("LR");
            let mut request = suite.request(&format!(
                "Think carefully step by step (privately) about this: how many distinct ways can you make change for 100 cents using pennies, nickels, dimes and quarters? Do not use tools. Then reply {token} followed by the number."
            ));
            request.reasoning = Some(ReasoningLevel::Max);
            chat.send(request);
            chat.settled(600).await;
        }
        // A steer while a subagent works.
        "subagent_steer" => {
            let delegate = if suite.id == HarnessId::Codex {
                "Spawn one subagent (spawn_agent) and wait for it"
            } else {
                "Use the Agent tool (foreground, general-purpose subagent) and wait for it"
            };
            let first = chat.sentinel("SA");
            chat.say(&format!(
                "{delegate}: its task is to run this exact shell command in the foreground: {} and then reply ok. After it returns, reply {first}.",
                suite.probe(3, 20)
            ));
            if chat
                .until(240, "the subagent's command to start", |c| {
                    c.suite.probes_alive() > 0
                })
                .await
            {
                let second = chat.sentinel("SB");
                chat.say(&format!(
                    "Also, at the very end, reply {second} on its own line."
                ));
                chat.settled(300).await;
            }
        }
        // The agent asks; the answer resumes the same turn, which ends once.
        "question" => {
            if suite.id != HarnessId::ClaudeCode {
                return Vec::new();
            }
            let token = chat.sentinel("QA");
            chat.say(&format!(
                "Use the AskUserQuestion tool to ask me to pick exactly one of two colors, red or blue. After I answer, reply {token} followed by my choice. No other tools."
            ));
            if chat
                .until(180, "the question", |c| {
                    c.status() == Some(SessionStatus::AwaitingInput)
                })
                .await
            {
                let asked = chat.events().into_iter().rev().find_map(|e| match e {
                    AgentEvent::InputRequested {
                        request_id,
                        questions,
                    } => Some((request_id, questions)),
                    _ => None,
                });
                if let Some((request_id, questions)) = asked {
                    let answers = questions
                        .iter()
                        .map(|q| zeron_proto::UserInputAnswer {
                            question_id: q.id.clone(),
                            labels: q.options.iter().take(1).cloned().collect(),
                        })
                        .collect();
                    chat.sends.lock().unwrap().push(Instant::now());
                    if !suite
                        .core
                        .sessions
                        .respond_input(&chat.id, &request_id, answers)
                        .unwrap()
                    {
                        chat.errors
                            .push("the answer found no pending question".into());
                    }
                    chat.settled(180).await;
                } else {
                    chat.errors
                        .push("AwaitingInput without an InputRequested event".into());
                }
            }
        }
        // A steer, then Stop at once: both cancelled, the work killed, and
        // the next message runs.
        "steer_then_stop" => {
            chat.say(&format!(
                "Run this exact shell command in the FOREGROUND and wait for it: {} . Then reply FINISHED.",
                suite.probe(4, 300)
            ));
            let marker = suite.probe_marker(4);
            if chat
                .until(180, "the command to start", |c| c.tool_started(&marker))
                .await
            {
                chat.say("Also list the files in this directory afterwards.");
                chat.stop();
                chat.until(60, "the stop to settle", |c| {
                    c.status() == Some(SessionStatus::Idle)
                })
                .await;
                if !chat
                    .until(20, "the stopped command to be gone", |c| {
                        c.suite.probes_alive() == 0
                    })
                    .await
                {
                    chat.errors
                        .push("a Stop left the agent's command running".into());
                }
                let token = chat.sentinel("ST");
                chat.say(&format!("Reply with exactly: {token} and nothing else."));
                chat.settled(180).await;
                // The cancelled steer must not have run in the new process.
                chat.runs_sent -= 1;
            }
        }
        other => panic!("unknown scenario {other}"),
    }
    let errors = chat.verdict().await;
    suite.kill_probes();
    errors
}

const SCENARIOS: &[&str] = &[
    "rapid_steers",
    "steer_during_tool",
    "boundary_race",
    "stop_races",
    "image_mid_turn",
    "model_switch_mid_turn",
    "long_reasoning",
    "subagent_steer",
    "question",
    "steer_then_stop",
];

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "uses real model quota; select harness and inexpensive model explicitly"]
async fn adapter_chaos() {
    let suite = Suite::new();
    suite.kill_probes();
    let rounds: usize = std::env::var("ZERON_CHAOS_ROUNDS")
        .ok()
        .and_then(|r| r.parse().ok())
        .unwrap_or(1);
    let only = std::env::var("ZERON_CHAOS_ONLY").ok();
    let mut failures = Vec::new();
    let mut runs = 0;
    for round in 0..rounds {
        for name in SCENARIOS {
            if only.as_deref().is_some_and(|o| o != *name) {
                continue;
            }
            let started = Instant::now();
            let errors = scenario(&suite, name, round).await;
            runs += 1;
            let verdict = if errors.is_empty() { "ok" } else { "FAILED" };
            println!(
                "{}: round {round} {name}: {verdict} ({:.0}s)",
                suite.name,
                started.elapsed().as_secs_f32()
            );
            for e in &errors {
                println!("    {e}");
            }
            if !errors.is_empty() {
                failures.push(format!("round {round} {name}"));
            }
        }
    }
    suite.core.shutdown().await;
    suite.kill_probes();
    println!(
        "{}: {}/{runs} scenario runs passed",
        suite.name,
        runs - failures.len()
    );
    assert!(failures.is_empty(), "failed: {failures:?}");
}
