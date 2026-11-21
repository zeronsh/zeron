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

/// What holds a long reply's blocks while its rows show (src/chat.rs
/// `set_aside`).
type Held = Vec<std::rc::Rc<dyn std::any::Any>>;

/// The thread's two entries: the prompt and its long reply.
fn long_thread_entries(steps: usize) -> (Json, Json) {
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
    (prompt, reply)
}

/// The thread as the engine holds it (`/longthread <steps>`), built into its
/// rows as a chat's view builds them, and what holds what they set aside.
fn long_thread(steps: usize) -> (Value, Held) {
    let (prompt, reply) = long_thread_entries(steps);
    let mut held: Held = Vec::new();
    let rows: Vec<Value> = [prompt, reply]
        .iter()
        .map(|entry| {
            let (row, aside) = crate::chat::built_as_shown(entry, Some("Opus"));
            held.extend(aside);
            row.unwrap()
        })
        .collect();
    (Value::List(rows.into()), held)
}

/// The chat page on another chat, then the thread of `steps` opened in it:
/// the app, the script calls its opening took (until it rests), the nodes
/// its window holds then, the values its rows handed the scripts, and what
/// holds its reply's blocks.
fn opened(steps: usize) -> (TestApp, u64, usize, u64, Held) {
    let fixture = ui().join("chat_fixture.luau");
    let mut app = TestApp::with_spare_time(move |runtime| {
        let host = ScriptHost::new(runtime, fixture);
        crate::chat::reply_functions(&host);
        host
    });
    app.ui.scene.borrow_mut().resize(960., 700.);
    app.frames(20);
    assert!(app.host.run_command("go_s7"));
    for _ in 0..30 {
        app.spend_spare_time();
        app.frame();
    }
    let messages = app.host.shared("chat.messages", Value::Nil);
    let calls = app.calls();
    let converted = zgx_script::values_converted();
    let (rows, held) = long_thread(steps);
    messages.set(rows);
    for _ in 0..30 {
        app.spend_spare_time();
        app.frame();
    }
    assert_eq!(app.error(), None);
    let calls = app.calls() - calls;
    let converted = zgx_script::values_converted() - converted;
    let nodes = app.ui.scene.borrow().len();
    let _ = app.find("transcript");
    (app, calls, nodes, converted, held)
}

