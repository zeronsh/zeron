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

/// Carried over a column, the cards from where it would land move down by
/// its height (their transforms; their layout stays), a dashed slot where
/// it would go; let go, every card is where it's laid out, nothing moved.
#[test]
fn orchestration_board_cards_make_room_and_settle() {
    let mut app = mount(1400., 900.);
    let from = card(&mut app, "next", LIVE);
    let first = "#orch-col-blocked >> role:button \"Orchestrator screen\"";
    let second = "#orch-col-blocked >> role:button \"sccache for iOS\"";
    let id2 = app.target(second).unwrap();
    let shown = |app: &mut TestApp, id| {
        app.ui.prepare_frame();
        app.ui.scene.borrow().transformed_bounds(id).y
    };
    let rest = shown(&mut app, id2);
    let one = rect(&mut app, first);
    let (x, y) = (from.x + 30., from.y + from.height / 2.);
    send(&mut app, InputEvent::PointerMove { x, y });
    send(&mut app, InputEvent::PointerDown { x, y, button: P });
    // Between the two cards: past the first's middle, above the second's.
    let (tx, ty) = (one.x + one.width / 2., one.y + one.height - 4.);
    for i in 1..=6 {
        let t = i as f32 / 6.;
        send(&mut app, InputEvent::PointerMove { x: x + (tx - x) * t, y: y + (ty - y) * t });
        app.frames(2);
    }
    app.frames(60);
    assert!(has(&mut app, "#orch-col-blocked >> .orch-column-drop"), "a slot where it would land");
    let room = from.height + 4.;
    let moved = shown(&mut app, id2);
    assert!((moved - rest - room).abs() < 1.5, "moved down by the carried card's height: {moved} vs {rest}, {room}");
    // (The first stays: it lands after it.)
    let id1 = app.target(first).unwrap();
    assert!((shown(&mut app, id1) - one.y).abs() < 0.5);
    send(&mut app, InputEvent::PointerUp { x: tx, y: ty, button: P });
    app.ui.input.forget_clicks();
    app.settle();
    // Let go: the card comes in where the room was, the second where it
    // stood aside, from the first frame (no spring back over the newcomer).
    app.frame();
    let landed = shown(&mut app, id2);
    assert!((landed - moved).abs() < 0.5, "it stays where it stood aside: {landed} vs {moved}");
    app.frames(60);
    assert!((shown(&mut app, id2) - moved).abs() < 0.5);
    let came = card(&mut app, "blocked", LIVE);
    assert!((came.y - (one.y + one.height + 4.)).abs() < 1.5, "the card is where the room was: {} vs {}", came.y, one.y + one.height + 4.);
    assert!(calls(&app).contains("move:it-live:blocked:"), "{}", calls(&app));
    assert!(!has(&mut app, ".orch-column-drop"));
}

/// Pictures of a drag under way (run on purpose, BOARD_SHOTS=<dir>): a card
/// carried among Blocked's, and one over an empty column.
#[test]
#[ignore = "run on purpose: writes pictures of a drag under way"]
fn orchestration_board_drag_pictures() {
    use zgx_gpu::GpuRenderer;
    let out = std::path::PathBuf::from(std::env::var("BOARD_SHOTS").expect("BOARD_SHOTS"));
    std::fs::create_dir_all(&out).unwrap();
    for (name, theme, filter, col, low) in [
        ("drag-among-dark", "zeron-dark", None, "blocked", false),
        ("drag-empty-light", "zeron-light", Some("orc-orch"), "review", true),
    ] {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui/orch_board_fixture.luau");
        let mut app = TestApp::new(|runtime| crate::plugins::lend_design_system(ScriptHost::new(runtime, path.clone()), &path));
        let (w, h, scale) = (1400f32, 640f32, 2f32);
        app.ui.scene.borrow_mut().resize(w, h);
        let mut gpu = GpuRenderer::new((w * scale) as u32, (h * scale) as u32).unwrap();
        gpu.set_scale_factor(scale);
        for font in crate::fonts() {
            font.unwrap().install(&mut gpu.text_system().borrow_mut());
        }
        gpu.install_text(&mut app.ui.scene.borrow_mut());
        app.ui.refresh_text_geometry();
        assert!(app.host.run_command(if theme.ends_with("dark") { "theme_dark" } else { "theme_light" }));
        if let Some(f) = filter {
            app.host.set_state("filter", Value::Str(f.into())).unwrap();
        }
        app.frames(30);
        let from = card(&mut app, "next", LIVE);
        let lane = rect(&mut app, &format!("#orch-col-{col}"));
        let (x, y) = (from.x + 30., from.y + from.height / 2.);
        send(&mut app, InputEvent::PointerMove { x, y });
        send(&mut app, InputEvent::PointerDown { x, y, button: P });
        let (tx, ty) = (lane.x + 40., if low { lane.y + 60. } else { lane.y + 130. });
        for i in 1..=8 {
            let t = i as f32 / 8.;
            send(&mut app, InputEvent::PointerMove { x: x + (tx - x) * t, y: y + (ty - y) * t });
            app.frames(2);
        }
        app.frames(60);
        app.ui.prepare_frame();
        let _ = app.ui.scene.borrow_mut().flush();
        let all = [zgx::scene::Rect { x: 0., y: 0., width: w, height: h }];
        gpu.render(&app.ui.scene.borrow(), &all).unwrap();
        let mut rgba = gpu.readback().unwrap();
        let back: [u8; 3] = if theme.ends_with("dark") { [0x1d, 0x1d, 0x22] } else { [0xe9, 0xe9, 0xee] };
        for px in rgba.chunks_exact_mut(4) {
            let a = px[3] as u32;
            for c in 0..3 {
                px[c] = (px[c] as u32 + back[c] as u32 * (255 - a) / 255).min(255) as u8;
            }
            px[3] = 255;
        }
        let file = std::fs::File::create(out.join(format!("{name}.png"))).unwrap();
        let mut enc = png::Encoder::new(std::io::BufWriter::new(file), (w * scale) as u32, (h * scale) as u32);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header().unwrap().write_image_data(&rgba).unwrap();
        send(&mut app, InputEvent::PointerUp { x: tx, y: ty, button: P });
    }
}
