//! Where the title bar's controls (the sidebar's toggle, back, forward)
//! start, for each system and each state of the window, as zgx-desktop
//! reports them (`ui.window.titlebar_area`, `controls`, `full_screen`).
//!
//! Only macOS has traffic lights at the bar's left, so only there is room
//! kept for them (88 in; the window's edge inset, 12, in full screen). On
//! Linux and Windows nothing stands there in any state, floating, tiled,
//! maximised or full screen, with buttons reported or none (a tiling
//! compositor such as Hyprland draws none; full screen hides them): the
//! row's 10, or past the window's own buttons where the desktop puts them at
//! the left. Where nothing says what system it is (headless, a preview) the
//! bar is drawn as on macOS.
//!
//! Mounted headless on the shell's fixture (ui/shell_fixture.luau).

use std::path::PathBuf;

use zgx::reactive::Signal;
use zgx::scene::Rect;
use zgx_script::{ScriptHost, testing::TestApp};

const W: f32 = 1200.;
const TOGGLE: &str = "role:button \"Toggle sidebar\"";
const FORWARD: &str = "role:button \"Forward\"";

fn ui_dir() -> PathBuf {
    std::env::var_os("ZERON_ZGX_TEST_UI")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui"))
}

/// A window's state as the host reports it: in full screen or not, the
/// title bar's free strip, the window's buttons.
#[derive(Clone, Copy, Debug)]
struct State {
    name: &'static str,
    full: bool,
    area: Option<Rect>,
    controls: Option<Rect>,
}

/// Buttons at the right of zgx's 40-point strip, `span` across (zgx-desktop
/// window_chrome.rs `area_for`): GNOME's `appmenu:close` is 40, its
/// default `minimize,maximize,close` 104; Windows' caption buttons 138 by 32.
fn right(name: &'static str, span: f32, h: f32) -> State {
    State {
        name,
        full: false,
        area: Some(Rect::new(0., 0., W - span, h)),
        controls: Some(Rect::new(W - span, 0., span, h)),
    }
}

/// No buttons in the bar: a `button-layout` of none, or a frame the
/// compositor keeps them off; the strip the window's whole width.
fn none_drawn() -> State {
    State {
        name: "no buttons drawn",
        full: false,
        area: Some(Rect::new(0., 0., W, 40.)),
        controls: None,
    }
}

/// Full screen: neither the strip nor the buttons reported.
fn full_screen() -> State {
    State {
        name: "full screen",
        full: true,
        area: None,
        controls: None,
    }
}

/// Nothing reported, out of full screen (as a window is before its first
/// report, or a compositor's maximised window that says none).
fn unreported() -> State {
    State {
        name: "nothing reported",
        full: false,
        area: None,
        controls: None,
    }
}

/// Linux's states: floating, tiled and maximised report the same strip
/// (zgx draws its buttons in each), so each layout stands for all three.
fn linux_states() -> Vec<State> {
    vec![
        right("floating / tiled / maximised, appmenu:close", 40., 40.),
        right(
            "floating / tiled / maximised, minimize,maximize,close",
            104.,
            40.,
        ),
        none_drawn(),
        unreported(),
        full_screen(),
    ]
}

fn windows_states() -> Vec<State> {
    vec![
        right("floating / maximised, caption buttons", 138., 32.),
        unreported(),
        full_screen(),
    ]
}

/// The shell's fixture on `platform` ("" for none said), in `state`.
fn mount(platform: &str, state: State) -> TestApp {
    let platform = platform.to_owned();
    let app = TestApp::new(move |runtime| {
        let host = ScriptHost::new(runtime, ui_dir().join("shell_fixture.luau"));
        if !platform.is_empty() {
            host.shared("platform", platform.as_str());
        }
        host.window_chrome(runtime.signal(state.area), runtime.signal(state.controls));
        host.window_full_screen(runtime.signal(state.full), || None);
        host
    });
    app.ui.scene.borrow_mut().resize(W, 800.);
    app.frames(60);
    assert_eq!(app.error(), None, "the shell failed to load");
    app
}