#[test]
fn a_long_threads_open_costs_what_its_end_costs() {
    let (mut long, calls, nodes, converted, _held) = opened(1000);
    let (_, short_calls, short_nodes, short_converted, _short_held) = opened(100);
    eprintln!(
        "open: 1000 steps {calls} calls, {nodes} nodes, {converted} values; 100 steps {short_calls} calls, {short_nodes} nodes, {short_converted} values"
    );
    // What its rows hand the scripts is its end's too: its blocks are the
    // host's, a run asked for as it's built. (Before, every one of its
    // blocks and tools, and again laid out compact: 22,117 values for 1000
    // steps, 4,896 for 100. Now 1,886 and 3,350.)
    assert!(converted < short_converted * 5 / 4, "1000 steps handed the scripts {converted} values, 100 {short_converted}");
    assert!(converted < 3_000, "1000 steps handed the scripts {converted} values as it opened");
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
    let (mut app, _, _, _, _held) = opened(steps);
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

/// This thread's instructions retired in user mode so far (Linux,
/// `perf_event_open`: needs `kernel.perf_event_paranoid` to allow it, or
/// root), or none.
#[cfg(target_os = "linux")]
fn instructions() -> Option<impl Fn() -> u64> {
    // perf_event_attr, 128 bytes: type HARDWARE (0) and its size, config
    // INSTRUCTIONS (1), and in its flags exclude_kernel and exclude_hv.
    let mut attr = [0u64; 16];
    attr[0] = 128 << 32;
    attr[1] = 1;
    attr[5] = (1 << 5) | (1 << 6);
    // SAFETY: perf_event_open(attr, this thread, any cpu, no group, no flags).
    let fd = unsafe { libc::syscall(libc::SYS_perf_event_open, attr.as_ptr(), 0, -1, -1, 0) } as i32;
    (fd >= 0).then_some(move || {
        let mut count = 0u64;
        // SAFETY: A counter's read is one u64.
        unsafe { libc::read(fd, std::ptr::from_mut(&mut count).cast(), 8) };
        count
    })
}

/// What the long reply's functions cost a call, by the app's own code or by
/// the reply module (`ZERON_ZGX_REPLY_MODULE=<library>`): run once each way
/// and compare.
///
///   sudo -E env ZERON_ZGX_REPLY_MODULE=<library or a path of the same length> \
///     <the release test binary> reply_module_costs --ignored --nocapture
///
/// Instructions are this thread's, in user mode; a script's loop of `CALLS`
/// calls, less the same loop calling a function that does nothing.
///
/// Where no counter is allowed, count from outside (cachegrind, which needs
/// no privileges): `REPLY_BENCH_CASE=<nothing|run|run_first|blocks|run_of|aside>`
/// runs that one loop `REPLY_BENCH_ROUNDS` times and nothing else, so two
/// runs of different rounds differ by the rounds between them alone.
#[cfg(target_os = "linux")]
#[test]
#[ignore = "a benchmark: release, with a counter (root, or perf_event_paranoid <= 2)"]
fn reply_module_costs() {
    const CALLS: usize = 20_000;
    const ROUNDS: usize = 21;
    let only = std::env::var("REPLY_BENCH_CASE").ok();
    let steps = 1000;
    let (rows, _held) = long_thread(steps);
    // The reply's runs, as its row lists them.
    let reply = &rows.list()[1];
    let runs = reply.field("runs").expect("a long reply's runs").list().to_vec();
    let key = |i: usize| match runs[i].field("key") {
        Some(Value::Str(k)) => k.to_string(),
        _ => unreachable!(),
    };
    let (first, last) = (key(0), key(runs.len() - 1));
    let blocks = steps * 2 + 1;
    let script = format!(
        r#"
        local function loop(name, arg)
            return function()
                for _ = 1, {CALLS} do ui.call_sync(name, arg) end
            end
        end
        ui.command {{ id = "nothing", title = "n", run = loop("bench.nothing", {{ id = "lt-a", key = "{last}" }}) }}
        ui.command {{ id = "run", title = "r", run = loop("chat.reply_run", {{ id = "lt-a", key = "{last}" }}) }}
        ui.command {{ id = "run_first", title = "f", run = loop("chat.reply_run", {{ id = "lt-a", key = "{first}" }}) }}
        ui.command {{ id = "blocks", title = "b", run = loop("chat.reply_blocks", "lt-a") }}
        ui.command {{ id = "run_of", title = "o", run = loop("chat.reply_run_of", {{ id = "lt-a", block = "{first}" }}) }}
        ui.command {{ id = "check", title = "c", run = function()
            assert(#ui.call_sync("chat.reply_blocks", "lt-a") == {blocks}, "chat.reply_blocks")
            assert(#ui.call_sync("chat.reply_run", {{ id = "lt-a", key = "{last}" }}) > 0, "chat.reply_run, the last")
            assert(#ui.call_sync("chat.reply_run", {{ id = "lt-a", key = "{first}" }}) > 0, "chat.reply_run, the first")
            assert(ui.call_sync("chat.reply_run_of", {{ id = "lt-a", block = "{first}" }}) == "{first}", "chat.reply_run_of")
        end }}
        return ui.column {{}}
        "#
    );
    let app = TestApp::new(move |runtime| {
        // (Counted from outside, a loop runs some hundred times slower: its
        // budgets are lengthened to match, the same in both arms.)
        let limits = zgx_script::Limits::default();
        let limits = zgx_script::Limits { load: limits.load * 1000, call: limits.call * 1000, ..limits };
        let host = ScriptHost::inline(runtime, "bench", script).limits(limits);
        crate::chat::reply_functions(&host);
        host.function("bench.nothing", |_| Value::Nil);
        host
    });
    assert_eq!(app.error(), None);
    // After the host loaded: the rows the scripts ask about are this arm's.
    let (rows, _held) = long_thread(steps);
    drop(rows);
    // Whole, to set aside again and again.
    let whole = crate::chat::built_unset(&long_thread_entries(steps).1, Some("Opus")).unwrap();
    // The loops read a reply that is held: let go of, each function answers
    // nil at once, and a loop of those measures nothing of the functions.
    // (Setting it aside again takes its place and lets go of it, so the
    // loops come first.)
    assert!(app.host.run_command("check"));
    assert_eq!(app.error(), None, "the reply's functions answer while it's held");
    if let Some(case) = only {
        let rounds: usize = std::env::var("REPLY_BENCH_ROUNDS").ok().and_then(|n| n.parse().ok()).unwrap_or(1);
        let arm = if crate::chat::reply_by_module() { "module" } else { "linked" };
        for _ in 0..rounds {
            if case == "aside" {
                let (kept, hold, _) = crate::chat::set_aside_as_shown(whole.clone());
                drop((kept, hold));
            } else {
                assert!(app.host.run_command(&case), "{case}");
            }
        }
        assert_eq!(app.error(), None, "{case}");
        println!("BENCH {arm} {case} rounds={rounds} calls={CALLS}");
        return;
    }
    let count = instructions().expect("perf_event_open: run as root or allow it");
    let median = |mut all: Vec<u64>| {
        all.sort_unstable();
        (all[all.len() / 2], all[0], all[all.len() - 1])
    };
    let timed = |command: &str| {
        let mut counts = Vec::with_capacity(ROUNDS);
        let mut nanos = Vec::with_capacity(ROUNDS);
        for _ in 0..ROUNDS {
            let (at, from) = (std::time::Instant::now(), count());
            assert!(app.host.run_command(command));
            counts.push(count() - from);
            nanos.push(at.elapsed().as_nanos() as u64);
        }
        assert_eq!(app.error(), None, "{command}");
        (median(counts), median(nanos))
    };
    let arm = if crate::chat::reply_by_module() { "module" } else { "linked" };
    let ((nothing, ..), _) = timed("nothing");
    println!("arm={arm} calls={CALLS} rounds={ROUNDS} runs={} blocks={blocks}", runs.len());
    println!("BENCH {arm} empty_loop_instr_per_call {:.1}", nothing as f64 / CALLS as f64);
    for command in ["run", "run_first", "blocks", "run_of"] {
        let ((mid, low, high), (ns, ..)) = timed(command);
        let per = |n: u64| (n as f64 - nothing as f64) / CALLS as f64;
        println!(
            "BENCH {arm} {command} instr_per_call {:.1} min {:.1} max {:.1} total_per_call {:.1} ns_per_call {:.1}",
            per(mid),
            per(low),
            per(high),
            mid as f64 / CALLS as f64,
            ns as f64 / CALLS as f64
        );
    }
    let mut aside_counts = Vec::new();
    let mut aside_nanos = Vec::new();
    // (`REPLY_BENCH_ASIDE=<rounds>`: more of them, for a profile.)
    let aside_rounds = std::env::var("REPLY_BENCH_ASIDE").ok().and_then(|n| n.parse().ok()).unwrap_or(ROUNDS);
    for _ in 0..aside_rounds {
        let row = whole.clone();
        let (at, from) = (std::time::Instant::now(), count());
        let (kept, hold, _) = crate::chat::set_aside_as_shown(row);
        drop((kept, hold));
        aside_counts.push(count() - from);
        aside_nanos.push(at.elapsed().as_nanos() as u64);
    }
    let ((mid, low, high), (ns, ..)) = (median(aside_counts), median(aside_nanos));
    println!("BENCH {arm} set_aside instr {mid} min {low} max {high} ns {ns}");
}
