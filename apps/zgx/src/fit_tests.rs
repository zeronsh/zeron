//! The window's fit in the real shell (ui/shell_fixture.luau, as the shell
//! lays out at each window width) against Zeron's own policy: which columns
//! show, the sidebar's width and the panel's, as a window narrows from wide
//! to narrow and back.

use zgx_script::{ScriptHost, testing::TestApp};

// Zeron's widths and policy, verbatim (crates/ui settings.rs,
// shell/panel_layout.rs `horizontal_panel_fit`), the
// reference this checks against.
const SIDEBAR_MIN: f32 = 242.0;
const RIGHT_PANE_MIN: f32 = 360.0;
// (Files' least: Zeron's is 220; here nothing (screens/shell/fit `TREE`): the
// tree stands beside the file inside the panel, whatever the panel's width.)
const FILES_PANEL_MIN: f32 = 0.0;
const CHAT_PANEL_MIN: f32 = 300.0;
// What the sidebar's column has past its limit: the window's edge before its card.
const SIDEBAR_EDGE: f32 = 5.0;

#[derive(Debug, Clone, Copy, PartialEq)]
struct HorizontalPanelFit {
    sidebar: bool,
    right: bool,
    files: bool,
    sidebar_limit: f32,
}

fn horizontal_panel_fit(
    viewport: f32,
    sidebar_width: f32,
    sidebar: (bool, u64),
    right: (bool, u64),
    files: (bool, u64),
    expanded: bool,
) -> HorizontalPanelFit {
    #[derive(PartialEq, Clone, Copy)]
    enum P {
        Sidebar,
        Right,
        Files,
    }
    let mut visible = [(P::Sidebar, sidebar.0, sidebar.1), (P::Right, right.0, right.1), (P::Files, files.0, files.1)];
    let viewport = viewport.max(0.0);
    let narrow_takeover = expanded && right.0 && viewport < CHAT_PANEL_MIN + RIGHT_PANE_MIN;
    if narrow_takeover {
        visible[0].1 = false;
        visible[2].1 = false;
    }
    loop {
        let (sidebar_open, right_open, files_open) = (visible[0].1, visible[1].1, visible[2].1);
        let chat_floor = if right_open && narrow_takeover { 0.0 } else { CHAT_PANEL_MIN };
        let minimum = chat_floor
            + if sidebar_open { SIDEBAR_MIN } else { 0.0 }
            + if right_open { RIGHT_PANE_MIN } else { 0.0 }
            + if files_open { FILES_PANEL_MIN } else { 0.0 };
        if minimum <= viewport {
            return HorizontalPanelFit {
                sidebar: sidebar_open,
                right: right_open,
                files: files_open,
                sidebar_limit: if sidebar_open {
                    (viewport
                        - chat_floor
                        - if right_open { RIGHT_PANE_MIN } else { 0.0 }
                        - if files_open { FILES_PANEL_MIN } else { 0.0 })
                    .min(sidebar_width)
                } else {
                    0.0
                },
            };
        }
        let victim = visible
            .iter()
            .enumerate()
            .filter(|(_, (_, open, _))| *open)
            .min_by_key(|(_, (panel, _, opened_at))| (expanded && *panel == P::Right, *opened_at))
            .map(|(index, _)| index);
        let Some(victim) = victim else {
            return HorizontalPanelFit { sidebar: false, right: false, files: false, sidebar_limit: 0.0 };
        };
        visible[victim].1 = false;
    }
}

fn mount() -> TestApp {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui/shell_fixture.luau");
    let app = TestApp::new(|runtime| ScriptHost::new(runtime, path));
    app.ui.scene.borrow_mut().resize(1800., 800.);
    app.frames(30);
    app
}

