//! Cost traces for the chat3 benchmark (docs/chat3-sync.md, "Proving the
//! cost"). Replays agent events through the REAL fold, `SegmentWriter` and
//! doc layouts on a virtual clock, once as a gen-2 chat and once as a gen-3
//! chat, and records exactly what each sends to its room:
//!
//! - gen 2: one Loro update per `STREAM_COMMIT_MS` commit (its real byte
//!   size), plus the host's threshold checkpoints (rows > 200 or > 512 KB),
//!   each the doc's real snapshot size at that moment;
//! - gen 3: the transcript plane's stored rows (their real JSON), live
//!   frames (bytes only — never stored) and full tool outputs.
//!
//! Both get the same state-log pushes (a user's prompt command and the
//! host's two status updates per turn), which neither design changes.
//!
//! `edge/test/workerd/chat3-cost.workerd.test.ts` replays the traces into
//! real gen-2 and gen-3 rooms and compares the rows Cloudflare bills.
//!
//! Events are compact arrays: `["p", t, bytes]` a state-log push,
//! `["c", t, bytes]` a gen-2 checkpoint, `["i", t, rows]` a gen-3 upsert batch
//! (each row `[ord, idx, entry, length, hash]`), `["o", t, part, bytes]` a
//! gen-3 tool output.
//!
//!   cargo run -p zeron-doc --example chat3_trace -- [journal.jsonl] > traces.json
//!
//! A journal's text is replaced by filler of the same length (structure,
//! sizes and cadence are what matter; content never leaves the machine).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use zeron_doc::transcript_plane::{Publication, Publisher};
use zeron_doc::{
    MessagePart, MessageRole, MessageStatus, STREAM_COMMIT_MS, SegmentWriter, SessionDoc, SessionMessageEntry,
    fold_event_into_parts,
};
use zeron_proto::{AgentEvent, DoneStatus, ToolCall};

/// Spacing of streamed deltas. Claude and Codex stream roughly 20–50 deltas
/// a second; 30 ms sits in the middle.
const DELTA_MS: i64 = 30;
/// How long a tool runs before its result (a middling shell command).
const TOOL_MS: i64 = 2_500;
/// Full output stored per tool (gen 3 only — gen 2 drops outputs). Recorded
/// journals predate output capture, so every tool gets this much.
const OUTPUT_BYTES: usize = 2_048;
/// The host's chat2 checkpoint thresholds (doc_host::chat2_maintenance).
const CHECKPOINT_ROWS: usize = 200;
const CHECKPOINT_BYTES: usize = 512 * 1024;
/// State-log pushes per turn, identical in both generations: the prompt
/// command a client queues, and the host marking it running then done.
const STATE_PUSHES_PER_TURN: usize = 3;
const STATE_PUSH_BYTES: usize = 400;

#[derive(Default)]
struct Trace {
    events: Vec<Value>,
    commits: usize,
}

struct Sim {
    gen2: SessionDoc,
    gen3: SessionDoc,
    updates: Arc<Mutex<Vec<usize>>>,
    publisher: Arc<Mutex<Publisher>>,
    _subs: (loro::Subscription, loro::Subscription),
    t2: Trace,
    t3: Trace,
    rows_since_checkpoint: usize,
    bytes_since_checkpoint: usize,
    now: i64,
    live_bytes: usize,
    live_frames: usize,
}

impl Sim {
    fn new() -> Self {
        let gen2 = SessionDoc::init("chat").unwrap();
        let gen3 = SessionDoc::init_split("chat").unwrap();
        let updates = Arc::new(Mutex::new(Vec::new()));
        let sink = updates.clone();
        let sub2 = gen2.doc().subscribe_local_update(Box::new(move |bytes| {
            sink.lock().unwrap().push(bytes.len());
            true
        }));
        let publisher = Arc::new(Mutex::new(Publisher::new(HashMap::new())));
        let feed = publisher.clone();
        let sub3 = gen3
            .transcript_doc()
            .subscribe_root(Arc::new(move |event| feed.lock().unwrap().observe(&event)));
        Self {
            gen2,
            gen3,
            updates,
            publisher,
            _subs: (sub2, sub3),
            t2: Trace::default(),
            t3: Trace::default(),
            rows_since_checkpoint: 0,
            bytes_since_checkpoint: 0,
            now: 0,
            live_bytes: 0,
            live_frames: 0,
        }
    }

