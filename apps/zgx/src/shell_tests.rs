//! The main window's title bar moves the window (ui/screens/shell): mounted
//! headless on its fixture (ui/shell_fixture.luau), a press anywhere along
//! the title bar's strip asks the host to move the window, as the desktop
//! host takes it (`Ui::take_window_gesture`), and the bar's controls keep
//! their presses. Pressed every few points across the window's width, so
//! anything laid over the strip (an overlay that takes the pointer, a row
//! of controls with gaps) shows as a dead zone at its place.

use zgx::{
    scene::Rect,
    input::{InputEvent, Modifiers, PointerButton},
    widgets::WindowGesture,
};
use zgx_script::{ScriptHost, testing::TestApp};

const W: f32 = 1200.;
const H: f32 = 800.;
/// The middle of the title bar's strip (Zeron's: 38 tall, its controls
/// centred 21 down).
const STRIP_Y: f32 = 21.;

fn mount() -> TestApp {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui/shell_fixture.luau");
    let app = TestApp::new(|runtime| ScriptHost::new(runtime, path));
    app.ui.scene.borrow_mut().resize(W, H);
    app.frames(30);
    app
}

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

/// Where the bar's controls showing now are (by their targets).
fn controls(app: &mut TestApp, targets: &[&str]) -> Vec<Rect> {
    targets
        .iter()
        .filter_map(|target| {
            app.find(target)?;
            Some(app.bounds(target))
        })
        .filter(|b| b.y < STRIP_Y && b.y + b.height > STRIP_Y)
        .collect()
}

/// Every point across the strip, but on a control (and a point either side
/// of one), moves the window: the dead zones, if any, by where they start.
fn dead_zones(app: &mut TestApp, targets: &[&str]) -> Vec<f32> {
    let on = controls(app, targets);
    let mut dead = Vec::new();
    let mut x = 2.;
    while x < W - 2. {
        let near = on.iter().any(|b| x >= b.x - 2. && x <= b.x + b.width + 2.);
        if !near && press(app, x, STRIP_Y) != Some(WindowGesture::Drag) {
            dead.push(x);
        }
        x += 10.;
    }
    dead
}

/// The bar's controls (the panel's in it too), and the edges that resize the sidebar and the files
/// beside a session (they run through the strip, and keep their presses:
/// a press there resizes).
const BAR: &[&str] = &[
    "role:button \"Toggle sidebar\"",
    "role:button \"Back\"",
    "role:button \"Forward\"",
    "role:button \"Sidebar view options\"",
    "role:button \"Panel\"",
    // The chats' tabs along the bar, and the panel's Expand beside its toggle.
    "#session-tabs >> role:tab",
    "role:button \"Expand panel\"",
    "role:button \"Show session details\"",
    "role:button \"Hide session details\"",
    "role:button \"Collapse panel\"",
    "\"Resize sidebar\"",
    "\"Resize files\"",
];

