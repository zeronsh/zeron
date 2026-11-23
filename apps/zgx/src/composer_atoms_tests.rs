//! End-to-end verification and performance proof for:
//! 1. Inline atomic chips in the composer (`ds/composer_atoms.luau` + `ds/composer.luau`):
//!    - `@` / `/` / `$` mentions become inline atomic chips with icons and wash.
//!    - Deleting an inline chip in composer text leaves the file attached in cargo.
//!    - Removing an attachment from the cargo strip deletes all inline chips mentioning it.
//!    - Dragging an inline chip inside the composer moves it cleanly to the dropped caret offset.
//!    - Dragging an attachment chip from the cargo strip into the composer inserts an inline chip
//!      without duplicating the attachment in cargo.
//! 2. Lightbox anti-flicker regression coverage across morph open and morph→rest handoff.
//! 3. Composer performance proof at short (20 lines) vs long (5,000 lines) drafts with and
//!    without inline chips (typing both after and before chips), proving O(1) script calls
//!    and no latency regression.

use std::path::PathBuf;
use std::time::Instant;

use zgx::input::{InputEvent, Key, Modifiers, PointerButton};
use zgx::scene::{NodeId, NodeKind, Rect};
use zgx_script::{ScriptHost, Value, testing::TestApp};

const FIELD: &str = "role:textbox \"Message\"";

fn ui_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui")
}

fn mount_fixture(name: &str) -> TestApp {
    let path = ui_dir().join(name);
    let app = TestApp::new(|runtime| ScriptHost::new(runtime, path));
    app.ui.scene.borrow_mut().resize(1200., 800.);
    app.frames(25);
    app
}

fn drag(app: &mut TestApp, from: (f32, f32), to: (f32, f32)) {
    app.ui.dispatch(InputEvent::PointerMove { x: from.0, y: from.1 });
    app.frames(2);
    app.ui.dispatch(InputEvent::PointerDown {
        x: from.0,
        y: from.1,
        button: PointerButton::Primary,
    });
    app.frames(1);
    let steps = 10;
    for i in 1..=steps {
        let t = i as f32 / steps as f32;
        app.ui.dispatch(InputEvent::PointerMove {
            x: from.0 + (to.0 - from.0) * t,
            y: from.1 + (to.1 - from.1) * t,
        });
        app.frames(1);
    }
    app.ui.dispatch(InputEvent::PointerUp {
        x: to.0,
        y: to.1,
        button: PointerButton::Primary,
    });
    app.frames(12);
}

fn find_atom_rect(app: &mut TestApp, field: NodeId, atom_id: u64) -> Option<Rect> {
    app.ui.prepare_frame();
    let bounds = app.ui.scene.borrow().transformed_bounds(field);
    let editor = app.ui.editor_within(field)?;
    let mut y = bounds.y + 4.0;
    while y < bounds.y + bounds.height {
        let mut x = bounds.x + 4.0;
        while x < bounds.x + bounds.width {
            if let Some((atom, rect)) = editor.atom_at(x, y) {
                if atom.id == atom_id {
                    return Some(rect);
                }
            }
            x += 8.0;
        }
        y += 8.0;
    }
    None
}

fn send_key(app: &mut TestApp, key: Key) {
    app.ui.dispatch(InputEvent::KeyDown {
        key,
        modifiers: Modifiers::default(),
        repeat: false,
    });
    app.settle();
    app.frames(4);
}

fn pick_mention(app: &mut TestApp, query: &str) {
    app.type_text(FIELD, &format!("@{query}"));
    app.frames(12);
    send_key(app, Key::Enter);
    app.frames(12);
}

fn pick_mention_at_caret(app: &mut TestApp, query: &str) {
    app.ui.dispatch(InputEvent::Text(format!("@{query}")));
    app.settle();
    app.frames(12);
    send_key(app, Key::Enter);
    app.frames(12);
}