/// The sidebar (opened first) and a session's panel (opened after), the
/// window narrowed 10 points at a time from 1800 to 480 and widened back:
/// at each width the shell shows what Zeron's policy shows, the sidebar at
/// its limit, the panel at its share. Where each column goes and comes back
/// is printed (`--nocapture`).
#[test]
fn a_resize_walk_shows_the_columns_zerons_policy_shows() {
    let mut app = mount();
    app.click("#shell-sidebar >> role:button \"Fix the build\"");
    app.frames(20);
    app.click("#titlebar >> role:button \"Panel\"");
    app.frames(60);
    let sidebar_width = 256.0;
    let mut faults = Vec::new();
    let mut last: Option<(bool, bool, bool)> = None;
    let mut drops = Vec::new();
    let widths: Vec<f32> = (48..=180).rev().chain(48..=180).map(|w| w as f32 * 10.).collect();
    for (step, &w) in widths.iter().enumerate() {
        app.ui.scene.borrow_mut().resize(w, 800.);
        // (Settled: a column the fit hides or brings back eases there.)
        app.frames(30);
        let side = app.bounds("#shell-sidebar").width;
        let panel = app.bounds("#shell-files").width;
        // (The sidebar opened first, then the panel: the tree, Zeron's Files, is
        // open with it and takes no room of its own.)
        let want = horizontal_panel_fit(w, sidebar_width, (true, 1), (true, 3), (true, 0), false);
        let shows = (side > 0.5, panel > 0.5);
        if shows != (want.sidebar, want.right) {
            faults.push(format!("{w}: shows sidebar {} panel {}, Zeron's {} {}", shows.0, shows.1, want.sidebar, want.right));
        }
        let tree = want.right && want.files;
        if want.sidebar && (side - SIDEBAR_EDGE - want.sidebar_limit).abs() > 0.5 {
            faults.push(format!("{w}: the sidebar is {side}, Zeron's {} and the window's edge", want.sidebar_limit));
        }
        if want.right {
            // (Its width is the person's call, not the window's: a resize
            // moves it only as it must, the chat keeping its least; see
            // `a_window_resize_moves_everything_one_way`.)
            let side = if want.sidebar { want.sidebar_limit } else { 0.0 };
            if w - side - panel < CHAT_PANEL_MIN - 0.5 {
                faults.push(format!("{w}: the panel ({panel}) leaves the chat {} (its least is {CHAT_PANEL_MIN})", w - side - panel));
            }
        }
        let now = (shows.0, tree, shows.1);
        if let Some(was) = last
            && was != now
        {
            let way = if step < widths.len() / 2 { "narrowing" } else { "widening" };
            drops.push(format!("{way} to {w}: sidebar {} tree {} panel {} (Zeron's sidebar {} files {} right {})", now.0, now.1, now.2, want.sidebar, want.files, want.right));
        }
        last = Some(now);
    }
    // (Where each column went and came back, for a reader asking:
    // `ZGX_FIT_WALK=1 cargo test … -- --nocapture`.)
    if std::env::var_os("ZGX_FIT_WALK").is_some() {
        eprintln!("FIT {}", drops.join("\nFIT "));
    }
    assert!(faults.is_empty(), "{}", faults.join("\n"));
}

/// A column hidden for room is still open: asked for (its toggle), it comes
/// back and the least recent other hides for it; the window widened, both
/// show again. The session's panel stays open throughout.
#[test]
fn a_hidden_column_asked_for_comes_back_and_the_other_gives_way() {
    let mut app = mount();
    app.click("#shell-sidebar >> role:button \"Fix the build\"");
    app.frames(20);
    app.click("#titlebar >> role:button \"Panel\"");
    app.frames(60);
    let panel_open = |app: &TestApp| {
        app.host
            .states()
            .into_iter()
            .find(|(n, _)| n.ends_with("::shell_panel_open"))
            .is_some_and(|(_, v)| format!("{v:?}").contains("a1"))
    };
    let shows = |app: &mut TestApp| (app.bounds("#shell-sidebar").width > 0.5, app.bounds("#shell-files").width > 0.5);
    // 880: no room for both; the sidebar, opened first, gives way.
    app.ui.scene.borrow_mut().resize(880., 800.);
    app.frames(30);
    assert_eq!(shows(&mut app), (false, true), "at 880 the panel (newer) shows, the sidebar hides");
    assert!(panel_open(&app));
    // The sidebar asked for: it comes back, the panel hides for it, open.
    app.click("#titlebar >> role:button \"Toggle sidebar\"");
    app.frames(30);
    assert_eq!(shows(&mut app), (true, false), "the sidebar asked for comes back; the panel gives way");
    assert!(panel_open(&app), "hidden for room isn't closed");
    // The panel asked for: back, the sidebar gives way again.
    app.click("#titlebar >> role:button \"Panel\"");
    app.frames(30);
    assert_eq!(shows(&mut app), (false, true), "the panel asked for comes back");
    // Room again: both.
    app.ui.scene.borrow_mut().resize(1400., 800.);
    app.frames(30);
    assert_eq!(shows(&mut app), (true, true), "widened, both show again");
    assert!(panel_open(&app));
}

