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

/// One call row's scene (kinds and boxes, no text) and every node above it to
/// the root, for zgx's layout work to reproduce alone (`DEBUG_TREE=1`).
fn dump_row(bench: &mut Bench) {
    let Some(button) = bench.app.find("role:button ~\"package part8 --\"") else {
        eprintln!("TREE no row in view");
        return;
    };
    let scene = bench.app.ui.scene.borrow();
    let kind = |id| {
        let k = format!("{:?}", scene.kind(id));
        k.split(|c: char| c == '(' || c == '{' || c == ' ').next().unwrap_or("").to_owned()
    };
    let line = |id, depth: usize| {
        let b = scene.bounds(id);
        format!("{:indent$}#{} {} {:.0},{:.0} {:.0}x{:.0} ({} children)", "", id.number(), kind(id), b.x, b.y, b.width, b.height, scene.children(id).len(), indent = depth * 2)
    };
    // The row's own root: the button's parent.
    let row = scene.parent(button).unwrap_or(button);
    let mut up = Vec::new();
    let mut at = scene.parent(row);
    while let Some(id) = at {
        up.push(id);
        at = scene.parent(id);
    }
    eprintln!("TREE above the row, from the root:");
    for (depth, id) in up.iter().rev().enumerate() {
        eprintln!("TREE {}", line(*id, depth));
    }
    eprintln!("TREE the row:");
    fn walk(scene: &zgx::scene::Scene, id: zgx::scene::NodeId, depth: usize, line: &dyn Fn(zgx::scene::NodeId, usize) -> String) {
        eprintln!("TREE {}", line(id, depth));
        for child in scene.children(id) {
            walk(scene, *child, depth + 1, line);
        }
    }
    walk(&scene, row, 0, &line);
}

thread_local! {
    /// Whether `DEBUG_NODE` has shown its node.
    static NODE_SHOWN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// Whether frames show their damage now (`DEBUG_DAMAGE`: the scroll's).
    static DAMAGE_SHOWN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Each damage rect (logical points) and the nodes it is: those whose box
/// matches it, else the deepest one holding it, with what's above.
fn show_damage(scene: &zgx::scene::Scene, damage: &[zgx::scene::Rect]) {
    let kind = |id| {
        let k = format!("{:?}", scene.kind(id));
        k.chars().take(40).collect::<String>()
    };
    let line = |id: zgx::scene::NodeId| {
        let b = scene.bounds(id);
        format!("#{} {} {:.1},{:.1} {:.1}x{:.1}", id.number(), kind(id), b.x, b.y, b.width, b.height)
    };
    let mut all = Vec::new();
    let mut stack = vec![(scene.root(), 0usize)];
    while let Some((id, depth)) = stack.pop() {
        all.push((id, depth));
        stack.extend(scene.children(id).iter().map(|c| (*c, depth + 1)));
    }
    thread_local!(static ONCE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) });
    if !ONCE.replace(true) {
        // Once: every box that fades its edges or blurs what's behind it.
        for (id, depth) in &all {
            let s = format!("{:?}", scene.style(*id));
            let fades = !s.contains("fade_edges: [0.0, 0.0]");
            let blur = s.find("backdrop").map(|at| s[at..].chars().take(120).collect::<String>());
            let shadow = s.find("blur").map(|at| s[at..].chars().take(60).collect::<String>());
            if fades || blur.as_deref().is_some_and(|b| !b.contains("None") && !b.contains("0.0,")) {
                let f = s.find("fade_edges").map(|at| s[at..].chars().take(160).collect::<String>()).unwrap_or_default();
                eprintln!("DAMAGE special d{depth} {} | {f} | {blur:?} | {shadow:?}", line(*id));
            }
        }
    }
    if std::env::var_os("DEBUG_DAMAGE_AT").is_some() {
        for (rect, radius) in scene.backdrop_filters() {
            eprintln!("DAMAGE backdrop filter {:.1},{:.1} {:.1}x{:.1} radius {radius}", rect.x, rect.y, rect.width, rect.height);
        }
        // What's over the composer's band: every box from y 660 to 731,
        // not inside the transcript's rows (x 278..962), with its effects.
        for (id, depth) in &all {
            let b = scene.bounds(*id);
            if b.y >= 640. && b.y < 731. && b.height > 0. && b.height < 120. && !(b.x >= 277. && b.x + b.width <= 963.5 && b.width > 600.) {
                let effects = scene.effects(*id);
                eprintln!("DAMAGE at d{depth} {} effects {:?} kind {}", line(*id), effects, format!("{:?}", scene.kind(*id)).chars().take(400).collect::<String>());
            }
        }
    }
    for r in damage {
        eprintln!("DAMAGE   rect {:.1},{:.1} {:.1}x{:.1} = {:.0} px (device)", r.x, r.y, r.width, r.height, r.width * r.height * SCALE * SCALE);
        let near = |a: f32, b: f32| (a - b).abs() <= 2.;
        let matching: Vec<_> = all
            .iter()
            .filter(|(id, _)| {
                let b = scene.bounds(*id);
                near(b.x, r.x) && near(b.y, r.y) && near(b.width, r.width) && near(b.height, r.height)
            })
            .collect();
        for (id, _) in matching.iter().take(4) {
            eprintln!("DAMAGE     is {}", line(*id));
        }
        // Boxes sharing an edge with it (what bounds it), outside rows.
        if r.width * r.height > 2000. {
            for (id, depth) in all.iter().filter(|(id, _)| {
                let b = scene.bounds(*id);
                b.width >= 200. && b.height < 400. && (near(b.y, r.y) || near(b.y + b.height, r.y + r.height))
            }).take(16) {
                let p = scene.parent(*id).map(|p| line(p)).unwrap_or_default();
                eprintln!("DAMAGE     edge d{depth} {} < {p} kind {}", line(*id), format!("{:?}", scene.kind(*id)).chars().take(700).collect::<String>());
            }
        }
        // The deepest holding it, and up from there.
        let holds = |id| {
            let b = scene.bounds(id);
            b.width * b.height > 0. && b.x <= r.x + 0.5 && b.y <= r.y + 0.5 && b.x + b.width >= r.x + r.width - 0.5 && b.y + b.height >= r.y + r.height - 0.5
        };
        if let Some((deepest, _)) = all.iter().filter(|(id, _)| holds(*id)).max_by_key(|(_, d)| *d) {
            eprintln!("DAMAGE     in {}", line(*deepest));
            let mut at = scene.parent(*deepest);
            for _ in 0..5 {
                let Some(id) = at else { break };
                eprintln!("DAMAGE       < {}", line(id));
                at = scene.parent(id);
            }
        }
        // What paints inside it: the leaves within, by kind.
        let mut inside: std::collections::BTreeMap<String, usize> = Default::default();
        for (id, _) in &all {
            if !scene.children(*id).is_empty() {
                continue;
            }
            let b = scene.bounds(*id);
            if b.width * b.height > 0. && b.x < r.x + r.width && b.x + b.width > r.x && b.y < r.y + r.height && b.y + b.height > r.y {
                let k = kind(*id);
                *inside.entry(k.split(|c: char| c == '(' || c == '{' || c == ' ').next().unwrap_or("").to_owned()).or_default() += 1;
            }
        }
        eprintln!("DAMAGE     leaves within: {inside:?}");
    }
}

