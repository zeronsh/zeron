//! The composer's tray over a wallpaper (ds/composer `.composer-tray`): the
//! pill's glass continued under the footer, tucked up behind the pill. Its
//! top is square and its foot takes the trays' radius, and it draws no edge
//! of its own (the pill keeps its edge): a rounded top, or a hairline round
//! it, showed as a second curve against the pill's corner.
//!
//! Read off the scene, where zgx paints it: a token `rounded` with
//! `rounded_tl`/`rounded_tr` of 0 drew a rounded top regardless (the
//! refinement lost to the token), so this checks what's painted, not the
//! props. With Zeron's radii and a template's (Hot Pink: the pill's 36, the
//! trays' 24), as ui/composer_place_fixture.luau sets them.

use std::path::PathBuf;

use zgx::scene::NodeKind;
use zgx_script::{ScriptHost, Value, testing::TestApp};

fn ui_dir() -> PathBuf {
    std::env::var_os("ZERON_ZGX_TEST_UI")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui"))
}

/// The fixture over its wallpaper, `radii` "default" or "hot".
fn composer(radii: &str) -> TestApp {
    let radii = radii.to_owned();
    let app = TestApp::new(move |runtime| {
        let host = ScriptHost::new(runtime, ui_dir().join("composer_place_fixture.luau"));
        host.shared("test_place_hot_radii", Value::Bool(radii == "hot"));
        host
    });
    app.ui.scene.borrow_mut().resize(900., 420.);
    // (The tray eases in: real time for its fade.)
    for _ in 0..30 {
        std::thread::sleep(std::time::Duration::from_millis(12));
        app.frames(2);
    }
    app
}

/// The tray's painted corners (top-left, top-right, bottom-right,
/// bottom-left) and whether any edge is drawn round it.
fn tray(app: &mut TestApp) -> ([f32; 4], bool) {
    let id = app.find(".composer-tray").unwrap_or_else(|| panic!("no tray over the wallpaper:\n{}", app.tree()));
    let scene = app.ui.scene.borrow();
    let quad = match scene.kind(id) {
        NodeKind::Quad(quad) | NodeKind::Panel { quad, .. } => quad.clone(),
        other => panic!("the tray paints no box: {other:?}"),
    };
    let corners = match quad.decoration.as_ref().and_then(|d| d.corners) {
        Some(c) => [c.top_left, c.top_right, c.bottom_right, c.bottom_left],
        None => [quad.radius; 4],
    };
    let sides = quad.decoration.as_ref().and_then(|d| d.border_widths);
    let edged = quad.border_width > 0.
        || sides.is_some_and(|s| s.top > 0. || s.right > 0. || s.bottom > 0. || s.left > 0.);
    (corners, edged)
}

#[test]
fn the_tray_is_square_on_top_rounded_at_its_foot_and_draws_no_edge() {
    for (radii, foot) in [("default", 16.), ("hot", 24.)] {
        let mut app = composer(radii);
        assert_eq!(app.error(), None, "{radii}: the fixture failed");
        let (corners, edged) = tray(&mut app);
        assert_eq!(corners, [0., 0., foot, foot], "{radii}: its top square, its foot the trays' radius");
        assert!(!edged, "{radii}: the tray draws no edge of its own");
    }
}
