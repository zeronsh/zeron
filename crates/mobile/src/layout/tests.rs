use std::sync::Arc;
use std::time::Instant;

use super::*;

struct Quiet;
impl LayoutListener for Quiet {
    fn frame_ready(&self, _revision: u64) {}
}

/// Fixed-advance fallback so tests never depend on a platform engine.
struct FixedFallback;
impl PlatformMeasurer for FixedFallback {
    fn measure(&self, _face: FaceRole, size: f32, _ligatures: bool, text: String) -> f32 {
        text.chars().count() as f32 * size * 1.1
    }
    fn measure_run(&self, _face: FaceRole, size: f32, _ligatures: bool, text: String) -> Vec<f32> {
        text.chars().map(|_| size * 1.1).collect()
    }
}

fn font(name: &str) -> Vec<u8> {
    std::fs::read(format!("{}/../ui/assets/fonts/{name}.ttf", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

pub(crate) fn text_system() -> Arc<TextSystem> {
    let faces = [
        (FaceRole::Sans, "Geist"),
        (FaceRole::SansMedium, "Geist-Medium"),
        (FaceRole::SansSemibold, "Geist-SemiBold"),
        (FaceRole::SansBold, "Geist-Bold"),
        (FaceRole::SansItalic, "Geist-Italic"),
        (FaceRole::SansSemiboldItalic, "Geist-SemiBoldItalic"),
        (FaceRole::Mono, "GeistMono"),
    ]
    .into_iter()
    .map(|(role, name)| FaceData { role, bytes: font(name) })
    .collect();
    TextSystem::new(faces, Some(Arc::new(FixedFallback)))
}

fn worker(width: f32) -> Worker {
    let ts = text_system();
    let mut w = Worker::new(&ts, Arc::new(Shared { frame: Mutex::new(Arc::new(LayoutFrame::empty())) }), Arc::new(Quiet));
    w.width = width;
    w
}

pub(crate) const RICH: &str = FIXTURE;

fn transcript(turns: usize) -> TranscriptInput {
    let mut entries = Vec::new();
    for i in 0..turns {
        entries.push(DebugEntry {
            id: format!("u{i}"),
            user: true,
            text: format!("Question {i}: can you explain how the layout engine handles **wrapping** of long lines like /Users/wing/project/src/some/deeply/nested/module.rs?"),
            streaming: false,
        });
        entries.push(DebugEntry {
            id: format!("a{i}"),
            user: false,
            text: RICH.to_owned(),
            streaming: false,
        });
    }
    debug_input(entries, false)
}

#[test]
fn paint_matches_measure_at_many_widths() {
    for width in [280.0, 320.0, 375.0, 393.0, 430.0, 744.0, 1024.0] {
        let mut w = worker(width);
        w.input = transcript(2);
        let frame = w.pass();
        assert!(frame.row_count() > 10, "rows: {}", frame.row_count());
        let mut y = 0.0;
        for i in 0..frame.row_count() {
            let p = frame.placement(i).unwrap();
            assert!((p.y - y).abs() < 0.01, "offsets are a prefix sum");
            y += p.height;
            let d = frame.display(i).unwrap();
            assert!((d.height - p.height).abs() < 0.01);
            let units = d.text.encode_utf16().count() as u32;
            for run in &d.runs {
                assert!(run.start + run.len <= units, "run slices the row text");
                if run.scroller.is_none() {
                    assert!(run.x >= 0.0 && run.x + run.width <= width + 0.5, "run inside width {width}: {run:?}");
                }
                assert!(run.baseline > 0.0 && run.baseline <= d.height + 0.5 || run.scroller.is_some());
            }
        }
    }
}

#[test]
fn streaming_converges_to_full_parse() {
    let mut w = worker(390.0);
    let mut last = None;
    let chars: Vec<char> = RICH.chars().collect();
    let mut shown = String::new();
    for chunk in chars.chunks(7) {
        shown.extend(chunk);
        w.input = debug_input(
            vec![DebugEntry { id: "a".into(), user: false, text: shown.clone(), streaming: true }],
            true,
        );
        last = Some(w.pass());
    }
    let _ = last;
    w.input = debug_input(vec![DebugEntry { id: "a".into(), user: false, text: shown, streaming: false }], false);
    let streamed = w.pass();
    let mut fresh = worker(390.0);
    fresh.input = transcript_one(RICH);
    let full = fresh.pass();
    assert_eq!(streamed.row_count(), full.row_count());
    for i in 0..full.row_count() {
        let (a, b) = (streamed.placement(i).unwrap(), full.placement(i).unwrap());
        assert_eq!(a.key, b.key);
        assert!((a.height - b.height).abs() < 0.01, "row {i}: {} vs {}", a.height, b.height);
    }
}

fn transcript_one(text: &str) -> TranscriptInput {
    debug_input(vec![DebugEntry { id: "a".into(), user: false, text: text.into(), streaming: false }], false)
}

#[test]
fn stable_prefix_rows_are_reused_while_streaming() {
    let mut w = worker(390.0);
    let base = "# Title\n\nFirst paragraph.\n\nSecond paragraph that keeps growing";
    w.input = debug_input(vec![DebugEntry { id: "a".into(), user: false, text: base.into(), streaming: true }], true);
    let f1 = w.pass();
    w.input = debug_input(vec![DebugEntry { id: "a".into(), user: false, text: format!("{base} with more words"), streaming: true }], true);
    let f2 = w.pass();
    // Heading + first paragraph keep their versions; the tail changes.
    for i in 0..2 {
        assert_eq!(f1.placement(i).unwrap().version, f2.placement(i).unwrap().version);
    }
    assert_ne!(f1.placement(2).unwrap().version, f2.placement(2).unwrap().version);
}

#[test]
fn toggles_expand_tool_groups_and_long_user_messages() {
    let mut w = worker(390.0);
    let long = (0..40).map(|i| format!("line {i} of a long pasted prompt")).collect::<Vec<_>>().join("\n");
    w.input = debug_input(vec![DebugEntry { id: "u".into(), user: true, text: long, streaming: false }], false);
    let folded = w.pass();
    let key = folded.placement(0).unwrap().key;
    w.builder.expanded.insert(key);
    w.builder.invalidate(key);
    let open = w.pass();
    assert!(open.placement(0).unwrap().height > folded.placement(0).unwrap().height * 3.0);
}

/// Thinking renders desktop-style: markdown flattened to styled detail lines
/// (bold/lists/code/quotes, underlined non-clickable links), not literal
/// markers (desktop PR #220; the mobile port dropped it).
#[test]
fn thinking_renders_styled_markdown_not_markers() {
    use zeron_doc::parts::{MessagePart, MessageStatus};
    use zeron_doc::schema::{MessageRole, SessionMessageEntry};
    let reasoning = concat!(
        "**Planning** the `fix`\n\n",
        "- point *one*\n",
        "- point two with a [link](https://example.com)\n\n",
        "```rust\n",
        "let x = 1;\n",
        "```\n\n",
        "> quoted text",
    );
    let mut w = worker(390.0);
    w.input = TranscriptInput {
        entries: vec![Arc::new(SessionMessageEntry { origin: None,
            id: "a".into(),
            role: MessageRole::Assistant,
            parts: vec![MessagePart::Reasoning { id: "r0".into(), text: reasoning.into() }],
            created_at: 0,
            device_id: String::new(),
            status: Some(MessageStatus::Complete),
            continuation_of: None,
            duration_ms: None,
        })],
        ..Default::default()
    };
    let folded = w.pass();
    let key = folded.placement(0).unwrap().key;
    w.builder.expanded.insert(key);
    w.builder.detail_open.insert(rows::row_key("a#g0/r0"), true);
    w.builder.invalidate(key);
    let frame = w.pass();
    let d = frame.display(0).unwrap();
    assert!(!d.text.contains("**"), "{}", d.text);
    assert!(!d.text.contains("`fix`"), "{}", d.text);
    assert!(!d.text.contains("```"), "{}", d.text);
    assert!(d.text.contains("Planning"), "{}", d.text);
    assert!(d.text.contains("• point one"), "{}", d.text);
    assert!(d.text.contains("let x = 1;"), "{}", d.text);
    assert!(d.text.contains("│ quoted text"), "{}", d.text);
    assert!(d.runs.iter().any(|r| r.decoration == display::Decoration::Underline), "{:?}", d.runs);
    assert!(d.links.is_empty(), "thought links must not be clickable");
    assert!(d.runs.iter().any(|r| r.color == display::ColorRole::TextFaint));
    // Bold is a distinct face: "Planning" isn't painted in the regular style.
    let slice = |r: &display::TextRun| -> String {
        d.text.encode_utf16().skip(r.start as usize).take(r.len as usize).filter_map(|u| char::from_u32(u as u32)).collect()
    };
    let planning = d.runs.iter().find(|r| slice(r) == "Planning").expect("a Planning run");
    let regular = d.runs.iter().find(|r| slice(r) == " the ").expect("a regular run");
    assert_ne!(planning.style, regular.style, "bold uses the semibold face");
    // Paint/measure hold across widths (display() debug-asserts equality).
    for width in [280.0, 320.0, 430.0, 744.0, 1024.0] {
        let mut w = worker(width);
        w.input = TranscriptInput {
            entries: vec![Arc::new(SessionMessageEntry { origin: None,
                id: "a".into(),
                role: MessageRole::Assistant,
                parts: vec![MessagePart::Reasoning { id: "r0".into(), text: reasoning.into() }],
                created_at: 0,
                device_id: String::new(),
                status: Some(MessageStatus::Complete),
                continuation_of: None,
                duration_ms: None,
            })],
            ..Default::default()
        };
        let key = w.pass().placement(0).unwrap().key;
        w.builder.expanded.insert(key);
        w.builder.detail_open.insert(rows::row_key("a#g0/r0"), true);
        w.builder.invalidate(key);
        let frame = w.pass();
        let d = frame.display(0).unwrap();
        assert!(d.text.contains("• point one"), "width {width}: {}", d.text);
        for run in &d.runs {
            assert!(run.x >= -0.5 && run.x + run.width <= width + 0.5, "width {width}: {run:?}");
        }
    }
}

/// A thought streamed through the incremental parser (live display mend)
/// settles to exactly the frame a fresh full parse lays out.
#[test]
fn streaming_thought_markdown_settles_to_the_fresh_parse() {
    use zeron_doc::parts::{MessagePart, MessageStatus};
    use zeron_doc::schema::{MessageRole, SessionMessageEntry};
    let text = "**Checking** the `parser`\n\n1. first step\n2. second with [docs](https://example.com)\n\n> note\n\n```rust\nfn main() {}\n```";
    let entry = |status: MessageStatus, text: &str| {
        Arc::new(SessionMessageEntry { origin: None,
            id: "a".into(),
            role: MessageRole::Assistant,
            parts: vec![MessagePart::Reasoning { id: "r0".into(), text: text.to_owned() }],
            created_at: 0,
            device_id: String::new(),
            status: Some(status),
            continuation_of: None,
            duration_ms: None,
        })
    };
    let mut live = worker(390.0);
    let mut shown = String::new();
    let mut key = 0;
    for chunk in text.chars().collect::<Vec<_>>().chunks(5) {
        shown.extend(chunk);
        live.input = TranscriptInput { entries: vec![entry(MessageStatus::Streaming, &shown)], ..Default::default() };
        let frame = live.pass();
        key = frame.placement(0).unwrap().key;
        assert!(!frame.display(0).unwrap().text.contains("**"), "mend holds while streaming");
    }
    live.builder.expanded.insert(key);
    live.builder.detail_open.insert(rows::row_key("a#g0/r0"), true);
    live.builder.invalidate(key);
    live.input = TranscriptInput { entries: vec![entry(MessageStatus::Complete, text)], ..Default::default() };
    let streamed = live.pass();

    let mut fresh = worker(390.0);
    fresh.input = TranscriptInput { entries: vec![entry(MessageStatus::Complete, text)], ..Default::default() };
    let folded = fresh.pass();
    let fresh_key = folded.placement(0).unwrap().key;
    assert_eq!(key, fresh_key);
    fresh.builder.expanded.insert(fresh_key);
    fresh.builder.detail_open.insert(rows::row_key("a#g0/r0"), true);
    fresh.builder.invalidate(fresh_key);
    let settled = fresh.pass();

    assert_eq!(streamed.row_count(), settled.row_count());
    for i in 0..settled.row_count() {
        let a = streamed.display(i).unwrap();
        let b = settled.display(i).unwrap();
        assert_eq!(a.text, b.text, "row {i} text");
        assert!((a.height - b.height).abs() < 0.01, "row {i} height");
    }
}

fn thought_input(text: &str, streaming: bool) -> TranscriptInput {
    use zeron_doc::parts::{MessagePart, MessageStatus};
    use zeron_doc::schema::{MessageRole, SessionMessageEntry};
    TranscriptInput {
        entries: vec![Arc::new(SessionMessageEntry { origin: None,
            id: "a".into(),
            role: MessageRole::Assistant,
            parts: vec![MessagePart::Reasoning { id: "r0".into(), text: text.to_owned() }],
            created_at: 0,
            device_id: String::new(),
            status: Some(if streaming { MessageStatus::Streaming } else { MessageStatus::Complete }),
            continuation_of: None,
            duration_ms: None,
        })],
        ..Default::default()
    }
}

/// A settled one-thought reply with its group and thought detail opened.
fn open_thought(width: f32, text: &str) -> RowDisplay {
    let mut w = worker(width);
    w.input = thought_input(text, false);
    let key = w.pass().placement(0).unwrap().key;
    w.builder.expanded.insert(key);
    w.builder.detail_open.insert(rows::row_key("a#g0/r0"), true);
    w.builder.invalidate(key);
    w.pass().display(0).unwrap()
}

fn run_text(d: &RowDisplay, r: &display::TextRun) -> String {
    String::from_utf16_lossy(&d.text.encode_utf16().skip(r.start as usize).take(r.len as usize).collect::<Vec<_>>())
}

/// Code keeps its indentation; nested items step right; a wrapped list item
/// hangs under its first word and a wrapped quote keeps its bar on every
/// line (desktop's re-indented wrap), at every width.
#[test]
fn thought_code_indents_and_wrapped_lines_hang() {
    let alpha = "alpha ".repeat(20);
    let omega = "omega ".repeat(20);
    let text = format!("```\nfn f() {{\n    let x = 1;\n}}\n```\n\n- top\n  - nested\n- {alpha}\n\n> {omega}");
    for width in [280.0, 320.0, 390.0] {
        let d = open_thought(width, &text);
        assert!(d.text.contains("    let x = 1;"), "code indentation survives: {}", d.text);
        let x_of = |needle: &str| d.runs.iter().find(|r| run_text(&d, r) == needle).unwrap_or_else(|| panic!("{needle}: {:?}", d.runs)).x;
        let top = x_of("top");
        assert!(x_of("nested") > top + 1.0, "nested item indents");
        let lines = |word: &str| {
            let runs: Vec<_> = d.runs.iter().filter(|r| run_text(&d, r).contains(word)).collect();
            let mut baselines: Vec<f32> = runs.iter().map(|r| r.baseline).collect();
            baselines.dedup_by(|a, b| (*a - *b).abs() < 0.5);
            (runs, baselines.len())
        };
        let (alpha_runs, alpha_lines) = lines("alpha");
        assert!(alpha_lines > 1, "width {width}: the long item wraps");
        for r in &alpha_runs {
            assert!((r.x - top).abs() < 0.5, "width {width}: wrapped item hangs under its text: {r:?}");
        }
        let (omega_runs, omega_lines) = lines("omega");
        assert!(omega_lines > 1, "width {width}: the long quote wraps");
        let bar_x = omega_runs[0].x;
        for r in &omega_runs {
            assert!((r.x - bar_x).abs() < 0.5, "width {width}: wrapped quote hangs: {r:?}");
        }
        let bars = d.runs.iter().filter(|r| run_text(&d, r).contains('│')).count();
        assert_eq!(bars, omega_lines, "width {width}: one bar per quoted line");
        for run in &d.runs {
            assert!(run.x >= -0.5 && run.x + run.width <= width + 0.5, "width {width}: {run:?}");
        }
    }
}

/// A long thought streaming past the visible cap stops growing, fades its
/// last line, and stops re-preparing: deltas below the fold reuse the body.
#[test]
fn long_streaming_thought_is_capped_and_reuses_its_body() {
    let mut w = worker(390.0);
    let mut text = String::new();
    let mut heights = Vec::new();
    let mut bodies = Vec::new();
    for i in 0..80 {
        text.push_str(&format!("Step {i}: **check** the `thing` and keep going.\n\n"));
        // Streaming: the group auto-expands and the tail thought auto-opens.
        w.input = thought_input(&text, true);
        let frame = w.pass();
        let d = frame.display(0).unwrap();
        assert!(!d.text.contains("**"));
        heights.push(d.height);
        bodies.push(w.builder.thought_body_for_test("a#r0").expect("thought prepared"));
        if i == 79 {
            assert!(d.fades.iter().any(|f| f.edge == display::FadeEdge::Bottom), "cut thought fades");
        }
    }
    let settled = heights[heights.len() - 1];
    assert!(heights[..5].windows(2).all(|p| p[1] > p[0]), "grows while short: {heights:?}");
    assert!(heights[40..].iter().all(|h| (h - settled).abs() < 0.01), "capped: {heights:?}");
    assert!(bodies[40..].windows(2).all(|p| Arc::ptr_eq(&p[0], &p[1])), "past the cap, deltas reuse the prepared body");
}

/// One huge paragraph (no line breaks for the line cap to act on) streaming
/// in: the visible text is clipped, so the body stops re-preparing too.
#[test]
fn huge_single_paragraph_thought_stops_reshaping() {
    let mut w = worker(390.0);
    let mut text = String::new();
    let mut heights = Vec::new();
    let mut bodies = Vec::new();
    for _ in 0..120 {
        text.push_str(&"streaming thought words ".repeat(10));
        w.input = thought_input(&text, true);
        let frame = w.pass();
        heights.push(frame.display(0).unwrap().height);
        bodies.push(w.builder.thought_body_for_test("a#r0").expect("thought prepared"));
    }
    assert!(text.len() > 25_000);
    let settled = heights[heights.len() - 1];
    assert!(heights[80..].iter().all(|h| (h - settled).abs() < 0.01), "capped: {heights:?}");
    assert!(bodies[80..].windows(2).all(|p| Arc::ptr_eq(&p[0], &p[1])), "past the budget, deltas reuse the prepared body");
}

/// Release-mode timings (run with `cargo test --release -p zeron-mobile -- --ignored --nocapture`).
#[test]
#[ignore]
fn bench_layout_passes() {
    let mut w = worker(393.0);
    w.input = transcript(300);
    let t = Instant::now();
    let frame = w.pass();
    let cold = t.elapsed();
    let t = Instant::now();
    w.width = 430.0;
    w.pass();
    let resize = t.elapsed();
    let t = Instant::now();
    for i in 0..frame.row_count().min(2000) {
        frame.display(i);
    }
    let display = t.elapsed() / frame.row_count().min(2000);
    // Streaming: append ~6 chars per update to a live tail after 600 entries.
    let mut entries = transcript(300).entries;
    let mut text = String::new();
    let mut total = std::time::Duration::ZERO;
    let updates = 400;
    for i in 0..updates {
        text.push_str(["word ", "more ", "text\n\n", "`code` ", "**bold** "][i % 5]);
        let mut e = entries.clone();
        e.push(Arc::new(SessionMessageEntry { origin: None,
            id: "live".into(),
            role: MessageRole::Assistant,
            parts: vec![MessagePart::Text { id: "t0".into(), text: text.clone() }],
            created_at: 0,
            device_id: String::new(),
            status: Some(MessageStatus::Streaming),
            continuation_of: None,
            duration_ms: None,
        }));
        w.input = TranscriptInput { entries: e, pending: vec![], working: true, working_since_ms: None, streaming: true };
        let t = Instant::now();
        w.pass();
        total += t.elapsed();
    }
    entries.clear();
    println!(
        "rows={} heap={:.1}MB cold={:?} resize={:?} display/row={:?} stream/update={:?}",
        frame.row_count(),
        frame.prepared_heap_bytes() as f64 / 1_048_576.0,
        cold,
        resize,
        display,
        total / updates as u32
    );
}

#[test]
fn user_mentions_render_as_accent_chips() {
    let mut w = worker(390.0);
    let text = "Look at [mod.rs](zeron-file:crates/mobile/src/layout/mod.rs) please".to_owned();
    w.input = debug_input(vec![DebugEntry { id: "u".into(), user: true, text, streaming: false }], false);
    let frame = w.pass();
    let d = frame.display(0).unwrap();
    assert!(d.text.contains("@mod.rs"), "{}", d.text);
    assert!(!d.text.contains("zeron-file:"));
    assert!(d.runs.iter().any(|r| r.color == display::ColorRole::Link));
}

#[test]
fn user_attachments_render_as_images_not_trailer_text() {
    let mut w = worker(390.0);
    let text = "Fix the header spacing\n\nAttached images (local files — open them to view):\n- /tmp/uploads/a/shot.png".to_owned();
    w.input = debug_input(vec![DebugEntry { id: "u".into(), user: true, text, streaming: false }], false);
    let frame = w.pass();
    let d = frame.display(0).unwrap();
    assert!(!d.text.contains("Attached images"), "{}", d.text);
    assert!(d.text.contains("Fix the header spacing"));
    assert!(d.widgets.iter().any(|w| matches!(&w.kind, display::WidgetKind::Image { reference } if reference.ends_with("shot.png"))));
}

#[test]
fn folded_user_message_fades_its_last_line() {
    let mut w = worker(390.0);
    let long = (0..40).map(|i| format!("line {i} of a long pasted prompt")).collect::<Vec<_>>().join("\n");
    w.input = debug_input(vec![DebugEntry { id: "u".into(), user: true, text: long, streaming: false }], false);
    let frame = w.pass();
    let d = frame.display(0).unwrap();
    let fade = d.fades.iter().find(|f| f.edge == display::FadeEdge::Bottom).expect("bottom fade");
    // Exactly one shown line sits in the fade band (the "Show more" label is below it).
    let in_band: Vec<f32> = d.runs.iter().map(|r| r.baseline).filter(|b| *b > fade.y && *b <= fade.y + fade.h).collect();
    assert!(!in_band.is_empty(), "fade covers the last shown line");
    assert!(in_band.iter().all(|b| (b - in_band[0]).abs() < 0.5));
}

#[test]
fn running_subagent_shows_a_spinner_after_its_spawn_resolves() {
    use zeron_doc::parts::{MessagePart, MessageStatus, SubagentStatus};
    use zeron_doc::schema::{MessageRole, SessionMessageEntry};
    let spawn = |status: SubagentStatus| MessagePart::Tool {
        id: "k1".into(),
        call: zeron_proto::ToolCall::Unknown { name: "Agent: scan the repo".into(), input: None },
        is_error: false,
        // Eager-done: the spawn call resolved while the subagent still runs.
        resolved: true,
        output: None,
        diff: None,
        output_ref: None,
        output_bytes: None,
        diff_ref: None,
        diff_stats: None,
        subagent_ref: Some("sub-1".into()),
        subagent_status: Some(status),
        subagent_tail: None,
    };
    let frame_for = |status: SubagentStatus| {
        let mut w = worker(390.0);
        w.input = TranscriptInput {
            entries: vec![Arc::new(SessionMessageEntry { origin: None,
                id: "a".into(),
                role: MessageRole::Assistant,
                parts: vec![spawn(status)],
                created_at: 0,
                device_id: String::new(),
                status: Some(MessageStatus::Complete),
                continuation_of: None,
                duration_ms: None,
            })],
            ..Default::default()
        };
        w.pass()
    };
    let spinners = |f: &LayoutFrame| f.display(0).unwrap().widgets.iter().filter(|w| matches!(w.kind, display::WidgetKind::Spinner)).count();
    assert_eq!(spinners(&frame_for(SubagentStatus::Running)), 1, "running subagent spins");
    assert_eq!(spinners(&frame_for(SubagentStatus::Done)), 0, "finished subagent is quiet");
    let failed = frame_for(SubagentStatus::Failed);
    assert!(failed.display(0).unwrap().runs.iter().any(|r| r.color == display::ColorRole::Danger), "failed subagent is tinted danger");
}

#[test]
fn links_get_hit_regions() {
    for md in [
        "See [the docs](https://example.com/docs) for details.",
        "1. first\n2. second\n3. third [link](https://ja.wikipedia.org/wiki/x)",
        "- [UIScrollView docs](https://developer.apple.com/documentation/uikit/uiscrollview)",
        "> quoted [link](https://example.com/q)",
    ] {
        let mut w = worker(390.0);
        w.input = transcript_one(md);
        w.pass();
        let frame = w.shared.frame.lock().unwrap().clone();
        let links: Vec<_> = (0..frame.row_count()).flat_map(|i| frame.display(i).unwrap().links).collect();
        assert!(!links.is_empty(), "no link hits for {md:?}");
    }
}
