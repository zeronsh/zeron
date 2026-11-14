//! A long thread, as ademark's A11 makes it: one reply of a thousand steps,
//! each a short text (one in five with a code block) and a call reading a
//! file, about 2,000 blocks, its last text holding `LONGTHREAD-END` (the
//! engine's mock `/longthread`). Its rows built by src/chat.rs from the
//! engine's entry, as the app builds them.
//!
//! Opening it costs what its end costs: a reply's blocks are windowed in runs
//! (ds/transcript's `blocks_of`), so the scripts a thousand steps call and
//! the nodes they make are about a hundred steps'. (Nov 13: 0.3.4 built all
//! 2,000 blocks' rows on every open, 44,000 script runs in one frame, about
//! 380 ms on an M1 where 0.2 took 96 ms.) Scrolled up, it passes through
//! every step to its first.

use std::path::PathBuf;

use serde_json::{Value as Json, json};
use zgx_script::{ScriptHost, Value, testing::TestApp};

use crate::transcript_scroll_tests::{moved, shown};

fn ui() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui")
}

/// The thread as the engine holds it (`/longthread <steps>`), built into its
/// rows as src/chat.rs builds them.
fn long_thread(steps: usize) -> Value {
    let mut parts: Vec<Json> = Vec::with_capacity(steps * 2 + 1);
    for n in 1..=steps {
        let mut text = format!("Step {n} of {steps}. I read src/invoices.ts and noted what step {n} needs from it.");
        if n % 5 == 0 {
            text.push_str("\n\n```ts\nfunction summarize(invoice: Invoice): InvoiceSummary {\n  const total = invoiceTotal(invoice);\n  return { id: invoice.id, totalCents: total };\n}\n```");
        }
        parts.push(json!({ "id": format!("t{}", 2 * (n - 1)), "kind": "text", "text": text }));
        parts.push(json!({
            "id": format!("toolu_long_{n:04}"),
            "kind": "tool",
            "call": { "kind": "readFile", "path": "/private/tmp/ademark-recording/arena/src/invoices.ts" },
            "isError": false,
            "resolved": true,
        }));
    }
    parts.push(json!({ "id": format!("t{}", 2 * steps), "kind": "text", "text": "All steps done. LONGTHREAD-END" }));
    let at = 1_794_000_000_000_i64;
    let prompt = json!({ "id": "lt-u", "role": "user", "parts": [{ "id": "lt-up", "kind": "text", "text": format!("/longthread {steps}") }], "createdAt": at });
    let reply = json!({ "id": "lt-a", "role": "assistant", "status": "complete", "parts": parts, "createdAt": at + 1000, "durationMs": 28 });
    let rows: Vec<Value> = [prompt, reply]
        .iter()
        .map(|entry| crate::chat::built_entry(entry, "doc", None, Some("Opus")).unwrap())
        .collect();
    Value::List(rows.into())
}

/// The chat page on another chat, then the thread of `steps` opened in it:
/// the app, the script calls its opening took (until it rests), and the
/// nodes its window holds then.
fn opened(steps: usize) -> (TestApp, u64, usize) {
    let fixture = ui().join("chat_fixture.luau");
    let mut app = TestApp::with_spare_time(move |runtime| ScriptHost::new(runtime, fixture));
    app.ui.scene.borrow_mut().resize(960., 700.);
    app.frames(20);
    assert!(app.host.run_command("go_s7"));
    for _ in 0..30 {
        app.spend_spare_time();
        app.frame();
    }
    let messages = app.host.shared("chat.messages", Value::Nil);
    let calls = app.calls();
    messages.set(long_thread(steps));
    for _ in 0..30 {
        app.spend_spare_time();
        app.frame();
    }
    assert_eq!(app.error(), None);
    let calls = app.calls() - calls;
    let nodes = app.ui.scene.borrow().len();
    let _ = app.find("transcript");
    (app, calls, nodes)
}

#[test]
fn a_long_threads_open_costs_what_its_end_costs() {
    let (mut long, calls, nodes) = opened(1000);
    let (_, short_calls, short_nodes) = opened(100);
    eprintln!("open: 1000 steps {calls} calls, {nodes} nodes; 100 steps {short_calls} calls, {short_nodes} nodes");
    // Ten times the steps, not ten times the nodes: a held run is a box.
    // (Before the window, all their blocks built: 1000 steps held 18,340
    // nodes and took 43,091 calls; 100, 2,148 and 4,383. Now 782 and 962;
    // 985 and 1,583.)
    assert!(nodes < short_nodes * 5 / 4, "1000 steps held {nodes} nodes once open, 100 held {short_nodes}");
    // Budgets of its own, about twice what it takes now.
    assert!(nodes < 2_000, "1000 steps held {nodes} nodes once open");
    assert!(calls < 2_500, "1000 steps took {calls} script calls to open");
    // Its end shows, opened there.
    let end = shown(&mut long);
    assert!(end.iter().any(|(words, _)| words == "All steps done. LONGTHREAD-END"), "the end shows: {end:?}");
}

/// Up from its end a trackpad's step a refresh, what shows moves by the step
/// each time (runs built as they come near, held ones above measured without
/// moving the view), through to its first step; and back to its end.
#[test]
fn a_long_thread_scrolled_up_passes_through_every_step_to_its_first() {
    let steps = 300;
    let (mut app, _, _) = opened(steps);
    let list = app.find("transcript").expect("the transcript");
    let step = 60.;
    let mut skips = Vec::new();
    let mut first = false;
    for i in 0..2000 {
        let before = shown(&mut app);
        if before.iter().any(|(words, _)| words.starts_with(&format!("Step 1 of {steps}."))) {
            first = true;
            break;
        }
        zgx_script::driver::wheel(&mut app.ui, list, 0., -step, None);
        app.spend_spare_time();
        app.frame();
        let after = shown(&mut app);
        let (d, n) = moved(&before, &after);
        if d.is_none_or(|d| (d - step).abs() > 1.) {
            skips.push(format!("step {i}: moved {d:?} ({n} texts in both)"));
        }
    }
    assert_eq!(app.error(), None);
    assert!(skips.is_empty(), "each step of {step} moved the view by {step}:\n{}", skips.join("\n"));
    assert!(first, "its first step came into view");
    // And from its very top, Jump to latest takes it back to its end.
    for _ in 0..30 {
        app.spend_spare_time();
        app.frame();
    }
    app.click("role:button \"Jump to latest\"");
    for _ in 0..90 {
        app.spend_spare_time();
        app.frame();
    }
    let end = shown(&mut app);
    assert!(end.iter().any(|(words, _)| words == "All steps done. LONGTHREAD-END"), "back at its end: {end:?}");
    assert_eq!(app.error(), None);
}
