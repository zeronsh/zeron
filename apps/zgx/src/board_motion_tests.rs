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
/// 100 ms, and each card in view becomes its row (decision 47's drift).
#[test]
fn the_switch_to_the_list_slides_rows_from_their_cards_never_scaled() {
    let mut app = mount("orch_board_places_fixture.luau", 1240., 820., "all");
    let card = one(&mut app, "#orch-card-it-chat").unwrap();
    let was = app.ui.scene.borrow().bounds(card);
    app.host.set_state("board_view", Value::from("list")).unwrap();
    app.frame();
    // (Where it starts: within 24 points of home, toward where its card
    // stood, `the_switch_drifts_each_arrival_at_most_24_points`.)
    let _ = was;
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

/// The orchestrator's call on a card going into a folded drawer: its slot
/// collapses while it travels, as any move's does. The card under it is
/// held where it stood by a translation that eases to nothing over the
/// travel, while a copy of the card goes to the drawer's head line.
#[test]
fn a_card_into_a_folded_drawer_closes_its_gap_as_it_travels() {
    let mut app = board("all");
    run(&app, "motion_pause");
    frames(&app, 47);
    for _ in 0..4 {
        if one(&mut app, "#orch-col-running >> .orch-card-ghost").is_some() {
            break;
        }
        app.frame();
    }
    assert!(one(&mut app, "#orch-col-running >> .orch-card-ghost").is_some(), "no copy in flight");
    assert!(one(&mut app, "#orch-col-running >> #orch-card-it-37").is_none(), "still in its list");
    // The card that was under it (it-38): from its old place, up to its new.
    let mut ys = Vec::new();
    for _ in 0..21 {
        let below = one(&mut app, "#orch-card-it-38").unwrap();
        ys.push(moved(&app.ui.scene.borrow(), below).1);
        app.frame();
    }
    eprintln!("zeron-zgx: BOARDMOTION drawer gap: it-38 held by {:?}", ys.iter().map(|y| y.round()).collect::<Vec<_>>());
    assert!(ys[0] > 10., "the slot was closed at once ({})", ys[0]);
    assert!(ys.windows(2).all(|w| w[1] <= w[0] + 0.01), "the slot did not close steadily: {ys:?}");
    assert!(ys[ys.len() / 2] < ys[0] * 0.2, "half way, the slot was still open: {ys:?}");
    frames(&app, 10);
    assert!(one(&mut app, ".orch-card-ghost").is_none(), "its copy stayed");
    let below = one(&mut app, "#orch-card-it-38").unwrap();
    assert_eq!(moved(&app.ui.scene.borrow(), below), (0., 0.));
}

/// The orchestrator's call on the switch: the leaving view fades over its
/// 100 ms, and nothing of the arriving one shows before 80 ms (at 60 ms,
/// the list's heads still read faintly through the board in the strips).
#[test]
fn the_switch_starts_the_rows_after_the_board_has_mostly_faded() {
    let mut app = mount("orch_board_places_fixture.luau", 1240., 820., "all");
    app.host.set_state("board_view", Value::from("list")).unwrap();
    // Frames at 17, 33, 50 ... 150 ms after the switch.
    for f in 1..=9 {
        app.frame();
        let rows = all(&mut app, ".orch-list-row");
        let cards = all(&mut app, ".orch-card");
        let scene = app.ui.scene.borrow();
        let row = rows.iter().map(|n| opacity(&scene, *n)).fold(0f32, f32::max);
        let card = cards.iter().map(|n| opacity(&scene, *n)).fold(0f32, f32::max);
        eprintln!("zeron-zgx: BOARDMOTION switch {} ms: board {card:.2} rows {row:.2}", f * 50 / 3);
        if f * 1000 / 60 < 80 {
            assert!(row < 0.02, "rows show at {} ms: {row}", f * 1000 / 60);
        }
        if f >= 8 {
            assert!(card < 0.01, "the board still shows at {} ms: {card}", f * 1000 / 60);
        }
    }
}

/// The card button `node` stands in, if any (it, or an ancestor).
fn card_of(scene: &Scene, node: NodeId, cards: &[NodeId]) -> Option<NodeId> {
    std::iter::once(node).chain(scene.ancestors(node)).find(|n| cards.contains(n))
}

/// Decision 46: a travelling card is over every card at rest, from its
/// first frame to its landing, solid all the while: out of a folded drawer
/// over its column's cards, and beside cards arriving by a crossfade.
#[test]
fn a_card_in_flight_is_over_every_card_at_rest_and_solid_from_its_first_frame() {
    for (command, moving) in [("engine_start", "it-keyed"), ("engine_five", "it-chat")] {
        let mut app = mount("orch_board_places_fixture.luau", 1240., 820., "all");
        run(&app, command);
        frames(&app, 47);
        let target = format!("#orch-card-{moving}");
        let mut travelling = 0;
        for f in 0..30 {
            app.frame();
            if one(&mut app, &format!("{target} >> .orch-card-underlay")).is_none()
                && all(&mut app, ".orch-card-underlay").iter().all(|_| true)
            {
                // (Not yet, or no longer, in flight.)
            }
            let Some(card) = one(&mut app, &target) else { continue };
            let cards = all(&mut app, ".orch-card");
            let unders = all(&mut app, ".orch-card-underlay");
            let scene = app.ui.scene.borrow();
            // In flight: its underlay (in its node) is shown.
            // (Its underlay: under it, its top left where the card's is.)
            let cb = scene.bounds(card);
            let mine = unders.iter().copied().find(|u| {
                let b = scene.bounds(*u);
                (b.x - cb.x).abs() < 2. && (b.y - cb.y).abs() < 2.
            });
            let Some(under) = mine else { continue };
            travelling += 1;
            assert!(opacity(&scene, under) > 0.995, "{command}: its underlay at {} in flight frame {f}", opacity(&scene, under));
            // (What draws nothing yet takes no hits: from when its content
            // shows.)
            if opacity(&scene, card) < 0.5 {
                continue;
            }
            let b = scene.bounds(card);
            for (px, py) in [(b.x + b.width / 2., b.y + b.height / 2.), (b.x + 20., b.y + 10.), (b.x + b.width - 20., b.y + b.height - 10.)] {
                let hits = scene.hit_test_all(px, py);
                let first = hits.iter().find_map(|h| card_of(&scene, *h, &cards));
                // (It, or another card in flight with it: none at rest.)
                let in_flight = |c: NodeId| {
                    let cb = scene.bounds(c);
                    unders.iter().any(|u| {
                        let b = scene.bounds(*u);
                        (b.x - cb.x).abs() < 2. && (b.y - cb.y).abs() < 2.
                    })
                };
                assert!(first.is_some_and(in_flight), "{command}: at {px},{py}, frame {f} of its flight, a card at rest is over it");
            }
        }
        assert!(travelling >= 10, "{command}: {travelling} frames in flight seen");
    }
}

fn foot_y(app: &mut TestApp, place: &str) -> f32 {
    let foot = one(app, &format!("#orch-col-{place} >> .orch-lane-foot")).unwrap();
    app.ui.scene.borrow().transform(foot).y
}

/// The orchestrator's sixth point and decision 48: a lane never eases at
/// mount, on a tab or a filter: everything in a column takes its place at
/// once.
#[test]
fn lanes_do_not_ease_at_mount_on_a_tab_or_a_filter() {
    let script = ui_dir().join("orch_board_places_fixture.luau");
    let mut app = TestApp::new(|runtime| crate::plugins::lend_design_system(ScriptHost::new(runtime, script.clone()), &script));
    app.ui.scene.borrow_mut().resize(1240., 820.);
    for f in 0..40 {
        app.frame();
        if f > 0 {
            for place in ["needs_you", "running", "done"] {
                assert_eq!(foot_y(&mut app, place), 0., "{place}'s lane eased at mount, frame {f}");
            }
        }
    }
    for (state, value) in [("board_tab", Value::from("running")), ("board_tab", Value::from("all")), ("filter", Value::from("orc-vpn")), ("filter", Value::Nil)] {
        app.host.set_state(state, value).unwrap();
        for f in 0..20 {
            app.frame();
            for place in ["running", "done"] {
                if one(&mut app, &format!("#orch-col-{place} >> .orch-lane-foot")).is_some() {
                    assert_eq!(foot_y(&mut app, place), 0., "{place}'s lane eased on {state}, frame {f}");
                }
            }
        }
    }
}

/// Decisions 48 and 49: as the detail opens and closes, its content keeps
/// its width, pinned to its box's right edge, while the box eases; each
/// column's drawer stays under its last card, moving with it, never
/// sliding across it nor opening a gap.
#[test]
fn the_detail_eases_its_box_and_the_columns_move_as_one() {
    let mut app = mount("orch_board_places_fixture.luau", 1240., 820., "all");
    let gap_of = |app: &mut TestApp| -> f32 {
        let cards = all(app, "#orch-col-running >> .orch-card");
        let head = one(app, "#orch-col-running >> .orch-drawer-head").unwrap();
        let scene = app.ui.scene.borrow();
        let last = cards.iter().map(|c| { let b = scene.bounds(*c); b.y + b.height }).fold(0f32, f32::max);
        scene.bounds(head).y - last
    };
    let rest = gap_of(&mut app);
    app.host.set_state("board_selected", Value::from("it-chat")).unwrap();
    let mut widths = Vec::new();
    for f in 0..16 {
        app.frame();
        let g = gap_of(&mut app);
        assert!((g - rest).abs() < 1., "opening, frame {f}: the drawer stands {g} under the last card ({rest} at rest)");
        if let (Some(content), Some(space)) = (one(&mut app, "#orch-board-detail"), one(&mut app, ".orch-board-detail-space")) {
            let scene = app.ui.scene.borrow();
            let (c, s) = (scene.bounds(content), scene.bounds(space));
            assert!((c.width - 420.).abs() < 0.5, "frame {f}: its content is {} wide", c.width);
            assert!(((c.x + c.width) - (s.x + s.width)).abs() < 0.5, "frame {f}: its content isn't pinned right");
            widths.push(s.width);
        }
    }
    assert!(widths.iter().any(|w| *w > 1. && *w < 431.), "the box never eased: {widths:?}");
    app.host.set_state("board_selected", Value::Nil).unwrap();
    for f in 0..16 {
        app.frame();
        let g = gap_of(&mut app);
        assert!((g - rest).abs() < 1., "closing, frame {f}: the drawer stands {g} under the last card ({rest} at rest)");
    }
}

/// Decision 47: across the switch, each arriving row or card starts within
/// 24 points of its home.
#[test]
fn the_switch_drifts_each_arrival_at_most_24_points() {
    for (from, to, target) in [("board", "list", ".orch-list-row"), ("list", "board", ".orch-card")] {
        let mut app = mount("orch_board_places_fixture.luau", 1240., 820., "all");
        app.host.set_state("board_view", Value::from(from)).unwrap();
        frames(&app, 40);
        app.host.set_state("board_view", Value::from(to)).unwrap();
        let mut most = 0f32;
        for _ in 0..24 {
            app.frame();
            let nodes = all(&mut app, target);
            let scene = app.ui.scene.borrow();
            for n in nodes {
                let (dx, dy) = moved(&scene, n);
                most = most.max((dx * dx + dy * dy).sqrt());
            }
        }
        eprintln!("zeron-zgx: BOARDMOTION switch {from}-to-{to}: farthest from home {most:.1} points");
        assert!(most <= 24.5, "{from} to {to}: an arrival started {most} points from home");
        assert!(most > 1., "{from} to {to}: nothing drifted");
    }
}

/// Decision 50: the empty line's words come in once the last card has gone.
#[test]
fn the_empty_lines_words_wait_for_the_last_card() {
    let mut app = mount("orch_board_places_fixture.luau", 1240., 820., "all");
    run(&app, "engine_five");
    let mut seen = Vec::new();
    for _ in 0..95 {
        app.frame();
        let words = one(&mut app, "#orch-col-running >> .orch-column-empty-words");
        let flying = all(&mut app, ".orch-card-underlay").len();
        if let Some(w) = words {
            let o = opacity(&app.ui.scene.borrow(), w);
            seen.push(o);
            if flying > 0 {
                assert!(o < 0.01, "its words show at {o} while a card is still on its way");
            }
        }
    }
    assert!(seen.last().is_some_and(|o| *o > 0.99), "its words never came: {seen:?}");
}
