//! What a turn's thinking and tool calls (its activity group, ds/activity)
//! cost, by how many calls and how much thinking it holds. The chat page's
//! fixture (ui/chat_fixture.luau) mounted headless and drawn offscreen by
//! zgx's renderer at a Retina scale, its transcript given what the host
//! gives it: each update of the turn built by src/chat.rs (`built_entry`,
//! `draft_of`) from an engine-shaped entry, the other rows kept as built.
//!
//! For each group (N calls, L bytes of thinking, folded or open), the
//! cost of a frame, split by zgx's phases (`ZGX_PROFILE`'s instructions),
//! and the host's build of each update:
//!
//! - `rest`: a settled turn, nothing new (what a frame costs that nothing
//!   asked for, and whether one is asked for at all);
//! - `ticks`: a turn at work, nothing new (its loaders turning);
//! - `call`: a new call arriving (the entry upserted, as the engine does);
//! - `output`: the running call's output growing (upserted);
//! - `think`: the thought being written growing in the draft;
//! - `scroll`: a settled turn scrolled past, a wheel's step a frame.
//!
//!   cargo test --release --bin zeron-zgx activity_cost -- --ignored --nocapture
//!   ACTIVITY_PIXELS=<dir> cargo test --release --bin zeron-zgx activity_pixels -- --ignored --nocapture
//!
//! The second hashes every frame drawn through a fixed script of the same
//! states into <dir>/frames.txt (raw RGBA kept beside it), to compare two
//! builds frame for frame.

use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use serde_json::{Value as Json, json};
use zgx::profile::{self, PHASES, Phase};
use zgx_gpu::GpuRenderer;
use zgx_script::{ScriptHost, Shared, Value, testing::TestApp};

const W: f32 = 1240.;
const H: f32 = 820.;
const SCALE: f32 = 2.;

fn ui() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui")
}

/// This thread's CPU time so far.
fn thread_cpu() -> Duration {
    let mut t = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: fills a timespec for this thread's CPU clock.
    unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut t) };
    Duration::new(t.tv_sec as u64, t.tv_nsec as u32)
}

/// Prose of about `len` bytes, in paragraphs of a few sentences.
fn prose(seed: &str, len: usize) -> String {
    let mut out = String::new();
    let mut i = 0;
    while out.len() < len {
        i += 1;
        out.push_str(&format!(
            "Step {i} of {seed}: the parser reads the frame, then the host applies it; **the rows** that changed are built again, \
             and `chat.rs` hands the transcript its list. Whether part {i} holds depends on what the group reads per update.\n\n"
        ));
    }
    out.truncate(len);
    out
}

/// The `i`th call: a command (with output), a read, a search or an edit.
fn call(i: usize, output_lines: usize) -> Json {
    let id = format!("t{i}");
    match i % 4 {
        0 => {
            let output: String = (0..output_lines).map(|l| format!("line {l} of command {i}: ok\n")).collect();
            json!({ "id": id, "kind": "tool", "call": { "kind": "exec", "command": format!("cargo test --package part{i} -- --nocapture") }, "output": output, "exitCode": 0 })
        }
        1 => json!({ "id": id, "kind": "tool", "call": { "kind": "readFile", "path": format!("/repo/src/part{i}.rs") }, "output": format!("fn part{i}() {{}}\n") }),
        2 => json!({ "id": id, "kind": "tool", "call": { "kind": "search", "pattern": format!("fn part{i}"), "path": "/repo/src" } }),
        _ => json!({ "id": id, "kind": "tool", "call": { "kind": "editFile", "path": format!("/repo/src/part{i}.rs") }, "diffStats": [{ "path": format!("/repo/src/part{i}.rs"), "additions": 3, "deletions": 1 }] }),
    }
}

/// The turn: a thought of `think` bytes, `n` calls, and, while it streams,
/// a thought being written (`tail`) after them.
fn turn(n: usize, think: usize, tail: Option<&str>, streaming: bool) -> Json {
    let mut parts = vec![json!({ "id": "r0", "kind": "reasoning", "text": prose("the plan", think) })];
    parts.extend((1..=n).map(|i| call(i, 4)));
    if let Some(tail) = tail {
        parts.push(json!({ "id": "rt", "kind": "reasoning", "text": tail }));
    }
    let mut entry = json!({ "id": "a1", "role": "assistant", "parts": parts, "createdAt": 1_762_000_000_000_i64 });
    if streaming {
        entry["status"] = json!("streaming");
    } else {
        entry["status"] = json!("complete");
        entry["durationMs"] = json!(42_000);
        entry["parts"].as_array_mut().unwrap().push(json!({ "id": "x1", "kind": "text", "text": prose("the answer", 1200) }));
    }
    entry
}