#[test]
fn the_title_bar_moves_the_window_across_its_width() {
    let mut app = mount();
    // (Told at the end: what follows has its own say.)
    let mut dead: Vec<(&str, Vec<f32>)> = Vec::new();
    // A new chat (the composer risen over the page, the wallpaper's room).
    dead.push(("a new chat's strip", dead_zones(&mut app, BAR)));

    // A session with its files open beside it.
    app.click("#shell-sidebar >> role:button \"Fix the build\"");
    app.frames(10);
    app.click("#titlebar >> role:button \"Panel\"");
    app.frames(40);
    assert!(app.find("#panel-strip").is_some(), "the panel's tabs show in its card");
    dead.push(("a session's strip, its files open", dead_zones(&mut app, BAR)));

    // The bar's controls keep their presses: no gesture, and they act.
    let files = app.bounds("#titlebar >> role:button \"Panel\"");
    let gesture = press(&mut app, files.x + files.width / 2., files.y + files.height / 2.);
    assert_eq!(gesture, None, "the files toggle keeps its press");
    app.frames(40);
    assert_eq!(app.bounds("#shell-files").width, 0., "the files toggle closed them");

    // Collapsed: the sidebar folds away entirely (no rail), the page the
    // window's width but its edges, the strip edge to edge.
    app.click("#titlebar >> role:button \"Toggle sidebar\"");
    app.frames(40);
    dead.push(("the collapsed window's strip", dead_zones(&mut app, BAR)));
    collapsed_alone(&mut app, "collapsed");
    // The session's tab on the strip: clear of the bar's controls, on their
    // line.
    let tab = app.bounds("#session-tabs >> role:tab \"Fix the build\"");
    let forward = app.bounds("#titlebar >> role:button \"Forward\"");
    assert!(tab.x >= forward.x + forward.width, "the tab {tab:?} runs under the controls (Forward {forward:?})");
    let mid = |b: Rect| b.y + b.height / 2.;
    assert!((mid(tab) - mid(forward)).abs() <= 1., "the tab {tab:?} isn't on the controls' line ({forward:?})");
    // Nor after the scripts reload, or the route moves (a new chat).
    app.host.reload().expect("the reload failed");
    app.frames(40);
    collapsed_alone(&mut app, "reloaded");
    // (A new chat, as ⌘N asks: the shortcut is the menu bar's, which a
    // headless window hasn't.)
    assert!(app.host.run_command("shell.new_chat"));
    app.frames(40);
    collapsed_alone(&mut app, "a new chat");
    app.click("#titlebar >> role:button \"Back\"");
    app.frames(40);
    app.click("#titlebar >> role:button \"Toggle sidebar\"");
    app.frames(40);

    // Settings, under the top bar: the bar's strip is still what moves the window.
    app.click("#shell-sidebar >> role:button \"Local\"");
    app.frames(30);
    assert!(app.find("#settings").is_some(), "settings opened");
    dead.push(("settings' strip", dead_zones(&mut app, BAR)));
    let dead: Vec<_> = dead.into_iter().filter(|(_, zones)| !zones.is_empty()).collect();
    assert!(dead.is_empty(), "points on the strip that don't move the window: {dead:?}");
}

/// The window's edge round the page's card.
const EDGE: f32 = 5.;

/// Collapsed, the sidebar is gone and nothing stands in for it: no rail
/// (screens/shell/view `rail`, off), the sidebar no wider than nothing, the
/// page the window's width but the window's edge each side, a card, a new
/// chat's too.
fn collapsed_alone(app: &mut TestApp, when: &str) {
    assert!(app.find("#shell-rail").is_none(), "{when}: a rail shows");
    assert_eq!(app.bounds("#shell-sidebar").width, 0., "{when}: the sidebar still shows");
    let want = W - 2. * EDGE;
    // (The gap round the card eases with the sidebar's motion: settled.)
    app.wait_until(|app| app.bounds("#shell-page").width == want);
    assert_eq!(app.bounds("#shell-page").width, want, "{when}: the page isn't the window's width");
}

#[test]
fn a_double_click_on_the_strip_asks_for_the_title_bars_double_click() {
    let mut app = mount();
    let (x, y, button) = (700., STRIP_Y, PointerButton::Primary);
    let mut last = None;
    for _ in 0..2 {
        app.ui.dispatch_with_modifiers(InputEvent::PointerDown { x, y, button }, Modifiers::default());
        last = app.ui.take_window_gesture();
        app.ui.dispatch_with_modifiers(InputEvent::PointerUp { x, y, button }, Modifiers::default());
    }
    app.ui.input.forget_clicks();
    assert_eq!(last, Some(WindowGesture::DoubleClick));
}