fn toggle_x(app: &mut TestApp) -> f32 {
    app.bounds(TOGGLE).x
}

#[test]
fn on_linux_the_controls_start_at_the_rows_padding_in_every_state() {
    for state in linux_states() {
        let mut app = mount("linux", state);
        assert_eq!(
            toggle_x(&mut app),
            10.,
            "linux, {}: no room kept for traffic lights",
            state.name
        );
        // Back and Forward follow it, no gap before them either.
        let forward = app.bounds(FORWARD);
        assert!(
            forward.x < 88.,
            "linux, {}: Forward {forward:?} pushed right",
            state.name
        );
    }
}

#[test]
fn on_windows_the_controls_start_at_the_rows_padding_in_every_state() {
    for state in windows_states() {
        let mut app = mount("windows", state);
        assert_eq!(
            toggle_x(&mut app),
            10.,
            "windows, {}: no room kept for traffic lights",
            state.name
        );
    }
}

/// A Linux desktop that puts its buttons at the left (`close,minimize,maximize:`,
/// 104 across with their margins; `close:`, 40): the controls start past
/// them, not at the lights' 88.
#[test]
fn on_linux_with_buttons_at_the_left_the_controls_start_past_them() {
    for span in [104., 40.] {
        let state = State {
            name: "buttons at the left",
            full: false,
            area: Some(Rect::new(span, 0., W - span, 40.)),
            controls: Some(Rect::new(0., 0., span, 40.)),
        };
        let mut app = mount("linux", state);
        assert_eq!(
            toggle_x(&mut app),
            span,
            "linux, {span} of buttons at the left"
        );
    }
}

/// Going maximised or full screen and back on Linux moves nothing.
#[test]
fn on_linux_the_controls_hold_still_as_the_window_changes_state() {
    let slots: std::rc::Rc<
        std::cell::RefCell<Option<(Signal<bool>, Signal<Option<Rect>>, Signal<Option<Rect>>)>>,
    > = Default::default();
    let given = slots.clone();
    let floating = right("floating", 40., 40.);
    let mut app = TestApp::new(move |runtime| {
        let host = ScriptHost::new(runtime, ui_dir().join("shell_fixture.luau"));
        host.shared("platform", "linux");
        let full = runtime.signal(false);
        let area = runtime.signal(floating.area);
        let controls = runtime.signal(floating.controls);
        host.window_chrome(area.clone(), controls.clone());
        host.window_full_screen(full.clone(), || None);
        *given.borrow_mut() = Some((full, area, controls));
        host
    });
    app.ui.scene.borrow_mut().resize(W, 800.);
    app.frames(60);
    assert_eq!(app.error(), None);
    let (full, area, controls) = slots.borrow_mut().take().unwrap();
    assert_eq!(toggle_x(&mut app), 10., "floating");
    for state in [full_screen(), unreported(), none_drawn(), floating] {
        full.set(state.full);
        area.set(state.area);
        controls.set(state.controls);
        app.frames(30);
        assert_eq!(toggle_x(&mut app), 10., "after {}", state.name);
    }
}

/// macOS as ever: 88 beside the lights, the edge inset in full screen, 88
/// before the lights are reported.
#[test]
fn on_macos_room_is_kept_for_the_traffic_lights() {
    let lights = State {
        name: "the lights reported",
        full: false,
        area: Some(Rect::new(72., 0., W - 72., 40.)),
        controls: Some(Rect::new(12., 13., 60., 14.)),
    };
    for (state, want) in [(lights, 88.), (unreported(), 88.), (full_screen(), 12.)] {
        let mut app = mount("macos", state);
        assert_eq!(toggle_x(&mut app), want, "macos, {}", state.name);
    }
}

/// Told no system (headless, a preview): drawn as on macOS.
#[test]
fn told_no_system_the_bar_is_drawn_as_on_macos() {
    assert_eq!(toggle_x(&mut mount("", unreported())), 88.);
}