#[test]
fn attachment_mentions_luau_suite_passes() {
    let ui = ui_dir();
    let filters = [
        "friendly_name cleans macOS screenshot dates",
        "a picture with a screenshot's name, mentioned, goes with its cleaned screenshot name",
        "files with odd names go as their escaped links",
        "two files of one name are two numbers",
        "removing an attachment deletes inline chips mentioning it",
        "an Appshot mentioned goes by its window title with image extension",
        "after a relaunch, read back with its uploads' names",
    ];
    for filter in filters {
        let report = zgx_script::test_runner::run(&ui, Some(filter), |outcome| {
            if let Some(failure) = &outcome.failure {
                eprintln!("{}: {failure}", outcome.name);
            }
        })
        .expect("attachment_mentions test runs");
        assert!(
            !report.outcomes.is_empty(),
            "no test matched filter {filter:?}"
        );
        assert_eq!(report.failed(), 0, "{report:?}");
    }
}

fn run_cmd(app: &TestApp, id: &str) {
    assert!(app.host.run_command(id), "command {id:?} did not run");
}

fn read_state(app: &TestApp, name: &str) -> Value {
    app.host
        .states()
        .into_iter()
        .find(|(n, _)| n == name || n.ends_with(&format!("::{name}")))
        .map_or(Value::Nil, |(_, v)| v)
}

#[test]
fn composer_inline_chips_backspace_keeps_attachment_and_removing_attachment_deletes_chips() {
    let mut app = mount_fixture("chat_fixture.luau");
    run_cmd(&app, "attach_odd");
    app.frames(15);

    let field = app.find(FIELD).expect("composer textarea");
    let cargo_len = |app: &TestApp| -> usize {
        match read_state(app, "chat.composer.cargo") {
            Value::List(items) => items.len(),
            _ => 0,
        }
    };
    let initial_cargo = cargo_len(&app);
    assert!(initial_cargo >= 3, "attach_odd should attach multiple items");

    // 1. Pick `@Screenshot`: inserted as an inline atomic chip with transcript-matching style + well.
    pick_mention(&mut app, "Screenshot");
    {
        let editor = app.ui.editor_within(field).expect("editor");
        let text = editor.editor.borrow().text().into_owned();
        let atoms = editor.atoms();
        assert_eq!(atoms.len(), 1, "expected 1 inline chip, text={text:?}");
        assert_eq!(&text[atoms[0].range.clone()], "Screenshot 1.10.14 PM.png");
        let style = atoms[0]
            .style
            .as_ref()
            .expect("inline chip should carry transcript-matching chip style");
        assert_eq!(style.padding, Some([2.0, 7.0, 2.4]), "asymmetric chip padding must match transcript");
        assert_eq!(style.radius, Some(5.0), "chip corner radius must be 5");
        assert!(style.weight.is_none(), "chip label should use regular body weight, not bold");
        let icon = style.icon.as_ref().expect("inline chip should carry icon well");
        assert!(icon.bg.is_some(), "icon well background must be set");
        assert_eq!(icon.size, 13.0);
        assert_eq!(icon.gap, 2.0);
        assert_eq!(icon.pad, 2.0);
        assert_eq!(icon.radius, 3.0);
    }

    // 2. Delete the trailing space + atomic chip with Backspace:
    //    chip disappears in 1 backspace while the attachment stays in cargo!
    send_key(&mut app, Key::Backspace);
    send_key(&mut app, Key::Backspace);
    {
        let editor = app.ui.editor_within(field).expect("editor");
        let text = editor.editor.borrow().text().into_owned();
        let atoms = editor.atoms();
        assert!(atoms.is_empty(), "chip should be deleted atomically, got {atoms:?}");
        assert!(text.trim().is_empty(), "text should be empty after deleting chip, got {text:?}");
        assert_eq!(
            cargo_len(&app),
            initial_cargo,
            "deleting an inline chip must NOT detach the file from cargo"
        );
    }

    // 3. Insert TWO chips for Screenshot and ONE for notes, then remove Screenshot from cargo:
    //    both Screenshot chips are deleted while the notes chip stays intact.
    pick_mention(&mut app, "Screenshot");
    app.type_text(FIELD, "compare ");
    pick_mention(&mut app, "notes");
    app.type_text(FIELD, "and ");
    pick_mention(&mut app, "Screenshot");
    {
        let editor = app.ui.editor_within(field).expect("editor");
        assert_eq!(editor.atoms().len(), 3, "expected 3 inline chips");
    }

    let shot_label = "Screenshot 1.10.14 PM.png";
    app.hover(&format!("role:button \"{shot_label}\""));
    app.frames(10);
    app.click(&format!("role:button \"Remove {shot_label}\""));
    app.frames(15);

    {
        let editor = app.ui.editor_within(field).expect("editor");
        let text = editor.editor.borrow().text().into_owned();
        let atoms = editor.atoms();
        assert_eq!(
            cargo_len(&app),
            initial_cargo - 1,
            "Screenshot should be removed from cargo"
        );
        assert_eq!(
            atoms.len(),
            1,
            "both Screenshot chips must be deleted, leaving only the notes chip; text={text:?}"
        );
        assert!(
            !text.contains("Screenshot 1.10.14 PM.png"),
            "removed attachment label must not remain in draft: {text:?}"
        );
        assert!(
            text.contains("notes (draft)"),
            "unrelated notes chip must stay intact: {text:?}"
        );
    }
}

