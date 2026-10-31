//! Synthetic Markdown / tool-call content and `.zbr.jsonl` turn scripts.
//!
//! TODO(b4): this is a minimal stand-in for the fixture generator on
//! `perf/b4-fixtures` (`src/fixture/{text,scripts}.rs`, bench.md 3.1), which
//! only offers `T-*` single-turn profiles so far. Switch to it once B4 lands
//! the cadence profiles and `P20`/`P200`. Same seed gives byte-identical
//! scripts here too.

use rand_chacha::ChaCha8Rng;
use rand_chacha::rand_core::{RngCore, SeedableRng};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

/// Seeded generator. Every draw comes from here so scripts are reproducible.
pub struct Rng(ChaCha8Rng);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(ChaCha8Rng::seed_from_u64(seed))
    }

    /// Uniform in `[0, 1)`.
    pub fn unit(&mut self) -> f64 {
        (self.0.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Uniform in `lo..=hi`.
    pub fn range(&mut self, lo: u64, hi: u64) -> u64 {
        if hi <= lo {
            return lo;
        }
        lo + self.0.next_u64() % (hi - lo + 1)
    }

    pub fn chance(&mut self, p: f64) -> bool {
        self.unit() < p
    }

    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.range(0, items.len() as u64 - 1) as usize]
    }

    /// Log-normal with the given median and shape.
    pub fn lognormal(&mut self, median: f64, sigma: f64) -> f64 {
        let u1 = self.unit().max(1e-12);
        let u2 = self.unit();
        let z = (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos();
        median * (sigma * z).exp()
    }
}

const WORDS: &[&str] = &[
    "stream", "engine", "session", "journal", "commit", "snapshot", "transcript", "message",
    "chunk", "buffer", "channel", "worker", "scheduler", "runtime", "handler", "checkpoint",
    "document", "revision", "delta", "frame", "parser", "encoder", "socket", "request", "reply",
    "debounce", "watcher", "index", "cache", "queue", "pipeline", "adapter", "harness", "sandbox",
    "workspace", "project", "branch", "diff", "patch", "merge", "build", "target", "release",
    "profile", "memory", "latency", "throughput", "allocation", "thread", "task", "future",
    "timeout", "retry", "backoff", "invariant", "boundary", "interface", "module", "crate",
    "dependency", "feature", "config", "schema", "migration", "storage", "database", "table",
    "column", "record", "lookup", "iterator", "closure", "lifetime", "borrow", "trait", "struct",
    "variant", "pattern", "result", "error", "context", "state", "event", "listener", "observer",
    "render", "layout", "widget", "theme", "token", "prompt", "model", "tool", "output", "input",
    "format", "decode", "validate", "normalize", "resolve", "dispatch", "forward", "persist",
    "restore", "recover", "flush", "compact", "rotate", "archive", "monitor", "measure", "report",
];

const VERBS: &[&str] = &[
    "keeps", "folds", "emits", "drains", "applies", "records", "rebuilds", "publishes",
    "validates", "forwards", "coalesces", "persists", "tracks", "serves", "replays", "resolves",
];

const GLUE: &[&str] = &[
    "so that", "because", "while", "before", "after", "until", "whenever", "although",
];

const LANG_LINES: &[&str] = &[
    "let mut folded = fold_event_into_parts(&folded, &event);",
    "writer.sync(&folded)?; // coalesced commit",
    "for entry in doc.read_entries(range)? { out.push(entry.into()); }",
    "match self.state.take() { Some(s) => s.finish(), None => return Ok(()) }",
    "tokio::select! { msg = rx.recv() => handle(msg), _ = tick.tick() => flush().await }",
    "const DEBOUNCE_MS: u64 = 120;",
    "export function applyDelta(view: View, delta: Delta): View {",
    "  return { ...view, entries: view.entries.map(e => patch(e, delta)) };",
    "git status --porcelain=v1 -z | wc -c",
    "cargo build --release -p engine 2>&1 | tail -n 20",
    "if let Some(tail) = self.tail.as_mut() { tail.extend_from_slice(chunk); }",
    "impl Drop for Guard { fn drop(&mut self) { self.release(); } }",
    "assert_eq!(view.count(), expected, \"count invariant\");",
    "const rows = await db.query('select id, body from messages order by seq');",
];

fn word(rng: &mut Rng) -> &'static str {
    rng.pick(WORDS)
}

