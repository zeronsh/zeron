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
