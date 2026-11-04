//! The title bar beside the window's own buttons, wherever the window
//! reports them (`ui.window.titlebar_area`, `ui.window.controls`, as
//! zgx-desktop's `WindowInfo` has them): Windows' caption buttons at the top
//! right, 46 by 32 each; GNOME's at the right of a 40-point strip; macOS's
//! traffic lights at the left. Mounted headless on the shell's fixture
//! (ui/shell_fixture.luau) with each window's report.
//!
//! And the menus where the system draws no menu bar: none is drawn (no row
//! of zgx's over the title bar, no button for it), and every item of them is
//! in the command palette, running as its menu item does.

use std::{cell::RefCell, rc::Rc};

use zgx::input::{InputEvent, Modifiers, PointerButton};
use zgx::scene::Rect;
use zgx::widgets::WindowGesture;
use zgx_script::{ScriptHost, Value, testing::TestApp};

const W: f32 = 1200.;
const H: f32 = 800.;

/// Zeron's title line beside the traffic lights: 21 down (38 tall, 4 of pad).

/// The room between the page's controls and the window's buttons at the right:
/// a group's gap (8), the divider (1), a group's gap.
const DIVIDED: f32 = 17.
const MAC_LINE: f32 = 21.;

/// What a window reports: its title bar's free strip and its buttons.
#[derive(Clone, Copy, Debug)]
struct Report {
    area: Option<Rect>,
    controls: Option<Rect>,
}

/// Windows: zgx's caption buttons, three of 46 by 32 at the top right.
fn windows(w: f32) -> Report {
    Report { area: Some(Rect::new(0., 0., w - 138., 32.)), controls: Some(Rect::new(w - 138., 0., 138., 32.)) }
}

/// GNOME's `appmenu:minimize,maximize,close`: libadwaita's circles at the
/// right of zgx's 40-point strip.
fn gnome(w: f32) -> Report {
    Report { area: Some(Rect::new(0., 0., w - 104., 40.)), controls: Some(Rect::new(w - 104., 0., 104., 40.)) }
}

/// macOS: the traffic lights' frames, centred 21, 44 and 67 in, 14 across.
fn macos(w: f32) -> Report {
    Report { area: Some(Rect::new(74., 0., w - 74., 40.)), controls: Some(Rect::new(14., 14., 60., 14.)) }
}

fn mount(report: Report) -> TestApp {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui/shell_fixture.luau");
    let app = TestApp::new(|runtime| ScriptHost::new(runtime, path));
    let (area, controls) = (app.ui.signal(report.area), app.ui.signal(report.controls));
    app.host.window_chrome(area, controls);
    app.ui.scene.borrow_mut().resize(W, H);
    app.frames(30);
    assert_eq!(app.error(), None, "the shell failed");
    app
}

fn mid(b: Rect) -> f32 {
    b.y + b.height / 2.
}

const TOGGLE: &str = "#titlebar >> role:button \"Toggle sidebar\"";
const BACK: &str = "#titlebar >> role:button \"Back\"";
const FORWARD: &str = "#titlebar >> role:button \"Forward\"";
const PANEL: &str = "#titlebar >> role:button \"Panel\"";
const VIEW: &str = "role:button \"Sidebar view options\"";

/// What a press (and release) at `(x, y)` asks of the window.
fn press(app: &mut TestApp, x: f32, y: f32) -> Option<WindowGesture> {
    let _ = app.ui.take_window_gesture();
    let button = PointerButton::Primary;
    app.ui.dispatch_with_modifiers(InputEvent::PointerMove { x, y }, Modifiers::default());
    app.ui.dispatch_with_modifiers(InputEvent::PointerDown { x, y, button }, Modifiers::default());
    let gesture = app.ui.take_window_gesture();
    app.ui.dispatch_with_modifiers(InputEvent::PointerUp { x, y, button }, Modifiers::default());
    app.ui.input.forget_clicks();
    app.settle();
    gesture
}

/// Everything on the bar on one line: the bar's own controls, the session's
/// title, the panel's toggle and the sidebar's "…", their middles on `line`.
fn on_line(app: &mut TestApp, line: f32, when: &str) {
    for target in [TOGGLE, BACK, FORWARD, PANEL, VIEW, "#page-title"] {
        if app.find(target).is_none() {
            continue;
        }
        let b = app.bounds(target);
        if b.width <= 0. {
            continue;
        }
        assert!((mid(b) - line).abs() <= 0.5, "{when}: {target} {b:?} isn't on the line {line}");
    }
}