/// How far a drag's preview stands from where it was grabbed: the row
/// `row` pressed 40 points in from its left at its middle, the pointer
/// moved `(dx, dy)` in steps past the drag threshold, against the row's box
/// moved by the same. Nothing, when the preview stays under the pointer
/// where the row was grabbed. (The drag is let go with Escape after.)
fn preview_off(app: &mut TestApp, row: &str, dx: f32, dy: f32) -> (f32, f32) {
    let node = app.target(row).expect("the row");
    let b = app.ui.scene.borrow().hit_bounds(node);
    let (x, y) = (b.x + 40., b.y + b.height / 2.);
    let button = PointerButton::Primary;
    let m = Modifiers::default();
    app.ui.dispatch_with_modifiers(InputEvent::PointerMove { x, y }, m);
    app.ui.dispatch_with_modifiers(InputEvent::PointerDown { x, y, button }, m);
    for step in 1..=4 {
        let part = step as f32 / 4.;
        app.ui.dispatch_with_modifiers(InputEvent::PointerMove { x: x + dx * part, y: y + dy * part }, m);
        app.frames(2);
    }
    let drag = app.ui.input.drag_state().expect("a drag under way");
    let preview = drag.preview.expect("its preview");
    let at = drag.preview_origin.unwrap_or_else(|| {
        let p = app.ui.scene.borrow().transformed_bounds(preview);
        (p.x, p.y)
    });
    app.key(zgx::input::Key::Escape);
    app.ui.dispatch_with_modifiers(InputEvent::PointerUp { x: x + dx, y: y + dy, button }, m);
    app.ui.input.forget_clicks();
    app.frames(30);
    (at.0 - (b.x + dx), at.1 - (b.y + dy))
}

fn assert_under_pointer(app: &mut TestApp, what: &str, title: &str) {
    let off = preview_off(app, &format!("#shell-sidebar >> role:button \"{title}\""), 30., 60.);
    assert!(
        off.0.abs() < 0.5 && off.1.abs() < 0.5,
        "{what}: the preview stood {off:?} from where \"{title}\" was grabbed"
    );
}

#[test]
fn a_dragged_session_stays_where_it_was_grabbed() {
    let mut app = mount();
    // Under a project, nothing pinned yet (Pinned's drop zone comes with the
    // drag: it once pushed the rows, and the grab, 70 points down).
    assert_under_pointer(&mut app, "a project's row", "Fix the build");
    assert_under_pointer(&mut app, "a later project's row", "Ship the release");
    // In a section of yours.
    assert!(app.host.run_command("section"));
    app.frames(30);
    assert_under_pointer(&mut app, "a section's row", "Answer the review");
    // Pinned.
    assert!(app.host.run_command("pin"));
    app.frames(30);
    assert_under_pointer(&mut app, "a pinned row", "Write the docs");
    assert_under_pointer(&mut app, "a project's row, with pins", "Fix the build");
    // Down a long list, scrolled.
    assert!(app.host.run_command("many"));
    app.frames(30);
    app.scroll("#shell-sidebar >> role:button \"Ship the release\"", 1500.);
    app.frames(30);
    let row = (1..=400)
        .map(|i| format!("Many {i}"))
        .find(|title| {
            let target = format!("#shell-sidebar >> role:button \"{title}\"");
            app.find(&target).is_some_and(|_| {
                let b = app.bounds(&target);
                b.y > 200. && b.y < 600.
            })
        })
        .expect("a row in view, scrolled");
    assert_under_pointer(&mut app, "a row down a scrolled list", &row);
}

#[test]
fn a_plugins_title_bar_controls_stand_on_glass_over_art() {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui/titlebar_fixture.luau");
    let mut app = TestApp::new(|runtime| ScriptHost::new(runtime, path));
    app.ui.scene.borrow_mut().resize(W, H);
    app.frames(60);
    let island = "#titlebar-actions-island";
    assert!(app.opacity(island) > 0.99, "over art, holding Focus: its island");
    // Its island is as wide as the row and the room either side of it.
    let row = app.bounds("#titlebar-actions");
    let glass = app.bounds(island);
    assert!(glass.x < row.x && glass.x + glass.width > row.x + row.width, "round the row");
    // A session's page (no art): no island.
    assert!(app.host.run_command("art"));
    app.frames(60);
    assert!(app.opacity(island) < 0.01, "no art, no island");
    // Over art with nothing in the row: no island (no empty glass).
    assert!(app.host.run_command("art"));
    assert!(app.host.run_command("empty"));
    app.frames(60);
    assert!(app.opacity(island) < 0.01, "an empty row, no island");
}