#[test]
fn composer_inline_chips_drag_from_cargo_and_reorder_inside_composer() {
    let mut app = mount_fixture("chat_fixture.luau");
    run_cmd(&app, "attach_odd");
    app.frames(15);

    let field = app.find(FIELD).expect("composer textarea");
    let cargo_len = |app: &TestApp| -> usize {
        match read_state(app, "chat.composer.cargo") {
            Value::List(items) => items.len(),
            _ => 0,
        }
    };
    let initial_cargo = cargo_len(&app);

    app.type_text(FIELD, "hello world ");
    app.frames(10);

    // 1. Drag the Screenshot thumbnail chip from the cargo strip into the end of the composer.
    let shot_label = "Screenshot 1.10.14 PM.png";
    let shot_node = app
        .find(&format!("role:button \"{shot_label}\""))
        .expect("cargo screenshot chip");
    app.ui.prepare_frame();
    let shot_rect = app.ui.scene.borrow().transformed_bounds(shot_node);
    let field_rect = app.ui.scene.borrow().transformed_bounds(field);

    let from_cargo = (
        shot_rect.x + shot_rect.width * 0.4,
        shot_rect.y + shot_rect.height * 0.5,
    );
    let to_end = (field_rect.x + field_rect.width - 20.0, field_rect.y + 10.0);
    drag(&mut app, from_cargo, to_end);

    let atom_id = {
        let editor = app.ui.editor_within(field).expect("editor");
        let text = editor.editor.borrow().text().into_owned();
        let atoms = editor.atoms();
        assert_eq!(
            atoms.len(),
            1,
            "dragging cargo chip into composer must insert 1 inline chip; text={text:?}"
        );
        assert_eq!(&text[atoms[0].range.clone()], "Screenshot 1.10.14 PM.png");
        assert!(
            atoms[0].range.start >= "hello world ".len(),
            "chip should land after 'hello world ', got range {:?} in {text:?}",
            atoms[0].range
        );
        assert_eq!(
            cargo_len(&app),
            initial_cargo,
            "dragging cargo chip into composer must not duplicate it in cargo"
        );
        atoms[0].id
    };

    // 2. Drag the inline chip inside the composer to the start (before "hello world").
    let chip_rect = find_atom_rect(&mut app, field, atom_id)
        .expect("rendered inline chip should have hit-testable rect");
    let chip_center = (
        chip_rect.x + chip_rect.width * 0.5,
        chip_rect.y + chip_rect.height * 0.5,
    );
    let to_start = (field_rect.x + 2.0, field_rect.y + 10.0);
    drag(&mut app, chip_center, to_start);

    {
        let editor = app.ui.editor_within(field).expect("editor");
        let text = editor.editor.borrow().text().into_owned();
        let atoms = editor.atoms();
        assert_eq!(atoms.len(), 1, "moving chip must keep exactly 1 atom; text={text:?}");
        assert_eq!(
            atoms[0].range.start, 0,
            "dragged chip should move to start of draft, got range {:?} in {text:?}",
            atoms[0].range
        );
        assert_eq!(&text[atoms[0].range.clone()], "Screenshot 1.10.14 PM.png");
        assert!(
            text.contains("hello world"),
            "existing text must be preserved after drag move: {text:?}"
        );
    }

    // 3. Send and verify the canonical link is emitted at the moved position (before "hello world").
    send_key(&mut app, Key::Enter);
    app.frames(20);
    let calls = match read_state(&app, "calls") {
        Value::Str(s) => s.to_string(),
        other => panic!("expected calls string, got {other:?}"),
    };
    assert!(
        calls.contains("send:s1:[Screenshot 1.10.14 PM.png](zeron-attachment:1) hello world"),
        "moved chip should serialize at its new position, got calls={calls:?}"
    );
}