    /// Everything the last commit made each room do.
    fn drain(&mut self) {
        let updates: Vec<usize> = std::mem::take(&mut *self.updates.lock().unwrap());
        for bytes in updates {
            self.push2(bytes);
        }
        let publications = self.publisher.lock().unwrap().collect(&self.gen3, self.now);
        let mut items = Vec::new();
        for publication in publications {
            match publication {
                // A row as [ord, idx, entry, length, content hash]: the room
                // only cares about a row's size and whether a rewrite is
                // identical, so the replay synthesizes bytes from the hash.
                Publication::Store(item) => items.push(json!([
                    item.key.ord,
                    item.key.idx,
                    item.entry,
                    item.json.len(),
                    format!("{:016x}", item.hash()),
                ])),
                Publication::Live(live) => {
                    self.live_frames += 1;
                    self.live_bytes += match live.op {
                        zeron_doc::transcript_plane::LiveOp::Part(json) => json.len(),
                        zeron_doc::transcript_plane::LiveOp::Append { text, .. } => text.len(),
                    } + 64;
                }
            }
        }
        if !items.is_empty() {
            self.t3.events.push(json!(["i", self.now, items]));
        }
    }

    fn push2(&mut self, bytes: usize) {
        self.t2.events.push(json!(["p", self.now, bytes]));
        self.rows_since_checkpoint += 1;
        self.bytes_since_checkpoint += bytes;
        if self.rows_since_checkpoint > CHECKPOINT_ROWS || self.bytes_since_checkpoint > CHECKPOINT_BYTES {
            let snapshot = self.gen2.export_snapshot().unwrap().len();
            self.t2.events.push(json!(["c", self.now, snapshot]));
            self.rows_since_checkpoint = 0;
            self.bytes_since_checkpoint = 0;
        }
    }

    fn state_pushes(&mut self) {
        for _ in 0..STATE_PUSHES_PER_TURN {
            self.push2(STATE_PUSH_BYTES);
            self.t3.events.push(json!(["p", self.now, STATE_PUSH_BYTES]));
        }
    }

    fn user(&mut self, id: &str, text: &str) {
        let entry = SessionMessageEntry {
            id: id.into(),
            role: MessageRole::User,
            parts: vec![MessagePart::Text {
                id: "t0".into(),
                text: text.into(),
            }],
            created_at: self.now,
            device_id: "host".into(),
            status: Some(MessageStatus::Complete),
            continuation_of: None,
            duration_ms: None,
        };
        self.gen2.push_message(&entry).unwrap();
        self.gen3.push_message(&entry).unwrap();
        self.drain();
        self.state_pushes();
    }

    fn output(&mut self, part: &str, bytes: usize) {
        self.t3.events.push(json!(["o", self.now, part, bytes]));
    }

    fn finish(mut self, name: &str, description: &str) -> Value {
        let snapshot2 = self.gen2.export_snapshot().unwrap().len();
        let state3 = self.gen3.export_snapshot().unwrap().len();
        let transcript3 = self.gen3.export_transcript_snapshot().unwrap().unwrap().len();
        let entries = self.gen3.read_entries().unwrap();
        let parts: usize = entries.iter().map(|e| e.parts.len()).sum();
        self.drain();
        json!({
            "name": name,
            "description": description,
            "params": {"deltaMs": DELTA_MS, "toolMs": TOOL_MS, "outputBytes": OUTPUT_BYTES, "commitMs": STREAM_COMMIT_MS},
            "durationMs": self.now,
            "entries": entries.len(),
            "parts": parts,
            "commits": self.t2.commits,
            "gen2": {"events": self.t2.events, "snapshotBytes": snapshot2},
            "gen3": {
                "events": self.t3.events,
                "stateSnapshotBytes": state3,
                "transcriptSnapshotBytes": transcript3,
                "liveFrames": self.live_frames,
                "liveBytes": self.live_bytes,
            },
        })
    }
}

