//! What a streaming reply's frames cost to draw: the chat fixture's
//! transcript (`ui/chat_fixture.luau`, a reply in text blocks, fading in
//! as it arrives) drawn offscreen by zgx's renderer after each frame, only
//! that frame's damage, as a window draws it. Its phases are counted
//! (`ZGX_PROFILE`): frames a chunk lands on apart from frames where only a
//! fade moves, at a window's pace (about 120 frames and 40 chunks a second;
//! fades run on the wall clock). A benchmark, not a check:
//!
//!     ZGX_PROFILE=1 cargo test --release --test fade_frame_cost -- --ignored --nocapture

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

#[derive(Default)]
struct Kind {
    frames: usize,
    wall: Vec<Duration>,
    phases: BTreeMap<String, Vec<(u64, Duration)>>,
    damaged: Vec<f32>,
}
impl Kind {
    fn add(&mut self, wall: Duration, phases: Vec<(zgx::profile::Phase, u64, Duration)>, damaged: f32) {
        self.frames += 1;
        self.wall.push(wall);
        self.damaged.push(damaged);
        for (phase, instructions, time) in phases {
            if instructions > 0 || !time.is_zero() {
                self.phases.entry(format!("{phase:?}")).or_default().push((instructions, time));
            }
        }
    }
    fn report(&self, name: &str) {
        let median = |mut v: Vec<f64>| -> f64 {
            if v.is_empty() {
                return 0.;
            }
            v.sort_by(|a, b| a.partial_cmp(b).unwrap());
            v[v.len() / 2]
        };
        eprintln!(
            "{name}: {} frames, wall median {:.1} µs, damage median {:.0} px²",
            self.frames,
            median(self.wall.iter().map(|d| d.as_secs_f64() * 1e6).collect()),
            median(self.damaged.iter().map(|&a| a as f64).collect()),
        );
        for (phase, values) in &self.phases {
            // (A phase a frame didn't run counts 0 for it.)
            let mut instr: Vec<f64> = values.iter().map(|v| v.0 as f64).collect();
            let mut time: Vec<f64> = values.iter().map(|v| v.1.as_secs_f64() * 1e6).collect();
            instr.resize(self.frames, 0.);
            time.resize(self.frames, 0.);
            eprintln!(
                "  {phase:>8}: median {:>8.1}K instr {:>7.1} µs   mean {:>7.1} µs",
                median(instr) / 1e3,
                median(time.clone()),
                time.iter().sum::<f64>() / self.frames as f64,
            );
        }
    }
}

#[test]
#[ignore = "a benchmark: ZGX_PROFILE=1, release, --ignored --nocapture"]
fn a_streaming_replys_frames_drawn() {
    assert!(zgx::profile::enabled(), "run with ZGX_PROFILE=1");
    let text = reply(80);
    let app = TestApp::new(|runtime| ScriptHost::new(runtime, ui_dir().join("chat_fixture.luau")));
    app.ui.scene.borrow_mut().resize(900., 700.);
    let mut gpu = zgx_gpu::GpuRenderer::new(900, 700).expect("a GPU");
    let mut draw = |app: &TestApp| -> f32 {
        let damage = app.ui.scene.borrow_mut().flush().damage;
        gpu.render(&app.ui.scene.borrow(), &damage).expect("drawn");
        damage.iter().map(|r| r.width * r.height).sum()
    };
    app.frames(2);
    assert!(app.host.run_command("go_s2"));
    app.frames(30);
    std::thread::sleep(Duration::from_millis(320));
    assert!(app.host.run_command("start_block_reply"));
    app.frames(5);
    draw(&app);
    let draft = app.host.shared("chat.draft", Value::Nil);
    let (mut chunks, mut fades) = (Kind::default(), Kind::default());
    let frame = Duration::from_micros(8_333);
    let mut end = 0;
    let mut n = 0;
    while end < 24_000 {
        // A chunk every third frame: 40 a second at 120 frames.
        let lands = n % 3 == 0;
        n += 1;
        if lands {
            end = (end + 24).min(text.len());
            while !text.is_char_boundary(end) {
                end += 1;
            }
            let words = Value::from(&text[..end]);
            draft.set(Value::record([
                ("id", Value::from("live")),
                ("text", words.clone()),
                ("part", Value::from("t")),
                ("part_text", words),
            ]));
        }
        let started = Instant::now();
        zgx::profile::take();
        app.frames(1);
        let damaged = draw(&app);
        let wall = started.elapsed();
        let phases = zgx::profile::take();
        // (Counted once the reply is long: its first few KB settle.)
        if end > 8_000 {
            if lands {
                chunks.add(wall, phases, damaged);
            } else {
                fades.add(wall, phases, damaged);
            }
        }
        std::thread::sleep(frame.saturating_sub(started.elapsed()));
    }
    assert_eq!(app.error(), None);
    chunks.report("frames a chunk lands on");
    fades.report("frames where only fades move");
}