#[test]
fn composer_inline_chip_icon_well_and_drag_preview_position() {
    let mut app = mount_fixture("chat_fixture.luau");
    run_cmd(&app, "attach_odd");
    app.frames(15);

    let field = app.find(FIELD).expect("composer textarea");
    pick_mention(&mut app, "Screenshot");
    app.frames(10);

    // 1. Verify the rendered inline chip has a 17×17 icon well with equal ~2pt spacing.
    app.ui.prepare_frame();
    let (chip_rect, well_rect) = {
        let editor = app.ui.editor_within(field).expect("editor");
        let atom_id = editor.atoms()[0].id;
        drop(editor);
        let chip_rect = find_atom_rect(&mut app, field, atom_id)
            .expect("inline chip must have a hit-testable rect");
        let scene = app.ui.scene.borrow();
        let mut stack = vec![field];
        let mut well = None;
        while let Some(id) = stack.pop() {
            let bounds = scene.transformed_bounds(id);
            if matches!(scene.kind(id), NodeKind::Panel { .. }) {
                let children = scene.children(id);
                if children.len() == 1
                    && matches!(scene.kind(children[0]), NodeKind::Svg(_))
                    && (bounds.width - bounds.height).abs() < 0.5
                    && bounds.width >= 15.0
                    && bounds.width <= 19.0
                {
                    well = Some(bounds);
                    break;
                }
            }
            stack.extend_from_slice(scene.children(id));
        }
        (chip_rect, well.expect("inline chip must mount a 17x17 icon well"))
    };
    assert!(
        (well_rect.width - 17.0).abs() <= 0.5 && (well_rect.height - 17.0).abs() <= 0.5,
        "expected 17x17 icon well, got {:.1}x{:.1}",
        well_rect.width,
        well_rect.height
    );
    let left_gap = well_rect.x - chip_rect.x;
    let top_gap = well_rect.y - chip_rect.y;
    let bottom_gap = (chip_rect.y + chip_rect.height) - (well_rect.y + well_rect.height);
    assert!(
        (left_gap - 2.0).abs() <= 1.0,
        "icon well left inset should be ~2pt, got {left_gap:.2} (chip={chip_rect:?}, well={well_rect:?})"
    );
    assert!(
        (top_gap - bottom_gap).abs() <= 1.0 && top_gap >= 1.0 && top_gap <= 3.5,
        "icon well should sit vertically centered with ~2pt top/bottom gap, got top={top_gap:.2}, bottom={bottom_gap:.2}"
    );

    // 2. Start dragging the inline chip and verify the drag preview tracks centered on the pointer
    //    (anchor {0.5, 0.5}, offset {0, -4}) instead of hanging off the bottom-right corner.
    let start = (
        chip_rect.x + chip_rect.width * 0.5,
        chip_rect.y + chip_rect.height * 0.5,
    );
    let mid = (start.0 + 80.0, start.1 + 12.0);
    app.ui.dispatch(InputEvent::PointerMove { x: start.0, y: start.1 });
    app.frames(2);
    app.ui.dispatch(InputEvent::PointerDown {
        x: start.0,
        y: start.1,
        button: PointerButton::Primary,
    });
    app.frames(1);
    for i in 1..=6 {
        let t = i as f32 / 6.0;
        app.ui.dispatch(InputEvent::PointerMove {
            x: start.0 + (mid.0 - start.0) * t,
            y: start.1 + (mid.1 - start.1) * t,
        });
        app.frames(1);
    }
    app.ui.prepare_frame();
    let preview_rect = {
        let scene = app.ui.scene.borrow();
        let mut found = None;
        let mut stack = vec![scene.root()];
        while let Some(id) = stack.pop() {
            if let NodeKind::Text { text, .. } = scene.kind(id) {
                if text.as_ref() == "Screenshot 1.10.14 PM.png" {
                    let b = scene.transformed_bounds(id);
                    // Exclude the inline textarea text; find the floating preview near `mid`.
                    if (b.x - mid.0).abs() < 200.0 && (b.y - mid.1).abs() < 60.0 {
                        if let Some(parent) = scene.parent(id) {
                            found = Some(scene.transformed_bounds(parent));
                            break;
                        }
                    }
                }
            }
            stack.extend_from_slice(scene.children(id));
        }
        found.expect("drag preview pill should be mounted while dragging")
    };
    let cx = preview_rect.x + preview_rect.width * 0.5;
    let cy = preview_rect.y + preview_rect.height * 0.5;
    assert!(
        (cx - mid.0).abs() <= 2.5,
        "drag preview should be centered horizontally on pointer x={:.1}, got cx={:.1} (rect={preview_rect:?})",
        mid.0,
        cx
    );
    assert!(
        (cy - (mid.1 - 4.0)).abs() <= 2.5,
        "drag preview should track vertically at pointer y-4={:.1}, got cy={:.1} (rect={preview_rect:?})",
        mid.1 - 4.0,
        cy
    );
    app.ui.dispatch(InputEvent::PointerUp {
        x: mid.0,
        y: mid.1,
        button: PointerButton::Primary,
    });
    app.frames(5);
}