fn prompt(id: &str, words: &str) -> Json {
    json!({ "id": id, "role": "user", "parts": [{ "id": format!("{id}p"), "kind": "text", "text": words }], "createdAt": 1_762_000_000_000_i64 })
}

fn reply(id: &str, len: usize) -> Json {
    json!({ "id": id, "role": "assistant", "status": "complete", "parts": [{ "id": format!("{id}x"), "kind": "text", "text": prose(id, len) }], "createdAt": 1_762_000_000_000_i64 })
}

fn built(entry: &Json) -> Value {
    crate::chat::built_entry(entry, "doc", None, Some("Opus")).expect("a row")
}

/// One frame's (or several frames') cost: each phase's instructions and
/// time, the thread's CPU, the host's build, the calls into scripts.
#[derive(Default, Clone, Copy)]
struct Cost {
    phases: [(u64, Duration); 12],
    cpu: Duration,
    host: Duration,
    calls: u64,
    frames: usize,
    drawn: usize,
    wanted: usize,
}

impl Cost {
    fn add(&mut self, other: &Cost) {
        for (a, b) in self.phases.iter_mut().zip(other.phases) {
            a.0 += b.0;
            a.1 += b.1;
        }
        self.cpu += other.cpu;
        self.host += other.host;
        self.calls += other.calls;
        self.frames += other.frames;
        self.drawn += other.drawn;
        self.wanted += other.wanted;
    }
    fn phase(&self, phase: Phase) -> u64 {
        let at = PHASES.iter().position(|p| *p == phase).unwrap();
        self.phases[at].0
    }
    fn instructions(&self) -> u64 {
        self.phases.iter().map(|p| p.0).sum()
    }
}

struct Bench {
    app: TestApp,
    /// Drawn offscreen (a benchmark's); none for a check of what's built.
    gpu: Option<GpuRenderer>,
    messages: Shared,
    draft: Shared,
    /// The rows before the turn, as built.
    before: Vec<Value>,
    after: Vec<Value>,
    /// The time `ui.now()` reads, when it is this test's (`Bench::on_test_clock`):
    /// on 1/60 s a frame, so what fades and turns by the clock draws the same
    /// frames each run.
    clock: Option<(zgx::clock::VirtualClock, Duration)>,
}

impl Bench {
    fn new() -> Self {
        Self::made(None, true)
    }

    /// Laid out, not drawn: what's built, without a GPU.
    fn headless() -> Self {
        Self::made(None, false)
    }

    /// With the interface's time this test's own (`clock`).
    fn on_test_clock() -> Self {
        Self::made(Some((zgx::clock::VirtualClock::new(), Duration::ZERO)), true)
    }

    /// `ms` passing, on the wall clock or the test's.
    fn pass(clock: &mut Option<(zgx::clock::VirtualClock, Duration)>, ms: u64) {
        match clock {
            Some((clock, at)) => {
                *at += Duration::from_millis(ms);
                clock.advance(*at, || {});
            }
            None => std::thread::sleep(Duration::from_millis(ms)),
        }
    }

    /// A frame's time passing on the test's clock (if it has one).
    fn tick(&mut self) {
        if let Some((clock, at)) = &mut self.clock {
            *at += Duration::from_secs_f64(1. / 60.);
            clock.advance(*at, || {});
        }
    }

    fn made(mut clock: Option<(zgx::clock::VirtualClock, Duration)>, draw: bool) -> Self {
        let fixture = ui().join("chat_fixture.luau");
        let app = TestApp::new(move |runtime| ScriptHost::new(runtime, fixture));
        let gpu = draw.then(|| {
            let mut gpu = GpuRenderer::new((W * SCALE) as u32, (H * SCALE) as u32).expect("a GPU");
            gpu.set_scale_factor(SCALE);
            for font in crate::fonts() {
                font.expect("the app's fonts").install(&mut gpu.text_system().borrow_mut());
            }
            gpu.install_text(&mut app.ui.scene.borrow_mut());
            app.ui.refresh_text_geometry();
            gpu
        });
        app.ui.scene.borrow_mut().resize(W, H);
        app.frames(2);
        assert!(app.host.run_command("go_s2"), "the fixture opens s2");
        app.frames(30);
        // (Past the moment a chat opens, when what's there stays as it is.)
        Self::pass(&mut clock, 320);
        let messages = app.host.shared("chat.messages", Value::Nil);
        let draft = app.host.shared("chat.draft", Value::Nil);
        let before = vec![built(&prompt("u0", "Where were we?")), built(&reply("a0", 3000)), built(&prompt("u1", "Make the group cheap."))];
        let mut bench = Self { app, gpu, messages, draft, before, after: Vec::new(), clock };
        bench.settle(10);
        bench
    }

