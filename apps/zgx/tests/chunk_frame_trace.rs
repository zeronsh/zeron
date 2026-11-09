//! What a streaming reply's frames do, as devtools trace them (`zgx inspect
//! frames`): for frames a chunk lands on and frames where it doesn't, what
//! ran (by name, its time), which nodes layout measured afresh, what was
//! damaged, and the engine's notes. A diagnostic, not a check:
//!
//!     cargo test --release --test chunk_frame_trace -- --ignored --nocapture

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use zgx_script::{ScriptHost, Value, testing::TestApp};

fn ui_dir() -> PathBuf {
    std::env::var_os("ZERON_ZGX_TEST_UI")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui"))
}

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
struct Group {
    /// Node lines as the tree had them when they were seen.
    lines: BTreeMap<u32, String>,
    frames: usize,
    runs: BTreeMap<String, (u32, f64)>,
    measured: Vec<u32>,
    nodes: BTreeMap<u32, (u32, f64)>,
    notes: BTreeMap<String, u32>,
    damage: Vec<f32>,
    phases: BTreeMap<&'static str, f64>,
}
impl Group {
    fn resolve(&mut self, frame: &zgx::trace::Frame, tree: &str) {
        let mut want: Vec<u32> = frame.layout.top.iter().map(|m| m.node).collect();
        for run in &frame.runs {
            if let Some(n) = run.name.as_deref().and_then(|r| r.strip_prefix('@')).and_then(|r| r.split(' ').next()).and_then(|n| n.parse().ok()) {
                want.push(n);
            }
        }
        for node in want {
            if self.lines.contains_key(&node) {
                continue;
            }
            if let Some(line) = tree.lines().find(|l| l.trim_start().starts_with(&format!("@{node} "))) {
                self.lines.insert(node, line.trim().chars().take(100).collect());
            }
        }
    }
    fn add(&mut self, frame: &zgx::trace::Frame, damaged: f32) {
        self.frames += 1;
        self.damage.push(damaged);
        for run in &frame.runs {
            let name = run.name.as_deref().unwrap_or("(unnamed)").to_string();
            let e = self.runs.entry(name).or_default();
            e.0 += 1;
            e.1 += run.micros as f64;
        }
        self.measured.push(frame.layout.measured);
        for m in &frame.layout.top {
            let e = self.nodes.entry(m.node).or_default();
            e.0 += 1;
            e.1 += m.self_micros as f64;
        }
        for note in &frame.notes {
            *self.notes.entry(note.to_string()).or_default() += 1;
        }
        for (phase, _, micros) in &frame.phases {
            *self.phases.entry(phase).or_default() += *micros as f64;
        }
    }
    fn report(&self, name: &str, tree: &str) {
        let n = self.frames.max(1) as f64;
        let mut measured = self.measured.clone();
        measured.sort_unstable();
        let mut damage = self.damage.clone();
        damage.sort_by(|a, b| a.partial_cmp(b).unwrap());
        eprintln!(
            "== {name}: {} frames; nodes measured afresh median {}; damage median {:.0} px²",
            self.frames,
            measured.get(measured.len() / 2).copied().unwrap_or(0),
            damage.get(damage.len() / 2).copied().unwrap_or(0.),
        );
        eprintln!("  phases (mean µs/frame): {}", self.phases.iter().map(|(p, t)| format!("{p} {:.1}", t / n)).collect::<Vec<_>>().join(", "));
        let mut runs: Vec<_> = self.runs.iter().collect();
        runs.sort_by(|a, b| b.1.1.partial_cmp(&a.1.1).unwrap());
        eprintln!("  runs (per frame: count, µs):");
        let source = |name: &str| -> String {
            let Some(node) = name.strip_prefix('@').and_then(|r| r.split(' ').next()).and_then(|n| n.parse::<u32>().ok()) else {
                return String::new();
            };
            self.lines.get(&node).cloned().unwrap_or_default()
        };
        for (name, (count, micros)) in runs.iter().take(18) {
            eprintln!("    {:>6.2}x {:>8.1} µs  {name}  {}", *count as f64 / n, micros / n, source(name));
        }
        let mut nodes: Vec<_> = self.nodes.iter().collect();
        nodes.sort_by(|a, b| b.1.1.partial_cmp(&a.1.1).unwrap());
        eprintln!("  costliest measured nodes (per frame µs own, in how many frames):");
        for (node, (count, micros)) in nodes.iter().take(10) {
            let line = self.lines.get(node).cloned().unwrap_or_else(|| {
                tree.lines()
                    .find(|l| l.trim_start().starts_with(&format!("@{node} ")))
                    .unwrap_or("?")
                    .trim()
                    .chars()
                    .take(110)
                    .collect()
            });
            eprintln!("    {:>7.1} µs in {count:>4}  {line}", micros / n);
        }
        let mut notes: Vec<_> = self.notes.iter().collect();
        notes.sort_by(|a, b| b.1.cmp(a.1));
        for (note, count) in notes.iter().take(8) {
            let note: String = note.chars().take(160).collect();
            eprintln!("  note x{count}: {note}");
        }
    }
}

