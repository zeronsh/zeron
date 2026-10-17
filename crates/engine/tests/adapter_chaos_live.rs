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
//!
//! `ZERON_TEST_HARNESS`: claude | codex | cursor | opencode | grok | devin |
//! pi | hermes | antigravity. `restart_durability` covers quitting and
//! force-killing the app mid-turn (same environment variables).
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use zeron_doc::SessionCommandPayload;
use zeron_engine::{EngineCore, HarnessRegistry};
use zeron_harness::{
    AcpHarness, ClaudeHarness, CodexHarness, CursorHarness, Harness, OpencodeHarness, PiHarness,
};
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
    /// User Stops, and agent deaths this scenario caused: each may cost a
    /// process.
    stops: usize,
    /// The scenario kills the agent: its turn may end errored.
    allow_errored: bool,
    monitor: tokio::task::JoinHandle<()>,
    errors: Vec<String>,
}

/// The selected harness: its id, a fresh driver, and a probe-marker base
/// unique to it (so parallel suites never count each other's work).
fn harness(name: &str) -> (HarnessId, Arc<dyn Harness>, u32) {
    match name {
        "claude" => (HarnessId::ClaudeCode, Arc::new(ClaudeHarness::new()), 270),
        "codex" => (HarnessId::Codex, Arc::new(CodexHarness::new()), 370),
        "cursor" => (HarnessId::Cursor, Arc::new(CursorHarness::new()), 470),
        "opencode" => (HarnessId::Opencode, Arc::new(OpencodeHarness::new()), 570),
        "grok" => (HarnessId::Grok, Arc::new(AcpHarness::grok()), 670),
        "devin" => (HarnessId::Devin, Arc::new(AcpHarness::devin()), 770),
        "pi" => (HarnessId::Pi, Arc::new(PiHarness::new()), 870),
        "hermes" => (HarnessId::Hermes, Arc::new(AcpHarness::hermes()), 970),
        "antigravity" => (
            HarnessId::Antigravity,
            Arc::new(AcpHarness::antigravity()),
            1070,
        ),
        other => panic!("unknown harness {other}"),
    }
}

fn engine(dir: &std::path::Path, name: &str) -> EngineCore {
    let (id, harness, _) = harness(name);
    let registry = HarnessRegistry::new();
    registry.register(harness);
    EngineCore::assemble(&dir.join("engine"), Arc::new(registry), id, None).unwrap()
}

impl Suite {
    fn new() -> Self {
        let name = std::env::var("ZERON_TEST_HARNESS").unwrap_or_else(|_| "claude".into());
        let (id, _, sleep_base) = harness(&name);
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().to_str().unwrap().to_owned();
        let core = engine(dir.path(), &name);
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
            resume_policy: Default::default(),
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

    /// SIGKILL every agent process this test runs, whole trees (an agent
    /// crash: OOM, `killall`). Only the current scenario's chat holds one:
    /// each scenario retires its chat's runtime when it ends.
    fn crash_agents(&self) -> usize {
        let mut tree = descendants(std::process::id());
        tree.reverse();
        for pid in &tree {
            let _ = std::process::Command::new("kill")
                .args(["-9", &pid.to_string()])
                .status();
        }
        tree.len()
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
            allow_errored: false,
            monitor,
            errors: Vec::new(),
        }
    }
}

