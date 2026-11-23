//! The board's motion (ui/ds/orch_board_motion.luau) as the scene shows it,
//! where `zgx test` can't look: the frame clock asking for nothing once the
//! moves land; a travelling card opaque within 120 ms, on a solid fill,
//! translated and never scaled; a lane easing with its cards so none hangs
//! outside it; the switch between the board and its list never scaling a
//! box. On the 200-card fixture (ui/orch_board_perf_fixture.luau) and the
//! places one. `ZERON_ZGX_TEST_UI` points it at another tree's `ui/` (no
//! build). Frames are 1/60 s of the test's time.

use zgx::scene::{Color, NodeId, NodeKind, Scene};
use zgx_script::{ScriptHost, Value, testing::TestApp};

fn ui_dir() -> std::path::PathBuf {
    std::env::var_os("ZERON_ZGX_TEST_UI")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui"))
}

fn mount(fixture: &str, w: f32, h: f32, tab: &str) -> TestApp {
    let script = ui_dir().join(fixture);
    let app = TestApp::new(|runtime| crate::plugins::lend_design_system(ScriptHost::new(runtime, script.clone()), &script));
    app.ui.scene.borrow_mut().resize(w, h);
    app.host.set_state("board_tab", Value::from(tab)).unwrap();
    frames(&app, 120);
    app
}

fn board(tab: &str) -> TestApp {
    mount("orch_board_perf_fixture.luau", 1500., 1400., tab)
}

fn frames(app: &TestApp, n: usize) {
    for i in 0..n {
        app.frame();
        assert_eq!(app.error(), None, "script error at frame {i}");
    }
}

/// Of `n` frames, how many the clock asked for after it.
fn wanted(app: &TestApp, n: usize) -> usize {
    (0..n)
        .filter(|_| {
            app.frame();
            app.clock.wants_frame()
        })
        .count()
}

fn run(app: &TestApp, command: &str) {
    assert!(app.host.run_command(command), "no command `{command}`");
}

fn all(app: &mut TestApp, target: &str) -> Vec<NodeId> {
    let _ = app.find("#orch-board");
    let target = zgx_script::driver::parse(target).unwrap();
    zgx_script::driver::find_all(&app.ui, &app.handle, &target).unwrap_or_default()
}

fn one(app: &mut TestApp, target: &str) -> Option<NodeId> {
    let found = all(app, target);
    assert!(found.len() <= 1, "{target}: {} nodes", found.len());
    found.first().copied()
}

/// Its opacity as painted: its own and every ancestor's.
fn opacity(scene: &Scene, node: NodeId) -> f32 {
    std::iter::once(node).chain(scene.ancestors(node)).map(|n| scene.effects(n).opacity).product()
}

/// Whether it, or anything it stands in, is scaled.
fn scaled(scene: &Scene, node: NodeId) -> bool {
    std::iter::once(node).chain(scene.ancestors(node)).any(|n| {
        let s = scene.local_transform(n).scale;
        (s.0 - 1.).abs() > 1e-3 || (s.1 - 1.).abs() > 1e-3
    })
}

/// Its own translation and every ancestor's, summed.
fn moved(scene: &Scene, node: NodeId) -> (f32, f32) {
    std::iter::once(node).chain(scene.ancestors(node)).fold((0., 0.), |(x, y), n| {
        let t = scene.transform(n);
        (x + t.x, y + t.y)
    })
}

fn fill(scene: &Scene, node: NodeId) -> Option<Color> {
    match scene.kind(node) {
        NodeKind::Panel { quad, .. } | NodeKind::Quad(quad) => Some(quad.fill),
        _ => None,
    }
}

