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
    let mut w = Worker::new(&ts, Arc::new(Shared { frame: Mutex::new(Arc::new(LayoutFrame::empty())) }), Arc::new(Quiet), None);
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
        entries: vec![Arc::new(SessionMessageEntry {
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
            entries: vec![Arc::new(SessionMessageEntry {
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
        Arc::new(SessionMessageEntry {
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
        entries: vec![Arc::new(SessionMessageEntry {
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
        e.push(Arc::new(SessionMessageEntry {
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
fn user_mentions_render_as_chips() {
    // The desktop's chips: a soft pill with the file theme's icon on a well,
    // the label in the body color — for files, folders, skills and commands.
    let mut w = worker(390.0);
    let skill = zeron_proto::invocation::Invocation::Skill { name: "review".into(), path: "/repo/SKILL.md".into(), command: None };
    let text = format!(
        "Look at [mod.rs](zeron-file:crates/mobile/src/layout/mod.rs) in [layout](zeron-file:crates/mobile/src/layout/) with {}",
        skill.link()
    );
    w.input = debug_input(vec![DebugEntry { id: "u".into(), user: true, text, streaming: false }], false);
    let frame = w.pass();
    let d = frame.display(0).unwrap();
    assert!(d.text.contains("mod.rs") && !d.text.contains("@mod.rs"), "{}", d.text);
    assert!(!d.text.contains("zeron-"), "{}", d.text);
    assert!(d.runs.iter().all(|r| r.color != display::ColorRole::Link));
    let icons: Vec<&str> = d
        .widgets
        .iter()
        .filter_map(|w| match &w.kind {
            display::WidgetKind::Icon { name, .. } => Some(name.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(icons, ["fileicon-files-rust", "fileicon-folders-folder", "wand.and.stars"]);
    assert_eq!(d.boxes.iter().filter(|b| b.color == display::ColorRole::ToolBadge).count(), 3);
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
fn synced_file_attachments_render_as_chips_and_name_pills() {
    // A desktop send mixing an image and ZIPs (one still a queued-upload ref),
    // with composer chips in the prompt. Chipped attachments show as their
    // chip only; the image chip opens its upload; the unchipped ZIP keeps a
    // name pill and never reaches image loading.
    let mut w = worker(390.0);
    let text = "Compare [Image 1](zeron-image:1) with [notes.zip](zeron-attachment:2)\n\nAttached images (local files — open them to view):\n- /tmp/uploads/ab12cd34-Image_1.png\n- /tmp/uploads/ab12cd34-notes.zip\n- pending://up-3/logs.zip".to_owned();
    w.input = debug_input(vec![DebugEntry { id: "u".into(), user: true, text, streaming: false }], false);
    let frame = w.pass();
    let d = frame.display(0).unwrap();
    assert!(!d.widgets.iter().any(|w| matches!(&w.kind, display::WidgetKind::Image { .. })));
    let icons: Vec<&str> = d
        .widgets
        .iter()
        .filter_map(|w| match &w.kind {
            display::WidgetKind::Icon { name, .. } => Some(name.as_str()),
            _ => None,
        })
        .collect();
    // The unchipped ZIP's pill, then the bubble's two chips.
    assert_eq!(icons, ["fileicon-files-compressed", "photo", "fileicon-files-compressed"]);
    assert!(d.text.contains("Compare Image 1 with notes.zip"), "{}", d.text);
    assert!(d.text.contains("logs.zip") && !d.text.contains("ab12cd34-"), "{}", d.text);
    assert!(!d.text.contains("zeron-image") && !d.text.contains("zeron-attachment"), "{}", d.text);
    let links: Vec<&str> = d.links.iter().map(|l| l.url.as_str()).collect();
    assert_eq!(links, ["zeron-preview://image?ref=%2Ftmp%2Fuploads%2Fab12cd34-Image_1.png"]);
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
            entries: vec![Arc::new(SessionMessageEntry {
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

#[test]
fn visual_rows_take_the_height_their_worker_measures() {
    struct Ping(Mutex<mpsc::Sender<u64>>);
    impl LayoutListener for Ping {
        fn frame_ready(&self, revision: u64) {
            let _ = self.0.lock().unwrap().send(revision);
        }
    }
    let (tx, frames) = mpsc::channel();
    let view = TranscriptView::new(text_system(), Arc::new(Ping(Mutex::new(tx))));
    let script = r#"return ui.column { gap = 10, ui.text "Hello", ui.canvas { height = 200, draw = function(g) end } }"#;
    view.visuals.set_fetch(Arc::new(move |path: &str| (path == "up/v1-visual.luau").then(|| script.as_bytes().to_vec())), 2.0);
    view.set_viewport(390.0, 1.0);
    view.set_input(TranscriptInput {
        entries: vec![Arc::new(SessionMessageEntry {
            id: "a1".into(),
            role: MessageRole::Assistant,
            parts: vec![MessagePart::Visual { id: "v1".into(), path: "up/v1-visual.luau".into(), title: "Chart".into() }],
            created_at: 0,
            device_id: String::new(),
            status: Some(MessageStatus::Complete),
            continuation_of: None,
            duration_ms: None,
        })],
        ..Default::default()
    });
    // The loading line first, then the laid-out visual (text, gap, canvas).
    let deadline = Instant::now() + std::time::Duration::from_secs(10);
    let (frame, row) = loop {
        let _ = frames.recv_timeout(deadline.saturating_duration_since(Instant::now())).expect("a frame");
        let frame = view.frame();
        if let Some(row) = frame.placement(0).filter(|p| p.kind == RowKind::Visual && p.height > 200.0) {
            break (frame, row);
        }
    };
    let display = frame.display(row.index).unwrap();
    let widget = display.widgets.iter().find(|w| matches!(&w.kind, display::WidgetKind::Visual { id } if id == "v1")).expect("a visual widget");
    assert!((widget.y + widget.h - row.height).abs() < 0.5, "the widget fills the row below its gap");
    assert!(view.visual("v1".into()).is_some());
    view.close();
}

/// Every row of a one-message transcript at `width`.
fn rows_of(md: &str, width: f32) -> Vec<RowDisplay> {
    let mut w = worker(width);
    w.input = transcript_one(md);
    let frame = w.pass();
    (0..frame.row_count()).map(|i| frame.display(i).unwrap()).collect()
}

/// Inline math paints as glyphs on the text's baseline, never as TeX, and
/// a tall formula grows its line.
#[test]
fn inline_math_paints_glyphs_and_grows_its_line() {
    let plain = rows_of("Plain text on one line.", 390.0).remove(0);
    let d = rows_of(r"A fraction $\frac{a^2}{b_k}$ on one line.", 390.0).remove(0);
    assert!(!d.glyphs.is_empty(), "formula glyphs");
    assert!(!d.text.contains(r"\frac") && !d.text.contains('$'), "{}", d.text);
    assert!(d.runs.iter().all(|r| !run_text(&d, r).contains('\u{FFFC}')), "the placeholder is never text");
    assert!(d.boxes.iter().any(|b| b.color == display::ColorRole::Text && b.radius == 0.0), "the fraction bar is a fill");
    assert!(d.height > plain.height + 1.0, "{} vs {}", d.height, plain.height);
    // Text and formula share one baseline.
    let baseline = d.runs[0].baseline;
    assert!(d.runs.iter().all(|r| (r.baseline - baseline).abs() < 0.01));
    let text_end = d.runs.iter().find(|r| run_text(&d, r).starts_with("A fraction")).map(|r| r.x + r.width).unwrap();
    let tail = d.runs.iter().find(|r| run_text(&d, r).contains("on one line")).unwrap();
    for g in &d.glyphs {
        assert!(g.x > text_end - 0.5 && g.x < tail.x, "glyph between the words: {g:?}");
        assert!(g.y > 0.0 && g.y <= d.height + 0.01 && g.scroller.is_none(), "{g:?} in {}", d.height);
        assert_eq!(g.color, display::ColorRole::Text);
    }
    // `start` points at the placeholder, for the streaming veil.
    let units: Vec<u16> = d.text.encode_utf16().collect();
    assert_eq!(units[d.glyphs[0].start as usize], 0xFFFC);
    assert_eq!(d.copy_text, r"A fraction $\frac{a^2}{b_k}$ on one line.");
}

/// Wrapped paragraphs with formulas measure exactly what they paint, and
/// every glyph stays inside its row — in every face at 100%, and in a sans and
/// a serif at 150%.
#[test]
fn math_paint_matches_measure_at_many_widths() {
    let md = concat!(
        r"Euler: $e^{i\pi} + 1 = 0$, sums $\sum_{k=1}^{n} k = \frac{n(n+1)}{2}$, roots $\sqrt[3]{x^3 + y^3}$ and ",
        r"matrices $\begin{pmatrix} a & b \\ c & d \end{pmatrix}$ all wrap with the prose around them, and ",
        r"[a link with $x^2$ inside](https://example.com) keeps its hit region.",
        "\n\n",
        r"- item $\frac{1}{2}$ first",
        "\n\n",
        "| a | b |\n|---|---|\n| $\\frac{x}{y}$ | $z_i^2$ |\n\n",
        r"$$\sum_{n=1}^{\infty} \frac{1}{n^2} = \frac{\pi^2}{6}$$",
        "\n\n",
        r"Too wide: $a_1 + a_2 + a_3 + a_4 + a_5 + a_6 + a_7 + a_8 + a_9 + a_{10} + a_{11} + a_{12} + a_{13} + a_{14}$ shrinks.",
        "\n\n",
        "| f | next |\n|---|---|\n| $a_1 + a_2 + a_3 + a_4 + a_5 + a_6 + a_7 + a_8 + a_9 + a_{10} + a_{11} + a_{12} + a_{13} + a_{14} + a_{15}$ | n |\n\n",
        "- ## Head $\\frac{a^2}{b^2}$ tall\n\n",
        r"~~struck $\frac{x}{y}$ math~~",
    );
    let settings = zatex::bundled::Face::ALL.map(|f| (f, 100)).into_iter().chain([(zatex::bundled::Face::Fira, 150), (zatex::bundled::Face::ComputerModern, 150)]);
    for ((face, size), width) in settings.flat_map(|f| [120.0, 200.0, 280.0, 320.0, 390.0, 744.0, 1024.0].map(|w| (f, w))) {
        let mut w = worker(width);
        w.math = MathSettings::new(true, face, size);
        w.input = transcript_one(md);
        let frame = w.pass();
        let mut glyphs = 0;
        for i in 0..frame.row_count() {
            let p = frame.placement(i).unwrap();
            let d = frame.display(i).unwrap();
            assert!((d.height - p.height).abs() < 0.01);
            for g in &d.glyphs {
                assert!(g.y > 0.0 && g.y < d.height + 0.5, "{face:?} width {width}: {g:?} in {}", d.height);
                match g.scroller {
                    Some(s) => {
                        let s = &d.scrollers[s as usize];
                        assert!(g.x >= -0.5 && g.x <= s.content_width + 0.5, "{face:?} width {width}: {g:?}");
                    }
                    None => assert!(g.x >= 0.0 && g.x <= width, "{face:?} width {width}: {g:?}"),
                }
            }
            glyphs += d.glyphs.len();
        }
        assert!(glyphs > 50, "{face:?} width {width}: {glyphs} glyphs");
    }
}

/// `$$…$$` is a centered block padded above and below; one wider than the
/// column scrolls.
#[test]
fn display_math_centers_and_scrolls_when_wide() {
    let tex = r"\int_0^1 x^2\,dx = \frac{1}{3}";
    let d = rows_of(&format!("$${tex}$$"), 390.0).remove(0);
    let f = zatex::bundled::layout(tex, zatex::bundled::Face::Fira, &zatex::Options::display(style::TYPE.body.0)).unwrap();
    let gap = rows::Gap::First.px(Px(1.0));
    let pad = markdown::geom::MATH_PAD_Y;
    assert!((d.height - (gap + f.height() + pad * 2.0)).abs() < 0.01, "{} vs {}", d.height, f.height());
    assert!(d.scrollers.is_empty() && d.runs.is_empty());
    let left = d.glyphs.iter().map(|g| g.x).fold(f32::INFINITY, f32::min);
    let column = rows::column_width(Px(1.0), 390.0);
    let x0 = (390.0 - column) / 2.0;
    assert!((left - (x0 + (column - f.width) / 2.0)).abs() < 1.0, "centered: {left}");
    assert_eq!(d.copy_text, format!("$${tex}$$"));

    let wide = format!("$${}$$", r"a_1 + a_2 + a_3 + a_4 + a_5 + a_6 + a_7 + a_8 + a_9 + a_{10} + a_{11} + a_{12} + a_{13}");
    let d = rows_of(&wide, 320.0).remove(0);
    assert_eq!(d.scrollers.len(), 1);
    assert!(d.scrollers[0].content_width > d.scrollers[0].w);
    assert!(d.glyphs.iter().all(|g| g.scroller == Some(0)));
}

/// TeX zatex can't lay out keeps its source: inline as a code chip with its
/// dollars, display as a `math` code block.
#[test]
fn invalid_math_falls_back_to_source() {
    let d = rows_of(r"Unknown $\foo{x}$ here.", 390.0).remove(0);
    assert!(d.glyphs.is_empty());
    assert!(d.text.contains(r"$\foo{x}$"), "{}", d.text);
    let chip = d.runs.iter().find(|r| run_text(&d, r).contains("foo")).unwrap();
    assert_eq!(chip.color, display::ColorRole::InlineCodeText);
    assert!(d.boxes.iter().any(|b| b.color == display::ColorRole::InlineCodeBackground));

    // (`$$\frac{a$$` never reaches layout: the parser keeps unbalanced
    // braces as text.)
    let d = rows_of(r"$$\frac{a}$$", 390.0).remove(0);
    assert!(d.glyphs.is_empty());
    assert!(d.text.contains(r"\frac{a}"), "{}", d.text);
    assert!(d.text.contains("math"), "labelled a math block: {}", d.text);
    assert!(d.boxes.iter().any(|b| b.color == display::ColorRole::CodeBackground));
}

/// A runaway formula (the reviewer's `\\`×8000: thousands of empty lines)
/// is rejected by its extent and shows as source — no taller than that
/// source as inline code (~10,000pt at 390pt), not a ~198,000pt canvas; as
/// display math it's a one-line `math` code block.
#[test]
fn runaway_math_falls_back_to_a_bounded_row() {
    let tex = r"\\".repeat(8000);
    let height = |md: &str| {
        let mut w = worker(390.0);
        w.input = transcript_one(md);
        let frame = w.pass();
        let d = frame.display(0).unwrap();
        assert!(d.glyphs.is_empty(), "rendered as source");
        assert!(d.text.contains(&tex[..64]), "source shown");
        assert!((frame.total - d.height) < 100.0, "one row");
        (d.height, w.math_failures.len())
    };
    let (inline, failures) = height(&format!("Hi ${tex}$"));
    assert_eq!(failures, 1);
    let (code, _) = height(&format!("Hi `{tex}`"));
    assert!(inline <= code + 1.0 && inline < 12_000.0, "inline {inline} vs code {code}");
    // Long display source wraps rather than becoming one huge scrolling line.
    let (display, failures) = height(&format!("$${tex}$$"));
    assert_eq!(failures, 1);
    assert!(display < 12_000.0, "display {display}");
    let d = rows_of(&format!("$${tex}$$"), 390.0).remove(0);
    assert!(d.scrollers.iter().all(|s| s.content_width < 5_000.0), "{:?}", d.scrollers.iter().map(|s| s.content_width).collect::<Vec<_>>());
    // Short failures keep the `math` code block (its label and bare source).
    let d = rows_of(r"$$\foo{x}$$", 390.0).remove(0);
    assert!(d.text.contains("math") && d.text.contains(r"\foo{x}") && !d.text.contains("$$"), "{}", d.text);
}

/// A formula zatex rejected is remembered: preparing its row again skips
/// layout; the memory stays bounded at `CAP` keys, oldest out first.
#[test]
fn failed_formulas_are_cached_and_bounded() {
    let md = r"Bad $\foo{x}$ and again $\foo{x}$, display: ";
    let mut w = worker(390.0);
    w.input = transcript_one(&format!("{md}\n\n$$\\foo{{x}}$$"));
    let first = w.pass().display(0).unwrap();
    // Inline and display are separate keys; the repeat is a hit.
    assert_eq!(w.math_failures.len(), 2);
    assert_eq!(w.math_failures.hits, 1);
    w.rebuild();
    let again = w.pass().display(0).unwrap();
    assert_eq!(w.math_failures.len(), 2);
    assert_eq!(w.math_failures.hits, 4, "all three formulas skipped layout");
    assert_eq!(first.text, again.text, "same fallback");
    // Another face or size is another key.
    assert!(w.set_math(MathSettings::new(true, zatex::bundled::Face::StixTwo, 100)));
    w.pass();
    assert_eq!(w.math_failures.len(), 4);
    // Bounded: past CAP the oldest are evicted.
    let many: String = (0..MathFailures::CAP + 50).map(|i| format!("$\\foo{{{i}}}$ ")).collect();
    w.input = transcript_one(&many);
    w.pass();
    assert_eq!(w.math_failures.len(), MathFailures::CAP);
    // Good formulas are never recorded.
    let mut ok = worker(390.0);
    ok.input = transcript_one(r"Fine $x^2$ here.");
    ok.pass();
    assert_eq!(ok.math_failures.len(), 0);
}

/// Half-streamed formulas fall back to their source and settle into the same
/// rows a fresh parse lays out.
#[test]
fn streaming_math_converges_to_full_parse() {
    let md = concat!(
        r"Inline $\frac{a}{b} + \sqrt{x}$ and $y^2$ in prose.",
        "\n\n",
        r"$$\sum_{k=1}^{n} k = \frac{n(n+1)}{2}$$",
        "\n\nAfter.",
    );
    let mut w = worker(390.0);
    let mut shown = String::new();
    for c in md.chars() {
        shown.push(c);
        w.input = debug_input(vec![DebugEntry { id: "a".into(), user: false, text: shown.clone(), streaming: true }], true);
        let frame = w.pass();
        for i in 0..frame.row_count() {
            frame.display(i).unwrap();
        }
    }
    w.input = transcript_one(md);
    let streamed = w.pass();
    let mut fresh = worker(390.0);
    fresh.input = transcript_one(md);
    let full = fresh.pass();
    assert_eq!(streamed.row_count(), full.row_count());
    for i in 0..full.row_count() {
        let (a, b) = (streamed.display(i).unwrap(), full.display(i).unwrap());
        assert_eq!(a.key, b.key);
        assert_eq!(a.text, b.text, "row {i}");
        assert_eq!(a.glyphs, b.glyphs, "row {i}");
        assert!((a.height - b.height).abs() < 0.01, "row {i}: {} vs {}", a.height, b.height);
    }
    assert!((0..full.row_count()).all(|i| !full.display(i).unwrap().glyphs.is_empty() || i == full.row_count() - 1));
}

#[test]
fn math_fonts_cross_ffi() {
    let fonts: Vec<Vec<u8>> = (0..6).map(display::math_font_data).collect();
    for (i, data) in fonts.iter().enumerate() {
        assert!(!data.is_empty(), "font {i}");
        assert!(fonts[..i].iter().all(|other| other != data), "font {i} is its own file");
    }
    assert!(display::math_font_data(6).is_empty());
    assert!(display::math_font_data(u8::MAX).is_empty());
}

/// Every FFI face lays formulas out in its own font (its table index), with
/// STIX Two (1) only for what it lacks; the order matches `math_font_data`.
#[test]
fn every_math_face_lays_out() {
    use display::MathFace;
    let faces = [MathFace::Fira, MathFace::StixTwo, MathFace::ComputerModern, MathFace::Libertinus, MathFace::Pagella, MathFace::Euler];
    let md = r"Sum $\sum_{k=1}^{n} \frac{x_k^2}{\sqrt{y}} + \mathcal{L} = \alpha$ here.";
    for (i, face) in faces.into_iter().enumerate() {
        let zface = zatex::bundled::Face::from(face);
        assert_eq!(zface.index() as usize, i, "{face:?}");
        let row = rows_with_math(md, MathSettings { face: zface, ..MathSettings::default() }).remove(0);
        assert!(row.glyphs.len() > 8, "{face:?}: {:?}", row.glyphs);
        assert!(row.glyphs.iter().all(|g| g.font == i as u8 || g.font == 1), "{face:?}: {:?}", row.glyphs);
        assert!(row.glyphs.iter().any(|g| g.font == i as u8), "{face:?} paints from its own font");
    }
}

/// Every row of a one-message transcript at 390pt, with `math` settings.
fn rows_with_math(md: &str, math: MathSettings) -> Vec<RowDisplay> {
    let mut w = worker(390.0);
    w.math = math;
    w.input = transcript_one(md);
    let frame = w.pass();
    (0..frame.row_count()).map(|i| frame.display(i).unwrap()).collect()
}

/// With math rendering off, formulas show their source: inline as a code
/// chip with its dollars, display as a `math` code block.
#[test]
fn math_off_shows_source() {
    let off = MathSettings { enabled: false, ..MathSettings::default() };
    let d = rows_with_math(r"A fraction $\frac{a}{b}$ here.", off).remove(0);
    assert!(d.glyphs.is_empty());
    assert!(d.text.contains(r"$\frac{a}{b}$"), "{}", d.text);
    let chip = d.runs.iter().find(|r| run_text(&d, r).contains(r"\frac")).unwrap();
    assert_eq!(chip.color, display::ColorRole::InlineCodeText);
    assert!(d.boxes.iter().any(|b| b.color == display::ColorRole::InlineCodeBackground));

    let d = rows_with_math(r"$$\int_0^1 x\,dx$$", off).remove(0);
    assert!(d.glyphs.is_empty());
    assert!(d.text.contains(r"\int_0^1 x\,dx"), "{}", d.text);
    assert!(d.text.contains("math"), "labelled a math block: {}", d.text);
    assert!(d.boxes.iter().any(|b| b.color == display::ColorRole::CodeBackground));
}

/// STIX Two paints from font 1 only; Fira paints Latin from font 0 and
/// falls back to STIX Two (font 1) for script letters it lacks.
#[test]
fn math_face_picks_glyph_fonts() {
    use zatex::bundled::Face;
    let md = r"Loss $\mathcal{L} = x^2 + \alpha$ here.";
    let stix = rows_with_math(md, MathSettings { face: Face::StixTwo, ..MathSettings::default() }).remove(0);
    assert!(!stix.glyphs.is_empty());
    assert!(stix.glyphs.iter().all(|g| g.font == 1), "{:?}", stix.glyphs);

    let fira = rows_with_math(r"Sum $x^2 + \alpha$ here.", MathSettings::default()).remove(0);
    assert!(!fira.glyphs.is_empty());
    assert!(fira.glyphs.iter().all(|g| g.font == 0), "{:?}", fira.glyphs);
    let cal = rows_with_math(r"$\mathcal{L}$", MathSettings::default()).remove(0);
    assert!(!cal.glyphs.is_empty());
    assert!(cal.glyphs.iter().all(|g| g.font == 1), "{:?}", cal.glyphs);
}

/// Changing the math settings rebuilds every row (new versions, so platform
/// caches miss) and keeps what the user unfolded.
#[test]
fn math_setting_change_relayouts_and_keeps_folds() {
    let mut w = worker(390.0);
    let long = (0..40).map(|i| format!("line {i} of a long pasted prompt")).collect::<Vec<_>>().join("\n");
    w.input = debug_input(
        vec![
            DebugEntry { id: "u".into(), user: true, text: long, streaming: false },
            DebugEntry { id: "a".into(), user: false, text: r"Inline $x^2$ math.".into(), streaming: false },
        ],
        false,
    );
    let folded = w.pass();
    let key = folded.placement(0).unwrap().key;
    w.builder.expanded.insert(key);
    w.builder.invalidate(key);
    let open = w.pass();
    assert!(!w.set_math(MathSettings::default()), "unchanged settings are a no-op");
    assert!(w.set_math(MathSettings { enabled: false, ..MathSettings::default() }));
    let off = w.pass();
    assert_eq!(off.row_count(), open.row_count());
    for i in 0..off.row_count() {
        assert_ne!(off.placement(i).unwrap().version, open.placement(i).unwrap().version, "row {i}");
    }
    assert!((off.placement(0).unwrap().height - open.placement(0).unwrap().height).abs() < 0.01, "still unfolded");
    let math = (0..off.row_count()).find(|&i| !open.display(i).unwrap().glyphs.is_empty()).expect("a formula row");
    let d = off.display(math).unwrap();
    assert!(d.glyphs.is_empty() && d.text.contains("$x^2$"), "{}", d.text);
}

/// The math size scales formulas against the text around them: glyphs, the
/// inline box (so the words after it move) and display blocks alike, and a
/// taller formula grows its line further.
#[test]
fn math_size_scales_formulas() {
    let at = |percent| MathSettings::new(true, zatex::bundled::Face::Fira, percent);
    let plain = rows_with_math("A fraction x on one line.", at(100)).remove(0);
    let md = r"A fraction $\frac{a^2}{b_k}$ on one line.";
    let (normal, big) = (rows_with_math(md, at(100)).remove(0), rows_with_math(md, at(135)).remove(0));
    assert_eq!(normal.glyphs.len(), big.glyphs.len());
    for (a, b) in normal.glyphs.iter().zip(&big.glyphs) {
        assert!((b.size - a.size * 1.35).abs() < 0.01, "{a:?} vs {b:?}");
    }
    // The inline box: from the end of the words before it to the start of
    // the words after it, less the fixed thin space either side.
    let box_w = |d: &RowDisplay| {
        let before = d.runs.iter().find(|r| run_text(d, r).starts_with("A fraction")).map(|r| r.x + r.width).unwrap();
        let after = d.runs.iter().find(|r| run_text(d, r).contains("on one line")).unwrap().x;
        after - before - 3.0
    };
    assert!((box_w(&big) - box_w(&normal) * 1.35).abs() < 0.05, "{} vs {}", box_w(&big), box_w(&normal));
    let (grow, grow_big) = (normal.height - plain.height, big.height - plain.height);
    assert!(grow > 1.0 && grow_big > grow + 1.0, "{grow} then {grow_big}");

    let tex = r"\int_0^1 x^2\,dx = \frac{1}{3}";
    let block = |percent| {
        let d = rows_with_math(&format!("$${tex}$$"), at(percent)).remove(0);
        d.height - rows::Gap::First.px(Px(1.0)) - markdown::geom::MATH_PAD_Y * 2.0
    };
    let f = zatex::bundled::layout(tex, zatex::bundled::Face::Fira, &zatex::Options::display(style::TYPE.body.0)).unwrap();
    assert!((block(100) - f.height()).abs() < 0.01);
    assert!((block(135) - f.height() * 1.35).abs() < 0.05, "{} vs {}", block(135), f.height() * 1.35);

    // Out of range sizes clamp.
    assert_eq!(at(10).size_percent, 75);
    assert_eq!(at(1000).size_percent, 200);
}

/// Changing only the math size rebuilds every row and keeps folds too.
#[test]
fn math_size_change_relayouts_and_keeps_folds() {
    let mut w = worker(390.0);
    let long = (0..40).map(|i| format!("line {i} of a long pasted prompt")).collect::<Vec<_>>().join("\n");
    w.input = debug_input(
        vec![
            DebugEntry { id: "u".into(), user: true, text: long, streaming: false },
            DebugEntry { id: "a".into(), user: false, text: r"Inline $\frac{x^2}{y}$ math.".into(), streaming: false },
        ],
        false,
    );
    let folded = w.pass();
    let key = folded.placement(0).unwrap().key;
    w.builder.expanded.insert(key);
    w.builder.invalidate(key);
    let open = w.pass();
    assert!(w.set_math(MathSettings { size_percent: 135, ..MathSettings::default() }));
    let big = w.pass();
    assert_eq!(big.row_count(), open.row_count());
    for i in 0..big.row_count() {
        assert_ne!(big.placement(i).unwrap().version, open.placement(i).unwrap().version, "row {i}");
    }
    assert!((big.placement(0).unwrap().height - open.placement(0).unwrap().height).abs() < 0.01, "still unfolded");
    let math = (0..big.row_count()).find(|&i| !open.display(i).unwrap().glyphs.is_empty()).expect("a formula row");
    let (a, b) = (open.display(math).unwrap(), big.display(math).unwrap());
    assert!((b.glyphs[0].size - a.glyphs[0].size * 1.35).abs() < 0.01);
    assert!(b.height > a.height);
}

/// Leftmost x of the content column in a row `width` wide.
fn column_x(width: f32) -> (f32, f32) {
    let column = rows::column_width(Px(1.0), width);
    ((width - column) / 2.0, column)
}

/// An inline formula wider than the column shrinks to fit it (glyphs,
/// offsets and rules alike) instead of painting off the edge, and its line
/// keeps the unscaled formula's height.
#[test]
fn overly_wide_inline_formula_shrinks_to_the_column() {
    let tex = r"a_1 + a_2 + a_3 + a_4 + a_5 + a_6 + a_7 + a_8 + a_9 + a_{10} + a_{11} + a_{12} + \frac{a_{13}}{a_{14}}";
    let f = zatex::bundled::layout(tex, zatex::bundled::Face::Fira, &zatex::Options::inline(style::TYPE.body.0)).unwrap();
    for width in [200.0, 320.0] {
        let (x0, column) = column_x(width);
        assert!(f.width > column, "the formula must overflow at {width}");
        let d = rows_of(&format!("${tex}$"), width).remove(0);
        assert!(!d.glyphs.is_empty());
        let right = x0 + column + 0.5;
        assert!(d.glyphs.iter().all(|g| g.x >= x0 && g.x <= right), "width {width}");
        assert!(d.glyphs.iter().all(|g| g.size < style::TYPE.body.0), "scaled glyphs");
        let rules: Vec<_> = d.boxes.iter().filter(|b| b.start.is_some()).collect();
        assert!(!rules.is_empty(), "the fraction bar");
        assert!(rules.iter().all(|b| b.x >= x0 && b.x + b.w <= right), "width {width}: {rules:?}");
        // The last glyph's origin lands near the column's right edge.
        let last = d.glyphs.iter().map(|g| g.x).fold(0f32, f32::max);
        assert!(last > x0 + column * 0.8, "fills the column: {last}");
    }
}

/// A table column grows to its widest formula (the table scrolls), so the
/// formula never paints over the next column.
#[test]
fn table_formula_cell_does_not_overlap_the_next_column() {
    let tex = r"a_1 + a_2 + a_3 + a_4 + a_5 + a_6 + a_7 + a_8 + a_9 + a_{10} + a_{11} + a_{12} + a_{13} + a_{14} + a_{15}";
    let d = rows_of(&format!("| f | next |\n|---|---|\n| ${tex}$ | after |\n"), 390.0).remove(0);
    assert_eq!(d.scrollers.len(), 1, "the widened table scrolls");
    let next = d.runs.iter().find(|r| run_text(&d, r) == "after").expect("next cell");
    let f = zatex::bundled::layout(tex, zatex::bundled::Face::Fira, &zatex::Options::inline(style::TYPE.body.0)).unwrap();
    let left = d.glyphs.iter().map(|g| g.x).fold(f32::INFINITY, f32::min);
    assert!(left + f.width <= next.x, "{left} + {} vs {}", f.width, next.x);
    assert!(d.glyphs.iter().all(|g| (g.size - style::TYPE.body.0).abs() < 0.01 || g.size < style::TYPE.body.0), "unscaled");
    assert!(d.glyphs.iter().any(|g| (g.size - style::TYPE.body.0).abs() < 0.01), "not shrunk");
}

/// A list marker sits on its item's first line even when that line is a
/// heading grown by a tall formula.
#[test]
fn list_marker_follows_a_heading_grown_by_a_formula() {
    let offset = |md: &str| {
        let d = rows_of(md, 390.0).remove(0);
        let marker = d.runs.iter().find(|r| run_text(&d, r) == "•").expect("marker").baseline;
        let head = d.runs.iter().find(|r| run_text(&d, r).starts_with("Head")).expect("heading").baseline;
        head - marker
    };
    let plain = offset("- ## Head tall\n");
    let grown = offset("- ## Head $\\frac{a^2}{b^2}$ tall\n");
    assert!((plain - grown).abs() < 0.01, "{plain} vs {grown}");
}

/// Strikethrough crosses a formula too: a 1pt fill in the run's color at the
/// strike offset, veiled with the formula.
#[test]
fn strikethrough_crosses_a_formula() {
    let d = rows_of(r"~~$x^2 + y^2$~~", 390.0).remove(0);
    assert!(!d.glyphs.is_empty());
    let start = d.glyphs[0].start;
    let baseline = d.glyphs[0].y;
    let strike = d.boxes.iter().find(|b| b.start == Some(start) && (b.y - (baseline - 5.0)).abs() < 0.01).expect("strike line");
    assert_eq!(strike.h, 1.0);
    assert_eq!(strike.color, display::ColorRole::Text);
    assert!(strike.w > 10.0);
}

/// Formula rules carry their placeholder offset so the streaming veil fades
/// them with the glyphs; chrome boxes don't.
#[test]
fn formula_rules_veil_with_their_glyphs() {
    let d = rows_of("Code chip `x` and $\\frac{a}{b}$.", 390.0).remove(0);
    let start = d.glyphs[0].start;
    assert!(d.boxes.iter().any(|b| b.start == Some(start)), "fraction bar");
    assert!(d.boxes.iter().filter(|b| b.color == display::ColorRole::InlineCodeBackground).all(|b| b.start.is_none()));
}

/// Scrolling display math pads its content so ink past the advance stays
/// visible at both scroll ends.
#[test]
fn scrolling_display_math_pads_its_sides() {
    let d = rows_of(&format!("$${}$$", r"a_1 + a_2 + a_3 + a_4 + a_5 + a_6 + a_7 + a_8 + a_9 + a_{10} + a_{11} + a_{12} + a_{13}"), 320.0).remove(0);
    let s = &d.scrollers[0];
    let left = d.glyphs.iter().map(|g| g.x).fold(f32::INFINITY, f32::min);
    assert!(left >= 3.0, "{left}");
    assert!(s.content_width > s.w);
}

/// Laid-out formulas are reused: a rebuild (or a streamed token
/// re-preparing its paragraph) doesn't lay any of them out again.
#[test]
fn laid_out_formulas_are_reused() {
    let mut w = worker(390.0);
    w.input = transcript_one(r"Fine $x^2$ and $\frac{a}{b}$ here.");
    let first = w.pass().display(0).unwrap();
    assert_eq!(w.math_failures.laid_hits, 0);
    w.rebuild();
    let again = w.pass().display(0).unwrap();
    assert_eq!(w.math_failures.laid_hits, 2, "both formulas came from the cache");
    assert_eq!(first.glyphs.len(), again.glyphs.len());
}

/// Formulas zatex accepts but this host can't afford show as source: an
/// inline one taller than a line can hold, or display math whose scrolling
/// canvas would be an outsized backing store.
#[test]
fn oversized_formulas_show_as_source() {
    for md in [
        r"Tall $\begin{matrix}x\\[40em]y\end{matrix}$ inline",
        r"$$\hspace{70em}\hspace{70em}\begin{matrix}x\\[95em]y\end{matrix}$$",
    ] {
        let d = rows_of(md, 390.0).remove(0);
        assert!(d.glyphs.is_empty(), "rendered as source: {md}");
        assert!(d.height < 2_000.0, "{md}: {}", d.height);
    }
    // Ordinary display math still renders.
    let d = rows_of(r"$$\sum_{i=1}^n i = \frac{n(n+1)}{2}$$", 390.0).remove(0);
    assert!(!d.glyphs.is_empty());
}

/// A table's rows share one scrolling canvas, so a tall formula in a cell
/// shows as source; the same formula in prose still renders. Long display
/// derivations narrower than the column render (only area is capped).
#[test]
fn table_cells_hold_formulas_short_and_long_derivations_render() {
    let tall = r"\begin{matrix}a\\b\\c\\d\\e\\f\\g\\h\\i\\j\\k\\l\\m\\n\end{matrix}";
    let cell = rows_of(&format!("| t | v |\n|---|---|\n| x | ${tall}$ |"), 390.0).remove(0);
    assert!(cell.glyphs.is_empty(), "tall cell formula shows as source");
    let header = rows_of(&format!("| ${tall}$ |\n|---|\n| x |"), 390.0).remove(0);
    assert!(header.glyphs.is_empty(), "header cells are held short too");
    // Ordinary cell math still renders.
    let vector = rows_of(r"| v |
|---|
| $\begin{pmatrix}1\\2\\3\\4\\5\end{pmatrix}$ |", 390.0).remove(0);
    assert!(!vector.glyphs.is_empty(), "a 5-row vector fits a cell");
    let prose = rows_of(&format!("so ${tall}$ here"), 390.0).remove(0);
    assert!(!prose.glyphs.is_empty(), "in prose it renders");
    let rows: String = (0..40).map(|i| format!("a_{{{i}}} &= \\frac{{b_{i}}}{{c_{i}}} \\\\ ")).collect();
    let d = rows_of(&format!("$$\\begin{{aligned}}{rows}\\end{{aligned}}$$"), 390.0).remove(0);
    assert!(!d.glyphs.is_empty(), "a 40-row derivation renders");
}

/// The formula cache stays within its bounds through many compactions:
/// every live entry keeps a use record, so eviction can always reach it.
#[test]
fn formula_cache_stays_bounded_through_compactions() {
    let mut w = worker(390.0);
    for round in 0..12 {
        let md: String = (0..1500).map(|i| format!("$x_{{{}}}$ ", round * 1500 + i)).collect();
        w.input = transcript_one(&md);
        w.pass();
        // Hit the newest ones again: uses pile up and force compactions.
        w.rebuild();
        w.pass();
    }
    let (live, uses, bytes) = w.math_failures.laid_stats();
    assert!(live <= 4096 && bytes <= 8 << 20, "{live} formulas, {bytes} bytes");
    assert!(uses >= live, "every live formula keeps a use: {uses} < {live}");
}

/// A row's formulas share one canvas budget: past it, the rest show as
/// source instead of each adding a full-size backing store.
#[test]
fn a_rows_formulas_share_one_budget() {
    let wide = r"$$\hspace{60em}\hspace{60em}\begin{matrix}x\\[30em]y\end{matrix}$$";
    let list: String = (0..40).map(|_| format!("- {wide}\n")).collect();
    let one = rows_of(&format!("- {wide}\n"), 390.0).remove(0);
    assert!(!one.glyphs.is_empty(), "one of them alone renders");
    let d = rows_of(&list, 390.0).remove(0);
    let rendered = d.glyphs.len() / one.glyphs.len();
    assert!(rendered < 40, "budget stops the row's formulas: {rendered}");
}

/// A header-only table leaves the cell limit behind it: the paragraph after
/// keeps the prose limit.
#[test]
fn a_header_only_table_leaves_prose_limits_alone() {
    let tall = r"\begin{matrix}a\\b\\c\\d\\e\\f\\g\\h\\i\\j\\k\\l\\m\\n\end{matrix}";
    let alone = rows_of(&format!("so ${tall}$ here"), 390.0).remove(0);
    assert!(!alone.glyphs.is_empty());
    let rows = rows_of(&format!("| h |\n|---|\n\nso ${tall}$ here"), 390.0);
    let after = rows.last().unwrap();
    assert_eq!(after.glyphs.len(), alone.glyphs.len(), "the paragraph after renders as on its own");
}

/// A table's canvas is all its columns by all its rows: formulas that widen
/// columns and raise rows multiply. Past its budget the table sets its
/// formulas as source, as a text table; a small math table still renders.
#[test]
fn math_tables_keep_a_bounded_canvas() {
    let wide = r"$\hspace{35em}x$";
    let tall = r"$\begin{matrix}a\\[7.5em]b\end{matrix}$";
    let header = format!("|{}|\n|{}|\n", [wide; 10].join("|"), ["---"; 10].join("|"));
    let body: String = (0..28).map(|_| format!("|{}|\n", [tall; 10].join("|"))).collect();
    let d = rows_of(&format!("{header}{body}"), 390.0).remove(0);
    let canvas: f32 = d.scrollers.iter().map(|s| s.content_width * s.h).sum::<f32>().max(390.0 * d.height);
    assert!(canvas < 8e6, "table canvas {canvas} pt²");
    let small = rows_of("| a | b |\n|---|---|\n| $x^2$ | $\\frac{1}{2}$ |", 390.0).remove(0);
    assert!(!small.glyphs.is_empty(), "a small math table renders its formulas");
}
