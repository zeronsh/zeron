//! The composer between a new chat and a session (ui/screens/chat/composer,
//! mounted headless on ui/shell_fixture.luau): another chat opened or a new
//! chat puts it where it goes at once, no fade and no slide. (The Lua tests can't read an opacity:
//! ui/tests/shell_route_motion.test.luau has the panel's side of it.)

use std::time::Duration;

use zgx_script::{ScriptHost, testing::TestApp};

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

/// A frame and a little of the clock, `n` times: each look at the dock's
/// opacity, the composer's top and its height (the pill's, with what hangs
/// off it).
fn watch(app: &mut TestApp, n: usize) -> Vec<(f32, f32, f32)> {
    (0..n)
        .map(|_| {
            app.frames(1);
            std::thread::sleep(Duration::from_millis(12));
            let b = app.bounds("chat-composer");
            (app.opacity("chat-composer-dock"), b.y, b.height)
        })
        .collect()
}

/// Every look after a switch: shown in full (no fade), at its final height
/// (nothing growing or shrinking), and already where it ends (`to`), give
/// or take a frame of its size being measured (a quarter of the way
/// between the two places; a slide would pass through the rest).
fn at_once(seen: &[(f32, f32, f32)], from: f32, to: f32, what: &str) {
    let way = (from - to).abs() / 4.;
    let tall = seen.last().unwrap().2;
    for (i, (opacity, y, h)) in seen.iter().enumerate() {
        assert!((opacity - 1.0).abs() < 0.01, "{what}: faded, frame {}: opacity {opacity}", i + 1);
        assert!((h - tall).abs() <= 0.5, "{what}: frame {} {h} tall, growing to {tall}", i + 1);
        assert!((y - to).abs() <= way, "{what}: frame {} at {y}, not yet where it goes ({to}, from {from})", i + 1);
    }
}

#[test]
fn a_new_chat_and_a_session_switch_the_composer_at_once() {
    let mut app = mount();
    app.click("#shell-sidebar >> role:button \"Fix the build\"");
    watch(&mut app, 60);
    let docked = app.bounds("chat-composer").y;
    assert!((app.opacity("chat-composer-dock") - 1.0).abs() < 0.01, "the composer shows, docked");

    // To a new chat: mid-page from the first frame, never faded.
    app.click("#shell-sidebar >> role:button \"New chat\"");
    let seen = watch(&mut app, 60);
    let hero = app.bounds("chat-composer").y;
    assert!(hero < docked - 50., "it stands mid-page on a new chat ({hero} vs {docked})");
    at_once(&seen, docked, hero, "to a new chat");

    // And back to the session: docked from the first frame.
    app.click("#shell-sidebar >> role:button \"Fix the build\"");
    let seen = watch(&mut app, 60);
    assert!((app.bounds("chat-composer").y - docked).abs() <= 1., "docked again");
    at_once(&seen, hero, docked, "back to the session");

    // Growing in the same chat (lines typed) still eases: seen on its way.
    std::thread::sleep(Duration::from_millis(500));
    let short = app.bounds("chat-composer").height;
    app.fill("#chat-composer >> role:textbox", "one\ntwo\nthree\nfour");
    let seen = watch(&mut app, 40);
    let tall = seen.last().unwrap().2;
    assert!(tall > short + 20., "the lines opened it out ({short} → {tall})");
    assert!(
        seen.iter().any(|(_, _, h)| *h > short + 1. && *h < tall - 1.),
        "it grew at once, not eased ({short} → {tall}): {:?}",
        seen.iter().map(|s| s.2).collect::<Vec<_>>()
    );
}
