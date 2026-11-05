//! A picture is never stretched or squeezed: in every small frame that crops
//! one (the composer's chip and its Appshot chip, a sent message's
//! thumbnail, an avatar, and a cropped `ui.image` with a `fallback` and
//! without: the dock's thumbnail's shape), the picture is drawn at its own
//! shape and
//! the frame crops it. zgx lays a `ui.image` out as a picture node at the
//! size its `fit` gives (`fitted_image_size`), so that node's box is what is
//! drawn: its width over its height must be the picture's own, within 1%.
//! Mounted headless on ui/picture_aspect_fixture.luau (a 2400 by 600 row of
//! circles).

use zgx::scene::{NodeId, NodeKind};
use zgx_script::{ScriptHost, testing::TestApp};

fn mount() -> TestApp {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui/picture_aspect_fixture.luau");
    let app = TestApp::new(|runtime| crate::plugins::lend_design_system(ScriptHost::new(runtime, path.clone()), &path));
    app.ui.scene.borrow_mut().resize(900., 600.);
    app.frames(30);
    assert_eq!(app.error(), None, "the fixture failed");
    app
}

/// Each picture under `root`: its drawn aspect (its node's box) and its own
/// (its pixels').
fn pictures_under(app: &TestApp, root: NodeId) -> Vec<(f32, f32)> {
    let scene = app.ui.scene.borrow();
    let mut stack = vec![root];
    let mut out = Vec::new();
    while let Some(id) = stack.pop() {
        if let NodeKind::Image(data) = scene.kind(id)
            && data.width() > 1
            && data.height() > 1
        {
            let b = scene.bounds(id);
            if b.width > 0. && b.height > 0. {
                out.push((b.width / b.height, data.width() as f32 / data.height() as f32));
            }
        }
        stack.extend_from_slice(scene.children(id));
    }
    out
}

fn assert_own_shape(app: &mut TestApp, target: &str) {
    let root = app.target(target).unwrap_or_else(|e| panic!("{target}: {e}\n{}", app.tree()));
    let found = pictures_under(app, root);
    assert!(!found.is_empty(), "{target}: no picture drawn\n{}", app.tree());
    for (drawn, own) in found {
        assert!(((drawn - own) / own).abs() < 0.01, "{target}: drawn at {drawn:.3}, its own shape {own:.3}");
    }
}

#[test]
fn a_cropped_picture_keeps_its_own_shape_in_every_small_frame() {
    let mut app = mount();
    assert_own_shape(&mut app, "aspect-cover");
    assert_own_shape(&mut app, "aspect-cover-fallback");
    assert_own_shape(&mut app, "aspect-avatar");
    assert_own_shape(&mut app, "\"wide.png\"");
    assert_own_shape(&mut app, "\"Safari\"");
    assert_own_shape(&mut app, "role:button \"Open wide.png\"");
}
