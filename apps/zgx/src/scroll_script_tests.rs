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

/// What the transcript draws as it scrolls, frame by frame: a long chat of
/// markdown replies, every tenth long (a reply of many windowed runs),
/// drawn offscreen on the test's clock, each drawn frame's pixels hashed.
/// A steady scroll, a fling and jumps (to the top, the end, the middle): run
/// on two trees, the lists the same is the transcript drawing the same.
///
///   SCROLL_PIXELS=<dir> cargo test --release --bin zeron-zgx scroll_pixels -- --ignored --nocapture
#[test]
#[ignore = "a comparison: run it --release"]
fn scroll_pixels() {
    use std::hash::{Hash, Hasher};
    use std::time::Duration;
    const W: f32 = 1240.;
    const H: f32 = 820.;
    const SCALE: f32 = 2.;
    // (Not asked for, as `scroll_script` runs it too: nothing to do.)
    let Some(out) = std::env::var_os("SCROLL_PIXELS").map(PathBuf::from) else {
        eprintln!("scroll_pixels: SCROLL_PIXELS=<dir> runs it");
        return;
    };
    std::fs::create_dir_all(&out).unwrap();
    // (`SCROLL_PIXELS_KEEP=steady:150,rest:90`: those frames' pixels too,
    // raw RGBA at 2x, 16 MB each.)
    let keep: Vec<String> = std::env::var("SCROLL_PIXELS_KEEP").map(|v| v.split(',').map(str::to_owned).collect()).unwrap_or_default();
    let clock = zgx::clock::VirtualClock::new();
    let mut at = Duration::ZERO;
    let fixture = ui().join("chat_fixture.luau");
    let mut app = TestApp::new(move |runtime| ScriptHost::new(runtime, fixture));
    let mut gpu = zgx_gpu::GpuRenderer::new((W * SCALE) as u32, (H * SCALE) as u32).expect("a GPU");
    gpu.set_scale_factor(SCALE);
    for font in crate::fonts() {
        font.expect("the app's fonts").install(&mut gpu.text_system().borrow_mut());
    }
    gpu.install_text(&mut app.ui.scene.borrow_mut());
    app.ui.refresh_text_geometry();
    app.ui.scene.borrow_mut().resize(W, H);
    app.frames(2);
    assert!(app.host.run_command("go_s2"), "the fixture opens s2");
    app.frames(30);
    let messages = app.host.shared("chat.messages", Value::Nil);
    let mut rows = Vec::new();
    for i in 0..60 {
        let at = 1_762_000_000_000_i64 + i as i64 * 60_000;
        let text = if i % 10 == 3 { (0..24).map(|k| markdown(i * 100 + k)).collect::<Vec<_>>().join("\n") } else { markdown(i) };
        rows.push(built(&json!({ "id": format!("u{i}"), "role": "user", "parts": [{ "id": format!("u{i}p"), "kind": "text", "text": format!("Question {i}: what about part {i}?") }], "createdAt": at })));
        rows.push(built(&json!({ "id": format!("a{i}"), "role": "assistant", "status": "complete", "parts": [{ "id": format!("a{i}x"), "kind": "text", "text": text }], "createdAt": at + 1000, "durationMs": 8000 })));
    }
    messages.set(Value::List(rows.into()));
    let mut lines = Vec::new();
    let mut shot = |app: &mut TestApp, name: &str, frames: usize, step: f32| {
        let node = app.find("transcript").expect("the transcript");
        for f in 0..frames {
            if step != 0. {
                zgx_script::driver::scroll(&mut app.ui, node, step);
            }
            at += Duration::from_secs_f64(1. / 60.);
            clock.advance(at, || {});
            app.frame();
            let report = app.ui.scene.borrow_mut().flush();
            if report.damage.is_empty() {
                continue;
            }
            gpu.render(&app.ui.scene.borrow(), &report.damage).expect("a frame drawn");
            let pixels = gpu.readback().expect("the frame's pixels");
            let mut h = std::collections::hash_map::DefaultHasher::new();
            pixels.hash(&mut h);
            // (And all but the rail's strip, the window's last 12 points:
            // what the transcript shows, apart from where its thumb is.)
            let row = (W * SCALE) as usize * 4;
            let keep_w = ((W - 12.) * SCALE) as usize * 4;
            let mut c = std::collections::hash_map::DefaultHasher::new();
            for line in pixels.chunks(row) {
                line[..keep_w.min(line.len())].hash(&mut c);
            }
            lines.push(format!("{name} frame {f} {:016x} {:016x}", h.finish(), c.finish()));
            if keep.iter().any(|k| *k == format!("{name}:{f}")) {
                std::fs::write(out.join(format!("{name}-f{f:03}.rgba")), &pixels).unwrap();
            }
        }
        assert_eq!(app.error(), None);
    };
    shot(&mut app, "opened", 120, 0.);
    shot(&mut app, "steady", 240, -8.);
    shot(&mut app, "rest", 120, 0.);
    shot(&mut app, "fling", 60, -160.);
    shot(&mut app, "fling-rest", 120, 0.);
    shot(&mut app, "top", 1, -1_000_000.);
    shot(&mut app, "top-rest", 90, 0.);
    shot(&mut app, "down", 120, 24.);
    shot(&mut app, "end", 1, 1_000_000.);
    shot(&mut app, "end-rest", 90, 0.);
    shot(&mut app, "middle", 1, -12_000.);
    shot(&mut app, "middle-rest", 90, 0.);
    shot(&mut app, "fling-down", 40, 200.);
    shot(&mut app, "last-rest", 120, 0.);
    std::fs::write(out.join("frames.txt"), lines.join("\n") + "\n").unwrap();
    println!("scroll_pixels: {} frames drawn, in {}", lines.len(), out.display());
}
