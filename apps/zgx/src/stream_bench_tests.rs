//! The stream bench, headless: a short recording replayed through the chat
//! view's real path reaches the transcript. The page is the bench's own
//! (ui/stream_bench_fixture.luau, nothing stubbed), its chat state a view made
//! as `chat::install` makes a window's, `chat.open` the same action: the page
//! opens the chat, `subscribe` hands it to the bench, and each frame goes to
//! `heard_frame`.

use super::*;
use std::{collections::VecDeque, future::Future, pin::Pin, time::Duration};
use zgx_script::testing::TestApp;

type Queued = Rc<RefCell<VecDeque<Pin<Box<dyn Future<Output = ()>>>>>>;

const TEXT: &str = "## Streamed through the host\n\n\
    The reply arrives a few words at a time, each chunk an append the engine sends.\n\n\
    - first, the row\n- then its words\n- then its end\n\n\
    Every chunk is published as the draft, and the page shows it as it comes.\n";

/// The window's chat view, as `chat::install` makes it, its `chat.open` too.
fn window_view(host: &ScriptHost) -> Rc<View> {
    let view = Rc::new(View {
        session: host.shared("chat.session", Value::Nil),
        messages: host.shared("chat.messages", Value::List(Rc::from([]))),
        subagents: host.shared("chat.subagents", Value::List(Rc::from([]))),
        draft: host.shared("chat.draft", draft_value("", "")),
        rpc: RefCell::default(),
        open: RefCell::default(),
        pending: RefCell::default(),
        generation: Cell::new(0),
        snapshot: Cell::new(false),
    });
    CHATS.with(|chats| {
        chats.views.borrow_mut().push(Rc::downgrade(&view));
        // (The page's catalog, as `chat::install` shares it.)
        chats.catalog.borrow_mut().push(host.shared("chat.catalog", chats.catalog_value()));
    });
    let v = view.clone();
    host.action("chat.open", move |id| {
        let id = text(&id);
        v.open_chat((!id.is_empty()).then_some(id));
    });
    // (The composer's models: an engine's, none here.)
    host.action("chat.catalog", |_| {});
    host.action("chat.catalog_on", |_| {});
    view
}

fn draft(view: &View) -> Json {
    view.draft
        .with_untracked(|value| serde_json::from_str(&value.to_json()).unwrap())
}

fn messages(view: &View) -> Json {
    view.messages
        .with_untracked(|value| serde_json::from_str(&value.to_json()).unwrap())
}

