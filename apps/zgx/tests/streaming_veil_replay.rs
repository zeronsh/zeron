//! Diagnostic & regression tests for streaming veil replay bugs:
//! when a markdown block finishes (or a later block streams in), neither the
//! finished block nor any previous block should replay its fade-in veil.

use std::path::PathBuf;
use std::time::Duration;

use zgx::scene::{Color, NodeId, NodeKind};
use zgx_script::{ScriptHost, Value, testing::TestApp};

fn ui_dir() -> PathBuf {
    std::env::var_os("ZERON_ZGX_TEST_UI")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui"))
}

fn pieces_under(app: &TestApp, root: NodeId) -> Vec<(String, Color)> {
    let scene = app.ui.scene.borrow();
    let mut stack = vec![root];
    while let Some(id) = stack.pop() {
        match scene.kind(id) {
            NodeKind::RichText { text } => {
                return text
                    .runs()
                    .iter()
                    .map(|run| (text.text()[run.range.clone()].to_owned(), run.color))
                    .collect();
            }
            NodeKind::Text { text, color, .. } => return vec![(text.to_string(), *color)],
            _ => stack.extend(scene.children(id).iter().rev().copied()),
        }
    }
    Vec::new()
}

fn pieces(app: &mut TestApp, words: &str) -> Vec<(String, Color)> {
    let node = app
        .find(&format!("~\"{words}\""))
        .unwrap_or_else(|| panic!("no {words}:\n{}", app.tree()));
    pieces_under(app, node)
}

/// Minimum alpha across non-empty runs whose text overlaps `needle` (or all non-empty runs if `needle` is empty).
fn min_alpha(runs: &[(String, Color)], needle: &str) -> u8 {
    runs.iter()
        .filter(|(t, _)| !t.trim().is_empty() && (needle.is_empty() || t.contains(needle) || needle.contains(t.as_str())))
        .map(|(_, c)| c.3)
        .min()
        .unwrap_or(255)
}

fn settle(app: &mut TestApp) {
    for _ in 0..45 {
        app.frames(1);
        std::thread::sleep(Duration::from_millis(12));
    }
}

fn setup_streaming_reply() -> TestApp {
    let app = TestApp::new(|runtime| ScriptHost::new(runtime, ui_dir().join("chat_fixture.luau")));
    app.ui.scene.borrow_mut().resize(900., 700.);
    app.frames(2);
    assert!(app.host.run_command("go_s2"));
    app.frames(20);
    std::thread::sleep(Duration::from_millis(320));
    assert!(app.host.run_command("start_block_reply"));
    app.frames(3);
    assert_eq!(app.error(), None);
    app
}

fn push_draft(app: &mut TestApp, text: &str) {
    let words = Value::from(text);
    app.host.shared("chat.draft", Value::Nil).set(Value::record([
        ("id", Value::from("live")),
        ("text", words.clone()),
        ("part", Value::from("t")),
        ("part_text", words),
    ]));
    app.frames(2);
    assert_eq!(app.error(), None);
}

fn check_no_replay(label: &str, before: &[(String, Color)], after: &[(String, Color)], needle: &str) {
    let was = min_alpha(before, needle);
    let now = min_alpha(after, needle);
    assert!(
        now >= was,
        "REPLAY BUG ({label}): alpha dropped from {was} to {now}!\n  before: {before:?}\n  after:  {after:?}"
    );
}

#[test]
fn probe_01_list_with_file_link() {
    let mut app = setup_streaming_reply();
    push_draft(&mut app, "- Check [chat.rs](apps/zgx/src/chat.rs) first");
    settle(&mut app);
    let before = pieces(&mut app, "Check ");

    push_draft(&mut app, "- Check [chat.rs](apps/zgx/src/chat.rs) first\n- Then plain item two");
    let after = pieces(&mut app, "Check ");
    check_no_replay("list item 1 with file link when item 2 arrives", &before, &after, "Check");
}

