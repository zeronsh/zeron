//! Writes apps/zgx/src/live_frames_fixture.json: a reply as the engine sends
//! it while it's written (a reset, then text appends, part patches and an
//! upsert, from the engine's own diff), with the engine's `entry_sum` of
//! every entry after each frame. The app's test applies the frames and
//! checks its sums agree; otherwise every reopen would silently be a reset.
//!
//! cargo run -p zeron-doc --example app_sum_fixture > apps/zgx/src/live_frames_fixture.json
use serde_json::json;
use zeron_doc::*;

fn tool(id: &str, output: Option<&str>) -> MessagePart {
    serde_json::from_value(json!({
        "kind": "tool", "id": id,
        "call": { "kind": "exec", "command": "cargo test -p zeron-doc" },
        "isError": false, "resolved": output.is_some(), "output": output,
    }))
    .unwrap()
}

fn main() {
    let text = |id: &str, t: &str| MessagePart::Text { id: id.into(), text: t.into() };
    let user = SessionMessageEntry {
        id: "u1".into(),
        role: MessageRole::User,
        parts: vec![text("u1-t", "Run the tests, then say how it went.")],
        created_at: 1_762_000_000_000,
        device_id: "dev".into(),
        status: None,
        continuation_of: None,
        duration_ms: None,
    };
    let reply = |parts: Vec<MessagePart>, status: MessageStatus, duration: Option<i64>| SessionMessageEntry {
        id: "a1".into(),
        role: MessageRole::Assistant,
        parts,
        created_at: 1_762_000_000_500,
        device_id: "dev".into(),
        status: Some(status),
        continuation_of: None,
        duration_ms: duration,
    };
    let versions = vec![
        vec![user.clone(), reply(vec![text("a1-t", "I'll ")], MessageStatus::Streaming, None)],
        vec![user.clone(), reply(vec![text("a1-t", "I'll run the tests – «zeron-doc» first.")], MessageStatus::Streaming, None)],
        vec![user.clone(), reply(vec![text("a1-t", "I'll run the tests – «zeron-doc» first."), tool("a1-c", None)], MessageStatus::Streaming, None)],
        vec![user.clone(), reply(vec![text("a1-t", "I'll run the tests – «zeron-doc» first."), tool("a1-c", Some("test result: ok. 198 passed"))], MessageStatus::Streaming, None)],
        vec![user.clone(), reply(vec![text("a1-t", "I'll run the tests – «zeron-doc» first."), tool("a1-c", Some("test result: ok. 198 passed")), text("a1-u", "All 198 pass.")], MessageStatus::Streaming, None)],
        vec![user.clone(), reply(vec![text("a1-t", "I'll run the tests – «zeron-doc» first."), tool("a1-c", Some("test result: ok. 198 passed")), text("a1-u", "All 198 pass. 🎉")], MessageStatus::Complete, Some(4200))],
    ];
    let options = DiffOptions { part_patches: true };
    let sums = |entries: &[SessionMessageEntry]| -> Vec<serde_json::Value> {
        entries.iter().map(|e| json!([e.id, entry_sum_of(e).to_string()])).collect()
    };
    let mut steps = vec![json!({ "frame": TranscriptFrame::reset(&versions[0]), "sums": sums(&versions[0]) })];
    for pair in versions.windows(2) {
        let frame = diff_transcript_with(&pair[0], &pair[1], options);
        steps.push(json!({ "frame": frame, "sums": sums(&pair[1]) }));
    }
    println!("{}", serde_json::to_string_pretty(&json!({ "steps": steps })).unwrap());
}
