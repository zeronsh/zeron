//! The board's motion (ui/ds/orch_board_motion.luau) leaves nothing running:
//! once cards have moved and landed, the frame clock asks for no frame the
//! board didn't ask for before they moved. On the 200-card fixture
//! (ui/orch_board_perf_fixture.luau), as `zgx test` can't see the clock.
//! `ZERON_ZGX_TEST_UI` points it at another tree's `ui/` (no build).

use zgx_script::{ScriptHost, Value, testing::TestApp};

fn board(tab: &str) -> TestApp {
    let ui = std::env::var_os("ZERON_ZGX_TEST_UI")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui"));
    let script = ui.join("orch_board_perf_fixture.luau");
    let app = TestApp::new(|runtime| crate::plugins::lend_design_system(ScriptHost::new(runtime, script.clone()), &script));
    app.ui.scene.borrow_mut().resize(1500., 1400.);
    app.host.set_state("board_tab", Value::from(tab)).unwrap();
    frames(&app, 120);
    app
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

/// Done's tab alone: no working ring shows, so a still board asks for no
/// frame at all; while cards move it does, and once they land it stops.
#[test]
fn a_still_board_asks_for_no_frame_before_or_after_its_cards_move() {
    for command in ["motion_one", "motion_four", "motion_many"] {
        let app = board("done");
        assert_eq!(wanted(&app, 60), 0, "a still board asked for frames");
        run(&app, command);
        // Holding (800 ms) then travelling (350 ms): frames asked for.
        let moving = wanted(&app, 75);
        assert!(moving > 0, "{command}: nothing moved");
        frames(&app, 30);
        assert_eq!(wanted(&app, 60), 0, "{command}: frames asked for after the moves landed");
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
    assert!(after <= before + 1, "after the moves: {after} frames of 120 asked for, {before} before");
}
