//! The orchestration board's drag, as a hand does it (ui/orch_board_fixture.luau,
//! ds/orch_board): pressed, moved and let go in raw pointer events. A fast
//! flick (one move far past the card, then let go) moves it as a slow drag
//! does; a drag begun on a card's outcome is still the card's; Escape
//! cancels; a press let go where it was opens the card's thread.

use zgx::input::{InputEvent, Key, Modifiers, PointerButton};
use zgx::scene::Rect;
use zgx_script::{ScriptHost, Value, testing::TestApp};

fn mount(w: f32, h: f32) -> TestApp {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui/orch_board_fixture.luau");
    let app = TestApp::new(|runtime| ScriptHost::new(runtime, path));
    app.ui.scene.borrow_mut().resize(w, h);
    app.frames(30);
    app
}

fn calls(app: &TestApp) -> String {
    app.host
        .states()
        .into_iter()
        .find(|(n, _)| n == "calls")
        .map(|(_, v)| match v {
            Value::Str(s) => s.to_string(),
            other => format!("{other:?}"),
        })
        .unwrap_or_default()
}

fn rect(app: &mut TestApp, target: &str) -> Rect {
    let id = app.target(target).unwrap_or_else(|e| panic!("{target}: {e}"));
    app.ui.prepare_frame();
    app.ui.scene.borrow().hit_bounds(id)
}

fn card(app: &mut TestApp, col: &str, title: &str) -> Rect {
    rect(app, &format!("#orch-col-{col} >> role:button \"{title}\""))
}

fn has(app: &mut TestApp, target: &str) -> bool {
    app.find(target).is_some()
}

fn send(app: &mut TestApp, event: InputEvent) {
    app.ui.dispatch_with_modifiers(event, Modifiers::default());
}

const P: PointerButton = PointerButton::Primary;

/// Pressed at `from`, moved through `path` (a frame after each move when
/// `frames`), let go at the last point.
fn gesture(app: &mut TestApp, from: (f32, f32), path: &[(f32, f32)], frames: bool) {
    let (x, y) = from;
    send(app, InputEvent::PointerMove { x, y });
    send(app, InputEvent::PointerDown { x, y, button: P });
    for &(x, y) in path {
        send(app, InputEvent::PointerMove { x, y });
        if frames {
            app.frames(1);
        }
    }
    let (x, y) = *path.last().unwrap_or(&from);
    send(app, InputEvent::PointerUp { x, y, button: P });
    app.ui.input.forget_clicks();
    app.settle();
    app.frames(20);
}

const LIVE: &str = "Live end-to-end test on a dev engine";

/// A flick: one move from the press far past the card (it leaves the card
/// at once), then let go there, no frame between.
#[test]
fn orchestration_board_fast_flick_moves_the_card() {
    let mut app = mount(1400., 900.);
    let from = card(&mut app, "next", LIVE);
    let to = rect(&mut app, "#orch-col-blocked");
    let start = (from.x + 30., from.y + from.height / 2.);
    let end = (to.x + to.width / 2., to.y + to.height - 8.);
    gesture(&mut app, start, &[end], false);
    let said = calls(&app);
    assert!(said.contains("move:it-live:blocked:"), "a flick moves it: {said:?}");
    assert!(has(&mut app, &format!("#orch-col-blocked >> role:button \"{LIVE}\"")));
    assert!(app.error().is_none(), "{:?}", app.error());
}

/// The same flick with frames drawn between (a real one spans a few).
#[test]
fn orchestration_board_fast_flick_across_frames_moves_the_card() {
    let mut app = mount(1400., 900.);
    let from = card(&mut app, "next", LIVE);
    let to = rect(&mut app, "#orch-col-review");
    let start = (from.x + 30., from.y + 10.);
    let end = (to.x + to.width / 2., to.y + 40.);
    gesture(&mut app, start, &[(start.0 + 3., start.1 + 1.), end], true);
    let said = calls(&app);
    assert!(said.contains("move:it-live:review:"), "{said:?}");
}

/// A slow drag, small steps with frames between, lands where it's let go.
#[test]
fn orchestration_board_slow_drag_moves_the_card() {
    let mut app = mount(1400., 900.);
    let from = card(&mut app, "next", LIVE);
    let to = rect(&mut app, "#orch-col-done");
    let start = (from.x + 30., from.y + from.height / 2.);
    let end = (to.x + to.width / 2., to.y + to.height - 8.);
    let path: Vec<(f32, f32)> = (1..=40)
        .map(|i| {
            let t = i as f32 / 40.;
            (start.0 + (end.0 - start.0) * t, start.1 + (end.1 - start.1) * t)
        })
        .collect();
    gesture(&mut app, start, &path, true);
    let said = calls(&app);
    assert!(said.contains("move:it-live:done:"), "{said:?}");
}