#[test]
fn a_recorded_stream_reaches_the_transcript_through_the_host() {
    // The recording, written as the generator writes one.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("frames.jsonl");
    let frames = record::frames(TEXT, 24, 1_700_000_000_000);
    let lines: Vec<String> = frames.iter().map(|f| f.to_string()).collect();
    std::fs::write(&path, lines.join("\n") + "\n").unwrap();
    // (Played on the test's executor, its frames 60 ms apart.)
    let queued: Queued = Rc::default();
    let q = queued.clone();
    set(Bench::new(
        path,
        Duration::from_millis(60),
        Duration::from_millis(30),
        Rc::new(move |work| q.borrow_mut().push_back(work)),
    ));

    let made: Rc<RefCell<Option<Rc<View>>>> = Rc::default();
    let slot = made.clone();
    let mut app = TestApp::new(|runtime| {
        let host = ScriptHost::new(
            runtime,
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("ui/stream_bench_fixture.luau"),
        );
        *slot.borrow_mut() = Some(window_view(&host));
        host
    });
    let view = made.borrow().clone().unwrap();
    app.ui.scene.borrow_mut().resize(900., 700.);
    let pump = |app: &TestApp| {
        while let Some(work) = queued.borrow_mut().pop_front() {
            app.executor.borrow_mut().spawn(work);
        }
    };
    // The page opened the chat: the bench has it, and its reset shows.
    assert!(
        app.wait_until(|app| {
            pump(app);
            messages(&view).as_array().is_some_and(|m| !m.is_empty())
        }),
        "the page never opened the bench's chat: {:?}",
        view.open_id()
    );
    assert_eq!(view.open_id().as_deref(), Some(record::CHAT));
    assert_eq!(
        view.session.with_untracked(|s| s.field("state").map(text)),
        Some("ready".into())
    );

    // Mid-stream: the draft is the reply's words so far (its text block's),
    // and the transcript shows them.
    let mut seen = None;
    assert!(
        app.wait_until(|app| {
            pump(app);
            let d = draft(&view);
            let so_far = d["part_text"].as_str().unwrap_or_default();
            if d["id"] != record::REPLY || so_far.len() < 40 {
                return false;
            }
            seen = Some(d.clone());
            app.find("~\"Streamed through the host\"").is_some()
        }),
        "the draft never showed the reply mid-stream: {}\n{}",
        draft(&view),
        app.tree()
    );
    let seen = seen.unwrap();
    assert_eq!(seen["part"], record::PART);
    let so_far = seen["part_text"].as_str().unwrap();
    assert!(
        TEXT.starts_with(so_far) && so_far.len() < TEXT.len(),
        "a prefix of the reply: {so_far:?}"
    );
    assert_eq!(seen["text"], seen["part_text"]);

    // Its end: the row holds the whole reply, the draft lets it go.
    assert!(
        app.wait_until(|app| {
            pump(app);
            messages(&view)
                .as_array()
                .is_some_and(|m| m.len() == 2 && m[1]["streaming"] != true)
        }),
        "the reply never ended: {}",
        messages(&view)
    );
    assert_eq!(draft(&view)["text"], "");
    app.frames(5);
    assert!(
        app.find("~\"each chunk an append the engine sends\"")
            .is_some(),
        "{}",
        app.tree()
    );
    assert!(
        app.find("~\"the page shows it as it comes\"").is_some(),
        "{}",
        app.tree()
    );
    assert_eq!(app.error(), None);
}

/// The standard recording (reply(80), 24-byte appends, 40 a second) replayed
/// through the host's chat path into the bench's page, headless, at a
/// window's pace (120 frames a second, each drawn offscreen by zgx's
/// renderer): the main thread's CPU over the stream, and, run under
/// `sample`, where it goes. A probe, not a check:
///
///     cargo test --release --features stream-bench --bin zeron-zgx -- --ignored --nocapture stream_bench_profile
/// Every node, as `zgx inspect tree --all` lists it, each line with the
/// lines of what holds it that say something (an id, a role or a text).
fn full_tree(app: &TestApp) -> std::collections::HashMap<u32, String> {
    let options = zgx_script::driver::TreeOptions {
        depth: None,
        budget: usize::MAX,
        all: true,
    };
    let (lines, _) = zgx_script::driver::tree(&app.ui, &app.handle, None, &options);
    if let Some(path) = std::env::var_os("PROBE_TREE_OUT") {
        let text: Vec<String> = lines
            .iter()
            .map(|l| {
                format!(
                    "{}@{} {} {} {:?} {:.0},{:.0} {:.0}x{:.0}",
                    "  ".repeat(l.depth),
                    l.number,
                    l.id.as_deref().unwrap_or(""),
                    l.role.unwrap_or(""),
                    l.text.as_deref().unwrap_or(""),
                    l.bounds.x,
                    l.bounds.y,
                    l.bounds.width,
                    l.bounds.height
                )
            })
            .collect();
        let _ = std::fs::write(path, text.join("\n"));
    }
    let mut out = std::collections::HashMap::new();
    let mut path: Vec<String> = Vec::new();
    for line in &lines {
        path.truncate(line.depth);
        let mut own = format!("@{}", line.number);
        if let Some(id) = &line.id {
            own.push_str(&format!(" #{id}"));
        }
        if let Some(role) = line.role {
            own.push_str(&format!(" {role}"));
        }
        if let Some(text) = &line.text {
            own.push_str(&format!(" {:?}", text.chars().take(40).collect::<String>()));
        }
        own.push_str(&format!(" {:.0}x{:.0}", line.bounds.width, line.bounds.height));
        let said: Vec<&String> = path.iter().rev().filter(|p| p.contains('#') || p.contains('"')).take(3).collect();
        let mut described = own.clone();
        for p in said {
            described.push_str(" < ");
            described.push_str(p);
        }
        out.insert(line.number, described);
        path.push(own);
    }
    out
}

