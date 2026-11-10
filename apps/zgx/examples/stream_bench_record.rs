//! Writes the stream bench's standard recording (src/stream_bench.rs): the
//! engine's `WatchDocMessages` frames for tests/streaming_cost.rs's
//! `reply(80)` (41,697 bytes) streaming as an assistant reply in 24-byte
//! appends, one JSON frame a line (src/stream_bench_record.rs says which).
//!
//!     cargo run --features stream-bench --example stream_bench_record -- /tmp/reply80.jsonl

#[allow(dead_code)]
#[path = "../src/stream_bench_record.rs"]
mod record;

use std::io::Write;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args()
        .nth(1)
        .ok_or("usage: stream_bench_record <out.jsonl>")?;
    let at_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_millis() as i64;
    // (`--tools <calls> <arrivals>`: a tool group at work, by part patches;
    // `--tools-settled <calls>`: one done, to scroll.)
    let args: Vec<String> = std::env::args().collect();
    let num = |i: usize| args.get(i).and_then(|v| v.parse::<usize>().ok()).unwrap_or(1000);
    let frames = match args.get(2).map(String::as_str) {
        Some("--tools") => record::tools(num(3), num(4), false, at_ms),
        Some("--tools-settled") => record::tools(num(3), 0, true, at_ms),
        // (A turn at work with nothing arriving: its row streaming, no more.)
        Some("--tools-working") => record::tools(num(3), 0, false, at_ms),
        // (A long chat at rest, to scroll: `--chat <turns>`.)
        Some("--chat") => record::chat(num(3), at_ms),
        _ => record::standard(at_ms),
    };
    let mut out = std::io::BufWriter::new(std::fs::File::create(&path)?);
    for frame in &frames {
        serde_json::to_writer(&mut out, frame)?;
        out.write_all(b"\n")?;
    }
    out.flush()?;
    let text = record::reply(80);
    eprintln!(
        "{path}: {} frames (a reset, the reply's row with its first {} bytes, {} appends of {} each, its end): {} bytes of reply",
        frames.len(),
        record::CHUNK,
        frames.len().saturating_sub(3),
        record::CHUNK,
        text.len()
    );
    Ok(())
}