/// Done's tab alone: no working ring shows, so a still board asks for no
/// frame at all; while cards move it does, and once they land it stops.
#[test]
fn a_still_board_asks_for_no_frame_before_or_after_its_cards_move() {
    for command in ["motion_one", "motion_four", "motion_many"] {
        let app = board("done");
        assert_eq!(wanted(&app, 60), 0, "a still board asked for frames");
        run(&app, command);
        // Holding (800 ms, staggered) then travelling (350 ms): frames asked for.
        let moving = wanted(&app, 85);
        assert!(moving > 0, "{command}: nothing moved");
        frames(&app, 30);
        assert_eq!(wanted(&app, 60), 0, "{command}: frames asked for after the moves landed");
        eprintln!("zeron-zgx: BOARDMOTION idle {command}: 0 of 60 frames asked for before and after, {moving} of 85 while moving");
    }
}

/// Every column: the working rings step on their own timer, as before.
/// What the board asks for after its moves land is what it asked for
/// before them.
#[test]
fn every_column_asks_for_the_same_frames_before_and_after_its_cards_move() {
    let app = board("all");
    let before = wanted(&app, 120);
    run(&app, "motion_many");
    frames(&app, 120);
    let after = wanted(&app, 120);
    eprintln!("zeron-zgx: BOARDMOTION idle all-columns: {before} of 120 frames asked for before, {after} after");
    assert!(after <= before + 1, "after the moves: {after} frames of 120 asked for, {before} before");
}

/// Decision 39: in flight it is opaque by 120 ms of its travel, on a solid
/// fill, translated and never scaled; landed, it rests on its own fill.
#[test]
fn a_travelling_card_is_opaque_by_120_ms_solid_and_never_scaled() {
    let mut app = board("all");
    let card = "#orch-card-it-37";
    let rest = {
        let node = one(&mut app, card).unwrap();
        fill(&app.ui.scene.borrow(), node).unwrap()
    };
    run(&app, "motion_one");
    // The hold: dimmed where it was.
    frames(&app, 30);
    let held_at;
    {
        let node = one(&mut app, card).unwrap();
        let scene = app.ui.scene.borrow();
        held_at = scene.bounds(node);
        assert!(opacity(&scene, node) < 0.7, "held: {}", opacity(&scene, node));
        assert_eq!(fill(&scene, node), Some(rest), "held: its own fill");
    }
    frames(&app, 17);
    // (Its travel begins in the frame its 800 ms fall in.)
    for _ in 0..4 {
        if one(&mut app, "#orch-col-done >> .orch-card-underlay").is_some() {
            break;
        }
        app.frame();
    }
    // Travel: frames 48 to 69. Each frame: never scaled; the first frames
    // translated; opaque from 120 ms (frame 8 of it) on; solid throughout.
    let mut travelled = false;
    let mut solid = None;
    let mut first = None;
    for f in 0..20 {
        let node = one(&mut app, card).expect("the travelling card");
        {
            let scene = app.ui.scene.borrow();
            assert!(!scaled(&scene, node), "scaled at frame {f} of its travel");
            let (dx, dy) = moved(&scene, node);
            if dx.abs() > 1. || dy.abs() > 1. {
                travelled = true;
            }
            if f == 0 {
                first = Some(scene.bounds(node));
            }
            let o = opacity(&scene, node);
            if f >= 8 {
                assert!(o > 0.995, "{o} at frame {f} of its travel");
            }
            assert_eq!(fill(&scene, node), Some(rest), "its own fill in flight, frame {f}");
        }
        // Under it, an opaque underlay.
        let under = one(&mut app, "#orch-col-done >> .orch-card-underlay").expect("its underlay in flight");
        {
            let scene = app.ui.scene.borrow();
            let c = fill(&scene, under).unwrap();
            assert_eq!(c.3, 255, "its underlay in flight, frame {f}: {c:?}");
            assert!(opacity(&scene, under) > 0.995 || f < 8);
            solid = Some(c);
        }
        app.frame();
    }
    assert!(travelled, "no frame of its travel was translated");
    frames(&app, 10);
    assert!(one(&mut app, "#orch-col-done >> .orch-card-underlay").is_none(), "landed: no underlay");
    let node = one(&mut app, card).unwrap();
    let scene = app.ui.scene.borrow();
    assert_eq!(moved(&scene, node), (0., 0.));
    assert_eq!(fill(&scene, node), Some(rest), "landed: its own fill");
    // It set out from where it held: its first frame nearer there than home.
    let (home, first) = (scene.bounds(node), first.unwrap());
    let d = |a: zgx::scene::Rect, b: zgx::scene::Rect| ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt();
    assert!(d(first, held_at) < d(home, held_at) * 0.5, "it set out at {first:?}, held at {held_at:?}, home {home:?}");
    eprintln!("zeron-zgx: BOARDMOTION fill rest={rest:?} in-flight={solid:?}");
}