/// The process's CPU time so far, every thread.
fn process_cpu() -> Duration {
    // SAFETY: fills a rusage for this process.
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) };
    let t = |v: libc::timeval| Duration::new(v.tv_sec as u64, v.tv_usec as u32 * 1000);
    t(usage.ru_utime) + t(usage.ru_stime)
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

/// A reply being written, changed by part patches alone (calls arriving,
/// finishing, failing, editing a file edited before, two at once): each row
/// made from the one kept (`LiveRow`) is the row built whole from the reply
/// as it is.
#[test]
fn a_reply_made_from_its_patches_is_the_reply_built_whole() {
    let patched = |entry: &Json, ids: &[&str]| {
        let set: std::collections::HashSet<String> = ids.iter().map(|s| s.to_string()).collect();
        crate::chat::trusting(Some(&set), || built(entry))
    };
    // Its row, and its draft as published after it (`draft_of`).
    let patched = |entry: &Json, ids: &[&str]| (patched(entry, ids), crate::chat::draft_of(entry));
    let whole =
        |entry: &Json| crate::chat::built_whole(|| (built(entry), crate::chat::draft_of(entry)));
    let mut entry = turn(0, 200, None, true);
    // Text before the run being written: its block kept as it was.
    entry["parts"]
        .as_array_mut()
        .unwrap()
        .push(json!({ "id": "x0", "kind": "text", "text": "Looking at it." }));
    entry["parts"].as_array_mut().unwrap().push(call(1000, 2));
    assert_eq!(
        (built(&entry), crate::chat::draft_of(&entry)),
        whole(&entry)
    );
    let hits = crate::chat::live_row_hits();
    let mut fast = 0;
    for i in 1..=60 {
        let id = format!("t{i}");
        // Arriving: its call alone.
        let mut part = call(i, 2);
        let output = part.as_object_mut().unwrap().remove("output");
        if i % 10 == 0 {
            // An edit of a file edited before: one file, counted once.
            part = json!({ "id": id, "kind": "tool", "call": { "kind": "editFile", "path": "/repo/src/part3.rs" } });
        }
        entry["parts"].as_array_mut().unwrap().push(part);
        assert_eq!(patched(&entry, &[&id]), whole(&entry), "call {i} arriving");
        fast += 1;
        // Finished: its output, every seventh failing.
        let last = entry["parts"].as_array_mut().unwrap().last_mut().unwrap();
        if let Some(output) = output {
            last["output"] = output;
        }
        if i % 7 == 0 {
            last["isError"] = json!(true);
        }
        assert_eq!(patched(&entry, &[&id]), whole(&entry), "call {i} finishing");
        fast += 1;
        // Two at once now and then: an earlier one's result late, a thought.
        if i % 9 == 0 && i > 2 {
            let earlier = format!("t{}", i - 2);
            let at = entry["parts"]
                .as_array()
                .unwrap()
                .iter()
                .position(|p| p["id"] == earlier.as_str())
                .unwrap();
            entry["parts"][at]["output"] = json!("late\n");
            let thought = format!("r{i}");
            entry["parts"]
                .as_array_mut()
                .unwrap()
                .push(json!({ "id": thought, "kind": "reasoning", "text": "Next, the rest." }));
            assert_eq!(
                patched(&entry, &[&earlier, &thought]),
                whole(&entry),
                "call {i}: two at once"
            );
            fast += 1;
        }
    }
    assert_eq!(
        crate::chat::live_row_hits() - hits,
        fast,
        "every change made from the kept row"
    );
    // Text after the run: no change of the run's, built whole, and right.
    entry["parts"]
        .as_array_mut()
        .unwrap()
        .push(json!({ "id": "x9", "kind": "text", "text": "Done." }));
    assert_eq!(
        patched(&entry, &["x9"]),
        whole(&entry),
        "text after the run"
    );
    assert_eq!(crate::chat::live_row_hits() - hits, fast, "built whole");
}