fn sentence(rng: &mut Rng) -> String {
    let mut out = String::new();
    let n = rng.range(7, 18);
    for i in 0..n {
        let w = word(rng);
        if i == 0 {
            let mut chars = w.chars();
            if let Some(first) = chars.next() {
                out.extend(first.to_uppercase());
                out.push_str(chars.as_str());
            }
        } else if rng.chance(0.07) {
            out.push_str(&format!("`{w}_{}`", word(rng)));
        } else if rng.chance(0.04) {
            out.push_str(&format!("**{w}**"));
        } else if rng.chance(0.02) {
            out.push_str(&format!("[{w}](https://example.invalid/{w}/{})", word(rng)));
        } else {
            out.push_str(w);
        }
        if i + 1 < n {
            if rng.chance(0.08) {
                out.push_str(&format!(" {} {}", rng.pick(VERBS), rng.pick(GLUE)));
            }
            out.push(' ');
        }
    }
    if rng.chance(0.05) {
        out.push_str(" \u{2014} see the note above");
    }
    out.push('.');
    out
}

fn paragraph(rng: &mut Rng) -> String {
    let n = rng.range(2, 5);
    (0..n).map(|_| sentence(rng)).collect::<Vec<_>>().join(" ")
}

fn heading(rng: &mut Rng) -> String {
    let level = if rng.chance(0.3) { "#" } else { "##" };
    format!("{level} {} {}", capital(word(rng)), word(rng))
}