impl Chat<'_> {
    fn sentinel(&mut self, tag: &str) -> String {
        let token = sentinel_token(tag);
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

        // A completion ping followed by more work with no user action in
        // between. (An Idle without a ping is a handoff — a turn-boundary
        // agent taking the message held for its turn end — not "done".)
        let sends = self.sends.lock().unwrap().clone();
        let timeline = self.timeline.lock().unwrap().clone();
        let mut idle_since: Option<Instant> = None;
        let mut last_ping: Option<String> = None;
        for (at, status, ping) in &timeline {
            let pinged = ping.is_some() && *ping != last_ping;
            if ping.is_some() {
                last_ping = ping.clone();
            }
            match status {
                SessionStatus::Idle if pinged => idle_since = Some(*at),
                SessionStatus::Idle => {}
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
        if !self.allow_errored
            && self.events().iter().any(|e| {
                matches!(
                    e,
                    AgentEvent::Done {
                        status: zeron_proto::DoneStatus::Errored,
                        ..
                    }
                )
            })
        {
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

/// A unique, word-like token to ask the agent to echo. Random hex reads as a
/// secret or an injection to some models (Grok declines to repeat it).
fn sentinel_token(tag: &str) -> String {
    const WORDS: &[&str] = &[
        "CEDAR", "MANGO", "RIVER", "TULIP", "OTTER", "MAPLE", "COMET", "BISON", "LEMON", "WALNUT",
        "HARBOR", "PEPPER",
    ];
    let id = uuid::Uuid::new_v4().as_u128();
    let word = WORDS[(id % WORDS.len() as u128) as usize];
    format!("{word}{}{}", 100 + (id >> 8) % 900, tag.to_uppercase())
}

/// Every process below `root`, parents before children.
fn descendants(root: u32) -> Vec<u32> {
    let mut out = Vec::new();
    let mut frontier = vec![root];
    while let Some(pid) = frontier.pop() {
        let children = std::process::Command::new("pgrep")
            .args(["-P", &pid.to_string()])
            .output()
            .map(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .split_whitespace()
                    .filter_map(|p| p.parse::<u32>().ok())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for child in children {
            out.push(child);
            frontier.push(child);
        }
    }
    out
}

/// The answer to every question an approval or ask raises: an allowing
/// option where one is offered, else the first.
fn answer_all(questions: &[zeron_proto::UserInputQuestion]) -> Vec<zeron_proto::UserInputAnswer> {
    questions
        .iter()
        .map(|q| {
            let allow = q.options.iter().find(|o| {
                let o = o.to_lowercase();
                (o.contains("allow")
                    || o.contains("yes")
                    || o.contains("approve")
                    || o.contains("accept"))
                    && !o.contains("don't")
                    && !o.contains("deny")
                    && !o.contains("reject")
            });
            zeron_proto::UserInputAnswer {
                question_id: q.id.clone(),
                labels: allow.or(q.options.first()).cloned().into_iter().collect(),
            }
        })
        .collect()
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
                    "Stop the essay. Say hi to my cat {token}, then list every word I asked you to remember. No tools."
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
                "Run this exact shell command in the FOREGROUND and wait for it: {} . Then say hi to my cat {first}.",
                suite.probe(1, 15)
            ));
            let marker = suite.probe_marker(1);
            if chat
                .until(180, "the command to start", |c| c.tool_started(&marker))
                .await
            {
                let second = chat.sentinel("TB");
                chat.say(&format!(
                    "When that is done, also say hi to my cat {second}."
                ));
                chat.settled(240).await;
            }
        }
        // A send landing right as the reply finishes.
        "boundary_race" => {
            let first = chat.sentinel("BA");
            chat.say(&format!(
                "Just say hi to my cat {first} in one short sentence."
            ));
            if chat
                .until(120, "the first reply", |c| {
                    c.text().contains(first.as_str())
                })
                .await
            {
                let second = chat.sentinel("BB");
                chat.say(&format!(
                    "Just say hi to my cat {second} in one short sentence."
                ));
                chat.settled(180).await;
            }
            // And a send right after Idle.
            let third = chat.sentinel("BC");
            chat.say(&format!(
                "Just say hi to my cat {third} in one short sentence."
            ));
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
            chat.say(&format!(
                "Just say hi to my cat {token} in one short sentence."
            ));
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
                chat.say(&format!(
                    "Just say hi to my cat {token} in one short sentence."
                ));
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
                    "Stop the essay. I attached a 1x1 test image. Just say hi to my cat {token} in one short sentence. No tools."
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
                    "Stop the essay. Just say hi to my cat {token} in one short sentence. No tools."
                ));
                request.model = Some(other.clone());
                chat.send(request);
                if chat.settled(240).await {
                    let token = chat.sentinel("MT");
                    let mut request = suite.request(&format!(
                        "Just say hi to my cat {token} in one short sentence."
                    ));
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
                "Think carefully step by step (privately) about this: how many distinct ways can you make change for 100 cents using pennies, nickels, dimes and quarters? Do not use tools. Then say hi to my cat {token} and tell me the number."
            ));
            request.reasoning = Some(ReasoningLevel::Max);
            chat.send(request);
            chat.settled(600).await;
        }
        // A steer while a subagent works.
        "subagent_steer" => {
            let delegate = match suite.id {
                HarnessId::Codex => "Spawn one subagent (spawn_agent) and wait for it",
                HarnessId::ClaudeCode => {
                    "Use the Agent tool (foreground, general-purpose subagent) and wait for it"
                }
                _ => {
                    "Delegate to one subagent (your task/subagent tool, if you have one; otherwise do it yourself) and wait for it"
                }
            };
            let first = chat.sentinel("SA");
            chat.say(&format!(
                "{delegate}: its task is to run this exact shell command in the foreground: {} and then reply ok. After it returns, say hi to my cat {first}.",
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
                    "Also, at the very end, say hi to my cat {second}."
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
                "Use the AskUserQuestion tool to ask me to pick exactly one of two colors, red or blue. After I answer, say hi to my cat {token} and tell me my choice. No other tools."
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
                    let answers = answer_all(&questions);
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
                chat.say(&format!(
                    "Just say hi to my cat {token} in one short sentence."
                ));
                chat.settled(180).await;
                // The cancelled steer must not have run in the new process.
                chat.runs_sent -= 1;
            }
        }
        // Several messages queued at once on an idle chat: every one runs,
        // in the order sent.
        "burst_idle" => {
            let first = chat.sentinel("BU");
            chat.say(&format!(
                "Just say hi to my cat {first} in one short sentence."
            ));
            if chat.settled(180).await {
                // However the agent takes them — one turn each, or together —
                // every message must reach it.
                let words = ["ALPHA", "BRAVO", "CHARLIE"];
                for word in words {
                    chat.say(&format!("Remember the word {word}. Reply only OK."));
                }
                let last = chat.sentinel("BZ");
                chat.say(&format!(
                    "Say hi to my cat {last}, then list every word I asked you to remember. No tools."
                ));
                if chat.settled(300).await {
                    let text = chat.text();
                    let tail = &text[text.rfind(last.as_str()).unwrap_or(0)..];
                    for word in words {
                        if !tail.contains(word) {
                            chat.errors
                                .push(format!("queued message {word} never reached the agent"));
                        }
                    }
                }
            }
        }
        // Stop on an idle chat holding no work: nothing to stop, and the
        // agent must not be torn down for it.
        "idle_stop" => {
            let first = chat.sentinel("IA");
            chat.say(&format!(
                "Just say hi to my cat {first} in one short sentence."
            ));
            if chat.settled(180).await {
                chat.stop();
                chat.stops -= 1;
                tokio::time::sleep(Duration::from_secs(2)).await;
                let second = chat.sentinel("IB");
                chat.say(&format!(
                    "Just say hi to my cat {second} in one short sentence."
                ));
                chat.settled(180).await;
            }
        }
        // A long foreground command with no output: silence is not the end.
        "silent_command" => {
            let token = chat.sentinel("SC");
            chat.say(&format!(
                "Run this exact shell command in the FOREGROUND and wait for it to finish (it takes about 75 seconds and prints nothing): {} . Then say hi to my cat {token}.",
                suite.probe(5, 75)
            ));
            chat.settled(420).await;
        }
        // The agent dies mid-command (OOM, `killall`): the turn must end,
        // visibly, and the next message resumes the same conversation.
        "crash_recovery" => {
            let word = format!("PELICAN{}", &uuid::Uuid::new_v4().simple().to_string()[..4])
                .to_uppercase();
            let first = chat.sentinel("CA");
            chat.say(&format!(
                "Hi! I named my new boat {word}. Just say hi to my cat {first} in one short sentence."
            ));
            if chat.settled(180).await {
                chat.say(&format!(
                    "Run this exact shell command in the FOREGROUND and wait for it: {} . Then reply FINISHED.",
                    suite.probe(6, 300)
                ));
                let marker = suite.probe_marker(6);
                if chat
                    .until(180, "the command to start", |c| c.tool_started(&marker))
                    .await
                {
                    chat.allow_errored = true;
                    chat.stops += 1;
                    let killed = suite.crash_agents();
                    if killed == 0 {
                        chat.errors.push("found no agent process to kill".into());
                    }
                    chat.until(120, "the crashed turn to end", |c| {
                        c.status() != Some(SessionStatus::Working)
                    })
                    .await;
                    let ended = chat
                        .events()
                        .iter()
                        .rev()
                        .any(|e| matches!(e, AgentEvent::Done { .. }));
                    if !ended {
                        chat.errors
                            .push("the crash ended the turn without a Done".into());
                    }
                    let second = chat.sentinel("CB");
                    let said = chat.text().len();
                    chat.say(&format!(
                        "What did I name my boat? Also say hi to my cat {second}. No tools."
                    ));
                    if chat.settled(240).await {
                        let reply = chat.text()[said..].to_uppercase();
                        if !reply.contains(&word) {
                            chat.errors.push(format!(
                                "the conversation did not survive the crash: {word} missing from {reply:?}"
                            ));
                        }
                    }
                }
            }
        }
        // Two chats on the same harness at once: each gets its own replies
        // and its own steer.
        "concurrent_chats" => {
            let mut other = suite.chat(&format!("{name}-b"), round);
            chat.say(ESSAY);
            other.say(ESSAY);
            if chat.streaming("chat A to stream").await && other.streaming("chat B to stream").await
            {
                let a = chat.sentinel("CA");
                let b = other.sentinel("CB");
                chat.say(&format!(
                    "Stop the essay. Just say hi to my cat {a} in one short sentence. No tools."
                ));
                other.say(&format!(
                    "Stop the essay. Just say hi to my cat {b} in one short sentence. No tools."
                ));
                chat.settled(240).await;
                other.settled(240).await;
                if chat.text().contains(b.as_str()) || other.text().contains(a.as_str()) {
                    chat.errors.push("a reply reached the other chat".into());
                }
            }
            let other_id = other.id.clone();
            let mut errors = other.verdict().await;
            let _ = suite.core.sessions.terminate(&other_id).await;
            for e in errors.drain(..) {
                chat.errors.push(format!("chat B: {e}"));
            }
        }
        // Approvals on: every permission the agent asks for is answered,
        // and the turn carries on to its end.
        "approval" => {
            let token = chat.sentinel("AP");
            let mut request = suite.request(&format!(
                "Run this exact shell command: echo approved > approval.txt . Then say hi to my cat {token}."
            ));
            request.auto_approve = false;
            chat.send(request);
            let deadline = Instant::now() + Duration::from_secs(300);
            let mut answered = std::collections::HashSet::new();
            loop {
                let pending = chat
                    .events()
                    .into_iter()
                    .filter_map(|e| match e {
                        AgentEvent::InputRequested {
                            request_id,
                            questions,
                        } => Some((request_id, questions)),
                        _ => None,
                    })
                    .filter(|(id, _)| !answered.contains(id))
                    .collect::<Vec<_>>();
                for (request_id, questions) in pending {
                    chat.sends.lock().unwrap().push(Instant::now());
                    let _ = suite.core.sessions.respond_input(
                        &chat.id,
                        &request_id,
                        answer_all(&questions),
                    );
                    answered.insert(request_id);
                }
                let done = chat.status() == Some(SessionStatus::Idle)
                    && chat.text().contains(token.as_str());
                if done || Instant::now() > deadline {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            chat.settled(30).await;
        }
        other => panic!("unknown scenario {other}"),
    }
    let id = chat.id.clone();
    let errors = chat.verdict().await;
    // Retire the chat's runtime: a later crash scenario kills every agent
    // this test runs.
    let _ = suite.core.sessions.terminate(&id).await;
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
    "burst_idle",
    "idle_stop",
    "silent_command",
    "crash_recovery",
    "concurrent_chats",
    "approval",
];

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "uses real model quota; select harness and inexpensive model explicitly"]
async fn adapter_chaos() {
    if std::env::var_os("RUST_LOG").is_some() {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
            .with_test_writer()
            .try_init();
    }
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

const RESTART_CHAT: &str = "restart";

fn status_of(core: &EngineCore) -> Option<SessionStatus> {
    core.sessions.session_status(RESTART_CHAT).map(|s| s.status)
}

fn text_of(core: &EngineCore) -> String {
    core.sessions
        .subscribe(RESTART_CHAT, 0)
        .map(|(events, _)| {
            events
                .into_iter()
                .filter_map(|e| match e.event {
                    AgentEvent::TextDelta { text } => Some(text),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

fn completed_of(core: &EngineCore) -> usize {
    core.sessions
        .subscribe(RESTART_CHAT, 0)
        .map(|(events, _)| {
            events
                .iter()
                .filter(|e| {
                    matches!(
                        e.event,
                        AgentEvent::Done {
                            status: zeron_proto::DoneStatus::Completed,
                            ..
                        }
                    )
                })
                .count()
        })
        .unwrap_or(0)
}

/// Send `prompt` and wait for its turn to complete; returns the reply text.
/// No reply token is demanded: some agents (Grok) refuse to echo one.
async fn ask(core: &EngineCore, name: &str, cwd: &str, prompt: &str, what: &str) -> String {
    let (done, said) = (completed_of(core), text_of(core).len());
    send_to(core, restart_request(name, cwd, prompt));
    let finished = tokio::time::timeout(Duration::from_secs(240), async {
        while !(completed_of(core) > done && status_of(core) == Some(SessionStatus::Idle)) {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    assert!(finished.is_ok(), "timed out: {what}:\n{}", dump(core));
    text_of(core)[said..].to_owned()
}

/// A completed turn after recovery's "— resuming" marker: the force-killed
/// turn was picked back up and ran to its end.
fn resumed_to_end(core: &EngineCore) -> bool {
    core.sessions
        .subscribe(RESTART_CHAT, 0)
        .is_ok_and(|(events, _)| {
            events
                .iter()
                .position(|e| {
                    matches!(&e.event, AgentEvent::Done { error: Some(note), .. }
                        if note.contains("resuming"))
                })
                .is_some_and(|at| {
                    events[at..].iter().any(|e| {
                        matches!(
                            e.event,
                            AgentEvent::Done {
                                status: zeron_proto::DoneStatus::Completed,
                                ..
                            }
                        )
                    })
                })
        })
}

fn send_to(core: &EngineCore, request: RunRequest) {
    core.doc_host
        .queue_command(
            RESTART_CHAT,
            SessionCommandPayload::Run {
                request,
                message_id: uuid::Uuid::new_v4().to_string(),
            },
        )
        .unwrap();
}

fn restart_request(name: &str, cwd: &str, prompt: &str) -> RunRequest {
    RunRequest {
        mcp: None,
        prompt: prompt.into(),
        harness: Some(harness(name).0),
        model: std::env::var("ZERON_TEST_MODEL").ok(),
        reasoning: None,
        model_options: Default::default(),
        cwd: cwd.into(),
        sandbox: SandboxLevel::DangerFullAccess,
        auto_approve: true,
        attachments: vec![],
        worktree: None,
        resume: None,
        resume_policy: Default::default(),
    }
}

async fn wait_for(secs: u64, what: &str, mut predicate: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while !predicate() {
        assert!(Instant::now() < deadline, "timed out: {what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// What the engine holds for the restart chat: journal events (text
/// folded away) and the transcript's entries.
fn dump(core: &EngineCore) -> String {
    let events: Vec<String> = core
        .sessions
        .subscribe(RESTART_CHAT, 0)
        .map(|(events, _)| {
            events
                .into_iter()
                .filter(|e| {
                    !matches!(
                        e.event,
                        AgentEvent::TextDelta { .. } | AgentEvent::ReasoningDelta { .. }
                    )
                })
                .map(|e| format!("{:?}", e.event).chars().take(200).collect())
                .collect()
        })
        .unwrap_or_default();
    let entries: Vec<String> = core
        .doc_host
        .open(RESTART_CHAT)
        .ok()
        .and_then(|h| h.doc().read_entries().ok())
        .unwrap_or_default()
        .iter()
        .map(|e| format!("{:?} {:?} {}", e.role, e.status, e.id))
        .collect();
    format!(
        "status {:?}\nevents {events:#?}\nentries {entries:#?}\ntext {:?}",
        status_of(core),
        text_of(core)
    )
}

fn probe_alive(marker: &str) -> bool {
    std::process::Command::new("pgrep")
        .args(["-f", marker])
        .output()
        .is_ok_and(|o| !o.stdout.is_empty())
}

/// Quitting the app mid-turn, and the app being force-killed mid-turn: after
/// the restart nothing reads as still running, the force-killed turn is
/// picked back up and finishes, and the conversation carries on with its
/// context.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "uses real model quota; select harness and inexpensive model explicitly"]
async fn restart_durability() {
    if std::env::var_os("RUST_LOG").is_some() {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
            .with_test_writer()
            .try_init();
    }
    let name = std::env::var("ZERON_TEST_HARNESS").unwrap_or_else(|_| "claude".into());
    let (id, _, base) = harness(&name);
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_str().unwrap().to_owned();
    let word = format!("HERON{}", &uuid::Uuid::new_v4().simple().to_string()[..4]).to_uppercase();

    // 1. A conversation with something to remember.
    let core = engine(dir.path(), &name);
    core.workspace
        .create_space("space", &core.device_id, &cwd, None, false)
        .unwrap();
    core.workspace
        .create_chat(RESTART_CHAT, Some("space"), None, None, None)
        .unwrap();
    core.workspace
        .set_chat_config(
            RESTART_CHAT,
            &ChatConfig {
                harness: id,
                model: std::env::var("ZERON_TEST_MODEL").ok(),
                reasoning: None,
                model_options: Default::default(),
                sandbox: SandboxLevel::DangerFullAccess,
            },
        )
        .unwrap();
    ask(
        &core,
        &name,
        &cwd,
        &format!("Hi! I named my new boat {word}. A one-line hello back is all I need."),
        "the opening reply",
    )
    .await;
    println!("{name}: opening turn done");

    if std::env::var_os("ZERON_RESTART_ONLY_FORCE_KILL").is_none() {
        // 2. Quit the app mid-command.
        let marker = format!("zr{}=1", base + 7);
        send_to(
            &core,
            restart_request(
                &name,
                &cwd,
                &format!(
                    "Please run this shell command in the foreground and wait for it to finish: python3 -c 'import time; {marker}; time.sleep(300)'"
                ),
            ),
        );
        wait_for(240, "the command to start", || probe_alive(&marker)).await;
        core.shutdown().await;
        drop(core);
        wait_for(30, "quitting to end the agent's command", || {
            !probe_alive(&marker)
        })
        .await;
        println!("{name}: quit mid-command; its work ended");

        // 3. Restart: nothing stuck, context intact.
        let core = engine(dir.path(), &name);
        wait_for(60, "the quit turn to read as ended", || {
            status_of(&core) != Some(SessionStatus::Working)
        })
        .await;
        let reply = ask(
            &core,
            &name,
            &cwd,
            "Quick question: what did I name my boat? No tools needed.",
            "the reply after a restart",
        )
        .await;
        assert!(
            reply.to_uppercase().contains(&word),
            "the conversation did not survive quitting the app ({word}): {reply:?}\n{}",
            dump(&core)
        );
        println!("{name}: restarted with the conversation intact");
        core.shutdown().await;
    } else {
        core.shutdown().await;
        drop(core);
    }

    // 4. Force-kill the app mid-command (a separate process: this one is the
    // app here), then restart: the interrupted turn is resumed and finishes.
    let crash_marker = format!("zr{}=1", base + 8);
    let mut child = tokio::process::Command::new(std::env::current_exe().unwrap())
        .args(["crash_phase", "--exact", "--ignored", "--nocapture"])
        .env("ZERON_CRASH_DIR", dir.path())
        .env("ZERON_CRASH_PROMPT", format!(
            "Please run this shell command in the foreground and wait for it to finish: python3 -c 'import time; {crash_marker}; time.sleep(30)'"
        ))
        .env("ZERON_CRASH_MARKER", &crash_marker)
        .process_group(0)
        .stdout(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let pgid = child.id().unwrap();
    {
        use tokio::io::AsyncBufReadExt;
        let mut lines = tokio::io::BufReader::new(child.stdout.take().unwrap()).lines();
        tokio::time::timeout(Duration::from_secs(300), async {
            while let Ok(Some(line)) = lines.next_line().await {
                if line.contains("CRASH-READY") {
                    return;
                }
            }
            panic!("the app process ended before its command started");
        })
        .await
        .expect("the app process to start the command");
    }
    let _ = std::process::Command::new("kill")
        .args(["-9", &format!("-{pgid}")])
        .status();
    let _ = child.wait().await;
    // Whatever the agent left behind outlives a force-kill; clear it so the
    // resumed run's own command is what we see.
    let _ = std::process::Command::new("pkill")
        .args(["-9", "-f", &crash_marker])
        .status();
    println!("{name}: app force-killed mid-command");

    let core = engine(dir.path(), &name);
    let resumed = tokio::time::timeout(Duration::from_secs(420), async {
        while !(resumed_to_end(&core) && status_of(&core) == Some(SessionStatus::Idle)) {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    assert!(
        resumed.is_ok(),
        "the force-killed turn was not resumed to its end:\n{}",
        dump(&core)
    );
    println!("{name}: the force-killed turn resumed and finished");
    let reply = ask(
        &core,
        &name,
        &cwd,
        "Remind me, what did I name my boat? No tools needed.",
        "the reply after a force-kill",
    )
    .await;
    assert!(
        reply.to_uppercase().contains(&word),
        "the conversation did not survive a force-kill ({word}): {reply:?}\n{}",
        dump(&core)
    );
    println!("{name}: force-kill survived with the conversation intact");
    core.shutdown().await;
    let _ = std::process::Command::new("pkill")
        .args(["-9", "-f", &format!("zr{}[0-9]=1", base / 10)])
        .status();
}

/// `restart_durability`'s app process: starts the command, reports, and
/// waits to be killed. Does nothing unless launched by that test.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "launched by restart_durability"]
async fn crash_phase() {
    if std::env::var_os("RUST_LOG").is_some() {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
            .with_test_writer()
            .try_init();
    }
    let Some(dir) = std::env::var_os("ZERON_CRASH_DIR") else {
        return;
    };
    let name = std::env::var("ZERON_TEST_HARNESS").unwrap_or_else(|_| "claude".into());
    let dir = std::path::PathBuf::from(dir);
    let core = engine(&dir, &name);
    let cwd = dir.to_str().unwrap().to_owned();
    send_to(
        &core,
        restart_request(&name, &cwd, &std::env::var("ZERON_CRASH_PROMPT").unwrap()),
    );
    let marker = std::env::var("ZERON_CRASH_MARKER").unwrap();
    wait_for(240, "the command to start", || probe_alive(&marker)).await;
    println!("CRASH-READY");
    tokio::time::sleep(Duration::from_secs(600)).await;
}