    /// The transcript with the turn as `entry` (the host's publish).
    fn publish(&mut self, entry: &Json) -> Duration {
        let start = thread_cpu();
        let row = built(entry);
        let draft = crate::chat::draft_of(entry);
        let host = thread_cpu() - start;
        let mut list = self.before.clone();
        list.push(row);
        list.extend(self.after.iter().cloned());
        self.messages.set(Value::List(Rc::from(list)));
        self.draft.set(draft);
        host
    }

    /// The draft alone (a thought growing: the host's `Change::Words`).
    fn publish_draft(&mut self, entry: &Json) -> Duration {
        let start = thread_cpu();
        let draft = crate::chat::draft_of(entry);
        let host = thread_cpu() - start;
        self.draft.set(draft);
        host
    }

    /// One frame as a window's: laid out, its damage drawn.
    fn frame(&mut self) -> Cost {
        let _ = profile::take();
        let calls = self.app.calls();
        let wanted = self.app.clock.wants_frame();
        self.tick();
        let start = thread_cpu();
        self.app.frame();
        let report = self.app.ui.scene.borrow_mut().flush();
        let drawn = !report.damage.is_empty();
        if drawn && let Some(gpu) = &mut self.gpu {
            gpu.render(&self.app.ui.scene.borrow(), &report.damage).expect("a frame drawn");
        }
        let cpu = thread_cpu() - start;
        let mut cost = Cost { cpu, calls: self.app.calls() - calls, frames: 1, drawn: drawn as usize, wanted: wanted as usize, ..Cost::default() };
        for (phase, instructions, time) in profile::take() {
            let at = PHASES.iter().position(|p| *p == phase).unwrap();
            cost.phases[at] = (instructions, time);
        }
        if let Some(error) = self.app.error() {
            panic!("{error}");
        }
        cost
    }

    fn settle(&mut self, frames: usize) {
        for _ in 0..frames {
            self.frame();
        }
    }

    fn frames(&mut self, frames: usize) -> Cost {
        let mut total = Cost::default();
        for _ in 0..frames {
            total.add(&self.frame());
        }
        total
    }

    fn nodes(&self) -> usize {
        self.app.ui.scene.borrow().len()
    }

    /// The turn's prompt brought to the view's top: the group in view.
    fn to_turn(&mut self) {
        self.app.scroll("transcript", -100_000.);
        self.settle(30);
        let top = self.app.bounds("transcript").y;
        let at = self.app.bounds("~\"Make the group cheap.\"").y;
        self.app.scroll("transcript", at - top - 40.);
        self.settle(30);
    }

    /// The group's header pressed (folds an open one, opens a folded one).
    fn toggle(&mut self) {
        // (Scrolled to it first, as a reader would: a long group's header
        // is above the view.)
        let top = self.app.bounds("transcript").y;
        let at = self.app.bounds("role:button ~\"Thought \"").y;
        if at < top + 20. {
            self.app.scroll("transcript", at - top - 100.);
            self.settle(30);
        }
        self.app.click("role:button ~\"Thought \"");
        self.settle(40);
    }
}

/// Whether `what` is measured (`ACTIVITY_ONLY`, a comma list; all without).
fn wanted(what: &str) -> bool {
    std::env::var("ACTIVITY_ONLY").map_or(true, |only| only.split(',').any(|w| w == what))
}