#[test]
fn probe_02_paragraph_with_link_then_next_paragraph() {
    let mut app = setup_streaming_reply();
    push_draft(&mut app, "See [chat.rs](apps/zgx/src/chat.rs) for details");
    settle(&mut app);
    let before = pieces(&mut app, "for details");

    push_draft(&mut app, "See [chat.rs](apps/zgx/src/chat.rs) for details.\n\nSecond paragraph starts here.");
    let after = pieces(&mut app, "for details");
    check_no_replay("p1 with file link when p2 starts", &before, &after, "See");
}

#[test]
fn probe_03_table_with_link_rows_and_next_paragraph() {
    let mut app = setup_streaming_reply();
    push_draft(&mut app, "| File | Role |\n|---|---|\n| [chat.rs](apps/zgx/src/chat.rs) | host |");
    settle(&mut app);
    let h_before = pieces(&mut app, "File");
    let r1_before = pieces(&mut app, "host");

    push_draft(&mut app, "| File | Role |\n|---|---|\n| [chat.rs](apps/zgx/src/chat.rs) | host |\n| plain.rs | helper |");
    let h_after = pieces(&mut app, "File");
    let r1_after = pieces(&mut app, "host");
    check_no_replay("table header when row2 arrives", &h_before, &h_after, "File");
    check_no_replay("table row1 when row2 arrives", &r1_before, &r1_after, "host");

    settle(&mut app);
    let r2_before = pieces(&mut app, "helper");
    push_draft(&mut app, "| File | Role |\n|---|---|\n| [chat.rs](apps/zgx/src/chat.rs) | host |\n| plain.rs | helper |\n\nAfter the table.");
    let r2_after = pieces(&mut app, "helper");
    check_no_replay("table row2 when next paragraph starts", &r2_before, &r2_after, "helper");
}

#[test]
fn probe_04_plain_table_then_next_paragraph() {
    let mut app = setup_streaming_reply();
    push_draft(&mut app, "| Alpha | Beta |\n|---|---|\n| one | two |");
    settle(&mut app);
    let before = pieces(&mut app, "two");

    push_draft(&mut app, "| Alpha | Beta |\n|---|---|\n| one | two |\n| three | four |\n\nNext paragraph.");
    let after = pieces(&mut app, "two");
    check_no_replay("plain table row1 when row2 + next paragraph arrive", &before, &after, "two");
}

#[test]
fn probe_05_streaming_web_link_in_list() {
    let mut app = setup_streaming_reply();
    push_draft(&mut app, "- First item settled already\n- Second with [the docs](https://ex");
    settle(&mut app);
    let i1_before = pieces(&mut app, "First item settled");
    let i2_before = pieces(&mut app, "Second with");

    push_draft(&mut app, "- First item settled already\n- Second with [the docs](https://example.com) done");
    let i1_after = pieces(&mut app, "First item settled");
    let i2_after = pieces(&mut app, "Second with");
    check_no_replay("list item 1 when item 2 web link finishes", &i1_before, &i1_after, "First item");
    check_no_replay("list item 2 prefix when web link finishes", &i2_before, &i2_after, "Second with");
}

#[test]
fn probe_06_code_block_closing_and_next_paragraph() {
    let mut app = setup_streaming_reply();
    push_draft(&mut app, "Intro paragraph settled.\n\n```rust\nfn hello() {}\n");
    settle(&mut app);
    let p1_before = pieces(&mut app, "Intro paragraph");
    let code_before = pieces(&mut app, "hello");

    push_draft(&mut app, "Intro paragraph settled.\n\n```rust\nfn hello() {}\n```\n\nAfter code.");
    let p1_after = pieces(&mut app, "Intro paragraph");
    let code_after = pieces(&mut app, "hello");
    check_no_replay("intro paragraph when code block closes", &p1_before, &p1_after, "Intro");
    check_no_replay("code block when fence closes", &code_before, &code_after, "hello");
}

