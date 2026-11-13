//! The code editor's sideways scroll: one offset, the textarea's own, that
//! the caret and a sideways wheel or swipe both move (ui/ds/code_editor's
//! `on_wheel`). A caret at a long line's end scrolls the code to it, and a
//! sideways swipe brings it back, whatever drove it there; the editor's
//! window of lines moving (a long vertical scroll) keeps it. On
//! ui/files_fixture.luau over ui/files_stub.luau's `zwide.txt` (600 lines
//! of some 310 characters).
//!
//! The offset isn't readable from outside, so it's read as the caret's
//! column a click at the code's left edge lands on (the status bar's
//! "Ln n, Col m").

use zgx::input::{InputEvent, Modifiers, PointerButton};
use zgx_script::{ScriptHost, testing::TestApp};

const EDITOR: &str = "role:textbox \"zwide.txt\"";

thread_local! {
    /// The code's left edge in the window, as it stands unscrolled.
    static LEFT: std::cell::Cell<f32> = const { std::cell::Cell::new(0.) };
}

fn mount() -> TestApp {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui/files_fixture.luau");
    let mut app = TestApp::new(|runtime| ScriptHost::new(runtime, path));
    app.ui.scene.borrow_mut().resize(900., 600.);
    app.frames(30);
    assert!(app.wait_until(|app| app.find("role:treeitem \"zwide.txt\"").is_some()), "the tree never listed zwide.txt");
    app.click("role:treeitem \"zwide.txt\"");
    assert!(frames_until(&mut app, |app| app.find(EDITOR).is_some()), "zwide.txt never opened");
    // (A script error's dialog covers the window's foot, and takes the
    // clicks meant for the code there.)
    if app.find("role:dialog \"Script error\"").is_some() {
        panic!("the fixture has a script error: {}", app.text("role:dialog \"Script error\""));
    }
    let left = text_left(&mut app);
    LEFT.with(|l| l.set(left));
    takes_clicks(&mut app);
    app
}

/// Until the editor takes a click (it takes none in its first moments,
/// coming in): one well down the code moves the caret off the first line.
fn takes_clicks(app: &mut TestApp) {
    for _ in 0..100 {
        let view = app.bounds("#file-viewer");
        click_at(app, LEFT.with(|l| l.get()) + 20., view.y + 300.);
        if caret(app).0 > 1 {
            return;
        }
        settle_for(app, 50);
    }
    panic!("the editor never took a click");
}