fn row(n: usize, think: usize, open: &str, what: &str, cost: &Cost, per: usize, nodes: usize) {
    if !wanted(what) {
        return;
    }
    let per = per.max(1) as f64;
    let k = |phase: Phase| cost.phase(phase) as f64 / per / 1e3;
    println!(
        "{n:>5} {:>4}K {open:<6} {what:<7} | {:>9.0} {:>8.0} {:>8.0} {:>8.0} {:>7.0} {:>7.0} {:>7.0} | {:>8.0} | {:>7.1} {:>7.1} | {:>6.0} | {:>4.2} {:>4.2} | {nodes:>6}",
        think / 1024,
        cost.instructions() as f64 / per / 1e3,
        k(Phase::Script),
        k(Phase::Update),
        k(Phase::Layout),
        k(Phase::Damage),
        k(Phase::Paint),
        k(Phase::Encode),
        cost.cpu.as_secs_f64() * 1e6 / per,
        cost.host.as_secs_f64() * 1e6 / per,
        // (Phases' own time, for a check against the CPU.)
        cost.phases.iter().map(|p| p.1).sum::<Duration>().as_secs_f64() * 1e6 / per,
        cost.calls as f64 / per,
        cost.drawn as f64 / cost.frames.max(1) as f64,
        cost.wanted as f64 / cost.frames.max(1) as f64,
    );
}

fn header() {
    println!(
        "{:>5} {:>5} {:<6} {:<7} | {:>9} {:>8} {:>8} {:>8} {:>7} {:>7} {:>7} | {:>8} | {:>7} {:>7} | {:>6} | {:>4} {:>4} | {:>6}",
        "calls", "think", "group", "what", "kinstr", "script", "update", "layout", "damage", "paint", "encode", "cpu µs", "host µs", "phs µs", "calls", "drew", "want", "nodes"
    );
}

/// Every scenario for one group: `n` calls, `think` bytes of thinking,
/// open or folded.
fn scenarios(n: usize, think: usize, open: bool) {
    let label = if open { "open" } else { "folded" };
    let started = Instant::now();
    // (Fewer samples where each is slow.)
    let samples = if n >= 1000 { 4 } else { 8 };

    // A turn at work: open while it works; folded by a press.
    let mut bench = Bench::new();
    let tail = prose("the thought being written", think);
    let mut calls = n;
    let mut entry = turn(calls, 1024, Some(&tail), true);
    bench.publish(&entry);
    bench.settle(60);
    if !open {
        bench.toggle();
        // (Back at the end, where a turn at work is followed.)
        bench.app.scroll("transcript", 100_000.);
    }
    bench.settle(30);
    let ticks = bench.frames(60);
    row(n, think, label, "ticks", &ticks, 60, bench.nodes());

    // The thought being written grows, a chunk a frame.
    // (What was there when the thought showed has faded in: a veil's fade
    // is at most 400 ms.)
    std::thread::sleep(Duration::from_millis(500));
    bench.settle(2);
    let mut think_cost = Cost::default();
    let mut text = tail.clone();
    for c in 0..samples * 4 {
        // (Paced as words arrive: the veil fades them in by the wall clock.)
        std::thread::sleep(Duration::from_millis(16));
        text.push_str(&format!("more words, chunk {c}; "));
        entry["parts"].as_array_mut().unwrap().last_mut().unwrap()["text"] = json!(text);
        let host = bench.publish_draft(&entry);
        let mut cost = bench.frame();
        cost.host = host;
        think_cost.add(&cost);
    }
    row(n, think, label, "think", &think_cost, samples * 4, bench.nodes());

    // New calls arrive (the thought done, a call after it), each a frame.
    let mut call_cost = Cost::default();
    for _ in 0..samples {
        calls += 1;
        entry = turn(calls, 1024, Some(&text), true);
        // (The tail thought done: a call is now last.)
        let last = entry["parts"].as_array_mut().unwrap().pop().unwrap();
        entry["parts"].as_array_mut().unwrap().insert(1, last);
        let host = bench.publish(&entry);
        let mut cost = bench.frame();
        cost.host = host;
        call_cost.add(&cost);
        bench.settle(24);
    }
    row(n, think, label, "call", &call_cost, samples, bench.nodes());

    // The running call's output grows.
    let mut output_cost = Cost::default();
    for k in 0..samples {
        let parts = entry["parts"].as_array_mut().unwrap();
        let last = parts.last_mut().unwrap();
        let had = last["output"].as_str().unwrap_or_default().to_owned();
        last["output"] = json!(format!("{had}more output {k}\n"));
        let host = bench.publish(&entry);
        let mut cost = bench.frame();
        cost.host = host;
        output_cost.add(&cost);
        bench.settle(4);
    }
    row(n, think, label, "output", &output_cost, samples, bench.nodes());
    drop(bench);

    // Settled: at rest, then scrolled past.
    let mut bench = Bench::new();
    bench.after = vec![built(&prompt("u2", "And the rest?")), built(&reply("a2", 6000))];
    let entry = turn(n, think, None, false);
    bench.publish(&entry);
    bench.settle(60);
    bench.to_turn();
    if open {
        // (Pressed open: the press and the frames of its opening.)
        let _ = profile::take();
        let (start, calls) = (thread_cpu(), bench.app.calls());
        bench.app.click("role:button ~\"Thought \"");
        let mut opening = Cost { cpu: thread_cpu() - start, calls: bench.app.calls() - calls, ..Cost::default() };
        for (phase, instructions, time) in profile::take() {
            let at = PHASES.iter().position(|p| *p == phase).unwrap();
            opening.phases[at] = (instructions, time);
        }
        opening.add(&bench.frames(30));
        row(n, think, label, "opening", &opening, 1, bench.nodes());
    }
    bench.settle(60);
    let rest = bench.frames(60);
    row(n, think, label, "rest", &rest, 60, bench.nodes());
    // From there on down past the turn, a wheel's step a frame.
    let mut scroll = Cost::default();
    for _ in 0..60 {
        bench.app.scroll("transcript", 120.);
        scroll.add(&bench.frame());
    }
    row(n, think, label, "scroll", &scroll, 60, bench.nodes());
    eprintln!("  ({n} calls, {}K, {label}: {:.1}s)", think / 1024, started.elapsed().as_secs_f64());
}

