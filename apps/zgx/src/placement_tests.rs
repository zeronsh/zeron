//! Where a session's composer and its text menu are drawn: the composer's
//! dock rests its gap above the page's foot (the window's edge under it), and the menu (ds/editor_menu, through the
//! window's menu host) opens at the secondary click, at 100% and at a zoom
//! (90%, the user's). In the scene, that is: the host's popup window over
//! it is zgx's (zgx-asks/popup-window-zoom). Mounted headless on the
//! shell's fixture, with the real editor-menu host.

use std::{cell::RefCell, rc::Rc};

use zgx::{
    input::{InputEvent, PointerButton},
    platform::PlatformService,
};
use zgx_script::{ScriptHost, testing::TestApp};

const W: f32 = 1240.;
const H: f32 = 820.;

/// The shell at `zoom`, as the host lays a window of W×H points out at it.
fn mount(zoom: f64) -> TestApp {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui/shell_fixture.luau");
    let app = TestApp::with_desktop(|runtime| ScriptHost::new(runtime, path));
    app.ui.set_zoom(zoom);
    app.ui.scene.borrow_mut().resize(W / zoom as f32, H / zoom as f32);
    let service = PlatformService(app.desktop.clone().unwrap());
    crate::editor_menu::install(&app.host, app.ui.downgrade(), Rc::new(RefCell::new(Some(service))));
    app.frames(30);
    assert_eq!(app.error(), None, "the shell failed");
    app
}

fn settle(app: &TestApp) {
    for _ in 0..8 {
        std::thread::sleep(std::time::Duration::from_millis(15));
        app.frames(2);
    }
}

#[test]
fn a_sessions_composer_sits_on_the_page_foot_and_its_menu_opens_at_the_click() {
    for zoom in [1., 0.9] {
        let mut app = mount(zoom);
        let (w, h) = (W / zoom as f32, H / zoom as f32);
        app.click("#shell-sidebar >> role:button \"Fix the build\"");
        settle(&app);
        settle(&app);

        // The dock rests its gap (screens/chat/dock's GAP, 8) above the
        // page's foot: its card's bottom, inside the card's 1-point edge,
        // the window's edge (5) under it.
        let dock = app.find("#chat-composer-dock").expect("the session's composer");
        let d = app.ui.scene.borrow().transformed_bounds(dock);
        let page = app.find("#shell-page").expect("the page");
        let p = app.ui.scene.borrow().transformed_bounds(page);
        assert!((h - (p.y + p.height) - 5.).abs() < 1., "at {zoom}: the page's card stands the window's edge above its bottom");
        let gap = (p.y + p.height - 1.) - (d.y + d.height);
        assert!((gap - 8.).abs() < 1., "at {zoom}: the dock ends {gap} above the page's foot ({p:?})");

        // Words in the draft, selected; a secondary click on them.
        app.type_text("#chat-composer >> role:textbox", "asdasd");
        settle(&app);
        let field = app.find("#chat-composer >> role:textbox").unwrap();
        let editor = app.ui.editor_within(field).expect("the draft's editor");
        editor.set_selection(0, 6);
        app.frames(2);
        let at = editor.text_area(2..3).unwrap();
        let (x, y) = (at.x + at.width / 2., at.y + at.height / 2.);
        app.ui.dispatch(InputEvent::PointerMove { x, y });
        app.ui.dispatch(InputEvent::PointerDown { x, y, button: PointerButton::Secondary });
        settle(&app);
        app.ui.dispatch(InputEvent::PointerUp { x, y, button: PointerButton::Secondary });
        settle(&app);

        // The menu hangs from the click: its left edge there, and (short of
        // room below, near the window's foot, it moves up, as AppKit's do)
        // the click within its height. All of it in the window.
        let menu = app.find("role:menu \"Context menu\"").expect("the menu");
        let m = app.ui.scene.borrow().transformed_bounds(menu);
        // (Its left is the 1×1 point's right edge it hangs from: a point on.)
        assert!((m.x - x - 1.).abs() < 1., "at {zoom}: the menu's left {} vs the click {x}", m.x);
        assert!(m.y <= y + 1. && y <= m.y + m.height, "at {zoom}: the menu {m:?} vs the click {y}");
        assert!(m.x + m.width <= w + 0.5 && m.y + m.height <= h + 0.5, "at {zoom}: the menu {m:?} past the window {w}×{h}");
        assert_eq!(app.error(), None);
    }
}
