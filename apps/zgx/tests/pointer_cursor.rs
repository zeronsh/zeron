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

/// Every resize handle Zeron v3 has shows its cursor under the pointer, a
/// chat alone and in a split (whose panes, page card and drop layers stand
/// over the page): the sidebar's edge and the right panel's edge. (Its
/// split panes and the panel's tab groups share their room equally: no
/// divider to drag. The files pane's tree keeps its width.) And a pane's
/// tab, in the title bar's band, shows the hand: nothing the window's width
/// stands in front of them while nothing is dragged.
#[test]
fn every_resize_edge_shows_its_cursor_alone_and_in_a_split() {
    use zgx::input::{InputEvent, Modifiers, PointerButton};
    let ui = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui");
    let mut app = TestApp::new(|runtime| ScriptHost::new(runtime, ui.join("shell_fixture.luau")));
    app.ui.scene.borrow_mut().resize(1400., 860.);
    app.frames(60);
    app.click("#shell-sidebar >> role:button \"Fix the build\"");
    app.frames(20);
    app.click("#titlebar >> role:button \"Panel\"");
    app.frames(60);
    let check = |app: &mut TestApp, when: &str| {
        let sidebar = app.find("shell-sidebar").expect("the sidebar");
        let panel = app.target("role:slider \"Resize files\"").expect("the panel's edge");
        let scene = app.ui.scene.borrow();
        let b = scene.visible_bounds(sidebar).expect("the sidebar shows");
        for y in [b.y + 120., b.y + b.height / 2., b.y + b.height - 120.] {
            assert_eq!(scene.cursor_at(b.x + b.width - 3., y), Some(Cursor::ColResize), "{when}: the sidebar's edge, y {y}");
        }
        let e = scene.visible_bounds(panel).expect("the panel's edge shows");
        for y in [e.y + 120., e.y + e.height / 2., e.y + e.height - 120.] {
            assert_eq!(scene.cursor_at(e.x + e.width / 2., y), Some(Cursor::ColResize), "{when}: the panel's edge, y {y}");
        }
    };
    check(&mut app, "a chat alone");
    // A second chat dropped at the page's right edge, a frame between moves.
    let row = app.bounds("#shell-sidebar >> role:button \"Tidy the tests\"");
    let page = app.bounds("shell-page");
    let (sx, sy) = (row.x + 40., row.y + row.height / 2.);
    let (x, y) = (page.x + page.width - 24., page.y + page.height / 2.);
    let (button, m) = (PointerButton::Primary, Modifiers::default());
    app.ui.dispatch_with_modifiers(InputEvent::PointerMove { x: sx, y: sy }, m);
    app.ui.dispatch_with_modifiers(InputEvent::PointerDown { x: sx, y: sy, button }, m);
    for step in 0..=8 {
        let part = step as f32 / 8.;
        app.ui.dispatch_with_modifiers(InputEvent::PointerMove { x: sx + 16. + (x - sx - 16.) * part, y: sy + 8. + (y - sy - 8.) * part }, m);
        app.frames(2);
    }
    app.ui.dispatch_with_modifiers(InputEvent::PointerUp { x, y, button }, m);
    app.ui.input.forget_clicks();
    app.frames(40);
    assert_eq!(app.tree().matches("\"Close pane\"").count(), 2, "split in two");
    assert!(app.error().is_none(), "{:?}", app.error());
    check(&mut app, "in a split");
    // A panel tab, in the title bar's band: the hand. (The bar is a strip
    // of no height its controls hang from: a box its size stood in front of
    // everything along the window's top.)
    app.click("#panel-open-files");
    app.frames(40);
    let tab = app.target("#panel-strip >> role:tab").expect("a panel tab");
    let scene = app.ui.scene.borrow();
    let r = scene.visible_bounds(tab).expect("the tab shows");
    assert_eq!(scene.cursor_at(r.x + r.width / 2., r.y + r.height / 2.), Some(Cursor::Pointer), "a panel tab under the title bar's band");
}
