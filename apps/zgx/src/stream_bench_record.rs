//! The stream bench's recording (src/stream_bench.rs, feature `stream-bench`):
//! a streaming reply as the engine's `WatchDocMessages` sends it to a chat
//! view, one frame a line (crates/doc/src/transcript_delta.rs
//! `TranscriptFrame`, each the `item` of a watch's message):
//!
//! - the opening tail, a `reset`: the prompt the reply answers;
//! - the reply's row, an `upsert` after the prompt: an assistant entry,
//!   `status = "streaming"`, its one text part holding the first chunk (a
//!   part is new to the engine's diff, so it comes whole, never appended);
//! - one `append` each chunk after it: the part's new bytes alone, with the
//!   part's whole length after it (`len`, the desync tripwire), as the
//!   engine's `try_text_append` diffs a text part that grew;
//! - the turn's end, an `upsert` of the whole entry, `status = "complete"`
//!   (a status change is never an append: the engine re-sends the entry).
//!
//! Each delta carries `count`, the transcript's length after it. Standalone
//! (serde_json alone): examples/stream_bench_record.rs writes the standard
//! recording with it, and the bench's test builds a short one.

use serde_json::{Value as Json, json};

/// The chat the bench opens (ui/stream_bench_fixture.luau shows it).
pub const CHAT: &str = "stream-bench";
/// The prompt's entry, and the reply's and its text part's.
pub const PROMPT: &str = "bench-prompt";
pub const REPLY: &str = "bench-reply";
pub const PART: &str = "bench-reply-text";
const DEVICE: &str = "bench-device";
/// Bytes a chunk, as tests/streaming_cost.rs cuts its reply.
pub const CHUNK: usize = 24;

/// A long reply of `parts` sections, each about 700 bytes: tests/streaming_cost.rs's
/// `reply`, byte for byte (`reply(80)` is 41,697 bytes).
pub fn reply(parts: usize) -> String {
    let mut out = String::new();
    for i in 1..=parts {
        out.push_str(&format!(
            "## Part {i}: what it costs\n\n\
             The plan costs $5 and $10 a seat, or \\$3 with the discount; the bound is $O(n \\log n)$ for part {i}, \
             and **the rest** follows from it. A [link](https://example.com/{i}) and `inline code` too.\n\n\
             $$\n\\sum_{{k=1}}^{{n}} k = \\frac{{n(n+1)}}{{2}}\n$$\n\n\
             | Item | Price | Note |\n|---|---|---|\n| seat {i} | $12 | monthly |\n| team | $99 | yearly, $x^2$ |\n\n\
             ```rust\nfn part_{i}(x: u32) -> u32 {{\n    // costs $$ nothing, `really`\n    x * {i}\n}}\n```\n\n\
             - first point, with `a $b$ c` and $x_{i}^2$\n- second point, about $20 even\n\n"
        ));
    }
    out
}

/// The cut points: `size` bytes a chunk, on character boundaries
/// (tests/streaming_cost.rs's `chunks`).
pub fn chunks(text: &str, size: usize) -> Vec<usize> {
    let mut at = Vec::new();
    let mut end = 0;
    while end < text.len() {
        end = (end + size).min(text.len());
        while !text.is_char_boundary(end) {
            end += 1;
        }
        at.push(end);
    }
    at
}

/// The reply entry with `text` so far.
fn reply_entry(text: &str, status: &str, at_ms: i64) -> Json {
    let mut entry = json!({
        "id": REPLY,
        "role": "assistant",
        "parts": [{ "id": PART, "kind": "text", "text": text }],
        "createdAt": at_ms + 1_000,
        "deviceId": DEVICE,
        "status": status,
    });
    if status != "streaming" {
        // (Stamped as the segment finishes: 40 chunks a second's worth.)
        entry["durationMs"] = json!(chunks(text, CHUNK).len() as i64 * 25);
    }
    entry
}

/// A delta frame, every field the engine serializes (`#[serde(default)]`
/// lists, empty ones included).
fn delta(upsert: Vec<Json>, append: Vec<Json>) -> Json {
    json!({ "upsert": upsert, "append": append, "remove": [], "count": 2 })
}

