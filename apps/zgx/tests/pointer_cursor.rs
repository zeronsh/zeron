//! The pointer's cursor is the one under it: the sidebar's edge shows the
//! column-resize cursor, a sidebar row the pointing hand. zgx gives the
//! cursor of the front-most node at the pointer (`Scene::cursor_at`); a
//! layer the window's size in front of the shell, even one that lets the
//! pointer through (the session hover card's, until it was made an anchor of
//! no size), took every cursor away.

use std::path::PathBuf;

use zgx::cursor::Cursor;
use zgx_script::{ScriptHost, testing::TestApp};

#[test]
fn the_sidebar_edge_shows_the_resize_cursor_and_a_row_the_hand() {
    let ui = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui");
    let mut app = TestApp::new(|runtime| ScriptHost::new(runtime, ui.join("shell_fixture.luau")));
    app.ui.scene.borrow_mut().resize(1200., 800.);
    app.frames(60);
    let edge = app.find("shell-sidebar").expect("the sidebar");
    let scene = app.ui.scene.borrow();
    let b = scene.visible_bounds(edge).expect("the sidebar shows");
    // Its edge: its last few points, down its whole height.
    for y in [b.y + 120., b.y + b.height / 2., b.y + b.height - 120.] {
        assert_eq!(scene.cursor_at(b.x + b.width - 3., y), Some(Cursor::ColResize), "at its edge, y {y}");
    }
    // A row: the hand, as a button.
    let row = scene
        .hit_test_all(100., 290.)
        .into_iter()
        .find_map(|id| scene.cursor_for(id))
        .expect("a sidebar row under the pointer");
    assert_eq!(row, Cursor::Pointer);
    assert_eq!(scene.cursor_at(100., 290.), Some(Cursor::Pointer));
}