#[test]
fn the_panels_edge_follows_the_pointer_at_once_and_opening_eases() {
    let mut app = mount();
    // (Wide enough that the panel opens short of its widest: the window's
    // fit caps it, as Zeron's does, and an edge dragged past that holds.)
    app.ui.scene.borrow_mut().resize(1600., 800.);
    app.frames(30);
    app.click("#shell-sidebar >> role:button \"Fix the build\"");
    app.frames(5);
    app.click("#titlebar >> role:button \"Panel\"");
    // Opening eases: a couple of frames on, it's on its way, not there.
    app.frames(2);
    let opening = app.bounds("#shell-files").width;
    app.frames(60);
    let open = app.bounds("#shell-files").width;
    assert!(opening > 0. && opening < open, "opening eases ({opening} of {open})");
    // The edge pressed and moved 50 points left, step by step: each frame
    // the panel is as wide as the pointer says, no ease trailing it.
    let edge = app.bounds("\"Resize files\"");
    let (x, y) = (edge.x + edge.width / 2., edge.y + edge.height / 2.);
    let (button, m) = (PointerButton::Primary, Modifiers::default());
    app.ui.dispatch_with_modifiers(InputEvent::PointerMove { x, y }, m);
    app.ui.dispatch_with_modifiers(InputEvent::PointerDown { x, y, button }, m);
    for step in 1..=5 {
        let to = x - 10. * step as f32;
        app.ui.dispatch_with_modifiers(InputEvent::PointerMove { x: to, y }, m);
        app.frames(1);
        let w = app.bounds("#shell-files").width;
        assert!((w - (open + 10. * step as f32)).abs() < 0.5, "step {step}: {w}, wanted {}", open + 10. * step as f32);
    }
    app.ui.dispatch_with_modifiers(InputEvent::PointerUp { x: x - 50., y, button }, m);
    app.ui.input.forget_clicks();
    app.frames(30);
    assert!((app.bounds("#shell-files").width - (open + 50.)).abs() < 0.5, "it stays where it was let go");
}

/// What plugins place by stands where it should: the window's root, the
/// sidebar's list and its actions, the title bar's, settings' list and page
/// (by id; classes and what's provided aren't in the tree).
#[test]
fn the_places_plugins_target_are_there() {
    let mut app = mount();
    for target in ["#shell", "#shell-sidebar", "#sidebar", "#sidebar-actions", "#titlebar", "#titlebar-actions"] {
        assert!(app.find(target).is_some(), "no {target}:\n{}", app.tree());
    }
    assert!(app.find("#sidebar-actions >> role:button \"New chat\"").is_some(), "New chat isn't in #sidebar-actions");
    assert!(app.find("#shell-sidebar >> #sidebar").is_some(), "#sidebar isn't the sidebar's");
    // (The rail is off: none.)
    assert!(app.find("#rail").is_none(), "a rail with the rail off");
    app.click("#shell-sidebar >> role:button \"Local\"");
    app.frames(30);
    for target in ["#settings-nav", "#settings-page"] {
        assert!(app.find(target).is_some(), "no {target}:\n{}", app.tree());
    }
}