/// One run of agent events, written exactly as `drive_run` writes them: the
/// fold on every event, a coalesced commit `STREAM_COMMIT_MS` after the first
/// dirty event, a new entry at every steer and turn end.
struct Run<'a> {
    sim: &'a mut Sim,
    folded: Vec<MessagePart>,
    entry: Option<(String, usize, Vec<MessagePart>, usize, Vec<MessagePart>)>,
    flush_at: Option<i64>,
    turn: usize,
    segment: usize,
}

impl<'a> Run<'a> {
    fn new(sim: &'a mut Sim) -> Self {
        Self {
            sim,
            folded: Vec::new(),
            entry: None,
            flush_at: None,
            turn: 0,
            segment: 0,
        }
    }

    fn begin_turn(&mut self, prompt_len: usize) {
        self.turn += 1;
        let text = filler(prompt_len.max(12));
        self.sim.user(&format!("u{}", self.turn), &text);
    }

    fn commit(&mut self) {
        if self.folded.is_empty() {
            return;
        }
        self.sim.t2.commits += 1;
        let (id, ix2, w2, ix3, w3) = match self.entry.take() {
            Some(entry) => entry,
            None => {
                self.segment += 1;
                let id = format!("a{}", self.segment);
                let (ix2, w2) = SegmentWriter::begin(&self.sim.gen2, &id, "host", self.sim.now).unwrap().into_state();
                let (ix3, w3) = SegmentWriter::begin(&self.sim.gen3, &id, "host", self.sim.now).unwrap().into_state();
                (id, ix2, w2, ix3, w3)
            }
        };
        let mut writer2 = SegmentWriter::resume(&self.sim.gen2, ix2, w2);
        writer2.sync(&self.folded).unwrap();
        let mut writer3 = SegmentWriter::resume(&self.sim.gen3, ix3, w3);
        writer3.sync(&self.folded).unwrap();
        let (ix2, w2) = writer2.into_state();
        let (ix3, w3) = writer3.into_state();
        self.entry = Some((id, ix2, w2, ix3, w3));
        self.sim.drain();
    }

    fn finish_segment(&mut self) {
        self.commit();
        if let Some((_, ix2, w2, ix3, w3)) = self.entry.take() {
            SegmentWriter::resume(&self.sim.gen2, ix2, w2)
                .finish(&self.folded, MessageStatus::Complete)
                .unwrap();
            SegmentWriter::resume(&self.sim.gen3, ix3, w3)
                .finish(&self.folded, MessageStatus::Complete)
                .unwrap();
            self.sim.drain();
        }
        self.folded.clear();
        self.flush_at = None;
    }

    /// Advance the clock to `at`, firing the pending commit on the way.
    fn advance(&mut self, at: i64) {
        if let Some(flush) = self.flush_at
            && flush <= at
        {
            self.sim.now = flush;
            self.commit();
            self.flush_at = None;
        }
        self.sim.now = self.sim.now.max(at);
    }

    fn event(&mut self, event: &AgentEvent) {
        let delay = match event {
            AgentEvent::TextDelta { .. } | AgentEvent::ReasoningDelta { .. } => DELTA_MS,
            AgentEvent::ToolResult { .. } => TOOL_MS,
            _ => 0,
        };
        self.advance(self.sim.now + delay);
        match event {
            AgentEvent::Steered { .. } => {
                self.finish_segment();
                self.begin_turn(80);
                return;
            }
            AgentEvent::Done { .. } => {
                self.finish_segment();
                return;
            }
            AgentEvent::ToolResult { id, output, .. } => {
                let bytes = output.as_ref().map_or(OUTPUT_BYTES, String::len);
                self.sim.output(id, bytes);
            }
            _ => {}
        }
        fold_event_into_parts(&mut self.folded, event);
        if !self.folded.is_empty() && self.flush_at.is_none() {
            self.flush_at = Some(self.sim.now + STREAM_COMMIT_MS as i64);
        }
    }

    fn end(mut self) {
        self.finish_segment();
    }
}