fn profiled() {
    // SAFETY: set before any thread reads the environment (the profile
    // reads it once, as it is first asked).
    unsafe { std::env::set_var("ZGX_PROFILE", "1") };
    assert!(profile::enabled());
}

#[test]
#[ignore = "a benchmark: run it --release, as the doc above says"]
fn activity_cost() {
    profiled();
    let only: Option<Vec<usize>> = std::env::var("ACTIVITY_CALLS").ok().map(|v| v.split(',').filter_map(|n| n.parse().ok()).collect());
    let thinks: Vec<usize> = std::env::var("ACTIVITY_THINK")
        .ok()
        .map(|v| v.split(',').filter_map(|n| n.parse::<usize>().ok()).map(|k| k * 1024).collect())
        .unwrap_or_else(|| vec![10 * 1024]);
    println!("per frame (kinstr: thousands of instructions; host: the row's build; calls: into scripts; drew/want: frames drawn / asked for)");
    header();
    for &think in &thinks {
        for &n in only.as_deref().unwrap_or(&[10, 100, 500, 2000]) {
            for open in [false, true] {
                scenarios(n, think, open);
            }
        }
    }
}

#[test]
#[ignore = "a comparison: run it --release, as the doc above says"]
fn activity_pixels() {
    let out = PathBuf::from(std::env::var("ACTIVITY_PIXELS").expect("ACTIVITY_PIXELS=<dir>"));
    std::fs::create_dir_all(&out).unwrap();
    // (Each frame's pixels too, raw, with ACTIVITY_PIXELS_KEEP: 15 MB each.)
    let keep = std::env::var_os("ACTIVITY_PIXELS_KEEP").is_some();
    let mut lines = Vec::new();
    let mut shot = |bench: &mut Bench, name: &str, frames: usize| {
        for f in 0..frames {
            bench.tick();
            bench.app.frame();
            let report = bench.app.ui.scene.borrow_mut().flush();
            if report.damage.is_empty() {
                continue;
            }
            let gpu = bench.gpu.as_mut().expect("drawn");
            gpu.render(&bench.app.ui.scene.borrow(), &report.damage).expect("a frame drawn");
            let pixels = gpu.readback().expect("the frame's pixels");
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            pixels.hash(&mut h);
            lines.push(format!("{name} frame {f} {:016x}", h.finish()));
            if keep {
                std::fs::write(out.join(format!("{name}-f{f:03}.rgba")), &pixels).unwrap();
            }
        }
        assert_eq!(bench.app.error(), None);
    };
    // At work: open, calls arriving, a thought being written, then folded.
    let mut bench = Bench::on_test_clock();
    let tail = prose("the thought being written", 2000);
    let mut entry = turn(12, 3000, Some(&tail), true);
    bench.publish(&entry);
    shot(&mut bench, "live-open", 90);
    for c in 0..6 {
        let mut text = tail.clone();
        text.push_str(&"more words arriving now; ".repeat(c + 1));
        entry["parts"].as_array_mut().unwrap().last_mut().unwrap()["text"] = json!(text);
        bench.publish_draft(&entry);
        shot(&mut bench, &format!("think-{c}"), 6);
    }
    for k in 13..16 {
        entry = turn(k, 3000, None, true);
        bench.publish(&entry);
        shot(&mut bench, &format!("call-{k}"), 30);
    }
    let parts = entry["parts"].as_array_mut().unwrap();
    parts.last_mut().unwrap()["output"] = json!("more output\n");
    bench.publish(&entry);
    shot(&mut bench, "output", 20);
    bench.app.click("role:button ~\"Thought \"");
    shot(&mut bench, "live-folded", 40);
    drop(bench);
    // Settled: folded, opened, a call opened, the thought shown whole, and
    // scrolled.
    let mut bench = Bench::on_test_clock();
    bench.after = vec![built(&prompt("u2", "And the rest?")), built(&reply("a2", 2000))];
    bench.publish(&turn(40, 3000, None, false));
    shot(&mut bench, "settled", 60);
    bench.to_turn();
    shot(&mut bench, "at-turn", 10);
    bench.app.click("role:button ~\"Thought \"");
    shot(&mut bench, "opened", 40);
    bench.app.click("role:button ~\"package part4 --\"");
    shot(&mut bench, "call-open", 40);
    bench.app.click("role:button ~\"Show all\"");
    shot(&mut bench, "thought-whole", 40);
    for s in 0..8 {
        bench.app.scroll("transcript", -300.);
        shot(&mut bench, &format!("scroll-{s}"), 3);
    }
    std::fs::write(out.join("frames.txt"), lines.join("\n") + "\n").unwrap();
    println!("activity_pixels: {} frames drawn, in {}", lines.len(), out.display());
}