#[test]
fn beside_windows_caption_buttons_zerons_controls_start_at_the_left_on_their_line() {
    let mut app = mount(windows(W));
    let caption = W - 138.;
    // No gap kept for traffic lights: the toggle 10 in, as the shipping
    // app's on Windows; Back and Forward after it; all on the caption
    // buttons' middle (16 down), not 21.
    let toggle = app.bounds(TOGGLE);
    assert_eq!(toggle.x, 10., "the toggle {toggle:?}");
    on_line(&mut app, 16., "a new chat");

    // A session: its title and the panel's toggle on that line, clear of the
    // caption buttons (the toggle as far in from them as from an edge).
    app.click("#shell-sidebar >> role:button \"Fix the build\"");
    app.frames(30);
    on_line(&mut app, 16., "a session");
    let panel = app.bounds(PANEL);
    assert!((panel.x + panel.width - (caption - DIVIDED)).abs() <= 0.5, "the panel's toggle {panel:?} isn't a divider's room before the caption buttons (from {caption})");
    let title = app.bounds("#page-title");
    assert!(title.x + title.width <= panel.x, "the title {title:?} runs under the panel's toggle {panel:?}");

    // Collapsed: the title starts past the controls, as the shipping app's
    // (10 + its 82 of controls + 8), still on the line.
    app.click(TOGGLE);
    app.frames(40);
    let (title, forward) = (app.bounds("#page-title"), app.bounds(FORWARD));
    assert!(title.x >= forward.x + forward.width, "the title {title:?} under the controls (Forward {forward:?})");
    assert_eq!(title.x, 100., "the title {title:?}");
    on_line(&mut app, 16., "collapsed");

    // The strip still moves the window, under the caption buttons' line,
    // but for the controls.
    for x in [60., 400., 700., caption - 60.] {
        let b = [TOGGLE, BACK, FORWARD, PANEL].iter().filter(|t| app.find(t).is_some()).map(|t| app.bounds(t)).any(|b| x >= b.x - 2. && x <= b.x + b.width + 2.);
        if !b {
            assert_eq!(press(&mut app, x, 16.), Some(WindowGesture::Drag), "a press at {x} on the caption buttons' line");
        }
    }
}

#[test]
fn the_panels_strip_clears_windows_caption_buttons_on_their_line() {
    let mut app = mount(windows(W));
    app.click("#shell-sidebar >> role:button \"Fix the build\"");
    app.frames(10);
    app.click(PANEL);
    app.frames(40);
    let strip = app.bounds("#panel-strip");
    assert!(strip.width > 0., "the panel's strip {strip:?}");
    for tab in ["role:tab"] {
        if app.find(tab).is_some() {
            let b = app.bounds(tab);
            assert!((mid(b) - 16.).abs() <= 0.5, "a tab {b:?} off the caption buttons' line");
            assert!(b.x + b.width <= W - 138., "a tab {b:?} under the caption buttons");
        }
    }
}

#[test]
fn beside_gnomes_buttons_at_the_right_the_line_stays_and_the_toggle_starts_at_the_left() {
    let mut app = mount(gnome(W));
    // (Their strip, 40, is the bar's height or more: the bar's own line.)
    let toggle = app.bounds(TOGGLE);
    assert_eq!(toggle.x, 10., "the toggle {toggle:?}");
    on_line(&mut app, MAC_LINE, "a new chat");
    app.click("#shell-sidebar >> role:button \"Fix the build\"");
    app.frames(30);
    let panel = app.bounds(PANEL);
    assert!((panel.x + panel.width - (W - 104. - DIVIDED)).abs() <= 0.5, "the panel's toggle {panel:?} isn't a divider's room before the window's buttons");
}

/// macOS exactly as before: the toggle 88 in beside the lights, the line 21
/// down, the collapsed title past the controls' island (180) by 4.
#[test]
fn beside_the_traffic_lights_nothing_moves() {
    for (report, when) in [(macos(W), "the lights reported"), (Report { area: None, controls: None }, "nothing reported (full screen)")] {
        let mut app = mount(report);
        assert_eq!(app.bounds(TOGGLE).x, 88., "{when}");
        on_line(&mut app, MAC_LINE, when);
        app.click("#shell-sidebar >> role:button \"Fix the build\"");
        app.frames(30);
        on_line(&mut app, MAC_LINE, when);
        app.click(TOGGLE);
        app.frames(40);
        assert_eq!(app.bounds("#page-title").x, 184., "{when}");
    }
}

/// The window resized, maximised, its buttons kept to their corner: what
/// clears them follows.
#[test]
fn the_bar_follows_the_window_as_it_resizes() {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui/shell_fixture.luau");
    let mut app = TestApp::new(|runtime| ScriptHost::new(runtime, path));
    let report = windows(W);
    let (area, controls) = (app.ui.signal(report.area), app.ui.signal(report.controls));
    app.host.window_chrome(area.clone(), controls.clone());
    app.ui.scene.borrow_mut().resize(W, H);
    app.frames(30);
    app.click("#shell-sidebar >> role:button \"Fix the build\"");
    app.frames(30);
    for w in [1600., 900.] {
        let r = windows(w);
        app.ui.scene.borrow_mut().resize(w, H);
        area.set(r.area);
        controls.set(r.controls);
        app.frames(30);
        let panel = app.bounds(PANEL);
        assert!(panel.x + panel.width <= w - 138. - DIVIDED + 0.5, "{w}: the panel's toggle {panel:?}");
        assert!(panel.x + panel.width >= w - 138. - DIVIDED - 0.5, "{w}: the panel's toggle {panel:?} isn't beside the caption buttons");
    }
    // Full screen: no buttons reported; the bar as beside nothing.
    area.set(None);
    controls.set(None);
    app.frames(30);
    let panel = app.bounds(PANEL);
    assert!(panel.x + panel.width >= 900. - 10. - 0.5, "full screen: the panel's toggle {panel:?} at the edge");
}