#[test]
fn probe_07_bracketed_text_and_unclosed_markers() {
    let mut app = setup_streaming_reply();
    push_draft(&mut app, "Check prefix [INFO] done and 2**3 here");
    settle(&mut app);
    let before = pieces(&mut app, "Check prefix");

    push_draft(&mut app, "Check prefix [INFO] done and 2**3 here\n\nNext paragraph.");
    let after = pieces(&mut app, "Check prefix");
    check_no_replay("paragraph with 2**3 when next paragraph starts", &before, &after, "Check prefix");
}

#[test]
fn probe_08_inline_math_and_display_math_closing() {
    let mut app = setup_streaming_reply();
    push_draft(&mut app, "First settled paragraph.\n\nFormula follows:\n$$\nx + y = z\n");
    settle(&mut app);
    let p1_before = pieces(&mut app, "First settled");

    push_draft(&mut app, "First settled paragraph.\n\nFormula follows:\n$$\nx + y = z\n$$\n\nAfter math.");
    let p1_after = pieces(&mut app, "First settled");
    check_no_replay("p1 when display math closes", &p1_before, &p1_after, "First settled");
}

#[test]
fn probe_09_html_and_link_def_mid_stream() {
    let mut app = setup_streaming_reply();
    push_draft(&mut app, "Alpha paragraph settled.\n\nBeta paragraph settled.\n\nGamma paragraph.");
    settle(&mut app);
    let p1_before = pieces(&mut app, "Alpha paragraph");
    let p2_before = pieces(&mut app, "Beta paragraph");

    // Link definition triggers whole = true / cut = 0 in math_source.
    push_draft(&mut app, "Alpha paragraph settled.\n\nBeta paragraph settled.\n\nGamma paragraph.\n\n[ref]: https://example.com\n\nDelta.");
    let p1_after = pieces(&mut app, "Alpha paragraph");
    let p2_after = pieces(&mut app, "Beta paragraph");
    check_no_replay("p1 when link def arrives", &p1_before, &p1_after, "Alpha");
    check_no_replay("p2 when link def arrives", &p2_before, &p2_after, "Beta");
}

#[test]
fn probe_10_long_reply_over_chunk_boundary() {
    let mut app = setup_streaming_reply();
    let mut text = String::new();
    let mut settled_markers: Vec<(String, Vec<(String, Color)>)> = Vec::new();
    for i in 1..=20 {
        let block = match i % 5 {
            1 => format!("## Heading number {i}\n\n"),
            2 => format!("Paragraph {i} with **bold** and `code_{i}` and [docs](https://example.com/{i}).\n\n"),
            3 => format!("- Item A of {i} referencing [file_{i}.rs](src/file_{i}.rs)\n- Item B of {i} plain text\n\n"),
            4 => format!("```rust\nfn block_{i}() {{ let x = {i}; }}\n```\n\n"),
            _ => format!("| ColA | ColB |\n|---|---|\n| val_{i} | note_{i} |\n\n"),
        };
        let marker = match i % 5 {
            1 => format!("Heading number {i}"),
            2 => format!("Paragraph {i} with"),
            3 => format!("Item A of {i}"),
            4 => format!("block_{i}"),
            _ => format!("val_{i}"),
        };
        let chars: Vec<usize> = block.char_indices().map(|(idx, _)| idx).collect();
        let c1 = chars[chars.len() / 3];
        let c2 = chars[chars.len() * 2 / 3];
        for piece in [&block[..c1], &block[c1..c2], &block[c2..]] {
            text.push_str(piece);
            push_draft(&mut app, &text);
            for (prev, before) in &settled_markers {
                if let Some(node) = app.find(&format!("~\"{prev}\"")) {
                    let after = pieces_under(&app, node);
                    let was = min_alpha(before, prev);
                    let now = min_alpha(&after, prev);
                    assert!(
                        now >= was,
                        "REPLAY BUG at block {i}: previous marker `{prev}` dropped alpha {was} -> {now}!\n  before: {before:?}\n  after:  {after:?}"
                    );
                }
            }
        }
        settle(&mut app);
        if let Some(node) = app.find(&format!("~\"{marker}\"")) {
            let runs = pieces_under(&app, node);
            settled_markers.push((marker, runs));
        }
    }
}