/// Dropped in the gutter between two columns, or on a column's head, it
/// lands in the nearest column all the same.
#[test]
fn orchestration_board_drop_between_columns_lands_in_the_nearest() {
    let mut app = mount(1400., 900.);
    let from = card(&mut app, "next", LIVE);
    let blocked = rect(&mut app, "#orch-col-blocked");
    let start = (from.x + 30., from.y + from.height / 2.);
    // Its head.
    let end = (blocked.x + blocked.width / 2., blocked.y + 8.);
    gesture(&mut app, start, &[end], true);
    assert!(calls(&app).contains("move:it-live:blocked:"), "{}", calls(&app));
}

/// A card pressed on its outcome drags as readily as on its title.
#[test]
fn orchestration_board_drag_from_the_outcome() {
    let mut app = mount(1400., 900.);
    let outcome = rect(&mut app, "#orch-col-review >> .orch-outcome");
    let to = rect(&mut app, "#orch-col-now");
    let start = (outcome.x + outcome.width / 2., outcome.y + outcome.height / 2.);
    let end = (to.x + to.width / 2., to.y + to.height - 8.);
    gesture(&mut app, start, &[end], true);
    let said = calls(&app);
    assert!(said.contains(":active:"), "dragged by its outcome, it moves: {said:?}");
}

/// Escape while carried: nothing moves, and the card is back where it was.
#[test]
fn orchestration_board_escape_cancels() {
    let mut app = mount(1400., 900.);
    let from = card(&mut app, "next", LIVE);
    let to = rect(&mut app, "#orch-col-blocked");
    let (x, y) = (from.x + 30., from.y + from.height / 2.);
    send(&mut app, InputEvent::PointerMove { x, y });
    send(&mut app, InputEvent::PointerDown { x, y, button: P });
    let end = (to.x + to.width / 2., to.y + to.height - 8.);
    for i in 1..=6 {
        let t = i as f32 / 6.;
        send(&mut app, InputEvent::PointerMove { x: x + (end.0 - x) * t, y: y + (end.1 - y) * t });
        app.frames(1);
    }
    assert!(app.ui.input.drag_state().is_some(), "a drag under way");
    app.key_with(Key::Escape, Modifiers::default());
    app.frames(5);
    send(&mut app, InputEvent::PointerUp { x: end.0, y: end.1, button: P });
    app.ui.input.forget_clicks();
    app.settle();
    app.frames(30);
    let said = calls(&app);
    assert!(!said.contains("move:"), "Escape cancels: {said:?}");
    assert!(has(&mut app, &format!("#orch-col-next >> role:button \"{LIVE}\"")));
    // (Nothing left lifted: every card at rest and shown.)
    assert!(app.opacity(&format!("#orch-col-next >> role:button \"{LIVE}\"")) > 0.99);
}

/// A press let go where it was, or after a wobble under the threshold, is a
/// click: the card's thread opens and nothing moves.
#[test]
fn orchestration_board_click_is_not_a_drag() {
    let mut app = mount(1400., 900.);
    let from = card(&mut app, "now", "Control room");
    let at = (from.x + 30., from.y + from.height / 2.);
    gesture(&mut app, at, &[], false);
    let said = calls(&app);
    assert!(said.contains("open:th-ui-control"), "{said:?}");
    assert!(!said.contains("move:"), "{said:?}");
    gesture(&mut app, at, &[(at.0 + 2., at.1 + 1.)], true);
    let said = calls(&app);
    assert_eq!(said.matches("open:th-ui-control").count(), 2, "{said:?}");
    assert!(!said.contains("move:"), "{said:?}");
}

/// Let go in the gutter between two columns, low down where the shorter
/// ones end: it lands in the nearer column (the board takes the drop, not
/// only the cards and columns under the pointer).
#[test]
fn orchestration_board_drop_in_the_gutter_lands_in_the_nearer_column() {
    let mut app = mount(1400., 900.);
    let from = card(&mut app, "next", LIVE);
    let blocked = rect(&mut app, "#orch-col-blocked");
    let review = rect(&mut app, "#orch-col-review");
    let start = (from.x + 30., from.y + from.height / 2.);
    // (A point past Blocked's right edge, nearer it than Review.)
    let gutter = blocked.x + blocked.width + (review.x - blocked.x - blocked.width) * 0.3;
    let end = (gutter, blocked.y + blocked.height - 4.);
    gesture(&mut app, start, &[end], true);
    assert!(calls(&app).contains("move:it-live:blocked:"), "{}", calls(&app));
}
