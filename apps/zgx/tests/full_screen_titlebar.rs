//! In macOS full screen the title bar's controls (the sidebar's toggle, back,
//! forward) slide over to where the hidden traffic lights were, and back as
//! the window comes out, as Zeron's do (crates/ui shell.rs
//! `titlebar_cluster_start`: 88 in, 12 in full screen, a 200 ms ease out).
//! The window draws no frames while macOS animates it in or out, so they
//! hold still through the system's animation and slide once it's done.
//!
//! The shell's fixture under a host that says what macOS says
//! (`ScriptHost::window_full_screen`, `window_chrome`), as measured on this
//! Mac: the lights 12 in and 60 across; going in, the transition (0.35 s)
//! with `full_screen` turning, the lights hidden; coming out (0.069 s), the
//! lights at 9,9 while it runs, at 12,13 once it's done. The toggle's box is
//! read off the scene each frame: still through the transition, then a
//! slide that lands on each, with no script run while it moves (the engine
//! animates it; one call starts it, once what macOS says has settled).

use std::{cell::RefCell, path::PathBuf, rc::Rc};

use zgx::reactive::Signal;
use zgx::scene::Rect;
use zgx_script::{ScriptHost, testing::TestApp};

fn ui_dir() -> PathBuf {
    std::env::var_os("ZERON_ZGX_TEST_UI")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui"))
}

/// What the host tells the window: full screen, the transition (going in,
/// and its seconds), and the traffic lights.
struct Window {
    full: Signal<bool>,
    moving: Signal<Option<(bool, f64)>>,
    lights: Signal<Option<Rect>>,
}

const LIGHTS: Rect = Rect {
    x: 12.,
    y: 13.,
    width: 60.,
    height: 14.,
};
const TOGGLE: &str = "role:button \"Toggle sidebar\"";

fn shell() -> (TestApp, Window) {
    let slot: Rc<RefCell<Option<Window>>> = Rc::default();
    let given = slot.clone();
    let app = TestApp::new(move |runtime| {
        let host = ScriptHost::new(runtime, ui_dir().join("shell_fixture.luau"));
        host.shared("platform", "macos");
        let window = Window {
            full: runtime.signal(false),
            moving: runtime.signal(None),
            lights: runtime.signal(Some(LIGHTS)),
        };
        host.window_chrome(
            runtime.signal(Some(Rect {
                x: 72.,
                y: 0.,
                width: 1128.,
                height: 40.,
            })),
            window.lights.clone(),
        );
        let moving = window.moving.clone();
        host.window_full_screen(window.full.clone(), move || moving.get());
        *given.borrow_mut() = Some(window);
        host
    });
    app.ui.scene.borrow_mut().resize(1200., 800.);
    app.frames(60);
    assert_eq!(app.error(), None, "the shell failed to load");
    let window = slot.borrow_mut().take().expect("the host was made");
    (app, window)
}

fn toggle_x(app: &mut TestApp) -> f32 {
    app.bounds(TOGGLE).x
}

/// The toggle's x each frame for `frames` frames, and the scripts' calls in
/// all but the first (the one the change itself runs in).
fn watch(app: &mut TestApp, frames: usize) -> (Vec<f32>, u64) {
    app.frame();
    let mut xs = vec![toggle_x(app)];
    let calls = app.calls();
    for _ in 1..frames {
        app.frame();
        xs.push(toggle_x(app));
    }
    (xs, app.calls() - calls)
}

#[test]
fn the_controls_slide_to_the_lights_place_in_full_screen_and_back() {
    let (mut app, window) = shell();
    assert_eq!(
        toggle_x(&mut app),
        88.,
        "out of full screen: 16 past the lights, Zeron's 88"
    );

    // Going in: the transition begins, then the window turns full screen;
    // nothing moves while macOS animates it.
    window.moving.set(Some((true, 0.35)));
    window.full.set(true);
    window.lights.set(None);
    app.frames(21);
    assert_eq!(toggle_x(&mut app), 88., "still while the system animates");
    // Done: once it has settled (50 ms, one call to start it), they slide
    // over (200 ms), the engine moving them.
    window.moving.set(None);
    app.frames(4);
    let (xs, calls) = watch(&mut app, 16);
    assert!(xs[5] < 88. && xs[5] > 12., "on their way: {xs:?}");
    assert!(xs.windows(2).all(|w| w[1] <= w[0]), "only left: {xs:?}");
    assert_eq!(
        *xs.last().unwrap(),
        12.,
        "at the window's edge inset: {xs:?}"
    );
    assert_eq!(calls, 0, "no script runs while they slide");

    // Coming out: the lights come back where macOS puts them meanwhile
    // (9,9), then at their place.
    window.moving.set(Some((false, 0.069)));
    window.full.set(false);
    window.lights.set(Some(Rect {
        x: 9.,
        y: 9.,
        width: 60.,
        height: 14.,
    }));
    app.frames(10);
    assert_eq!(toggle_x(&mut app), 12., "still while the system animates");
    window.moving.set(None);
    // (The lights come back a moment after the transition ends.)
    app.frame();
    window.lights.set(Some(LIGHTS));
    app.frames(4);
    let (xs, calls) = watch(&mut app, 16);
    assert!(xs[5] > 12. && xs[5] < 88., "on their way back: {xs:?}");
    assert!(xs.windows(2).all(|w| w[1] >= w[0]), "only right: {xs:?}");
    assert_eq!(*xs.last().unwrap(), 88., "back past the lights: {xs:?}");
    assert_eq!(calls, 0, "no script runs while they slide back");
}

#[test]
fn off_macos_the_controls_stay_where_they_are() {
    // Linux and Windows: no transition, no traffic lights; full screen
    // changes nothing of the bar (another layout is theirs).
    let slot: Rc<RefCell<Option<Signal<bool>>>> = Rc::default();
    let given = slot.clone();
    let mut app = TestApp::new(move |runtime| {
        let host = ScriptHost::new(runtime, ui_dir().join("shell_fixture.luau"));
        host.shared("platform", "linux");
        let full = runtime.signal(false);
        host.window_full_screen(full.clone(), || None);
        *given.borrow_mut() = Some(full);
        host
    });
    app.ui.scene.borrow_mut().resize(1200., 800.);
    app.frames(60);
    assert_eq!(app.error(), None);
    let full = slot.borrow_mut().take().unwrap();
    assert_eq!(toggle_x(&mut app), 88.);
    full.set(true);
    app.frames(30);
    assert_eq!(toggle_x(&mut app), 88.);
}