/// A session's card (Zeron's hover card): the pointer resting on its row,
/// it shows after a beat, beside the row, 284 wide, with how long ago it
/// moved, where it runs, its project and its branch: one surface over the
/// window (no tooltip's frame around it); a press puts it away.
#[test]
fn a_rested_on_session_shows_its_card() {
    let mut app = mount();
    let row = "#sidebar >> role:button \"Write the docs\"";
    let shown = |app: &mut TestApp, text: &str| x_of(app, &format!("\"{text}\"")).is_some();
    let r = app.bounds(row);
    for dx in [20., 24., 28.] {
        zgx_script::driver::hover_at(&mut app.ui, (r.x + dx, r.y + r.height / 2.));
        app.frames(1);
    }
    assert!(!shown(&mut app, "1h ago"), "not at once");
    assert!(app.wait_until(|app| shown(app, "1h ago")), "the card shows after a rest");
    app.frames(20);
    for text in ["MacBook Pro", "Atlas", "docs"] {
        assert!(shown(&mut app, text), "the card says {text:?}");
    }
    // Beside the row, 10 to its right, 284 wide: one surface, no tooltip.
    let (x, _) = x_of(&mut app, "\"1h ago\"").unwrap();
    assert!(x > r.x + r.width + 10., "the card's text at {x} stands right of the row {r:?}");
    assert!(app.find("role:dialog \"Tooltip\"").is_none(), "no tooltip around it");
    // A press: gone.
    app.click(row);
    app.frames(20);
    assert!(!shown(&mut app, "1h ago"), "a press puts it away");
}

/// A session renamed in its row (Zeron's inline rename): a double click on
/// it puts a field over its title, the title selected; ↩ keeps the name
/// typed, Esc the old one; its menu's Rename does the same, in the row, not
/// a dialog.
#[test]
fn a_session_is_renamed_in_its_row() {
    let mut app = mount();
    let calls = |app: &TestApp| {
        app.host
            .states()
            .into_iter()
            .find(|(n, _)| n == "calls" || n.ends_with("::calls"))
            .map_or(String::new(), |(_, v)| match v {
                zgx_script::Value::Str(s) => s.to_string(),
                _ => String::new(),
            })
    };
    let field = "#sidebar >> role:textbox \"Session name\"";
    app.double_click("#sidebar >> role:button \"Write the docs\"");
    // (It opens once the click is over: the next turn.)
    app.wait_until(|app| app.find(field).is_some());
    assert!(app.find(field).is_some(), "a double click opens the field");
    assert!(app.ui.input.focused() == app.target(field).ok(), "focused, to type");
    // (Its title selected: what's typed replaces it.)
    app.ui.dispatch(zgx::input::InputEvent::Text("Write the guide".into()));
    app.key(zgx::input::Key::Enter);
    app.frames(10);
    assert!(app.find(field).is_none(), "↩ closes it");
    assert!(calls(&app).contains("rename:a2:Write the guide"), "↩ keeps it: {}", calls(&app));
    assert!(app.find("#sidebar >> role:button \"Write the guide\"").is_some(), "the row says so");
    // Esc: the old name.
    app.double_click("#sidebar >> role:button \"Write the guide\"");
    assert!(app.wait_until(|app| app.find(field).is_some()), "open again");
    app.ui.dispatch(zgx::input::InputEvent::Text("Nope".into()));
    app.key(zgx::input::Key::Escape);
    app.frames(10);
    assert!(app.find(field).is_none(), "Esc closes it");
    assert!(!calls(&app).contains("Nope"), "Esc keeps the old name");
    // Its menu's Rename: the same field, in the row.
    app.click("#sidebar >> role:button \"Tidy the tests\"");
    app.frames(10);
    app.keys("shift+F10");
    assert!(app.wait_until(|app| app.find("role:menuitem \"Rename\"").is_some()));
    app.frames(30);
    app.click("role:menuitem \"Rename\"");
    app.wait_until(|app| app.find(field).is_some());
    assert!(app.find(field).is_some(), "the menu's Rename opens the field in the row");
    assert!(app.find("role:dialog \"Rename session\"").is_none(), "not the dialog");
}

/// Where `selector` stands now, if it's laid out and shown: its x, its width.
fn x_of(app: &mut TestApp, selector: &str) -> Option<(f32, f32)> {
    let target = zgx_script::driver::parse(selector).ok()?;
    app.ui.prepare_frame();
    let found = zgx_script::driver::find_all(&app.ui, &app.handle, &target).unwrap_or_default();
    let scene = app.ui.scene.borrow();
    let id = found.into_iter().find(|&id| scene.layout_visible(id))?;
    let b = scene.transformed_bounds(id);
    Some((b.x, b.width))
}