/// `text` streamed as a reply in `chunk`-byte appends, the prompt sent at
/// `at_ms` (epoch millis): the frames, in order (see the module's doc).
pub fn frames(text: &str, chunk: usize, at_ms: i64) -> Vec<Json> {
    let prompt = json!({
        "id": PROMPT,
        "role": "user",
        "parts": [{ "id": "bench-prompt-text", "kind": "text", "text": "Write the long reply: headings, prices, math, a table, code and a list, eighty times." }],
        "createdAt": at_ms,
        "deviceId": DEVICE,
    });
    let mut out = vec![json!({ "reset": [prompt] })];
    let cuts = chunks(text, chunk);
    let first = cuts.first().copied().unwrap_or(0);
    out.push(delta(
        vec![json!({ "after": PROMPT, "entry": reply_entry(&text[..first], "streaming", at_ms) })],
        vec![],
    ));
    let mut from = first;
    for &end in cuts.iter().skip(1) {
        out.push(delta(
            vec![],
            vec![json!({ "entry": REPLY, "part": PART, "text": &text[from..end], "len": end })],
        ));
        from = end;
    }
    out.push(delta(
        vec![json!({ "after": PROMPT, "entry": reply_entry(text, "complete", at_ms) })],
        vec![],
    ));
    out
}

/// The standard benchmark's frames: `reply(80)` in 24-byte chunks.
pub fn standard(at_ms: i64) -> Vec<Json> {
    frames(&reply(80), CHUNK, at_ms)
}

/// The `i`th call of a tool group: a command (with output), a read, a search
/// or an edit (src/activity_cost_tests.rs's `call`).
fn call(i: usize) -> Json {
    let id = format!("t{i}");
    match i % 4 {
        0 => json!({ "id": id, "kind": "tool", "call": { "kind": "exec", "command": format!("cargo test --package part{i} -- --nocapture") }, "output": format!("line 0 of command {i}: ok\nline 1 of command {i}: ok\n"), "exitCode": 0 }),
        1 => json!({ "id": id, "kind": "tool", "call": { "kind": "readFile", "path": format!("/repo/src/part{i}.rs") }, "output": format!("fn part{i}() {{}}\n") }),
        2 => json!({ "id": id, "kind": "tool", "call": { "kind": "search", "pattern": format!("fn part{i}"), "path": "/repo/src" } }),
        _ => json!({ "id": id, "kind": "tool", "call": { "kind": "editFile", "path": format!("/repo/src/part{i}.rs") }, "diffStats": [{ "path": format!("/repo/src/part{i}.rs"), "additions": 3, "deletions": 1 }] }),
    }
}

/// A reply at work on a long tool group, as the engine sends it with part
/// patches (`partPatches`): the prompt (a reset), the reply's row with a
/// thought and `calls` calls (an upsert, streaming), then `arrivals` calls,
/// each in two frames: arriving (its call alone, a part added) and finished
/// (its output, the part again in its place). `settled`: the row comes
/// complete instead, and no calls follow (a group to scroll).
pub fn tools(calls: usize, arrivals: usize, settled: bool, at_ms: i64) -> Vec<Json> {
    let prompt = json!({
        "id": PROMPT,
        "role": "user",
        "parts": [{ "id": "bench-prompt-text", "kind": "text", "text": "Go through every part and fix what fails." }],
        "createdAt": at_ms,
        "deviceId": DEVICE,
    });
    let mut parts = vec![json!({ "id": "r0", "kind": "reasoning", "text": "Looking at each part in turn, running its tests, reading what fails and editing it." })];
    parts.extend((1..=calls).map(call));
    let mut entry = json!({
        "id": REPLY,
        "role": "assistant",
        "parts": parts,
        "createdAt": at_ms + 1_000,
        "deviceId": DEVICE,
        "status": if settled { "complete" } else { "streaming" },
    });
    if settled {
        entry["durationMs"] = json!(600_000);
    }
    let mut out = vec![json!({ "reset": [prompt] }), delta(vec![json!({ "after": PROMPT, "entry": entry })], vec![])];
    if settled {
        return out;
    }
    let patch = |at: usize, part: Json| {
        json!({ "upsert": [], "append": [], "remove": [], "count": 2, "parts": [{ "entry": REPLY, "at": at, "part": part, "len": at + 1 }] })
    };
    for k in 0..arrivals {
        let i = calls + 1 + k;
        let at = calls + 1 + k;
        let done = call(i);
        let mut arriving = done.clone();
        if let Some(o) = arriving.as_object_mut() {
            o.remove("output");
            o.remove("exitCode");
            o.remove("diffStats");
        }
        out.push(patch(at, arriving));
        out.push(patch(at, done));
    }
    out
}