/// A group open as its turn works holds what shows of it, not a row for
/// every call: a hundred calls or two thousand, about the same nodes, and a
/// call arriving builds its own row alone. (The benchmark above measures
/// what that costs; this keeps it so, and runs with every test.)
#[test]
fn a_long_group_builds_what_shows_of_it() {
    let sample = |n: usize| -> (usize, usize) {
        let mut bench = Bench::headless();
        let mut entry = turn(n, 1024, None, true);
        bench.publish(&entry);
        bench.settle(60);
        let before = bench.nodes();
        entry["parts"].as_array_mut().unwrap().push(call(n + 1, 4));
        bench.publish(&entry);
        bench.settle(30);
        (before, bench.nodes() - before)
    };
    let (few, few_new) = sample(100);
    let (many, many_new) = sample(2000);
    // (Before the group was windowed: 2465 nodes for 100 calls, 40465 for
    // 2000.)
    assert!(many <= few + 400, "an open group of 2000 calls holds {many} nodes, of 100 {few}");
    assert!(many_new <= few_new + 20, "a call arriving built {many_new} nodes in a group of 2000, {few_new} in one of 100");
}

/// A reply being written, built again as a call arrives, keeps each call it
/// had as the very value it was: the page has nothing to do for them, and the
/// host builds only what changed.
#[test]
fn a_reply_built_again_keeps_its_calls_as_they_were() {
    let ptr = |v: &Value| match v {
        Value::Map(m) => Rc::as_ptr(m) as *const u8,
        _ => std::ptr::null(),
    };
    let tools = |row: &Value| row.field("tools").expect("its tools").list().to_vec();
    let a = turn(50, 1024, None, true);
    let mut b = a.clone();
    b["parts"].as_array_mut().unwrap().push(call(51, 4));
    // (The last call's output growing, too: that one alone is built again.)
    let mut c = b.clone();
    c["parts"].as_array_mut().unwrap().last_mut().unwrap()["output"] = json!("more output\n");
    let (ta, tb, tc) = (tools(&built(&a)), tools(&built(&b)), tools(&built(&c)));
    assert_eq!(tb.len(), ta.len() + 1);
    for (i, (x, y)) in ta.iter().zip(&tb).enumerate() {
        assert_eq!(ptr(x), ptr(y), "call {i} was built again");
    }
    for (i, (x, y)) in tb.iter().zip(&tc).take(tb.len() - 1).enumerate() {
        assert_eq!(ptr(x), ptr(y), "call {i} was built again");
    }
    assert_ne!(ptr(tb.last().unwrap()), ptr(tc.last().unwrap()), "the changed call is built again");
    assert_eq!(tc.last().unwrap().field("output"), Some(&Value::from("more output\n")));
}