/// A session's menu's Copy, as Zeron's: its folder, its harness's link and
/// id where it has them (a Codex thread's link, `codex://threads/…`), its
/// own id; each copied, and said so.
#[test]
fn a_sessions_copy_menu_copies_and_says_so() {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui/shell_fixture.luau");
    let mut app = TestApp::with_desktop(|runtime| ScriptHost::new(runtime, path.clone()));
    app.ui.scene.borrow_mut().resize(W, H);
    app.frames(30);
    app.click("#sidebar >> role:button \"Write the docs\"");
    app.frames(10);
    app.keys("shift+F10");
    assert!(app.wait_until(|app| app.find("role:menuitem \"Copy\"").is_some()));
    app.frames(20);
    app.click("role:menuitem \"Copy\"");
    assert!(app.wait_until(|app| app.find("role:menuitem \"Harness session ID\"").is_some()), "the submenu:\n{}", app.tree());
    assert!(app.find("role:menuitem \"Codex conversation link\"").is_some(), "a Codex thread's link");
    app.frames(20);
    app.click("role:menuitem \"Codex conversation link\"");
    // (A menu's item acts after its flash, on the clock.)
    app.wait_until(|app| !app.desktop.as_ref().unwrap().clipboard().is_empty());
    assert_eq!(app.desktop.as_ref().unwrap().clipboard(), "codex://threads/th%207%2Fa", "its id encoded, as production's");
    assert!(app.wait_until(|app| app.find("\"Codex conversation link copied\"").is_some()), "said so");
    // Another without a harness id (a window of its own): none offered.
    let mut app = TestApp::with_desktop(|runtime| ScriptHost::new(runtime, path.clone()));
    app.ui.scene.borrow_mut().resize(W, H);
    app.frames(30);
    app.click("#sidebar >> role:button \"Tidy the tests\"");
    app.frames(10);
    app.keys("shift+F10");
    assert!(app.wait_until(|app| app.find("role:menuitem \"Copy\"").is_some()));
    app.frames(20);
    app.click("role:menuitem \"Copy\"");
    assert!(app.wait_until(|app| app.find("role:menuitem \"Session ID\"").is_some()));
    assert!(app.find("role:menuitem \"Harness session ID\"").is_none(), "no harness id, none offered");
    // Its Zeron link (every session's), as the host makes it.
    app.frames(20);
    app.click("role:menuitem \"Zeron conversation link\"");
    assert!(
        app.wait_until(|app| app.desktop.as_ref().unwrap().clipboard().starts_with("zeron://open/chat/a3?workspace=")),
        "the Zeron link: {:?}",
        app.desktop.as_ref().unwrap().clipboard()
    );
    assert!(app.wait_until(|app| app.find("\"Zeron conversation link copied\"").is_some()), "said so");
}

