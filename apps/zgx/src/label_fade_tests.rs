//! The sidebar's labels fade out at their end where they don't fit, never
//! cut (ds/fade_text): mounted headless on ui/shell_fixture.luau, with a
//! section of yours named longer than the sidebar is wide. Every text the
//! sidebar shows that runs past the box clipping it has a fade at that
//! box's right edge, and a label that fits has none. (A section's title was
//! plain text: cut mid-letter at the row's end.)

use zgx::scene::NodeKind;
use zgx_script::{ScriptHost, testing::TestApp};

const LONG: &str = "Everything to read again before the release goes out on Friday";

fn mount() -> TestApp {
    // (This checkout's `ui/`, or `ZERON_ZGX_TEST_UI`'s: another copy, to see
    // the test fail on code from before a fix.)
    let ui = std::env::var_os("ZERON_ZGX_TEST_UI")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui"));
    let path = ui.join("shell_fixture.luau");
    let app = TestApp::new(|runtime| ScriptHost::new(runtime, path));
    app.ui.scene.borrow_mut().resize(1200., 800.);
    app.frames(30);
    app
}

/// Each text shown in the sidebar: its words, whether it runs past the box
/// clipping it, and whether that box fades its right edge.
fn labels(app: &mut TestApp) -> Vec<(String, bool, bool)> {
    let sidebar = app.find("shell-sidebar").expect("the sidebar");
    let scene = app.ui.scene.borrow();
    let mut out = Vec::new();
    let mut stack = vec![sidebar];
    while let Some(n) = stack.pop() {
        stack.extend(scene.children(n).iter().copied());
        let NodeKind::Text { text, .. } = scene.kind(n) else {
            continue;
        };
        if text.trim().is_empty()
            || !std::iter::once(n)
                .chain(scene.ancestors(n))
                .all(|a| scene.effects(a).opacity > 0.)
        {
            continue;
        }
        let Some(clip) = scene.ancestors(n).find(|&a| scene.style(a).clip) else {
            continue;
        };
        let (t, c) = (scene.bounds(n), scene.bounds(clip));
        // (Off the clip altogether, a row scrolled away: not a label cut.)
        if t.x >= c.x + c.width || t.y >= c.y + c.height || t.y + t.height <= c.y {
            continue;
        }
        let over = t.x + t.width > c.x + c.width + 0.5;
        out.push((
            text.to_string(),
            over,
            scene.style(clip).fade_edges_x[1] > 0.,
        ));
    }
    out
}

#[test]
fn a_sidebar_label_that_does_not_fit_fades_at_its_end() {
    let mut app = mount();
    assert!(app.host.run_command("long_section"));
    app.frames(30);
    let seen = labels(&mut app);
    let long = seen
        .iter()
        .find(|(t, ..)| t == LONG)
        .expect("the long section's title shows");
    assert!(long.1, "the long title runs past its row");
    for (text, over, fades) in &seen {
        assert_eq!(
            over,
            fades,
            "{text:?}: {}",
            if *over {
                "cut, not faded"
            } else {
                "fits, yet faded"
            }
        );
    }
}

/// The words of a node's subtree, and where they start.
fn words_x(app: &mut TestApp, id: &str) -> f32 {
    let root = app.find(id).unwrap_or_else(|| panic!("{id} shows"));
    let scene = app.ui.scene.borrow();
    let mut stack = vec![root];
    while let Some(n) = stack.pop() {
        stack.extend(scene.children(n).iter().copied());
        if let NodeKind::Text { text, .. } = scene.kind(n)
            && !text.trim().is_empty()
        {
            return scene.bounds(n).x;
        }
    }
    panic!("{id} says nothing")
}

/// Under Star on GitHub, the connection line's words start where the
/// banner's do, whichever glyph leads them (they stood 5 to the left after
/// the offline dot, on them after the reconnecting ring).
#[test]
fn the_foot_s_words_share_one_edge() {
    for state in [
        "connection_offline",
        "connection_unsaved",
        "connection_reconnecting",
    ] {
        let mut app = mount();
        assert!(app.host.run_command(state));
        app.frames(30);
        let (line, star) = (
            words_x(&mut app, "connection-line"),
            words_x(&mut app, "star-banner"),
        );
        assert!(
            (line - star).abs() < 0.5,
            "{state}: the line's words at {line}, Star on GitHub's at {star}"
        );
    }
}