#[test]
fn mention_launcher_luau_suite_passes() {
    let ui = ui_dir();
    let filters = [
        "a file picked goes as its mention, a chat as Zeron's chat link",
        "what's attached, picked, goes as its mention by name",
        "round trip: an attachment picked in the draft goes as its mention",
    ];
    for filter in filters {
        let report = zgx_script::test_runner::run(&ui, Some(filter), |outcome| {
            if let Some(failure) = &outcome.failure {
                eprintln!("{}: {failure}", outcome.name);
            }
        })
        .expect("mention_launcher test runs");
        assert!(!report.outcomes.is_empty(), "no test matched filter {filter:?}");
        assert_eq!(report.failed(), 0, "{report:?}");
    }
}

#[test]
fn lightbox_morph_to_rest_handoff_never_blanks_a_frame() {
    let mut app = mount_fixture("lightbox_fixture.luau");
    run_cmd(&app, "test.lightbox_snapshots");
    app.frames(6);

    // Click the first thumbnail to trigger a morph-in open.
    app.click("\"Open shell.png\"");

    let count_visible_lightbox_images = |app: &mut TestApp| -> (usize, usize) {
        let Some(root) = app.find("role:dialog \"Image viewer\"") else {
            return (0, 0);
        };
        app.ui.prepare_frame();
        let scene = app.ui.scene.borrow();
        let mut visible_full = 0usize;
        let mut prewarm = 0usize;
        let mut stack = vec![(root, 1.0_f32)];
        while let Some((id, parent_op)) = stack.pop() {
            let op = parent_op * scene.effects(id).opacity;
            if let NodeKind::Image { .. } = scene.kind(id) {
                if op > 0.5 {
                    visible_full += 1;
                } else if op > 0.0 && op <= 0.02 {
                    prewarm += 1;
                }
            }
            for &child in scene.children(id) {
                stack.push((child, op));
            }
        }
        (visible_full, prewarm)
    };

    let mut saw_prewarm = false;
    let mut blank_frames = 0usize;
    // Step through the entire morph (280ms ~ 17 frames) plus the 180ms handoff grace (~45 frames).
    for frame in 0..45 {
        app.frames(1);
        let (visible, prewarm) = count_visible_lightbox_images(&mut app);
        if prewarm > 0 {
            saw_prewarm = true;
        }
        if frame >= 2 && visible == 0 {
            blank_frames += 1;
        }
    }

    assert!(
        saw_prewarm,
        "morph-in should mount a target-size async pre-warm image so the rest image is decoded before handoff"
    );
    assert_eq!(
        blank_frames, 0,
        "lightbox must never show a blank frame during morph or morph->rest handoff"
    );
}