#[test]
#[ignore = "a probe: release, --ignored --nocapture"]
fn stream_bench_profile() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("frames.jsonl");
    let frames = record::standard(1_700_000_000_000);
    let lines: Vec<String> = frames.iter().map(|f| f.to_string()).collect();
    std::fs::write(&path, lines.join("\n") + "\n").unwrap();
    let queued: Queued = Rc::default();
    let q = queued.clone();
    let every = Duration::from_millis(25);
    set(Bench::new(
        path,
        every,
        // (Long enough to measure the page at rest first.)
        Duration::from_millis(7_000),
        Rc::new(move |work| q.borrow_mut().push_back(work)),
    ));
    // `PROBE_TRACE=1`: what ran each frame, by name (devtools' trace; it
    // costs, so the CPU it reports then is not the page's alone).
    let tracing = std::env::var_os("PROBE_TRACE").is_some();
    if tracing {
        zgx::profile::set_recording(true);
        zgx::trace::set_enabled(true);
        zgx::trace::set_kept(4);
    }
    let made: Rc<RefCell<Option<Rc<View>>>> = Rc::default();
    let slot = made.clone();
    let mut app = TestApp::new(|runtime| {
        let host = ScriptHost::new(
            runtime,
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("ui/stream_bench_fixture.luau"),
        );
        *slot.borrow_mut() = Some(window_view(&host));
        host
    });
    let view = made.borrow().clone().unwrap();
    let (w, h) = (1114u32, 685u32);
    app.ui.scene.borrow_mut().resize(w as f32, h as f32);
    // `PROBE_SCALE=2`: drawn at a Retina window's backing scale.
    let scale: u32 = std::env::var("PROBE_SCALE").ok().and_then(|v| v.parse().ok()).unwrap_or(1);
    let mut gpu = zgx_gpu::GpuRenderer::new(w * scale, h * scale).expect("a GPU");
    gpu.set_scale_factor(scale as f32);
    let pump = |app: &TestApp| {
        while let Some(work) = queued.borrow_mut().pop_front() {
            app.executor.borrow_mut().spawn(work);
        }
    };
    let frame = Duration::from_micros(8_333);
    let mut draw = |app: &TestApp| {
        pump(app);
        app.frames(1);
        let damage = app.ui.scene.borrow_mut().flush().damage;
        gpu.render(&app.ui.scene.borrow(), &damage).expect("drawn");
    };
    // (Its id alone: the whole draft as JSON would cost more than the page.)
    let streaming = |view: &View| {
        view.draft
            .with_untracked(|d| d.field("id").map(text))
            .as_deref()
            == Some(record::REPLY)
    };
    // What ran in the frames drawn, by name: (count, µs); layout's nodes
    // measured afresh; frames that did anything the trace keeps.
    #[derive(Default)]
    struct Ran {
        runs: std::collections::BTreeMap<String, (u32, f64)>,
        measured: Vec<u32>,
        traced: u32,
        notes: std::collections::BTreeMap<String, u32>,
        /// Bindings by their node, unfolded.
        bound: std::collections::BTreeMap<u32, (u32, f64)>,
        /// Bindings' first runs (as their node mounts) and runs again (a
        /// signal they read was written): (count, µs).
        first: (u32, f64),
        again: (u32, f64),
        /// Runs again by the signal written that caused them.
        causes: std::collections::BTreeMap<usize, (u32, f64)>,
    }
    impl Ran {
        fn add(&mut self, after: u64) {
            for f in zgx::trace::frames(4).iter().filter(|f| f.index > after) {
                self.traced += 1;
                self.measured.push(f.layout.measured);
                for run in &f.runs {
                    let name = run.name.as_deref().unwrap_or("(unnamed)");
                    if let Some(node) = name
                        .strip_suffix(" binding")
                        .and_then(|n| n.strip_prefix('@'))
                        .and_then(|n| n.parse::<u32>().ok())
                    {
                        let e = self.bound.entry(node).or_default();
                        e.0 += 1;
                        e.1 += run.micros as f64;
                        let (slot, us) = (if run.cause.is_none() { &mut self.first } else { &mut self.again }, run.micros as f64);
                        slot.0 += 1;
                        slot.1 += us;
                        if let Some(cause) = run.cause {
                            let c = self.causes.entry(cause).or_default();
                            c.0 += 1;
                            c.1 += us;
                        }
                    }
                    // (Node ids folded: one name for every row's.)
                    let name: String = name
                        .split(' ')
                        .map(|w| {
                            if w.len() > 1 && w.starts_with('@') && w[1..].chars().all(|c| c.is_ascii_digit()) {
                                "@n"
                            } else {
                                w
                            }
                        })
                        .collect::<Vec<_>>()
                        .join(" ");
                    let e = self.runs.entry(name).or_default();
                    e.0 += 1;
                    e.1 += run.micros as f64;
                }
                for note in &f.notes {
                    let note: String = note.chars().take(100).collect();
                    *self.notes.entry(note).or_default() += 1;
                }
            }
        }
        fn report(&self, name: &str, secs: f64, tree: &std::collections::HashMap<u32, String>) {
            let mut m = self.measured.clone();
            m.sort_unstable();
            eprintln!(
                "  {name}: {:.1} traced frames/s; nodes measured afresh median {} p90 {}",
                self.traced as f64 / secs,
                m.get(m.len() / 2).copied().unwrap_or(0),
                m.get(m.len() * 9 / 10).copied().unwrap_or(0),
            );
            let mut runs: Vec<_> = self.runs.iter().collect();
            runs.sort_by(|a, b| b.1.1.partial_cmp(&a.1.1).unwrap());
            for (run, (count, micros)) in runs.iter().take(25) {
                eprintln!("    {:>7.1}/s {:>8.1} µs/s  {run}", *count as f64 / secs, micros / secs);
            }
            eprintln!(
                "    bindings: first runs {:.1}/s {:.1} µs/s; runs again {:.1}/s {:.1} µs/s from {} signals",
                self.first.0 as f64 / secs,
                self.first.1 / secs,
                self.again.0 as f64 / secs,
                self.again.1 / secs,
                self.causes.len()
            );
            let mut causes: Vec<_> = self.causes.iter().collect();
            causes.sort_by(|a, b| b.1.1.partial_cmp(&a.1.1).unwrap());
            for (cause, (count, micros)) in causes.iter().take(8) {
                eprintln!("      signal {cause:#x}: {:.1}/s {:.1} µs/s", *count as f64 / secs, micros / secs);
            }
            let mut bound: Vec<_> = self.bound.iter().collect();
            bound.sort_by(|a, b| b.1.1.partial_cmp(&a.1.1).unwrap());
            for (node, (count, micros)) in bound.iter().take(12) {
                let line = tree
                    .get(node)
                    .map(|l| l.chars().take(220).collect::<String>())
                    .unwrap_or_else(|| format!("@{node} (gone)"));
                eprintln!("    binding {:>7.1}/s {:>8.1} µs/s  {line}", *count as f64 / secs, micros / secs);
            }
            for (note, count) in &self.notes {
                eprintln!("    note {:.1}/s: {note}", *count as f64 / secs);
            }
        }
    }
    // Frames on absolute deadlines, a window's 120 a second (a late one
    // doesn't push the next ones later).
    let paced = std::cell::Cell::new(std::time::Instant::now());
    let tick = |app: &TestApp, draw: &mut dyn FnMut(&TestApp), ran: &mut Ran| {
        let before = zgx::trace::frame_index();
        draw(app);
        if tracing {
            ran.add(before);
        }
        let next = (paced.get() + frame).max(std::time::Instant::now() - frame);
        paced.set(next);
        std::thread::sleep(next.saturating_duration_since(std::time::Instant::now()));
    };
    // The page at rest, the chat open and its reply not begun: from 2 s in
    // (once it has settled) for 4 s.
    let mut ran = Ran::default();
    let begun = std::time::Instant::now();
    while begun.elapsed() < Duration::from_secs(2) {
        tick(&app, &mut draw, &mut ran);
    }
    assert!(!streaming(&view), "the reply began before the page was measured at rest");
    let mut ran = Ran::default();
    let (cpu, at) = (Cpu::now(), std::time::Instant::now());
    let mut n = 0u32;
    while at.elapsed() < Duration::from_secs(4) {
        tick(&app, &mut draw, &mut ran);
        n += 1;
    }
    let (took, spent) = (at.elapsed(), cpu.since());
    eprintln!(
        "stream_bench_profile: at rest: {n} frames over {:.2} s: {}",
        took.as_secs_f64(),
        spent.describe(took)
    );
    if tracing {
        ran.report("at rest", took.as_secs_f64(), &full_tree(&app));
    }
    // Until the reply starts.
    let mut ran = Ran::default();
    while !streaming(&view) && begun.elapsed() < Duration::from_secs(15) {
        tick(&app, &mut draw, &mut ran);
    }
    assert!(streaming(&view), "the reply never started");
    let mut ran = Ran::default();
    let cpu = Cpu::now();
    let started = std::time::Instant::now();
    let mut n = 0u32;
    // (The tree mid-stream, to name the nodes bound then.)
    let mut mid_tree = std::collections::HashMap::new();
    // `ZGX_PROFILE=1`: each phase's time over the stream, against the
    // thread's CPU clock; what no phase counts is the rest.
    let profiling = zgx::profile::enabled();
    let mut phases: std::collections::BTreeMap<String, f64> = Default::default();
    zgx::profile::take();
    while streaming(&view) {
        tick(&app, &mut draw, &mut ran);
        n += 1;
        if tracing && n == 1500 {
            mid_tree = full_tree(&app);
        }
        if profiling {
            for (phase, _, time) in zgx::profile::take() {
                *phases.entry(format!("{phase:?}")).or_default() += time.as_secs_f64();
            }
        }
    }
    let took = started.elapsed();
    let spent = cpu.since();
    if profiling {
        let secs = took.as_secs_f64();
        let counted: f64 = phases.values().sum();
        let thread = spent.thread.as_secs_f64();
        eprintln!(
            "  phases: {} ; counted {:.1} ms/s of the thread's {:.1} ms/s CPU ({:.0}%); not in a phase {:.1} ms/s",
            phases
                .iter()
                .filter(|(_, t)| **t > 0.)
                .map(|(p, t)| format!("{p} {:.1}", t / secs * 1e3))
                .collect::<Vec<_>>()
                .join(", "),
            counted / secs * 1e3,
            thread / secs * 1e3,
            100. * counted / thread.max(1e-9),
            (thread - counted) / secs * 1e3,
        );
    }
    eprintln!(
        "stream_bench_profile: {n} frames over {:.2} s ({:.1}/s): {}",
        took.as_secs_f64(),
        n as f64 / took.as_secs_f64(),
        spent.describe(took)
    );
    if tracing {
        ran.report("streaming", took.as_secs_f64(), &mid_tree);
    }
    assert_eq!(app.error(), None);
}
