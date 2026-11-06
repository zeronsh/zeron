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

/// The room between the page's controls and the window's buttons at the right,
/// one rhythm by ink: the panel toggle's glyph (12 of ink in its 24) to
/// Minimize's (10 in its 46) as far as one caption glyph to the next (36).
const DIVIDED: f32 = 12.;
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

/// macOS: the traffic lights' frames as zgx's inset title bar reports them,
/// 12 in and 60 across.
fn macos(w: f32) -> Report {
    Report { area: Some(Rect::new(72., 0., w - 72., 40.)), controls: Some(Rect::new(12., 13., 60., 16.)) }
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
const TAB: &str = "#session-tabs >> role:tab";

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

/// Everything on the bar on one line: the bar's own controls, the panel's
/// toggle and the sidebar's "…", their middles on `line`.
fn on_line(app: &mut TestApp, line: f32, when: &str) {
    for target in [TOGGLE, BACK, FORWARD, PANEL, VIEW] {
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

    // A session: the panel's toggle on that line, clear of the caption
    // buttons (the toggle as far in from them as from an edge), and its tab
    // short of the toggle.
    app.click("#shell-sidebar >> role:button \"Fix the build\"");
    app.frames(30);
    on_line(&mut app, 16., "a session");
    let panel = app.bounds(PANEL);
    assert!((panel.x + panel.width - (caption - DIVIDED)).abs() <= 0.5, "the panel's toggle {panel:?} isn't one rhythm's room before the caption buttons (from {caption})");
    let tab = app.bounds(TAB);
    assert!(tab.x + tab.width <= panel.x, "the tab {tab:?} runs under the panel's toggle {panel:?}");

    // Collapsed: the tabs start past the controls, as the shipping app's title
    // did (10 + its 82 of controls + 8), the controls still on the line.
    app.click(TOGGLE);
    app.frames(40);
    let (tab, forward) = (app.bounds(TAB), app.bounds(FORWARD));
    assert!(tab.x >= forward.x + forward.width, "the tab {tab:?} under the controls (Forward {forward:?})");
    on_line(&mut app, 16., "collapsed");

    // The strip still moves the window, under the caption buttons' line,
    // but for the controls and the tab.
    for x in [60., 400., 700., caption - 60.] {
        let mut boxes = Vec::new();
        for t in [TOGGLE, BACK, FORWARD, PANEL, TAB] {
            if app.find(t).is_some() {
                boxes.push(app.bounds(t));
            }
        }
        let b = boxes.iter().any(|b| x >= b.x - 2. && x <= b.x + b.width + 2.);
        if !b {
            assert_eq!(press(&mut app, x, 16.), Some(WindowGesture::Drag), "a press at {x} on the caption buttons' line");
        }
    }
    let tab = app.bounds(TAB);
    assert!((tab.x - 100.).abs() <= 1., "the tab {tab:?}");
}

/// The panel's tabs stand in its card, under the caption buttons. What stands on
/// their line is the top bar's: the chats' tabs and Expand, clear of them.
#[test]
fn the_panels_strip_clears_windows_caption_buttons_on_their_line() {
    let mut app = mount(windows(W));
    app.click("#shell-sidebar >> role:button \"Fix the build\"");
    app.frames(10);
    app.click(PANEL);
    app.frames(40);
    let strip = app.bounds("#panel-strip");
    assert!(strip.width > 0., "the panel's strip {strip:?}");
    assert!(strip.y >= 32., "the panel's strip {strip:?} under the caption buttons");
    let expand = app.bounds("#panel-expand");
    assert!((mid(expand) - 16.).abs() <= 0.5, "Expand {expand:?} off the caption buttons' line");
    assert!(expand.x + expand.width <= W - 138., "Expand {expand:?} under the caption buttons");
    let tab = app.bounds(TAB);
    assert!(tab.x + tab.width <= W - 138., "a tab {tab:?} under the caption buttons");
    assert!((mid(tab) - 16.).abs() <= 0.5, "a tab {tab:?} off the caption buttons' line");
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
    assert!((panel.x + panel.width - (W - 104. - 10.)).abs() <= 0.5, "the panel's toggle {panel:?} isn't the row's padding before the window's buttons");
}

/// macOS exactly as before: the toggle 88 in beside the lights, the line 21
/// down, and collapsed, the tabs past the controls (Forward's end, 172) by 8, on
/// that line.
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
        let (tab, forward) = (app.bounds(TAB), app.bounds(FORWARD));
        assert!((tab.x - (forward.x + forward.width + 8.)).abs() <= 1., "{when}: the tab {tab:?} (Forward {forward:?})");
        assert!((mid(tab) - MAC_LINE).abs() <= 0.5, "{when}: the tab {tab:?} isn't on the line");
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

/// The host calls the shell fixture's stub heard (`settings_stub_calls`).
fn stub_calls(app: &TestApp) -> Vec<String> {
    app.host
        .states()
        .into_iter()
        .find(|(n, _)| n.ends_with("settings_stub_calls"))
        .map(|(_, v)| v.list().iter().filter_map(|c| if let Value::Str(s) = c { Some(s.to_string()) } else { None }).collect())
        .unwrap_or_default()
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
    let listed = crate::palette_commands(&crate::menu_bar::Bar::of(&all, &[], &[], None).menus, &all);
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
    let items = crate::palette_commands(&crate::menu_bar::Bar::of(&all, &[], &[], None).menus, &all);
    let items = items.list().to_vec();
    assert!(items.len() >= 25, "the menus hold {} items", items.len());
    let field = "#palette-search";
    // (On a session whose agent isn't working, where Archive session is one
    // of the palette's own.)
    app.click("#shell-sidebar >> role:button \"Tidy the tests\"");
    app.frames(30);
    // The palette's own rows for these (they run themselves, and are found
    // by their own words); the rest are listed from the menus, each run by
    // its menu item's id.
    let own = ["shell.new_chat", "shell.new_project", "shell.archive", "shell.toggle_sidebar", "shell.settings", "shell.reload", "shell.reload_interface"];
    for item in &items {
        let (id, title) = (text(item, "id"), text(item, "title"));
        // (The app's own: Zeron's shell and panel, the host's, plugins'. The
        // fixture's scaffolding commands, standing in for the engine, aren't
        // the app's menu items. Search is the palette itself.)
        let app_s = id.starts_with("shell.") || id.starts_with("panel.") || id.starts_with("zeron.") || id.contains('/');
        if !app_s || id == "shell.palette" {
            continue;
        }
        assert!(app.host.run_command("shell.palette"), "the palette didn't open");
        app.wait_until(|app| app.find(field).is_some());
        app.frames(20);
        let label = if id == crate::plugins::USE_ZERON_TEMPLATE { "Use Zeron v3".to_owned() } else { title.clone() };
        app.type_text(field, &label);
        app.frames(20);
        let row = format!("role:option \"{label}\"");
        assert!(app.find(&row).is_some(), "{id} (\"{title}\") isn't in the palette: {}", app.texts().join("|"));
        // (Run through the menu item's id: checked on rows no other of the
        // palette's shares a label with, Zeron's Pin and the host's way back.)
        if !own.contains(&id.as_str()) && (id == "shell.pin" || id == crate::plugins::USE_ZERON_TEMPLATE) {
            // (The fixture's host calls are its stub's, kept in a log:
            // ui/settings_stub.luau.)
            ran.borrow_mut().clear();
            app.click(&row);
            app.frames(20);
            let asked = format!("app.run_menu_command:{id}");
            let logged = stub_calls(&app);
            assert!(ran.borrow().contains(&id) || logged.contains(&asked), "{id}: the palette ran something else: {logged:?}");
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

/// The page's card stays concentric with the window's corner where the host
/// says what it is (Windows): its radius less the 4 the card is set in by, 4
/// on Windows 11's 8; square once the window is (maximized, full screen,
/// Windows 10). Told nothing (macOS), the theme's own.
#[test]
fn the_cards_corner_follows_the_windows() {
    let mut app = mount(windows(W));
    app.click("#shell-sidebar >> role:button \"Fix the build\"");
    app.frames(30);
    let corner = |app: &mut TestApp| {
        let node = app.find("#shell-page").expect("the page's card");
        match app.ui.scene.borrow().kind(node) {
            zgx::scene::NodeKind::Panel { quad, .. } | zgx::scene::NodeKind::Quad(quad) => quad.radius,
            _ => f32::NAN,
        }
    };
    let theirs = corner(&mut app);
    assert!(theirs >= 8., "the theme's own corner while told nothing: {theirs}");
    let told = app.host.shared("window.corner_radius", Value::Nil);
    for (window, card) in [(8., 4.), (0., 0.), (8., 4.)] {
        told.set(Value::from(window));
        app.frames(10);
        assert_eq!(corner(&mut app), card, "a window's {window} corner");
    }
}