/// Where the panel's strip stands, settled: its own start from the panel's
/// (the hold: none at rest), and its first tab's from the panel's and how
/// much of it shows.
fn strip_rest(app: &mut TestApp) -> (f32, f32, f32, f32) {
    let panel = app.bounds("#shell-files").x;
    let strip = app.bounds("#panel-strip").x;
    let target = zgx_script::driver::parse("#panel-strip >> role:tab").unwrap();
    app.ui.prepare_frame();
    let found = zgx_script::driver::find_all(&app.ui, &app.handle, &target).unwrap_or_default();
    let scene = app.ui.scene.borrow();
    let id = *found.first().expect("a tab");
    let b = scene.transformed_bounds(id);
    let shows = scene.visible_bounds(id).map_or(0., |r| r.width);
    ((strip - panel).round(), (b.x - panel).round(), b.width.round(), shows.round())
}

fn drag(app: &mut TestApp, edge: &str, dx: f32) {
    use zgx::input::{InputEvent, Modifiers, PointerButton};
    let b = app.bounds(edge);
    let (x, y) = (b.x + b.width / 2., b.y + b.height / 2.);
    let (button, m) = (PointerButton::Primary, Modifiers::default());
    app.ui.dispatch_with_modifiers(InputEvent::PointerMove { x, y }, m);
    app.ui.dispatch_with_modifiers(InputEvent::PointerDown { x, y, button }, m);
    for step in 1..=4 {
        app.ui.dispatch_with_modifiers(InputEvent::PointerMove { x: x + dx * step as f32 / 4., y }, m);
        app.frames(1);
    }
    app.ui.dispatch_with_modifiers(InputEvent::PointerUp { x: x + dx, y, button }, m);
    app.ui.input.forget_clicks();
}

/// However the panel and the sidebar are moved (edges dragged, toggled and
/// toggled back mid-way, expanded and back, the window narrowed under the
/// panel's width and widened), the panel's strip settles where a fresh one
/// stands: at the panel's start, its first tab whole after its padding.
#[test]
fn the_panels_strip_settles_where_a_fresh_one_stands() {
    let mut app = mount();
    app.ui.scene.borrow_mut().resize(1600., 800.);
    app.frames(30);
    app.click("#shell-sidebar >> role:button \"Fix the build\"");
    app.frames(20);
    app.click("#titlebar >> role:button \"Panel\"");
    app.frames(40);
    app.click("#panel-open-files");
    app.frames(60);
    let fresh = strip_rest(&mut app);
    // (Inside the panel card's 1-point edge.)
    assert_eq!(fresh.0, 1., "a fresh strip stands at the panel's start");
    assert_eq!(fresh.2, fresh.3, "a fresh strip's first tab shows whole");
    let mut faults = Vec::new();
    let mut check = |app: &mut TestApp, what: &str| {
        app.frames(60);
        let now = strip_rest(app);
        if now != fresh {
            faults.push(format!("after {what}: strip at {}, tab at {} ({} of {} showing); fresh: {fresh:?}", now.0, now.1, now.3, now.2));
        }
    };
    drag(&mut app, "\"Resize files\"", -120.);
    check(&mut app, "the panel's edge dragged wider");
    drag(&mut app, "\"Resize files\"", 160.);
    check(&mut app, "the panel's edge dragged narrower");
    drag(&mut app, "\"Resize sidebar\"", 80.);
    check(&mut app, "the sidebar's edge dragged");
    assert!(app.host.run_command("shell.toggle_files"));
    app.frames(4);
    assert!(app.host.run_command("shell.toggle_files"));
    check(&mut app, "the panel closed and opened again mid-way");
    assert!(app.host.run_command("shell.toggle_sidebar"));
    app.frames(4);
    assert!(app.host.run_command("shell.toggle_sidebar"));
    check(&mut app, "the sidebar folded and back mid-way");
    app.click("#panel-expand");
    app.frames(4);
    app.click("#panel-expand");
    check(&mut app, "expanded and back mid-way");
    app.click("#panel-expand");
    app.frames(60);
    app.click("#panel-expand");
    check(&mut app, "expanded and back");
    // Narrowed until the panel's width is capped, then widened.
    for w in [1300., 1180., 1600.] {
        app.ui.scene.borrow_mut().resize(w, 800.);
        check(&mut app, &format!("the window resized to {w}"));
    }
    drag(&mut app, "\"Resize files\"", -400.);
    app.ui.scene.borrow_mut().resize(1250., 800.);
    check(&mut app, "the panel dragged wide, then the window narrowed under it");
    assert!(faults.is_empty(), "{}", faults.join("\n"));
}