#[test]
fn probe_11_paragraph_interrupted_by_fence_or_list() {
    let mut app = setup_streaming_reply();
    push_draft(&mut app, "Paragraph one settled.\n\nParagraph two has code right after:");
    settle(&mut app);
    let p1_before = pieces(&mut app, "Paragraph one");
    let p2_before = pieces(&mut app, "Paragraph two");

    // Fence interrupts p2 without blank line -> splits into p2 + code block.
    push_draft(&mut app, "Paragraph one settled.\n\nParagraph two has code right after:\n```rust\nlet a = 1;\n```");
    let p1_after = pieces(&mut app, "Paragraph one");
    let p2_after = pieces(&mut app, "Paragraph two");
    check_no_replay("p1 when fence interrupts p2", &p1_before, &p1_after, "Paragraph one");
    check_no_replay("p2 when fence interrupts p2", &p2_before, &p2_after, "Paragraph two");
}

#[test]
fn probe_12_quote_with_file_link_and_second_paragraph() {
    let mut app = setup_streaming_reply();
    push_draft(&mut app, "> See [chat.rs](apps/zgx/src/chat.rs) first");
    settle(&mut app);
    let before = pieces(&mut app, "See ");

    push_draft(&mut app, "> See [chat.rs](apps/zgx/src/chat.rs) first\n>\n> Second quote paragraph.");
    let after = pieces(&mut app, "See ");
    check_no_replay("quote p1 with file link when quote p2 arrives", &before, &after, "See");
}

#[test]
fn probe_13_table_cell_with_file_link_when_row2_arrives() {
    let mut app = setup_streaming_reply();
    push_draft(&mut app, "| File | Role |\n|---|---|\n| See [chat.rs](apps/zgx/src/chat.rs) here | host |");
    settle(&mut app);
    let cell_before = pieces(&mut app, "See ");

    push_draft(&mut app, "| File | Role |\n|---|---|\n| See [chat.rs](apps/zgx/src/chat.rs) here | host |\n| plain.rs | helper |");
    let cell_after = pieces(&mut app, "See ");
    check_no_replay("table cell with file link when row2 arrives", &cell_before, &cell_after, "See");

    settle(&mut app);
    let cell_before2 = pieces(&mut app, "See ");
    push_draft(&mut app, "| File | Role |\n|---|---|\n| See [chat.rs](apps/zgx/src/chat.rs) here | host |\n| plain.rs | helper |\n\nNext paragraph.");
    let cell_after2 = pieces(&mut app, "See ");
    check_no_replay("table cell with file link when next paragraph arrives", &cell_before2, &cell_after2, "See");
}

#[test]
fn probe_14_pending_link_in_list_and_paragraph() {
    let mut app = setup_streaming_reply();
    // Pending link in item 1 (zgx:pending-link) while item 1 is still streaming, then closes and item 2 arrives.
    push_draft(&mut app, "- Open [the docs](https://example.com/guide");
    settle(&mut app);
    let i1_before = pieces(&mut app, "Open ");

    push_draft(&mut app, "- Open [the docs](https://example.com/guide)\n- Second plain item");
    let i1_after = pieces(&mut app, "Open ");
    check_no_replay("list item 1 pending link closing + item 2", &i1_before, &i1_after, "Open");

    // Also test a bare bracket "[INFO" -> "[INFO] server started" in a paragraph followed by p2
    let mut app2 = setup_streaming_reply();
    push_draft(&mut app2, "Log prefix [INFO");
    settle(&mut app2);
    let p_before = pieces(&mut app2, "Log prefix");
    push_draft(&mut app2, "Log prefix [INFO] server started");
    let p_after = pieces(&mut app2, "Log prefix");
    check_no_replay("paragraph prefix when [INFO closes to [INFO]", &p_before, &p_after, "Log prefix");
}

