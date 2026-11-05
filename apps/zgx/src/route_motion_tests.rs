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

/// Where a new chat's composer stands on the page: its middle across from the
/// page's, and its top down from the page's middle.
fn on_page(app: &mut TestApp) -> (f32, f32) {
    let c = app.bounds("#chat-composer");
    let p = app.bounds("#shell-page");
    ((c.x + c.width / 2.) - (p.x + p.width / 2.), c.y - (p.y + p.height / 2.))
}

/// A frame, a 60 Hz frame's time after the last: what eases on the clock
/// (a glide after the page) is seen on its way.
fn tick(app: &mut TestApp) {
    std::thread::sleep(Duration::from_millis(16));
    app.frame();
}

/// A press on `edge`, dragged `dx` in `n` steps, a frame each: `look` after
/// each frame, and for a few after it's let go.
fn drag_edge(app: &mut TestApp, edge: &str, dx: f32, n: usize, look: &mut dyn FnMut(&mut TestApp, String)) {
    use zgx::input::{InputEvent, Modifiers, PointerButton};
    let b = app.bounds(edge);
    let (x, y) = (b.x + b.width / 2., b.y + b.height / 2.);
    let (button, m) = (PointerButton::Primary, Modifiers::default());
    app.ui.dispatch_with_modifiers(InputEvent::PointerMove { x, y }, m);
    app.ui.dispatch_with_modifiers(InputEvent::PointerDown { x, y, button }, m);
    for step in 1..=n {
        let to = x + dx * step as f32 / n as f32;
        app.ui.dispatch_with_modifiers(InputEvent::PointerMove { x: to, y }, m);
        tick(app);
        look(app, format!("{edge} dragged to {to}"));
    }
    app.ui.dispatch_with_modifiers(InputEvent::PointerUp { x: x + dx, y, button }, m);
    app.ui.input.forget_clicks();
    for f in 1..=10 {
        tick(app);
        look(app, format!("{edge} let go, frame {f}"));
    }
}

/// A new chat's composer holds its place on the page as the page is resized:
/// the window narrowed and widened, made shorter and taller, the sidebar's
/// edge dragged and the panel's. At every frame, the first after each step
/// included, it stands where it stood on the page (centred across, the same
/// way down from the page's middle): it moves with the page in the same
/// frame, never a frame late, never gliding after it.
#[test]
fn a_new_chats_composer_holds_its_place_as_the_page_resizes() {
    let mut app = mount();
    // (Settled, and past the moment after the sidebar's first fit when its
    // place follows at once.)
    for _ in 0..40 {
        tick(&mut app);
    }
    let rest = on_page(&mut app);
    assert!(rest.0.abs() <= 0.5, "a new chat's composer stands centred on the page: {rest:?}");
    let mut faults = Vec::new();
    let mut look = |app: &mut TestApp, what: String| {
        let now = on_page(app);
        if (now.0 - rest.0).abs() > 0.5 || (now.1 - rest.1).abs() > 0.5 {
            faults.push(format!("{what}: {now:?} on the page, was {rest:?}"));
        }
    };
    // The window, a few points a frame, as a hand drags its corner: narrower
    // and shorter, then back.
    let down: Vec<(f32, f32)> = (0..60).map(|i| (1200. - 8. * i as f32, 800. - 4. * i as f32)).collect();
    let up: Vec<(f32, f32)> = down.iter().rev().copied().collect();
    for (w, h) in down.into_iter().chain(up) {
        app.ui.scene.borrow_mut().resize(w, h);
        tick(&mut app);
        look(&mut app, format!("the window at {w}x{h}"));
    }
    for f in 1..=20 {
        tick(&mut app);
        look(&mut app, format!("the window let go, frame {f}"));
    }
    drag_edge(&mut app, "\"Resize sidebar\"", 120., 12, &mut look);
    drag_edge(&mut app, "\"Resize sidebar\"", -120., 12, &mut look);
    // The panel, open beside a new chat: its edge both ways.
    app.click("#titlebar >> role:button \"Panel\"");
    for _ in 0..40 {
        tick(&mut app);
    }
    drag_edge(&mut app, "\"Resize files\"", -120., 12, &mut look);
    drag_edge(&mut app, "\"Resize files\"", 160., 12, &mut look);
    assert!(faults.is_empty(), "{} frames moved it:\n{}", faults.len(), faults.join("\n"));
}
