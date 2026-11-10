//! What a frame where only a streaming reply's fade moves costs to draw: the
//! chat fixture's transcript (ui/chat_fixture.luau) with a 14 KB reply, then
//! a chunk at a time, each followed by 30 frames where only its words fade
//! in, drawn offscreen by zgx's renderer (only each frame's damage). Its
//! phases are counted (`ZGX_PROFILE`) over the fade-only frames alone; run
//! under `sample`, nearly all its main thread is those frames. A probe:
//!
//!     ZGX_PROFILE=1 cargo test --release --test fade_only_cost -- --ignored --nocapture

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use zgx_script::{ScriptHost, Value, testing::TestApp};

fn ui_dir() -> PathBuf {
    std::env::var_os("ZERON_ZGX_TEST_UI")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui"))
}

/// tests/streaming_cost.rs's reply.
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

#[test]
#[ignore = "a probe: ZGX_PROFILE=1, release, --ignored --nocapture"]
fn frames_where_only_a_fade_moves() {
    let text = reply(80);
    let app = TestApp::new(|runtime| ScriptHost::new(runtime, ui_dir().join("chat_fixture.luau")));
    app.ui.scene.borrow_mut().resize(900., 700.);
    let mut gpu = zgx_gpu::GpuRenderer::new(900, 700).expect("a GPU");
    let mut draw = |app: &TestApp| -> (f32, usize) {
        let damage = app.ui.scene.borrow_mut().flush().damage;
        gpu.render(&app.ui.scene.borrow(), &damage).expect("drawn");
        (damage.iter().map(|r| r.width * r.height).sum(), damage.len())
    };
    app.frames(2);
    assert!(app.host.run_command("go_s2"));
    app.frames(30);
    std::thread::sleep(Duration::from_millis(320));
    assert!(app.host.run_command("start_block_reply"));
    app.frames(5);
    draw(&app);
    let draft = app.host.shared("chat.draft", Value::Nil);
    let write = |end: usize| {
        let words = Value::from(&text[..end]);
        draft.set(Value::record([
            ("id", Value::from("live")),
            ("text", words.clone()),
            ("part", Value::from("t")),
            ("part_text", words),
        ]));
    };
    let frame = Duration::from_micros(8_333);
    // To 14 KB at a window's pace, so what's settled is as a long reply's.
    let mut end = 0;
    let mut n = 0;
    while end < 14_000 {
        let started = Instant::now();
        if n % 3 == 0 {
            end = (end + 24).min(text.len());
            while !text.is_char_boundary(end) {
                end += 1;
            }
            write(end);
        }
        n += 1;
        app.frames(1);
        draw(&app);
        std::thread::sleep(frame.saturating_sub(started.elapsed()));
    }
    // Then a chunk, and 30 frames where only its fade moves; over again.
    let (mut phases, mut frames, mut area, mut rects) = (BTreeMap::<String, (u64, f64)>::new(), 0u32, 0f64, 0usize);
    let mut walls = Vec::new();
    for _ in 0..150 {
        end = (end + 24).min(text.len());
        while !text.is_char_boundary(end) {
            end += 1;
        }
        write(end);
        app.frames(1);
        draw(&app);
        for _ in 0..30 {
            let started = Instant::now();
            zgx::profile::take();
            app.frames(1);
            let (a, r) = draw(&app);
            walls.push(started.elapsed());
            if a > 0. {
                frames += 1;
                area += a as f64;
                rects += r;
                for (phase, instructions, time) in zgx::profile::take() {
                    let e = phases.entry(format!("{phase:?}")).or_default();
                    e.0 += instructions;
                    e.1 += time.as_secs_f64() * 1e6;
                }
            }
            std::thread::sleep(frame.saturating_sub(started.elapsed()));
        }
    }
    assert_eq!(app.error(), None);
    walls.sort();
    eprintln!(
        "fade-only frames that drew: {frames} of {}; damage mean {:.0} px² in {:.1} rects; wall median {:.1} µs",
        walls.len(),
        area / frames.max(1) as f64,
        rects as f64 / frames.max(1) as f64,
        walls[walls.len() / 2].as_secs_f64() * 1e6
    );
    for (phase, (instructions, micros)) in &phases {
        eprintln!(
            "  {phase:>8}: mean {:>8.1}K instr {:>7.1} µs",
            *instructions as f64 / frames.max(1) as f64 / 1e3,
            micros / frames.max(1) as f64
        );
    }
}
