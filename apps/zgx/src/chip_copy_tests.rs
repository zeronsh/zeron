//! A sent message's mentions are chips (ds/markdown `mention_chip`): one
//! pill each, its room the span's own padding (zgx d6582b32), not thin
//! spaces in its text. So a selection across the message copies the names
//! as written, with no U+2009 around them. Mounted headless on the chat
//! fixture, whose `link_turn` sends "Look at [chat.rs](zeron-file:…) please,
//! as in [Long chat](zeron-chat:s1), and [the site](https://…)".

use zgx_script::{ScriptHost, testing::TestApp};

#[test]
fn copying_a_sent_message_with_chips_copies_the_names_alone() {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui/chat_fixture.luau");
    let mut app = TestApp::new(|runtime| ScriptHost::new(runtime, path));
    app.ui.scene.borrow_mut().resize(900., 700.);
    app.frames(30);
    assert_eq!(app.error(), None, "the fixture failed");
    assert!(app.host.run_command("go_s2"));
    app.frames(30);
    assert!(app.host.run_command("link_turn"));
    app.frames(30);
    let prompt = app.target("#lu1").expect("the prompt with mentions");
    assert!(app.ui.select_all_text(prompt), "its text selects");
    let copied = app.ui.selected_text();
    assert!(copied.contains("chat.rs") && copied.contains("Long chat"), "the names are copied: {copied:?}");
    assert!(!copied.contains('\u{2009}'), "no thin spaces: {copied:?}");
    assert!(copied.contains("Look at chat.rs please, as in Long chat,"), "the words read as written: {copied:?}");
}

#[test]
fn inline_chip_icon_well_has_equal_spacing_around_all_sides() {
    use zgx::scene::NodeKind;

    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui/chat_fixture.luau");
    let mut app = TestApp::new(|runtime| ScriptHost::new(runtime, path));
    app.ui.scene.borrow_mut().resize(900., 700.);
    app.frames(30);
    assert_eq!(app.error(), None, "the fixture failed");
    assert!(app.host.run_command("go_s2"));
    app.frames(30);
    assert!(app.host.run_command("link_turn"));
    app.frames(30);
    let prompt = app.target("#lu1").expect("the prompt with mentions");

    let scene = app.ui.scene.borrow();
    // Collect rich-text nodes (their chip background runs) and icon-well panel nodes inside the prompt.
    let mut stack = vec![prompt];
    let mut chip_runs = Vec::new();
    let mut wells = Vec::new();
    while let Some(id) = stack.pop() {
        let bounds = scene.bounds(id);
        match scene.kind(id) {
            NodeKind::RichText { text: rich } => {
                let (wrap, inset) = scene.text_wrap(id, bounds.width);
                let layout = scene.shape_rich_text(rich, Some(wrap));
                let runs = rich.runs();
                for (i, run) in runs.iter().enumerate() {
                    if run.background.is_some()
                        && i + 1 < runs.len()
                        && runs[i + 1].background == run.background
                        && run.padding[0] > 0.
                        && runs[i + 1].padding[0] == 0.
                    {
                        // Icon placeholder run immediately followed by the chip's label run.
                        if let Some(sel) = layout.selection(run.range.clone()).into_iter().next() {
                            let label_sel = layout.selection(runs[i + 1].range.clone()).into_iter().next();
                            chip_runs.push((
                                zgx::scene::Rect::new(
                                    bounds.x + inset + sel.x,
                                    bounds.y + sel.y,
                                    sel.width,
                                    sel.height,
                                ),
                                label_sel.map(|r| bounds.x + inset + r.x),
                                run.font_size,
                                run.padding,
                            ));
                        }
                    }
                }
            }
            NodeKind::Panel { .. } => {
                // Chip icon well: square panel wrapping a single SVG icon.
                let children = scene.children(id);
                if children.len() == 1
                    && matches!(scene.kind(children[0]), NodeKind::Svg(_))
                    && (bounds.width - bounds.height).abs() < 0.5
                    && bounds.width >= 14.
                    && bounds.width <= 22.
                {
                    wells.push(bounds);
                }
            }
            _ => {}
        }
        stack.extend_from_slice(scene.children(id));
    }

    assert!(!wells.is_empty(), "expected at least one chip icon well in #lu1");
    assert_eq!(
        wells.len(),
        chip_runs.len(),
        "expected one icon placeholder run per chip icon well"
    );

    wells.sort_by(|a, b| a.y.total_cmp(&b.y).then_with(|| a.x.total_cmp(&b.x)));
    chip_runs.sort_by(|a, b| a.0.y.total_cmp(&b.0.y).then_with(|| a.0.x.total_cmp(&b.0.x)));

    for (well, (slot, label_x, font_size, pad)) in wells.iter().zip(chip_runs.iter()) {
        // Pill vertical band in zgx-text: 0.92*font_size above baseline + 0.24*font_size below + 2*pad[1].
        let pill_height = font_size * (0.92 + 0.24) + 2. * pad[1];
        let left_gap = well.x - slot.x;
        let right_gap = label_x.map(|lx| lx - (well.x + well.width)).unwrap_or(left_gap);
        let vertical_gap = (pill_height - well.height) / 2.;
        eprintln!(
            "chip geometry: well={:.1}x{:.1} pill_h={:.2} -> left={:.2} right={:.2} vert={:.2}",
            well.width, well.height, pill_height, left_gap, right_gap, vertical_gap
        );
        assert!(
            (left_gap - right_gap).abs() <= 0.5,
            "left_gap ({left_gap:.2}) and right_gap ({right_gap:.2}) should be equal"
        );
        assert!(
            (left_gap - vertical_gap).abs() <= 0.5,
            "left_gap ({left_gap:.2}) and vertical_gap ({vertical_gap:.2}) should be equal"
        );
    }
}