#[test]
fn probe_15_code_block_after_link_in_list() {
    let mut app = setup_streaming_reply();
    push_draft(&mut app, "- See [chat.rs](apps/zgx/src/chat.rs) first\n\n  ```rust\n  fn demo() {}\n  ```");
    settle(&mut app);
    let p_before = pieces(&mut app, "See ");
    let c_before = pieces(&mut app, "demo");

    push_draft(&mut app, "- See [chat.rs](apps/zgx/src/chat.rs) first\n\n  ```rust\n  fn demo() {}\n  ```\n\n- Second item");
    let p_after = pieces(&mut app, "See ");
    let c_after = pieces(&mut app, "demo");
    check_no_replay("list item 1 link paragraph when item 2 arrives", &p_before, &p_after, "See");
    check_no_replay("code block after link paragraph in list when item 2 arrives", &c_before, &c_after, "demo");
}

#[test]
fn probe_16_heading_with_file_link_then_paragraph() {
    let mut app = setup_streaming_reply();
    push_draft(&mut app, "## Changes in [chat.rs](apps/zgx/src/chat.rs)");
    settle(&mut app);
    let h_before = pieces(&mut app, "Changes in");

    // Finish heading with punctuation and start the next paragraph in the same chunk.
    push_draft(&mut app, "## Changes in [chat.rs](apps/zgx/src/chat.rs)!\n\nHere is what changed.");
    let h_after = pieces(&mut app, "Changes in");
    check_no_replay("heading with file link when paragraph arrives", &h_before, &h_after, "Changes in");
}

#[test]
fn probe_17_unclosed_marker_after_2_pow_3() {
    let mut app = setup_streaming_reply();
    push_draft(&mut app, "Value is 2**3 here");
    settle(&mut app);
    let before = pieces(&mut app, "Value is");

    push_draft(&mut app, "Value is 2**3 here\n\nNext paragraph.");
    let after = pieces(&mut app, "Value is");
    check_no_replay("text after 2**3 when block finishes", &before, &after, "here");
}

#[test]
fn probe_18_boost_drop_backwards_alpha() {
    let mut app = setup_streaming_reply();
    // Push 6 chunks rapidly (~25ms apart) so #going reaches 6 (boost = 1 + 0.3*(6-2) = 2.2).
    // Chunk 1 has ema ~160ms => ms ~300-400ms, at boost=2.2 it completes in ~180ms wall time.
    // Wait until chunks 1..4 are past ms/boost (~180ms), then push chunk 7 while #going drops.
    let mut text = String::from("Alpha ");
    push_draft(&mut app, &text);
    for word in ["bravo ", "charlie ", "delta ", "echo ", "foxtrot "] {
        std::thread::sleep(Duration::from_millis(25));
        text.push_str(word);
        push_draft(&mut app, &text);
    }
    // Wait ~190ms: chunk 1 (started at t=0, ms~400, boost=2.2 => p = 190*2.2/400 > 1.0) is now 255!
    // Wait until chunk 1 reaches 255 while still within ms (400ms) of start.
    let mut chunk1_peak: u8 = 0;
    for _ in 0..20 {
        app.frames(1);
        let p = pieces(&mut app, "Alpha");
        let a = min_alpha(&p, "Alpha");
        chunk1_peak = chunk1_peak.max(a);
        if chunk1_peak == 255 {
            break;
        }
        std::thread::sleep(Duration::from_millis(12));
    }
    // Now wait a bit more so earlier chunks expire if left <= 0, OR push one more chunk while
    // fewer chunks remain going: check that Alpha NEVER drops below its peak!
    let mut min_after = chunk1_peak;
    for i in 0..15 {
        if i == 3 {
            text.push_str("golf ");
            push_draft(&mut app, &text);
        }
        app.frames(1);
        let p = pieces(&mut app, "Alpha");
        let a = min_alpha(&p, "Alpha");
        if a < min_after {
            min_after = a;
        }
        std::thread::sleep(Duration::from_millis(15));
    }
    assert!(
        min_after >= chunk1_peak,
        "BOOST DROP BUG: Alpha peaked at {chunk1_peak} then dropped back to {min_after}!"
    );
}