// ── The menus ───────────────────────────────────────────────────────────────

/// Where the system draws no menu bar, the host draws none either: the
/// main window's chrome is its toasts' layer alone.
#[test]
fn no_menu_bar_is_drawn_over_the_title_bar() {
    let source = include_str!("plugins/host_bar.rs");
    let code: String = source.lines().filter(|l| !l.trim_start().starts_with("//")).collect::<Vec<_>>().join("\n");
    assert!(!code.contains("app_menu_bar("), "the host mounts zgx's drawn menu bar");
    // (The main window's composition: nothing above the scripts' view but a
    // layer, so the shell fills the window from its top.)
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui/shell_fixture.luau");
    let mut app = TestApp::wrapped(
        move |runtime| ScriptHost::new(runtime, path),
        |scripts| crate::plugins::host_bar::compose(zgx::compose::column(), scripts),
    );
    app.ui.scene.borrow_mut().resize(W, H);
    app.frames(30);
    let shell = app.bounds("#shell");
    assert_eq!((shell.y, shell.height), (0., H), "the shell under something at the window's top: {shell:?}");
    let texts = app.texts().join("|");
    assert!(!texts.split('|').any(|t| t == "File" || t == "Edit" || t == "Go"), "a menu bar's titles: {texts}");
}

/// A record's text field, or "".
fn text(record: &Value, key: &str) -> String {
    match record.field(key) {
        Some(Value::Str(s)) => s.to_string(),
        _ => String::new(),
    }
}

/// Mounts the shell with what `serve_commands` publishes for the palette,
/// and hears each item the palette runs (`app.run_menu_command`).
fn with_menus() -> (TestApp, Rc<RefCell<Vec<String>>>) {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui/shell_fixture.luau");
    let ran: Rc<RefCell<Vec<String>>> = Rc::default();
    let heard = ran.clone();
    let app = TestApp::new(move |runtime| {
        let host = ScriptHost::new(runtime, path);
        host.action("app.run_menu_command", move |id| {
            if let Value::Str(id) = id {
                heard.borrow_mut().push(id.to_string());
            }
        });
        host
    });
    app.ui.scene.borrow_mut().resize(W, H);
    app.frames(30);
    assert_eq!(app.error(), None, "the shell failed");
    let all = app.host.commands().get();
    let listed = crate::palette_commands(&crate::menus(&all), &all);
    app.host.shared("app.menu_commands", Value::List(Rc::from([]))).set(listed);
    app.frames(10);
    (app, ran)
}

/// Every menu item, each by its title: found in the palette, and run as
/// its menu item would be (the id it's run by). The host's way back to the
/// default interface among them.
#[test]
fn every_menu_item_is_in_the_command_palette() {
    let (mut app, ran) = with_menus();
    let all = app.host.commands().get();
    let items = crate::palette_commands(&crate::menus(&all), &all);
    let items = items.list().to_vec();
    assert!(items.len() >= 25, "the menus hold {} items", items.len());
    let field = "#palette-search";
    // (On a session, where Archive session is one of the palette's own.)
    app.click("#shell-sidebar >> role:button \"Fix the build\"");
    app.frames(30);
    // The palette's own rows for these (they run themselves, and are found
    // by their own words); the rest are listed from the menus, each run by
    // its menu item's id.
    let own = ["shell.new_chat", "shell.new_project", "shell.archive", "shell.toggle_sidebar", "shell.settings", "shell.reload", "shell.reload_interface"];
    for item in &items {
        let (id, title) = (text(item, "id"), text(item, "title"));
        // (Search is the palette itself.)
        if id == "shell.palette" {
            continue;
        }
        assert!(app.host.run_command("shell.palette"), "the palette didn't open");
        app.wait_until(|app| app.find(field).is_some());
        app.frames(20);
        let label = if id == crate::plugins::USE_ZERON_TEMPLATE { "Use the default interface".to_owned() } else { title.clone() };
        app.type_text(field, &label);
        app.frames(20);
        let row = format!("role:option ~\"{label}\"");
        assert!(app.find(&row).is_some(), "{id} (\"{title}\") isn't in the palette: {}", app.texts().join("|"));
        if !own.contains(&id.as_str()) {
            ran.borrow_mut().clear();
            app.click(&row);
            app.frames(20);
            assert_eq!(*ran.borrow(), vec![id.clone()], "{id}: the palette ran something else");
        }
        // (Closed, whatever the row did: Escape clears, then closes.)
        for _ in 0..3 {
            if app.find(field).is_some() {
                app.key(zgx::input::Key::Escape);
                app.frames(10);
            }
        }
    }
    let ids: Vec<String> = items.iter().map(|i| text(i, "id")).collect();
    assert!(ids.iter().any(|i| i == crate::plugins::USE_ZERON_TEMPLATE), "the way back to the default interface: {ids:?}");
    assert!(!ids.iter().any(|i| i == crate::plugins::KEEP_TEMPLATE), "Keep is the preview toast's alone");
}