/// Deleting a session is asked once more (Zeron's "Delete session?"): its
/// title in the question; Cancel keeps it, Delete deletes it.
#[test]
fn deleting_a_session_asks_first() {
    let mut app = mount();
    let calls = |app: &TestApp| {
        app.host.states().into_iter().find(|(n, _)| n == "calls" || n.ends_with("::calls")).map_or(String::new(), |(_, v)| match v {
            zgx_script::Value::Str(s) => s.to_string(),
            _ => String::new(),
        })
    };
    let ask = |app: &mut TestApp| {
        app.click("#sidebar >> role:button \"Tidy the tests\"");
        app.frames(10);
        app.keys("shift+F10");
        assert!(app.wait_until(|app| app.find("role:menuitem \"Delete…\"").is_some()));
        app.frames(20);
        app.click("role:menuitem \"Delete…\"");
        let open = |app: &mut TestApp| {
            app.host.states().into_iter().any(|(n, v)| n.ends_with("::shell_delete_open") && v == zgx_script::Value::Bool(true))
        };
        assert!(app.wait_until(|app| open(app)), "the question");
        // (Its panel rises as it opens: pressed where it settles.)
        app.frames(60);
        app.ui.prepare_frame();
    };
    ask(&mut app);
    assert!(app.find("\"“Tidy the tests” will be permanently deleted. This can’t be undone.\"").is_some(), "its title in it:\n{}", app.tree());
    app.click("role:dialog \"Delete session?\" >> role:button \"Cancel\"");
    app.frames(30);
    assert!(!calls(&app).contains("delete:a3"), "Cancel keeps it");
    // (Asked again in a window of its own: a menu opened right after another
    // closed lands late headless.)
    let mut app = mount();
    ask(&mut app);
    app.click("role:dialog \"Delete session?\" >> role:button \"Delete\"");
    assert!(app.wait_until(|app| calls(app).contains("delete:a3")), "Delete deletes it: {}", calls(&app));
}

/// The panel's edge takes the keys too (a separator): ← widens the panel a
/// step, → narrows it, ⇧ four steps; never past what leaves the chat its
/// least.
#[test]
fn the_panels_edge_resizes_from_the_keys() {
    let mut app = mount();
    app.click("#shell-sidebar >> role:button \"Fix the build\"");
    app.frames(20);
    app.click("#titlebar >> role:button \"Panel\"");
    app.frames(60);
    let edge = app.target("role:slider \"Resize files\"").expect("the edge, a slider of the panel's width");
    assert!(app.ui.input.focus(&app.ui.scene, Some(edge)), "it takes focus");
    let w = |app: &mut TestApp| app.bounds("#shell-files").width;
    let open = w(&mut app);
    app.keys("shift+ArrowRight");
    app.frames(40);
    assert!((w(&mut app) - (open - 64.)).abs() < 0.5, "⇧→ four steps narrower: {} from {open}", w(&mut app));
    app.key(zgx::input::Key::ArrowLeft);
    app.frames(40);
    assert!((w(&mut app) - (open - 64. + 16.)).abs() < 0.5, "← a step wider: {}", w(&mut app));
    for _ in 0..40 {
        app.key(zgx::input::Key::ArrowLeft);
    }
    app.frames(40);
    let side = app.bounds("#shell-sidebar").width;
    assert!(W - side - w(&mut app) >= 300. - 0.5, "the chat keeps its least");
}

/// A session carried from the sidebar over the chat says "Drop to mention"
/// (Zeron's overlay), and dropped there it's a chat chip on the composer.
#[test]
fn a_session_dropped_on_the_chat_is_mentioned() {
    use zgx::input::{InputEvent, PointerButton};
    let mut app = mount();
    app.click("#sidebar >> role:button \"Fix the build\"");
    app.frames(20);
    let row = app.bounds("#sidebar >> role:button \"Write the docs\"");
    let to = app.bounds("#chat-composer");
    let (x0, y0) = (row.x + 40., row.y + row.height / 2.);
    let (x1, y1) = (to.x + to.width / 2., to.y + to.height / 2.);
    let button = PointerButton::Primary;
    app.ui.dispatch(InputEvent::PointerMove { x: x0, y: y0 });
    app.ui.dispatch(InputEvent::PointerDown { x: x0, y: y0, button });
    for i in 1..=12 {
        let t = i as f32 / 12.;
        app.ui.dispatch(InputEvent::PointerMove { x: x0 + (x1 - x0) * t, y: y0 + (y1 - y0) * t });
        app.frames(1);
    }
    app.frames(5);
    assert!(app.find("#chat-drop-mention").is_some(), "carried over the chat: Drop to mention\n{}", app.tree());
    assert!(app.find("#chat-drop").is_none(), "not Drop to attach");
    app.ui.dispatch(InputEvent::PointerUp { x: x1, y: y1, button });
    app.ui.input.forget_clicks();
    app.frames(20);
    assert!(app.find("#chat-drop-mention").is_none(), "dropped: gone");
    assert!(app.find("#chat-composer >> \"Write the docs\"").is_some(), "a chip for it on the composer:\n{}", app.tree());
}