/// Frames over `ms` of real time: what's timed settles (the editor coming
/// in takes no click in its first moments, and frames alone don't pass
/// time).
fn settle_for(app: &mut TestApp, ms: u64) {
    let until = std::time::Instant::now() + std::time::Duration::from_millis(ms);
    while std::time::Instant::now() < until {
        app.frames(2);
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// Frames, and real time for host calls and timers, until `done` holds.
fn frames_until(app: &mut TestApp, mut done: impl FnMut(&mut TestApp) -> bool) -> bool {
    for _ in 0..2000 {
        app.frames(2);
        if done(app) {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(3));
    }
    done(app)
}

/// A press and release at `(x, y)`.
fn click_at(app: &mut TestApp, x: f32, y: f32) {
    let (button, m) = (PointerButton::Primary, Modifiers::default());
    // (Each a click of its own: two on one line would be a double click,
    // which selects a word.)
    app.ui.input.forget_clicks();
    app.ui.dispatch_with_modifiers(InputEvent::PointerMove { x, y }, m);
    app.ui.dispatch_with_modifiers(InputEvent::PointerDown { x, y, button }, m);
    app.ui.dispatch_with_modifiers(InputEvent::PointerUp { x, y, button }, m);
    app.ui.input.forget_clicks();
    app.settle();
    app.frames(4);
}

/// Where the code's text starts in the window: the textarea's left edge at
/// rest (scrolled, it slides left of it, under the clip).
fn text_left(app: &mut TestApp) -> f32 {
    let view = app.bounds("#file-viewer");
    let b = app.bounds(EDITOR);
    // (The gutter's numbers end before it; the textarea's own x is less as it
    // slides.)
    b.x.max(view.x + 40.)
}

/// A wheel or swipe over the code: `dx` sideways, `dy` down.
fn wheel(app: &mut TestApp, dx: f32, dy: f32) {
    let b = app.bounds(EDITOR);
    // (Over the code that shows.)
    let view = app.bounds("#file-viewer");
    let y = (view.y + view.height / 2.).clamp(b.y + 1., b.y + b.height - 1.);
    let x = view.x + view.width / 2.;
    app.ui.dispatch(InputEvent::Scroll { x, y, delta_x: dx, delta_y: dy });
    app.settle();
    app.frames(4);
}

/// The caret's line and column, as the status bar says them.
fn caret(app: &mut TestApp) -> (u32, u32) {
    // (The status bar's own text: a dump of the whole window is cut off past
    // a budget of nodes, and a window of long lines spends it.)
    let text = app.text("role:button \"Go to Line\"");
    let at = text.find("Ln ").unwrap_or_else(|| panic!("no caret in the status bar: {text:?}"));
    let rest = &text[at + 3..];
    let line: u32 = rest.split(',').next().and_then(|s| s.trim().parse().ok()).expect("a line");
    let col: u32 = rest
        .split("Col ")
        .nth(1)
        .and_then(|s| s.split(|c: char| !c.is_ascii_digit()).next())
        .and_then(|s| s.parse().ok())
        .expect("a column");
    (line, col)
}

/// The column a click at the code's left edge lands on, on the first line
/// showing: how far the code is scrolled sideways.
fn left_column(app: &mut TestApp) -> u32 {
    let b = app.bounds(EDITOR);
    let view = app.bounds("#file-viewer");
    // (The first line in view: under the header, in the window.)
    let y = (view.y + 50.).max(b.y + 4.);
    let x = LEFT.with(|l| l.get());
    click_at(app, x + 2., y);
    caret(app).1
}

/// The keys to a line's end and start: ⌘→ and ⌘← on a Mac; End and Home
/// elsewhere, where Ctrl (`cmd`) with an arrow moves a word.
const LINE_END: &str = if cfg!(target_os = "macos") { "cmd+right" } else { "end" };
const LINE_START: &str = if cfg!(target_os = "macos") { "cmd+left" } else { "home" };

#[test]
fn a_sideways_swipe_brings_back_what_the_caret_scrolled_away() {
    let mut app = mount();
    assert_eq!(app.error(), None, "the fixture failed");
    // The caret at the end of a long line: the code scrolls to it.
    let b = app.bounds(EDITOR);
    let view = app.bounds("#file-viewer");
    click_at(&mut app, b.x + 40., (view.y + 50.).max(b.y + 4.));
    app.keys(LINE_END);
    app.frames(10);
    let (_, end_col) = caret(&mut app);
    assert!(end_col > 300, "the caret never reached the line's end (column {end_col})");
    let scrolled = left_column(&mut app);
    assert!(scrolled > 40, "the code never scrolled to the caret (left edge at column {scrolled})");
    // A sideways swipe back: to the line's start.
    app.keys(LINE_END);
    app.frames(6);
    wheel(&mut app, -100_000., 0.);
    assert_eq!(left_column(&mut app), 1, "a sideways swipe didn't bring the code back");
    // And out again, by swipe alone.
    wheel(&mut app, 400., 0.);
    let swiped = left_column(&mut app);
    assert!(swiped > 20, "a sideways swipe didn't move the code out (column {swiped})");
    // The caret to the line's start (⌘←, or Home): back to the start, as an
    // editor does.
    app.keys(LINE_START);
    app.frames(6);
    assert_eq!(left_column(&mut app), 1, "the caret at the line's start didn't bring the code back");
}

#[test]
fn a_moving_window_of_lines_keeps_the_sideways_scroll() {
    let mut app = mount();
    assert_eq!(app.error(), None, "the fixture failed");
    let b = app.bounds(EDITOR);
    let view = app.bounds("#file-viewer");
    click_at(&mut app, b.x + 40., (view.y + 50.).max(b.y + 4.));
    // Sideways a way: the caret to the line's end, then part way back by
    // swipe. (Headless, the textarea learns how far it can scroll as it's
    // drawn or as the caret moves; the caret does here.)
    app.keys(LINE_END);
    app.frames(6);
    wheel(&mut app, -1000., 0.);
    // (Read off where the code stands, not by a click: a caret placed at the
    // left edge is within the editor's caret margin and scrolls the code two
    // columns, so a click to look would move what it measures. It also moved
    // the caret off the line's end, which hid the code going back to it.)
    let before = sideways(&mut app);
    assert!(before > 150., "not scrolled sideways ({before} pt)");
    // Far down and back (the editor's window of lines moves), then sideways
    // where it was, not back at the caret.
    for _ in 0..20 {
        wheel(&mut app, 0., 900.);
    }
    for _ in 0..20 {
        wheel(&mut app, 0., -900.);
    }
    app.frames(20);
    let after = sideways(&mut app);
    assert!((after - before).abs() < 0.5, "the window moving lost the sideways scroll: {before} pt → {after} pt");
}

/// How far the code is scrolled sideways, in points: the textarea slides
/// left of where its text starts at rest.
fn sideways(app: &mut TestApp) -> f32 {
    LEFT.with(|l| l.get()) - app.bounds(EDITOR).x
}

/// Holds a drag from `(x0, y)` with the pointer at `(x1, y)` for about `ms`
/// (the editor carries the code on a timer while it's past the edge), then
/// lets go.
fn drag_and_hold(app: &mut TestApp, x0: f32, x1: f32, y: f32, ms: u64) {
    let (button, m) = (PointerButton::Primary, Modifiers::default());
    app.ui.input.forget_clicks();
    app.ui.dispatch_with_modifiers(InputEvent::PointerMove { x: x0, y }, m);
    app.ui.dispatch_with_modifiers(InputEvent::PointerDown { x: x0, y, button }, m);
    app.frames(2);
    for step in 1..=8 {
        let x = x0 + (x1 - x0) * step as f32 / 8.;
        app.ui.dispatch_with_modifiers(InputEvent::PointerMove { x, y }, m);
        app.frames(1);
    }
    let until = std::time::Instant::now() + std::time::Duration::from_millis(ms);
    while std::time::Instant::now() < until {
        app.ui.dispatch_with_modifiers(InputEvent::PointerMove { x: x1, y }, m);
        app.frames(1);
        std::thread::sleep(std::time::Duration::from_millis(8));
    }
    app.ui.dispatch_with_modifiers(InputEvent::PointerUp { x: x1, y, button }, m);
    app.ui.input.forget_clicks();
    app.settle();
    app.frames(6);
}

#[test]
fn a_selection_dragged_past_the_edge_carries_the_code_sideways() {
    let mut app = mount();
    assert_eq!(app.error(), None, "the fixture failed");
    let left = LEFT.with(|l| l.get());
    let view = app.bounds("#file-viewer");
    let y = view.y + 50.;
    // Pressed near the line's start, dragged past the right edge and held.
    drag_and_hold(&mut app, left + 30., view.x + view.width + 60., y, 600);
    let (_, col) = caret(&mut app);
    let text = app.text("role:button \"Go to Line\"");
    assert!(text.contains("selected"), "nothing was selected: {text:?}");
    let visible_cols = ((view.x + view.width - left) / 7.) as u32;
    assert!(col > visible_cols, "the drag didn't carry the code past what showed (column {col}, about {visible_cols} showing)");
    // And back past the left edge: to the line's start. (A click first: a
    // press inside the selection would drag the text instead.)
    let scrolled = left_column(&mut app);
    assert!(scrolled > 1, "the code isn't scrolled to start with (column {scrolled})");
    let right = view.x + view.width - 20.;
    // (Far past it, so it carries at the quickest, and long enough to come
    // all the way back.)
    drag_and_hold(&mut app, right, view.x + 2., y, 3000);
    assert_eq!(left_column(&mut app), 1, "a drag past the left edge didn't carry the code back");
}

/// How many lines apart two clicks 280 points apart down the code land:
/// 280 over a row's height.
/// (Signed: while the editor is built again at a new size between the two
/// clicks, the second can land above the first.)
fn rows_in_280(app: &mut TestApp) -> i64 {
    let view = app.bounds("#file-viewer");
    let x = LEFT.with(|l| l.get()) + 20.;
    // (Well under the header and the bar over the code's first lines.)
    let y = view.y + 200.;
    click_at(app, x, y);
    let a = caret(app).0;
    click_at(app, x, y + 280.);
    i64::from(caret(app).0) - i64::from(a)
}

#[test]
fn settings_code_font_size_sizes_the_code_and_its_rows() {
    let mut app = mount();
    assert_eq!(app.error(), None, "the fixture failed");
    // At the design system's 13 points, rows of 20.
    // (Give or take one, by where in a row each click falls.)
    let rows = rows_in_280(&mut app);
    assert!((13..=15).contains(&rows), "rows aren't 20 points at the design system's size: 280 points held {rows} rows");
    // Settings' code font size at 18: the editor is built again, its rows
    // scaled with it (28).
    app.host.set_state("settings_code_font_size", zgx_script::Value::from(18.)).expect("the pref");
    app.settle();
    assert!(frames_until(&mut app, |app| app.find(EDITOR).is_some()), "the editor never came back");
    takes_clicks(&mut app);
    let mut seen = 0;
    assert!(
        frames_until(&mut app, |app| app.find(EDITOR).is_some() && {
            seen = rows_in_280(app);
            (9..=11).contains(&seen)
        }),
        "rows didn't grow with the code's size: 280 points held {seen} rows"
    );
}