#[test]
#[ignore = "a diagnostic: release, --ignored --nocapture"]
fn what_a_streaming_replys_frames_do() {
    let text = reply(80);
    // (On before anything mounts: effects are named as they're made.)
    zgx::profile::set_recording(true);
    zgx::trace::set_enabled(true);
    let mut app = TestApp::new(|runtime| ScriptHost::new(runtime, ui_dir().join("chat_fixture.luau")));
    app.ui.scene.borrow_mut().resize(900., 700.);
    app.frames(2);
    assert!(app.host.run_command("go_s2"));
    app.frames(30);
    std::thread::sleep(Duration::from_millis(320));
    assert!(app.host.run_command("start_block_reply"));
    app.frames(5);
    let damaged = |app: &TestApp| -> f32 {
        app.ui.scene.borrow_mut().flush().damage.iter().map(|r| r.width * r.height).sum()
    };
    damaged(&app);
    let draft = app.host.shared("chat.draft", Value::Nil);
    zgx::trace::set_kept(4);
    let (mut chunks, mut others) = (Group::default(), Group::default());
    let mut paragraphs = Group::default();
    // Each chunk frame's cost (µs of script, update and layout) by the kind
    // of block being written: the last block of what has come.
    let mut by_kind: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    let kind_at = |end: usize| -> String {
        let blocks = zgx_script::markdown::parse(&text[..end], &Default::default());
        blocks
            .last()
            .and_then(|b| b.field("t").map(|t| t.to_string()))
            .unwrap_or_default()
    };
    let frame = Duration::from_micros(8_333);
    let (mut end, mut n) = (0, 0);
    while end < 14_000 {
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
        let before = zgx::trace::frame_index();
        app.frames(1);
        let area = damaged(&app);
        if end > 8_000 {
            for f in zgx::trace::frames(4).iter().filter(|f| f.index > before) {
                if lands {
                    chunks.add(f, area);
                    let cost: f64 = f
                        .phases
                        .iter()
                        .filter(|(p, ..)| matches!(*p, "script" | "update" | "layout"))
                        .map(|(_, _, us)| *us as f64)
                        .sum();
                    let kind = kind_at(end);
                    if kind == "p" {
                        let tree = app.tree();
                        paragraphs.resolve(f, &tree);
                        paragraphs.add(f, area);
                    }
                    by_kind.entry(kind).or_default().push(cost);
                } else {
                    others.add(f, area)
                }
            }
            if zgx::trace::frames(4).iter().all(|f| f.index <= before) {
                // (A frame that did nothing the trace keeps.)
                others.frames += 1;
                others.damage.push(area);
            }
        }
        std::thread::sleep(frame.saturating_sub(started.elapsed()));
    }
    assert_eq!(app.error(), None);
    let tree = app.tree();
    chunks.report("frames a chunk lands on", &tree);
    eprintln!("== a chunk's script + update + layout by the block being written (median µs, frames):");
    for (kind, mut costs) in by_kind {
        costs.sort_by(|a, b| a.partial_cmp(b).unwrap());
        eprintln!("  {kind:>8}: {:>8.1} µs  ({} frames)", costs[costs.len() / 2], costs.len());
    }
    others.report("other frames", &tree);
    paragraphs.report("chunk frames writing a paragraph", &tree);
}
