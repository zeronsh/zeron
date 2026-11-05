//! A GIF a reply names plays: in the reply and in the lightbox it opens in,
//! the picture zgx shows carries its animation (zgx 260f0d33: an image view
//! whose picture is a GIF's first frame plays the rest, and holds it still
//! under the system's reduced motion). Mounted headless on the reply
//! pictures' fixture, whose "The spinner" is a two-frame GIF.

use zgx::scene::{NodeId, NodeKind};
use zgx_script::{ScriptHost, testing::TestApp};

fn mount() -> TestApp {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui/reply_pictures_fixture.luau");
    let app = TestApp::new(|runtime| ScriptHost::new(runtime, path));
    // (Tall enough that the list builds every reply.)
    app.ui.scene.borrow_mut().resize(1000., 7000.);
    app.frames(30);
    assert_eq!(app.error(), None, "the fixture failed");
    app
}

/// The pictures under `root` that carry an animation (a GIF's first frame,
/// which zgx plays), counted.
fn animated_under(app: &TestApp, root: NodeId) -> usize {
    let scene = app.ui.scene.borrow();
    let mut stack = vec![root];
    let mut n = 0;
    while let Some(id) = stack.pop() {
        if let NodeKind::Image(data) = scene.kind(id)
            && data.animation().is_some_and(|a| a.len() > 1)
        {
            n += 1;
        }
        stack.extend_from_slice(scene.children(id));
    }
    n
}

#[test]
fn a_gif_in_a_reply_and_its_lightbox_plays() {
    let mut app = mount();
    let tile = "role:button \"Open The spinner\"";
    let shown = app.wait_until(|app| app.find(tile).is_some());
    if !shown {
        let tree = app.tree();
        let near: Vec<&str> = tree.lines().filter(|l| l.contains("pinner") || l.contains("GIF") || l.contains("Open ")).collect();
        panic!("the GIF's tile never showed:\n{}", near.join("\n"));
    }
    app.frames(20);
    let tile_id = app.target(tile).expect("the GIF's tile");
    assert_eq!(animated_under(&app, tile_id), 1, "the reply's GIF carries its animation");

    // Opened, the lightbox shows it playing too.
    let root = app.ui.scene.borrow().root();
    let before = animated_under(&app, root);
    app.click(tile);
    app.frames(60);
    let after = animated_under(&app, root);
    assert!(after > before, "the lightbox's picture carries the animation ({before} → {after})");
    assert_eq!(app.error(), None);
}