/// Decision 40: as Running empties, its lane eases with its cards (the
/// foot translated, nothing scaled), and no card nor the drawer ever shows
/// below the lane's foot.
#[test]
fn a_shrinking_lane_eases_and_nothing_hangs_outside_it() {
    let mut app = mount("orch_board_places_fixture.luau", 1240., 820., "all");
    run(&app, "engine_five");
    let mut eased = false;
    for f in 0..100 {
        app.frame();
        let foot = one(&mut app, "#orch-col-running >> .orch-lane-foot").unwrap();
        let cards = all(&mut app, "#orch-col-running >> .orch-card");
        let drawer = all(&mut app, "#orch-col-running >> .orch-drawer-head");
        let scene = app.ui.scene.borrow();
        let fb = scene.bounds(foot);
        let bottom = fb.y + fb.height;
        if scene.transform(foot).y.abs() > 0.5 {
            eased = true;
        }
        assert!(!scaled(&scene, foot), "the lane's foot is scaled at frame {f}");
        for node in cards.iter().chain(drawer.iter()) {
            if opacity(&scene, *node) < 0.01 {
                continue;
            }
            let b = scene.bounds(*node);
            assert!(b.y + b.height <= bottom + 1.5, "frame {f}: a node ends at {} under the lane's foot at {bottom}", b.y + b.height);
        }
    }
    assert!(eased, "the lane's foot never moved");
    let foot = one(&mut app, "#orch-col-running >> .orch-lane-foot").unwrap();
    assert_eq!(app.ui.scene.borrow().transform(foot).y, 0.);
}

/// Decision 43: the switch never scales a box, the leaving view is gone by
/// 100 ms, and each card in view becomes its row from where it stood.
#[test]
fn the_switch_to_the_list_slides_rows_from_their_cards_never_scaled() {
    let mut app = mount("orch_board_places_fixture.luau", 1240., 820., "all");
    let card = one(&mut app, "#orch-card-it-chat").unwrap();
    let was = app.ui.scene.borrow().bounds(card);
    app.host.set_state("board_view", Value::from("list")).unwrap();
    app.frame();
    {
        // (By its id: it shows nothing yet, which targets pass over.)
        let row = app.find("orch-row-it-chat").expect("its row");
        let scene = app.ui.scene.borrow();
        let b = scene.bounds(row);
        assert!((b.x - was.x).abs() < 2. && (b.y - was.y).abs() < 2., "its row starts at {b:?}, its card stood at {was:?}");
    }
    for f in 0..24 {
        let rows = all(&mut app, ".orch-list-row");
        let cards = all(&mut app, ".orch-card");
        {
            let scene = app.ui.scene.borrow();
            for node in rows.iter().chain(cards.iter()) {
                assert!(!scaled(&scene, *node), "a box scaled at frame {f}");
            }
            if f >= 7 {
                for node in &cards {
                    assert!(opacity(&scene, *node) < 0.01, "a card still shows at frame {f}");
                }
            }
        }
        app.frame();
    }
    let row = one(&mut app, "#orch-row-it-chat").unwrap();
    let scene = app.ui.scene.borrow();
    assert_eq!(moved(&scene, row), (0., 0.));
    assert!(opacity(&scene, row) > 0.995);
}
