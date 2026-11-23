//! The orchestration board under a hand (orch_board_places_fixture.luau,
//! ds/orch_board), in raw pointer events. Nothing on it is moved by a
//! drag: the engine says where each card stands. A card pressed and
//! carried to another column carries only a reference to it (for a
//! composer to take, ds/orch_ref), asks nothing and stays where its row
//! puts it; a press let go where it was, or after a wobble, opens its
//! detail beside the board.

use zgx::input::{InputEvent, Modifiers, PointerButton};
use zgx::scene::Rect;
use zgx_script::{Value, testing::TestApp};

fn mount(w: f32, h: f32) -> TestApp {
    let app = TestApp::new(|runtime| crate::fixture_tests::host(runtime, "orch_board_places_fixture.luau"));
    app.ui.scene.borrow_mut().resize(w, h);
    app.frames(30);
    app
}

/// A state of the fixture's, as text ("" when unset or not a string).
fn state(app: &TestApp, name: &str) -> String {
    app.host
        .states()
        .into_iter()
        .find(|(n, _)| n == name)
        .map(|(_, v)| match v {
            Value::Str(s) => s.to_string(),
            _ => String::new(),
        })
        .unwrap_or_default()
}

fn rect(app: &mut TestApp, target: &str) -> Rect {
    let id = app.target(target).unwrap_or_else(|e| panic!("{target}: {e}"));
    app.ui.prepare_frame();
    app.ui.scene.borrow().hit_bounds(id)
}

fn has(app: &mut TestApp, target: &str) -> bool {
    app.find(target).is_some()
}

fn send(app: &mut TestApp, event: InputEvent) {
    app.ui.dispatch_with_modifiers(event, Modifiers::default());
}

const P: PointerButton = PointerButton::Primary;

/// Pressed at `from`, moved through `path` with a frame after each move,
/// let go at the last point. Whether a drag was under way at any step.
fn gesture(app: &mut TestApp, from: (f32, f32), path: &[(f32, f32)]) -> bool {
    let (x, y) = from;
    let mut dragged = false;
    send(app, InputEvent::PointerMove { x, y });
    send(app, InputEvent::PointerDown { x, y, button: P });
    for &(x, y) in path {
        send(app, InputEvent::PointerMove { x, y });
        app.frames(1);
        dragged |= app.ui.input.drag_state().is_some();
    }
    let (x, y) = *path.last().unwrap_or(&from);
    send(app, InputEvent::PointerUp { x, y, button: P });
    app.ui.input.forget_clicks();
    app.settle();
    app.frames(20);
    dragged
}

/// A card carried from Running to Done, slowly and in a flick: a reference
/// drag and nothing else (no call, the card still in Running, nothing
/// opened).
#[test]
fn orchestration_board_a_card_carried_away_stays_put() {
    let mut app = mount(1500., 1000.);
    let from = rect(&mut app, "#orch-col-running >> #orch-card-it-chat");
    let to = rect(&mut app, "#orch-col-done");
    let start = (from.x + 30., from.y + from.height / 2.);
    let end = (to.x + to.width / 2., to.y + to.height - 8.);
    let slow: Vec<(f32, f32)> = (1..=30)
        .map(|i| {
            let t = i as f32 / 30.;
            (start.0 + (end.0 - start.0) * t, start.1 + (end.1 - start.1) * t)
        })
        .collect();
    // (Its one drag is a reference to it: let go on the board, it lands nowhere.)
    assert!(gesture(&mut app, start, &slow), "a card carried is a reference drag");
    gesture(&mut app, start, &[end]);
    assert_eq!(state(&app, "calls"), "", "nothing asked of the host");
    assert!(has(&mut app, "#orch-col-running >> #orch-card-it-chat"));
    assert!(!has(&mut app, "#orch-col-done >> #orch-card-it-chat"));
    assert!(!has(&mut app, "#orch-board-detail"), "let go elsewhere, it isn't a click");
    // (Every card at rest and shown.)
    assert!(app.opacity("#orch-card-it-chat") > 0.99);
    assert!(app.error().is_none(), "{:?}", app.error());
}

/// A press let go where it was, or after a wobble under the threshold, is a
/// click: the card's detail opens beside the board; its thread is not
/// opened and nothing is asked of the host.
#[test]
fn orchestration_board_a_click_opens_the_detail() {
    let mut app = mount(1500., 1000.);
    let from = rect(&mut app, "#orch-card-it-chat");
    let at = (from.x + 30., from.y + from.height / 2.);
    gesture(&mut app, at, &[]);
    assert_eq!(state(&app, "board_selected"), "it-chat");
    assert!(has(&mut app, "#orch-board-detail"));
    assert_eq!(state(&app, "calls"), "", "a click opens the card, never its thread");
    let other = rect(&mut app, "#orch-card-it-tool");
    let at = (other.x + 30., other.y + other.height / 2.);
    gesture(&mut app, at, &[(at.0 + 2., at.1 + 1.)]);
    assert_eq!(state(&app, "board_selected"), "it-tool");
    assert_eq!(state(&app, "calls"), "");
}
