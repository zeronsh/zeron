//! The orchestration board under a hand (orch_board_places_fixture.luau,
//! ds/orch_board), in raw pointer events. A drag never sets a column: the
//! engine says where each card stands. A card carried carries a reference
//! to it (for a composer to take, ds/orch_ref); let go on another column
//! it does what its menu does there (ds/orch_board_drop), and the card
//! goes where the engine then puts it; let go on its own column, or off
//! the board, it asks nothing. A press let go where it was, or after a
//! wobble, opens its detail beside the board.

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

/// Pressed on `card`'s left and carried to `end`: slowly (30 moves) or in
/// a flick (one). Whether a drag was under way.
fn carry(app: &mut TestApp, card: &str, end: (f32, f32), slow: bool) -> bool {
    let from = rect(app, card);
    let start = (from.x + 30., from.y + from.height / 2.);
    let path: Vec<(f32, f32)> = if slow {
        (1..=30)
            .map(|i| {
                let t = i as f32 / 30.;
                (start.0 + (end.0 - start.0) * t, start.1 + (end.1 - start.1) * t)
            })
            .collect()
    } else {
        vec![end]
    };
    gesture(app, start, &path)
}

/// A card carried from Running to Done, slowly and in a flick: marked done
/// (its menu's Mark done, once each), and in Done once the engine's frame
/// says so; nothing opened, and every card shown again once let go.
#[test]
fn orchestration_board_a_card_carried_to_done_is_marked_done() {
    for slow in [true, false] {
        let mut app = mount(1500., 1000.);
        let to = rect(&mut app, "#orch-col-done");
        let end = (to.x + to.width / 2., to.y + to.height - 8.);
        assert!(carry(&mut app, "#orch-col-running >> #orch-card-it-chat", end, slow), "a card carried is a drag");
        assert_eq!(state(&app, "calls"), "close:it-chat:false", "slow: {slow}");
        app.frames(80);
        assert!(has(&mut app, "#orch-col-done >> #orch-card-it-chat"), "slow: {slow}");
        assert!(!has(&mut app, "#orch-board-detail"), "let go elsewhere, it isn't a click");
        assert!(app.opacity("#orch-card-it-tool") > 0.99);
        assert!(app.error().is_none(), "{:?}", app.error());
    }
}

/// A card let go on its own column, or off every column (the tabs over
/// them), asks nothing: it stays in Running, shown again, nothing opened.
#[test]
fn orchestration_board_a_card_let_go_on_its_own_column_or_off_the_board_stays_put() {
    let mut app = mount(1500., 1000.);
    let own = rect(&mut app, "#orch-card-it-ios");
    let tabs = rect(&mut app, "#orch-board-tabs");
    for end in [(own.x + own.width / 2., own.y + own.height / 2.), (tabs.x + tabs.width + 40., tabs.y + tabs.height / 2.)] {
        assert!(carry(&mut app, "#orch-col-running >> #orch-card-it-chat", end, true));
        assert_eq!(state(&app, "calls"), "", "nothing asked of the host");
        assert!(has(&mut app, "#orch-col-running >> #orch-card-it-chat"));
        assert!(!has(&mut app, "#orch-board-detail"), "let go elsewhere, it isn't a click");
        // (Carried no more: shown at rest.)
        assert!(app.opacity("#orch-card-it-chat") > 0.99);
    }
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