#[test]
fn probe_19_plain_streaming_reply_non_windowed_with_file_link() {
    let app = TestApp::new(|runtime| ScriptHost::new(runtime, ui_dir().join("chat_fixture.luau")));
    app.ui.scene.borrow_mut().resize(900., 700.);
    app.frames(2);
    assert!(app.host.run_command("go_s2"));
    app.frames(20);
    std::thread::sleep(Duration::from_millis(320));
    assert!(app.host.run_command("start_reply"));
    app.frames(3);
    assert_eq!(app.error(), None);

    let mut app = app;
    let set_text = |app: &mut TestApp, text: &str| {
        app.host.shared("chat.draft", Value::Nil).set(Value::record([
            ("id", Value::from("live")),
            ("text", Value::from(text)),
        ]));
        app.frames(2);
        assert_eq!(app.error(), None);
    };
    set_text(&mut app, "Check [chat.rs](apps/zgx/src/chat.rs) first");
    settle(&mut app);
    let before = pieces(&mut app, "Check ");

    set_text(&mut app, "Check [chat.rs](apps/zgx/src/chat.rs) first.\n\nSecond paragraph.");
    let after = pieces(&mut app, "Check ");
    check_no_replay("non-windowed reply p1 with file link when p2 arrives", &before, &after, "Check");
}

#[test]
fn probe_20_table_inside_quote_with_sibling_paragraphs() {
    let mut app = setup_streaming_reply();
    push_draft(&mut app, "> Intro line.\n>\n> | A | B |\n> |---|---|\n> | one | two |");
    settle(&mut app);
    let intro_before = pieces(&mut app, "Intro line");
    let cell_before = pieces(&mut app, "two");

    push_draft(&mut app, "> Intro line.\n>\n> | A | B |\n> |---|---|\n> | one | two |\n>\n> Outro line.");
    let intro_after = pieces(&mut app, "Intro line");
    let cell_after = pieces(&mut app, "two");
    check_no_replay("quote intro when outro arrives after table", &intro_before, &intro_after, "Intro line");
    check_no_replay("table cell inside quote when outro arrives", &cell_before, &cell_after, "two");
}

#[test]
fn probe_21_paragraph_with_file_link_in_block1_then_block2_streams_and_catalog_changes() {
    // When a reply has settled paragraph 1 (with file link) and streaming paragraph 2,
    // a later rebuild of block 1's ui.presence (e.g. skill catalog load) must not replay block 1.
    let mut app = setup_streaming_reply();
    push_draft(&mut app, "First [chat.rs](apps/zgx/src/chat.rs) paragraph.\n\nSecond plain paragraph.");
    settle(&mut app);
    let p1_before = pieces(&mut app, "First ");

    // Trigger a third paragraph so v.block moves to 3, and simultaneously modify paragraph 1's link chip resolution if possible,
    // or rebuild run 1 with paragraph 1 + 2 + 3 (e.g. via link def reset).
    push_draft(&mut app, "First [chat.rs](apps/zgx/src/chat.rs) paragraph.\n\nSecond plain paragraph.\n\n[ref]: https://example.com\n\nThird paragraph.");
    let p1_after = pieces(&mut app, "First ");
    check_no_replay("p1 with file link when whole reparse happens at block 3", &p1_before, &p1_after, "First");
}

#[test]
fn probe_22_new_text_still_fades_in_and_settles() {
    let mut app = setup_streaming_reply();
    push_draft(&mut app, "Brand new streaming words arriving now.");
    let fresh = pieces(&mut app, "Brand new");
    let initial_alpha = min_alpha(&fresh, "Brand new");
    assert!(
        initial_alpha < 200,
        "expected newly streamed text to start fading in (<200 alpha), got {initial_alpha} in {fresh:?}"
    );
    settle(&mut app);
    let settled = pieces(&mut app, "Brand new");
    let final_alpha = min_alpha(&settled, "Brand new");
    assert_eq!(
        final_alpha, 255,
        "expected streamed text to settle to full opacity, got {final_alpha} in {settled:?}"
    );
}