fn capital(w: &str) -> String {
    let mut chars = w.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

fn list(rng: &mut Rng) -> String {
    let ordered = rng.chance(0.4);
    let n = rng.range(3, 6);
    (0..n)
        .map(|i| {
            let mark = if ordered {
                format!("{}.", i + 1)
            } else {
                "-".to_string()
            };
            format!("{mark} {}", sentence(rng))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn code_block(rng: &mut Rng) -> String {
    let lang = *rng.pick(&["rust", "ts", "sh"]);
    let n = rng.range(3, 9);
    let body = (0..n)
        .map(|_| *rng.pick(LANG_LINES))
        .collect::<Vec<_>>()
        .join("\n");
    format!("```{lang}\n{body}\n```")
}

/// Markdown prose of at least `min_bytes` (blocks are never cut).
pub fn markdown(rng: &mut Rng, min_bytes: usize) -> String {
    let mut out = String::new();
    while out.len() < min_bytes {
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        let roll = rng.unit();
        let block = if roll < 0.12 {
            heading(rng)
        } else if roll < 0.28 {
            list(rng)
        } else if roll < 0.40 {
            code_block(rng)
        } else {
            paragraph(rng)
        };
        out.push_str(&block);
    }
    out
}

fn tool_output(rng: &mut Rng, bytes: usize) -> String {
    let mut out = String::with_capacity(bytes + 80);
    let mut line_no = 1;
    while out.len() < bytes {
        let line = *rng.pick(LANG_LINES);
        out.push_str(&format!("{line_no:>5}\t{line}\n"));
        line_no += 1;
    }
    out
}

/// Pacing of a streamed turn (bench.md 3.1 cadence profiles).
#[derive(Clone, Copy, Debug)]
pub struct Cadence {
    pub interval_ms: u64,
    /// Fractional jitter on the interval.
    pub jitter: f64,
    pub median_chars: f64,
    pub pause_ms: (u64, u64),
}

impl Cadence {
    pub fn opus() -> Self {
        Self {
            interval_ms: 35,
            jitter: 0.3,
            median_chars: 9.0,
            pause_ms: (500, 6000),
        }
    }

    pub fn haiku() -> Self {
        Self {
            interval_ms: 20,
            jitter: 0.3,
            median_chars: 14.0,
            pause_ms: (300, 3000),
        }
    }

    pub fn burst() -> Self {
        Self {
            interval_ms: 0,
            jitter: 0.0,
            median_chars: 4096.0,
            pause_ms: (0, 0),
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "opus" => Some(Self::opus()),
            "haiku" => Some(Self::haiku()),
            "burst" => Some(Self::burst()),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct TurnOpts {
    pub seed: u64,
    /// First eight characters of the chat id: the marker prefix zb-replay uses.
    pub chat8: String,
    /// Stop once thinking, text and tool I/O add up to this many bytes...
    pub payload_bytes: usize,
    /// ...and the scripted timeline is at least this long.
    pub min_ms: u64,
    pub cadence: Cadence,
    /// Put a marker on every Nth top-level text delta (0 = never).
    pub marker_every: usize,
    pub tools: bool,
    pub thinking: bool,
}

#[derive(Clone, Debug)]
pub struct BuiltTurn {
    /// The `.zbr.jsonl` file content.
    pub source: String,
    /// Concatenated top-level text deltas as zb-replay renders them, markers included.
    pub text: String,
    pub text_sha256: String,
    pub duration_ms: u64,
    pub markers: usize,
    pub deltas: usize,
    pub tool_calls: usize,
    pub payload_bytes: usize,
}

struct Builder {
    rng: Rng,
    opts: TurnOpts,
    steps: Vec<Value>,
    frames: usize,
    text: String,
    ms: u64,
    payload: usize,
    deltas: usize,
    text_deltas: usize,
    markers: usize,
    tool_calls: usize,
    block: usize,
}

impl Builder {
    fn interval(&mut self) -> u64 {
        let base = self.opts.cadence.interval_ms as f64;
        if base == 0.0 {
            return 0;
        }
        let jitter = self.opts.cadence.jitter;
        let factor = 1.0 + jitter * (self.rng.unit() * 2.0 - 1.0);
        (base * factor).max(1.0) as u64
    }

    fn frame(&mut self, dt: u64, f: Value) {
        self.ms += dt;
        self.frames += 1;
        self.steps.push(json!({"dt": dt, "f": f}));
    }

    fn block_start(&mut self, content_block: Value) -> usize {
        let index = self.block;
        self.block += 1;
        self.frame(
            0,
            json!({"type":"stream_event","event":{"type":"content_block_start","index":index,"content_block":content_block}}),
        );
        index
    }

    fn block_stop(&mut self, index: usize) {
        self.frame(
            0,
            json!({"type":"stream_event","event":{"type":"content_block_stop","index":index}}),
        );
    }

    /// Split `body` into deltas of log-normal size, cut on char boundaries.
    fn chunks(&mut self, body: &str) -> Vec<String> {
        let median = self.opts.cadence.median_chars;
        let mut out = Vec::new();
        let mut start = 0;
        while start < body.len() {
            let want = if median >= 1000.0 {
                median as usize
            } else {
                self.rng.lognormal(median, 0.7).clamp(1.0, 40.0) as usize
            };
            let mut end = (start + want.max(1)).min(body.len());
            while !body.is_char_boundary(end) {
                end += 1;
            }
            out.push(body[start..end].to_string());
            start = end;
        }
        out
    }

    fn text_block(&mut self, body: &str) {
        let index = self.block_start(json!({"type":"text","text":""}));
        for chunk in self.chunks(body) {
            let dt = self.interval();
            self.deltas += 1;
            self.text_deltas += 1;
            let marked = self.opts.marker_every > 0
                && self.text_deltas % self.opts.marker_every == 0;
            // zb-replay renders the marker; the script carries the plain chunk,
            // while `text` holds what the engine will end up with.
            self.text.push_str(&chunk);
            if marked {
                self.steps.push(json!({"dt": 0, "zb": "marker"}));
                self.markers += 1;
                self.text
                    .push_str(&format!(" [[zb:{}:{}]]", self.opts.chat8, self.markers));
            }
            self.payload += chunk.len();
            self.frame(
                dt,
                json!({"type":"stream_event","event":{"type":"content_block_delta","index":index,"delta":{"type":"text_delta","text":chunk}}}),
            );
        }
        self.block_stop(index);
    }

    fn thinking_block(&mut self, body: &str) {
        let index = self.block_start(json!({"type":"thinking","thinking":""}));
        for chunk in self.chunks(body) {
            let dt = self.interval();
            self.deltas += 1;
            self.payload += chunk.len();
            self.frame(
                dt,
                json!({"type":"stream_event","event":{"type":"content_block_delta","index":index,"delta":{"type":"thinking_delta","thinking":chunk}}}),
            );
        }
        self.block_stop(index);
    }

    fn tool_call(&mut self) {
        let (name, input) = match self.rng.range(0, 4) {
            0 => (
                "Read",
                json!({"file_path": format!("/work/src/{}/{}.rs", word(&mut self.rng), word(&mut self.rng))}),
            ),
            1 => (
                "Grep",
                json!({"pattern": word(&mut self.rng), "path": "/work/src"}),
            ),
            2 => (
                "Edit",
                json!({"file_path": format!("/work/src/{}.rs", word(&mut self.rng)),
                    "old_string": sentence(&mut self.rng), "new_string": sentence(&mut self.rng)}),
            ),
            3 => (
                "Bash",
                json!({"command": format!("cargo test -p {} --lib {}", word(&mut self.rng), word(&mut self.rng))}),
            ),
            _ => (
                "Write",
                json!({"file_path": format!("/work/notes/{}.md", word(&mut self.rng)),
                    "content": markdown(&mut self.rng, 300)}),
            ),
        };
        let index = self.block_start(
            json!({"type":"tool_use","id":"$TOOL_ID","name":name,"input":{}}),
        );
        let raw = input.to_string();
        let mut start = 0;
        while start < raw.len() {
            let mut end = (start + self.rng.range(12, 40) as usize).min(raw.len());
            while !raw.is_char_boundary(end) {
                end += 1;
            }
            let dt = self.interval();
            self.deltas += 1;
            self.payload += end - start;
            self.frame(
                dt,
                json!({"type":"stream_event","event":{"type":"content_block_delta","index":index,"delta":{"type":"input_json_delta","partial_json":&raw[start..end]}}}),
            );
            start = end;
        }
        self.block_stop(index);
        let (lo, hi) = self.opts.cadence.pause_ms;
        let pause = self.rng.range(lo, hi);
        // Median 2 KB, p95 ~40 KB, capped at 200 KB (bench.md 3.1).
        let size = self.rng.lognormal(2048.0, 1.82).clamp(64.0, 200_000.0) as usize;
        let output = tool_output(&mut self.rng, size);
        self.payload += output.len();
        self.tool_calls += 1;
        self.frame(
            pause,
            json!({"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"$TOOL_ID","content":output,"is_error":false}]}}),
        );
    }
}

/// Build one turn. Deterministic in `opts`.
pub fn build_turn(opts: &TurnOpts) -> BuiltTurn {
    let mut b = Builder {
        rng: Rng::new(opts.seed),
        opts: opts.clone(),
        steps: Vec::new(),
        frames: 0,
        text: String::new(),
        ms: 0,
        payload: 0,
        deltas: 0,
        text_deltas: 0,
        markers: 0,
        tool_calls: 0,
        block: 0,
    };
    loop {
        if b.opts.thinking && b.rng.chance(0.45) {
            let n = b.rng.range(200, 1400) as usize;
            let body = markdown(&mut b.rng, n);
            b.thinking_block(&body);
        }
        let n = b.rng.range(300, 2400) as usize;
        let body = markdown(&mut b.rng, n);
        b.text_block(&body);
        let enough = b.payload >= b.opts.payload_bytes && b.ms >= b.opts.min_ms;
        if enough {
            break;
        }
        if b.opts.tools && b.rng.chance(0.65) {
            b.tool_call();
        }
    }
    let text_sha256 = format!("{:x}", Sha256::digest(b.text.as_bytes()));
    let mut source = json!({
        "zb": "meta", "v": 1, "name": format!("b7-{}-{}", b.opts.chat8, b.opts.seed),
        "seed": b.opts.seed, "text_bytes": b.text.len(), "text_sha256": text_sha256,
        "frames": b.frames,
    })
    .to_string();
    source.push('\n');
    for step in &b.steps {
        source.push_str(&step.to_string());
        source.push('\n');
    }
    source.push_str("{\"dt\":0,\"zb\":\"turn_end\",\"usage\":{\"input_tokens\":0,\"output_tokens\":0}}\n");
    BuiltTurn {
        source,
        text: b.text,
        text_sha256,
        duration_ms: b.ms,
        markers: b.markers,
        deltas: b.deltas,
        tool_calls: b.tool_calls,
        payload_bytes: b.payload,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::replay::script::Script;

    fn opts(cadence: Cadence, markers: usize) -> TurnOpts {
        TurnOpts {
            seed: 7,
            chat8: "b7000001".into(),
            payload_bytes: 30_000,
            min_ms: 0,
            cadence,
            marker_every: markers,
            tools: true,
            thinking: true,
        }
    }

    #[test]
    fn same_seed_is_byte_identical_and_parses() {
        let a = build_turn(&opts(Cadence::opus(), 20));
        let b = build_turn(&opts(Cadence::opus(), 20));
        assert_eq!(a.source, b.source);
        let script = Script::parse(&a.source).unwrap();
        assert_eq!(script.meta.text_bytes, a.text.len());
        assert_eq!(script.meta.text_sha256, a.text_sha256);
        assert!(a.payload_bytes >= 30_000);
        assert!(a.markers > 0 && a.tool_calls > 0);
        assert!(a.text.contains("[[zb:b7000001:1]]"));
    }

    #[test]
    fn duration_floor_is_met() {
        let mut o = opts(Cadence::opus(), 0);
        o.payload_bytes = 1;
        o.min_ms = 8_000;
        let turn = build_turn(&o);
        assert!(turn.duration_ms >= 8_000);
        let burst = build_turn(&opts(Cadence::burst(), 0));
        assert_eq!(burst.duration_ms, 0);
        assert!(!burst.text.contains("[[zb:"));
    }
}