fn x_of(app: &mut TestApp, selector: &str) -> Option<(f32, f32)> {
    let target = zgx_script::driver::parse(selector).ok()?;
    app.ui.prepare_frame();
    let found = zgx_script::driver::find_all(&app.ui, &app.handle, &target).unwrap_or_default();
    let scene = app.ui.scene.borrow();
    let id = found.into_iter().find(|&id| scene.layout_visible(id))?;
    let b = scene.transformed_bounds(id);
    Some((b.x, b.width))
}

/// The most a column's ease moves anything in one 60 Hz frame, past the window's
/// own step: the sidebar's on Zeron's column motion (CSS ease,
/// cubic-bezier(.25,.1,.25,1), its steepest about 2.25 times its mean), over
/// its 261 in its ≈191 ms.
const EASE_STEP: f32 = 57.0;

/// A window resized continuously moves everything in it one way, with the
/// window: the panel's edge, its first tab, the file's header and tree and
/// text, the chat, the sidebar's width; and what stands at the panel's
/// right (the file's "…") keeps its distance from the window's right edge.
/// Slow and fast, narrowing and widening, across where Zeron's fit hides or
/// shows a column (those frames alone step, as the column goes or comes).
#[test]
fn a_window_resize_moves_everything_one_way() {
    let mut app = mount();
    app.ui.scene.borrow_mut().resize(1600., 800.);
    app.frames(30);
    app.click("#shell-sidebar >> role:button \"Fix the build\"");
    app.frames(20);
    app.click("#titlebar >> role:button \"Panel\"");
    app.frames(40);
    app.click("#panel-open-files");
    assert!(app.wait_until(|app| app.find("#shell-files >> role:treeitem \"README.md\"").is_some()));
    app.click("#shell-files >> role:treeitem \"README.md\"");
    app.frames(60);
    let parts = [
        "#shell-files",
        "#panel-strip >> role:tab",
        "#shell-files >> role:button \"More\"",
        "#file-viewer-head",
        "#shell-files >> role:textbox \"README.md\"",
        "#transcript",
    ];
    let fit = |w: f32| {
        let f = horizontal_panel_fit(w, 256., (true, 1), (true, 3), (true, 0), false);
        (f.sidebar, f.right, f.files)
    };
    let mut faults = Vec::new();
    for step in [4., 16.] {
        let down: Vec<f32> = (0..).map(|i| 1600. - step * i as f32).take_while(|w| *w >= 700.).collect();
        let up: Vec<f32> = down.iter().rev().copied().collect();
        for sweep in [down, up] {
            // (Each from rest: a sweep starting on the last's ease, the panel's
            // long tail, would see it as the window's doing.)
            app.frames(30);
            let mut last: Option<(f32, Vec<Option<f32>>, f32, Option<f32>)> = None;
            let mut changed_at: Option<usize> = None;
            for (frame, &w) in sweep.iter().enumerate() {
                app.ui.scene.borrow_mut().resize(w, 800.);
                app.frame();
                let xs: Vec<Option<f32>> = parts.iter().map(|p| x_of(&mut app, p).map(|(x, _)| x)).collect();
                let side = app.bounds("#shell-sidebar").width;
                let more = x_of(&mut app, parts[2]).map(|(x, wd)| w - x - wd);
                if let Some((lw, lxs, lside, lmore)) = &last {
                    let dw = w - lw;
                    // A column going or coming (the fit changed within its
                    // ease): what's beside it glides with it, at most a
                    // frame of that ease past the window's own step. Else
                    // everything moves with the window, one way.
                    if fit(*lw) != fit(w) {
                        changed_at = Some(frame);
                    }
                    let easing = changed_at.is_some_and(|at| frame - at < 20);
                    let most = dw.abs() + if easing { EASE_STEP } else { 0.25 };
                    for (i, (a, b)) in lxs.iter().zip(&xs).enumerate() {
                        if let (Some(a), Some(b)) = (a, b) {
                            // (No more than the window's own step, the file. A
                            // sidebar coming back with the chat at its least
                            // moves it, the panel giving way, within the ease
                            // above.)
                            let file = i == 3 || i == 4;
                            if file && !easing && (b - a).abs() > dw.abs() + 1. {
                                faults.push(format!("step {step}, {lw} → {w}: the file ({}) moved {a} → {b}", parts[i]));
                            }
                            if (b - a).abs() > most + dw.abs() {
                                faults.push(format!("step {step}, {lw} → {w}: {} jumped {a} → {b}", parts[i]));
                            }
                            if !easing && (b - a) * dw < -0.25 {
                                faults.push(format!("step {step}, {lw} → {w}: {} went back, {a} → {b}", parts[i]));
                            }
                        }
                    }
                    if (side - lside).abs() > most || (!easing && (side - lside) * dw < -0.25) {
                        faults.push(format!("step {step}, {lw} → {w}: the sidebar {lside} → {side}"));
                    }
                    if let (Some(a), Some(b)) = (lmore, more)
                        && (b - a).abs() > 0.25
                    {
                        faults.push(format!("step {step}, {lw} → {w}: the file's … moved from the window's right, {a} → {b}"));
                    }
                }
                last = Some((w, xs, side, more));
            }
        }
    }
    assert!(faults.is_empty(), "{}", faults.join("\n"));
}

