//! What a streaming reply costs a chunk, by how much of it is already there:
//! a long reply (about 40 KB: headings, tables, code, `$` amounts, inline and
//! display math) arrives in small chunks, the way the host sends it (the
//! whole text so far in `chat.draft`, src/chat.rs `publish_draft`), and each
//! chunk's work is timed:
//!
//! - `parse`: the transcript's parse alone (ds/markdown_find `parse` with
//!   the reply's key: the math read first, then `ui.markdown`, pictures
//!   together), on `ui/markdown_stream_fixture.luau`;
//! - `frame`: the frame after the chunk lands, in the chat fixture's
//!   transcript (`ui/chat_fixture.luau`, a reply in text blocks as the host
//!   sends one, fading in as it arrives): everything a chunk sets off,
//!   with a frame after it where nothing is new beside it (what drawing
//!   the reply costs alone) and the calls into scripts a chunk makes.
//!
//! The per-chunk cost must not grow with the reply. A benchmark, not a
//! check (timings vary by machine): run it in release and read the table,
//!
//!     cargo test --release --test streaming_cost -- --ignored --nocapture

use std::path::PathBuf;
use std::time::{Duration, Instant};

use zgx_script::{ScriptHost, Value, testing::TestApp};

fn ui_dir() -> PathBuf {
    std::env::var_os("ZERON_ZGX_TEST_UI")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui"))
}

/// A long reply of `parts` sections, each about 700 bytes.
fn reply(parts: usize) -> String {
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

/// The cut points: `size` bytes a chunk, on character boundaries.
fn chunks(text: &str, size: usize) -> Vec<usize> {
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

/// Medians of `samples` (`(length, time)`) in buckets of `bucket` bytes.
fn table(name: &str, samples: &[(usize, Duration)], bucket: usize) {
    eprintln!("{name}: per-chunk time by reply length (median, p90; µs)");
    let mut from = 0;
    while from < samples.last().map_or(0, |s| s.0) {
        let mut times: Vec<f64> = samples
            .iter()
            .filter(|(len, _)| *len > from && *len <= from + bucket)
            .map(|(_, t)| t.as_secs_f64() * 1e6)
            .collect();
        times.sort_by(|a, b| a.partial_cmp(b).unwrap());
        if !times.is_empty() {
            let median = times[times.len() / 2];
            let p90 = times[(times.len() * 9 / 10).min(times.len() - 1)];
            let max = times[times.len() - 1];
            eprintln!("  {:>3}-{:>3} KB  {median:>9.1}  {p90:>9.1}  (max {max:.1})", from / 1024, (from + bucket) / 1024);
        }
        from += bucket;
    }
}

const CHUNK: usize = 24;
const BUCKET: usize = 4096;

#[test]
#[ignore = "a benchmark: run in release with --ignored --nocapture"]
fn a_streaming_reply_costs_a_chunk_what_is_new() {
    let text = reply(80);
    let cuts = chunks(&text, CHUNK);
    eprintln!("reply: {} bytes in {} chunks of {CHUNK}", text.len(), cuts.len());

    // The parse alone.
    let app = TestApp::new(|runtime| ScriptHost::new(runtime, ui_dir().join("markdown_stream_fixture.luau")));
    app.frames(2);
    let source = app.host.shared("bench.text", Value::from(""));
    let mut parse = Vec::new();
    for &end in &cuts {
        source.set(Value::from(&text[..end]));
        let start = Instant::now();
        assert!(app.host.run_command("parse"));
        parse.push((end, start.elapsed()));
    }
    assert_eq!(app.error(), None);
    table("parse", &parse, BUCKET);

    // The transcript: the frame each chunk sets off.
    let app = TestApp::new(|runtime| ScriptHost::new(runtime, ui_dir().join("chat_fixture.luau")));
    app.ui.scene.borrow_mut().resize(900., 700.);
    app.frames(2);
    assert!(app.host.run_command("go_s2"));
    app.frames(30);
    std::thread::sleep(Duration::from_millis(320));
    assert!(app.host.run_command("start_block_reply"));
    app.frames(5);
    let draft = app.host.shared("chat.draft", Value::Nil);
    let mut frame = Vec::new();
    let mut idle = Vec::new();
    let mut calls = Vec::new();
    for &end in &cuts {
        let start = Instant::now();
        // (As the host sends it: the words twice, the text block's and the reply's.)
        let words = Value::from(&text[..end]);
        draft.set(Value::record([
            ("id", Value::from("live")),
            ("text", words.clone()),
            ("part", Value::from("t")),
            ("part_text", words),
        ]));
        let called = app.calls();
        app.frames(1);
        frame.push((end, start.elapsed()));
        calls.push((end, Duration::from_micros(app.calls() - called)));
        // (A frame with nothing new: what drawing the reply costs alone.)
        let start = Instant::now();
        app.frames(1);
        idle.push((end, start.elapsed()));
        if let Some(e) = app.error() {
            panic!("at {end}: {e}\n{}", &text[end.saturating_sub(200)..end]);
        }
    }
    assert_eq!(app.error(), None);
    table("frame", &frame, BUCKET);
    table("idle frame (nothing new)", &idle, BUCKET);
    table("script calls a chunk (as µs: a count)", &calls, BUCKET);
}