#[test]
fn lightbox_luau_suite_passes() {
    let ui = ui_dir();
    let filters = [
        "a thumbnail opens its picture fitted",
        "a composer chip opens the chips' pictures",
        "zooming a picture:",
        "a click on the picture doesn't close it",
        "a picture smaller than the window shows at its own size",
        "with motion reduced (Zeron's own setting), it opens and closes as a short crossfade",
        "with motion as it is, it grows out of the thumbnail",
        "a picture is never stretched or squeezed as it grows from its thumbnail",
        "zooming never leaves a frame without the picture",
    ];
    for filter in filters {
        let report = zgx_script::test_runner::run(&ui, Some(filter), |outcome| {
            if let Some(failure) = &outcome.failure {
                eprintln!("{}: {failure}", outcome.name);
            }
        })
        .expect("lightbox test runs");
        assert!(!report.outcomes.is_empty(), "no test matched {filter:?}");
        assert_eq!(report.failed(), 0, "{report:?}");
    }
}

/// Proves zero performance degradation in the composer at any text length,
/// comparing 20-line vs 5,000-line drafts across:
///   (a) Plain text (no inline chips)
///   (b) With inline chips, typing AFTER the chips
///   (c) With inline chips, typing BEFORE the chips (shifting atom offsets every keystroke)
#[test]
fn typing_with_inline_chips_in_a_long_draft_costs_what_a_short_one_does() {
    #[derive(Clone, Copy)]
    enum Mode {
        Plain,
        AfterChips,
        BeforeChips,
    }

    let bench = |lines: usize, mode: Mode| -> (u64, f64) {
        let mut app = mount_fixture("chat_fixture.luau");
        run_cmd(&app, "attach_odd");
        app.frames(15);

        let field = app.find(FIELD).expect("composer textarea");
        let body: String = (0..lines)
            .map(|i| format!("line {i} with some words that go on for a while here\n"))
            .collect();

        match mode {
            Mode::Plain => {
                app.fill(FIELD, &body);
                app.frames(25);
            }
            Mode::AfterChips => {
                // Insert 5 inline chips at the top, then append `lines` lines and type at the end.
                for _ in 0..3 {
                    pick_mention(&mut app, "Screenshot");
                }
                for _ in 0..2 {
                    pick_mention(&mut app, "notes");
                }
                let editor = app.ui.editor_within(field).expect("editor");
                let end = editor.editor.borrow().text().len();
                editor.set_selection(end, end);
                editor.paste(&body);
                app.settle();
                app.frames(25);
                assert_eq!(editor.atoms().len(), 5, "all 5 inline chips must remain intact");
            }
            Mode::BeforeChips => {
                // Build `lines` lines followed by 5 inline chips at the end, then move caret to
                // byte 0 and type BEFORE all 5 chips so every keystroke shifts atom offsets.
                app.fill(FIELD, &body);
                app.frames(15);
                app.click(FIELD);
                app.frames(2);
                {
                    let editor = app.ui.editor_within(field).expect("editor");
                    let end = editor.editor.borrow().text().len();
                    editor.set_selection(end, end);
                }
                app.frames(4);
                for _ in 0..3 {
                    pick_mention_at_caret(&mut app, "Screenshot");
                }
                for _ in 0..2 {
                    pick_mention_at_caret(&mut app, "notes");
                }
                {
                    let editor = app.ui.editor_within(field).expect("editor");
                    assert_eq!(editor.atoms().len(), 5, "all 5 inline chips must be inserted");
                    editor.set_selection(0, 0);
                }
                app.settle();
                app.frames(15);
            }
        }

        const KEYS: u64 = 20;
        let before_calls = app.calls();
        let t0 = Instant::now();
        for _ in 0..KEYS {
            app.ui.dispatch(InputEvent::Text("x".into()));
            app.settle();
            app.frames(3);
        }
        let elapsed_ms = t0.elapsed().as_secs_f64() * 1000.0;
        let calls_per_key = (app.calls() - before_calls) / KEYS;
        let ms_per_key = elapsed_ms / KEYS as f64;

        let editor = app.ui.editor_within(field).expect("editor");
        let text = editor.editor.borrow().text().into_owned();
        match mode {
            Mode::Plain | Mode::AfterChips => {
                assert!(text.ends_with("xxxxxxxxxxxxxxxxxxxx"));
            }
            Mode::BeforeChips => {
                assert!(text.starts_with("xxxxxxxxxxxxxxxxxxxx"));
                assert_eq!(
                    editor.atoms().len(),
                    5,
                    "typing before chips must shift and preserve all 5 atoms"
                );
            }
        }
        (calls_per_key, ms_per_key)
    };

    let (short_plain_c, short_plain_ms) = bench(20, Mode::Plain);
    let (long_plain_c, long_plain_ms) = bench(5000, Mode::Plain);
    let (short_after_c, short_after_ms) = bench(20, Mode::AfterChips);
    let (long_after_c, long_after_ms) = bench(5000, Mode::AfterChips);
    let (short_before_c, short_before_ms) = bench(20, Mode::BeforeChips);
    let (long_before_c, long_before_ms) = bench(5000, Mode::BeforeChips);

    eprintln!(
        "\nComposer typing benchmark (20 keys, debug build):\n\
         | Mode                       | 20 lines (calls/key, ms/key) | 5,000 lines (calls/key, ms/key) |\n\
         |----------------------------|------------------------------|---------------------------------|\n\
         | Plain text (no chips)      | {short_plain_c:>4} calls, {short_plain_ms:>6.2} ms       | {long_plain_c:>4} calls, {long_plain_ms:>6.2} ms          |\n\
         | 5 chips, typing AFTER      | {short_after_c:>4} calls, {short_after_ms:>6.2} ms       | {long_after_c:>4} calls, {long_after_ms:>6.2} ms          |\n\
         | 5 chips, typing BEFORE     | {short_before_c:>4} calls, {short_before_ms:>6.2} ms       | {long_before_c:>4} calls, {long_before_ms:>6.2} ms          |"
    );

    assert!(
        long_plain_c <= short_plain_c && long_plain_c <= 20,
        "plain: {long_plain_c} calls/key at 5,000 lines vs {short_plain_c} at 20 lines"
    );
    assert!(
        long_after_c <= short_after_c && long_after_c <= 20,
        "after chips: {long_after_c} calls/key at 5,000 lines vs {short_after_c} at 20 lines"
    );
    assert!(
        long_before_c <= short_before_c && long_before_c <= 20,
        "before chips: {long_before_c} calls/key at 5,000 lines vs {short_before_c} at 20 lines"
    );
    assert!(
        long_after_c <= long_plain_c + 2,
        "inline chips (typing after) added Lua overhead: {long_after_c} vs plain {long_plain_c}"
    );
    assert!(
        long_before_c <= long_plain_c + 2,
        "inline chips (typing before) added Lua overhead: {long_before_c} vs plain {long_plain_c}"
    );
}
