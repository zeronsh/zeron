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
/// The sidebar's default width (screens/shell/fit_policy `SIDEBAR_DEFAULT`).
const SIDEBAR_DEFAULT: f32 = 272.;

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
    let sidebar_width = SIDEBAR_DEFAULT;
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
            // (Its width at its share, the chat keeping its least; see
            // `a_window_resize_keeps_the_chat_and_the_panel_at_half_each`.)
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

/// Zeron's share of `available` for the panel (shell/panel_layout.rs
/// `content_split`): half, each side keeping its least.
fn content_split(available: f32) -> f32 {
    (available / 2.).max(RIGHT_PANE_MIN).min((available - CHAT_PANEL_MIN).max(RIGHT_PANE_MIN))
}

/// The panel's share in a window `vw` wide beside a sidebar `side` wide
/// (its card's width; 0 folded): Zeron's split of what the sidebar and the
/// window's gaps round the cards leave the chat and the panel. (The window's
/// gap before the sidebar's card and the gap between cards after it, or the
/// window's edge alone; the gap before the panel and the window's edge
/// after it. Zeron's columns had none.)
fn share_of(vw: f32, side: f32) -> f32 {
    let gaps = if side > 0.5 { SIDEBAR_EDGE + 4. } else { 5. } + 4. + 5.;
    content_split(vw - side - gaps)
}

/// A window resized continuously keeps the chat and the panel at half each,
/// as Zeron's does (`observe_viewport_width`), at once: the change shared
/// between them, each keeping its least. Everything in it moves one way,
/// with the window: the panel's edge, its first tab, the file's header and
/// tree and text, the chat, the sidebar's width; and what stands at the
/// panel's right (the file's "…") keeps its distance from the window's
/// right edge. Slow and fast, narrowing and widening, across where Zeron's
/// fit hides or shows a column (those frames alone step, as the column goes
/// or comes, the panel easing to its new share). (This was the chat taking
/// the whole change, the panel moving only at the chat's least; Gaël asked
/// for Zeron's balance back.)
#[test]
fn a_window_resize_keeps_the_chat_and_the_panel_at_half_each() {
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
            let mut was_narrow: Option<bool> = None;
            for (frame, &w) in sweep.iter().enumerate() {
                app.ui.scene.borrow_mut().resize(w, 800.);
                app.frame();
                // (The Files pane crossing its narrow line, 288 + 240, as the
                // panel narrows or widens with the window: the tree goes or
                // comes beside the file then, at once, Wing's narrow rule;
                // the file moves by the tree's width that once.)
                let narrow = app.bounds("files-pane").width < 288. + 240.;
                let crossed = was_narrow.is_some_and(|n| n != narrow);
                was_narrow = Some(narrow);
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
                            // sidebar coming back moves it, within the ease above.)
                            let file = i == 3 || i == 4;
                            if file && crossed {
                                continue;
                            }
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
                    // (Not at the panel's least, where its docked tree leaves the
                    // file narrower than its head's controls, which run past it: as
                    // its edge dragged there does.)
                    let roomy = app.bounds("#shell-files").width >= RIGHT_PANE_MIN + 20.;
                    if let (Some(a), Some(b)) = (lmore, more)
                        && roomy
                        && (b - a).abs() > 0.25
                    {
                        faults.push(format!("step {step}, {lw} → {w}: the file's … moved from the window's right, {a} → {b}"));
                    }
                    // At rest between the fit's changes, the panel at its
                    // share of what the sidebar leaves.
                    let panel = app.bounds("#shell-files").width;
                    let card = if side > 0.5 { side - SIDEBAR_EDGE } else { 0. };
                    if !easing && fit(w).1 && (panel - share_of(w, card)).abs() > 1. {
                        faults.push(format!("step {step}, at {w}: the panel is {panel}, its share {}", share_of(w, card)));
                    }
                }
                last = Some((w, xs, side, more));
            }
        }
    }
    assert!(faults.is_empty(), "{}", faults.join("\n"));
}

/// A panel dragged narrower in a wide window keeps its tree beside the file while
/// there's room for a file beside it (288 + 240); under that the file has the
/// panel's whole width and the tree closes (ds/files_pane's narrow pane): there's
/// no floating it either way. While the edge is dragged the panel is as wide as
/// the pointer says (no jump), and let go it stays there.
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
        assert_eq!(docked(app), w >= 528., "dragged to {w}: the tree beside the file only with room for both");
        app.ui.dispatch_with_modifiers(InputEvent::PointerUp { x: to, y, button }, m);
        app.ui.input.forget_clicks();
        app.frames(30);
        let now = app.bounds("#shell-files").width;
        assert!((now - w).abs() < 0.5, "let go at {w}, the panel is {now}");
        docked(app)
    };
    for w in [668., 638., 658., 678.] {
        assert!(drag_to(&mut app, w), "at {w}: the tree stays beside the file");
    }
    // Narrow: the file has the whole panel, the tree closed, not floating over it.
    assert!(!drag_to(&mut app, 420.), "at 420: the file has the panel's whole width");
    let panel = x_of(&mut app, "#shell-files").unwrap().0;
    let file = x_of(&mut app, "#file-viewer-head").unwrap().0;
    assert!((file - panel).abs() <= 1.5, "at 420 the file starts at the panel's start (its card's hairline in) ({file} against {panel})");
    assert!((app.bounds("#shell-files").width - 420.).abs() < 0.5, "it stays where it was let go");
}