/// A panel dragged narrow in a wide window keeps its tree beside the file: there's
/// no floating it. While the edge is dragged the panel is as wide as the pointer
/// says (no jump), and let go it stays there.
#[test]
fn a_narrow_panel_keeps_its_tree_beside_the_file() {
    use zgx::input::{InputEvent, Modifiers, PointerButton};
    let mut app = mount();
    app.ui.scene.borrow_mut().resize(1600., 800.);
    app.frames(30);
    app.click("#shell-sidebar >> role:button \"Fix the build\"");
    app.frames(20);
    app.click("#titlebar >> role:button \"Panel\"");
    app.frames(40);
    app.click("#panel-open-files");
    assert!(app.wait_until(|app| app.find("#shell-files >> role:treeitem \"README.md\"").is_some()));
    app.click("#shell-files >> role:treeitem \"README.md\"");
    app.frames(60);
    // Beside the tree: the file starts past it.
    let docked = |app: &mut TestApp| {
        let panel = x_of(app, "#shell-files").unwrap().0;
        let file = x_of(app, "#file-viewer-head").unwrap().0;
        file - panel > 100.
    };
    assert!(docked(&mut app), "a wide panel has its tree beside the file");
    let (button, m) = (PointerButton::Primary, Modifiers::default());
    // The edge dragged to make the panel `w` wide and let go: where the tree
    // stands then, and mid-drag the panel follows the pointer.
    let drag_to = |app: &mut TestApp, w: f32| {
        let edge = app.bounds("\"Resize files\"");
        let (x, y) = (edge.x + edge.width / 2., edge.y + edge.height / 2.);
        let open = app.bounds("#shell-files").width;
        app.ui.input.forget_clicks();
        app.ui.dispatch_with_modifiers(InputEvent::PointerMove { x, y }, m);
        app.ui.dispatch_with_modifiers(InputEvent::PointerDown { x, y, button }, m);
        let to = x + open - w;
        app.ui.dispatch_with_modifiers(InputEvent::PointerMove { x: to, y }, m);
        app.frames(30);
        let now = app.bounds("#shell-files").width;
        assert!((now - w).abs() < 0.5, "dragged to {w}, the panel is {now}");
        assert!(docked(app), "dragged to {w}: the tree stays beside the file");
        app.ui.dispatch_with_modifiers(InputEvent::PointerUp { x: to, y, button }, m);
        app.ui.input.forget_clicks();
        app.frames(30);
        let now = app.bounds("#shell-files").width;
        assert!((now - w).abs() < 0.5, "let go at {w}, the panel is {now}");
        docked(app)
    };
    for w in [668., 638., 658., 678., 420.] {
        assert!(drag_to(&mut app, w), "at {w}: the tree stays beside the file");
    }
    assert!((app.bounds("#shell-files").width - 420.).abs() < 0.5, "it stays where it was let go");
}

