//! Setup's footer keeps its words in the scheme's ink as Make it yours
//! switches schemes: Continue (the secondary pill, `$text.primary`) and
//! Back at rest (a ghost pill, `$text.secondary`), each inherited from its
//! button. Before zgx d81d2c33 (#1439) text inheriting a token colour
//! followed the first theme switch and then kept that ink, so Light then
//! Dark left Light's near-black words on Dark's fill.
//!
//! Read off the scene, where zgx paints it (the Lua test kit can't see
//! colours): each button's text node against the window's token, after
//! each scheme card is clicked (the pointer left on the card, so Back is at
//! rest).

use std::path::PathBuf;

use zgx::scene::{Color, NodeId, NodeKind};
use zgx::theme::tok;
use zgx_script::{ScriptHost, testing::TestApp};

fn ui_dir() -> PathBuf {
    std::env::var_os("ZERON_ZGX_TEST_UI")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui"))
}

/// The colour the first text node under `node` paints.
fn ink(app: &TestApp, node: NodeId) -> Option<Color> {
    let scene = app.ui.scene.borrow();
    let mut stack = vec![node];
    while let Some(id) = stack.pop() {
        if let NodeKind::Text { color, .. } = scene.kind(id) {
            return Some(*color);
        }
        stack.extend(scene.children(id).iter().rev().copied());
    }
    None
}

/// The painted ink of button `name`'s words.
fn words(app: &mut TestApp, name: &str) -> Option<Color> {
    let button = app
        .find(&format!("role:button \"{name}\""))
        .unwrap_or_else(|| panic!("no {name}:\n{}", app.tree()));
    ink(app, button)
}

#[test]
fn the_footer_wears_each_schemes_ink() {
    // Setup's window, opened on Make it yours, signed out (Back, not Sign
    // out, at the bottom left).
    let mut app = TestApp::new(|runtime| {
        let host = ScriptHost::new(runtime, ui_dir().join("app.luau"));
        host.shared("onboarding.start", "appearance");
        host.shared("onboarding.arrived", true);
        // The window's chrome follows the scheme (`ui.invoke("appearance")`).
        host.action("appearance", |_| {});
        host
    });
    app.ui.scene.borrow_mut().resize(1205., 705.);
    app.frames(30);
    assert_eq!(app.error(), None, "setup failed to load");
    // Back and forth, through System, as the teammate who found it did.
    for scheme in ["Light", "Dark", "Light", "System", "Dark", "Light"] {
        app.click(&format!("role:radio \"{scheme}\""));
        app.frames(30);
        let theme = app.ui.theme().root().clone();
        assert_eq!(
            words(&mut app, "Continue"),
            Some(theme.color_untracked(tok::TEXT_PRIMARY)),
            "after {scheme}: Continue's words in the scheme's text.primary"
        );
        assert_eq!(
            words(&mut app, "Back"),
            Some(theme.color_untracked(tok::TEXT_SECONDARY)),
            "after {scheme}: Back's words at rest in the scheme's text.secondary"
        );
    }
}