fn filler(len: usize) -> String {
    const WORDS: &[&str] = &["sync", "room", "row", "part", "tool", "plane", "edge", "doc", "turn", "chat"];
    let mut out = String::with_capacity(len + 8);
    let mut i = 0;
    while out.len() < len {
        out.push_str(WORDS[i % WORDS.len()]);
        out.push(' ');
        i += 1;
    }
    out.truncate(len);
    out
}

/// A journal's events with their text replaced by same-length filler.
fn anonymize(event: AgentEvent) -> Option<AgentEvent> {
    Some(match event {
        AgentEvent::TextDelta { text } => AgentEvent::TextDelta { text: filler(text.len()) },
        AgentEvent::ReasoningDelta { text } => AgentEvent::ReasoningDelta { text: filler(text.len()) },
        AgentEvent::ToolCall { id, call } => {
            let size = serde_json::to_string(&call).map(|s| s.len()).unwrap_or(40).min(300);
            AgentEvent::ToolCall {
                id,
                call: ToolCall::Exec { command: filler(size) },
            }
        }
        AgentEvent::ToolResult { id, is_error, output, .. } => AgentEvent::ToolResult {
            id,
            is_error,
            output: output.map(|o| filler(o.len())),
            diff: None,
        },
        e @ (AgentEvent::Steered { .. } | AgentEvent::Done { .. } | AgentEvent::AssistantMessageCompleted { .. }) => e,
        // Subagents stream into their own docs (their own rooms); usage and
        // session bookkeeping never reach the transcript.
        _ => return None,
    })
}

fn journal_workload(path: &str) -> Value {
    let text = std::fs::read_to_string(path).expect("journal readable");
    let mut sim = Sim::new();
    let mut run = Run::new(&mut sim);
    run.begin_turn(120);
    let mut finished = false;
    for line in text.lines() {
        let Ok(value) = serde_json::from_str::<Value>(line) else { continue };
        let Ok(event) = serde_json::from_value::<AgentEvent>(value["event"].clone()) else {
            continue;
        };
        let Some(event) = anonymize(event) else { continue };
        if finished && !matches!(event, AgentEvent::Done { .. }) {
            run.begin_turn(120);
        }
        finished = matches!(event, AgentEvent::Done { .. });
        run.event(&event);
    }
    run.end();
    sim.finish(
        "recorded",
        "A real Claude Code session (this chat's design and build turns), text replaced by same-length filler",
    )
}

/// One long answer: 3 minutes of reasoning and text, no tools.
fn long_answer() -> Value {
    let mut sim = Sim::new();
    let mut run = Run::new(&mut sim);
    run.begin_turn(200);
    for i in 0..2_000 {
        run.event(&AgentEvent::ReasoningDelta { text: format!("thinking {i} ") });
    }
    for i in 0..4_000 {
        run.event(&AgentEvent::TextDelta { text: format!("word{i} ") });
    }
    run.event(&done());
    run.end();
    sim.finish("long-answer", "One turn: 60 s of reasoning then 120 s of answer text, no tools")
}

/// A tool-heavy turn: 80 shell commands with a little text between them.
fn tool_heavy() -> Value {
    let mut sim = Sim::new();
    let mut run = Run::new(&mut sim);
    run.begin_turn(200);
    for i in 0..80 {
        for w in 0..20 {
            run.event(&AgentEvent::TextDelta { text: format!("step{i}.{w} ") });
        }
        run.event(&AgentEvent::ToolCall {
            id: format!("call_{i}"),
            call: ToolCall::Exec { command: format!("cargo test -p crate{i}") },
        });
        run.event(&AgentEvent::ToolResult {
            id: format!("call_{i}"),
            is_error: false,
            output: None,
            diff: None,
        });
    }
    run.event(&done());
    run.end();
    sim.finish("tool-heavy", "One turn: 80 shell commands, ~20 text deltas before each")
}

fn done() -> AgentEvent {
    AgentEvent::Done {
        status: DoneStatus::Completed,
        result: None,
        error: None,
        session_id: None,
    }
}

fn main() {
    let mut workloads = Vec::new();
    if let Some(path) = std::env::args().nth(1) {
        workloads.push(journal_workload(&path));
    }
    workloads.push(long_answer());
    workloads.push(tool_heavy());
    println!("{}", serde_json::to_string(&json!({ "workloads": workloads })).unwrap());
}