/// The same with no text in the reply at all: its draft's words its text's
/// (none), as kept.
#[test]
fn a_reply_with_no_text_made_from_its_patches_is_the_reply_built_whole() {
    let mut entry = turn(1, 100, None, true);
    let whole =
        |entry: &Json| crate::chat::built_whole(|| (built(entry), crate::chat::draft_of(entry)));
    assert_eq!(
        (built(&entry), crate::chat::draft_of(&entry)),
        whole(&entry)
    );
    for i in 2..=20 {
        let id = format!("t{i}");
        entry["parts"].as_array_mut().unwrap().push(call(i, 1));
        let set: std::collections::HashSet<String> = [id].into_iter().collect();
        let fast = crate::chat::trusting(Some(&set), || built(&entry));
        assert_eq!(
            (fast, crate::chat::draft_of(&entry)),
            whole(&entry),
            "call {i}"
        );
    }
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
    /// Spent in the time frames leave over (a window's lists building rows
    /// ahead): instructions, and calls into scripts.
    spare: u64,
    spare_calls: u64,
    /// The whole process's CPU over the frame (every thread: its spare time,
    /// the GPU driver's): what the 5% budget is in.
    process: Duration,
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
        self.spare += other.spare;
        self.spare_calls += other.spare_calls;
        self.process += other.process;
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
    /// `ACTIVITY_PUBLISH_AT_FRAME`: the host publishes as the next frame
    /// begins (its time already that frame's), not a frame before it; the
    /// frame then takes no time of its own (`ticked`).
    at_frame: bool,
    ticked: bool,
    /// With time frames leave over, as a window has (`TestApp::with_spare_time`).
    spare: bool,
    /// A frame's time on the test's clock: 1/60 s unless set (`activity_goal`'s 1/120).
    step: f64,
}

impl Bench {
    /// With spare time, as a window has: the benchmark's.
    fn windowed() -> Self {
        Self::made_with(None, true, true)
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
        if std::mem::take(&mut self.ticked) {
            return;
        }
        if let Some((clock, at)) = &mut self.clock {
            *at += Duration::from_secs_f64(self.step);
            clock.advance(*at, || {});
        }
    }

    fn made(clock: Option<(zgx::clock::VirtualClock, Duration)>, draw: bool) -> Self {
        Self::made_with(clock, draw, false)
    }

