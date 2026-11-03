//! A modal opening closes what floats over the page first (zgx #1419): the
//! composer's Work in panel left open under ⌘K stayed up, drawn over the
//! palette's device list (it had reached past the window's edge, so zgx
//! showed it in a popup window of its own, over everything). Mounted
//! headless on the shell's fixture.

use zgx::scene::NodeId;
use zgx_script::{ScriptHost, testing::TestApp};

fn mount() -> TestApp {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui/shell_fixture.luau");
    let app = TestApp::new(|runtime| ScriptHost::new(runtime, path));
    app.ui.scene.borrow_mut().resize(1240., 820.);
    app.frames(30);
    assert_eq!(app.error(), None, "the shell failed");
    app
}

/// Whether `target` is there and shows: its line in the tree (`line`
/// names it there) not marked hidden.
fn showing(app: &mut TestApp, target: &str, line: &str) -> bool {
    app.find(target).is_some() && app.tree().lines().any(|l| l.contains(line) && !l.contains(" hidden"))
}

/// The overlay (a modal's, a popover's panel) `node` shows in, if any.
fn overlay_of(app: &TestApp, node: NodeId) -> Option<NodeId> {
    let overlays: Vec<NodeId> = app.ui.portals().into_iter().map(|(_, overlay)| overlay).collect();
    let scene = app.ui.scene.borrow();
    let mut at = Some(node);
    while let Some(id) = at {
        if overlays.contains(&id) {
            return Some(id);
        }
        at = scene.parent(id);
    }
    None
}

const WORK_IN: &str = "role:button \"Device\"";
const WORK_IN_LINE: &str = "button \"Device\"";

#[test]
fn a_modal_opening_closes_the_popups_and_draws_over_them() {
    let mut app = mount();
    assert!(app.host.run_command("shell.new_chat"));
    app.frames(30);
    // A new chat's Work in panel, open.
    app.click("role:button \"Branch\"");
    app.frames(30);
    assert!(showing(&mut app, WORK_IN, WORK_IN_LINE), "the Work in panel opened");
    let row = app.find(WORK_IN).unwrap();
    let r = app.ui.scene.borrow().transformed_bounds(row);
    let (x, y) = (r.x + r.width / 2., r.y + r.height / 2.);

    // ⌘K: the palette opens, and the panel has closed first.
    assert!(app.host.run_command("shell.palette"));
    app.frames(30);
    assert!(showing(&mut app, "#palette-search", "#palette-search"), "the palette opened");
    assert!(!showing(&mut app, WORK_IN, WORK_IN_LINE), "the Work in panel closed as the palette opened");
    // And where the panel was, the palette (its card or its scrim) is what
    // the pointer meets: nothing floats over it.
    let palette = app.find("#palette-search").unwrap();
    let palette = overlay_of(&app, palette).expect("the palette's overlay");
    let hit = app.ui.scene.borrow().hit_test(x, y).expect("something there");
    assert_eq!(overlay_of(&app, hit), Some(palette), "the palette is on top at ({x}, {y})");
    assert_eq!(app.error(), None);
}