/// What the shell asked of the host, in order (the fixture's `calls`).
fn said(app: &TestApp) -> String {
    app.host.states().into_iter().find(|(n, _)| n == "calls" || n.ends_with("::calls")).map_or(String::new(), |(_, v)| match v {
        zgx_script::Value::Str(s) => s.to_string(),
        _ => String::new(),
    })
}

/// A project's header's right click, as Zeron's: `item` from its menu.
fn project_menu(app: &mut TestApp, project: &str, item: &str) {
    let head = app.bounds(&format!("#sidebar >> \"{project}\""));
    zgx_script::driver::right_click_at(&mut app.ui, (head.x + head.width / 2., head.y + head.height / 2.));
    let item = format!("role:menuitem \"{item}\"");
    assert!(app.wait_until(|app| app.find(&item).is_some()), "the project's menu:\n{}", app.tree());
    app.frames(20);
    app.click(&item);
}

fn dialog_open(app: &TestApp, name: &str) -> bool {
    app.host.states().into_iter().any(|(n, v)| n.ends_with(&format!("::{name}")) && v == zgx_script::Value::Bool(true))
}

/// Edit project… on a project's header asks its name (filled in) and its
/// icon, and renames it; its icon left as it was is left alone.
#[test]
fn a_projects_header_renames_it() {
    let mut app = mount();
    project_menu(&mut app, "Borealis", "Edit project…");
    assert!(app.wait_until(|app| dialog_open(app, "project_edit_open")), "the question");
    app.frames(60);
    assert_eq!(app.text("role:dialog \"Edit project\" >> role:textbox"), "Borealis", "its name to start from");
    app.fill("role:dialog \"Edit project\" >> role:textbox", "  Aurora  ");
    app.frames(10);
    app.click("role:dialog \"Edit project\" >> role:button \"Save\"");
    assert!(app.wait_until(|app| said(app).contains("rename_project:borealis:Aurora")), "renamed, trimmed: {}", said(&app));
    assert!(app.wait_until(|app| app.find("#sidebar >> \"Aurora\"").is_some()), "its header says so");
}

/// Remove… on a project's header asks once more, saying what goes with it
/// (Zeron's "Remove project?": its sessions on its device); Cancel keeps it,
/// Remove removes it.
#[test]
fn removing_a_project_asks_first() {
    let mut app = mount();
    project_menu(&mut app, "Borealis", "Remove…");
    assert!(app.wait_until(|app| dialog_open(app, "shell_project_remove_open")), "the question");
    app.frames(60);
    app.ui.prepare_frame();
    assert!(
        app.find("\"Removing “Borealis” permanently deletes its 2 sessions on devbox. This can’t be undone.\"").is_some(),
        "what goes with it:\n{}",
        app.tree()
    );
    app.click("role:dialog \"Remove project?\" >> role:button \"Cancel\"");
    app.frames(30);
    assert!(!said(&app).contains("delete_project"), "Cancel keeps it");
    // (Asked again in a window of its own: a menu opened right after another
    // closed lands late headless.)
    let mut app = mount();
    project_menu(&mut app, "Borealis", "Remove…");
    assert!(app.wait_until(|app| dialog_open(app, "shell_project_remove_open")));
    app.frames(60);
    app.click("role:dialog \"Remove project?\" >> role:button \"Remove\"");
    assert!(app.wait_until(|app| said(app).contains("delete_project:borealis")), "Remove removes it: {}", said(&app));
    // (Its rows close where they were as they go, the sidebar's motion: a
    // frame at a time, so its clock moves.)
    assert!(
        app.wait_until(|app| {
            app.frames(1);
            app.find("#sidebar >> role:button \"Ship the release\"").is_none()
        }),
        "its sessions with it"
    );
}