/// The sidebar folding or coming back splits the chat and the panel again,
/// 50/50, as Zeron's `toggle_sidebar` did (`resplit_panels`), a width dragged
/// on the panel's edge forgotten as there; and it eases there on the
/// sidebar's own clock, so the chat between them only ever widens (or
/// narrows) a frame at a time, never the other way first, and the panel
/// steps a little each frame. (Gaël's call over Wing's earlier rule, where
/// the sidebar took only what the chat needed and the panel held.)
#[test]
fn the_sidebar_toggled_splits_the_chat_and_the_panel_again_easing() {
    for (width, dragged) in [(1340., false), (1340., true), (1000., false)] {
        let mut app = mount();
        app.ui.scene.borrow_mut().resize(width, 800.);
        app.frames(30);
        app.click("#shell-sidebar >> role:button \"Fix the build\"");
        app.frames(20);
        app.click("#titlebar >> role:button \"Panel\"");
        app.frames(60);
        if dragged {
            drag(&mut app, "\"Resize files\"", -80.);
            app.frames(40);
        }
        for (folding, card) in [(true, 0.), (false, SIDEBAR_DEFAULT)] {
            let chat = |app: &mut TestApp| app.bounds("#shell-page-room").width;
            let mut chats = vec![chat(&mut app)];
            let mut panels = vec![app.bounds("#shell-files").width];
            assert!(app.host.run_command("shell.toggle_sidebar"));
            for _ in 0..24 {
                app.frame();
                chats.push(chat(&mut app));
                panels.push(app.bounds("#shell-files").width);
            }
            let what = format!("{width}{}, the sidebar {}", if dragged { " dragged" } else { "" }, if folding { "folding" } else { "back" });
            let way = if folding { 1. } else { -1. };
            assert!(chats.windows(2).all(|w| (w[1] - w[0]) * way >= -0.05), "{what}: the chat one way: {chats:?}");
            let jump = panels.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0., f32::max);
            let moved = (panels[panels.len() - 1] - panels[0]).abs();
            assert!(moved > 20. && jump < moved * 0.6, "{what}: the panel eases there: {panels:?}");
            let panel = *panels.last().unwrap();
            assert!((panel - share_of(width, card)).abs() < 0.5, "{what}: the panel at its share {}: {panel}", share_of(width, card));
            assert!((chats.last().unwrap() - panel).abs() < 0.5 || panel <= RIGHT_PANE_MIN + 0.5, "{what}: the chat {} beside the panel {panel}", chats.last().unwrap());
            app.frames(30);
        }
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

/// A panel width you chose (its edge dragged) holds only until the panel's
/// next toggle, as Zeron's `right_pane_width` does: the window resized, it
/// stays where it was dragged; closed and opened again, it comes back at its
/// share. And while it's opening, its edge is out of reach: a drag there does
/// nothing (Zeron's handle is gone while the panel moves).
#[test]
fn a_dragged_panel_width_holds_until_the_panel_is_toggled() {
    let mut app = mount();
    app.ui.scene.borrow_mut().resize(1500., 800.);
    app.frames(30);
    app.click("#shell-sidebar >> role:button \"Fix the build\"");
    app.frames(20);
    app.click("#titlebar >> role:button \"Panel\"");
    app.frames(60);
    let side = app.bounds("#shell-sidebar").width - SIDEBAR_EDGE;
    let split = app.bounds("#shell-files").width;
    assert!((split - share_of(1500., side)).abs() < 0.5, "it opens at its share: {split}");
    drag(&mut app, "\"Resize files\"", 140.);
    app.frames(60);
    let dragged = app.bounds("#shell-files").width;
    assert!((dragged - split).abs() > 60., "the drag moved it: {split} → {dragged}");
    app.ui.scene.borrow_mut().resize(1400., 800.);
    app.frames(30);
    let held = app.bounds("#shell-files").width;
    assert!((held - dragged).abs() < 0.5, "the window resized, it holds where it was dragged: {held}, was {dragged}");
    for _ in 0..2 {
        app.click("#titlebar >> role:button \"Panel\"");
        app.frames(60);
        app.ui.input.forget_clicks();
        app.click("#titlebar >> role:button \"Panel\"");
        // (Mid-way, its edge pressed and dragged: nothing.)
        app.frames(4);
        drag(&mut app, "\"Resize files\"", 120.);
        app.frames(60);
        let reopened = app.bounds("#shell-files").width;
        let share = share_of(1400., side);
        assert!((reopened - share).abs() < 0.5, "reopened at {reopened}, its share is {share} (dragged to {dragged})");
    }
}

/// Where a seam takes the pointer: its handle `label`, `by` points from its
/// middle.
fn seam_at(app: &mut TestApp, label: &str, by: f32) -> (f32, f32) {
    let b = app.bounds(&format!("\"{label}\""));
    (b.x + b.width / 2. + by, b.y + b.height / 2.)
}

fn double_click(app: &mut TestApp, (x, y): (f32, f32)) {
    use zgx::input::{InputEvent, Modifiers, PointerButton};
    let (button, m) = (PointerButton::Primary, Modifiers::default());
    app.ui.input.forget_clicks();
    app.ui.dispatch_with_modifiers(InputEvent::PointerMove { x, y }, m);
    for _ in 0..2 {
        app.ui.dispatch_with_modifiers(InputEvent::PointerDown { x, y, button }, m);
        app.ui.dispatch_with_modifiers(InputEvent::PointerUp { x, y, button }, m);
        app.frame();
    }
    app.ui.input.forget_clicks();
}

/// Double-clicking a seam puts it back, easing there as a toggle does: the
/// sidebar's edge to its default width, the chat and the panel splitting
/// again with it, and the panel's to its share (either forgetting a width
/// dragged), as Zeron's `reset_panel_widths`. The panel's seam takes the pointer 10 points
/// either side of its middle, under the top bar; the sidebar's, 10 inside
/// its gap's middle to the gap's far side (on its own side: the top bar's
/// first tabs reach over its head).
#[test]
fn double_clicking_a_seam_puts_it_back() {
    let mut app = mount();
    app.ui.scene.borrow_mut().resize(1500., 800.);
    app.frames(30);
    app.click("#shell-sidebar >> role:button \"Fix the build\"");
    app.frames(20);
    app.click("#titlebar >> role:button \"Panel\"");
    app.frames(60);
    for label in ["Resize sidebar", "Resize files"] {
        let b = app.bounds(&format!("\"{label}\""));
        if label == "Resize files" {
            assert!((b.width - 20.).abs() < 0.5, "{label} takes the pointer 10 either side: {}", b.width);
        } else {
            assert!(b.width > 10. && b.width < 20., "{label} takes the pointer on its own side: {}", b.width);
        }
        assert!(b.y >= 42. - 0.5, "{label} starts under the top bar: {}", b.y);
    }
    let sidebar = |app: &mut TestApp| app.bounds("#shell-sidebar").width - SIDEBAR_EDGE;
    let panel = |app: &mut TestApp| app.bounds("#shell-files").width;
    let share = share_of(1500., SIDEBAR_DEFAULT);
    // (The panel's width chosen first: the sidebar's reset forgets it too.)
    drag(&mut app, "\"Resize files\"", 100.);
    app.frames(30);
    assert!(panel(&mut app) < share - 60., "dragged to {}", panel(&mut app));
    drag(&mut app, "\"Resize sidebar\"", 90.);
    app.frames(30);
    assert!((sidebar(&mut app) - (SIDEBAR_DEFAULT + 90.)).abs() < 0.5, "dragged to {}", sidebar(&mut app));
    // (Near the reach's end, in the sidebar's side.)
    let b = app.bounds("\"Resize sidebar\"");
    double_click(&mut app, (b.x + 1., b.y + b.height / 2.));
    app.frames(3);
    let mid = sidebar(&mut app);
    assert!(mid > SIDEBAR_DEFAULT + 0.5 && mid < SIDEBAR_DEFAULT + 89.5, "it eases back: {mid} on the way");
    app.frames(60);
    assert!((sidebar(&mut app) - SIDEBAR_DEFAULT).abs() < 0.5, "back to its default: {}", sidebar(&mut app));
    assert!((panel(&mut app) - share).abs() < 0.5, "the panel back to its share {share}: {}", panel(&mut app));
    drag(&mut app, "\"Resize files\"", -150.);
    app.frames(30);
    let dragged = panel(&mut app);
    assert!(dragged > share + 100., "dragged to {dragged}");
    let at = seam_at(&mut app, "Resize files", -9.);
    double_click(&mut app, at);
    app.frames(3);
    let mid = panel(&mut app);
    assert!(mid > share + 0.5 && mid < dragged - 0.5, "it eases back: {mid} on the way");
    app.frames(60);
    assert!((panel(&mut app) - share).abs() < 0.5, "back to its share {share}: {}", panel(&mut app));
    // (Forgotten: the window resized, it keeps to its share.)
    app.ui.scene.borrow_mut().resize(1400., 800.);
    app.frames(30);
    assert!((panel(&mut app) - share_of(1400., SIDEBAR_DEFAULT)).abs() < 0.5, "it keeps to its share: {}", panel(&mut app));
}

/// The light in a seam's gap, `label`'s: its opacity. (Inside the seam, or,
/// the sidebar's, just before it.)
fn seam_light(app: &mut TestApp, label: &str) -> f32 {
    let id = app.find(&format!("\"{label}\"")).expect("the seam");
    app.ui.prepare_frame();
    let scene = app.ui.scene.borrow();
    let light = scene.children(id).first().copied().unwrap_or_else(|| {
        let siblings = scene.children(scene.parent(id).expect("its parent"));
        let at = siblings.iter().position(|&n| n == id).expect("itself");
        siblings[at - 1]
    });
    scene.effects(light).opacity
}

/// A seam dragged past its limit bounces: what it moves goes a few points
/// further (at most 5) and comes back, once, as Zeron's resize-edge bounce;
/// its light, which fades in under the pointer, dims while the drag is held
/// there, and comes back as it's dragged inside again. The panel's edge past
/// its least and the sidebar's past its most.
#[test]
fn a_seam_dragged_past_its_limit_bounces() {
    use zgx::input::{InputEvent, Modifiers, PointerButton};
    let mut app = mount();
    app.ui.scene.borrow_mut().resize(1500., 800.);
    app.frames(30);
    app.click("#shell-sidebar >> role:button \"Fix the build\"");
    app.frames(20);
    app.click("#titlebar >> role:button \"Panel\"");
    app.frames(60);
    let (button, m) = (PointerButton::Primary, Modifiers::default());
    // (`label`'s seam dragged `by`, then held there: each frame's width of
    // `what`, and its light then; dragged back `back`, its light again.)
    let mut walk = |app: &mut TestApp, label: &str, what: &dyn Fn(&mut TestApp) -> f32, by: f32, back: f32| {
        let (x, y) = seam_at(app, label, 0.);
        app.ui.input.forget_clicks();
        app.ui.dispatch_with_modifiers(InputEvent::PointerMove { x, y }, m);
        app.frames(3);
        let fading = seam_light(app, label);
        assert!(fading > 0.02 && fading < 0.98, "{label}'s light fades in: {fading}");
        app.frames(12);
        assert!(seam_light(app, label) > 0.98, "{label}'s light, under the pointer");
        app.ui.dispatch_with_modifiers(InputEvent::PointerDown { x, y, button }, m);
        let mut seen = Vec::new();
        for step in 1..=4 {
            app.ui.dispatch_with_modifiers(InputEvent::PointerMove { x: x + by * step as f32 / 4., y }, m);
            app.frame();
            seen.push(what(app));
        }
        for _ in 0..20 {
            app.frame();
            seen.push(what(app));
        }
        let held = seam_light(app, label);
        app.ui.dispatch_with_modifiers(InputEvent::PointerMove { x: x + by + back, y }, m);
        app.frames(15);
        let inside = seam_light(app, label);
        app.ui.dispatch_with_modifiers(InputEvent::PointerUp { x: x + by + back, y, button }, m);
        app.ui.input.forget_clicks();
        app.frames(30);
        (seen, held, inside)
    };
    let panel = |app: &mut TestApp| app.bounds("#shell-files").width;
    let (seen, held, inside) = walk(&mut app, "Resize files", &panel, 600., -400.);
    let least = seen.iter().copied().fold(f32::MAX, f32::min);
    let rest = *seen.last().unwrap();
    assert!((rest - RIGHT_PANE_MIN).abs() < 0.5, "held at its least: {rest}");
    assert!(least < RIGHT_PANE_MIN - 2. && least >= RIGHT_PANE_MIN - 5.5, "it gave a few points past its least and came back: {seen:?}");
    assert!(held < 0.02, "its light dims, held at its least: {held}");
    assert!(inside > 0.98, "and comes back inside: {inside}");
    let sidebar = |app: &mut TestApp| app.bounds("#shell-sidebar").width - SIDEBAR_EDGE;
    let (seen, held, _) = walk(&mut app, "Resize sidebar", &sidebar, 300., -200.);
    let most = seen.iter().copied().fold(f32::MIN, f32::max);
    let rest = *seen.last().unwrap();
    assert!((rest - 400.).abs() < 0.5, "held at its most: {rest}");
    assert!(most > 402. && most <= 405.5, "it gave a few points past its most and came back: {seen:?}");
    assert!(held < 0.02, "its light dims, held at its most: {held}");
}
