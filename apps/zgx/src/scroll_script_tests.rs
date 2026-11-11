//! What a steady scroll of the transcript costs in script: the chat page's
//! fixture (ui/chat_fixture.luau) mounted headless, with an inspector so
//! each run is named, its transcript a long chat of markdown replies built
//! by src/chat.rs, scrolled a wheel's step (8 points) a frame. For each
//! frame of the scroll, the script's instructions (`ZGX_PROFILE`) and the
//! runs it made, by name; a steady scroll's frames should run none.
//!
//!   cargo test --release --bin zeron-zgx scroll_script -- --ignored --nocapture
//!
//! (`SCROLL_TURNS`, 60 unless set: how many prompt-and-reply turns;
//! `SCROLL_STEPS`, 120: how many frames of the scroll.)

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde_json::{Value as Json, json};
use zgx::profile::{self, Phase};
use zgx_script::{ScriptHost, Value, testing::TestApp};

fn ui() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui")
}

/// A reply of markdown: headings, prose, a list, code and a table.
fn markdown(i: usize) -> String {
    format!(
        "## Part {i}: what changed\n\n\
         The parser reads the frame, then the host applies it; **the rows** that changed are built again, \
         and `chat.rs` hands the transcript its list. A [link](https://example.com/{i}) and `inline code` too.\n\n\
         - first point about part {i}, with `a b c`\n- second point, about the cost\n- third, the rest\n\n\
         ```rust\nfn part_{i}(x: u32) -> u32 {{\n    x * {i}\n}}\n```\n\n\
         | Item | Cost | Note |\n|---|---|---|\n| row {i} | 12 | monthly |\n| team | 99 | yearly |\n\n\
         Whether part {i} holds depends on what the group reads per update, and on how the list lays out \
         what it keeps as the view moves past it.\n"
    )
}

fn built(entry: &Json) -> Value {
    crate::chat::built_entry(entry, "doc", None, Some("Opus")).expect("a row")
}

#[test]
#[ignore = "a benchmark: run it --release"]
fn scroll_script() {
    // SAFETY: set before any thread reads the environment.
    unsafe { std::env::set_var("ZGX_PROFILE", "1") };
    assert!(profile::enabled());
    let turns: usize = std::env::var("SCROLL_TURNS").ok().and_then(|v| v.parse().ok()).unwrap_or(60);
    let fixture = ui().join("chat_fixture.luau");
    // (With time frames leave over, as a window has: rows built ahead of the
    // scroll in it, not in its frames. `SCROLL_NAMED=1`: an inspector names
    // the runs instead, with no spare time.)
    let named = std::env::var_os("SCROLL_NAMED").is_some();
    let inspector = zgx::inspect::Inspector::new();
    let mut app = if named {
        TestApp::inspected(move |runtime| ScriptHost::new(runtime, fixture), &inspector)
    } else {
        TestApp::with_spare_time(move |runtime| ScriptHost::new(runtime, fixture))
    };
    app.ui.scene.borrow_mut().resize(1240., 820.);
    app.frames(2);
    assert!(app.host.run_command("go_s2"), "the fixture opens s2");
    app.frames(30);
    let messages = app.host.shared("chat.messages", Value::Nil);
    let mut rows = Vec::new();
    for i in 0..turns {
        let at = 1_762_000_000_000_i64 + i as i64 * 60_000;
        rows.push(built(&json!({ "id": format!("u{i}"), "role": "user", "parts": [{ "id": format!("u{i}p"), "kind": "text", "text": format!("Question {i}: what about part {i}?") }], "createdAt": at })));
        rows.push(built(&json!({ "id": format!("a{i}"), "role": "assistant", "status": "complete", "parts": [{ "id": format!("a{i}x"), "kind": "text", "text": markdown(i) }], "createdAt": at + 1000, "durationMs": 8000 })));
    }
    messages.set(Value::List(rows.into()));
    app.frames(90);
    // Up from the end a way, then a steady scroll on up.
    let node = app.find("transcript").expect("the transcript");
    for _ in 0..20 {
        zgx_script::driver::scroll(&mut app.ui, node, -200.);
        app.frames(3);
    }
    for _ in 0..60 {
        app.spend_spare_time();
        app.frame();
    }
    let steps: usize = std::env::var("SCROLL_STEPS").ok().and_then(|v| v.parse().ok()).unwrap_or(120);
    let _ = profile::take();
    eprintln!("SCROLL steady scroll starts");
    zgx::trace::set_enabled(true);
    let mut script = Vec::with_capacity(steps);
    for _ in 0..steps {
        if !named {
            app.spend_spare_time();
            let _ = profile::take();
        }
        zgx_script::driver::scroll(&mut app.ui, node, -8.);
        app.frame();
        let phases = profile::take();
        script.push(phases.iter().filter(|p| p.0 == Phase::Script).map(|p| p.1).sum::<u64>());
    }
    let mut names: BTreeMap<String, usize> = BTreeMap::new();
    let mut causes: BTreeMap<usize, (usize, BTreeMap<String, usize>)> = BTreeMap::new();
    let mut micros: BTreeMap<usize, f64> = BTreeMap::new();
    let (mut runs, mut writes, mut written) = (0, 0, 0);
    for frame in zgx::trace::frames(4 * steps) {
        written += frame.writes.len();
        writes += frame.writes.iter().map(|w| w.1 as usize).sum::<usize>();
        for run in &frame.runs {
            runs += 1;
            let name = run.name.as_deref().unwrap_or("?").to_owned();
            *names.entry(name.clone()).or_default() += 1;
            let c = causes.entry(run.cause.unwrap_or(0)).or_default();
            c.0 += 1;
            *micros.entry(run.cause.unwrap_or(0)).or_default() += run.micros as f64;
            *c.1.entry(name).or_default() += 1;
        }
    }
    println!("SCROLL signals written: {:.1} a frame ({:.1} writes)", written as f64 / steps as f64, writes as f64 / steps as f64);
    let mut by_cause: Vec<_> = causes.into_iter().collect();
    by_cause.sort_by(|a, b| b.1.0.cmp(&a.1.0));
    for (cause, (n, names)) in by_cause.iter().take(8) {
        let mut top: Vec<_> = names.iter().collect();
        top.sort_by(|a, b| b.1.cmp(a.1));
        let top: Vec<String> = top.iter().take(4).map(|(k, v)| format!("{k} x{v}")).collect();
        println!("SCROLL cause {cause:#x}: {n} runs, {:.0} µs: {}", micros.get(cause).copied().unwrap_or(0.), top.join(", "));
    }
    zgx::trace::set_enabled(false);
    assert_eq!(app.error(), None);
    script.sort_unstable();
    let total: u64 = script.iter().sum();
    println!(
        "SCROLL {turns} turns, {steps} frames of 8 pt: script {:.0}k instructions a frame on average, median {:.0}k, most {:.0}k; {runs} runs ({:.1} a frame)",
        total as f64 / steps as f64 / 1e3,
        script[steps / 2] as f64 / 1e3,
        script[steps - 1] as f64 / 1e3,
        runs as f64 / steps as f64,
    );
    let mut named: Vec<(String, usize)> = names.into_iter().collect();
    named.sort_by(|a, b| b.1.cmp(&a.1));
    for (name, n) in named.iter().take(25) {
        println!("SCROLL   {n:>5}  {name}");
    }
}