    fn made_with(mut clock: Option<(zgx::clock::VirtualClock, Duration)>, draw: bool, spare: bool) -> Self {
        let fixture = ui().join("chat_fixture.luau");
        let app = if spare {
            TestApp::with_spare_time(move |runtime| ScriptHost::new(runtime, fixture))
        } else {
            TestApp::new(move |runtime| ScriptHost::new(runtime, fixture))
        };
        let gpu = draw.then(|| {
            let mut gpu = GpuRenderer::new((W * SCALE) as u32, (H * SCALE) as u32).expect("a GPU");
            gpu.set_scale_factor(SCALE);
            // (`DEBUG_SCROLL_COPY`: copying scrolls as a window does; offscreen
            // on macOS the renderer repaints them instead.)
            if std::env::var_os("DEBUG_SCROLL_COPY").is_some() {
                gpu.debug_allow_scroll_copies();
            }
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
        let at_frame = std::env::var_os("ACTIVITY_PUBLISH_AT_FRAME").is_some();
        let mut bench = Self { app, gpu, messages, draft, before, after: Vec::new(), clock, at_frame, ticked: false, spare, step: 1. / 60. };
        bench.settle(10);
        bench
    }

    /// A publish's time: as the next frame begins, with `at_frame`.
    fn publishing(&mut self) {
        if self.at_frame && !self.ticked {
            self.tick();
            self.ticked = true;
        }
    }

    /// The transcript with the turn as `entry` (the host's publish).
    fn publish(&mut self, entry: &Json) -> Duration {
        self.publishing();
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

    /// The transcript with the turn as `entry`, changed since it was last
    /// built in the parts `patched` alone (as part patches bring a change).
    fn publish_patched(&mut self, entry: &Json, patched: &[String]) -> Duration {
        let patched: std::collections::HashSet<String> = patched.iter().cloned().collect();
        self.publishing();
        let start = thread_cpu();
        let row = crate::chat::trusting(Some(&patched), || built(entry));
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
        self.publishing();
        let start = thread_cpu();
        let draft = crate::chat::draft_of(entry);
        let host = thread_cpu() - start;
        self.draft.set(draft);
        host
    }

    /// One frame as a window's: laid out, its damage drawn.
    fn frame(&mut self) -> Cost {
        // A window's refresh: its spare time first (lists building rows
        // ahead), then the frame's own work.
        let (mut spare, mut spare_calls) = (0, 0);
        let process_from = process_cpu();
        let debug = std::env::var_os("DEBUG_FRAMES").is_some();
        if debug {
            eprintln!("@SPARE");
        }
        if self.spare {
            let _ = profile::take();
            let calls = self.app.calls();
            self.app.spend_spare_time();
            spare_calls = self.app.calls() - calls;
            spare = profile::take().iter().map(|(_, instructions, _)| instructions).sum();
        }
        if debug {
            eprintln!("@FRAME");
        }
        let _ = profile::take();
        let calls = self.app.calls();
        let wanted = self.app.clock.wants_frame();
        self.tick();
        let start = thread_cpu();
        self.app.frame();
        let report = self.app.ui.scene.borrow_mut().flush();
        let drawn = !report.damage.is_empty();
        if drawn && let Some(gpu) = &mut self.gpu {
            let stats = gpu.render(&self.app.ui.scene.borrow(), &report.damage).expect("a frame drawn");
            if std::env::var_os("DEBUG_GPU").is_some() && zgx::trace::enabled() {
                eprintln!("GPU {stats:?}");
            }
            if std::env::var_os("DEBUG_DAMAGE").is_some() && DAMAGE_SHOWN.get() {
                eprintln!("DAMAGE frame: {} rects, {} px repainted, {} scroll copies, {} layer repaints, {} backdrop copies", report.damage.len(), stats.damaged_pixels, stats.scroll_copies, stats.layer_repaints, stats.backdrop_copies);
                show_damage(&self.app.ui.scene.borrow(), &report.damage);
            }
        }
        let cpu = thread_cpu() - start;
        let process = process_cpu() - process_from;
        let mut cost = Cost { cpu, process, calls: self.app.calls() - calls, frames: 1, drawn: drawn as usize, wanted: wanted as usize, spare, spare_calls, ..Cost::default() };
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
        "{n:>5} {:>4}K {open:<6} {what:<7} | {:>9.0} {:>8.0} {:>8.0} {:>8.0} {:>7.0} {:>7.0} {:>7.0} | {:>8.0} | {:>7.1} {:>7.1} | {:>6.0} | {:>4.2} {:>4.2} | {nodes:>6} | {:>8.0} {:>6.1} | {:>8.0}",
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
        cost.spare as f64 / per / 1e3,
        cost.spare_calls as f64 / per,
        (cost.process + cost.host).as_secs_f64() * 1e6 / per,
    );
}

fn header() {
    println!(
        "{:>5} {:>5} {:<6} {:<7} | {:>9} {:>8} {:>8} {:>8} {:>7} {:>7} {:>7} | {:>8} | {:>7} {:>7} | {:>6} | {:>4} {:>4} | {:>6} | {:>8} {:>6} | {:>8}",
        "calls", "think", "group", "what", "kinstr", "script", "update", "layout", "damage", "paint", "encode", "cpu µs", "host µs", "phs µs", "calls", "drew", "want", "nodes", "spare k", "sp.cl", "proc µs"
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
    let mut bench = Bench::windowed();
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
    if std::env::var_os("DEBUG_TREE").is_some() && open {
        dump_row(&mut bench);
    }
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

    // New calls arrive (the thought done, a call after it), each a frame:
    // as the engine sends them to a host that asked for part patches, the
    // new call alone changed (the rest of the reply as it was built).
    entry = turn(calls, 1024, Some(&text), true);
    {
        // (The tail thought done: calls follow it.)
        let parts = entry["parts"].as_array_mut().unwrap();
        let last = parts.pop().unwrap();
        parts.insert(1, last);
    }
    bench.publish(&entry);
    bench.settle(24);
    let mut call_cost = Cost::default();
    // (And the second after each: the summary's roll, the row's arrival.)
    let mut call_after = Cost::default();
    for _ in 0..samples {
        calls += 1;
        let new = call(calls, 4);
        let id = new["id"].as_str().unwrap().to_owned();
        entry["parts"].as_array_mut().unwrap().push(new);
        if std::env::var_os("DEBUG_FRAMES").is_some() {
            eprintln!("@CALL {calls}");
        }
        let tracing = std::env::var_os("DEBUG_TRACE").is_some() && calls == n + 2;
        if tracing {
            zgx::trace::set_enabled(true);
        }
        let host = bench.publish_patched(&entry, &[id]);
        let mut cost = bench.frame();
        cost.host = host;
        call_cost.add(&cost);
        if tracing {
            // (A few frames into the arrival's animations.)
            // (`DEBUG_TRACE_FRAMES`: how many of the arrival's frames, 7 unless set.)
            let traced: usize = std::env::var("DEBUG_TRACE_FRAMES").ok().and_then(|n| n.parse().ok()).unwrap_or(7);
            bench.frames(traced - 1);
            for frame in zgx::trace::frames(2 * traced) {
                let mut runs = frame.runs.clone();
                runs.sort_by(|a, b| b.micros.total_cmp(&a.micros));
                let phases: String = frame.phases.iter().filter(|p| p.1 > 0).map(|p| format!(" {} {}k", p.0, p.1 / 1000)).collect();
                let total: u64 = frame.phases.iter().map(|p| p.1).sum();
                eprintln!(
                    "TRACE frame {} | measured {} nodes ({:.0} µs) | runs {} | {}k instr:{phases}",
                    frame.index, frame.layout.measured, frame.layout.micros, frame.runs.len(), total / 1000
                );
                for run in runs.iter().take(12) {
                    eprintln!("TRACE   run {:>7.1} µs {}", run.micros, run.name.as_deref().unwrap_or("?"));
                }
                // Each measured node by its number: its kind, box and the
                // three nodes above it (as `dump_row` numbers them).
                let mut root = bench.app.find("transcript").expect("the transcript");
                let scene = bench.app.ui.scene.borrow();
                while let Some(up) = scene.parent(root) {
                    root = up;
                }
                let mut by_number = std::collections::HashMap::new();
                let mut stack = vec![root];
                while let Some(id) = stack.pop() {
                    by_number.insert(id.number(), id);
                    stack.extend(scene.children(id).iter().copied());
                }
                let kind = |id| {
                    let k = format!("{:?}", scene.kind(id));
                    k.split(|c: char| c == '(' || c == '{' || c == ' ').next().unwrap_or("").to_owned()
                };
                // (`DEBUG_NODE=<n>`: that node's kind, what's above it, and the
                // texts under the box above it.)
                // (A number: that node, as the frame's runs name it.)
                let asked = std::env::var("DEBUG_NODE").ok();
                let numbered = asked.as_deref().and_then(|n| n.parse::<u32>().ok());
                let pick = match numbered {
                    Some(n) => Some(n),
                    None if asked.is_some() && frame.index > 40 => frame.layout.top.first().map(|m| m.node),
                    None => None,
                };
                if let Some(n) = pick
                    && !NODE_SHOWN.replace(true)
                    && let Some(&id) = by_number.get(&n)
                {
                    let mut at = Some(id);
                    for _ in 0..8 {
                        let Some(node) = at else { break };
                        let b = scene.bounds(node);
                        eprintln!("NODE #{} {} {:.0},{:.0} {:.0}x{:.0}", node.number(), format!("{:?}", scene.kind(node)).chars().take(160).collect::<String>(), b.x, b.y, b.width, b.height);
                        at = scene.parent(node);
                    }
                    let mut stack = vec![scene.parent(scene.parent(id).unwrap_or(id)).unwrap_or(id)];
                    while let Some(node) = stack.pop() {
                        if let zgx::scene::NodeKind::Text { text, .. } = scene.kind(node) {
                            eprintln!("NODE text {text:?}");
                        }
                        stack.extend(scene.children(node).iter().copied());
                    }
                }
                for m in &frame.layout.top {
                    let Some(&id) = by_number.get(&m.node) else {
                        eprintln!("TRACE   measured #{} {:.1} µs (own {:.1}) (gone)", m.node, m.micros, m.self_micros);
                        continue;
                    };
                    let b = scene.bounds(id);
                    let mut up = String::new();
                    let mut at = scene.parent(id);
                    for _ in 0..3 {
                        let Some(p) = at else { break };
                        up.push_str(&format!(" < #{} {}", p.number(), kind(p)));
                        at = scene.parent(p);
                    }
                    eprintln!(
                        "TRACE   measured #{} {} {:.0},{:.0} {:.0}x{:.0} ({} children) {:.1} µs (own {:.1}){up}",
                        m.node, kind(id), b.x, b.y, b.width, b.height, scene.children(id).len(), m.micros, m.self_micros
                    );
                }
            }
            zgx::trace::set_enabled(false);
        }
        let mut after = cost;
        for f in 0..60 {
            let frame = bench.frame();
            if std::env::var_os("DEBUG_ARRIVAL").is_some() && calls == n + 2 {
                let k = |phase: Phase| frame.phase(phase) / 1000;
                eprintln!(
                    "ARRIVAL f{f:02} {}k | script {}k update {}k layout {}k paint {}k encode {}k | calls {} | proc {} µs",
                    frame.instructions() / 1000, k(Phase::Script), k(Phase::Update), k(Phase::Layout), k(Phase::Paint), k(Phase::Encode),
                    frame.calls, frame.process.as_micros()
                );
            }
            after.add(&frame);
        }
        call_after.add(&after);
    }
    row(n, think, label, "call", &call_cost, samples, bench.nodes());
    row(n, think, label, "call+1s", &call_after, samples, bench.nodes());

    // The running call's output grows (that call patched).
    let mut output_cost = Cost::default();
    for k in 0..samples {
        let parts = entry["parts"].as_array_mut().unwrap();
        let last = parts.last_mut().unwrap();
        let had = last["output"].as_str().unwrap_or_default().to_owned();
        last["output"] = json!(format!("{had}more output {k}\n"));
        let id = last["id"].as_str().unwrap().to_owned();
        let host = bench.publish_patched(&entry, &[id]);
        let mut cost = bench.frame();
        cost.host = host;
        output_cost.add(&cost);
        bench.settle(4);
    }
    row(n, think, label, "output", &output_cost, samples, bench.nodes());
    drop(bench);

    // Settled: at rest, then scrolled past.
    let mut bench = Bench::windowed();
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
    // (`DEBUG_DAMAGE`: each scroll frame's damage, `DEBUG_SCROLL_STEP` points a step.)
    let step: f32 = std::env::var("DEBUG_SCROLL_STEP").ok().and_then(|s| s.parse().ok()).unwrap_or(120.);
    DAMAGE_SHOWN.set(true);
    let noting = std::env::var_os("DEBUG_DAMAGE").is_some();
    if noting {
        zgx::trace::set_enabled(true);
    }
    for _ in 0..60 {
        bench.app.scroll("transcript", step);
        scroll.add(&bench.frame());
    }
    DAMAGE_SHOWN.set(false);
    if noting {
        // The renderer's notes on the scroll's frames: what copied, what
        // repainted and why.
        let mut notes: std::collections::BTreeMap<String, usize> = Default::default();
        for frame in zgx::trace::frames(120) {
            for note in &frame.notes {
                *notes.entry(note.to_string()).or_default() += 1;
            }
        }
        for (note, n) in notes {
            eprintln!("DAMAGE note x{n}: {note}");
        }
        zgx::trace::set_enabled(false);
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

/// Wing's goal, measured as a window would spend it: under 5% of a core at
/// 120 fps, for a long tool group streaming (calls arriving once a second,
/// each finishing half a second later), folded and open, and for scrolling
/// it open (a wheel's 8 points every 1/120 s). Frames on a 120 Hz test clock,
/// drawn offscreen with scrolls copied as a window copies them, spare time
/// spent as a window spends it; a frame only when one is wanted (an idle
/// window draws none). The process's CPU over the time simulated is the
/// figure (`ACTIVITY_GOAL_CALLS`, 1,000 unless set; `ACTIVITY_GOAL_SECONDS`, 20).
#[test]
#[ignore = "a benchmark: run it --release"]
fn activity_goal() {
    profiled();
    // SAFETY: set before the renderer reads it, on this test's thread alone.
    unsafe { std::env::set_var("DEBUG_SCROLL_COPY", "1") };
    let n: usize = std::env::var("ACTIVITY_GOAL_CALLS").ok().and_then(|v| v.parse().ok()).unwrap_or(1000);
    let seconds: f64 = std::env::var("ACTIVITY_GOAL_SECONDS").ok().and_then(|v| v.parse().ok()).unwrap_or(20.);
    let report = |what: &str, cost: &Cost, frames: usize, ticks: usize, seconds: f64| {
        let names: String = PHASES
            .iter()
            .zip(cost.phases.iter())
            .filter(|(_, p)| p.0 > 0)
            .map(|(name, p)| format!(" {}={:.2}M/s", name.name(), p.0 as f64 / 1e6 / seconds))
            .collect();
        println!(
            "GOAL {what}: {:.2}% CPU ({:.1} ms process in {seconds:.0} s; main thread {:.2}%), {frames}/{ticks} frames drawn, host {:.1} ms; spare {:.1}M/s;{names}",
            cost.process.as_secs_f64() / seconds * 100.,
            cost.process.as_secs_f64() * 1e3,
            cost.cpu.as_secs_f64() / seconds * 100.,
            cost.host.as_secs_f64() * 1e3,
            cost.spare as f64 / 1e6 / seconds,
        );
    };
    // (`ACTIVITY_GOAL_ONLY=scroll`: the scroll alone.)
    let streams = std::env::var("ACTIVITY_GOAL_ONLY").ok().as_deref() != Some("scroll");
    for open in [false, true].into_iter().filter(|_| streams) {
        let mut bench = Bench::made_with(Some((zgx::clock::VirtualClock::new(), Duration::ZERO)), true, true);
        bench.step = 1. / 120.;
        bench.after = vec![built(&prompt("u2", "And the rest?"))];
        let mut entry = turn(n, 2000, None, true);
        bench.publish(&entry);
        bench.settle(120);
        bench.to_turn();
        // (A group at work is open unless folded: pressed to fold it.)
        if !open {
            bench.toggle();
        }
        bench.settle(240);
        let mut cost = Cost::default();
        let (mut frames, ticks) = (0, (seconds * 120.) as usize);
        let mut next = n + 1;
        let mut finishing: Option<(usize, Json)> = None;
        for tick in 0..ticks {
            let mut host = Duration::ZERO;
            let published = if tick % 120 == 0 {
                // A call arriving: its call alone.
                let mut part = call(next, 4);
                let output = part.as_object_mut().unwrap().remove("output");
                let id = format!("t{next}");
                entry["parts"].as_array_mut().unwrap().push(part);
                host += bench.publish_patched(&entry, &[id]);
                finishing = output.map(|o| (next, o));
                next += 1;
                true
            } else if tick % 120 == 60
                && let Some((i, output)) = finishing.take()
            {
                // Finished: its output.
                entry["parts"].as_array_mut().unwrap().last_mut().unwrap()["output"] = output;
                host += bench.publish_patched(&entry, &[format!("t{i}")]);
                true
            } else {
                false
            };
            if published || bench.app.clock.wants_frame() {
                let mut c = bench.frame();
                c.host = host;
                cost.add(&c);
                frames += 1;
            } else {
                bench.tick();
            }
        }
        report(&format!("{n} calls streaming, {}", if open { "open" } else { "folded" }), &cost, frames, ticks, seconds);
    }
    // Scrolled open, down past the turn and back.
    let mut bench = Bench::made_with(Some((zgx::clock::VirtualClock::new(), Duration::ZERO)), true, true);
    bench.step = 1. / 120.;
    bench.after = vec![built(&prompt("u2", "And the rest?"))];
    bench.publish(&turn(n, 2000, None, false));
    bench.settle(120);
    bench.to_turn();
    bench.toggle();
    bench.settle(240);
    let mut cost = Cost::default();
    let ticks = (seconds * 120.) as usize;
    for tick in 0..ticks {
        // (Down for half, up for half: it stays in the group.)
        let dy = if tick < ticks / 2 { 8. } else { -8. };
        bench.app.scroll("transcript", dy);
        cost.add(&bench.frame());
    }
    report(&format!("{n} calls open, scrolled ({} nodes at the end)", bench.nodes()), &cost, ticks, ticks, seconds);
}

/// The goal (`activity_goal`) for long thinking: a reply whose thought is
/// written for minutes (`ACTIVITY_GOAL_SECONDS`, 120 unless set: 20 more
/// characters every 50 ms, as a model thinks), its group folded and open,
/// and a long thought (`ACTIVITY_GOAL_THINK` KB, 50) shown whole and
/// scrolled (a wheel's 8 points every 1/120 s, for 10 s).
#[test]
#[ignore = "a benchmark: run it --release"]
fn activity_goal_thinking() {
    profiled();
    // SAFETY: set before the renderer reads it, on this test's thread alone.
    unsafe { std::env::set_var("DEBUG_SCROLL_COPY", "1") };
    let seconds: f64 = std::env::var("ACTIVITY_GOAL_SECONDS").ok().and_then(|v| v.parse().ok()).unwrap_or(120.);
    let think_kb: usize = std::env::var("ACTIVITY_GOAL_THINK").ok().and_then(|v| v.parse().ok()).unwrap_or(50);
    let report = |what: &str, cost: &Cost, frames: usize, ticks: usize, seconds: f64| {
        let names: String = PHASES
            .iter()
            .zip(cost.phases.iter())
            .filter(|(_, p)| p.0 > 0)
            .map(|(name, p)| format!(" {}={:.2}M/s", name.name(), p.0 as f64 / 1e6 / seconds))
            .collect();
        println!(
            "GOAL {what}: {:.2}% CPU ({:.1} ms process in {seconds:.0} s; main thread {:.2}%), {frames}/{ticks} frames drawn, host {:.1} ms; spare {:.1}M/s;{names}",
            cost.process.as_secs_f64() / seconds * 100.,
            cost.process.as_secs_f64() * 1e3,
            cost.cpu.as_secs_f64() / seconds * 100.,
            cost.host.as_secs_f64() * 1e3,
            cost.spare as f64 / 1e6 / seconds,
        );
    };
    let only = std::env::var("ACTIVITY_GOAL_ONLY").ok();
    for open in [true, false].into_iter().filter(|_| only.as_deref() != Some("scroll")) {
        let mut bench = Bench::made_with(Some((zgx::clock::VirtualClock::new(), Duration::ZERO)), true, true);
        bench.step = 1. / 120.;
        let mut text = prose("the thought being written", 2000);
        let mut entry = turn(10, 1024, Some(&text), true);
        bench.publish(&entry);
        bench.settle(120);
        // (A group at work is open unless folded: pressed to fold it, and
        // back at the end, where a turn at work is followed.)
        if !open {
            bench.toggle();
            bench.app.scroll("transcript", 100_000.);
        }
        bench.settle(240);
        let mut cost = Cost::default();
        let (mut frames, ticks) = (0, (seconds * 120.) as usize);
        let words = "and then the parser reads the next frame; ";
        for tick in 0..ticks {
            let mut host = Duration::ZERO;
            let published = tick % 6 == 0;
            if published {
                let at = (tick / 6) % words.len().saturating_sub(20).max(1);
                text.push_str(&words[at..(at + 20).min(words.len())]);
                if tick % 600 == 0 {
                    text.push_str("\n\n");
                }
                entry["parts"].as_array_mut().unwrap().last_mut().unwrap()["text"] = json!(text);
                host += bench.publish_draft(&entry);
            }
            if published || bench.app.clock.wants_frame() {
                let mut c = bench.frame();
                c.host = host;
                cost.add(&c);
                frames += 1;
            } else {
                bench.tick();
            }
        }
        report(
            &format!("thinking {seconds:.0} s ({} KB at the end), {}", text.len() / 1024, if open { "open" } else { "folded" }),
            &cost,
            frames,
            ticks,
            seconds,
        );
    }
    // A long thought shown whole, scrolled down and back.
    let mut bench = Bench::made_with(Some((zgx::clock::VirtualClock::new(), Duration::ZERO)), true, true);
    bench.step = 1. / 120.;
    bench.after = vec![built(&prompt("u2", "And the rest?"))];
    bench.publish(&turn(10, think_kb * 1024, None, false));
    bench.settle(120);
    bench.to_turn();
    bench.toggle();
    bench.app.click("role:button ~\"Show all\"");
    bench.settle(240);
    let mut cost = Cost::default();
    let ticks = 10 * 120;
    let traced = std::env::var_os("DEBUG_GOAL_TRACE").is_some();
    for tick in 0..ticks {
        let dy = if tick < ticks / 2 { 8. } else { -8. };
        if traced && tick == 300 {
            zgx::trace::set_enabled(true);
        }
        bench.app.scroll("transcript", dy);
        cost.add(&bench.frame());
        if traced && tick == 303 {
            // (`DEBUG_GOAL_TRACE`: a few scroll frames' runs and what they laid out.)
            for frame in zgx::trace::frames(8) {
                let mut runs = frame.runs.clone();
                runs.sort_by(|a, b| b.micros.total_cmp(&a.micros));
                let phases: String = frame.phases.iter().filter(|p| p.1 > 0).map(|p| format!(" {} {}k", p.0, p.1 / 1000)).collect();
                eprintln!("TRACE frame {} | measured {} | runs {} |{phases}", frame.index, frame.layout.measured, frame.runs.len());
                for run in runs.iter().take(8) {
                    eprintln!("TRACE   run {:>7.1} µs {}", run.micros, run.name.as_deref().unwrap_or("?"));
                }
            }
            zgx::trace::set_enabled(false);
        }
    }
    report(&format!("a {think_kb} KB thought shown whole, scrolled ({} nodes)", bench.nodes()), &cost, ticks, ticks, 10.);
}

#[test]
#[ignore = "a comparison: run it --release, as the doc above says"]
fn activity_pixels() {
    let out = PathBuf::from(std::env::var("ACTIVITY_PIXELS").expect("ACTIVITY_PIXELS=<dir>"));
    std::fs::create_dir_all(&out).unwrap();
    // (Each frame's pixels too, raw, with ACTIVITY_PIXELS_KEEP: 15 MB each.)
    // (Or only those of states named starting so: ACTIVITY_PIXELS_KEEP=live-open.)
    let keep_only = std::env::var("ACTIVITY_PIXELS_KEEP").ok();
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
            if keep_only.as_deref().is_some_and(|only| only.is_empty() || only == "1" || name.starts_with(only)) {
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

/// What one closed call's row costs to build, alone: an open group of calls
/// (ui/activity_rows_fixture.luau) in a window tall enough to show them all,
/// a hundred calls joining it at once. Per row: instructions by phase, calls
/// into scripts, scene nodes.
///
///   cargo test --release --bin zeron-zgx activity_row_cost -- --ignored --nocapture
#[test]
#[ignore = "a benchmark: run it --release, as the doc above says"]
fn activity_row_cost() {
    profiled();
    let fixture = ui().join("activity_rows_fixture.luau");
    let app = TestApp::new(move |runtime| ScriptHost::new(runtime, fixture));
    app.ui.scene.borrow_mut().resize(900., 4000.);
    let rows = app.host.shared("bench.rows", Value::Num(0.));
    let shown = app.host.shared("bench.shown", Value::Bool(false));
    // (One call shown first: the group's header and frame, apart from rows.)
    rows.set(Value::Num(1.));
    shown.set(Value::Bool(true));
    app.frames(60);
    shown.set(Value::Bool(false));
    app.frames(60);
    assert_eq!(app.error(), None);
    let k = 100;
    let header = app.ui.scene.borrow().len();
    let _ = profile::take();
    shown.set(Value::Bool(true));
    app.frames(1);
    let one: u64 = profile::take().iter().map(|p| p.1).sum();
    let nodes = app.ui.scene.borrow().len();
    shown.set(Value::Bool(false));
    app.frames(60);
    rows.set(Value::Num((k + 1) as f64));
    app.frames(2);
    let _ = profile::take();
    let calls = app.calls();
    let start = thread_cpu();
    shown.set(Value::Bool(true));
    app.frames(1);
    let cpu = thread_cpu() - start;
    let calls = app.calls() - calls;
    let phases = profile::take();
    let added = app.ui.scene.borrow().len() - nodes;
    let total = phases.iter().map(|p| p.1).sum::<u64>().saturating_sub(one);
    let _ = header;
    let per = |n: u64| n as f64 / k as f64 / 1e3;
    print!("a row: {:.0}k instr ({:.1} µs cpu), {:.1} calls, {:.1} nodes |", per(total), cpu.as_secs_f64() * 1e6 / k as f64, calls as f64 / k as f64, added as f64 / k as f64);
    for (phase, instructions, _) in &phases {
        if *instructions > 0 {
            print!(" {} {:.0}k", phase.name(), per(*instructions));
        }
    }
    println!();
    assert_eq!(app.error(), None);
    // (`DEBUG_ROW_TREE`: one row's nodes, kinds and boxes.)
    if std::env::var_os("DEBUG_ROW_TREE").is_some() {
        let scene = app.ui.scene.borrow();
        let mut stack = vec![scene.root()];
        let mut verb = None;
        while let Some(id) = stack.pop() {
            if format!("{:?}", scene.kind(id)).starts_with("Text { text: \"Edited\"") {
                verb = Some(id);
                break;
            }
            stack.extend(scene.children(id).iter().copied());
        }
        // The verb's line, its button, the row.
        let mut row = verb.expect("an edit's row");
        for _ in 0..3 {
            row = scene.parent(row).unwrap();
        }
        fn walk(scene: &zgx::scene::Scene, id: zgx::scene::NodeId, depth: usize) {
            let b = scene.bounds(id);
            let k = format!("{:?}", scene.kind(id));
            eprintln!("ROW {:indent$}#{} {} {:.0},{:.0} {:.0}x{:.0}", "", id.number(), k.chars().take(60).collect::<String>(), b.x, b.y, b.width, b.height, indent = depth * 2);
            for c in scene.children(id) {
                walk(scene, *c, depth + 1);
            }
        }
        walk(&scene, row, 0);
    }
}