/// The sidebar opening beside an open panel takes only what the chat needs:
/// with room for the chat's least, the panel keeps its width; short of it,
/// the panel gives just that much; the sidebar closing again gives it back.
/// (It was halved to a fresh share, squeezing a panel the chat had room
/// beside.)
#[test]
fn the_sidebar_opening_takes_only_what_the_chat_needs_from_the_panel() {
    // (The panel opens at half the chat's and its room: at 1500, 750, with 944 of
    // room beside the sidebar; at 1000, 500, with 444.)
    for (width, keeps) in [(1500., true), (1000., false)] {
        let mut app = mount();
        app.ui.scene.borrow_mut().resize(width, 800.);
        app.frames(30);
        app.click("#shell-sidebar >> role:button \"Fix the build\"");
        app.frames(20);
        assert!(app.host.run_command("shell.toggle_sidebar"));
        app.frames(60);
        app.click("#titlebar >> role:button \"Panel\"");
        app.frames(60);
        let before = app.bounds("#shell-files").width;
        assert!(app.host.run_command("shell.toggle_sidebar"));
        app.frames(60);
        let side = app.bounds("#shell-sidebar").width - SIDEBAR_EDGE;
        let after = app.bounds("#shell-files").width;
        let chat = width - side - after;
        assert!(chat >= CHAT_PANEL_MIN - 0.5, "{width}: the chat has {chat}");
        if keeps {
            assert!((after - before).abs() < 0.5, "{width}: the panel went {before} → {after} with room for the chat");
        } else {
            assert!(after < before - 0.5, "{width}: the panel should give way ({before} → {after})");
            assert!((chat - CHAT_PANEL_MIN).abs() < 1., "{width}: it gave more than the chat needed (chat {chat})");
        }
        assert!(app.host.run_command("shell.toggle_sidebar"));
        app.frames(60);
        let back = app.bounds("#shell-files").width;
        assert!((back - before).abs() < 0.5, "{width}: the sidebar gone, the panel is {back}, was {before}");
    }
}

/// The panel opening in a window with room for the sidebar, the chat and
/// the panel leaves the sidebar open. And the panel opens at its share beside the sidebar,
/// not at one worked out as if the sidebar were gone.
#[test]
fn the_panel_opening_never_closes_the_sidebar_for_its_tree() {
    let mut app = mount();
    app.ui.scene.borrow_mut().resize(1150., 800.);
    app.frames(30);
    app.click("#shell-sidebar >> role:button \"Fix the build\"");
    app.frames(20);
    let side = app.bounds("#shell-sidebar").width;
    assert!(side > 200., "the sidebar is open: {side}");
    app.click("#titlebar >> role:button \"Panel\"");
    app.frames(60);
    let side_after = app.bounds("#shell-sidebar").width;
    let panel = app.bounds("#shell-files").width;
    assert!((side_after - side).abs() < 0.5, "the sidebar went {side} → {side_after}");
    assert!(1150. - side_after - panel >= CHAT_PANEL_MIN - 0.5, "the chat has {}", 1150. - side_after - panel);
}

/// A panel width you chose (its edge dragged) is the width it opens at
/// again, as Zeron's `right_pane_width` is: closed and opened, it comes back
/// there, not at the split a fresh panel takes.
#[test]
fn the_panel_reopens_at_the_width_its_edge_was_dragged_to() {
    let mut app = mount();
    app.ui.scene.borrow_mut().resize(1500., 800.);
    app.frames(30);
    app.click("#shell-sidebar >> role:button \"Fix the build\"");
    app.frames(20);
    app.click("#titlebar >> role:button \"Panel\"");
    app.frames(60);
    let split = app.bounds("#shell-files").width;
    drag(&mut app, "\"Resize files\"", 140.);
    app.frames(60);
    let dragged = app.bounds("#shell-files").width;
    assert!((dragged - split).abs() > 60., "the drag moved it: {split} → {dragged}");
    for _ in 0..2 {
        app.click("#titlebar >> role:button \"Panel\"");
        app.frames(60);
        app.click("#titlebar >> role:button \"Panel\"");
        app.frames(60);
        let reopened = app.bounds("#shell-files").width;
        assert!((reopened - dragged).abs() < 0.5, "reopened at {reopened}, was dragged to {dragged} (a fresh split is {split})");
    }
}
